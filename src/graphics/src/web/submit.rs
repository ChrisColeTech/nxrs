//! Submission on the web device: each command buffer's records are resolved into WebGPU-shaped packets: passes,
//! pipelines (made from the bound pipeline, the dynamic state at the draw and the pass's formats), bind groups (from
//! the bound descriptor sets), draws, dispatches, copies; the uploads the submission needs from host-visible memory
//! (by linear-memory address), the read-backs it owes host-visible memory, and the values to signal when done. One
//! ring packet (`op::SUBMIT_BEGIN`) carries the whole submission; objects it needs are created by packets before it.

use super::cmd::{Attachment, Cmd, Dyn};
use super::fmt;
use super::obj::{self, objs, op, Desc, Objs, Pipeline};
use super::ring::{self, Pk, Word};
use ash::vk;
use ash::vk::Handle;
use shaders::wgsl::{Bind, Kind, Wgsl};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

type R = vk::Result;

/// Objects made once: bind group layouts, pipeline layouts, pipelines and bind groups by content; stand-ins for
/// unbound descriptors.
#[derive(Default)]
struct Cache {
    /// by the hash of their entries (`LayEnt`)
    bgl: HashMap<u64, u32>,
    playout: HashMap<[u32; 4], u32>,
    rpipe: HashMap<u64, u32>,
    cpipe: HashMap<u64, u32>,
    bg: HashMap<u64, u32>,
    /// pipelines by Vulkan handle -> keys made from it (forgotten with it)
    by_pipe: HashMap<u64, Vec<u64>>,
    empty_bgl: u32,
    empty_bg: u32,
    g3_bgl: u32,
    g3_bg: u32,
    dummy_buf: u32,
    dummy_tex: HashMap<(&'static str, &'static str, bool), u32>,
    dummy_sampler: [u32; 2],
    nearest: HashMap<u32, u32>,
}

/// A bind group layout entry (binding, visibility, then the kind's facts), hashed for the layout cache; JSON only
/// when a new layout is made.
#[derive(Hash)]
enum LayEnt {
    Buffer(u32, u32, &'static str),
    /// sample type, view dimension, multisampled
    Texture(u32, u32, &'static str, &'static str, bool),
    /// access, format, view dimension
    StorageTexture(u32, u32, &'static str, &'static str, &'static str),
    Sampler(u32, u32, &'static str),
}

impl LayEnt {
    fn json(&self) -> serde_json::Value {
        match *self {
            LayEnt::Buffer(b, v, ty) => serde_json::json!({ "binding": b, "visibility": v, "buffer": { "type": ty } }),
            LayEnt::Texture(b, v, st, dim, multi) => serde_json::json!({ "binding": b, "visibility": v,
                "texture": { "sampleType": st, "viewDimension": dim, "multisampled": multi } }),
            LayEnt::StorageTexture(b, v, access, format, dim) => serde_json::json!({ "binding": b, "visibility": v,
                "storageTexture": { "access": access, "format": format, "viewDimension": dim } }),
            LayEnt::Sampler(b, v, ty) => serde_json::json!({ "binding": b, "visibility": v, "sampler": { "type": ty } }),
        }
    }
}

/// A bind group entry (binding, then the object id and for a buffer its range), hashed for the group cache.
#[derive(Hash)]
enum GrpEnt {
    Buffer(u32, u32, u64, u64),
    View(u32, u32),
    Sampler(u32, u32),
}

impl GrpEnt {
    fn json(&self) -> serde_json::Value {
        match *self {
            GrpEnt::Buffer(b, id, off, size) => serde_json::json!({ "binding": b, "buffer": id, "offset": off, "size": size }),
            GrpEnt::View(b, id) => serde_json::json!({ "binding": b, "view": id }),
            GrpEnt::Sampler(b, id) => serde_json::json!({ "binding": b, "sampler": id }),
        }
    }
}

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache::default())).lock().unwrap()
}

const DUMMY_BUF_SIZE: u64 = 65536;

/// The fixed objects (at instance creation, after the worker is up).
pub fn init() {
    let mut c = cache();
    let mut p = Pk::default();
    p.u32(obj::UNIFORM_RING).u32(64 | 8 | 4).u64(obj::UNIFORM_RING_SIZE);
    ring::push(op::BUFFER, &p.0);
    c.empty_bgl = obj::new_id();
    obj::json(op::BGL, &serde_json::json!({ "id": c.empty_bgl, "entries": [] }));
    c.empty_bg = obj::new_id();
    obj::json(op::BINDGROUP, &serde_json::json!({ "id": c.empty_bg, "layout": c.empty_bgl, "entries": [] }));
    c.g3_bgl = obj::new_id();
    obj::json(
        op::BGL,
        &serde_json::json!({ "id": c.g3_bgl, "entries": [
            { "binding": 0, "visibility": 7, "buffer": { "type": "uniform", "hasDynamicOffset": true } },
            { "binding": 1, "visibility": 7, "buffer": { "type": "uniform", "hasDynamicOffset": true } },
        ]}),
    );
    c.g3_bg = obj::new_id();
    obj::json(
        op::BINDGROUP,
        &serde_json::json!({ "id": c.g3_bg, "layout": c.g3_bgl, "entries": [
            { "binding": 0, "buffer": obj::UNIFORM_RING, "offset": 0, "size": 128 },
            { "binding": 1, "buffer": obj::UNIFORM_RING, "offset": 0, "size": 16 },
        ]}),
    );
    c.dummy_buf = obj::gpu_buffer(
        DUMMY_BUF_SIZE,
        vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::VERTEX_BUFFER,
    );
    for (i, cmp) in [false, true].into_iter().enumerate() {
        let id = obj::new_id();
        let mut j = serde_json::json!({ "id": id, "magFilter": "nearest", "minFilter": "nearest" });
        if cmp {
            j["compare"] = "always".into();
        }
        obj::json(op::SAMPLER, &j);
        c.dummy_sampler[i] = id;
    }
}

/// A worker object is gone (a buffer, texture, view or sampler destroyed by the replay).
pub fn release(id: u32) {
    obj::destroy(id);
}

pub fn forget_pipeline(h: u64) {
    let mut c = cache();
    if let Some(keys) = c.by_pipe.remove(&h) {
        for k in keys {
            if let Some(id) = c.rpipe.remove(&k).or_else(|| c.cpipe.remove(&k)) {
                obj::destroy(id);
            }
        }
    }
}

/// Waits until the worker has finished everything submitted so far.
pub fn wait_idle() {
    let w = Word::new(0);
    let mut s = Pk::default();
    sub(&mut s, op::SIGNAL, |p| {
        p.u64(w.addr()).u64(1).u64(0);
    });
    ring::push(op::SUBMIT_BEGIN, &s.0);
    ring::wait_until(u64::MAX, || w.get() != 0);
    // the word may be written after the wait returns only if the worker stopped: keep it
    std::mem::forget(w);
}

fn sub(s: &mut Pk, op: u32, f: impl FnOnce(&mut Pk)) {
    let at = s.0.len();
    s.u32(op).u32(0);
    f(s);
    while s.0.len() % 4 != 0 {
        s.0.push(0);
    }
    let len = (s.0.len() - at - 8) as u32;
    s.0[at + 4..at + 8].copy_from_slice(&len.to_le_bytes());
}

fn red_once(m: &str) {
    crate::log::red_once(&format!("web: {m}"));
}

struct Pass {
    colors: Vec<Option<(vk::Format, bool)>>,
    depth: Option<vk::Format>,
    samples: u32,
    extent: (u32, u32),
    /// no attachment at all: WebGPU has no such pass; nothing is emitted and its draws are refused
    empty: bool,
}

#[derive(Default, Clone, PartialEq)]
struct Bound {
    pipe: u32,
    groups: [u32; 4],
    g3: (u32, u32),
    vb: Vec<(u32, u32, u64, u64)>,
    ib: (u32, u32, u64),
    viewport: [u32; 6],
    scissor: [u32; 4],
    blend: [u32; 4],
    stencil_ref: u32,
}

/// One submission being resolved.
struct Rs<'a> {
    o: &'a Objs,
    s: Pk,
    uploads: Vec<(u32, u64, u64, u64)>,
    readbacks: Vec<(u32, u64, u64, u64)>,
    uniforms: Vec<u8>,
    pass: Option<Pass>,
    compute: bool,
    gfx: Option<Arc<Pipeline>>,
    gfx_h: u64,
    cmp: Option<Arc<Pipeline>>,
    cmp_h: u64,
    sets: [[u64; 8]; 2],
    push: [u8; 128],
    dynst: Dyn,
    vbufs: HashMap<u32, (u64, u64, u64)>,
    ibuf: Option<(u64, u64, vk::IndexType)>,
    bound: Bound,
    last_uni: Option<([u8; 128], [u8; 16], u32, u32)>,
    /// bind groups dropped from the cache while this submission was resolved: destroyed only after it is in the ring
    /// (it may have bound them already; destroyed before it, its draws found no group: WEB-WPAGE)
    retired: Vec<u32>,
}

