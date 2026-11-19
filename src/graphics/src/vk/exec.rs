//! Replay of one submission's canonical records into its command buffers.
//!
//! State belongs to each NVN queue (`QState`) and carries over between its submissions, as NVN's command-buffer state
//! does on a queue. Rendering is dynamic rendering over the bound targets, begun at the first draw or clear after a
//! target change and ended by anything that cannot run inside it (copies, dispatches, barriers); a full barrier
//! follows each of those. Window origin: NVN's default lower-left origin is Vulkan's framebuffer orientation once the
//! picture is flipped at present, so viewports are flipped only for an upper-left origin; the winding is inverted
//! whenever they are not (Vulkan measures a triangle's area with the opposite sign).

use super::queue::{Post, Sub};
use super::shader::{InKind, Module, Res};
use super::{layout as L, Gpu};
use crate::nvn::obj::*;
use crate::nvn::types::*;
use crate::tex::format::{self, Kind};
use ash::vk;
use gpu_allocator::MemoryLocation;
use std::sync::Arc;

const DRV_SIZE: usize = 0xA00;

#[derive(Clone, Copy)]
struct Rt {
    tex: Option<tex_t>,
    view: Option<view_t>,
}

pub struct QState {
    pub origin_ll: bool,
    prog: [Option<Arc<Module>>; 6],
    blend: [blend_st; 8],
    color: color_st,
    chmask: u32,
    ms: ms_st,
    poly: poly_st,
    ds: ds_st,
    vattr: [u32; 32],
    nvattr: usize,
    vstream: [vs_st; 16],
    vbuf: [(u64, u64); 16],
    ubo: [[(u64, u64); 16]; 7],
    ssbo: [[(u64, u64); 16]; 7],
    tex: [[u32; 256]; 7],
    samp: [[u32; 128]; 7],
    img: [[u32; 32]; 7],
    tex_pool: usize,
    samp_pool: usize,
    rt_n: usize,
    rt: [Rt; 8],
    rt_depth: Rt,
    vp: [[f32; 4]; 16],
    vp_origin: u32,
    depth_range: [f32; 2],
    scissor: Option<[i32; 4]>,
    stencil: [[u32; 3]; 2],
    poly_off: [f32; 3],
    blend_color: [f32; 4],
    prim_restart: (bool, u32),
    line_width: f32,
    depth_clamp: bool,
    rast_discard: bool,
    patch: u32,
    // per submission
    in_pass: bool,
    pass_extent: vk::Extent2D,
    pass_formats: ([i32; 8], usize, i32, u8),
    /// SAMPLES_PASSED counting is on (ResetCounter); the occlusion query open in the current pass
    vis_on: bool,
    occl_q: Option<u32>,
    /// the size of the last pass with colour targets (shadows=0 tells shadow-map passes by a depth-only size of their own)
    last_color: vk::Extent2D,
    /// NVN2 push constants: c[24] of the graphics (0) and compute (1) groups
    push: Box<[[u8; 0x800]; 2]>,
    /// conditional render: draws and clears go when true
    render_on: bool,
    alpha_ref: f32,
    depth_bounds: (bool, f32, f32),
    sample_mask: u32,
    point_size: f32,
    tess_levels: [f32; 6],
    provoking_last: bool,
    vp_swizzle: [u32; 16],
    tf: [(u64, u64); 4],
    tf_on: bool,
    /// transform feedback: buffers of the capture open around the current draw (0: none), and whether the next
    /// draw continues the capture (its counters) rather than starting at the buffers' start
    tf_active: u32,
    tf_cont: bool,
    /// a mesh draw's records (buffer, offset, bytes) and their stride in words (push constant byte 12)
    mesh_rec: Option<(vk::Buffer, u64, u64)>,
    mesh_stride: u32,
    /// graphics program keys bound whose modules the library did not have (the draw waits for the live translator)
    missing: Vec<(crate::nvn::prog::ShaderKey, u32)>,
    /// block aliases of compressed targets to copy into their images when the pass ends
    copyback: Vec<(vk::Image, vk::Image, u32, u32, vk::Extent2D, vk::Extent2D, u32)>,
    /// a stage's bindings generation (new at every bind for it) and its driver constant banks c0 / c2 built for it
    bgen: [u64; 7],
    cbank: std::cell::RefCell<[Option<CBank>; 7]>,
    /// push constants' generation (new at every PushConstants)
    pgen: u64,
    /// staged constant blocks by (kind, key): where they are in submission `serial` (support buffer by pass size,
    /// a module's c1, the push banks by generation; each was hashed and staged again at every draw)
    scache: std::cell::RefCell<std::collections::HashMap<(u8, u64), (u64, (vk::Buffer, u64))>>,
}

/// A stage's c0 / c2 as last built: valid while its bindings generation, dispatch size and base instance stand; where
/// they were staged in submission `sub` (Pokemon Legends: Z-A, ~1,700 draws a frame: building, hashing and staging
/// 5 KB a stage a draw was most of the 8 ms of descriptor work a frame)
struct CBank {
    gen: u64,
    dispatch: [u32; 3],
    base_instance: u32,
    c0: Arc<Vec<u8>>,
    c2: Arc<Vec<u8>>,
    staged: Option<(u64, (vk::Buffer, u64), (vk::Buffer, u64))>,
}

fn new_bgen() -> u64 {
    static G: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    G.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl QState {
    /// `bytes` staged in `sub`, once per submission for the same (kind, key).
    fn staged(&self, g: &Gpu, sub: &mut Sub, kind: u8, key: u64, bytes: impl FnOnce() -> Vec<u8>) -> (vk::Buffer, u64) {
        let mut c = self.scache.borrow_mut();
        if let Some(&(serial, r)) = c.get(&(kind, key)) {
            if serial == sub.serial {
                return r;
            }
        }
        if c.len() > 4096 {
            c.clear();
        }
        let r = sub.stage_bytes(g, &bytes());
        c.insert((kind, key), (sub.serial, r));
        r
    }

    fn bump(&mut self, st: usize) {
        if st < 7 {
            self.bgen[st] = new_bgen();
        }
    }

    /// c0 / c2 of stage `st` and where they are staged in `sub` (built and staged again only when they changed).
    fn banks(&self, g: &Gpu, sub: &mut Sub, st: usize, dispatch: [u32; 3], base_instance: u32) -> (Arc<Vec<u8>>, Arc<Vec<u8>>, (vk::Buffer, u64), (vk::Buffer, u64)) {
        let mut cb = self.cbank.borrow_mut();
        let ok = cb[st].as_ref().is_some_and(|c| c.gen == self.bgen[st] && c.dispatch == dispatch && c.base_instance == base_instance);
        if !ok {
            cb[st] = Some(CBank {
                gen: self.bgen[st],
                dispatch,
                base_instance,
                c0: Arc::new(c0_bytes(self, st, dispatch, base_instance)),
                c2: Arc::new(c2_bytes(self, st)),
                staged: None,
            });
        }
        let c = cb[st].as_mut().unwrap();
        let (s0, s2) = match c.staged {
            Some((serial, s0, s2)) if serial == sub.serial => (s0, s2),
            _ => {
                let s0 = sub.stage_bytes(g, &c.c0);
                let s2 = sub.stage_bytes(g, &c.c2);
                c.staged = Some((sub.serial, s0, s2));
                (s0, s2)
            }
        };
        (c.c0.clone(), c.c2.clone(), s0, s2)
    }

    pub fn new(origin_ll: bool) -> QState {
        let mut blend = [blend_st(0); 8];
        for (i, b) in blend.iter_mut().enumerate() {
            b.set(blend_field::TARGET, i as u32);
            b.set(blend_field::SRC_RGB, blend_func::ONE);
            b.set(blend_field::DST_RGB, blend_func::ZERO);
            b.set(blend_field::SRC_A, blend_func::ONE);
            b.set(blend_field::DST_A, blend_func::ZERO);
            b.set(blend_field::EQ_RGB, blend_eq::ADD);
            b.set(blend_field::EQ_A, blend_eq::ADD);
        }
        let mut ds = ds_st(0);
        ds.set(ds_field::DW, 1);
        ds.set(ds_field::DF, cmp::LESS);
        ds.set(ds_field::SF_F, cmp::ALWAYS);
        ds.set(ds_field::SF_B, cmp::ALWAYS);
        for f in [ds_field::FAIL_F, ds_field::ZFAIL_F, ds_field::ZPASS_F, ds_field::FAIL_B, ds_field::ZFAIL_B, ds_field::ZPASS_B] {
            ds.set(f, stencil_op::KEEP);
        }
        QState {
            origin_ll,
            prog: Default::default(),
            blend,
            color: color_st::new(0, logic_op::COPY, cmp::ALWAYS),
            chmask: 0xffff_ffff,
            ms: ms_st { enable: 1, a2c_dither: 1, ..Default::default() },
            poly: poly_st::new(face::NONE, front_face::CCW, polygon_mode::FILL, 0),
            ds,
            vattr: [0; 32],
            nvattr: 0,
            vstream: [vs_st::default(); 16],
            vbuf: [(0, 0); 16],
            ubo: [[(0, 0); 16]; 7],
            ssbo: [[(0, 0); 16]; 7],
            tex: [[0; 256]; 7],
            samp: [[0; 128]; 7],
            img: [[0; 32]; 7],
            tex_pool: 0,
            samp_pool: 0,
            rt_n: 0,
            rt: [Rt { tex: None, view: None }; 8],
            rt_depth: Rt { tex: None, view: None },
            vp: [[0.0; 4]; 16],
            vp_origin: 0,
            depth_range: [0.0, 1.0],
            scissor: None,
            stencil: [[0xff, 0xff, 0]; 2],
            poly_off: [0.0; 3],
            blend_color: [0.0; 4],
            prim_restart: (false, u32::MAX),
            line_width: 1.0,
            depth_clamp: false,
            rast_discard: false,
            patch: 3,
            in_pass: false,
            pass_extent: vk::Extent2D::default(),
            pass_formats: ([0; 8], 0, 0, 1),
            copyback: Vec::new(),
            bgen: core::array::from_fn(|_| new_bgen()),
            cbank: Default::default(),
            pgen: new_bgen(),
            scache: Default::default(),
            vis_on: false,
            occl_q: None,
            last_color: vk::Extent2D::default(),
            missing: Vec::new(),
            push: Box::new([[0; 0x800]; 2]),
            render_on: true,
            alpha_ref: 0.0,
            depth_bounds: (false, 0.0, 1.0),
            sample_mask: u32::MAX,
            point_size: 1.0,
            tess_levels: [1.0; 6],
            provoking_last: true,
            vp_swizzle: [0x0302_0100; 16],
            tf: [(0, 0); 4],
            tf_on: false,
            tf_active: 0,
            tf_cont: false,
            mesh_rec: None,
            mesh_stride: 0,
        }
    }
}

fn barrier(g: &Gpu, sub: &Sub) {
    super::queue::full_barrier(g, sub.main);
}

/// Append counters that are indirect draw arguments (Donkey Kong Bananza's terrain). Every chunk has a 64-byte record
/// the CPU never touches, one NVNdrawElementsIndirectData, all zero. The depth pre-pass draws the chunk with a
/// passthrough geometry shader that has the record as a written storage buffer: per visible triangle it adds 3 to
/// word 0 and stores the indices behind the old count into a second buffer. The shading pass draws that list with
/// DrawElementsIndirect(arguments = the record). Nothing in the game's command stream or on its CPU writes the record
/// again, so taken literally the count grows without end and the draw has 0 instances. What a frame needs: a list
/// holding this frame's triangles, drawn once. So an indirect draw whose arguments a draw's shader wrote as a storage
/// buffer in the same frame gets one instance when its record says none, and its count goes back to 0 when the next
/// frame begins. Records written by dispatches or by the CPU are not touched. `NXGFX_APPEND_RESET=0` turns it off.
mod append {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct St {
        /// address -> the frame a draw last bound it as a written storage buffer
        bound: HashMap<u64, u64>,
        /// records in use by indirect draws
        on: Vec<u64>,
        started: u64,
    }
    static ST: Mutex<Option<St>> = Mutex::new(None);

    fn enabled() -> bool {
        static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *V.get_or_init(|| std::env::var("NXGFX_APPEND_RESET").as_deref() != Ok("0"))
    }
    fn frame() -> u64 {
        super::super::present::frames() + 2
    }

    pub fn bound(a: u64) {
        if !enabled() {
            return;
        }
        let mut g = ST.lock().unwrap();
        let s = g.get_or_insert_with(|| St { bound: HashMap::new(), on: Vec::new(), started: 0 });
        if s.bound.len() > 65536 {
            let f = frame();
            s.bound.retain(|_, v| *v + 600 >= f);
        }
        s.bound.insert(a, frame());
    }

    fn fill(g: &Gpu, sub: &mut Sub, a: u64, v: u32) {
        if let Some((b, o, _)) = super::super::mem::resolve(g, sub, a, 4) {
            unsafe {
                super::super::queue::full_barrier(g, sub.main);
                g.dev().cmd_fill_buffer(sub.main, b, o, 4, v);
                super::super::queue::full_barrier(g, sub.main);
            }
            super::super::mem::note_gpu_write(g, sub, a, 4);
        }
    }

    /// An indirect draw with its arguments at `ind` (inside the pass: fills end and reopen it).
    pub fn draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, ind: u64, indexed: bool) {
        if !enabled() {
            return;
        }
        let f = frame();
        let (resets, fresh) = {
            let mut gl = ST.lock().unwrap();
            let Some(s) = gl.as_mut() else { return };
            let mut resets = Vec::new();
            if s.started != f {
                s.started = f;
                let bound = &s.bound;
                s.on.retain(|a| bound.get(a).is_some_and(|&b| b + 600 >= f));
                resets = s.on.clone();
            }
            let recent = ind != 0 && s.bound.get(&ind).is_some_and(|&b| b + 1 >= f);
            let fresh = recent && !s.on.contains(&ind) && s.on.len() < 8192;
            if fresh {
                s.on.push(ind);
            }
            (resets, fresh)
        };
        if resets.is_empty() && !fresh {
            return;
        }
        // fills cannot run inside a pass: the draw opens its pass again
        end_pass(g, sub, qs);
        for a in resets {
            fill(g, sub, a, 0);
            super::super::perf::inc(super::super::perf::C::AppendReset);
        }
        if fresh {
            // SAFETY: the record in guest memory (the CPU's view of the instance count)
            let inst = unsafe { core::ptr::read_unaligned((ind + 4) as *const u32) };
            if inst == 0 {
                fill(g, sub, ind + 4, 1);
            }
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::vklog!("append counters: a draw's shader appends to the arguments of a later indirect {}draw ({ind:#x}); such counts start each frame at 0, and a record with 0 instances draws once (NXGFX_APPEND_RESET=0 turns it off)", if indexed { "indexed " } else { "" });
            }
        }
    }
}

/// Pass dumps (diagnostics): `NXGFX_DUMP_AT_S=<s>[,<s>...]` (or the control command `passdump`): at each moment the
/// colour targets of one frame's passes as they leave them, `<dir>/pass-<frame>-<n>-<target>-<w>x<h>-<format>.png`
/// (`NXGFX_DUMP_DIR`, default `captures`), with one log line per pass (its draws, targets, formats).
pub mod passdump {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static FRAME: AtomicU64 = AtomicU64::new(0); // the frame being dumped (0: none)
    static PASS: AtomicU64 = AtomicU64::new(0);
    static REQ: Mutex<Vec<f64>> = Mutex::new(Vec::new());
    static INIT: std::sync::Once = std::sync::Once::new();

    pub fn request_now() {
        REQ.lock().unwrap().push(0.0);
    }

    /// The frame being replayed is being dumped (its draws' resources are logged too).
    pub fn active() -> bool {
        let f = FRAME.load(Ordering::Relaxed);
        f != 0 && super::super::present::frames() + 1 == f
    }
    pub fn pass_no() -> u64 {
        PASS.load(Ordering::Relaxed)
    }

    /// Every present: starts a dump frame when one is due.
    pub fn frame(n: u64, secs: f64) {
        INIT.call_once(|| {
            if let Ok(v) = std::env::var("NXGFX_DUMP_AT_S") {
                REQ.lock().unwrap().extend(v.split(',').filter_map(|x| x.trim().parse::<f64>().ok()));
            }
        });
        if FRAME.load(Ordering::Relaxed) != 0 && FRAME.load(Ordering::Relaxed) + 1 <= n {
            FRAME.store(0, Ordering::Relaxed);
        }
        let mut r = REQ.lock().unwrap();
        if let Some(i) = r.iter().position(|&t| t <= secs) {
            r.remove(i);
            FRAME.store(n + 1, Ordering::Relaxed);
            PASS.store(0, Ordering::Relaxed);
            crate::vklog!("pass dump: frame {}", n + 1);
        }
    }