fn h64(f: impl FnOnce(&mut std::collections::hash_map::DefaultHasher)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f(&mut h);
    h.finish()
}

impl<'a> Rs<'a> {
    fn new(o: &'a Objs) -> Rs<'a> {
        Rs {
            o,
            s: Pk::default(),
            uploads: Vec::new(),
            readbacks: Vec::new(),
            retired: Vec::new(),
            uniforms: Vec::new(),
            pass: None,
            compute: false,
            gfx: None,
            gfx_h: 0,
            cmp: None,
            cmp_h: 0,
            sets: [[0; 8]; 2],
            push: [0; 128],
            dynst: Dyn::default(),
            vbufs: HashMap::new(),
            ibuf: None,
            bound: Bound::default(),
            last_uni: None,
        }
    }

    fn put(&mut self, op: u32, f: impl FnOnce(&mut Pk)) {
        sub(&mut self.s, op, f);
    }

    fn end_compute(&mut self) {
        if self.compute {
            self.put(op::COMPUTE_END, |_| {});
            self.compute = false;
            self.bound = Bound::default();
        }
    }

    /// A buffer's worker id, and when it is host-visible, the upload of [off, off + n) it needs first.
    fn buf_read(&mut self, h: u64, off: u64, n: u64) -> Option<(u32, u64)> {
        let b = self.o.buf.get(&h)?;
        let n = if n == vk::WHOLE_SIZE { b.size.saturating_sub(off) } else { n.min(b.size.saturating_sub(off)) };
        if !b.host.is_null() && n > 0 {
            self.uploads.push((b.id, off, b.host as u64 + off, n));
        }
        Some((b.id, n))
    }

    /// As `buf_read`, for a range the GPU writes: a host-visible buffer gets it back after the submission.
    fn buf_write(&mut self, h: u64, off: u64, n: u64) -> Option<(u32, u64)> {
        let b = self.o.buf.get(&h)?;
        let n = if n == vk::WHOLE_SIZE { b.size.saturating_sub(off) } else { n.min(b.size.saturating_sub(off)) };
        if !b.host.is_null() && n > 0 {
            self.readbacks.push((b.id, off, n, b.host as u64 + off));
        }
        Some((b.id, n))
    }

    fn run(&mut self, cmds: &[Cmd]) {
        for c in cmds {
            match c {
                Cmd::BeginRendering { colors, depth, stencil, .. } => {
                    self.end_compute();
                    self.begin_pass(colors, depth.as_ref().or(stencil.as_ref()), depth.is_some(), stencil.is_some());
                }
                Cmd::EndRendering => {
                    if self.pass.take().is_some_and(|p| !p.empty) {
                        self.put(op::PASS_END, |_| {});
                    }
                    self.bound = Bound::default();
                }
                Cmd::BindPipeline { compute, p } => {
                    let pl = self.o.pipe.get(p).cloned();
                    if *compute {
                        self.cmp = pl;
                        self.cmp_h = *p;
                    } else {
                        if let Some(Pipeline::Gfx(g)) = pl.as_deref() {
                            // static state replaces the command buffer's (Vulkan: binding a pipeline sets its static state)
                            apply_static(&mut self.dynst, g);
                        }
                        self.gfx = pl;
                        self.gfx_h = *p;
                    }
                }
                Cmd::BindSets { compute, first, sets, .. } => {
                    for (i, s) in sets.iter().enumerate() {
                        if let Some(slot) = self.sets[*compute as usize].get_mut(*first as usize + i) {
                            *slot = *s;
                        }
                    }
                }
                Cmd::Push { off, data } => {
                    let o = (*off as usize).min(128);
                    let n = data.len().min(128 - o);
                    self.push[o..o + n].copy_from_slice(&data[..n]);
                }
                Cmd::Set(s) => self.dynst.apply(s),
                Cmd::BindVb { first, bufs } => {
                    for (i, &(b, o, s, st)) in bufs.iter().enumerate() {
                        self.vbufs.insert(first + i as u32, (b, o, s));
                        if st != 0 {
                            let bind = first + i as u32;
                            if let Some(x) = self.dynst.vbind.iter_mut().find(|x| x.0 == bind) {
                                x.1 = st as u32;
                            }
                        }
                    }
                }
                Cmd::BindIb { buf, off, ty } => self.ibuf = Some((*buf, *off, *ty)),
                Cmd::Draw { vc, ic, fv, fi } => {
                    if self.draw_setup(false, *fi, 0) {
                        self.put(op::DRAW, |p| {
                            p.u32(*vc).u32(*ic).u32(*fv).u32(*fi);
                        });
                    }
                }
                Cmd::DrawIndexed { ic, inst, fi, vo, finst } => {
                    if self.draw_setup(true, *finst, *vo) {
                        self.put(op::DRAW_INDEXED, |p| {
                            p.u32(*ic).u32(*inst).u32(*fi).i32(*vo).u32(*finst);
                        });
                    }
                }
                Cmd::DrawIndirect { buf, off, count, stride, indexed } => {
                    for k in 0..*count {
                        let at = off + (k as u64) * (*stride as u64).max(if *indexed { 20 } else { 16 });
                        if !self.draw_setup(*indexed, 0, 0) {
                            break;
                        }
                        let Some((id, _)) = self.buf_read(*buf, at, if *indexed { 20 } else { 16 }) else { break };
                        self.put(op::DRAW_INDIRECT, |p| {
                            p.u32(id).u64(at).u32(*indexed as u32);
                        });
                    }
                }
                Cmd::Dispatch { x, y, z } => {
                    if self.dispatch_setup() {
                        self.put(op::DISPATCH, |p| {
                            p.u32(*x).u32(*y).u32(*z);
                        });
                    }
                }
                Cmd::DispatchIndirect { buf, off } => {
                    if self.dispatch_setup() {
                        if let Some((id, _)) = self.buf_read(*buf, *off, 12) {
                            self.put(op::DISPATCH_INDIRECT, |p| {
                                p.u32(id).u64(*off);
                            });
                        }
                    }
                }
                Cmd::CopyBuffer { src, dst, regions } => {
                    self.end_compute();
                    for r in regions {
                        let (Some((s, n)), Some((d, _))) =
                            (self.buf_read(*src, r.src_offset, r.size), self.buf_write(*dst, r.dst_offset, r.size))
                        else {
                            continue;
                        };
                        self.put(op::COPY_B2B, |p| {
                            p.u32(s).u64(r.src_offset).u32(d).u64(r.dst_offset).u64(n & !3);
                        });
                    }
                }
                Cmd::CopyBufferToImage { src, dst, regions } => {
                    self.end_compute();
                    self.copy_b2t(*src, *dst, regions);
                }
                Cmd::CopyImageToBuffer { src, dst, regions } => {
                    self.end_compute();
                    self.copy_t2b(*src, *dst, regions);
                }
                Cmd::CopyImage { src, dst, regions } => {
                    self.end_compute();
                    let (Some(si), Some(di)) = (self.o.img.get(src), self.o.img.get(dst)) else { continue };
                    let (sn, dn) = (si.tex.map_or("", |t| t.name), di.tex.map_or("", |t| t.name));
                    if sn.trim_end_matches("-srgb") != dn.trim_end_matches("-srgb") {
                        red_once(&format!("an image copy between {sn} and {dn} (WebGPU copies only between one format and its srgb twin): not copied"));
                        continue;
                    }
                    for r in regions {
                        let (s3, d3) = (si.ty == vk::ImageType::TYPE_3D, di.ty == vk::ImageType::TYPE_3D);
                        let depth = if s3 { r.extent.depth } else { r.src_subresource.layer_count };
                        self.put(op::COPY_T2T, |p| {
                            p.u32(si.id).u32(r.src_subresource.mip_level).u32(r.src_offset.x as u32).u32(r.src_offset.y as u32);
                            p.u32(if s3 { r.src_offset.z as u32 } else { r.src_subresource.base_array_layer });
                            p.u32(di.id).u32(r.dst_subresource.mip_level).u32(r.dst_offset.x as u32).u32(r.dst_offset.y as u32);
                            p.u32(if d3 { r.dst_offset.z as u32 } else { r.dst_subresource.base_array_layer });
                            p.u32(r.extent.width).u32(r.extent.height).u32(depth.max(1));
                            p.u32(aspect_code(r.src_subresource.aspect_mask, si));
                        });
                    }
                }
                Cmd::Blit { src, dst, regions, filter } => {
                    self.end_compute();
                    let (Some(si), Some(di)) = (self.o.img.get(src), self.o.img.get(dst)) else { continue };
                    for r in regions {
                        let [a, b] = r.src_offsets;
                        let [c, d] = r.dst_offsets;
                        self.put(op::BLIT, |p| {
                            p.u32(si.id).u32(r.src_subresource.mip_level).u32(r.src_subresource.base_array_layer);
                            p.i32(a.x).i32(a.y).i32(b.x).i32(b.y);
                            p.u32(di.id).u32(r.dst_subresource.mip_level).u32(r.dst_subresource.base_array_layer);
                            p.i32(c.x).i32(c.y).i32(d.x).i32(d.y);
                            p.u32((*filter == vk::Filter::LINEAR) as u32);
                        });
                    }
                }
                Cmd::ClearColor { image, color, ranges } => {
                    self.end_compute();
                    let Some(im) = self.o.img.get(image) else { continue };
                    // SAFETY: a clear colour is 16 bytes of any interpretation
                    let raw: [u32; 4] = unsafe { color.uint32 };
                    let kind = match im.tex.map_or("float", |t| t.sample) {
                        "sint" => 1,
                        "uint" => 2,
                        _ => 0,
                    };
                    for (mip, layer) in subresources(im, ranges) {
                        self.put(op::CLEAR_TEX, |p| {
                            p.u32(im.id).u32(mip).u32(layer).u32(kind);
                            for v in raw {
                                p.u32(v);
                            }
                        });
                    }
                }
                Cmd::ClearDepth { image, value, ranges } => {
                    self.end_compute();
                    let Some(im) = self.o.img.get(image) else { continue };
                    for (mip, layer) in subresources(im, ranges) {
                        self.put(op::CLEAR_TEX, |p| {
                            p.u32(im.id).u32(mip).u32(layer).u32(3).f32(value.depth).u32(value.stencil).u32(0).u32(0);
                        });
                    }
                }
                Cmd::ClearAttachments { atts, rects } => {
                    if self.pass.as_ref().is_none_or(|p| p.empty) {
                        continue;
                    }
                    for a in atts {
                        for r in rects {
                            let depth = a.aspect_mask.intersects(vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL);
                            // SAFETY: as above
                            let raw: [u32; 4] = unsafe { a.clear_value.color.uint32 };
                            self.put(op::CLEAR_RECT, |p| {
                                p.u32(if depth { 0xff } else { a.color_attachment });
                                p.i32(r.rect.offset.x).i32(r.rect.offset.y).u32(r.rect.extent.width).u32(r.rect.extent.height);
                                if depth {
                                    // SAFETY: as above
                                    let ds = unsafe { a.clear_value.depth_stencil };
                                    p.u32(a.aspect_mask.as_raw()).f32(ds.depth).u32(ds.stencil).u32(0);
                                } else {
                                    for v in raw {
                                        p.u32(v);
                                    }
                                }
                            });
                        }
                    }
                    self.bound = Bound::default();
                }
                Cmd::Fill { buf, off, size, value } => {
                    self.end_compute();
                    let Some((id, n)) = self.buf_write(*buf, *off, *size) else { continue };
                    let n = n & !3;
                    if *value == 0 {
                        self.put(op::CLEAR_BUFFER, |p| {
                            p.u32(id).u64(*off).u64(n);
                        });
                    } else {
                        self.put(op::FILL_BUFFER, |p| {
                            p.u32(id).u64(*off).u64(n).u32(*value);
                        });
                    }
                }
                Cmd::Update { buf, off, data } => {
                    self.end_compute();
                    let Some((id, _)) = self.buf_write(*buf, *off, data.len() as u64) else { continue };
                    self.put(op::UPLOAD, |p| {
                        // an inline upload: source address 0, the bytes follow
                        p.u32(id).u64(*off).u64(0).u64(data.len() as u64);
                        p.0.extend_from_slice(data);
                    });
                }
                Cmd::Unsupported(what) => red_once(&format!("{what}: not on the web device")),
            }
        }
    }

    fn begin_pass(&mut self, colors: &[Attachment], ds: Option<&Attachment>, has_depth: bool, has_stencil: bool) {
        let mut pass = Pass { colors: Vec::new(), depth: None, samples: 1, extent: (0, 0), empty: false };
        let view_info = |h: u64| -> Option<(u32, vk::Format, u32, (u32, u32), bool)> {
            let v = self.o.view.get(&h)?;
            let im = self.o.img.get(&v.image)?;
            let ext = ((im.extent.width >> v.base_mip).max(1), (im.extent.height >> v.base_mip).max(1));
            let int = matches!(im.tex.map_or("float", |t| t.sample), "sint" | "uint");
            Some((v.id, v.format, im.samples, ext, int))
        };
        let mut cols = Vec::new();
        for a in colors {
            match view_info(a.view) {
                Some((id, f, s, ext, int)) => {
                    pass.samples = s;
                    pass.extent = ext;
                    pass.colors.push(Some((f, int)));
                    let resolve = self.o.view.get(&a.resolve).map_or(0, |v| v.id);
                    cols.push((id, resolve, a, int));
                }
                None => {
                    pass.colors.push(None);
                    cols.push((0, 0, a, false));
                }
            }
        }
        let dsv = ds.and_then(|a| view_info(a.view).map(|i| (i, a)));
        if let Some(((_, f, s, ext, _), _)) = &dsv {
            pass.depth = Some(*f);
            pass.samples = *s;
            if pass.extent == (0, 0) {
                pass.extent = *ext;
            }
        }
        if dsv.is_none() && pass.colors.iter().all(|c| c.is_none()) {
            pass.empty = true;
            self.pass = Some(pass);
            self.bound = Bound::default();
            return;
        }
        self.put(op::PASS_BEGIN, |p| {
            p.u32(cols.len() as u32);
            for (id, resolve, a, int) in &cols {
                // SAFETY: clear values are 16 bytes of any interpretation
                let raw: [u32; 4] = unsafe { a.clear.color.uint32 };
                p.u32(*id).u32(*resolve).u32((a.load == vk::AttachmentLoadOp::CLEAR) as u32);
                p.u32((a.store == vk::AttachmentStoreOp::STORE || a.store == vk::AttachmentStoreOp::NONE) as u32);
                p.u32(*int as u32);
                for v in raw {
                    p.u32(v);
                }
            }
            match &dsv {
                Some(((id, f, ..), a)) => {
                    let t = fmt::tex(*f);
                    // SAFETY: as above
                    let c = unsafe { a.clear.depth_stencil };
                    p.u32(*id);
                    p.u32(t.is_some_and(|t| t.depth) as u32 * (1 + (has_depth && a.load == vk::AttachmentLoadOp::CLEAR) as u32));
                    p.u32(t.is_some_and(|t| t.stencil) as u32 * (1 + (has_stencil && a.load == vk::AttachmentLoadOp::CLEAR) as u32));
                    p.f32(c.depth).u32(c.stencil);
                }
                None => {
                    p.u32(0).u32(0).u32(0).f32(0.0).u32(0);
                }
            }
        });
        self.pass = Some(pass);
        self.bound = Bound::default();
    }

    /// The bind group layout entries of a pipeline's binds for group `g`, with the bound resources' sample types; and
    /// the resources to bind.
    fn group(&mut self, compute: bool, stages: &[(&Wgsl, u32)], g: u32) -> Option<(u32, u32)> {
        let mut binds: Vec<(Bind, u32)> = Vec::new();
        for (w, vis) in stages {
            for b in w.binds.iter().filter(|b| b.group == g) {
                match binds.iter_mut().find(|x| x.0.binding == b.binding) {
                    Some(x) => x.1 |= vis,
                    None => binds.push((b.clone(), *vis)),
                }
            }
        }
        if binds.is_empty() {
            return None;
        }
        binds.sort_by_key(|b| b.0.binding);
        let mut layout: Vec<LayEnt> = Vec::new();
        let mut entries: Vec<GrpEnt> = Vec::new();
        // first pass: textures decide whether their paired samplers must be non-filtering
        let mut unfilterable: Vec<u32> = Vec::new();
        for (b, vis) in &binds {
            let set = self.sets[compute as usize].get(b.set as usize).copied().unwrap_or(0);
            let desc = self.o.dset.get(&set).and_then(|s| s.d.get(&(b.src_binding, 0)).copied()).unwrap_or(Desc::None);
            match &b.kind {
                Kind::Uniform { .. } | Kind::Storage { .. } => {
                    let uniform = matches!(b.kind, Kind::Uniform { .. });
                    let need = if let Kind::Uniform { min } = b.kind { min as u64 } else { 4 };
                    let ro = matches!(b.kind, Kind::Storage { read_only: true }) || uniform;
                    let ty = if uniform {
                        "uniform"
                    } else if ro {
                        "read-only-storage"
                    } else {
                        "storage"
                    };
                    layout.push(LayEnt::Buffer(b.binding, *vis, ty));
                    let (id, off, size) = match desc {
                        Desc::Buf { buf, off, range } => {
                            let bsize = self.o.buf.get(&buf).map_or(0, |x| x.size);
                            let mut n = if range == vk::WHOLE_SIZE { bsize.saturating_sub(off) } else { range };
                            // WebGPU checks a binding against the shader's whole declared size (a constant bank is
                            // 64 KiB); Vulkan reads past a short range: bind up to what the shader declares
                            if n < need {
                                n = need.min(bsize.saturating_sub(off));
                            }
                            if uniform {
                                n = n.min(65536);
                            }
                            n &= !3;
                            if n < need {
                                red_once(&format!("a {ty} binding of {n} bytes where the shader declares {need}: a zero stand-in"));
                                (0, 0, 0)
                            } else {
                                let r = if ro { self.buf_read(buf, off, n) } else { self.buf_write(buf, off, n) };
                                if !ro {
                                    self.buf_read(buf, off, n);
                                }
                                match r {
                                    Some((id, _)) => (id, off, n),
                                    None => (0, 0, 0),
                                }
                            }
                        }
                        _ => (0, 0, 0),
                    };
                    let (id, off, size) = if id == 0 {
                        (cache().dummy_buf, 0, need.clamp(16, DUMMY_BUF_SIZE))
                    } else {
                        (id, off, size)
                    };
                    entries.push(GrpEnt::Buffer(b.binding, id, off, size));
                }
                Kind::Texture { sample, dim, multi } => {
                    let mut view = match desc {
                        Desc::Img { view, .. } => self.o.view.get(&view),
                        _ => None,
                    };
                    // a view WebGPU cannot bind where the shader's texture type is (a colour stand-in where a
                    // shadow lookup reads depth, an integer texture where floats are read): a zero stand-in
                    if let Some(t) = view.and_then(|v| fmt::tex(v.format)) {
                        let fits = match *sample {
                            "depth" => t.depth,
                            "float" => matches!(t.sample, "float" | "unfilterable-float" | "depth"),
                            s => t.sample == s,
                        };
                        if !fits {
                            red_once(&format!("a {} texture bound where the shader reads {sample}: a stand-in", t.name));
                            view = None;
                        }
                    }
                    let st = match view.and_then(|v| fmt::tex(v.format)) {
                        Some(t) if *sample == "float" && !t.filter => "unfilterable-float",
                        Some(t) if *sample == "float" && t.depth && !t.stencil => "unfilterable-float",
                        _ => *sample,
                    };
                    if st == "unfilterable-float" {
                        unfilterable.push(b.src_binding);
                    }
                    layout.push(LayEnt::Texture(b.binding, *vis, st, *dim, *multi));
                    let id = match view {
                        Some(v) => v.id,
                        None => dummy_texture(dim, st, *multi),
                    };
                    entries.push(GrpEnt::View(b.binding, id));
                }
                Kind::StorageTexture { format, access, dim } => {
                    layout.push(LayEnt::StorageTexture(b.binding, *vis, *access, *format, *dim));
                    let id = match desc {
                        Desc::Img { view, .. } => self.o.view.get(&view).map_or(0, |v| v.id),
                        _ => 0,
                    };
                    if id == 0 {
                        red_once("an unbound storage image: the draw is refused");
                        return Some((0, 0));
                    }
                    entries.push(GrpEnt::View(b.binding, id));
                }
                Kind::Sampler { .. } => {}
            }
        }
        for (b, vis) in &binds {
            let Kind::Sampler { comparison } = b.kind else { continue };
            let set = self.sets[compute as usize].get(b.set as usize).copied().unwrap_or(0);
            let desc = self.o.dset.get(&set).and_then(|s| s.d.get(&(b.src_binding, 0)).copied()).unwrap_or(Desc::None);
            let nonfilter = unfilterable.contains(&b.src_binding);
            let ty = if comparison {
                "comparison"
            } else if nonfilter {
                "non-filtering"
            } else {
                "filtering"
            };
            layout.push(LayEnt::Sampler(b.binding, *vis, ty));
            let s = match desc {
                Desc::Img { sampler, .. } | Desc::Sampler(sampler) => self.o.sampler.get(&sampler).copied(),
                _ => None,
            };
            let id = match s {
                Some(id) if nonfilter => nearest_twin(id),
                Some(id) => id,
                None => cache().dummy_sampler[comparison as usize],
            };
            entries.push(GrpEnt::Sampler(b.binding, id));
        }
        // WEB-W8: the caches are keyed by the entries' plain values; JSON is written only for a layout or group not
        // seen before (building and printing JSON for every draw's groups was most of the web replay's "sets" time)
        let lkey = h64(|h| layout.hash(h));
        let mut c = cache();
        let bgl = match c.bgl.get(&lkey) {
            Some(&id) => id,
            None => {
                let id = obj::new_id();
                let lj = serde_json::Value::Array(layout.iter().map(LayEnt::json).collect()).to_string();
                ring::push(op::BGL, format!("{{\"id\":{id},\"entries\":{lj}}}").as_bytes());
                c.bgl.insert(lkey, id);
                id
            }
        };
        let k = h64(|h| {
            bgl.hash(h);
            entries.hash(h);
        });
        let bg = match c.bg.get(&k) {
            Some(&id) => id,
            None => {
                if c.bg.len() > 50_000 {
                    self.retired.extend(c.bg.drain().map(|(_, id)| id));
                }
                let id = obj::new_id();
                let ekey = serde_json::Value::Array(entries.iter().map(GrpEnt::json).collect()).to_string();
                ring::push(op::BINDGROUP, format!("{{\"id\":{id},\"layout\":{bgl},\"entries\":{ekey}}}").as_bytes());
                c.bg.insert(k, id);
                id
            }
        };
        Some((bgl, bg))
    }

    /// Bind group layouts and groups 0-2 of a pipeline's stages; None: refused (red line given).
    fn groups(&mut self, compute: bool, stages: &[(&Wgsl, u32)]) -> Option<([u32; 3], [u32; 3])> {
        let (eb, eg) = {
            let c = cache();
            (c.empty_bgl, c.empty_bg)
        };
        let mut bgl = [eb; 3];
        let mut bg = [eg; 3];
        for g in 0..3 {
            if let Some((l, b)) = self.group(compute, stages, g) {
                if l == 0 {
                    return None;
                }
                bgl[g as usize] = l;
                bg[g as usize] = b;
            }
        }
        Some((bgl, bg))
    }

    fn playout(&mut self, bgl: [u32; 3]) -> u32 {
        let mut c = cache();
        let key = [bgl[0], bgl[1], bgl[2], c.g3_bgl];
        if let Some(&id) = c.playout.get(&key) {
            return id;
        }
        let id = obj::new_id();
        obj::json(op::PLAYOUT, &serde_json::json!({ "id": id, "bgls": key }));
        c.playout.insert(key, id);
        id
    }

    /// The per-draw uniforms (push constants, draw words) in this submission's uniform block; their offsets.
    fn uniforms(&mut self, draw: [u8; 16]) -> (u32, u32) {
        if let Some((p, d, po, dof)) = &self.last_uni {
            if *p == self.push && *d == draw {
                return (*po, *dof);
            }
        }
        let po = self.uniforms.len() as u32;
        self.uniforms.extend_from_slice(&self.push);
        self.uniforms.resize(po as usize + 256, 0);
        let dof = self.uniforms.len() as u32;
        self.uniforms.extend_from_slice(&draw);
        self.uniforms.resize(dof as usize + 256, 0);
        self.last_uni = Some((self.push, draw, po, dof));
        (po, dof)
    }

    fn set_groups(&mut self, bg: [u32; 3], uni: (u32, u32)) {
        let g3 = cache().g3_bg;
        for (i, &b) in bg.iter().enumerate() {
            if self.bound.groups[i] != b {
                self.bound.groups[i] = b;
                self.put(op::SET_BIND_GROUP, |p| {
                    p.u32(i as u32).u32(b).u32(0);
                });
            }
        }
        if self.bound.groups[3] != g3 || self.bound.g3 != uni {
            self.bound.groups[3] = g3;
            self.bound.g3 = uni;
            self.put(op::SET_BIND_GROUP, |p| {
                p.u32(3).u32(g3).u32(2).u32(uni.0).u32(uni.1);
            });
        }
    }

    fn dispatch_setup(&mut self) -> bool {
        if self.pass.is_some() {
            red_once("a dispatch inside a render pass");
            return false;
        }
        let cs = match self.cmp.as_deref() {
            Some(Pipeline::Compute { cs }) => cs.clone(),
            _ => {
                red_once("a dispatch without a compute pipeline");
                return false;
            }
        };
        if !self.compute {
            self.put(op::COMPUTE_BEGIN, |_| {});
            self.compute = true;
            self.bound = Bound::default();
        }
        let Some((bgl, bg)) = self.groups(true, &[(&cs.0, 4)]) else { return false };
        let layout = self.playout(bgl);
        let key = h64(|h| {
            self.cmp_h.hash(h);
            layout.hash(h);
        });
        let id = {
            let mut c = cache();
            match c.cpipe.get(&key) {
                Some(&id) => id,
                None => {
                    let id = obj::new_id();
                    obj::json(op::CPIPE, &serde_json::json!({ "id": id, "layout": layout, "module": cs.1, "entry": cs.0.entry }));
                    c.cpipe.insert(key, id);
                    c.by_pipe.entry(self.cmp_h).or_default().push(key);
                    id
                }
            }
        };
        if self.bound.pipe != id {
            self.bound.pipe = id;
            self.put(op::SET_PIPELINE, |p| {
                p.u32(id);
            });
        }
        let uni = self.uniforms([0; 16]);
        self.set_groups(bg, uni);
        true
    }

    fn draw_setup(&mut self, indexed: bool, first_instance: u32, base_vertex: i32) -> bool {
        let Some(pass) = &self.pass else {
            red_once("a draw outside a render pass");
            return false;
        };
        if pass.empty {
            red_once("a draw in a pass with no attachment (WebGPU has no such pass): refused");
            return false;
        }
        let Some(Pipeline::Gfx(g)) = self.gfx.as_deref() else {
            red_once("a draw without a graphics pipeline");
            return false;
        };
        let g = g.clone();
        let d = self.dynst.clone();
        let topo = match d.topology {
            vk::PrimitiveTopology::POINT_LIST => "point-list",
            vk::PrimitiveTopology::LINE_LIST => "line-list",
            vk::PrimitiveTopology::LINE_STRIP => "line-strip",
            vk::PrimitiveTopology::TRIANGLE_LIST => "triangle-list",
            vk::PrimitiveTopology::TRIANGLE_STRIP => "triangle-strip",
            t => {
                red_once(&format!("topology {t:?} (WebGPU has none): the draw is refused"));
                return false;
            }
        };
        let tri = topo.starts_with("triangle");
        if d.discard || (tri && d.cull == vk::CullModeFlags::FRONT_AND_BACK) {
            // nothing is rasterized: the same result as the draw on Vulkan (vertex stages cannot write on WebGPU)
            return false;
        }
        let pass_colors = pass.colors.clone();
        let pass_depth = pass.depth;
        let samples = if pass.samples > 1 { 4 } else { 1 };
        let extent = pass.extent;
        let mut stages: Vec<(&Wgsl, u32)> = vec![(&g.vs.0, 1)];
        if let Some(fs) = &g.fs {
            stages.push((&fs.0, 2));
        }
        let Some((bgl, bg)) = self.groups(false, &stages) else { return false };
        let layout = self.playout(bgl);
        // vertex buffers: one WebGPU slot per Vulkan binding the shader's inputs read; inputs no attribute names read
        // a zero stand-in (WebGPU requires every input be fed)
        let mut slots: Vec<u32> = Vec::new();
        let mut vbl: Vec<serde_json::Value> = Vec::new();
        let mut missing = Vec::new();
        let mut per_slot: HashMap<u32, Vec<serde_json::Value>> = HashMap::new();
        for &(loc, kind, n) in &g.vs.0.inputs {
            match d.vattr.iter().find(|a| a.0 == loc) {
                Some(&(_, binding, f, off)) => {
                    let Some(vf) = fmt::vertex(f) else {
                        red_once(&format!("vertex format {f:?} (not a WebGPU vertex format): the draw is refused"));
                        return false;
                    };
                    if !slots.contains(&binding) {
                        slots.push(binding);
                    }
                    per_slot.entry(binding).or_default().push(serde_json::json!({ "shaderLocation": loc, "format": vf, "offset": off }));
                }
                None => missing.push((loc, kind, n)),
            }
        }
        slots.sort();
        for b in &slots {
            let (stride, inst) = d.vbind.iter().find(|x| x.0 == *b).map_or((0, false), |x| (x.1, x.2));
            if stride % 4 != 0 {
                red_once(&format!("a vertex stride of {stride} (WebGPU: a multiple of 4): the draw is refused"));
                return false;
            }
            vbl.push(serde_json::json!({ "arrayStride": stride, "stepMode": if inst { "instance" } else { "vertex" },
                "attributes": per_slot.remove(b).unwrap_or_default() }));
        }
        if !missing.is_empty() {
            let attrs: Vec<serde_json::Value> = missing
                .iter()
                .map(|&(loc, k, _)| {
                    let f = match k {
                        'u' => "uint32x4",
                        'i' => "sint32x4",
                        _ => "float32x4",
                    };
                    serde_json::json!({ "shaderLocation": loc, "format": f, "offset": 0 })
                })
                .collect();
            vbl.push(serde_json::json!({ "arrayStride": 0, "stepMode": "instance", "attributes": attrs }));
        }
        if vbl.len() > 8 {
            red_once("more than 8 vertex buffers: the draw is refused");
            return false;
        }
        let strip_fmt = if indexed && topo.ends_with("strip") {
            match self.ibuf.map(|b| b.2) {
                Some(vk::IndexType::UINT32) => "uint32",
                _ => "uint16",
            }
        } else {
            ""
        };
        if d.logic_op {
            red_once("a logic op (WebGPU has none): drawn without it");
        }
        if d.polygon != vk::PolygonMode::FILL {
            red_once("line / point polygon mode (WebGPU has none): drawn filled");
        }
        let fs_outs: Vec<u32> = g.fs.as_ref().map_or(Vec::new(), |f| f.0.outputs.clone());
        let targets: Vec<serde_json::Value> = pass_colors
            .iter()
            .enumerate()
            .map(|(i, c)| match c {
                None => serde_json::Value::Null,
                Some((f, int)) => {
                    let t = fmt::tex(*f);
                    let mask = if fs_outs.contains(&(i as u32)) { d.mask[i.min(7)].as_raw() } else { 0 };
                    let mut j = serde_json::json!({ "format": t.map_or("rgba8unorm", |t| t.name), "writeMask": mask });
                    if d.blend[i.min(7)] && !int && t.is_some_and(|t| t.blend) {
                        let e = d.eq[i.min(7)];
                        j["blend"] = serde_json::json!({
                            "color": { "operation": blend_op(e.color_blend_op), "srcFactor": factor(e.src_color_blend_factor), "dstFactor": factor(e.dst_color_blend_factor) },
                            "alpha": { "operation": blend_op(e.alpha_blend_op), "srcFactor": factor(e.src_alpha_blend_factor), "dstFactor": factor(e.dst_alpha_blend_factor) },
                        });
                        if matches!(e.color_blend_op, vk::BlendOp::MIN | vk::BlendOp::MAX) {
                            j["blend"]["color"]["srcFactor"] = "one".into();
                            j["blend"]["color"]["dstFactor"] = "one".into();
                        }
                        if matches!(e.alpha_blend_op, vk::BlendOp::MIN | vk::BlendOp::MAX) {
                            j["blend"]["alpha"]["srcFactor"] = "one".into();
                            j["blend"]["alpha"]["dstFactor"] = "one".into();
                        }
                    }
                    j
                }
            })
            .collect();
        let mut desc = serde_json::json!({
            "layout": layout,
            "vertex": { "module": g.vs.1, "entryPoint": g.vs.0.entry, "buffers": vbl },
            "primitive": {
                "topology": topo,
                "cullMode": if tri { match d.cull { vk::CullModeFlags::FRONT => "front", vk::CullModeFlags::BACK => "back", _ => "none" } } else { "none" },
                "frontFace": if d.front == vk::FrontFace::CLOCKWISE { "cw" } else { "ccw" },
            },
            "multisample": { "count": samples, "alphaToCoverageEnabled": d.a2c && samples > 1 },
        });
        if !strip_fmt.is_empty() {
            desc["primitive"]["stripIndexFormat"] = strip_fmt.into();
        }
        if d.depth_clamp {
            desc["primitive"]["unclippedDepth"] = true.into();
        }
        if let Some(fs) = &g.fs {
            desc["fragment"] = serde_json::json!({ "module": fs.1, "entryPoint": fs.0.entry, "targets": targets });
        }
        if let Some(df) = pass_depth {
            let t = fmt::tex(df);
            let has_s = t.is_some_and(|t| t.stencil);
            let has_d = t.is_some_and(|t| t.depth);
            let face = |i: usize| {
                let (fail, pass, dfail, cmp) = d.stencil_ops[i];
                if d.stencil_test && has_s {
                    serde_json::json!({ "compare": obj::compare(cmp), "failOp": stencil(fail), "depthFailOp": stencil(dfail), "passOp": stencil(pass) })
                } else {
                    serde_json::json!({ "compare": "always", "failOp": "keep", "depthFailOp": "keep", "passOp": "keep" })
                }
            };
            let mut ds = serde_json::json!({
                "format": t.map_or("depth32float", |t| t.name),
                "stencilFront": face(0), "stencilBack": face(1),
                "stencilReadMask": d.stencil_read[0] & 0xff, "stencilWriteMask": if d.stencil_test { d.stencil_write[0] & 0xff } else { 0 },
            });
            if has_d {
                ds["depthWriteEnabled"] = (d.depth_test && d.depth_write).into();
                ds["depthCompare"] = if d.depth_test { obj::compare(d.depth_compare) } else { "always" }.into();
                if d.bias_enable && tri {
                    ds["depthBias"] = (d.bias[0].round() as i32).into();
                    ds["depthBiasSlopeScale"] = d.bias[2].into();
                    ds["depthBiasClamp"] = d.bias[1].into();
                }
            }
            desc["depthStencil"] = ds;
        }
        let dstr = desc.to_string();
        let key = h64(|h| dstr.hash(h));
        let id = {
            let mut c = cache();
            match c.rpipe.get(&key) {
                Some(&id) => id,
                None => {
                    let id = obj::new_id();
                    desc["id"] = id.into();
                    obj::json(op::RPIPE, &desc);
                    c.rpipe.insert(key, id);
                    c.by_pipe.entry(self.gfx_h).or_default().push(key);
                    id
                }
            }
        };
        if self.bound.pipe != id {
            self.bound.pipe = id;
            self.put(op::SET_PIPELINE, |p| {
                p.u32(id);
            });
        }
        // the per-draw uniform: Y direction, depth mode, base instance / vertex
        let vp = d.viewport.unwrap_or(vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: extent.0 as f32,
            height: extent.1 as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        });
        let flipped = vp.height < 0.0;
        let mut draw = [0u8; 16];
        draw[0..4].copy_from_slice(&(if flipped { 1.0f32 } else { -1.0f32 }).to_le_bytes());
        draw[4..8].copy_from_slice(&(if d.clip_neg { 1.0f32 } else { 0.0f32 }).to_le_bytes());
        draw[8..12].copy_from_slice(&first_instance.to_le_bytes());
        draw[12..16].copy_from_slice(&(base_vertex as u32).to_le_bytes());
        let uni = self.uniforms(draw);
        self.set_groups(bg, uni);
        // vertex and index buffers
        let mut vbs = Vec::new();
        for (slot, b) in slots.iter().enumerate() {
            let (bh, off, size) = self.vbufs.get(b).copied().unwrap_or((0, 0, 0));
            match self.buf_read(bh, off, size) {
                Some((id, n)) => vbs.push((slot as u32, id, off, n)),
                None => vbs.push((slot as u32, cache().dummy_buf, 0, 16)),
            }
        }
        if !missing.is_empty() {
            vbs.push((slots.len() as u32, cache().dummy_buf, 0, 64));
        }
        if self.bound.vb != vbs {
            for &(slot, id, off, n) in &vbs {
                self.put(op::SET_VB, |p| {
                    p.u32(slot).u32(id).u64(off).u64(n);
                });
            }
            self.bound.vb = vbs;
        }
        if indexed {
            let Some((bh, off, ty)) = self.ibuf else {
                red_once("an indexed draw without an index buffer");
                return false;
            };
            if ty == vk::IndexType::UINT8_EXT {
                red_once("8-bit indices (WebGPU has none): the draw is refused");
                return false;
            }
            let Some((id, n)) = self.buf_read(bh, off, vk::WHOLE_SIZE) else { return false };
            let f = (ty == vk::IndexType::UINT32) as u32;
            if self.bound.ib != (id, f, off) {
                self.bound.ib = (id, f, off);
                self.put(op::SET_IB, |p| {
                    p.u32(id).u32(f).u64(off).u64(n & !3);
                });
            }
        }
        // viewport (positive height: the shader flips Y), scissor (inside the target), blend constant, stencil ref
        let (y, h) = if flipped { (vp.y + vp.height, -vp.height) } else { (vp.y, vp.height) };
        let (mn, mx) = (vp.min_depth.min(vp.max_depth), vp.max_depth.max(vp.min_depth));
        let v = [vp.x, y, vp.width.max(0.0), h.max(0.0), mn.clamp(0.0, 1.0), mx.clamp(0.0, 1.0)].map(f32::to_bits);
        if self.bound.viewport != v {
            self.bound.viewport = v;
            self.put(op::VIEWPORT, |p| {
                for x in v {
                    p.u32(x);
                }
            });
        }
        let sc = d.scissor.unwrap_or(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D { width: extent.0, height: extent.1 },
        });
        let x0 = (sc.offset.x.max(0) as u32).min(extent.0);
        let y0 = (sc.offset.y.max(0) as u32).min(extent.1);
        let x1 = ((sc.offset.x as i64 + sc.extent.width as i64).clamp(0, extent.0 as i64)) as u32;
        let y1 = ((sc.offset.y as i64 + sc.extent.height as i64).clamp(0, extent.1 as i64)) as u32;
        let s = [x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0)];
        if self.bound.scissor != s {
            self.bound.scissor = s;
            self.put(op::SCISSOR, |p| {
                for x in s {
                    p.u32(x);
                }
            });
        }
        let bc = d.blend_const.map(f32::to_bits);
        if self.bound.blend != bc {
            self.bound.blend = bc;
            self.put(op::BLEND_CONST, |p| {
                for x in bc {
                    p.u32(x);
                }
            });
        }
        if self.bound.stencil_ref != d.stencil_ref[0] {
            self.bound.stencil_ref = d.stencil_ref[0];
            self.put(op::STENCIL_REF, |p| {
                p.u32(d.stencil_ref[0]);
            });
        }
        true
    }

    fn copy_b2t(&mut self, src: u64, dst: u64, regions: &[vk::BufferImageCopy]) {
        let Some(im) = self.o.img.get(&dst) else { return };
        let Some(t) = im.tex else { return };
        let Some(b) = self.o.buf.get(&src) else { return };
        let (bid, host) = (b.id, b.host);
        for r in regions {
            let (w, h) = (r.image_extent.width, r.image_extent.height);
            let rl = if r.buffer_row_length == 0 { w } else { r.buffer_row_length };
            let ih = if r.buffer_image_height == 0 { h } else { r.buffer_image_height };
            let bpr = rl.div_ceil(t.bw) * t.bytes;
            let rpi = ih.div_ceil(t.bh);
            let is3 = im.ty == vk::ImageType::TYPE_3D;
            let (z, d) = if is3 {
                (r.image_offset.z as u32, r.image_extent.depth)
            } else {
                (r.image_subresource.base_array_layer, r.image_subresource.layer_count)
            };
            let aspect = aspect_code(r.image_subresource.aspect_mask, im);
            if !buffer_copy_ok(&t, aspect, true) {
                red_once(&format!("a copy into {} ({:?}): WebGPU copies none into it, not copied", t.name, r.image_subresource.aspect_mask));
                continue;
            }
            if host.is_null() {
                self.put(op::COPY_B2T, |p| {
                    p.u32(bid).u64(r.buffer_offset).u32(bpr).u32(rpi);
                    p.u32(im.id).u32(r.image_subresource.mip_level).u32(r.image_offset.x as u32).u32(r.image_offset.y as u32).u32(z);
                    p.u32(w).u32(h).u32(d.max(1)).u32(aspect);
                });
            } else {
                // straight from the host memory (WebGPU's writeTexture has no row alignment rule)
                let src = host as u64 + r.buffer_offset;
                self.put(op::UPLOAD_TEX, |p| {
                    p.u32(im.id).u32(r.image_subresource.mip_level).u32(r.image_offset.x as u32).u32(r.image_offset.y as u32).u32(z);
                    p.u32(w).u32(h).u32(d.max(1)).u32(aspect);
                    p.u64(src).u32(bpr).u32(rpi);
                });
            }
        }
    }

    fn copy_t2b(&mut self, src: u64, dst: u64, regions: &[vk::BufferImageCopy]) {
        let Some(im) = self.o.img.get(&src).cloned() else { return };
        let Some(t) = im.tex else { return };
        for r in regions {
            let (w, h) = (r.image_extent.width, r.image_extent.height);
            let rl = if r.buffer_row_length == 0 { w } else { r.buffer_row_length };
            let ih = if r.buffer_image_height == 0 { h } else { r.buffer_image_height };
            let bpr = rl.div_ceil(t.bw) * t.bytes;
            let rpi = ih.div_ceil(t.bh);
            let is3 = im.ty == vk::ImageType::TYPE_3D;
            let (z, d) = if is3 {
                (r.image_offset.z as u32, r.image_extent.depth)
            } else {
                (r.image_subresource.base_array_layer, r.image_subresource.layer_count)
            };
            let total = bpr as u64 * rpi as u64 * (d.max(1) as u64 - 1) + bpr as u64 * (h.div_ceil(t.bh) as u64 - 1)
                + w.div_ceil(t.bw) as u64 * t.bytes as u64;
            let aspect = aspect_code(r.image_subresource.aspect_mask, &im);
            if !buffer_copy_ok(&t, aspect, false) {
                red_once(&format!("a copy out of {} ({:?}): WebGPU copies none out of it, not copied", t.name, r.image_subresource.aspect_mask));
                continue;
            }
            let Some((bid, _)) = self.buf_write(dst, r.buffer_offset, total) else { continue };
            self.put(op::COPY_T2B, |p| {
                p.u32(im.id).u32(r.image_subresource.mip_level).u32(r.image_offset.x as u32).u32(r.image_offset.y as u32).u32(z);
                p.u32(w).u32(h).u32(d.max(1)).u32(aspect);
                p.u32(bid).u64(r.buffer_offset).u32(bpr).u32(rpi);
            });
        }
    }
}

/// The static state of a bound pipeline replaces the command buffer's for every state it does not make dynamic.
fn apply_static(d: &mut Dyn, g: &obj::Gfx) {
    use vk::DynamicState as D;
    let dy = |s: D| g.dynamic.contains(&s);
    let s = &g.stat;
    if !dy(D::PRIMITIVE_TOPOLOGY) {
        d.topology = s.topology;
    }
    if !dy(D::PRIMITIVE_RESTART_ENABLE) {
        d.restart = s.restart;
    }
    if !dy(D::CULL_MODE) {
        d.cull = s.cull;
    }
    if !dy(D::FRONT_FACE) {
        d.front = s.front;
    }
    if !dy(D::DEPTH_TEST_ENABLE) {
        d.depth_test = s.depth_test;
    }
    if !dy(D::DEPTH_WRITE_ENABLE) {
        d.depth_write = s.depth_write;
    }
    if !dy(D::DEPTH_COMPARE_OP) {
        d.depth_compare = s.depth_compare;
    }
    if !dy(D::STENCIL_TEST_ENABLE) {
        d.stencil_test = s.stencil_test;
    }
    if !dy(D::STENCIL_OP) {
        d.stencil_ops = s.stencil_ops;
    }
    if !dy(D::STENCIL_COMPARE_MASK) {
        d.stencil_read = s.stencil_read;
    }
    if !dy(D::STENCIL_WRITE_MASK) {
        d.stencil_write = s.stencil_write;
    }
    if !dy(D::STENCIL_REFERENCE) {
        d.stencil_ref = s.stencil_ref;
    }
    if !dy(D::DEPTH_BIAS_ENABLE) {
        d.bias_enable = s.bias_enable;
    }
    if !dy(D::DEPTH_BIAS) {
        d.bias = s.bias;
    }
    if !dy(D::BLEND_CONSTANTS) {
        d.blend_const = s.blend_const;
    }
    if !dy(D::VIEWPORT) && !dy(D::VIEWPORT_WITH_COUNT) && s.viewport.is_some() {
        d.viewport = s.viewport;
    }
    if !dy(D::SCISSOR) && !dy(D::SCISSOR_WITH_COUNT) && s.scissor.is_some() {
        d.scissor = s.scissor;
    }
    if !dy(D::RASTERIZER_DISCARD_ENABLE) {
        d.discard = s.discard;
    }
    if !dy(D::DEPTH_CLAMP_ENABLE_EXT) {
        d.depth_clamp = s.depth_clamp;
    }
    if !dy(D::POLYGON_MODE_EXT) {
        d.polygon = s.polygon;
    }
    if !dy(D::RASTERIZATION_SAMPLES_EXT) {
        d.samples = s.samples;
    }
    if !dy(D::ALPHA_TO_COVERAGE_ENABLE_EXT) {
        d.a2c = s.a2c;
    }
    if !dy(D::LOGIC_OP_ENABLE_EXT) {
        d.logic_op = s.logic_op;
    }
    if !dy(D::COLOR_BLEND_ENABLE_EXT) {
        d.blend = s.blend;
    }
    if !dy(D::COLOR_BLEND_EQUATION_EXT) {
        d.eq = s.eq;
    }
    if !dy(D::COLOR_WRITE_MASK_EXT) {
        d.mask = s.mask;
    }
    if !dy(D::DEPTH_CLIP_NEGATIVE_ONE_TO_ONE_EXT) {
        d.clip_neg = s.clip_neg;
    }
    if !dy(D::VERTEX_INPUT_EXT) {
        d.vbind = s.vbind.clone();
        d.vattr = s.vattr.clone();
    }
}