    fn dir() -> std::path::PathBuf {
        std::env::var("NXGFX_DUMP_DIR").map(Into::into).unwrap_or_else(|_| "captures".into())
    }

    pub fn pass_end(g: &Gpu, sub: &mut Sub, qs: &QState) {
        let f = FRAME.load(Ordering::Relaxed);
        if f == 0 || super::super::present::frames() + 1 != f {
            return;
        }
        let p = PASS.fetch_add(1, Ordering::Relaxed);
        let mut desc = vec![];
        for i in 0..qs.rt_n {
            let Some(t) = qs.rt[i].tex else { continue };
            let Some(tf) = crate::tex::format::tex_format(t.d.format) else { continue };
            desc.push(format!("{i}:{:?} {}x{}", tf.vk, t.d.width, t.d.height));
            let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
            if bw != 1 || bh != 1 || bpb == 0 || !matches!(tf.kind, Kind::Float | Kind::Uint | Kind::Sint) {
                continue;
            }
            let v = qs.rt[i].view;
            let level = v.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
            let layer = v.filter(|v| v.bits & VW_LAYERS != 0).map(|v| v.min_layer as u32).unwrap_or(0);
            let (w, h) = ((t.d.width >> level).max(1), (t.d.height >> level).max(1));
            let n = w as u64 * h as u64 * bpb as u64;
            let img = { g.images.lock().unwrap().by_serial.get(&t.serial).map(|im| im.image) };
            let Some(img) = img else { continue };
            let Ok(buf) = g.buffer(n, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "pass dump") else { continue };
            let r = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: level, base_array_layer: layer, layer_count: 1 },
                image_offset: vk::Offset3D::default(),
                image_extent: vk::Extent3D { width: w, height: h, depth: 1 },
            };
            unsafe { g.dev().cmd_copy_image_to_buffer(sub.main, img, vk::ImageLayout::GENERAL, buf.buf, &[r]) };
            super::super::queue::full_barrier(g, sub.main);
            let ptr = buf.ptr as usize;
            let path = dir().join(format!("pass-{f:06}-{p:03}-t{i}-{w}x{h}-{:?}.png", tf.vk));
            let fmt = tf.vk;
            sub.transient.push(buf);
            sub.post.push(Post::Callback(Box::new(move |_g: &Gpu| {
                let np = (w * h) as usize;
                // SAFETY: the read-back buffer, kept until after this callback
                let src = unsafe { core::slice::from_raw_parts(ptr as *const u8, n as usize) };
                let mut out = vec![0u8; np * 4];
                if super::super::shot::to_rgba8(fmt, src, np, &mut out) {
                    let _ = std::fs::create_dir_all(dir());
                    let _ = super::super::shot::png_write(&path, &out, w, h);
                }
            })));
        }
        let ds = qs.rt_depth.tex.map(|t| format!(" depth {}x{} fmt {:#x}", t.d.width, t.d.height, t.d.format)).unwrap_or_default();
        crate::vklog!("pass dump {f}/{p}: extent {}x{}, targets [{}]{ds}", qs.pass_extent.width, qs.pass_extent.height, desc.join(", "));
    }
}

/// NVN2's DrawMeshTasks*: the bound task and mesh shaders through VK_EXT_mesh_shader. Direct: `x` (`y`, `z`)
/// workgroups. Indirect: the game's records ({count, first}, 3D {x, y, z}) are written by the GPU earlier in the same
/// submission, so the workgroup counts are copied on the GPU into Vulkan's {x, y, z} records (outside the pass) and
/// drawn from there; with a count buffer, MultiDraw…IndirectCount reads the draw count on the GPU too.
fn mesh_draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, m: crate::nvn::cmd2::p_mesh) {
    if !g.d.caps.mesh {
        crate::log::red_once("task and mesh draws: the device has no VK_EXT_mesh_shader, they are not drawn");
        super::perf::inc(super::perf::C::DrawsSkipped);
        return;
    }
    let indirect = m.ind != 0;
    // indirect: build {x, y, z} records first (a copy is not allowed inside a pass)
    let mut args: Option<(vk::Buffer, u64, u32)> = None;
    if indirect {
        let n = m.n.max(1) as u32;
        let src_stride = if m.stride > 0 { m.stride as u64 } else if m.three_d != 0 { 12 } else { 8 };
        let span = src_stride * (n as u64 - 1) + if m.three_d != 0 { 12 } else { 8 };
        let Some((sb, so, _)) = super::mem::resolve(g, sub, m.ind, span) else {
            crate::log::red_once(&format!("DrawMeshTasksIndirect record at {:#x} is in no memory pool: draw skipped", m.ind));
            super::perf::inc(super::perf::C::DrawsSkipped);
            return;
        };
        end_pass(g, sub, qs);
        let ones: Vec<u8> = (0..n).flat_map(|_| [0u32, 1, 1]).flat_map(|w| w.to_le_bytes()).collect();
        let (ab, ao) = sub.stage_bytes_fresh(g, &ones);
        let dev = g.dev();
        let regions: Vec<vk::BufferCopy> = (0..n as u64)
            .map(|i| vk::BufferCopy { src_offset: so + i * src_stride, dst_offset: ao + i * 12, size: if m.three_d != 0 { 12 } else { 4 } })
            .collect();
        unsafe { dev.cmd_copy_buffer(sub.main, sb, ab, &regions) };
        barrier(g, sub);
        args = Some((ab, ao, n));
    }
    // the draw records the task / mesh shader reads its first workgroup from (lane S: `mesh_draw`, stride at push
    // constant byte 12): the game's records for an indirect draw, a staged {count, first} for a direct one, zero for 3D
    if indirect {
        let src_stride = if m.stride > 0 { m.stride as u64 } else if m.three_d != 0 { 12 } else { 8 };
        let span = src_stride * (m.n.max(1) as u64 - 1) + if m.three_d != 0 { 12 } else { 8 };
        qs.mesh_rec = super::mem::resolve(g, sub, m.ind, span).map(|(b, o, _)| (b, o, span));
        qs.mesh_stride = if m.three_d != 0 { 0 } else { (src_stride / 4) as u32 };
        if m.three_d != 0 {
            let (b, o) = sub.stage_bytes(g, &[0u8; 16]);
            qs.mesh_rec = Some((b, o, 16));
        }
    } else {
        let rec: [u32; 4] = [m.x.max(0) as u32, if m.three_d != 0 { 0 } else { m.first.max(0) as u32 }, 0, 0];
        let bytes: Vec<u8> = rec.iter().flat_map(|x| x.to_le_bytes()).collect();
        let (b, o) = sub.stage_bytes(g, &bytes);
        qs.mesh_rec = Some((b, o, 16));
        qs.mesh_stride = 0;
    }
    let ok = draw_setup_ex(g, sub, qs, prim::TRIANGLES, 0, true);
    if !ok {
        qs.mesh_rec = None;
        qs.mesh_stride = 0;
        return;
    }
    let ms = &g.d.mesh;
    match args {
        None => {
            let (x, y, z) = if m.three_d != 0 { (m.x, m.y, m.z) } else { (m.x, 1, 1) };
            if x > 0 && y > 0 && z > 0 {
                unsafe { ms.cmd_draw_mesh_tasks(sub.main, x as u32, y as u32, z as u32) };
            }
        }
        Some((ab, ao, n)) => {
            if m.cnt != 0 {
                match super::mem::resolve(g, sub, m.cnt, 4) {
                    Some((cb, co, _)) => unsafe { ms.cmd_draw_mesh_tasks_indirect_count(sub.main, ab, ao, cb, co, n, 12) },
                    None => crate::log::red_once(&format!("mesh draw count at {:#x} is in no memory pool: draw skipped", m.cnt)),
                }
            } else {
                unsafe { ms.cmd_draw_mesh_tasks_indirect(sub.main, ab, ao, n, 12) };
            }
        }
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let k = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if k & 0xfff == 0 {
        crate::vklog!("mesh draws so far: {}; this one {} ({} {} {}), indirect {}", k + 1, if m.three_d != 0 { "3D" } else { "1D" }, m.x, m.y, m.z, indirect);
    }
    qs.mesh_rec = None;
    qs.mesh_stride = 0;
}

/// MultiDraw{Arrays,Elements}IndirectCount: Vulkan's indirect draws with a count the GPU reads.
fn multi_draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, m: crate::nvn::cmd2::p_multi_draw) {
    if m.max <= 0 {
        return;
    }
    let indexed = m.itype >= 0;
    let base = p_draw { prim: m.prim, instances: 1, ..Default::default() };
    if !draw_setup(g, sub, qs, base.prim, 0) {
        return;
    }
    let rec_n = if indexed { 20 } else { 16 };
    let stride = if m.stride > 0 { m.stride as u32 } else { rec_n };
    let span = stride as u64 * (m.max as u64 - 1) + rec_n as u64;
    let (Some((ib, io, _)), Some((cb, co, _))) = (super::mem::resolve(g, sub, m.ind, span), super::mem::resolve(g, sub, m.cnt, 4)) else { return };
    let dev = g.dev();
    unsafe {
        if indexed {
            let ity = match m.itype {
                index_type::U8 if g.d.caps.index_u8 => vk::IndexType::UINT8_EXT,
                index_type::U16 => vk::IndexType::UINT16,
                _ => vk::IndexType::UINT32,
            };
            let Some((xb, xo, _)) = super::mem::resolve(g, sub, m.ibuf, 1) else { return };
            dev.cmd_bind_index_buffer(sub.main, xb, xo, ity);
            dev.cmd_draw_indexed_indirect_count(sub.main, ib, io, cb, co, m.max as u32, stride);
        } else {
            dev.cmd_draw_indirect_count(sub.main, ib, io, cb, co, m.max as u32, stride);
        }
    }
}

/// DrawTexture: the texture's source rectangle drawn into the destination rectangle of colour target 0 (a filtered
/// blit; NVN draws it without the bound program, depth test or blending).
fn draw_texture(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: crate::nvn::cmd2::p_draw_tex) {
    if !qs.render_on {
        return;
    }
    let (tb, _) = pool_bases(qs);
    let id = (d.handle & 0xfffff) as i32 + tb;
    let Some((serial, _)) = g.tables.tex_at(id) else {
        crate::log::red_once("DrawTexture of an unregistered texture");
        return;
    };
    let Some(Rt { tex: Some(dst), view }) = qs.rt.first().copied() else { return };
    end_pass(g, sub, qs);
    let src_img = {
        let mut imgs = g.images.lock().unwrap();
        g.prepare_image(&mut imgs, serial, sub);
        imgs.by_serial.get(&serial).map(|im| (im.image, im.tf))
    };
    let Some((si, stf)) = src_img else { return };
    let Some((di, _, _, _)) = target_image(g, sub, &dst) else { return };
    let level = view.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
    let o = |v: [f32; 4], i: usize| vk::Offset3D { x: v[i] as i32, y: v[i + 1] as i32, z: i as i32 / 2 };
    let r = vk::ImageBlit {
        src_subresource: vk::ImageSubresourceLayers { aspect_mask: super::image::upload_aspect(&stf), mip_level: 0, base_array_layer: 0, layer_count: 1 },
        src_offsets: [o(d.src, 0), o(d.src, 2)],
        dst_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: level, base_array_layer: 0, layer_count: 1 },
        dst_offsets: [o(d.dst, 0), o(d.dst, 2)],
    };
    unsafe { g.dev().cmd_blit_image(sub.main, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &[r], vk::Filter::LINEAR) };
    barrier(g, sub);
}

/// Closes the open occlusion query: its samples join the sum at completion.
fn end_query(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    if let Some(q) = qs.occl_q.take() {
        unsafe { g.dev().cmd_end_query(sub.main, sub.occl, q) };
        sub.post.push(Post::Occl { q });
    }
}

fn end_pass(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    end_query(g, sub, qs);
    if qs.in_pass {
        unsafe { g.dev().cmd_end_rendering(sub.main) };
        qs.in_pass = false;
        barrier(g, sub);
        passdump::pass_end(g, sub, qs);
        // compressed targets drawn through a block alias: the texels become the blocks
        // (through a buffer: an image copy cannot name a level smaller than one block, a buffer copy can)
        for (src, dst, level, layer, e, texels, bpb) in qs.copyback.drain(..) {
            let size = e.width as u64 * e.height as u64 * bpb as u64;
            let Ok(tmp) = g.buffer(size, vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuOnly, "block copy") else { continue };
            let sub_l = |mip, layer| vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: mip, base_array_layer: layer, layer_count: 1 };
            let up = vk::BufferImageCopy { buffer_offset: 0, buffer_row_length: 0, buffer_image_height: 0, image_subresource: sub_l(0, 0), image_offset: vk::Offset3D::default(), image_extent: vk::Extent3D { width: e.width, height: e.height, depth: 1 } };
            let down = vk::BufferImageCopy { image_subresource: sub_l(level, layer), image_extent: vk::Extent3D { width: texels.width, height: texels.height, depth: 1 }, ..up };
            unsafe {
                g.dev().cmd_copy_image_to_buffer(sub.main, src, vk::ImageLayout::GENERAL, tmp.buf, &[up]);
                super::queue::full_barrier(g, sub.main);
                g.dev().cmd_copy_buffer_to_image(sub.main, tmp.buf, dst, vk::ImageLayout::GENERAL, &[down]);
            }
            barrier(g, sub);
            sub.transient.push(tmp);
        }
    }
}

pub fn end(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    end_pass(g, sub, qs);
}

/// The image of a texture snapshot for writing (render target, copy destination): made GPU-owned, prepared in the
/// upload command buffer when new. Returns (image, format info, extent, levels, layers, samples).
fn target_image(g: &Gpu, sub: &mut Sub, t: &tex_t) -> Option<(vk::Image, format::TexFormat, vk::Extent3D, vk::SampleCountFlags)> {
    let mut imgs = g.images.lock().unwrap();
    if !g.ensure_image(&mut imgs, t) {
        return None;
    }
    g.tex_gpu_written(&mut imgs, t.serial);
    g.prepare_image(&mut imgs, t.serial, sub);
    let im = &imgs.by_serial[&t.serial];
    Some((im.image, im.tf, im.extent, im.samples))
}
fn source_image(g: &Gpu, sub: &mut Sub, t: &tex_t) -> Option<(vk::Image, format::TexFormat, vk::Extent3D)> {
    let mut imgs = g.images.lock().unwrap();
    if !g.ensure_image(&mut imgs, t) {
        return None;
    }
    g.prepare_image(&mut imgs, t.serial, sub);
    let im = &imgs.by_serial[&t.serial];
    Some((im.image, im.tf, im.extent))
}
/// The layers a render target's view covers (a layered pass draws into all of them).
fn attach_layers(im: &super::image::Image, v: Option<&view_t>) -> u32 {
    let lvl = v.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
    let total = if im.ty == vk::ImageType::TYPE_3D { (im.extent.depth >> lvl).max(1) } else { im.layers };
    match v.filter(|v| v.bits & VW_LAYERS != 0) {
        Some(v) => (v.nlayers as u32).max(1).min(total.saturating_sub(v.min_layer as u32).max(1)),
        None => total,
    }
}

fn attach_view_layers(g: &Gpu, sub: &mut Sub, qs: &mut QState, t: &tex_t, v: Option<&view_t>) -> Option<((vk::ImageView, vk::Format, vk::Extent2D, vk::SampleCountFlags, Kind), u32)> {
    let _ = target_image(g, sub, t)?;
    let mut imgs = g.images.lock().unwrap();
    let im = imgs.by_serial.get_mut(&t.serial)?;
    let lvl = v.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
    let layer = v.filter(|v| v.bits & VW_LAYERS != 0).map(|v| v.min_layer as u32).unwrap_or(0);
    let e = vk::Extent2D { width: (im.extent.width >> lvl).max(1), height: (im.extent.height >> lvl).max(1) };
    let fmt = g.raw_view_format(im, v);
    let (samples, kind) = (im.samples, im.tf.kind);
    // a compressed texture rendered through an uncompressed view of its block size (a game encoding blocks on the
    // GPU): Vulkan cannot render into a compressed image, so the pass draws into an image of the view's format, one
    // texel per block, copied into the blocks at the end of the pass
    if !im.usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) && fmt != im.format {
        let (_, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
        if bw > 1 || bh > 1 || im.bc7 {
            let (bw, bh) = if im.bc7 { (4, 4) } else { (bw, bh) };
            let be = vk::Extent2D { width: e.width.div_ceil(bw), height: e.height.div_ceil(bh) };
            let dst = im.image;
            let im_bc7 = im.bc7;
            drop(imgs);
            let (img, view) = block_alias(g, sub, fmt, be)?;
            let bpb = crate::tex::layout::fmt_bpb(t.d.format).0.max(if im_bc7 { 16 } else { 0 });
            qs.copyback.push((img, dst, lvl, layer, be, e, bpb));
            return Some(((view, fmt, be, samples, Kind::Float), 1));
        }
    }
    let fmt = g.view_format(im, v);
    let layers = attach_layers(im, v);
    let view = g.image_view(im, v, true)?;
    Some(((view, fmt, e, samples, kind), layers))
}

/// An uncompressed image standing in for a compressed render target (one texel per block), kept per format and size.
fn block_alias(g: &Gpu, sub: &mut Sub, fmt: vk::Format, e: vk::Extent2D) -> Option<(vk::Image, vk::ImageView)> {
    type Alias = (vk::Format, vk::Extent2D, vk::Image, vk::ImageView);
    static ALIASES: std::sync::Mutex<Vec<Alias>> = std::sync::Mutex::new(Vec::new());
    let mut a = ALIASES.lock().unwrap();
    if let Some(x) = a.iter().find(|x| x.0 == fmt && x.1 == e) {
        return Some((x.2, x.3));
    }
    let ci = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(fmt)
        .extent(vk::Extent3D { width: e.width, height: e.height, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let img = unsafe { g.dev().create_image(&ci, None) }.ok()?;
    let req = unsafe { g.dev().get_image_memory_requirements(img) };
    let al = g
        .alloc
        .lock()
        .unwrap()
        .allocate(&gpu_allocator::vulkan::AllocationCreateDesc {
            name: "block alias",
            requirements: req,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: gpu_allocator::vulkan::AllocationScheme::GpuAllocatorManaged,
        })
        .ok()?;
    unsafe { g.dev().bind_image_memory(img, al.memory(), al.offset()) }.ok()?;
    std::mem::forget(al); // kept for the process (one per format and size)
    let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
    let vci = vk::ImageViewCreateInfo::default().image(img).view_type(vk::ImageViewType::TYPE_2D).format(fmt).subresource_range(range);
    let view = unsafe { g.dev().create_image_view(&vci, None) }.ok()?;
    let b = [vk::ImageMemoryBarrier2::default()
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::GENERAL)
        .image(img)
        .subresource_range(range)];
    unsafe { g.dev().cmd_pipeline_barrier2(sub.pre, &vk::DependencyInfo::default().image_memory_barriers(&b)) };
    crate::vklog!("compressed render target: an uncompressed {fmt:?} {}x{} stands in for its blocks", e.width, e.height);
    a.push((fmt, e, img, view));
    Some((img, view))
}

fn begin_pass(g: &Gpu, sub: &mut Sub, qs: &mut QState) -> bool {
    if qs.in_pass {
        return true;
    }
    let mut colors = Vec::new();
    let mut formats = [0i32; 8];
    let mut extent = vk::Extent2D { width: u32::MAX, height: u32::MAX };
    let mut samples = vk::SampleCountFlags::TYPE_1;
    let mut layers = u32::MAX;
    for i in 0..qs.rt_n {
        let r = qs.rt[i];
        let a = r.tex.and_then(|t| attach_view_layers(g, sub, qs, &t, r.view.as_ref()));
        match a {
            Some(((view, fmt, e, s, _), nl)) => {
                layers = layers.min(nl);
                colors.push(
                    vk::RenderingAttachmentInfo::default()
                        .image_view(view)
                        .image_layout(vk::ImageLayout::GENERAL)
                        .load_op(vk::AttachmentLoadOp::LOAD)
                        .store_op(vk::AttachmentStoreOp::STORE),
                );
                formats[i] = fmt.as_raw();
                extent.width = extent.width.min(e.width);
                extent.height = extent.height.min(e.height);
                samples = s;
            }
            None => colors.push(vk::RenderingAttachmentInfo::default().image_view(vk::ImageView::null())),
        }
    }
    let mut depth = None;
    let mut dfmt = 0;
    if let Some(t) = qs.rt_depth.tex {
        let dv = qs.rt_depth.view;
        if let Some(((view, fmt, e, s, kind), nl)) = attach_view_layers(g, sub, qs, &t, dv.as_ref()) {
            layers = layers.min(nl);
            depth = Some((view, kind));
            dfmt = fmt.as_raw();
            extent.width = extent.width.min(e.width);
            extent.height = extent.height.min(e.height);
            samples = s;
        }
    }
    if extent.width == u32::MAX {
        crate::log::red_once("a draw or clear with no render target bound");
        return false;
    }
    let da = depth.map(|(v, _)| {
        vk::RenderingAttachmentInfo::default().image_view(v).image_layout(vk::ImageLayout::GENERAL).load_op(vk::AttachmentLoadOp::LOAD).store_op(vk::AttachmentStoreOp::STORE)
    });
    let mut ri = vk::RenderingInfo::default()
        .render_area(vk::Rect2D { offset: vk::Offset2D::default(), extent })
        // a layered pass: as many layers as every attachment's view has (the shaders choose one per primitive)
        .layer_count(layers.clamp(1, g.d.props.limits.max_framebuffer_layers))
        .color_attachments(&colors);
    let da_arr;
    if let (Some(d), Some((_, kind))) = (da, depth) {
        da_arr = [d];
        if kind != Kind::Stencil {
            ri = ri.depth_attachment(&da_arr[0]);
        }
        if matches!(kind, Kind::DepthStencil | Kind::Stencil) {
            ri = ri.stencil_attachment(&da_arr[0]);
        }
    }
    unsafe { g.dev().cmd_begin_rendering(sub.main, &ri) };
    super::perf::inc(super::perf::C::Passes);
    super::perf::render_size(extent.width, extent.height, depth.is_some());
    qs.in_pass = true;
    qs.pass_extent = extent;
    if formats[..qs.rt_n].iter().any(|&f| f != 0) {
        qs.last_color = extent;
    }
    qs.pass_formats = (formats, qs.rt_n, dfmt, samples.as_raw() as u8);
    true
}

fn blend_factor(f: u32) -> vk::BlendFactor {
    use vk::BlendFactor as B;
    match f {
        1 => B::ZERO,
        2 => B::ONE,
        3 => B::SRC_COLOR,
        4 => B::ONE_MINUS_SRC_COLOR,
        5 => B::SRC_ALPHA,
        6 => B::ONE_MINUS_SRC_ALPHA,
        7 => B::DST_ALPHA,
        8 => B::ONE_MINUS_DST_ALPHA,
        9 => B::DST_COLOR,
        10 => B::ONE_MINUS_DST_COLOR,
        11 => B::SRC_ALPHA_SATURATE,
        16 => B::SRC1_COLOR,
        17 => B::ONE_MINUS_SRC1_COLOR,
        18 => B::SRC1_ALPHA,
        19 => B::ONE_MINUS_SRC1_ALPHA,
        97 => B::CONSTANT_COLOR,
        98 => B::ONE_MINUS_CONSTANT_COLOR,
        99 => B::CONSTANT_ALPHA,
        100 => B::ONE_MINUS_CONSTANT_ALPHA,
        _ => B::ONE,
    }
}
fn blend_op(e: u32) -> vk::BlendOp {
    match e {
        2 => vk::BlendOp::SUBTRACT,
        3 => vk::BlendOp::REVERSE_SUBTRACT,
        4 => vk::BlendOp::MIN,
        5 => vk::BlendOp::MAX,
        _ => vk::BlendOp::ADD,
    }
}
fn stencil_op_vk(o: u32) -> vk::StencilOp {
    use vk::StencilOp as S;
    match o {
        2 => S::ZERO,
        3 => S::REPLACE,
        4 => S::INCREMENT_AND_CLAMP,
        5 => S::DECREMENT_AND_CLAMP,
        6 => S::INVERT,
        7 => S::INCREMENT_AND_WRAP,
        8 => S::DECREMENT_AND_WRAP,
        _ => S::KEEP,
    }
}
fn topology(p: i32) -> vk::PrimitiveTopology {
    use vk::PrimitiveTopology as T;
    match p {
        prim::POINTS => T::POINT_LIST,
        prim::LINES => T::LINE_LIST,
        prim::LINE_LOOP | prim::LINE_STRIP => T::LINE_STRIP,
        prim::TRIANGLES | prim::QUADS => T::TRIANGLE_LIST,
        prim::TRIANGLE_STRIP | prim::QUAD_STRIP => T::TRIANGLE_STRIP,
        prim::TRIANGLE_FAN | prim::POLYGON => T::TRIANGLE_FAN,
        prim::LINES_ADJACENCY => T::LINE_LIST_WITH_ADJACENCY,
        prim::LINE_STRIP_ADJACENCY => T::LINE_STRIP_WITH_ADJACENCY,
        prim::TRIANGLES_ADJACENCY => T::TRIANGLE_LIST_WITH_ADJACENCY,
        prim::TRIANGLE_STRIP_ADJACENCY => T::TRIANGLE_STRIP_WITH_ADJACENCY,
        _ => T::PATCH_LIST,
    }
}

/// A zero-filled buffer that stands in for unbound uniform, storage and vertex bindings.
fn zero_buffer(g: &Gpu, sub: &mut Sub) -> vk::Buffer {
    static ZERO: std::sync::OnceLock<super::Buf> = std::sync::OnceLock::new();
    let mut fresh = false;
    let b = ZERO.get_or_init(|| {
        fresh = true;
        g.buffer(
            65536,
            vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            "zero",
        )
        .expect("zero buffer")
    });
    if fresh {
        unsafe { g.dev().cmd_fill_buffer(sub.pre, b.buf, 0, vk::WHOLE_SIZE, 0) };
        super::queue::full_barrier(g, sub.pre);
    }
    b.buf
}

/// The driver's constant segment c0 of a stage: storage buffer table, compute dispatch size and uniform table, base
/// instance.
fn c0_bytes(qs: &QState, st: usize, dispatch: [u32; 3], base_instance: u32) -> Vec<u8> {
    let mut v = vec![0u8; DRV_SIZE];
    // NVN stages 3 and 4 hold NVN2's task and mesh shaders' storage tables (nxnative dmsh_abi.h)
    let base = match st {
        0 => 0x110,
        5 => 0x310,
        2 => 0x410,
        1 => 0x510,
        3 => 0x670,
        4 => 0x770,
        _ => 0,
    };
    if base != 0 {
        for r in 0..16 {
            let (a, n) = qs.ssbo[st][r];
            let o = base + 0x10 * r;
            v[o..o + 8].copy_from_slice(&a.to_le_bytes());
            v[o + 8..o + 12].copy_from_slice(&(n as u32).to_le_bytes());
        }
    }
    if st == 5 {
        for (i, d) in dispatch.iter().enumerate() {
            v[0x200 + 4 * i..0x204 + 4 * i].copy_from_slice(&d.to_le_bytes());
        }
        for r in 0..16 {
            let (a, n) = qs.ubo[5][r];
            let o = 0x210 + 0x10 * r;
            if o + 12 <= 0x310 {
                v[o..o + 8].copy_from_slice(&a.to_le_bytes());
                v[o + 8..o + 12].copy_from_slice(&(n as u32).to_le_bytes());
            }
        }
    } else {
        v[0x644..0x648].copy_from_slice(&base_instance.to_le_bytes());
    }
    v
}

/// The handle table c2 of a stage: one 32-bit handle word every 8 bytes per slot.
fn c2_bytes(qs: &QState, st: usize) -> Vec<u8> {
    let mut v = vec![0u8; DRV_SIZE];
    let mut put = |o: usize, h: u32| v[o..o + 4].copy_from_slice(&h.to_le_bytes());
    for r in 0..41 {
        put(0x20 + 8 * r, qs.tex[st][r]);
    }
    for r in 0..8 {
        put(0x120 + 8 * r, qs.img[st][r]);
    }
    for r in 0..128 {
        put(0x168 + 8 * r, qs.tex[st][128 + r]);
        put(0x568 + 8 * r, qs.samp[st][r]);
    }
    v
}

/// The words of constant bank `slot` of a stage as the shader reads them: c0 / c2 as the host builds them, c1 the
/// shader's constants, c3 + r the game's uniform buffer (read from guest memory).
fn bank_word(qs: &QState, st: usize, m: &Module, c0: &[u8], c2: &[u8], slot: i32, word: u32) -> u32 {
    let at = 4 * word as usize;
    let rd = |b: &[u8]| -> u32 { b.get(at..at + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())).unwrap_or(0) };
    match slot {
        0 => rd(c0),
        -1 | 2 => rd(c2),
        1 => m.c1.as_deref().map(rd).unwrap_or(0),
        s if (3..19).contains(&s) => {
            let (a, n) = qs.ubo[st][(s - 3) as usize];
            if a == 0 || at as u64 + 4 > n {
                0
            } else {
                unsafe { core::ptr::read_unaligned((a + at as u64) as *const u32) }
            }
        }
        _ => 0,
    }
}

fn bank_u64(qs: &QState, st: usize, m: &Module, c0: &[u8], c2: &[u8], slot: i32, byte: u32) -> (u64, u64) {
    let lo = bank_word(qs, st, m, c0, c2, slot, byte / 4) as u64;
    let hi = bank_word(qs, st, m, c0, c2, slot, byte / 4 + 1) as u64;
    let n = bank_word(qs, st, m, c0, c2, slot, byte / 4 + 2) as u64;
    (lo | hi << 32, n)
}

/// The NVN target a shader's sampler type asks for (dimension | array 0x100 | multisample 0x400).
fn target_of(ty: u32) -> i32 {
    let arr = ty & 0x100 != 0;
    match ty & 0xff {
        1 => if arr { target::T1D_ARRAY } else { target::T1D },
        4 => target::T3D,
        5 => if arr { target::CUBE_ARRAY } else { target::CUBE },
        _ if ty & 0x400 != 0 => if arr { target::T2D_MS_ARRAY } else { target::T2D_MS },
        _ => if arr { target::T2D_ARRAY } else { target::T2D },
    }
}

/// The view of a registered texture ID with the type the shader declares; a stand-in when there is none.
fn tex_view(g: &Gpu, sub: &mut Sub, id: i32, ty: u32, storage: bool) -> vk::ImageView {
    if let Some(&v) = sub.memo_views.get(&(id, ty, storage)) {
        return v;
    }
    let v = tex_view_uncached(g, sub, id, ty, storage);
    sub.memo_views.insert((id, ty, storage), v);
    v
}

fn tex_view_uncached(g: &Gpu, sub: &mut Sub, id: i32, ty: u32, storage: bool) -> vk::ImageView {
    let want = target_of(ty);
    if let Some((serial, view)) = g.tables.tex_at(id) {
        let mut imgs = g.images.lock().unwrap();
        if let Some(im) = imgs.by_serial.get_mut(&serial) {
            let mut v = view.unwrap_or_default();
            let base_target = if v.bits & VW_TARGET != 0 { v.target as i32 } else { im.tex.d.target };
            // a square 2D array of whole cubes is made cube compatible (image.rs): viewed as a cube where the shader
            // samples one (environment maps kept as 2D arrays; Turbo 78ece7b)
            let first = if v.bits & VW_LAYERS != 0 { v.min_layer as u32 } else { 0 };
            let cube_ok = im.ty == vk::ImageType::TYPE_2D
                && im.extent.width == im.extent.height
                && im.layers >= 6
                && im.layers % 6 == 0
                && first + 6 <= im.layers
                && im.samples == vk::SampleCountFlags::TYPE_1;
            let compatible = match (base_target, want) {
                (a, b) if a == b => true,
                (target::T2D_ARRAY, target::CUBE | target::CUBE_ARRAY) if cube_ok => true,
                (target::T2D | target::RECTANGLE, target::T2D) => true,
                (target::T2D | target::T2D_ARRAY | target::CUBE | target::CUBE_ARRAY | target::RECTANGLE, target::T2D_ARRAY | target::T2D) => true,
                (target::CUBE_ARRAY, target::CUBE) | (target::CUBE, target::CUBE_ARRAY) => true,
                // a multisampled sampler type over a single-sample texture: the view type is the same in Vulkan
                (target::T2D | target::T2D_MS, target::T2D_MS | target::T2D) => true,
                (target::T2D_ARRAY | target::T2D_MS_ARRAY, target::T2D_MS_ARRAY | target::T2D_ARRAY) => true,
                _ => false,
            };
            if compatible && (!storage || im.usage.contains(vk::ImageUsageFlags::STORAGE)) {
                if base_target != want {
                    v.bits |= VW_TARGET;
                    v.target = want as u8;
                }
                let view = if storage { g.storage_view(im, Some(&v)) } else { g.image_view(im, Some(&v), false) };
                if let Some(view) = view {
                    return view;
                }
            } else {
                crate::log::red_once(&format!("texture ID {id}: target {base_target} bound where the shader samples target {want}"));
            }
        }
    }
    {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16 {
            crate::vklog!("texture ID {id} (target {want}{}) has no registered texture: a zero stand-in is bound", if storage { ", image" } else { "" });
        }
    }
    dummy_view(g, sub, want, storage)
}