/// Whether WebGPU copies this aspect of the texture between it and a buffer (`into`: buffer to texture): depth is
/// written only into depth16unorm and read only from depth16unorm / depth32float(-stencil8); stencil both ways; a
/// combined depth-stencil texture one aspect at a time.
fn buffer_copy_ok(t: &fmt::Tex, aspect: u32, into: bool) -> bool {
    if !t.depth && !t.stencil {
        return true;
    }
    match aspect {
        2 => t.stencil,
        _ if t.depth && t.stencil && aspect == 0 => false,
        _ if !t.depth => true,
        _ if into => t.name == "depth16unorm",
        _ => matches!(t.name, "depth16unorm" | "depth32float" | "depth32float-stencil8"),
    }
}

fn aspect_code(a: vk::ImageAspectFlags, im: &obj::Image) -> u32 {
    let t = im.tex;
    if a == vk::ImageAspectFlags::DEPTH && t.is_some_and(|t| t.stencil) {
        1
    } else if a == vk::ImageAspectFlags::STENCIL && t.is_some_and(|t| t.depth) {
        2
    } else {
        0
    }
}

fn subresources(im: &obj::Image, ranges: &[vk::ImageSubresourceRange]) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for r in ranges {
        let mips = if r.level_count == vk::REMAINING_MIP_LEVELS { im.mips - r.base_mip_level } else { r.level_count };
        let layers = if im.ty == vk::ImageType::TYPE_3D {
            im.extent.depth
        } else if r.layer_count == vk::REMAINING_ARRAY_LAYERS {
            im.layers - r.base_array_layer
        } else {
            r.layer_count
        };
        for m in r.base_mip_level..r.base_mip_level + mips {
            let base = if im.ty == vk::ImageType::TYPE_3D { 0 } else { r.base_array_layer };
            for l in base..base + layers {
                v.push((m, l));
            }
        }
    }
    v
}