/// 1x1 zero images standing in for unbound textures, one per view type.
fn dummy_view(g: &Gpu, sub: &mut Sub, target: i32, storage: bool) -> vk::ImageView {
    static DUMMY: std::sync::Mutex<Option<(vk::Image, vk::Image, Vec<(i32, bool, vk::ImageView)>)>> = std::sync::Mutex::new(None);
    let mut d = DUMMY.lock().unwrap();
    if d.is_none() {
        let mk = |ty: vk::ImageType, layers: u32, cube: bool| {
            let ci = vk::ImageCreateInfo::default()
                .flags(if cube { vk::ImageCreateFlags::CUBE_COMPATIBLE } else { vk::ImageCreateFlags::empty() })
                .image_type(ty)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D { width: 1, height: 1, depth: 1 })
                .mip_levels(1)
                .array_layers(layers)
                .samples(vk::SampleCountFlags::TYPE_1)
                .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST);
            let i = unsafe { g.dev().create_image(&ci, None) }.unwrap();
            let req = unsafe { g.dev().get_image_memory_requirements(i) };
            let a = g
                .alloc
                .lock()
                .unwrap()
                .allocate(&gpu_allocator::vulkan::AllocationCreateDesc { name: "dummy", requirements: req, location: MemoryLocation::GpuOnly, linear: false, allocation_scheme: gpu_allocator::vulkan::AllocationScheme::GpuAllocatorManaged })
                .unwrap();
            unsafe { g.dev().bind_image_memory(i, a.memory(), a.offset()) }.unwrap();
            std::mem::forget(a);
            i
        };
        let i2 = mk(vk::ImageType::TYPE_2D, 6, true);
        let i3 = mk(vk::ImageType::TYPE_3D, 1, false);
        for (im, layers) in [(i2, 6), (i3, 1)] {
            let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: layers };
            let b = [vk::ImageMemoryBarrier2::default()
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(im)
                .subresource_range(range)];
            unsafe {
                g.dev().cmd_pipeline_barrier2(sub.pre, &vk::DependencyInfo::default().image_memory_barriers(&b));
                g.dev().cmd_clear_color_image(sub.pre, im, vk::ImageLayout::GENERAL, &vk::ClearColorValue { float32: [0.0; 4] }, &[range]);
            }
        }
        super::queue::full_barrier(g, sub.pre);
        *d = Some((i2, i3, Vec::new()));
    }
    let (i2, i3, views) = d.as_mut().unwrap();
    if let Some(v) = views.iter().find(|v| v.0 == target && v.1 == storage) {
        return v.2;
    }
    let (img, vt, layers) = match target {
        target::T3D => (*i3, vk::ImageViewType::TYPE_3D, 1),
        target::CUBE => (*i2, vk::ImageViewType::CUBE, 6),
        target::CUBE_ARRAY => (*i2, vk::ImageViewType::CUBE_ARRAY, 6),
        target::T2D_ARRAY | target::T2D_MS_ARRAY => (*i2, vk::ImageViewType::TYPE_2D_ARRAY, 1),
        _ => (*i2, vk::ImageViewType::TYPE_2D, 1),
    };
    let mut usage = vk::ImageViewUsageCreateInfo::default().usage(if storage { vk::ImageUsageFlags::STORAGE } else { vk::ImageUsageFlags::SAMPLED });
    let ci = vk::ImageViewCreateInfo::default()
        .image(img)
        .view_type(vt)
        .format(vk::Format::R8G8B8A8_UNORM)
        .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: layers })
        .push_next(&mut usage);
    let v = unsafe { g.dev().create_image_view(&ci, None) }.unwrap();
    views.push((target, storage, v));
    v
}

fn dummy_sampler(g: &Gpu) -> vk::Sampler {
    let p = samp_par { minf: 1, magf: 1, ws: 1, wt: 1, wr: 1, cmp_func: 2, lod_max: 1000.0, aniso: 1.0, ..Default::default() };
    g.tables.sampler(&g.d, &p)
}

/// Ryujinx's support buffer as lane C's shaders read it: alpha test off, no BGRA targets, the viewport's inverse and
/// size, render scale 1.
fn support_bytes(w: f32, h: f32) -> Vec<u8> {
    let mut v = vec![0u8; 1392];
    let put = |v: &mut Vec<u8>, o: usize, f: f32| v[o..o + 4].copy_from_slice(&f.to_le_bytes());
    put(&mut v, 144, 2.0 / w.max(1.0));
    put(&mut v, 148, 2.0 / h.max(1.0));
    put(&mut v, 152, 1.0);
    put(&mut v, 160, w);
    put(&mut v, 164, h);
    put(&mut v, 168, 1.0);
    for i in 0..73 {
        for k in 0..4 {
            put(&mut v, 192 + 16 * i + 4 * k, 1.0);
        }
    }
    v
}

/// The descriptor set of one stage of the draw or dispatch.
fn stage_set(g: &Gpu, sub: &mut Sub, qs: &QState, m: &Module, dispatch: [u32; 3], base_instance: u32) -> vk::DescriptorSet {
    let st = m.stage as usize;

    let zero = zero_buffer(g, sub);
    let max_ubo = (g.d.props.limits.max_uniform_buffer_range as u64).min(65536);
    let (c0, c2, c0s, c2s) = qs.banks(g, sub, st, dispatch, base_instance);
    let (c0, c2) = (&c0[..], &c2[..]);
    let (tb, sb) = pool_bases(qs);
    let mut bufs: Vec<(u32, vk::DescriptorType, vk::DescriptorBufferInfo)> = Vec::new();
    let mut imgs: Vec<(u32, vk::DescriptorType, vk::DescriptorImageInfo)> = Vec::new();
    let mut tbufs: Vec<(u32, vk::DescriptorType, vk::BufferView)> = Vec::new();
    let dumping = passdump::active();
    let mut said: Vec<String> = Vec::new();
    for (b, r) in &m.res {
        let b = *b;
        match r {
            Res::Support => {
                let (wi, hi) = (qs.pass_extent.width.max(1), qs.pass_extent.height.max(1));
                let (buf, off) = qs.staged(g, sub, 0, (wi as u64) << 32 | hi as u64, || support_bytes(wi as f32, hi as f32).to_vec());
                bufs.push((b, vk::DescriptorType::UNIFORM_BUFFER, vk::DescriptorBufferInfo { buffer: buf, offset: off, range: 1392 }));
            }
            Res::Cbuf { bank } => {
                let info = match *bank {
                    0 => vk::DescriptorBufferInfo { buffer: c0s.0, offset: c0s.1, range: DRV_SIZE as u64 },
                    1 => match &m.c1 {
                        Some(c1) if !c1.is_empty() => {
                            let (buf, off) = qs.staged(g, sub, 1, c1.as_ptr() as u64 ^ (c1.len() as u64) << 48, || c1.to_vec());
                            vk::DescriptorBufferInfo { buffer: buf, offset: off, range: (c1.len() as u64).min(max_ubo) }
                        }
                        _ => vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: max_ubo },
                    },
                    2 => vk::DescriptorBufferInfo { buffer: c2s.0, offset: c2s.1, range: DRV_SIZE as u64 },
                    24 => {
                        let grp = (st == 5) as usize;
                        let (buf, off) = qs.staged(g, sub, 2 + grp as u8, qs.pgen, || qs.push[grp][..].to_vec());
                        vk::DescriptorBufferInfo { buffer: buf, offset: off, range: 0x800 }
                    }
                    k @ 25..=29 => {
                        // graphics stage i reads its push block 1 + i through its own bank (NVN2)
                        let b = (k - 24) as usize * 0x100;
                        let (buf, off) = qs.staged(g, sub, k as u8, qs.pgen, || qs.push[0][b..b + 0x100].to_vec());
                        vk::DescriptorBufferInfo { buffer: buf, offset: off, range: 0x100 }
                    }
                    k @ 3..=18 => {
                        let (a, n) = qs.ubo[st][(k - 3) as usize];
                        if dumping && a != 0 && n >= 64 && super::mem::room(a) >= 64 {
                            // SAFETY: guest memory inside a registered pool (identity mapped)
                            let f = unsafe { core::slice::from_raw_parts(a as *const f32, 16) };
                            said.push(format!("c{k}@{a:#x}+{n:#x}={:?}", f.iter().map(|v| (v * 1000.0).round() / 1000.0).collect::<Vec<_>>()));
                        } else if dumping {
                            said.push(format!("c{k}@{a:#x}+{n:#x}"));
                        }
                        match if a != 0 && n != 0 { super::mem::resolve(g, sub, a, n.min(max_ubo)) } else { None } {
                            Some((buf, off, _)) => {
                                if off % g.d.caps.uniform_align != 0 {
                                    crate::log::red_once(&format!("uniform buffer at {a:#x}: offset not aligned to {}", g.d.caps.uniform_align));
                                }
                                vk::DescriptorBufferInfo { buffer: buf, offset: off, range: n.min(max_ubo) }
                            }
                            None => {
                                if a != 0 {
                                    let k: String = m.key.iter().map(|b| format!("{b:02x}")).collect();
                                    crate::log::red_once(&format!("shader {k} (stage {st}): uniform bank c{k2} at {a:#x} (+{n:#x}) is in no memory pool: zeros bound", k2 = *bank));
                                }
                                vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: max_ubo }
                            }
                        }
                    }
                    _ => vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: max_ubo },
                };
                bufs.push((b, vk::DescriptorType::UNIFORM_BUFFER, info));
            }
            Res::Sbuf { cb_slot, cb_off, written } => {
                let (a, mut n) = bank_u64(qs, st, m, &c0, &c2, *cb_slot, *cb_off);
                // A buffer bound with size 0 is still the memory at its address: an NVN shader holds the address and
                // reads and writes through it, the size is only what the driver tells it. Donkey Kong Bananza binds the
                // list its depth-sampling dispatch appends to this way. It runs to the end of its pool, at most 1 MB.
                if a != 0 && n == 0 {
                    n = super::mem::room(a).min(1 << 20);
                    super::perf::inc(super::perf::C::SbufSize0);
                    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    if n > 0 && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::vklog!("a storage buffer bound with size 0 ({a:#x}): bound to the end of its memory pool, {n} bytes (later ones are not logged)");
                    }
                }
                if *written && st < 5 && a != 0 {
                    append::bound(a);
                }
                let info = match if a != 0 && n != 0 { super::mem::resolve(g, sub, a, n) } else { None } {
                    Some((buf, off, _)) => {
                        if *written {
                            super::mem::note_gpu_write(g, sub, a, n);
                        }
                        if off % g.d.caps.storage_align != 0 {
                            crate::log::red_once(&format!("storage buffer at {a:#x}: offset not aligned to {}", g.d.caps.storage_align));
                        }
                        vk::DescriptorBufferInfo { buffer: buf, offset: off, range: n }
                    }
                    None => vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: 65536 },
                };
                bufs.push((b, vk::DescriptorType::STORAGE_BUFFER, info));
            }
            Res::Tex { handle, cbuf_slot, ty } | Res::Img { handle, cbuf_slot, ty } => {
                let is_img = matches!(r, Res::Img { .. });
                let word = bank_word(qs, st, m, &c0, &c2, *cbuf_slot, handle & 0x3fff);
                let kind = handle >> 28;
                let tid = (word & 0xfffff) as i32 + tb;
                if *ty == 2 {
                    // a buffer texture / buffer image: a texel buffer view over its memory
                    let dt = if is_img { vk::DescriptorType::STORAGE_TEXEL_BUFFER } else { vk::DescriptorType::UNIFORM_TEXEL_BUFFER };
                    if let Some(t) = g.tables.buf_tex_at(tid) {
                        if let Some(v) = g.texel_view(sub, &t, is_img) {
                            if is_img {
                                super::mem::note_gpu_write(g, sub, t.mem as u64, t.d.width as u64 * crate::tex::layout::fmt_bpb(t.d.format).0 as u64);
                            }
                            tbufs.push((b, dt, v));
                        }
                    }
                    continue;
                }
                let sid = match kind {
                    0 => (word >> 20) as i32,
                    1 => (bank_word(qs, st, m, &c0, &c2, *cbuf_slot, handle >> 14 & 0x3fff) >> 20) as i32,
                    2 => (handle >> 14 & 0x3fff) as i32,
                    _ => {
                        crate::log::red_once("constant sampler handles are not bound yet");
                        0
                    }
                } + sb;
                let view = super::perf::timed(super::perf::C::TexViewNs, || tex_view(g, sub, tid, *ty, is_img));
                if dumping {
                    let what = g
                        .tables
                        .tex_at(tid)
                        .and_then(|(serial, v)| {
                            g.images.lock().unwrap().by_serial.get(&serial).map(|im| {
                                let d = &im.tex.d;
                                format!("ser{serial} fmt{:#x} {}x{}x{} t{} v{:?}", d.format, d.width, d.height, d.depth, d.target, v.map(|v| (v.bits, v.target, v.base_level, v.min_layer)))
                            })
                        })
                        .unwrap_or_else(|| "none".into());
                    said.push(format!("{}{b}=id{tid}/s{sid} ty{ty}: {what}", if is_img { "img" } else { "tex" }));
                }
                if is_img {
                    imgs.push((b, vk::DescriptorType::STORAGE_IMAGE, vk::DescriptorImageInfo { sampler: vk::Sampler::null(), image_view: view, image_layout: vk::ImageLayout::GENERAL }));
                } else {
                    let s = g.tables.samp_at(sid).unwrap_or_else(|| dummy_sampler(g));
                    imgs.push((b, vk::DescriptorType::COMBINED_IMAGE_SAMPLER, vk::DescriptorImageInfo { sampler: s, image_view: view, image_layout: vk::ImageLayout::GENERAL }));
                }
            }
            Res::Pool => {}
            Res::MeshDraw => {
                let info = match qs.mesh_rec {
                    Some((buf, off, range)) => vk::DescriptorBufferInfo { buffer: buf, offset: off, range },
                    None => {
                        crate::log::red_once("a task / mesh shader's draw records are not bound (a draw that is not a mesh draw)");
                        vk::DescriptorBufferInfo { buffer: zero, offset: 0, range: 65536 }
                    }
                };
                bufs.push((b, vk::DescriptorType::STORAGE_BUFFER, info));
            }
        }
    }
    if dumping {
        let k: String = m.key.iter().map(|b| format!("{b:02x}")).collect();
        crate::vklog!("pass dump {}: stage {st} {k}: {}", passdump::pass_no(), said.join("; "));
    }
    // the same writes for the same stage layout earlier in this submission: that set is bound again
    let mut key: Vec<u8> = Vec::with_capacity(16 + 32 * (bufs.len() + imgs.len() + tbufs.len()));
    key.extend_from_slice(&(st as u64).to_le_bytes());
    for (b, ty, i) in &bufs {
        key.extend_from_slice(&[*b as u8, ty.as_raw() as u8]);
        key.extend_from_slice(&vk::Handle::as_raw(i.buffer).to_le_bytes());
        key.extend_from_slice(&i.offset.to_le_bytes());
        key.extend_from_slice(&i.range.to_le_bytes());
    }
    for (b, ty, i) in &imgs {
        key.extend_from_slice(&[*b as u8, ty.as_raw() as u8]);
        key.extend_from_slice(&vk::Handle::as_raw(i.image_view).to_le_bytes());
        key.extend_from_slice(&vk::Handle::as_raw(i.sampler).to_le_bytes());
    }
    for (b, ty, v) in &tbufs {
        key.extend_from_slice(&[*b as u8, ty.as_raw() as u8]);
        key.extend_from_slice(&vk::Handle::as_raw(*v).to_le_bytes());
    }
    let kh = super::queue::memo_hash(&key);
    if let Some(&set) = sub.memo_sets.get(&kh) {
        return set;
    }
    let set = sub.alloc_set(g, g.layout.stage_sets[st]);
    super::perf::inc(super::perf::C::DescSets);
    sub.memo_sets.insert(kh, set);
    let mut writes: Vec<vk::WriteDescriptorSet> = bufs
        .iter()
        .map(|(b, ty, i)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*b).descriptor_type(*ty).buffer_info(std::slice::from_ref(i)))
        .collect();
    writes.extend(imgs.iter().map(|(b, ty, i)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*b).descriptor_type(*ty).image_info(std::slice::from_ref(i))));
    writes.extend(tbufs.iter().map(|(b, ty, v)| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(*b).descriptor_type(*ty).texel_buffer_view(std::slice::from_ref(v))));
    unsafe { g.dev().update_descriptor_sets(&writes, &[]) };
    set
}

fn pool_bases(qs: &QState) -> (i32, i32) {
    let tb = if qs.tex_pool != 0 { crate::nvn::pool::dpool_base_of(qs.tex_pool as *const dpool_t, true) } else { 0 };
    let sb = if qs.samp_pool != 0 { crate::nvn::pool::dpool_base_of(qs.samp_pool as *const dpool_t, false) } else { 0 };
    (tb, sb)
}

fn push_constants(g: &Gpu, sub: &Sub, qs: &QState, flipped: bool) {
    let (tb, sb) = pool_bases(qs);
    let pc: [u32; 4] = [tb as u32, sb as u32, flipped as u32, qs.mesh_stride];
    let bytes = unsafe { core::slice::from_raw_parts(pc.as_ptr() as *const u8, 16) };
    unsafe { g.dev().cmd_push_constants(sub.main, g.layout.pipeline, g.layout.push_stages, 0, bytes) };
}

fn bind_sets(g: &Gpu, sub: &mut Sub, qs: &QState, mods: &[Arc<Module>], bp: vk::PipelineBindPoint, dispatch: [u32; 3], base_instance: u32) {
    let mut pool = false;
    for m in mods {
        let set = super::perf::timed(super::perf::C::SetNs, || stage_set(g, sub, qs, m, dispatch, base_instance));
        unsafe { g.dev().cmd_bind_descriptor_sets(sub.main, bp, g.layout.pipeline, m.stage, &[set], &[]) };
        pool |= m.uses_pool;
    }
    if pool {
        unsafe { g.dev().cmd_bind_descriptor_sets(sub.main, bp, g.layout.pipeline, L::POOL_SET, &[g.tables.set], &[]) };
    }
}

/// Alpha to coverage as the game sets it; the control command `a2c 0` (or `NXGFX_A2C=0`) turns it off live
/// (diagnostics: dithered foliage and hair).
pub static A2C: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// `fps_target`'s frame skip (nxconfig), read again when the settings change.
fn frame_skip() -> u32 {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    static GEN: AtomicU64 = AtomicU64::new(0);
    static V: AtomicU32 = AtomicU32::new(0);
    let g = nxconfig::generation();
    if GEN.swap(g, Ordering::Relaxed) != g {
        V.store(nxconfig::frame_skip(), Ordering::Relaxed);
    }
    V.load(Ordering::Relaxed)
}

/// The `shadows` option (nxconfig), read again when the settings change.
fn shadows_on() -> bool {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static GEN: AtomicU64 = AtomicU64::new(0);
    static ON: AtomicBool = AtomicBool::new(true);
    let g = nxconfig::generation();
    if GEN.swap(g, Ordering::Relaxed) != g {
        ON.store(nxconfig::flag("shadows"), Ordering::Relaxed);
    }
    ON.load(Ordering::Relaxed)
}

/// Everything a draw needs bound; false when it cannot be drawn (it is skipped).
fn draw_setup(g: &Gpu, sub: &mut Sub, qs: &mut QState, prim_ty: i32, base_instance: u32) -> bool {
    draw_setup_ex(g, sub, qs, prim_ty, base_instance, false)
}

/// As `draw_setup`; `mesh`: a task / mesh draw (NVN2), its program's task and mesh shaders in places 3 and 4.
fn draw_setup_ex(g: &Gpu, sub: &mut Sub, qs: &mut QState, prim_ty: i32, base_instance: u32, mesh: bool) -> bool {
    if !qs.missing.is_empty() {
        // shaders the build did not translate: wait for the live translator (once per set), then bind what came
        let keys: Vec<_> = qs.missing.iter().map(|m| m.0).collect();
        let dev = g.dev();
        super::live::wait_for(&keys, || keys.iter().all(|k| g.shaders.get_ex(dev, k, true).is_some()));
        let missing = std::mem::take(&mut qs.missing);
        for (k, stages) in missing {
            match g.shaders.get(dev, &k) {
                Some(m) if stages & (1 << m.stage) != 0 => {
                    let s = m.stage as usize;
                    qs.prog[s] = Some(m);
                }
                Some(_) => {}
                None => qs.missing.push((k, stages)),
            }
        }
    }
    let first = if mesh { qs.prog[4].clone().filter(|m| m.mesh == 2) } else { qs.prog[0].clone() };
    let Some(vs) = first else {
        crate::log::red_once(if mesh { "a mesh draw with no mesh shader bound" } else { "draw with no vertex shader bound" });
        super::perf::inc(super::perf::C::DrawsSkipped);
        super::perf::inc(super::perf::C::SkipNoShader);
        return false;
    };
    if !super::perf::timed(super::perf::C::PassNs, || begin_pass(g, sub, qs)) {
        super::perf::inc(super::perf::C::DrawsSkipped);
        super::perf::inc(super::perf::C::SkipNoTarget);
        return false;
    }
    if !qs.render_on {
        super::perf::inc(super::perf::C::DrawsSkipped);
        return false;
    }
    // fps_target 30 / 20: the game keeps its 60 steps a second, only every 2nd / 3rd frame is drawn
    let skip = frame_skip();
    if skip > 0 && super::present::frames() % (skip as u64 + 1) != 0 {
        super::perf::inc(super::perf::C::DrawsSkipped);
        return false;
    }
    // shadows=0: a depth-only pass of a size other than the last colour pass's is a shadow map; its draws are skipped
    if !shadows_on() && qs.pass_formats.0[..qs.pass_formats.1].iter().all(|&f| f == 0) && qs.pass_formats.2 != 0 && qs.pass_extent != qs.last_color {
        super::perf::inc(super::perf::C::DrawsSkipped);
        return false;
    }
    if qs.vis_on && qs.occl_q.is_none() && sub.occl_n < 256 {
        let q = sub.occl_n;
        sub.occl_n += 1;
        let flags = if g.d.caps.occlusion_precise { vk::QueryControlFlags::PRECISE } else { vk::QueryControlFlags::empty() };
        unsafe { g.dev().cmd_begin_query(sub.main, sub.occl, q, flags) };
        qs.occl_q = Some(q);
    }
    // a mesh draw: the task (if any) and mesh shaders and the pixel shader; any other draw leaves task and mesh shaders out
    let mods: Vec<Arc<Module>> = if mesh {
        let mut v: Vec<Arc<Module>> = [3usize, 4, 1].iter().filter_map(|&s| qs.prog[s].clone()).filter(|m| m.stage == 1 || m.mesh != 0).collect();
        // the pixel shader reads the mesh shader's per-primitive outputs per primitive
        let locs = v.iter().find(|m| m.mesh == 2).map(|m| m.per_primitive.clone()).unwrap_or_default();
        if !locs.is_empty() {
            if let Some(i) = v.iter().position(|m| m.stage == 1) {
                if let Some(pp) = g.shaders.per_primitive(g.dev(), &v[i].key, &locs) {
                    v[i] = pp;
                }
            }
        }
        v
    } else {
        [0usize, 1, 2, 3, 4].iter().filter_map(|&s| qs.prog[s].clone()).filter(|m| m.mesh == 0).collect()
    };
    // transform feedback: the last vertex-processing stage's capture variant, the capture opened around the draw
    let mut mods = mods;
    let mut xfb_layout = None;
    if qs.tf_on && !mesh && g.d.caps.xfb {
        let last = if mods.iter().any(|m| m.stage == 2) { 2 } else if mods.iter().any(|m| m.stage == 4) { 4 } else { 0 };
        if let Some(i) = mods.iter().position(|m| m.stage == last) {
            match super::xfb::variant_module(g.dev(), &mods[i]) {
                Some(v) => {
                    xfb_layout = mods[i].xfb.clone();
                    mods[i] = v;
                }
                None if mods[i].xfb.is_none() => crate::log::red_once("transform feedback on, but the vertex stage's reflection names no capture: not written"),
                None => {}
            }
        }
    }
    let mut key = super::pipe::GfxKey::default();
    for m in &mods {
        key.shaders[m.stage as usize] = m.key;
    }
    let (formats, n, dfmt, samples) = qs.pass_formats;
    key.colors = formats;
    key.ncolor = n as u8;
    key.depth = dfmt;
    key.samples = samples;
    let Some(pipe) = super::perf::timed(super::perf::C::PipeLookNs, || g.gfx_pipeline(&key, &mods)) else {
        super::perf::inc(super::perf::C::DrawsSkipped);
        super::perf::inc(super::perf::C::SkipNoPipeline);
        return false;
    };
    super::perf::inc(super::perf::C::Draws);
    let dev = g.dev();
    let cb = sub.main;
    unsafe { dev.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipe) };
    bind_sets(g, sub, qs, &mods, vk::PipelineBindPoint::GRAPHICS, [0; 3], base_instance);

    // viewport and origin
    let o = if qs.vp_origin != 0 { qs.vp_origin == 2 } else { !qs.origin_ll };
    let upper_left = o;
    push_constants(g, sub, qs, upper_left);
    let [x, y, w, h] = qs.vp[0];
    let (w, h) = if w == 0.0 || h == 0.0 { (qs.pass_extent.width as f32, qs.pass_extent.height as f32) } else { (w, h) };
    let vp = if upper_left {
        vk::Viewport { x, y: y + h, width: w, height: -h, min_depth: qs.depth_range[0].clamp(0.0, 1.0), max_depth: qs.depth_range[1].clamp(0.0, 1.0) }
    } else {
        vk::Viewport { x, y, width: w, height: h, min_depth: qs.depth_range[0].clamp(0.0, 1.0), max_depth: qs.depth_range[1].clamp(0.0, 1.0) }
    };
    let sc = match qs.scissor {
        Some([sx, sy, sw, sh]) => vk::Rect2D { offset: vk::Offset2D { x: sx.max(0), y: sy.max(0) }, extent: vk::Extent2D { width: sw.max(0) as u32, height: sh.max(0) as u32 } },
        None => vk::Rect2D { offset: vk::Offset2D::default(), extent: qs.pass_extent },
    };
    let ds = qs.ds;
    let poly = qs.poly;
    unsafe {
        dev.cmd_set_viewport_with_count(cb, &[vp]);
        dev.cmd_set_scissor_with_count(cb, &[sc]);
        if qs.line_width != 1.0 {
            crate::log::red_once("line widths other than 1 are drawn 1 pixel wide");
        }
        dev.cmd_set_line_width(cb, 1.0);
        let off_on = poly.offset_en() & (1 << poly.mode().min(2)) != 0;
        dev.cmd_set_depth_bias_enable(cb, off_on);
        dev.cmd_set_depth_bias(cb, qs.poly_off[1], qs.poly_off[2], qs.poly_off[0]);
        dev.cmd_set_blend_constants(cb, &qs.blend_color);
        dev.cmd_set_depth_bounds(cb, qs.depth_bounds.1.clamp(0.0, 1.0), qs.depth_bounds.2.clamp(0.0, 1.0));
        dev.cmd_set_depth_bounds_test_enable(cb, qs.depth_bounds.0 && g.d.caps.depth_bounds);
        for (i, f) in [vk::StencilFaceFlags::FRONT, vk::StencilFaceFlags::BACK].iter().enumerate() {
            dev.cmd_set_stencil_compare_mask(cb, *f, qs.stencil[i][0]);
            dev.cmd_set_stencil_write_mask(cb, *f, qs.stencil[i][1]);
            dev.cmd_set_stencil_reference(cb, *f, qs.stencil[i][2]);
        }
        dev.cmd_set_cull_mode(cb, vk::CullModeFlags::from_raw(poly.cull()));
        // NVN CW = 0 / CCW = 1; Vulkan COUNTER_CLOCKWISE = 0 / CLOCKWISE = 1. Unflipped, the winding inverts.
        let ccw = poly.front() == front_face::CCW;
        let vk_ccw = if upper_left { ccw } else { !ccw };
        dev.cmd_set_front_face(cb, if vk_ccw { vk::FrontFace::COUNTER_CLOCKWISE } else { vk::FrontFace::CLOCKWISE });
        if !mesh {
            dev.cmd_set_primitive_topology(cb, topology(prim_ty));
        }
        dev.cmd_set_depth_test_enable(cb, ds.get(ds_field::DT) != 0);
        dev.cmd_set_depth_write_enable(cb, ds.get(ds_field::DW) != 0);
        dev.cmd_set_depth_compare_op(cb, super::tables::compare_op(ds.get(ds_field::DF)));
        dev.cmd_set_stencil_test_enable(cb, ds.get(ds_field::ST) != 0);
        dev.cmd_set_stencil_op(
            cb,
            vk::StencilFaceFlags::FRONT,
            stencil_op_vk(ds.get(ds_field::FAIL_F)),
            stencil_op_vk(ds.get(ds_field::ZPASS_F)),
            stencil_op_vk(ds.get(ds_field::ZFAIL_F)),
            super::tables::compare_op(ds.get(ds_field::SF_F)),
        );
        dev.cmd_set_stencil_op(
            cb,
            vk::StencilFaceFlags::BACK,
            stencil_op_vk(ds.get(ds_field::FAIL_B)),
            stencil_op_vk(ds.get(ds_field::ZPASS_B)),
            stencil_op_vk(ds.get(ds_field::ZFAIL_B)),
            super::tables::compare_op(ds.get(ds_field::SF_B)),
        );
        dev.cmd_set_rasterizer_discard_enable(cb, qs.rast_discard);
        if !mesh {
            dev.cmd_set_primitive_restart_enable(cb, qs.prim_restart.0 && !matches!(prim_ty, prim::POINTS | prim::LINES | prim::TRIANGLES | prim::QUADS | prim::PATCHES));
        }
        let caps = g.d.caps;
        if caps.logic_op_dyn {
            g.d.eds2.cmd_set_logic_op(cb, vk::LogicOp::from_raw(qs.color.logic_op() as i32));
        }
        if caps.patch_dyn && !mesh && mods.iter().any(|m| m.stage == 3 || m.stage == 4) {
            g.d.eds2.cmd_set_patch_control_points(cb, qs.patch.max(1));
        }
        let e3 = &g.d.eds3;
        if caps.eds3_poly {
            e3.cmd_set_polygon_mode(cb, match poly.mode() { 0 => vk::PolygonMode::POINT, 1 => vk::PolygonMode::LINE, _ => vk::PolygonMode::FILL });
        }
        let sflag = vk::SampleCountFlags::from_raw(samples.max(1) as u32);
        if caps.eds3_samples {
            e3.cmd_set_rasterization_samples(cb, sflag);
        }
        if caps.eds3_sample_mask {
            e3.cmd_set_sample_mask(cb, sflag, &[qs.sample_mask]);
        }
        if caps.eds3_a2c {
            e3.cmd_set_alpha_to_coverage_enable(cb, qs.ms.a2c != 0 && A2C.load(std::sync::atomic::Ordering::Relaxed));
        }
        if caps.eds3_logic_en {
            e3.cmd_set_logic_op_enable(cb, qs.color.logic_op() != logic_op::COPY);
        }
        if caps.eds3_clamp {
            e3.cmd_set_depth_clamp_enable(cb, qs.depth_clamp);
        }
        if caps.eds3_clip_neg {
            e3.cmd_set_depth_clip_negative_one_to_one(cb, !crate::nvn::device::DEPTH_ZERO.load(std::sync::atomic::Ordering::Relaxed));
        }
        if n > 0 {
            let en: Vec<vk::Bool32> = (0..n).map(|i| (qs.color.blend_en() >> i & 1) as vk::Bool32).collect();
            let eq: Vec<vk::ColorBlendEquationEXT> = (0..n)
                .map(|i| {
                    let b = qs.blend[i];
                    vk::ColorBlendEquationEXT {
                        src_color_blend_factor: blend_factor(b.get(blend_field::SRC_RGB)),
                        dst_color_blend_factor: blend_factor(b.get(blend_field::DST_RGB)),
                        color_blend_op: blend_op(b.get(blend_field::EQ_RGB)),
                        src_alpha_blend_factor: blend_factor(b.get(blend_field::SRC_A)),
                        dst_alpha_blend_factor: blend_factor(b.get(blend_field::DST_A)),
                        alpha_blend_op: blend_op(b.get(blend_field::EQ_A)),
                    }
                })
                .collect();
            // the channel mask, and only the components the pixel shader's output map says it writes (a component it does
            // not write keeps the target's value, as on the GPU: the translator stores nothing there)
            let omap = qs.prog[1].as_ref().and_then(|m| m.omap).unwrap_or(u32::MAX);
            let wm: Vec<vk::ColorComponentFlags> = (0..n).map(|i| vk::ColorComponentFlags::from_raw(qs.chmask >> (4 * i) & omap >> (4 * i) & 0xf)).collect();
            e3.cmd_set_color_blend_enable(cb, 0, &en);
            e3.cmd_set_color_blend_equation(cb, 0, &eq);
            e3.cmd_set_color_write_mask(cb, 0, &wm);
        }
    }
    if qs.color.alpha_func() != cmp::ALWAYS && qs.color.alpha_func() != 0 {
        crate::log::red_once("alpha test is not implemented yet (it belongs in the translated fragment shader)");
    }
    if !mesh {
        super::perf::timed(super::perf::C::VtxNs, || vertex_input(g, sub, qs, &vs));
    }
    if let Some(l) = xfb_layout {
        tf_begin(g, sub, qs, &l);
        super::perf::inc(super::perf::C::TfCaptures);
    }
    true
}