fn dummy_texture(dim: &'static str, sample: &'static str, multi: bool) -> u32 {
    let mut c = cache();
    if let Some(&id) = c.dummy_tex.get(&(dim, sample, multi)) {
        return id;
    }
    let format = match sample {
        "sint" => "rgba8sint",
        "uint" => "rgba8uint",
        "depth" => "depth32float",
        "unfilterable-float" => "r32float",
        _ => "rgba8unorm",
    };
    let tex = obj::new_id();
    let (d, tdim) = match dim {
        "cube" | "cube-array" => (6, "2d"),
        "3d" => (1, "3d"),
        "1d" => (1, "1d"),
        _ => (1, "2d"),
    };
    obj::json(
        op::TEXTURE,
        &serde_json::json!({ "id": tex, "format": format, "dim": tdim, "w": 1, "h": 1, "d": d, "mips": 1,
            "samples": if multi { 4 } else { 1 }, "usage": if multi { 16 | 4 } else { 4 | 2 }, "viewFormats": [] }),
    );
    let view = obj::new_id();
    obj::json(
        op::VIEW,
        &serde_json::json!({ "id": view, "tex": tex, "format": format, "dim": dim, "baseMip": 0, "mips": 1,
            "baseLayer": 0, "layers": d, "aspect": "all", "swizzle": "rgba" }),
    );
    c.dummy_tex.insert((dim, sample, multi), view);
    view
}