fn vertex_input(g: &Gpu, sub: &mut Sub, qs: &QState, vs: &Module) {
    let mut binds: Vec<vk::VertexInputBindingDescription2EXT> = Vec::new();
    let mut attrs: Vec<vk::VertexInputAttributeDescription2EXT> = Vec::new();
    let mut streams_used = 0u32;
    let mut need_zero = false;
    for &(loc, kind) in &vs.inputs {
        let a = va_st(if (loc as usize) < qs.nvattr { qs.vattr[loc as usize] } else { 0 });
        if a.fmt() != 0 {
            let s = a.stream().min(15);
            streams_used |= 1 << s;
            attrs.push(vk::VertexInputAttributeDescription2EXT::default().location(loc).binding(s).format(format::vertex_format(a.fmt() as i32)).offset(a.offset()));
        } else {
            need_zero = true;
            let f = match kind {
                InKind::Float => vk::Format::R32G32B32A32_SFLOAT,
                InKind::Sint => vk::Format::R32G32B32A32_SINT,
                InKind::Uint => vk::Format::R32G32B32A32_UINT,
            };
            attrs.push(vk::VertexInputAttributeDescription2EXT::default().location(loc).binding(31).format(f).offset(0));
        }
    }
    let mut bufs: Vec<(u32, vk::Buffer, u64)> = Vec::new();
    for s in 0..16u32 {
        if streams_used & (1 << s) == 0 {
            continue;
        }
        let st = qs.vstream[s as usize];
        let (rate, div) = if st.divisor == 0 { (vk::VertexInputRate::VERTEX, 1) } else { (vk::VertexInputRate::INSTANCE, st.divisor) };
        let div = if div > 1 && !g.d.caps.divisor {
            crate::log::red_once("vertex divisor > 1 without VK_EXT_vertex_attribute_divisor");
            1
        } else {
            div
        };
        binds.push(vk::VertexInputBindingDescription2EXT::default().binding(s).stride(st.stride).input_rate(rate).divisor(div));
        let (a, n) = qs.vbuf[s as usize];
        match if a != 0 { super::mem::resolve(g, sub, a, n.max(1)) } else { None } {
            Some((b, o, _)) => bufs.push((s, b, o)),
            None => {
                let z = zero_buffer(g, sub);
                bufs.push((s, z, 0));
            }
        }
    }
    if need_zero {
        binds.push(vk::VertexInputBindingDescription2EXT::default().binding(31).stride(0).input_rate(vk::VertexInputRate::VERTEX).divisor(1));
        let z = zero_buffer(g, sub);
        bufs.push((31, z, 0));
    }
    unsafe {
        g.d.vin.cmd_set_vertex_input(sub.main, &binds, &attrs);
        for (s, b, o) in bufs {
            g.dev().cmd_bind_vertex_buffers(sub.main, s, &[b], &[o]);
        }
    }
}

/// Index data of a quads draw as triangles (u32), from a vertex range or from the game's index buffer.
fn quads_indices(n: u32, src: Option<(&[u8], i32)>) -> Vec<u32> {
    let get = |i: u32| -> u32 {
        match src {
            None => i,
            Some((b, ty)) => match ty {
                index_type::U8 => b.get(i as usize).copied().unwrap_or(0) as u32,
                index_type::U16 => u16::from_le_bytes([b[2 * i as usize], b[2 * i as usize + 1]]) as u32,
                _ => u32::from_le_bytes(b[4 * i as usize..4 * i as usize + 4].try_into().unwrap()),
            },
        }
    };
    let mut v = Vec::with_capacity((n / 4 * 6) as usize);
    for q in 0..n / 4 {
        let b = q * 4;
        v.extend_from_slice(&[get(b), get(b + 1), get(b + 2), get(b), get(b + 2), get(b + 3)]);
    }
    v
}

fn draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_draw) {
    let inst = if d.instances <= 0 { 1 } else { d.instances as u32 };
    append::draw(g, sub, qs, d.ind, false);
    if !draw_setup(g, sub, qs, d.prim, d.base_instance as u32) {
        return;
    }
    let dev = g.dev();
    if d.ind != 0 {
        if let Some((b, o, _)) = super::mem::resolve(g, sub, d.ind, 16) {
            unsafe { dev.cmd_draw_indirect(sub.main, b, o, 1, 16) };
        }
        return;
    }
    if d.prim == prim::QUADS {
        let idx = quads_indices(d.count as u32, None);
        let bytes = unsafe { core::slice::from_raw_parts(idx.as_ptr() as *const u8, idx.len() * 4) };
        let (b, o) = sub.stage_bytes(g, bytes);
        unsafe {
            dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT32);
            dev.cmd_draw_indexed(sub.main, idx.len() as u32, inst, 0, d.first, d.base_instance as u32);
        }
        return;
    }
    if d.prim == prim::LINE_LOOP {
        crate::log::red_once("LINE_LOOP drawn as a line strip (the closing segment is missing)");
    }
    unsafe { dev.cmd_draw(sub.main, d.count as u32, inst, d.first as u32, d.base_instance as u32) };
}

fn draw_idx(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_draw_idx) {
    let inst = if d.instances <= 0 { 1 } else { d.instances as u32 };
    let isz = match d.itype {
        index_type::U8 => 1u64,
        index_type::U16 => 2,
        _ => 4,
    };
    append::draw(g, sub, qs, d.ind, true);
    if !draw_setup(g, sub, qs, d.prim, d.base_instance as u32) {
        return;
    }
    let dev = g.dev();
    if d.prim == prim::QUADS && d.ind == 0 {
        let src = unsafe { core::slice::from_raw_parts(d.ibuf as *const u8, (d.count as u64 * isz) as usize) };
        let idx = quads_indices(d.count as u32, Some((src, d.itype)));
        let bytes = unsafe { core::slice::from_raw_parts(idx.as_ptr() as *const u8, idx.len() * 4) };
        let (b, o) = sub.stage_bytes(g, bytes);
        unsafe {
            dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT32);
            dev.cmd_draw_indexed(sub.main, idx.len() as u32, inst, 0, d.base_vertex, d.base_instance as u32);
        }
        return;
    }
    let ity = match d.itype {
        index_type::U8 if g.d.caps.index_u8 => vk::IndexType::UINT8_EXT,
        index_type::U16 => vk::IndexType::UINT16,
        index_type::U32 => vk::IndexType::UINT32,
        _ => {
            // u8 indices widened to u16 on the CPU
            let src = unsafe { core::slice::from_raw_parts(d.ibuf as *const u8, d.count.max(0) as usize) };
            let w: Vec<u16> = src.iter().map(|&b| b as u16).collect();
            let bytes = unsafe { core::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2) };
            let (b, o) = sub.stage_bytes(g, bytes);
            unsafe {
                dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT16);
                dev.cmd_draw_indexed(sub.main, d.count as u32, inst, 0, d.base_vertex, d.base_instance as u32);
            }
            return;
        }
    };
    if qs.prim_restart.0 {
        let all = match d.itype {
            index_type::U8 => 0xff,
            index_type::U16 => 0xffff,
            _ => u32::MAX,
        };
        if qs.prim_restart.1 != all && qs.prim_restart.1 != u32::MAX {
            crate::log::red_once("primitive restart with an index other than all ones");
        }
    }
    let n = if d.ind != 0 { 1u64 << 20 } else { d.count.max(0) as u64 * isz };
    let Some((ib, io, _)) = super::mem::resolve(g, sub, d.ibuf, n.max(1).min(1 << 20)) else { return };
    unsafe {
        dev.cmd_bind_index_buffer(sub.main, ib, io, ity);
        if d.ind != 0 {
            if let Some((b, o, _)) = super::mem::resolve(g, sub, d.ind, 20) {
                dev.cmd_draw_indexed_indirect(sub.main, b, o, 1, 20);
            }
        } else {
            dev.cmd_draw_indexed(sub.main, d.count as u32, inst, 0, d.base_vertex, d.base_instance as u32);
        }
    }
}

fn dispatch(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_dispatch) {
    end_pass(g, sub, qs);
    let Some(cs) = qs.prog[5].clone() else {
        crate::log::red_once("dispatch with no compute shader bound");
        return;
    };
    let Some(pipe) = g.compute_pipeline(&cs) else { return };
    super::perf::inc(super::perf::C::Dispatches);
    unsafe { g.dev().cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, pipe) };
    bind_sets(g, sub, qs, std::slice::from_ref(&cs), vk::PipelineBindPoint::COMPUTE, [d.x as u32, d.y as u32, d.z as u32], 0);
    push_constants(g, sub, qs, false);
    unsafe {
        if d.ind != 0 {
            if let Some((b, o, _)) = super::mem::resolve(g, sub, d.ind, 12) {
                g.dev().cmd_dispatch_indirect(sub.main, b, o);
            }
        } else {
            g.dev().cmd_dispatch(sub.main, d.x.max(0) as u32, d.y.max(0) as u32, d.z.max(0) as u32);
        }
    }
    barrier(g, sub);
}

fn clear_rect(qs: &QState) -> vk::ClearRect {
    let full = vk::Rect2D { offset: vk::Offset2D::default(), extent: qs.pass_extent };
    let rect = match qs.scissor {
        Some([x, y, w, h]) => {
            let x0 = x.max(0) as u32;
            let y0 = y.max(0) as u32;
            let x1 = ((x + w).max(0) as u32).min(qs.pass_extent.width);
            let y1 = ((y + h).max(0) as u32).min(qs.pass_extent.height);
            vk::Rect2D { offset: vk::Offset2D { x: x0 as i32, y: y0 as i32 }, extent: vk::Extent2D { width: x1.saturating_sub(x0), height: y1.saturating_sub(y0) } }
        }
        None => full,
    };
    vk::ClearRect { rect, base_array_layer: 0, layer_count: 1 }
}

fn region_copy(t: &tex_t, view: Option<&view_t>, r: &NVNcopyRegion, tf: &format::TexFormat) -> (vk::ImageSubresourceLayers, vk::Offset3D, vk::Extent3D) {
    let level = view.filter(|v| v.bits & VW_LEVELS != 0).map(|v| v.base_level as u32).unwrap_or(0);
    let layer0 = view.filter(|v| v.bits & VW_LAYERS != 0).map(|v| v.min_layer as u32).unwrap_or(0);
    let is3d = t.d.target == target::T3D;
    (
        vk::ImageSubresourceLayers {
            aspect_mask: super::image::upload_aspect(tf),
            mip_level: level,
            base_array_layer: if is3d { layer0 } else { layer0 + r.zoffset.max(0) as u32 },
            layer_count: if is3d { 1 } else { r.depth.max(1) as u32 },
        },
        vk::Offset3D { x: r.xoffset, y: r.yoffset, z: if is3d { r.zoffset } else { 0 } },
        vk::Extent3D { width: r.width.max(1) as u32, height: r.height.max(1) as u32, depth: if is3d { r.depth.max(1) as u32 } else { 1 } },
    )
}

/// A buffer <-> texture copy's buffer layout (texels per row, rows per image) and byte size.
fn buffer_layout(t: &tex_t, r: &NVNcopyRegion, strides: Option<p_copy_strides>) -> (u32, u32, u64) {
    let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
    let tight_row = (r.width.max(1) as u32).div_ceil(bw) as u64 * bpb as u64;
    let rows = (r.height.max(1) as u32).div_ceil(bh) as u64;
    let s = strides.unwrap_or_default();
    let row = if s.row > 0 { s.row as u64 } else { tight_row };
    let img = if s.image > 0 { s.image as u64 } else { row * rows };
    let row_texels = if s.row > 0 && bpb > 0 { (row / bpb as u64) as u32 * bw } else { 0 };
    let img_rows = if s.image > 0 && row > 0 { (img / row) as u32 * bh } else { 0 };
    (row_texels, img_rows, img * r.depth.max(1) as u64)
}

fn rd_payload<T: Copy>(rec: &[u8]) -> T {
    // SAFETY: payload structs are plain data for which all-zero is valid (short records read as zero-extended).
    let mut t: T = unsafe { core::mem::zeroed() };
    let n = core::mem::size_of::<T>().min(rec.len() - 8);
    unsafe { core::ptr::copy_nonoverlapping(rec.as_ptr().add(8), &mut t as *mut T as *mut u8, n) };
    t
}
unsafe fn rd_at<T: Copy>(rec: &[u8], off: usize) -> T {
    core::ptr::read_unaligned(rec.as_ptr().add(8 + off) as *const T)
}

/// Whether the wait at the start of a parked stream now passes.
pub fn wait_passes(g: &Gpu, stream: &[u8]) -> bool {
    if stream.len() < 8 {
        return true;
    }
    let h = unsafe { rd::<op_hdr>(stream.as_ptr()) };
    if h.op != op::WAIT_EVENT {
        return true;
    }
    let (addr, mode, value): (u64, u32, u32) = unsafe { (rd_at(stream, 0), rd_at(stream, 8), rd_at(stream, 12)) };
    event_wait_passes(g, addr, mode, value)
}

fn event_wait_passes(g: &Gpu, addr: u64, mode: u32, value: u32) -> bool {
    let cur = unsafe { (*(addr as *const std::sync::atomic::AtomicU32)).load(std::sync::atomic::Ordering::Acquire) };
    if crate::nvn::sync::event_passes(cur, mode, value) {
        return true;
    }
    // a GPU WRITE signal encoded before and still in flight releases it: the queue runs it first
    matches!(g.q.pending_signals.lock().unwrap().get(&addr), Some(&(m, v, _)) if m == event_signal::WRITE && crate::nvn::sync::event_passes(v, mode, value))
}

/// Replays `stream` (canonical records). Returns the offset of a wait that parks the queue, if any.
pub fn run(g: &Gpu, sub: &mut Sub, qs: &mut QState, stream: &[u8]) -> Option<usize> {
    let mut p = 0usize;
    while p + 8 <= stream.len() {
        let h = unsafe { rd::<op_hdr>(stream.as_ptr().add(p)) };
        let sz = h.size as usize;
        if sz < 8 || p + sz > stream.len() {
            crate::log::red("replay: a record does not fit its stream");
            break;
        }
        let rec = &stream[p..p + sz];
        if !record(g, sub, qs, h.op, rec) {
            return Some(p);
        }
        p += sz;
    }
    None
}

/// One record; false = this wait parks the queue.
fn record(g: &Gpu, sub: &mut Sub, qs: &mut QState, opc: u32, rec: &[u8]) -> bool {
    let r = record_op(g, sub, qs, opc, rec);
    // a capture is open around one draw only (no other pipeline may be bound while it is active)
    if qs.tf_active > 0 {
        tf_end(g, sub, qs);
    }
    r
}

/// The four capture counters (Vulkan continues a capture of several draws from them).
fn tf_counters(g: &Gpu) -> Option<vk::Buffer> {
    static B: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let b = B.get_or_init(|| {
        g.buffer(16, vk::BufferUsageFlags::TRANSFORM_FEEDBACK_COUNTER_BUFFER_EXT, MemoryLocation::GpuOnly, "transform feedback counters")
            .ok()
            .map(|b| {
                let h = vk::Handle::as_raw(b.buf) as usize;
                std::mem::forget(b);
                h
            })
    });
    b.map(|h| vk::Handle::from_raw(h as u64))
}

/// Opens the capture of the draw about to be recorded (its pipeline uses the capture variant).
fn tf_begin(g: &Gpu, sub: &mut Sub, qs: &mut QState, l: &super::xfb::Layout) {
    let xfb = &g.d.xfb;
    let mut bufs = vec![];
    let mut offs = vec![];
    let mut sizes = vec![];
    for i in 0..l.nbuf as usize {
        let (a, n) = qs.tf[i];
        match if a != 0 && n != 0 { super::mem::resolve(g, sub, a, n) } else { None } {
            Some((b, o, _)) if o % 4 == 0 => {
                super::mem::note_gpu_write(g, sub, a, n);
                bufs.push(b);
                offs.push(o);
                sizes.push(n);
            }
            _ => {
                crate::log::red_once(&format!("transform feedback buffer {i} ({a:#x}, {n} bytes) is in no memory pool or not word aligned: the capture is not written"));
                bufs.push(zero_buffer(g, sub));
                offs.push(0);
                sizes.push(16);
            }
        }
    }
    let cb = sub.main;
    unsafe { (xfb.fp().cmd_bind_transform_feedback_buffers_ext)(cb, 0, bufs.len() as u32, bufs.as_ptr(), offs.as_ptr(), sizes.as_ptr()) };
    let cnt = tf_counters(g);
    match (qs.tf_cont, cnt) {
        (true, Some(c)) => {
            let cs: Vec<vk::Buffer> = (0..l.nbuf).map(|_| c).collect();
            let co: Vec<u64> = (0..l.nbuf as u64).map(|i| 4 * i).collect();
            unsafe { (xfb.fp().cmd_begin_transform_feedback_ext)(cb, 0, cs.len() as u32, cs.as_ptr(), co.as_ptr()) };
        }
        _ => unsafe { (xfb.fp().cmd_begin_transform_feedback_ext)(cb, 0, 0, core::ptr::null(), core::ptr::null()) },
    }
    qs.tf_active = l.nbuf;
}

fn tf_end(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    let n = qs.tf_active;
    qs.tf_active = 0;
    qs.tf_cont = true;
    let cb = sub.main;
    match tf_counters(g) {
        Some(c) => {
            let cs: Vec<vk::Buffer> = (0..n).map(|_| c).collect();
            let co: Vec<u64> = (0..n as u64).map(|i| 4 * i).collect();
            unsafe { (g.d.xfb.fp().cmd_end_transform_feedback_ext)(cb, 0, cs.len() as u32, cs.as_ptr(), co.as_ptr()) };
        }
        None => unsafe { (g.d.xfb.fp().cmd_end_transform_feedback_ext)(cb, 0, 0, core::ptr::null(), core::ptr::null()) },
    }
}