/// A sampler's nearest-filtering twin (a texture WebGPU cannot filter takes a non-filtering sampler).
fn nearest_twin(id: u32) -> u32 {
    let mut c = cache();
    if let Some(&t) = c.nearest.get(&id) {
        return t;
    }
    let t = obj::new_id();
    obj::json(op::SAMPLER, &serde_json::json!({ "id": t, "twinOf": id }));
    c.nearest.insert(id, t);
    t
}

fn factor(f: vk::BlendFactor) -> &'static str {
    use vk::BlendFactor as B;
    match f {
        B::ZERO => "zero",
        B::ONE => "one",
        B::SRC_COLOR => "src",
        B::ONE_MINUS_SRC_COLOR => "one-minus-src",
        B::DST_COLOR => "dst",
        B::ONE_MINUS_DST_COLOR => "one-minus-dst",
        B::SRC_ALPHA => "src-alpha",
        B::ONE_MINUS_SRC_ALPHA => "one-minus-src-alpha",
        B::DST_ALPHA => "dst-alpha",
        B::ONE_MINUS_DST_ALPHA => "one-minus-dst-alpha",
        B::CONSTANT_COLOR | B::CONSTANT_ALPHA => "constant",
        B::ONE_MINUS_CONSTANT_COLOR | B::ONE_MINUS_CONSTANT_ALPHA => "one-minus-constant",
        B::SRC_ALPHA_SATURATE => "src-alpha-saturated",
        B::SRC1_COLOR => "src1",
        B::ONE_MINUS_SRC1_COLOR => "one-minus-src1",
        B::SRC1_ALPHA => "src1-alpha",
        B::ONE_MINUS_SRC1_ALPHA => "one-minus-src1-alpha",
        _ => "one",
    }
}

fn blend_op(o: vk::BlendOp) -> &'static str {
    match o {
        vk::BlendOp::SUBTRACT => "subtract",
        vk::BlendOp::REVERSE_SUBTRACT => "reverse-subtract",
        vk::BlendOp::MIN => "min",
        vk::BlendOp::MAX => "max",
        _ => "add",
    }
}

fn stencil(o: vk::StencilOp) -> &'static str {
    use vk::StencilOp as S;
    match o {
        S::ZERO => "zero",
        S::REPLACE => "replace",
        S::INCREMENT_AND_CLAMP => "increment-clamp",
        S::DECREMENT_AND_CLAMP => "decrement-clamp",
        S::INVERT => "invert",
        S::INCREMENT_AND_WRAP => "increment-wrap",
        S::DECREMENT_AND_WRAP => "decrement-wrap",
        _ => "keep",
    }
}

// ---------- submission ----------

fn merge(v: &mut Vec<(u32, u64, u64, u64)>) {
    // (id, dst offset, src, len): ranges of one buffer that touch become one
    v.sort_by_key(|x| (x.0, x.1));
    let mut out: Vec<(u32, u64, u64, u64)> = Vec::new();
    for x in v.drain(..) {
        if let Some(l) = out.last_mut() {
            if l.0 == x.0 && x.1 <= l.1 + l.3 && x.2.wrapping_sub(x.1) == l.2.wrapping_sub(l.1) {
                let end = (x.1 + x.3).max(l.1 + l.3);
                l.3 = end - l.1;
                continue;
            }
        }
        out.push(x);
    }
    *v = out;
}