fn record_op(g: &Gpu, sub: &mut Sub, qs: &mut QState, opc: u32, rec: &[u8]) -> bool {
    let dev = g.dev();
    match opc {
        op::END => {}
        op::SET_RT => {
            end_pass(g, sub, qs);
            let r: p_set_rt = rd_payload(rec);
            qs.rt_n = r.n.clamp(0, 8) as usize;
            for i in 0..8 {
                qs.rt[i] = Rt { tex: None, view: None };
                if i < qs.rt_n && !r.color[i].is_null() {
                    let t = unsafe { *r.color[i] };
                    if t.valid() {
                        qs.rt[i] = Rt { tex: Some(t), view: if r.has_cview[i] != 0 { Some(r.cview[i]) } else { None } };
                    }
                }
            }
            qs.rt_depth = Rt { tex: None, view: None };
            if !r.depth.is_null() {
                let t = unsafe { *r.depth };
                if t.valid() {
                    qs.rt_depth = Rt { tex: Some(t), view: if r.has_dview != 0 { Some(r.dview) } else { None } };
                }
            }
        }
        op::CLEAR_COLOR => {
            super::perf::inc(super::perf::C::Clears);
            let c: p_clear_color = rd_payload(rec);
            let i = c.index.clamp(0, 7) as usize;
            if i >= qs.rt_n || qs.rt[i].tex.is_none() || !begin_pass(g, sub, qs) {
                return true;
            }
            if c.mask & 0xf != 0xf {
                crate::log::red_once("ClearColor with a partial channel mask clears every channel");
            }
            let kind = qs.rt[i].tex.and_then(|t| format::tex_format(t.d.format)).map(|f| f.kind).unwrap_or(Kind::Float);
            let v = match kind {
                Kind::Uint => vk::ClearColorValue { uint32: c.rgba.map(|x| x as u32) },
                Kind::Sint => vk::ClearColorValue { int32: c.rgba.map(|x| x as i32) },
                _ => vk::ClearColorValue { float32: c.rgba },
            };
            let a = [vk::ClearAttachment { aspect_mask: vk::ImageAspectFlags::COLOR, color_attachment: i as u32, clear_value: vk::ClearValue { color: v } }];
            let r = clear_rect(qs);
            if r.rect.extent.width > 0 && r.rect.extent.height > 0 {
                unsafe { dev.cmd_clear_attachments(sub.main, &a, &[r]) };
            }
        }
        op::CLEAR_DS => {
            super::perf::inc(super::perf::C::Clears);
            let c: p_clear_ds = rd_payload(rec);
            let Some(t) = qs.rt_depth.tex else { return true };
            if !begin_pass(g, sub, qs) {
                return true;
            }
            let kind = format::tex_format(t.d.format).map(|f| f.kind).unwrap_or(Kind::Depth);
            let mut asp = vk::ImageAspectFlags::empty();
            if c.depth_mask != 0 && kind != Kind::Stencil {
                asp |= vk::ImageAspectFlags::DEPTH;
            }
            if c.stencil_mask != 0 && matches!(kind, Kind::DepthStencil | Kind::Stencil) {
                asp |= vk::ImageAspectFlags::STENCIL;
            }
            if !asp.is_empty() {
                let a = [vk::ClearAttachment {
                    aspect_mask: asp,
                    color_attachment: 0,
                    clear_value: vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: c.depth.clamp(0.0, 1.0), stencil: c.stencil as u32 } },
                }];
                let r = clear_rect(qs);
                if r.rect.extent.width > 0 && r.rect.extent.height > 0 {
                    unsafe { dev.cmd_clear_attachments(sub.main, &a, &[r]) };
                }
            }
        }
        op::VIEWPORTS => {
            let v: p_viewports = rd_payload(rec);
            if v.first == 0 {
                qs.vp_origin = v.v[0][4].to_bits();
            }
            for i in 0..v.n.clamp(0, 16) as usize {
                let k = v.first as usize + i;
                if k < 16 {
                    qs.vp[k] = [v.v[i][0], v.v[i][1], v.v[i][2], v.v[i][3]];
                }
            }
        }
        op::SCISSOR => {
            let s: p_scissor = rd_payload(rec);
            qs.scissor = Some([s.x, s.y, s.w, s.h]);
        }
        op::DEPTH_RANGE => {
            let d: p_depth_range = rd_payload(rec);
            qs.depth_range = [d.n, d.f];
        }
        op::BIND_PROGRAM => {
            let mut b: p_bind_program = rd_payload(rec);
            // NVN2's task and mesh stage bits take the tessellation stages' places
            b.stages = (b.stages & 0x3f) | if b.stages & 0x40 != 0 { 8 } else { 0 } | if b.stages & 0x80 != 0 { 16 } else { 0 };
            if b.prog.is_null() {
                for s in 0..6 {
                    if b.stages & (1 << s) != 0 {
                        qs.prog[s] = None;
                    }
                }
                return true;
            }
            let pr = unsafe { &*b.prog };
            if pr.magic != MAGIC_PROG {
                crate::log::red_once("BindProgram of a finalized program");
                return true;
            }
            let keys = unsafe { crate::nvn::prog::prog_keys(pr) };
            for s in 0..6 {
                if b.stages & (1 << s) != 0 {
                    qs.prog[s] = None;
                }
            }
            if b.stages & 0x1f != 0 {
                qs.missing.clear();
            }
            for k in keys {
                match g.shaders.get(dev, &k) {
                    Some(m) => {
                        let s = m.stage as usize;
                        if b.stages & (1 << s) != 0 {
                            qs.prog[s] = Some(m);
                        }
                    }
                    None if b.stages & 0x1f != 0 => qs.missing.push((k, b.stages as u32)),
                    None => {}
                }
            }
        }
        op::BIND_STATE => {
            let b: p_bind_state = rd_payload(rec);
            let u32_ = u32::from_le_bytes(b.bytes[..4].try_into().unwrap());
            let u64_ = u64::from_le_bytes(b.bytes[..8].try_into().unwrap());
            match b.kind {
                0 => {
                    let bs = blend_st(u64_);
                    qs.blend[bs.get(blend_field::TARGET) as usize & 7] = bs;
                }
                1 => qs.color = color_st(u32_),
                2 => qs.chmask = u32_,
                3 => qs.ms = unsafe { core::ptr::read_unaligned(b.bytes.as_ptr() as *const ms_st) },
                4 => qs.poly = poly_st(u32_),
                5 => qs.ds = ds_st(u64_),
                _ => {}
            }
        }
        op::BIND_VATTR => {
            let n: i32 = unsafe { rd_at(rec, 4) };
            let n = n.clamp(0, 32) as usize;
            for i in 0..n {
                qs.vattr[i] = unsafe { rd_at(rec, 8 + 4 * i) };
            }
            qs.nvattr = n;
        }
        op::BIND_VSTREAM => {
            let n: i32 = unsafe { rd_at(rec, 4) };
            for i in 0..n.clamp(0, 16) as usize {
                qs.vstream[i] = unsafe { rd_at(rec, 8 + 8 * i) };
            }
        }
        op::BIND_VBUF => {
            let first: i32 = unsafe { rd_at(rec, 0) };
            let n: i32 = unsafe { rd_at(rec, 4) };
            for i in 0..n.clamp(0, 16) as usize {
                let r: NVNbufferRange = unsafe { rd_at(rec, 8 + 16 * i) };
                let k = first as usize + i;
                if k < 16 {
                    qs.vbuf[k] = (r.address, r.size);
                }
            }
        }
        op::BIND_VBUF1 => {
            let b: p_bind_vbuf1 = rd_payload(rec);
            if (b.index as usize) < 16 {
                qs.vbuf[b.index as usize] = (b.addr, b.size as u64);
            }
        }
        op::BIND_UBO | op::BIND_UBO_SMALL => {
            let b: p_bind_buf = if opc == op::BIND_UBO {
                rd_payload(rec)
            } else {
                let s: p_bind_ubo_small = rd_payload(rec);
                p_bind_buf { stage: s.stage as i32, index: s.index as i32, addr: s.addr, size: s.size as u64 }
            };
            if (b.stage as usize) < 7 && (b.index as usize) < 16 {
                qs.ubo[b.stage as usize][b.index as usize] = (b.addr, b.size);
                qs.bump(b.stage as usize);
                qs.bump(5);
            }
        }
        op::BIND_SSBO => {
            let b: p_bind_buf = rd_payload(rec);
            if (b.stage as usize) < 7 && (b.index as usize) < 16 {
                qs.ssbo[b.stage as usize][b.index as usize] = (b.addr, b.size);
                qs.bump(b.stage as usize);
            }
        }
        op::BIND_TEX | op::BIND_SAMP | op::BIND_IMG => {
            let (stage, first, n): (i32, i32, i32) = unsafe { (rd_at(rec, 0), rd_at(rec, 4), rd_at(rec, 8)) };
            let (defer, src): (i32, u64) = unsafe { (rd_at(rec, 12), rd_at(rec, 16)) };
            if (stage as usize) < 7 {
                qs.bump(stage as usize);
                for i in 0..n.clamp(0, 32) as usize {
                    // a deferred bind reads the game's handle array now, as the queue runs it
                    let h: u64 = if defer != 0 && src != 0 { unsafe { core::ptr::read_unaligned((src + 8 * i as u64) as *const u64) } } else { unsafe { rd_at(rec, 24 + 8 * i) } };
                    let k = first as usize + i;
                    match opc {
                        op::BIND_TEX if k < 256 => qs.tex[stage as usize][k] = h as u32,
                        op::BIND_SAMP if k < 128 => qs.samp[stage as usize][k] = h as u32,
                        op::BIND_IMG if k < 32 => qs.img[stage as usize][k] = h as u32,
                        _ => {}
                    }
                }
            }
        }
        op::BIND_TEX1 => {
            let b: p_bind_tex1 = rd_payload(rec);
            if (b.stage as usize) < 7 && (b.slot as usize) < 256 {
                qs.tex[b.stage as usize][b.slot as usize] = b.h as u32;
                qs.bump(b.stage as usize);
            }
        }
        op::SET_TEX_POOL => qs.tex_pool = rd_payload::<usize>(rec),
        op::SET_SAMP_POOL => qs.samp_pool = rd_payload::<usize>(rec),
        op::STENCIL => {
            let s: p_stencil = rd_payload(rec);
            let v = match s.which {
                0 => s.value_mask,
                1 => s.mask,
                _ => s.r#ref,
            } as u32;
            for f in 0..2 {
                if s.face as u32 & (1 << f) != 0 {
                    qs.stencil[f][s.which.clamp(0, 2) as usize] = v;
                }
            }
        }
        op::POLY_OFFSET => {
            let o: p_poly_offset = rd_payload(rec);
            qs.poly_off = [o.factor, o.units, o.clamp];
        }
        op::BLEND_COLOR => qs.blend_color = rd_payload::<p_blend_color>(rec).rgba,
        op::PRIM_RESTART => {
            let r: p_prim_restart = rd_payload(rec);
            qs.prim_restart = (r.on != 0, r.index);
        }
        op::LINE_WIDTH => qs.line_width = rd_payload::<p_line_width>(rec).w,
        op::DEPTH_CLAMP => qs.depth_clamp = rd_payload::<p_depth_clamp>(rec).on != 0,
        op::RAST_DISCARD => qs.rast_discard = rd_payload::<p_rast_discard>(rec).on != 0,
        op::PATCH_SIZE => qs.patch = rd_payload::<p_patch_size>(rec).n.max(1) as u32,
        op::DRAW => draw(g, sub, qs, rd_payload(rec)),
        op::DRAW_IDX => draw_idx(g, sub, qs, rd_payload(rec)),
        op::DISPATCH => dispatch(g, sub, qs, rd_payload(rec)),
        op::BARRIER => {
            end_pass(g, sub, qs);
            barrier(g, sub);
        }
        op::COPY_B2B => {
            end_pass(g, sub, qs);
            let [src, dst, n]: [u64; 3] = unsafe { rd_at(rec, 0) };
            if let (Some((sb, so, _)), Some((db, doff, _))) = (super::mem::resolve(g, sub, src, n), super::mem::resolve(g, sub, dst, n)) {
                unsafe { dev.cmd_copy_buffer(sub.main, sb, db, &[vk::BufferCopy { src_offset: so, dst_offset: doff, size: n }]) };
                super::mem::note_gpu_write(g, sub, dst, n);
                barrier(g, sub);
            }
        }
        op::UPDATE_UB => {
            end_pass(g, sub, qs);
            let [dst, n]: [u64; 2] = unsafe { rd_at(rec, 0) };
            let data = &rec[24..24 + n as usize];
            if let Some((db, doff, _)) = super::mem::resolve(g, sub, dst, n) {
                let (sb, so) = sub.stage_bytes(g, data);
                unsafe { dev.cmd_copy_buffer(sub.main, sb, db, &[vk::BufferCopy { src_offset: so, dst_offset: doff, size: n }]) };
                super::mem::note_gpu_write(g, sub, dst, n);
                barrier(g, sub);
            }
        }
        op::CLEAR_BUF => {
            end_pass(g, sub, qs);
            let c: p_clear_buf = rd_payload(rec);
            if let Some((b, o, _)) = super::mem::resolve(g, sub, c.addr, c.size) {
                unsafe { dev.cmd_fill_buffer(sub.main, b, o, c.size & !3, c.value) };
                super::mem::note_gpu_write(g, sub, c.addr, c.size);
                barrier(g, sub);
            }
        }
        op::COPY_B2T | op::COPY_T2B => {
            super::perf::inc(super::perf::C::TexCopies);
            end_pass(g, sub, qs);
            let c: p_copy_b2t = rd_payload(rec);
            if c.tex.is_null() {
                return true;
            }
            let t = unsafe { *c.tex };
            let strides = (rec.len() - 8 >= COPY_STRIDES_OFF + 16).then(|| unsafe { rd_at::<p_copy_strides>(rec, COPY_STRIDES_OFF) });
            let (row, img_rows, bytes) = buffer_layout(&t, &c.r, strides);
            let view = (c.has_view != 0).then_some(c.view);
            let img = if opc == op::COPY_B2T { target_image(g, sub, &t).map(|x| (x.0, x.1)) } else { source_image(g, sub, &t).map(|x| (x.0, x.1)) };
            let Some((image, tf)) = img else { return true };
            if tf.expand3 != 0 || tf.astc.is_some() {
                crate::log::red_once("buffer <-> texture copy of an RGB or ASTC texture is not converted yet");
                return true;
            }
            let Some((b, o, _)) = super::mem::resolve(g, sub, c.src, bytes) else { return true };
            let (sl, off, ext) = region_copy(&t, view.as_ref(), &c.r, &tf);
            let r = vk::BufferImageCopy { buffer_offset: o, buffer_row_length: row, buffer_image_height: img_rows, image_subresource: sl, image_offset: off, image_extent: ext };
            unsafe {
                if opc == op::COPY_B2T {
                    dev.cmd_copy_buffer_to_image(sub.main, b, image, vk::ImageLayout::GENERAL, &[r]);
                } else {
                    dev.cmd_copy_image_to_buffer(sub.main, image, vk::ImageLayout::GENERAL, b, &[r]);
                }
            }
            if opc == op::COPY_T2B {
                super::mem::note_gpu_write(g, sub, c.src, bytes);
            }
            barrier(g, sub);
        }
        op::COPY_T2T => {
            super::perf::inc(super::perf::C::TexCopies);
            end_pass(g, sub, qs);
            let c: p_copy_t2t = rd_payload(rec);
            if c.src.is_null() || c.dst.is_null() {
                return true;
            }
            let (s, d) = unsafe { (*c.src, *c.dst) };
            let Some((si, stf, _)) = source_image(g, sub, &s) else { return true };
            let Some((di, dtf, _, _)) = target_image(g, sub, &d) else { return true };
            let (ssl, soff, sext) = region_copy(&s, (c.has_sv != 0).then_some(&c.sv), &c.sr, &stf);
            let (dsl, doff, dext) = region_copy(&d, (c.has_dv != 0).then_some(&c.dv), &c.dr, &dtf);
            unsafe {
                if sext == dext && stf.vk == dtf.vk {
                    let r = vk::ImageCopy { src_subresource: ssl, src_offset: soff, dst_subresource: dsl, dst_offset: doff, extent: sext };
                    dev.cmd_copy_image(sub.main, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &[r]);
                } else {
                    let end = |o: vk::Offset3D, e: vk::Extent3D| vk::Offset3D { x: o.x + e.width as i32, y: o.y + e.height as i32, z: o.z + e.depth as i32 };
                    let r = vk::ImageBlit { src_subresource: ssl, src_offsets: [soff, end(soff, sext)], dst_subresource: dsl, dst_offsets: [doff, end(doff, dext)] };
                    let filter = if c.flags & 1 != 0 { vk::Filter::LINEAR } else { vk::Filter::NEAREST };
                    dev.cmd_blit_image(sub.main, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &[r], filter);
                }
            }
            barrier(g, sub);
        }
        op::CLEAR_TEX => {
            end_pass(g, sub, qs);
            let c: p_clear_tex = rd_payload(rec);
            if c.tex.is_null() {
                return true;
            }
            let t = unsafe { *c.tex };
            let Some((image, tf, ext, _)) = target_image(g, sub, &t) else { return true };
            let view = (c.has_view != 0).then_some(c.view);
            let (sl, off, e) = region_copy(&t, view.as_ref(), &c.r, &tf);
            if off.x != 0 || off.y != 0 || e.width < (ext.width >> sl.mip_level).max(1) || e.height < (ext.height >> sl.mip_level).max(1) {
                crate::log::red_once("ClearTexture of part of a level clears the whole level");
            }
            let range = vk::ImageSubresourceRange { aspect_mask: sl.aspect_mask, base_mip_level: sl.mip_level, level_count: 1, base_array_layer: sl.base_array_layer, layer_count: sl.layer_count };
            unsafe {
                if matches!(tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                    let v = vk::ClearDepthStencilValue { depth: f32::from_bits(c.c[0]), stencil: c.c[1] };
                    dev.cmd_clear_depth_stencil_image(sub.main, image, vk::ImageLayout::GENERAL, &v, &[range]);
                } else {
                    let v = if c.is_uint != 0 { vk::ClearColorValue { uint32: c.c } } else { vk::ClearColorValue { float32: c.c.map(f32::from_bits) } };
                    dev.cmd_clear_color_image(sub.main, image, vk::ImageLayout::GENERAL, &v, &[range]);
                }
            }
            barrier(g, sub);
        }
        op::SIGNAL_EVENT => {
            let (addr, mode, value): (u64, u32, u32) = unsafe { (rd_at(rec, 0), rd_at(rec, 8), rd_at(rec, 12)) };
            sub.post.push(Post::Event { addr, mode, value });
            let mut ps = g.q.pending_signals.lock().unwrap();
            if mode == event_signal::WRITE {
                ps.insert(addr, (mode, value, g.q.last_fence() + 1));
            } else {
                ps.remove(&addr);
            }
        }
        op::WAIT_EVENT => {
            let (addr, mode, value): (u64, u32, u32) = unsafe { (rd_at(rec, 0), rd_at(rec, 8), rd_at(rec, 12)) };
            if !event_wait_passes(g, addr, mode, value) {
                end_pass(g, sub, qs);
                return false;
            }
        }
        op::WAIT_SYNC => {}
        op::FENCE_SYNC => {
            let s: u64 = unsafe { rd_at(rec, 0) };
            if s != 0 {
                let so = s as *mut sync_t;
                unsafe {
                    (*so).fenced = 1;
                    (*so).fence = g.q.last_fence() + 1;
                }
            }
        }
        op::REPORT_COUNTER => {
            let (kind, addr): (u64, u64) = unsafe { (rd_at(rec, 0), rd_at(rec, 8)) };
            let kind = kind as u32;
            if kind == counter::SAMPLES_PASSED {
                end_query(g, sub, qs);
                sub.post.push(Post::OcclReport { addr });
            } else {
                if kind != counter::TIMESTAMP && kind != counter::TIMESTAMP_TOP {
                    crate::log::red_once(&format!("counter report of type {kind}: reported as 0 (timestamps and samples passed are counted)"));
                }
                let stage = if kind == counter::TIMESTAMP_TOP { vk::PipelineStageFlags2::TOP_OF_PIPE } else { vk::PipelineStageFlags2::ALL_COMMANDS };
                let q = if sub.queries < 256 { sub.query() } else { u32::MAX };
                if q != u32::MAX {
                    unsafe { dev.cmd_write_timestamp2(sub.main, stage, sub.query, q) };
                }
                sub.post.push(Post::Counter { addr, kind, query: q });
            }
        }
        op::PUSH_CONST => {
            let p: crate::nvn::cmd2::p_push_const = rd_payload(rec);
            let (o, n) = (p.offset as usize, p.size as usize);
            if o + n <= 0x800 {
                qs.pgen = new_bgen();
                let grp = &mut qs.push[(p.compute != 0) as usize][o..o + n];
                if p.deferred != 0 {
                    unsafe { core::ptr::copy_nonoverlapping(p.src as *const u8, grp.as_mut_ptr(), n) };
                } else {
                    let at = core::mem::size_of::<crate::nvn::cmd2::p_push_const>();
                    grp.copy_from_slice(&rec[8 + at..8 + at + n]);
                }
            }
        }
        op::BUFS_DEFERRED => {
            let b: crate::nvn::cmd2::p_bufs_deferred = rd_payload(rec);
            if (b.stage as usize) < 7 {
                qs.bump(b.stage as usize);
                qs.bump(5);
                for i in 0..b.n.clamp(0, 16) {
                    let k = (b.first + i) as usize;
                    if k >= 16 {
                        break;
                    }
                    let r: [u64; 2] = unsafe { core::ptr::read_unaligned((b.src + 16 * i as u64) as *const [u64; 2]) };
                    if b.ssbo != 0 {
                        qs.ssbo[b.stage as usize][k] = (r[0], r[1]);
                    } else {
                        qs.ubo[b.stage as usize][k] = (r[0], r[1]);
                    }
                }
            }
        }
        op::RENDER_COND => {
            let r: crate::nvn::cmd2::p_render_cond = rd_payload(rec);
            qs.render_on = match r.mode {
                2 => true,
                3 => false,
                m if r.addr != 0 => {
                    // the two 64-bit values the condition compares, as the queue reaches it (reports land at completion)
                    let v: [u64; 2] = unsafe { core::ptr::read_unaligned(r.addr as *const [u64; 2]) };
                    if m == 0 { v[0] == v[1] } else { v[0] != v[1] }
                }
                _ => true,
            };
        }
        op::ALPHA_REF => qs.alpha_ref = rd_payload::<crate::nvn::cmd2::p_f32x4>(rec).v[0],
        op::DEPTH_BOUNDS => {
            let p: crate::nvn::cmd2::p_f32x4 = rd_payload(rec);
            qs.depth_bounds = (p.i[0] != 0, p.v[0], p.v[1]);
        }
        op::SAMPLE_MASK => qs.sample_mask = rd_payload::<crate::nvn::cmd2::p_f32x4>(rec).i[0] as u32,
        op::POINT_SIZE => qs.point_size = rd_payload::<crate::nvn::cmd2::p_f32x4>(rec).v[0],
        op::TESS_LEVELS => {
            let p: crate::nvn::cmd2::p_f32x4 = rd_payload(rec);
            if p.i[0] == 0 {
                qs.tess_levels[4..6].copy_from_slice(&p.v[..2]);
            } else {
                qs.tess_levels[..4].copy_from_slice(&p.v);
            }
        }
        op::PROVOKING => qs.provoking_last = rd_payload::<crate::nvn::cmd2::p_f32x4>(rec).i[0] != 0,
        op::VP_SWIZZLE => {
            let p: crate::nvn::cmd2::p_f32x4 = rd_payload(rec);
            if (0..16).contains(&p.i[0]) {
                qs.vp_swizzle[p.i[0] as usize] = p.i[1] as u32;
                if p.i[1] as u32 != 0x0302_0100 {
                    crate::log::red_once("viewport swizzles other than the identity are not applied (VK_NV_viewport_swizzle)");
                }
            }
        }
        op::TF_BIND => {
            let b: crate::nvn::cmd2::p_tf_bind = rd_payload(rec);
            if (0..4).contains(&b.index) {
                qs.tf[b.index as usize] = (b.addr, b.size);
            }
        }
        op::TF_STATE => {
            let s: crate::nvn::cmd2::p_tf_state = rd_payload(rec);
            match s.what & 0xff {
                0 => {
                    qs.tf_on = true;
                    qs.tf_cont = false;
                }
                3 => qs.tf_on = true,
                1 | 2 => qs.tf_on = false,
                _ => crate::log::red_once("DrawTransformFeedback is not replayed yet (a draw of the count a capture wrote)"),
            }
            if qs.tf_on && !g.d.caps.xfb {
                crate::log::red_once("transform feedback: the device has no VK_EXT_transform_feedback, captures are not written");
            }
        }
        op::MULTI_DRAW => {
            let m: crate::nvn::cmd2::p_multi_draw = rd_payload(rec);
            multi_draw(g, sub, qs, m);
        }
        op::DRAW_TEX => {
            let d: crate::nvn::cmd2::p_draw_tex = rd_payload(rec);
            draw_texture(g, sub, qs, d);
        }
        op::MESH => {
            let m: crate::nvn::cmd2::p_mesh = rd_payload(rec);
            mesh_draw(g, sub, qs, m);
        }
        op::REPORT_VALUE => {
            let r: crate::nvn::cmd2::p_report_value = rd_payload(rec);
            let (a, v) = (r.addr, r.value);
            sub.post.push(Post::Callback(Box::new(move |_g: &Gpu| unsafe { core::ptr::write_unaligned(a as *mut u32, v) })));
        }
        op::DOWNSAMPLE => {
            end_pass(g, sub, qs);
            let t: crate::nvn::cmd2::p_tex2 = rd_payload(rec);
            let Some((si, stf, sext)) = source_image(g, sub, &t.src) else { return true };
            let Some((di, _dtf, dext, _)) = target_image(g, sub, &t.dst) else { return true };
            let asp = super::image::upload_aspect(&stf);
            let sl = |l: u32| vk::ImageSubresourceLayers { aspect_mask: asp, mip_level: l, base_array_layer: 0, layer_count: 1 };
            let sl0 = if t.has_sv != 0 && t.sv.bits & VW_LEVELS != 0 { t.sv.base_level as u32 } else { 0 };
            let dl0 = if t.has_dv != 0 && t.dv.bits & VW_LEVELS != 0 { t.dv.base_level as u32 } else { 0 };
            let e = |x: vk::Extent3D, l: u32| vk::Offset3D { x: (x.width >> l).max(1) as i32, y: (x.height >> l).max(1) as i32, z: 1 };
            let r = vk::ImageBlit { src_subresource: sl(sl0), src_offsets: [vk::Offset3D::default(), e(sext, sl0)], dst_subresource: sl(dl0), dst_offsets: [vk::Offset3D::default(), e(dext, dl0)] };
            unsafe { dev.cmd_blit_image(sub.main, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &[r], if stf.kind == Kind::Float { vk::Filter::LINEAR } else { vk::Filter::NEAREST }) };
            barrier(g, sub);
        }
        op::DLSS => {
            end_pass(g, sub, qs);
            let d: crate::nvn::dlss::p_dlss = rd_payload(rec);
            if d.has & 3 != 3 {
                crate::log::red_once("EvaluateDlss without its input colour or output texture: skipped");
                return true;
            }
            let Some((si, stf, sext)) = source_image(g, sub, &d.color) else { return true };
            let Some((di, _dtf, dext, _)) = target_image(g, sub, &d.out) else { return true };
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::vklog!("DLSS evaluation: {}x{} -> {}x{} as a filtered upscale (no DLSS network on this GPU path)", d.iw, d.ih, d.ow, d.oh);
            }
            let (iw, ih) = (if d.iw > 0 { d.iw } else { sext.width as i32 }, if d.ih > 0 { d.ih } else { sext.height as i32 });
            let (ow, oh) = (if d.ow > 0 { d.ow } else { dext.width as i32 }, if d.oh > 0 { d.oh } else { dext.height as i32 });
            let asp = super::image::upload_aspect(&stf);
            let sl = vk::ImageSubresourceLayers { aspect_mask: asp, mip_level: 0, base_array_layer: 0, layer_count: 1 };
            let r = vk::ImageBlit {
                src_subresource: sl,
                src_offsets: [vk::Offset3D { x: d.cbase[0], y: d.cbase[1], z: 0 }, vk::Offset3D { x: (d.cbase[0] + iw).min(sext.width as i32), y: (d.cbase[1] + ih).min(sext.height as i32), z: 1 }],
                dst_subresource: sl,
                dst_offsets: [vk::Offset3D { x: d.obase[0], y: d.obase[1], z: 0 }, vk::Offset3D { x: (d.obase[0] + ow).min(dext.width as i32), y: (d.obase[1] + oh).min(dext.height as i32), z: 1 }],
            };
            unsafe { dev.cmd_blit_image(sub.main, si, vk::ImageLayout::GENERAL, di, vk::ImageLayout::GENERAL, &[r], vk::Filter::LINEAR) };
            barrier(g, sub);
        }
        op::RESOLVE_DEPTH => {
            crate::log::red_once("ResolveDepthBuffer: a multisample depth resolve is not replayed yet");
        }
        op::RESET_COUNTER => {
            let kind: u64 = unsafe { rd_at(rec, 0) };
            if kind as u32 == counter::SAMPLES_PASSED {
                end_query(g, sub, qs);
                sub.post.push(Post::OcclReset);
                qs.vis_on = true;
            }
        }
        _ => crate::log::red_once(&format!("replay: record op {opc} is not implemented yet")),
    }
    true
}