/// `signals`: (done word, value, queued word) of each semaphore or fence the submission signals.
fn submit_one(cbs: &[vk::CommandBuffer], signals: &[(u64, u64, u64)]) {
    let o = objs();
    let mut rs = Rs::new(&o);
    for &c in cbs {
        // SAFETY: a command buffer of this device
        let cb = unsafe { super::cmd::cb(c) };
        rs.run(&cb.cmds);
        rs.end_compute();
        if rs.pass.take().is_some_and(|p| !p.empty) {
            rs.put(op::PASS_END, |_| {});
        }
    }
    let mut ups = std::mem::take(&mut rs.uploads);
    let mut rbs = std::mem::take(&mut rs.readbacks);
    let mut uniforms = std::mem::take(&mut rs.uniforms);
    let body = std::mem::take(&mut rs.s);
    let retired = std::mem::take(&mut rs.retired);
    drop(rs);
    drop(o);
    if uniforms.len() as u64 > obj::UNIFORM_RING_SIZE {
        red_once("a submission's per-draw uniforms exceed the uniform ring: later draws read wrong constants");
        uniforms.truncate(obj::UNIFORM_RING_SIZE as usize);
    }
    merge(&mut ups);
    let mut head = Pk::default();
    if !uniforms.is_empty() {
        sub(&mut head, op::UPLOAD, |p| {
            p.u32(obj::UNIFORM_RING).u64(0).u64(0).u64(uniforms.len() as u64);
            p.0.extend_from_slice(&uniforms);
        });
    }
    for (id, off, src, n) in ups {
        sub(&mut head, op::UPLOAD, |p| {
            p.u32(id).u64(off).u64(src).u64(n & !3);
        });
    }
    let mut tail = Pk::default();
    rbs.sort_by_key(|x| (x.0, x.1));
    for (id, off, n, dst) in rbs {
        sub(&mut tail, op::READBACK, |p| {
            p.u32(id).u64(off).u64(n).u64(dst);
        });
    }
    for &(addr, v, queued) in signals {
        sub(&mut tail, op::SIGNAL, |p| {
            p.u64(addr).u64(v).u64(queued);
        });
    }
    ring::push_parts(op::SUBMIT_BEGIN, &[&head.0, &body.0, &tail.0]);
    for id in retired {
        obj::destroy(id);
    }
}

/// A semaphore's or fence's (done word, queued word).
fn sem_word(h: u64) -> Option<(u64, u64)> {
    objs().sem.get(&h).map(|s| (s.word.addr(), s.queued.addr()))
}

pub unsafe extern "system" fn queue_submit2(_q: vk::Queue, n: u32, si: *const vk::SubmitInfo2<'_>, fence: vk::Fence) -> R {
    let infos = if si.is_null() || n == 0 { &[][..] } else { std::slice::from_raw_parts(si, n as usize) };
    for (k, s) in infos.iter().enumerate() {
        let cbs: Vec<vk::CommandBuffer> = if s.p_command_buffer_infos.is_null() {
            vec![]
        } else {
            std::slice::from_raw_parts(s.p_command_buffer_infos, s.command_buffer_info_count as usize)
                .iter()
                .map(|c| c.command_buffer)
                .collect()
        };
        let mut signals: Vec<(u64, u64, u64)> = Vec::new();
        if !s.p_signal_semaphore_infos.is_null() {
            for x in std::slice::from_raw_parts(s.p_signal_semaphore_infos, s.signal_semaphore_info_count as usize) {
                if let Some((a, q)) = sem_word(x.semaphore.as_raw()) {
                    signals.push((a, x.value.max(1), q));
                }
            }
        }
        if k + 1 == infos.len() && fence != vk::Fence::null() {
            if let Some((a, q)) = sem_word(fence.as_raw()) {
                signals.push((a, 1, q));
            }
        }
        submit_one(&cbs, &signals);
    }
    if infos.is_empty() && fence != vk::Fence::null() {
        if let Some((a, q)) = sem_word(fence.as_raw()) {
            submit_one(&[], &[(a, 1, q)]);
        }
    }
    vk::Result::SUCCESS
}

pub unsafe extern "system" fn queue_submit(_q: vk::Queue, n: u32, si: *const vk::SubmitInfo<'_>, fence: vk::Fence) -> R {
    let infos = if si.is_null() || n == 0 { &[][..] } else { std::slice::from_raw_parts(si, n as usize) };
    for (k, s) in infos.iter().enumerate() {
        let cbs = if s.p_command_buffers.is_null() {
            vec![]
        } else {
            std::slice::from_raw_parts(s.p_command_buffers, s.command_buffer_count as usize).to_vec()
        };
        let mut values: &[u64] = &[];
        super::icd::walk(s.p_next as *mut std::ffi::c_void, |t, p| {
            if t == vk::StructureType::TIMELINE_SEMAPHORE_SUBMIT_INFO {
                let ti = &*(p as *const vk::TimelineSemaphoreSubmitInfo<'_>);
                if !ti.p_signal_semaphore_values.is_null() {
                    values = std::slice::from_raw_parts(ti.p_signal_semaphore_values, ti.signal_semaphore_value_count as usize);
                }
            }
        });
        let mut signals = Vec::new();
        if !s.p_signal_semaphores.is_null() {
            for (i, x) in std::slice::from_raw_parts(s.p_signal_semaphores, s.signal_semaphore_count as usize).iter().enumerate() {
                if let Some((a, q)) = sem_word(x.as_raw()) {
                    signals.push((a, values.get(i).copied().unwrap_or(1).max(1), q));
                }
            }
        }
        if k + 1 == infos.len() && fence != vk::Fence::null() {
            if let Some((a, q)) = sem_word(fence.as_raw()) {
                signals.push((a, 1, q));
            }
        }
        submit_one(&cbs, &signals);
    }
    if infos.is_empty() && fence != vk::Fence::null() {
        if let Some((a, q)) = sem_word(fence.as_raw()) {
            submit_one(&[], &[(a, 1, q)]);
        }
    }
    let _ = Ordering::Relaxed;
    vk::Result::SUCCESS
}
