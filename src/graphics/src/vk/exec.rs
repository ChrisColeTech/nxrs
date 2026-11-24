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
use ash::vk::Handle;
use gpu_allocator::MemoryLocation;
use std::sync::Arc;

const DRV_SIZE: usize = 0xA00;

#[path = "render_chain.rs"]
pub(super) mod render_chain;

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
    /// counts the vertex attribute / stream / buffer binds (vertex input skipped while nothing of it changed)
    vgen: u64,
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
    /// Actual views supplied to the open rendering instance, for the opt-in attachment diagnostic.
    chain_views: [vk::ImageView; 9],
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
    cons_raster: bool,
    patch: u32,
    // per submission
    in_pass: bool,
    pass_extent: vk::Extent2D,
    /// the resolution scale of the open pass's targets (1 unless `resolution` scales render targets)
    pass_scale: f32,
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
    /// the bound pixel shader takes output 1 as target 0's second source (dual-source variant)
    dual: bool,
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
    /// Opt-in, one-shot raw-pointer initializer probe; reads are queued at existing pass boundaries.
    raw_probe: std::cell::RefCell<Option<super::gmem::probe::Probe>>,
    /// Observe the actual selected shader's binding, including later descriptor-cache hits, without rebuilding it.
    raw_probe_source: std::cell::Cell<Option<(u64, vk::DescriptorBufferInfo, bool)>>,
    /// graphics program keys bound whose modules the library did not have (the draw waits for the live translator)
    missing: Vec<(crate::nvn::prog::ShaderKey, u32)>,
    /// block aliases of compressed targets to copy into their images when the pass ends
    copyback: Vec<(
        vk::Image,
        vk::Image,
        u32,
        u32,
        vk::Extent2D,
        vk::Extent2D,
        u32,
    )>,
    /// a stage's generations of what its driver banks hold (c0: storage buffers, compute's uniform table, dispatch
    /// size; c2: texture, sampler and image handles), new at every bind that changes them, and the banks built for them
    bgen: [[u64; 2]; 7],
    cbank: std::cell::RefCell<[[Option<CBank>; 2]; 7]>,
    /// a stage's texture and image descriptors as last resolved (`TexPart`)
    tpart: std::cell::RefCell<[Option<TexPart>; 7]>,
    /// a stage's whole set as last written into the descriptor buffer, and what it was made from (`SetKey`)
    sets: std::cell::RefCell<super::lost::bindings::SetCache<SetKey>>,
    /// the id of the c2 bank `banks` returned last
    c2id: std::cell::Cell<u64>,
    /// the vertex input and vertex buffers last set in this queue's command buffer: (submission, layout words,
    /// buffers by stream) (Pokemon Legends Z-A's draws repeat them: they were set again at every draw)
    vin_last: std::cell::RefCell<(u64, u64, [(vk::Buffer, u64); 32])>,
    /// what the vertex input last set in submission `vin_last.0` was made from: (vertex binds' generation, the vertex
    /// shader's inputs, the pools' generation); the same again sets nothing (Donkey Kong Bananza: 4.4 ms a frame of
    /// vertex input over ~4,500 draws, most repeating the draw before)
    vin_from: std::cell::Cell<(u64, usize, u64)>,
    /// push constants' generation (new at every PushConstants)
    pgen: u64,
    /// staged constant blocks by (kind, key): where they are in submission `serial` (support buffer by pass size,
    /// a module's c1, the push banks by generation; each was hashed and staged again at every draw)
    scache: std::cell::RefCell<std::collections::HashMap<(u8, u64), (u64, (vk::Buffer, u64))>>,
    ndve: NdveTransaction,
}

#[derive(Clone)]
struct ComputeSnapshot {
    shader: Arc<Module>,
    dispatch: p_dispatch,
    ubo: [(u64, u64); 16],
    ssbo: [(u64, u64); 16],
    tex: [u32; 256],
    samp: [u32; 128],
    img: [u32; 32],
    tex_pool: usize,
    samp_pool: usize,
    push: [u8; 0x800],
    pool_generation: u64,
    textures: Vec<(i32, u32, Option<super::tables::CapturedTexture>)>,
    images: Vec<(i32, u32, Option<super::tables::CapturedTexture>)>,
}

#[derive(Default)]
struct NdveTransaction {
    pending: Vec<ComputeSnapshot>,
}

/// A stage's texture / image descriptors as last resolved: valid for the same module while its handle bank (c2), the
/// pools' bases and the submission stand (Pokemon Legends Z-A binds new uniform buffers at nearly every draw and its
/// textures far less often: their views and samplers were looked up again at each, ~2 ms a frame). Not kept for a
/// shader that reads handles from a uniform buffer (bindless) or for buffer textures.
struct TexPart {
    module: (usize, crate::nvn::prog::ShaderKey),
    gen: u64,
    pools: (i32, i32),
    serial: u64,
    image_gen: u64,
    imgs: Vec<(u32, vk::DescriptorType, vk::DescriptorImageInfo)>,
}

/// What a stage's descriptor set is made from, when nothing else goes into it (`set_key`): the same key in the same
/// submission and descriptor-buffer chunk is the same set, and its offset is bound again instead of building and
/// writing it (Donkey Kong Bananza: 15,400 sets a frame, 27 ms, most of them the set of the draw before).
#[derive(Clone, Copy, PartialEq)]
struct SetKey {
    module: (usize, crate::nvn::prog::ShaderKey),
    bgen: [u64; 2],
    ubo: [(u64, u64); 16],
    pgen: u64,
    extent: (u32, u32),
    pools: (i32, i32),
    dispatch: [u32; 3],
    base_instance: u32,
    serial: u64,
    image_gen: u64,
    chunk: usize,
    /// a task / mesh shader's draw records (`Res::MeshDraw`): each mesh draw stages its own {count, first}, so a set
    /// reused from an earlier draw read that draw's `first` (lane K2)
    mesh: (u64, u64, u64),
}

/// The key of stage set `m` for this draw, or None when the set reads something the key does not cover: texture
/// handles from a uniform buffer or a bound buffer (bindless), buffer textures, the resolution scale, a pass dump.
fn set_key(
    g: &Gpu,
    sub: &Sub,
    qs: &QState,
    m: &Module,
    dispatch: [u32; 3],
    base_instance: u32,
) -> Option<SetKey> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_SET_CACHE").as_deref() != Ok("0"))
        || passdump::active()
        || super::scale::active()
    {
        return None;
    }
    let plain = m.res.iter().all(|(_, r)| match r {
        Res::Tex { cbuf_slot, ty, .. } | Res::Img { cbuf_slot, ty, .. } => {
            let (t, s) = tex_slots(*cbuf_slot);
            (t == 2 || t == -1) && (s == 2 || s == -1) && *ty != 2
        }
        _ => true,
    });
    if !plain {
        return None;
    }
    let st = m.stage as usize;
    let (di, bi) = if st == 5 {
        (dispatch, 0)
    } else {
        ([0; 3], base_instance)
    };
    Some(SetKey {
        module: (m as *const Module as usize, m.key),
        bgen: qs.bgen[st],
        ubo: qs.ubo[st],
        pgen: qs.pgen,
        extent: (qs.pass_extent.width, qs.pass_extent.height),
        pools: pool_bases(qs),
        dispatch: di,
        base_instance: bi,
        serial: sub.serial,
        image_gen: g.tables.generation(),
        chunk: sub.db_cur,
        mesh: match qs.mesh_rec {
            Some((b, o, r)) if m.res.iter().any(|(_, r)| matches!(r, Res::MeshDraw)) => {
                (vk::Handle::as_raw(b), o, r)
            }
            _ => (0, 0, 0),
        },
    })
}

/// A stage's c0 or c2 as last built: valid while its generation (and for c0 the dispatch size and base instance)
/// stands; where it was staged in submission `serial` (Pokemon Legends: Z-A, ~1,700 draws a frame: building, hashing
/// and staging 5 KB a stage a draw was most of the descriptor work; a uniform buffer bind, the most common one, changes
/// neither bank of a graphics stage, and a texture bind only c2)
struct CBank {
    gen: u64,
    dispatch: [u32; 3],
    base_instance: u32,
    bytes: Arc<Vec<u8>>,
    staged: Option<(u64, (vk::Buffer, u64))>,
    /// c2's sources as built (texture, sampler, image handles): a bind of the same handles keeps the bank unbuilt
    src: Option<Box<C2Src>>,
    /// new whenever the bytes change (what depends on them is kept by it)
    id: u64,
}

type C2Src = ([u32; 256], [u32; 128], [u32; 32]);

fn new_bgen() -> u64 {
    static G: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    G.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl QState {
    /// `bytes` staged in `sub`, once per submission for the same (kind, key).
    fn staged(
        &self,
        g: &Gpu,
        sub: &mut Sub,
        kind: u8,
        key: u64,
        bytes: impl FnOnce() -> Vec<u8>,
    ) -> (vk::Buffer, u64) {
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

    /// c0 of stage `st` changes (storage buffers; compute's uniform buffers)
    fn bump0(&mut self, st: usize) {
        if st < 7 {
            self.bgen[st][0] = new_bgen();
        }
    }
    /// c2 of stage `st` changes (texture, sampler, image handles)
    fn bump2(&mut self, st: usize) {
        if st < 7 {
            self.bgen[st][1] = new_bgen();
        }
    }

    /// c0 / c2 of stage `st` and where they are staged in `sub` (built and staged again only when they changed).
    fn banks(
        &self,
        g: &Gpu,
        sub: &mut Sub,
        st: usize,
        dispatch: [u32; 3],
        base_instance: u32,
    ) -> (
        Arc<Vec<u8>>,
        Arc<Vec<u8>>,
        (vk::Buffer, u64),
        (vk::Buffer, u64),
    ) {
        let mut cb = self.cbank.borrow_mut();
        let [b0, b2] = &mut cb[st];
        // c2 does not depend on the dispatch size or base instance
        let (di, bi) = if st == 5 {
            (dispatch, 0)
        } else {
            ([0; 3], base_instance)
        };
        // a bind that leaves the bytes as they were (the same handles bound again) keeps the staged copy
        let renew = |slot: &mut Option<CBank>,
                     gen: u64,
                     di: [u32; 3],
                     bi: u32,
                     build: &dyn Fn() -> Vec<u8>| {
            if slot
                .as_ref()
                .is_some_and(|c| c.gen == gen && c.dispatch == di && c.base_instance == bi)
            {
                return;
            }
            let bytes = build();
            match slot {
                Some(c) if *c.bytes == bytes => {
                    c.gen = gen;
                    c.dispatch = di;
                    c.base_instance = bi;
                }
                _ => {
                    *slot = Some(CBank {
                        gen,
                        dispatch: di,
                        base_instance: bi,
                        bytes: Arc::new(bytes),
                        staged: None,
                        src: None,
                        id: new_bgen(),
                    })
                }
            }
        };
        renew(b0, self.bgen[st][0], di, bi, &|| {
            c0_bytes(self, st, dispatch, base_instance)
        });
        // c2: the handles themselves compared first (a few hundred words; building the bank wrote 2.5 KB)
        let gen2 = self.bgen[st][1];
        if let Some(c) = b2.as_mut() {
            if c.gen != gen2
                && c.src.as_ref().is_some_and(|s| {
                    s.0 == self.tex[st] && s.1 == self.samp[st] && s.2 == self.img[st]
                })
            {
                c.gen = gen2;
            }
        }
        if !b2.as_ref().is_some_and(|c| c.gen == gen2) {
            renew(b2, gen2, [0; 3], 0, &|| c2_bytes(self, st));
            if let Some(c) = b2.as_mut() {
                c.src = Some(Box::new((self.tex[st], self.samp[st], self.img[st])));
            }
        }
        // staged once per submission (equal bytes staged earlier in it are shared: equal sets are bound again)
        let mut staged = |c: &mut CBank| match c.staged {
            Some((serial, s)) if serial == sub.serial => s,
            _ => {
                let s = sub.stage_bytes(g, &c.bytes);
                c.staged = Some((sub.serial, s));
                s
            }
        };
        let (c0, c2) = (b0.as_mut().unwrap(), b2.as_mut().unwrap());
        let (s0, s2) = (staged(c0), staged(c2));
        self.c2id.set(c2.id);
        (c0.bytes.clone(), c2.bytes.clone(), s0, s2)
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
        for f in [
            ds_field::FAIL_F,
            ds_field::ZFAIL_F,
            ds_field::ZPASS_F,
            ds_field::FAIL_B,
            ds_field::ZFAIL_B,
            ds_field::ZPASS_B,
        ] {
            ds.set(f, stencil_op::KEEP);
        }
        QState {
            origin_ll,
            prog: Default::default(),
            blend,
            color: color_st::new(0, logic_op::COPY, cmp::ALWAYS),
            chmask: 0xffff_ffff,
            ms: ms_st {
                enable: 1,
                a2c_dither: 1,
                ..Default::default()
            },
            poly: poly_st::new(face::NONE, front_face::CCW, polygon_mode::FILL, 0),
            cons_raster: false,
            ds,
            vattr: [0; 32],
            nvattr: 0,
            vstream: [vs_st::default(); 16],
            vbuf: [(0, 0); 16],
            vgen: 0,
            ubo: [[(0, 0); 16]; 7],
            ssbo: [[(0, 0); 16]; 7],
            tex: [[0; 256]; 7],
            samp: [[0; 128]; 7],
            img: [[0; 32]; 7],
            tex_pool: 0,
            samp_pool: 0,
            rt_n: 0,
            rt: [Rt {
                tex: None,
                view: None,
            }; 8],
            rt_depth: Rt {
                tex: None,
                view: None,
            },
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
            chain_views: [vk::ImageView::null(); 9],
            pass_scale: 1.0,
            pass_formats: ([0; 8], 0, 0, 1),
            copyback: Vec::new(),
            bgen: core::array::from_fn(|_| [new_bgen(), new_bgen()]),
            cbank: Default::default(),
            tpart: Default::default(),
            sets: Default::default(),
            c2id: Default::default(),
            vin_last: std::cell::RefCell::new((0, 0, [(vk::Buffer::null(), u64::MAX); 32])),
            vin_from: std::cell::Cell::new((u64::MAX, 0, 0)),
            pgen: new_bgen(),
            scache: Default::default(),
            ndve: Default::default(),
            vis_on: false,
            occl_q: None,
            last_color: vk::Extent2D::default(),
            missing: Vec::new(),
            push: Box::new([[0; 0x800]; 2]),
            render_on: true,
            alpha_ref: 0.0,
            dual: false,
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
            raw_probe: std::cell::RefCell::new(None),
            raw_probe_source: std::cell::Cell::new(None),
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
/// DrawElementsIndirect(arguments = the record). DKB also resets these records through a raw-pointer vertex shader;
/// this legacy fallback was added before those stores worked. An indirect draw whose arguments a draw's shader wrote as a storage
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
        let s = g.get_or_insert_with(|| St {
            bound: HashMap::new(),
            on: Vec::new(),
            started: 0,
        });
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

/// Resolve canonical raw backing separately from the final successful view-creation interpretation.
/// Missing/evicted metadata and unsupported subresources remain unknown, including stand-in views.
fn chain_resource(g: &Gpu, serial: u64, view: vk::ImageView) -> Option<render_chain::Resource> {
    let imgs = g.images.lock().unwrap();
    let im = imgs.by_serial.get(&imgs.live(serial))?;
    let mut owner = im;
    for _ in 0..16 {
        let Some(next) = owner.alias_of else { break };
        owner = imgs.by_serial.get(&imgs.live(next))?;
    }
    if owner.alias_of.is_some() {
        return None;
    }
    Some(render_chain::resolve_images(
        owner,
        im,
        vk::Handle::as_raw(view),
        super::image::resolved_view(im, view),
    ))
}

fn chain_pass_end(g: &Gpu, sub: &mut Sub, qs: &QState, at: render_chain::Point) {
    use render_chain::{Observation, Sample};
    for slot in 0..9 {
        if slot != 8 && slot >= qs.rt_n {
            continue;
        }
        let rt = if slot == 8 { qs.rt_depth } else { qs.rt[slot] };
        let Some(t) = rt.tex else { continue };
        let Some(resource) = chain_resource(g, t.serial, qs.chain_views[slot]) else {
            continue;
        };
        let Some((mut sample, n)) = render_chain::reserve(at, resource) else {
            continue;
        };
        if sample == Sample::Recorded {
            match g.buffer(
                n,
                vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuToCpu,
                "render chain",
            ) {
                Ok(buf) => {
                    let copy = vk::BufferImageCopy::default()
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::from_raw(resource.aspect))
                                .layer_count(1),
                        )
                        .image_extent(vk::Extent3D {
                            width: resource.width,
                            height: resource.height,
                            depth: 1,
                        });
                    super::queue::full_barrier(g, sub.main);
                    // SAFETY: verified actual attachment, single sample/level/layer, tightly packed
                    // allocation reserved within the diagnostic byte cap. Pass has already ended.
                    unsafe {
                        g.dev().cmd_copy_image_to_buffer(
                            sub.main,
                            <vk::Image as vk::Handle>::from_raw(resource.image),
                            vk::ImageLayout::GENERAL,
                            buf.buf,
                            &[copy],
                        );
                    }
                    super::queue::full_barrier(g, sub.main);
                    let ptr = buf.ptr as usize;
                    sub.transient.push(buf);
                    let path = passdump::dir().join(resource.raw_filename(at, slot));
                    sub.post.push(Post::Callback(Box::new(move |_g| {
                        // SAFETY: this exact submission completed; transient storage is freed after callbacks.
                        let bytes =
                            unsafe { core::slice::from_raw_parts(ptr as *const u8, n as usize) };
                        let result = path
                            .parent()
                            .map_or(Ok(()), std::fs::create_dir_all)
                            .and_then(|_| std::fs::write(&path, bytes));
                        match result {
                            Ok(()) => crate::vklog!(
                                "render chain completed {at:?} slot {slot}: {} bytes at {}",
                                n,
                                path.display()
                            ),
                            Err(e) => crate::log::red(&format!(
                                "render chain output failed {at:?} slot {slot}: {e}"
                            )),
                        }
                    })));
                }
                Err(e) => {
                    sample = Sample::AllocationFailed;
                    crate::log::red(&format!(
                        "render chain allocation failed {at:?} slot {slot}: {e}"
                    ));
                }
            }
        }
        render_chain::producer(Observation {
            at,
            slot,
            resource,
            sample,
        });
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
                REQ.lock()
                    .unwrap()
                    .extend(v.split(',').filter_map(|x| x.trim().parse::<f64>().ok()));
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

    pub(super) fn dir() -> std::path::PathBuf {
        std::env::var("NXGFX_DUMP_DIR")
            .map(Into::into)
            .unwrap_or_else(|_| "captures".into())
    }

    /// `NXGFX_DUMP_TEX=<w>x<h>`: a sampled texture of that size, once per dump frame, is read back at the end of the
    /// pass that sampled it: `tex-<frame>-ser<serial>-gpu.bin` (the image's level 0 as the GPU has it, tightly packed)
    /// and `tex-<frame>-ser<serial>-mem.bin` (its storage in guest memory, as laid out by the game), with a log line.
    static TEXQ: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    static TEXDONE: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
    pub fn want_tex(serial: u64, w: u32, h: u32) {
        let Ok(sz) = std::env::var("NXGFX_DUMP_TEX") else {
            return;
        };
        if sz != format!("{w}x{h}") {
            return;
        }
        let f = FRAME.load(Ordering::Relaxed);
        let mut d = TEXDONE.lock().unwrap();
        if d.contains(&(f, serial)) {
            return;
        }
        d.push((f, serial));
        TEXQ.lock().unwrap().push(serial);
    }
    fn dump_textures(g: &Gpu, sub: &mut Sub, f: u64) {
        let q: Vec<u64> = std::mem::take(&mut *TEXQ.lock().unwrap());
        for serial in q {
            let (img, tex, fmt, ext) = {
                let imgs = g.images.lock().unwrap();
                let Some(im) = imgs.by_serial.get(&serial) else {
                    continue;
                };
                (im.image, im.tex, im.format, im.extent)
            };
            let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(tex.d.format);
            // compressed images as their blocks (an ASTC texture's image is decoded: left out)
            let decoded =
                crate::tex::format::tex_format(tex.d.format).is_some_and(|tf| tf.astc.is_some());
            if bpb == 0 || decoded && (bw != 1 || bh != 1) {
                continue;
            }
            let n = ext.width.div_ceil(bw) as u64 * ext.height.div_ceil(bh) as u64 * bpb as u64;
            let Ok(buf) = g.buffer(
                n,
                vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuToCpu,
                "texture dump",
            ) else {
                continue;
            };
            let r = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                image_offset: vk::Offset3D::default(),
                image_extent: vk::Extent3D {
                    width: ext.width,
                    height: ext.height,
                    depth: 1,
                },
            };
            super::super::queue::full_barrier(g, sub.main);
            unsafe {
                g.dev().cmd_copy_image_to_buffer(
                    sub.main,
                    img,
                    vk::ImageLayout::GENERAL,
                    buf.buf,
                    &[r],
                )
            };
            super::super::queue::full_barrier(g, sub.main);
            let ptr = buf.ptr as usize;
            sub.transient.push(buf);
            // the storage now, as the CPU sees it
            let mem: Vec<u8> = if !tex.mem.is_null()
                && super::super::mem::room(tex.mem as u64) >= tex.l.size
            {
                unsafe { core::slice::from_raw_parts(tex.mem as *const u8, tex.l.size as usize) }
                    .to_vec()
            } else {
                Vec::new()
            };
            let d = dir();
            crate::vklog!(
                "texture dump {f}: ser{serial} {}x{} format {:#x} ({fmt:?}) at {:#x} ({} bytes of storage, layout {:?})",
                tex.d.width,
                tex.d.height,
                tex.d.format,
                tex.mem as u64,
                tex.l.size,
                tex.l
            );
            sub.post.push(Post::Callback(Box::new(move |_g: &Gpu| {
                let src = unsafe { core::slice::from_raw_parts(ptr as *const u8, n as usize) };
                let _ = std::fs::create_dir_all(&d);
                let _ = std::fs::write(d.join(format!("tex-{f:06}-ser{serial}-gpu.bin")), src);
                let _ = std::fs::write(d.join(format!("tex-{f:06}-ser{serial}-mem.bin")), &mem);
            })));
        }
    }

    pub fn pass_end(g: &Gpu, sub: &mut Sub, qs: &QState) {
        let f = FRAME.load(Ordering::Relaxed);
        if f == 0 || super::super::present::frames() + 1 != f {
            return;
        }
        dump_textures(g, sub, f);
        let p = PASS.fetch_add(1, Ordering::Relaxed);
        if render_chain::enabled() {
            chain_pass_end(
                g,
                sub,
                qs,
                render_chain::Point {
                    frame: f,
                    submission: sub.serial,
                    pass: p,
                },
            );
        }
        let mut desc = vec![];
        for i in 0..qs.rt_n {
            let Some(t) = qs.rt[i].tex else { continue };
            let Some(tf) = crate::tex::format::tex_format(t.d.format) else {
                continue;
            };
            let vw = qs.rt[i]
                .view
                .map(|v| {
                    format!(
                        " v{:#x} L{}+{} lv{}",
                        v.bits, v.min_layer, v.nlayers, v.base_level
                    )
                })
                .unwrap_or_default();
            desc.push(format!(
                "{i}:{:?} {}x{}x{} ser{} @{:p}{vw}",
                tf.vk, t.d.width, t.d.height, t.d.depth, t.serial, t.mem
            ));
            let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
            if bw != 1
                || bh != 1
                || bpb == 0
                || !matches!(tf.kind, Kind::Float | Kind::Uint | Kind::Sint)
            {
                continue;
            }
            // NXGFX_DUMP_PNG=0: the log lines only (dumps every few seconds over a long run)
            // NXGFX_DUMP_PNG=<w>x<h>: pictures of targets of that size only
            match std::env::var("NXGFX_DUMP_PNG").as_deref() {
                Ok("0") => continue,
                Ok(sz) if sz.contains('x') && sz != format!("{}x{}", t.d.width, t.d.height) => {
                    continue
                }
                _ => {}
            }
            // Bound readback memory while tracing a small part of a large frame.
            if let Ok(passes) = std::env::var("NXGFX_DUMP_PASSES") {
                if !passes.split(',').any(|s| s.trim().parse::<u64>() == Ok(p)) {
                    continue;
                }
            }
            let v = qs.rt[i].view;
            let level = v
                .filter(|v| v.bits & VW_LEVELS != 0)
                .map(|v| v.base_level as u32)
                .unwrap_or(0);
            let layer = v
                .filter(|v| v.bits & VW_LAYERS != 0)
                .map(|v| v.min_layer as u32)
                .unwrap_or(0);
            let (w, h) = ((t.d.width >> level).max(1), (t.d.height >> level).max(1));
            let n = w as u64 * h as u64 * bpb as u64;
            let img = {
                g.images
                    .lock()
                    .unwrap()
                    .by_serial
                    .get(&t.serial)
                    .map(|im| im.image)
            };
            let Some(img) = img else { continue };
            let Ok(buf) = g.buffer(
                n,
                vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuToCpu,
                "pass dump",
            ) else {
                continue;
            };
            let r = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: level,
                    base_array_layer: layer,
                    layer_count: 1,
                },
                image_offset: vk::Offset3D::default(),
                image_extent: vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                },
            };
            unsafe {
                g.dev().cmd_copy_image_to_buffer(
                    sub.main,
                    img,
                    vk::ImageLayout::GENERAL,
                    buf.buf,
                    &[r],
                )
            };
            super::super::queue::full_barrier(g, sub.main);
            let ptr = buf.ptr as usize;
            let path = dir().join(format!("pass-{f:06}-{p:03}-t{i}-{w}x{h}-{:?}.png", tf.vk));
            let fmt = tf.vk;
            sub.transient.push(buf);
            sub.post.push(Post::Callback(Box::new(move |_g: &Gpu| {
                let np = (w * h) as usize;
                // SAFETY: the read-back buffer, kept until after this callback
                let src = unsafe { core::slice::from_raw_parts(ptr as *const u8, n as usize) };
                if std::env::var("NXGFX_DUMP_RAW").as_deref() == Ok("1") {
                    let _ = std::fs::create_dir_all(dir());
                    let _ = std::fs::write(path.with_extension("bin"), src);
                }
                let mut out = vec![0u8; np * 4];
                if super::super::shot::to_rgba8(fmt, src, np, &mut out) {
                    let _ = std::fs::create_dir_all(dir());
                    let _ = super::super::shot::png_write(&path, &out, w, h);
                }
            })));
        }
        let ds = qs
            .rt_depth
            .tex
            .map(|t| {
                format!(
                    " depth {}x{} fmt {:#x} ser{} @{:p}",
                    t.d.width, t.d.height, t.d.format, t.serial, t.mem
                )
            })
            .unwrap_or_default();
        crate::vklog!(
            "pass dump {f}/{p}: extent {}x{}, targets [{}]{ds}",
            qs.pass_extent.width,
            qs.pass_extent.height,
            desc.join(", ")
        );
    }
}

/// NVN2's DrawMeshTasks*: the bound task and mesh shaders through VK_EXT_mesh_shader. Direct: `x` (`y`, `z`)
/// workgroups. Indirect: the game's records ({count, first}, 3D {x, y, z}) are written by the GPU earlier in the same
/// submission, so the workgroup counts are copied on the GPU into Vulkan's {x, y, z} records (outside the pass) and
/// drawn from there; with a count buffer, MultiDraw…IndirectCount reads the draw count on the GPU too.
/// A checkpoint before a draw or dispatch (vk/lost.rs: what the GPU was doing if it stops responding).
fn checkpoint(g: &Gpu, sub: &Sub, qs: &QState, direct: bool, what: impl FnOnce() -> String) -> u64 {
    super::lost::checkpoint(g, sub.main, direct, || {
        let keys: Vec<String> = qs
            .prog
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                m.as_ref()
                    .map(|m| format!("{}:{}", i, crate::nvn::prog::key_hex(&m.key)))
            })
            .collect();
        format!("{} with shaders [{}]", what(), keys.join(" "))
    })
}

#[path = "mesh_contract.rs"]
mod mesh_contract;

struct MeshDeviceContract {
    task_grid: mesh_contract::GridLimits,
    mesh_grid: mesh_contract::GridLimits,
    task: mesh_contract::Limits,
    mesh: mesh_contract::Limits,
}

fn mesh_limits(g: &Gpu) -> &'static MeshDeviceContract {
    static L: std::sync::OnceLock<MeshDeviceContract> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut mp = vk::PhysicalDeviceMeshShaderPropertiesEXT::default();
        let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut mp);
        unsafe {
            g.d.instance
                .get_physical_device_properties2(g.d.pdev, &mut p2)
        };
        use mesh_contract::{GridLimits, Limits};
        MeshDeviceContract {
            task_grid: GridLimits { axes: mp.max_task_work_group_count, total: mp.max_task_work_group_total_count },
            mesh_grid: GridLimits { axes: mp.max_mesh_work_group_count, total: mp.max_mesh_work_group_total_count },
            task: Limits {
                local: GridLimits { axes: mp.max_task_work_group_size, total: mp.max_task_work_group_invocations },
                shared: mp.max_task_shared_memory_size, payload: mp.max_task_payload_size,
                combined: mp.max_task_payload_and_shared_memory_size, vertices: 0, primitives: 0,
                output: [0; 5],
            },
            mesh: Limits {
                local: GridLimits { axes: mp.max_mesh_work_group_size, total: mp.max_mesh_work_group_invocations },
                shared: mp.max_mesh_shared_memory_size, payload: mp.max_task_payload_size,
                combined: mp.max_mesh_payload_and_shared_memory_size,
                vertices: mp.max_mesh_output_vertices, primitives: mp.max_mesh_output_primitives,
                output: [mp.max_mesh_output_memory_size, mp.max_mesh_payload_and_output_memory_size,
                    mp.max_mesh_output_components, mp.mesh_output_per_vertex_granularity,
                    mp.mesh_output_per_primitive_granularity],
            },
        }
    })
}

fn mesh_stage_contract(m: &Module, limits: mesh_contract::Limits) -> Result<mesh_contract::Stage, &'static str> {
    thread_local! {
        static CACHE: std::cell::RefCell<Vec<(u64, mesh_contract::Stage)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, stage)) = cache.iter().find(|(id, _)| *id == m.id) { return Ok(stage.clone()); }
        let stage = mesh_contract::reflect(&m.words)?;
        mesh_contract::validate(&stage, limits)?;
        if cache.len() == 128 { cache.remove(0); }
        cache.push((m.id, stage.clone()));
        Ok(stage)
    })
}

static CLAMP_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/clamp_counts_comp.spv"));

/// The clamp kernel (`shaders/clamp_counts.comp`): (layout, pipeline), made once; None when it cannot be.
fn clamp_pipe(g: &Gpu) -> Option<(vk::PipelineLayout, vk::Pipeline)> {
    static P: std::sync::OnceLock<Option<(vk::PipelineLayout, vk::Pipeline)>> =
        std::sync::OnceLock::new();
    *P.get_or_init(|| unsafe {
        if !g.d.caps.bda {
            return None;
        }
        let pcr = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .size(32)];
        let layout = g
            .dev()
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&pcr),
                None,
            )
            .ok()?;
        let w: Vec<u32> = CLAMP_SPV
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let m = g
            .dev()
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&w), None)
            .ok()?;
        let st = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(m)
            .name(c"main");
        let flags = if g.d.caps.descbuf {
            vk::PipelineCreateFlags::DESCRIPTOR_BUFFER_EXT
        } else {
            vk::PipelineCreateFlags::empty()
        };
        let p = g
            .dev()
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .flags(flags)
                    .stage(st)
                    .layout(layout)],
                None,
            )
            .ok()?[0];
        g.dev().destroy_shader_module(m, None);
        Some((layout, p))
    })
}

/// Indirect records `n` x `stride` bytes at (b, o) copied into staging with their first three words held to `maxv`
/// (outside a pass: a copy and a dispatch). A count from garbage made the GPU fault or work for ever (GPU resets).
/// `NXGFX_CLAMP_INDIRECT=0`: the records as they are. Returns where to read them from.
fn clamp_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_CLAMP_INDIRECT").as_deref() != Ok("0"))
}

/// `n` records of `stride` bytes, the last `last` bytes long; `maxv`: the first three words' limits, and (when not 0) the
/// limit of word 0 x word 1.
fn clamped_indirect(
    g: &Gpu,
    sub: &mut Sub,
    b: vk::Buffer,
    o: u64,
    n: u32,
    stride: u32,
    last: u32,
    maxv: [u32; 4],
) -> (vk::Buffer, u64) {
    if !clamp_on() || n == 0 || stride % 4 != 0 || last < 12 {
        return (b, o);
    }
    let Some((layout, pipe)) = clamp_pipe(g) else {
        return (b, o);
    };
    let bytes = (n as u64 - 1) * stride as u64 + last as u64;
    let (sb, so) = sub.stage_bytes_fresh(g, &vec![0u8; bytes as usize]);
    let Some(addr) = sub.addr_of(sb).map(|a| a + so) else {
        return (b, o);
    };
    let dev = g.dev();
    unsafe {
        dev.cmd_copy_buffer(
            sub.main,
            b,
            sb,
            &[vk::BufferCopy {
                src_offset: o,
                dst_offset: so,
                size: bytes,
            }],
        );
        super::queue::full_barrier(g, sub.main);
        dev.cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, pipe);
        let pc: [u32; 8] = [
            addr as u32,
            (addr >> 32) as u32,
            n,
            stride / 4,
            maxv[0],
            maxv[1],
            maxv[2],
            maxv[3],
        ];
        dev.cmd_push_constants(
            sub.main,
            layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            core::slice::from_raw_parts(pc.as_ptr() as *const u8, 32),
        );
        dev.cmd_dispatch(sub.main, 1, 1, 1);
        super::queue::full_barrier(g, sub.main);
    }
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| crate::vklog!("indirect counts are held to the device's limits before they are read (NXGFX_CLAMP_INDIRECT=0 off)"));
    (sb, so)
}

/// The device's compute workgroup count limits.
fn compute_limits(g: &Gpu) -> [u32; 4] {
    let l = g.d.props.limits.max_compute_work_group_count;
    [l[0], l[1], l[2], 0]
}

/// An indirect draw's limits: 16M vertices or indices, 256K instances, 256M of the two together (a garbage record
/// drew for seconds; NXGFX_DRAW_LIMIT = the product's limit in millions).
fn draw_limits() -> [u32; 4] {
    static P: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let prod = *P.get_or_init(|| {
        std::env::var("NXGFX_DRAW_LIMIT")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .map(|m| m.saturating_mul(1 << 20))
            .unwrap_or(1 << 28)
    });
    [1 << 24, 1 << 18, u32::MAX, prod]
}

/// A plain indirect draw's record, held to `draw_limits` before the draw reads it. The copy and the dispatch may not be
/// inside a render pass: the pass ends here (the draw's set-up begins it again).
fn clamped_draw_record(
    g: &Gpu,
    sub: &mut Sub,
    qs: &mut QState,
    ind: u64,
    rec: u32,
) -> Option<(vk::Buffer, u64)> {
    let (b, o, _) = super::mem::resolve(g, sub, ind, rec as u64)?;
    if !clamp_on() {
        return Some((b, o));
    }
    end_pass(g, sub, qs);
    let used = clamped_indirect(g, sub, b, o, 1, rec, rec, draw_limits());
    if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
        p.before_indirect(g, sub, ind, (b, o), used, rec);
    }
    Some(used)
}

fn mesh_draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, m: crate::nvn::cmd2::p_mesh) {
    let draw_checkpoint = checkpoint(g, sub, qs, false, || {
        format!(
            "a mesh draw ({}, {} {}x{}x{}, first {}, n {}, stride {})",
            if m.ind != 0 { "indirect" } else { "direct" },
            if m.three_d != 0 { "3D" } else { "1D" },
            m.x,
            m.y,
            m.z,
            m.first,
            m.n,
            m.stride
        )
    });
    // a mesh pipeline has no vertex input: the next draw sets it again
    qs.vin_last.borrow_mut().0 = 0;
    if !g.d.caps.mesh {
        crate::log::red_once("task and mesh draws: the device has no VK_EXT_mesh_shader, they are not drawn");
        super::perf::inc(super::perf::C::DrawsSkipped);
        return;
    }
    let limits = mesh_limits(g);
    let has_task = qs.prog[3].as_ref().is_some_and(|m| m.mesh == 1);
    let contract = (|| {
        let mesh = qs.prog[4].as_ref().filter(|m| m.mesh == 2).ok_or("missing mesh stage")?;
        let mesh = mesh_stage_contract(mesh, limits.mesh)?;
        if let Some(task) = qs.prog[3].as_ref().filter(|m| m.mesh == 1) {
            let task = mesh_stage_contract(task, limits.task)?;
            if task.payload != mesh.payload { return Err("task/mesh payload interface mismatch"); }
        } else if mesh.payload != 0 { return Err("taskless mesh declares task payload"); }
        Ok::<_, &'static str>(())
    })();
    // A draw outside the device's mesh contract is not recorded: skipping it is GPU-safe, so the process
    // continues (status 72 is reserved for states that cannot be recovered).
    if let Err(why) = contract {
        crate::log::red_once(&format!("mesh draw not drawn: {why}"));
        super::perf::inc(super::perf::C::DrawsSkipped);
        return;
    }
    let indirect = m.ind != 0;
    // Indirect execution remains the held pre-alignment path. Its GPU-generated
    // counts/total validation are unresolved; CPU admission must not reject every
    // valid indirect draw or pretend a CPU read proves its GPU arguments.
    // NXGFX_MESH_REF=1 (diagnosis, lane K): mkw-recomp's rule for MKW's mesh draws: only direct 1D draws, from group 0
    // (its `first` ignored); indirect, 3D and multi draws skipped
    static REF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // 1 = both; skip = only the skipping; first0 = only `first` dropped
    let mode =
        REF.get_or_init(|| std::env::var("NXGFX_MESH_REF").is_ok_and(|v| v == "1" || v == "skip"));
    static FIRST0: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let first0 = *FIRST0
        .get_or_init(|| std::env::var("NXGFX_MESH_REF").is_ok_and(|v| v == "1" || v == "first0"));
    // skipind / skip3d (diagnosis): only the indirect, or only the 3D, mesh draws skipped
    static PART: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    let (skip_ind, skip_3d) =
        *PART.get_or_init(|| match std::env::var("NXGFX_MESH_REF").as_deref() {
            Ok("skipind") => (true, false),
            Ok("skip3d") => (false, true),
            _ => (false, false),
        });
    if (*mode && (indirect || m.three_d != 0))
        || (skip_ind && indirect)
        || (skip_3d && m.three_d != 0)
    {
        crate::log::red_once(
            "NXGFX_MESH_REF: indirect / 3D mesh draws skipped (mkw-recomp's rule)",
        );
        super::perf::inc(super::perf::C::DrawsSkipped);
        return;
    }
    let m = if first0 {
        crate::nvn::cmd2::p_mesh { first: 0, ..m }
    } else {
        m
    };
    // indirect: build {x, y, z} records first (a copy is not allowed inside a pass)
    let mut args: Option<(vk::Buffer, u64, u32)> = None;
    let mut rec_copy: Option<(vk::Buffer, u64, u64)> = None;
    if indirect {
        let plan = mesh_contract::indirect_layout(m.n, m.stride, m.three_d != 0);
        let (n, src_stride, span) = (plan.count, plan.stride, plan.span);
        let Some((sb, so, _)) = super::mem::resolve(g, sub, m.ind, span) else {
            crate::log::red_once(&format!(
                "DrawMeshTasksIndirect record at {:#x} is in no memory pool: draw skipped",
                m.ind
            ));
            super::perf::inc(super::perf::C::DrawsSkipped);
            return;
        };
        end_pass(g, sub, qs);
        let ones: Vec<u8> = (0..n)
            .flat_map(|_| [0u32, 1, 1])
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let (ab, ao) = sub.stage_bytes_fresh(g, &ones);
        let dev = g.dev();
        let regions: Vec<vk::BufferCopy> = (0..n as u64)
            .map(|i| vk::BufferCopy {
                src_offset: so + i * src_stride,
                dst_offset: ao + i * 12,
                size: plan.count_bytes,
            })
            .collect();
        unsafe { dev.cmd_copy_buffer(sub.main, sb, ab, &regions) };
        // the records the task shader reads `first` from: a copy of their own, taken here with the counts (an aligned
        // binding, and the values the draw's counts came from)
        let (rb, ro) = sub.stage_bytes_fresh(g, &vec![0u8; span as usize]);
        unsafe {
            dev.cmd_copy_buffer(
                sub.main,
                sb,
                rb,
                &[vk::BufferCopy {
                    src_offset: so,
                    dst_offset: ro,
                    size: span,
                }],
            )
        };
        rec_copy = Some((rb, ro, span));
        barrier(g, sub);
        // Restore the prior clamp plumbing with the prior task-axis limits. This
        // is scope containment under the GPU hold, not a complete indirect fix.
        let l = limits.task_grid.axes;
        let (ab, ao) = clamped_indirect(g, sub, ab, ao, n, 12, 12, [l[0], l[1], l[2], 0]);
        args = Some((ab, ao, n));
    }
    // the draw records the task / mesh shader reads its first workgroup from (lane S: `mesh_draw`, stride at push
    // constant byte 12): the game's records for an indirect draw, a staged {count, first} for a direct one, zero for 3D
    if indirect {
        qs.mesh_rec = rec_copy;
        qs.mesh_stride = mesh_contract::indirect_layout(m.n, m.stride, m.three_d != 0).shader_stride;
        if m.three_d != 0 {
            let (b, o) = sub.stage_bytes(g, &[0u8; 16]);
            qs.mesh_rec = Some((b, o, 16));
        }
    } else {
        let rec: [u32; 4] = [
            m.x.max(0) as u32,
            if m.three_d != 0 {
                0
            } else {
                m.first.max(0) as u32
            },
            0,
            0,
        ];
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
            let (x, y, z) = if m.three_d != 0 {
                (m.x, m.y, m.z)
            } else {
                (m.x, 1, 1)
            };
            let limits = if has_task { limits.task_grid } else { limits.mesh_grid };
            let groups = (|| {
                let groups = [u32::try_from(x), u32::try_from(y), u32::try_from(z)];
                let groups = [groups[0].clone().map_err(|_| "negative mesh grid")?,
                    groups[1].clone().map_err(|_| "negative mesh grid")?,
                    groups[2].clone().map_err(|_| "negative mesh grid")?];
                mesh_contract::grid(groups, limits)?;
                Ok::<_, &'static str>(groups)
            })();
            // Guest grid values outside the device limits (or negative) are skipped, not clamped:
            // a partial grid would draw a wrong result.
            let [x, y, z] = match groups {
                Ok(groups) => groups,
                Err(why) => {
                    crate::log::red_once(&format!("mesh draw not drawn: {why}"));
                    super::perf::inc(super::perf::C::DrawsSkipped);
                    [0, 0, 0]
                }
            };
            if x > 0 && y > 0 && z > 0 {
                mesh_binding_evidence(sub, qs, draw_checkpoint);
                unsafe { ms.cmd_draw_mesh_tasks(sub.main, x, y, z) };
            }
        }
        Some((ab, ao, n)) => {
            if m.cnt != 0 {
                match super::mem::resolve(g, sub, m.cnt, 4) {
                    Some((cb, co, _)) => {
                        mesh_binding_evidence(sub, qs, draw_checkpoint);
                        unsafe {
                            ms.cmd_draw_mesh_tasks_indirect_count(sub.main, ab, ao, cb, co, n, 12)
                        }
                    }
                    None => crate::log::red_once(&format!(
                        "mesh draw count at {:#x} is in no memory pool: draw skipped",
                        m.cnt
                    )),
                }
            } else {
                stale_check(sub, ab, "mesh draw records");
                mesh_binding_evidence(sub, qs, draw_checkpoint);
                unsafe { ms.cmd_draw_mesh_tasks_indirect(sub.main, ab, ao, n, 12) };
            }
        }
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let k = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if k & 0xfff == 0 {
        crate::vklog!(
            "mesh draws so far: {}; this one {} ({} {} {}), indirect {}",
            k + 1,
            if m.three_d != 0 { "3D" } else { "1D" },
            m.x,
            m.y,
            m.z,
            indirect
        );
    }
    qs.mesh_rec = None;
    qs.mesh_stride = 0;
}

fn mesh_binding_evidence(sub: &Sub, qs: &QState, checkpoint: u64) {
    use super::lost::bindings;
    if !bindings::enabled() || checkpoint == 0 {
        return;
    }
    let (missing_stages, stages) = qs
        .sets
        .borrow()
        .capture(|r| super::lost::binding_id(r.buffer, r.address));
    bindings::record(bindings::Draw {
        checkpoint,
        setup_last: super::lost::last_recorded_checkpoint().max(checkpoint),
        serial: sub.serial,
        command_buffer: vk::Handle::as_raw(sub.main),
        frame: sub.frame,
        pool_epoch: super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire),
        missing_stages,
        stages,
    });
}

/// MultiDraw{Arrays,Elements}IndirectCount: Vulkan's indirect draws with a count the GPU reads.
fn multi_draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, m: crate::nvn::cmd2::p_multi_draw) {
    checkpoint(g, sub, qs, false, || "an indirect multi-draw".to_string());
    if m.max <= 0 {
        return;
    }
    let indexed = m.itype >= 0;
    let base = p_draw {
        prim: m.prim,
        instances: 1,
        ..Default::default()
    };
    let rec_n = if indexed { 20 } else { 16 };
    let stride = if m.stride > 0 { m.stride as u32 } else { rec_n };
    let span = stride as u64 * (m.max as u64 - 1) + rec_n as u64;
    let (Some((ib, io, _)), Some((cb, co, _))) = (
        super::mem::resolve(g, sub, m.ind, span),
        super::mem::resolve(g, sub, m.cnt, 4),
    ) else {
        return;
    };
    // the records held to the draw limits first (outside the pass); the count is held to m.max by Vulkan
    let raw = (ib, io);
    let (ib, io) = if clamp_on() && stride % 4 == 0 && m.max <= 4096 {
        end_pass(g, sub, qs);
        clamped_indirect(g, sub, ib, io, m.max as u32, stride, rec_n, draw_limits())
    } else {
        (ib, io)
    };
    if !qs.in_pass {
        if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
            p.before_multi(
                g,
                sub,
                m.ind,
                raw,
                (ib, io),
                rec_n,
                stride,
                m.max as u32,
                (cb, co),
            );
        }
    }
    if !draw_setup(g, sub, qs, base.prim, 0) {
        return;
    }
    let dev = g.dev();
    unsafe {
        if indexed {
            let ity = match m.itype {
                index_type::U8 if g.d.caps.index_u8 => vk::IndexType::UINT8_EXT,
                index_type::U16 => vk::IndexType::UINT16,
                _ => vk::IndexType::UINT32,
            };
            let Some((xb, xo, _)) = super::mem::resolve(g, sub, m.ibuf, 1) else {
                return;
            };
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
    g.materialize(id);
    let Some((serial, _)) = g.tables.tex_at(id) else {
        crate::log::red_once("DrawTexture of an unregistered texture");
        return;
    };
    let serial = g.images.lock().unwrap().live(serial);
    let Some(Rt {
        tex: Some(dst),
        view,
    }) = qs.rt.first().copied()
    else {
        return;
    };
    end_pass(g, sub, qs);
    let src_img = {
        let mut imgs = g.images.lock().unwrap();
        g.prepare_image(&mut imgs, serial, sub);
        imgs.by_serial.get(&serial).map(|im| (im.image, im.tf))
    };
    let Some((si, stf)) = src_img else { return };
    let Some((di, _, _, _)) = target_image(g, sub, &dst) else {
        return;
    };
    let level = view
        .filter(|v| v.bits & VW_LEVELS != 0)
        .map(|v| v.base_level as u32)
        .unwrap_or(0);
    let (ss, ds) = (img_scale(g, serial), img_scale(g, dst.serial));
    let o = |v: [f32; 4], i: usize, s: f32| vk::Offset3D {
        x: (v[i] * s) as i32,
        y: (v[i + 1] * s) as i32,
        z: i as i32 / 2,
    };
    let r = vk::ImageBlit {
        src_subresource: vk::ImageSubresourceLayers {
            aspect_mask: super::image::upload_aspect(&stf),
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        },
        src_offsets: [o(d.src, 0, ss), o(d.src, 2, ss)],
        dst_subresource: vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: level,
            base_array_layer: 0,
            layer_count: 1,
        },
        dst_offsets: [o(d.dst, 0, ds), o(d.dst, 2, ds)],
    };
    unsafe {
        g.dev().cmd_blit_image(
            sub.main,
            si,
            vk::ImageLayout::GENERAL,
            di,
            vk::ImageLayout::GENERAL,
            &[r],
            vk::Filter::LINEAR,
        )
    };
    barrier(g, sub);
}

/// Closes the open occlusion query: its samples join the sum at completion.
fn end_query(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    if let Some(q) = qs.occl_q.take() {
        unsafe { g.dev().cmd_end_query(sub.main, sub.occl, q) };
        sub.post.push(Post::Occl { q });
    }
}

/// Linear render targets the CPU can read (NVN_TEXTURE_FLAGS_LINEAR / LINEAR_RENDER_TARGET in a pool the CPU can
/// access; Turbo `rt_readback`, on by default): on the console the pool is the CPU's memory too, so what the GPU draws
/// there the CPU reads (Mario Kart World draws its decoded animations into an 8x1218 RGBA16F linear target that its CPU
/// animation decoder reads). At the end of a pass each such target's level / layer is copied into its storage, which
/// reaches guest memory at the submission's fence (the write-back). `NXGFX_RT_READBACK=0` off.
fn linear_rt_readback(g: &Gpu, sub: &mut Sub, qs: &QState) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_RT_READBACK").as_deref() != Ok("0")) {
        return;
    }
    use crate::nvn::types::{mem_flags, tex_flags};
    for i in 0..qs.rt_n {
        let Some(t) = qs.rt[i].tex else { continue };
        let lin = t.d.flags as u32 & (tex_flags::LINEAR | tex_flags::LINEAR_RENDER_TARGET) != 0;
        if !lin
            || t.mem.is_null()
            || t.pool.is_null()
            || t.d.target == target::T3D
            || t.d.stride == 0
        {
            continue;
        }
        if unsafe { (*t.pool).flags } & mem_flags::CPU_NO_ACCESS != 0 {
            continue;
        }
        let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
        if bw != 1 || bh != 1 || bpb == 0 || img_scale(g, t.serial) != 1.0 {
            continue;
        }
        let v = qs.rt[i].view;
        let level = v
            .filter(|v| v.bits & crate::nvn::obj::VW_LEVELS != 0)
            .map(|v| v.base_level as u32)
            .unwrap_or(0);
        let layer = v
            .filter(|v| v.bits & crate::nvn::obj::VW_LAYERS != 0)
            .map(|v| v.min_layer as u32)
            .unwrap_or(0);
        if level != 0 {
            continue; // a linear texture has one level
        }
        let (w, h) = (t.d.width.max(1), t.d.height.max(1));
        let row = t.d.stride as u64;
        let bytes = row * (h as u64 - 1) + w as u64 * bpb as u64;
        let addr = t.mem as u64 + layer as u64 * t.l.layer_size;
        let image = {
            let imgs = g.images.lock().unwrap();
            let Some(im) = imgs.by_serial.get(&t.serial) else {
                continue;
            };
            if im.tf.astc.is_some() || im.tf.expand3 != 0 || im.bc7 {
                continue;
            }
            im.image
        };
        let Some((b, o, _)) = super::mem::resolve(g, sub, addr, bytes) else {
            continue;
        };
        if row % bpb as u64 != 0 {
            continue;
        }
        let r = vk::BufferImageCopy {
            buffer_offset: o,
            buffer_row_length: (row / bpb as u64) as u32,
            buffer_image_height: h,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: layer,
                layer_count: 1,
            },
            image_offset: vk::Offset3D::default(),
            image_extent: vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
        };
        unsafe {
            g.dev()
                .cmd_copy_image_to_buffer(sub.main, image, vk::ImageLayout::GENERAL, b, &[r])
        };
        super::mem::note_gpu_write(g, sub, addr, bytes);
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
            crate::vklog!(
                "linear render target ser{} {w}x{h} format {:#x} stride {row} at {addr:#x}: read back into its memory for the CPU (the console's shared memory)",
                t.serial,
                t.d.format
            );
        }
        barrier(g, sub);
    }
}

fn end_pass(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    end_query(g, sub, qs);
    if qs.in_pass {
        unsafe { g.dev().cmd_end_rendering(sub.main) };
        qs.in_pass = false;
        barrier(g, sub);
        if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
            p.after_init_pass(g, sub);
        }
        passdump::pass_end(g, sub, qs);
        linear_rt_readback(g, sub, qs);
        // compressed targets drawn through a block alias: the texels become the blocks
        // (through a buffer: an image copy cannot name a level smaller than one block, a buffer copy can)
        for (src, dst, level, layer, e, texels, bpb) in qs.copyback.drain(..) {
            let size = e.width as u64 * e.height as u64 * bpb as u64;
            let Ok(tmp) = g.buffer(
                size,
                vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST,
                MemoryLocation::GpuOnly,
                "block copy",
            ) else {
                continue;
            };
            let sub_l = |mip, layer| vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: mip,
                base_array_layer: layer,
                layer_count: 1,
            };
            let up = vk::BufferImageCopy {
                buffer_offset: 0,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: sub_l(0, 0),
                image_offset: vk::Offset3D::default(),
                image_extent: vk::Extent3D {
                    width: e.width,
                    height: e.height,
                    depth: 1,
                },
            };
            let down = vk::BufferImageCopy {
                image_subresource: sub_l(level, layer),
                image_extent: vk::Extent3D {
                    width: texels.width,
                    height: texels.height,
                    depth: 1,
                },
                ..up
            };
            unsafe {
                g.dev().cmd_copy_image_to_buffer(
                    sub.main,
                    src,
                    vk::ImageLayout::GENERAL,
                    tmp.buf,
                    &[up],
                );
                super::queue::full_barrier(g, sub.main);
                g.dev().cmd_copy_buffer_to_image(
                    sub.main,
                    tmp.buf,
                    dst,
                    vk::ImageLayout::GENERAL,
                    &[down],
                );
            }
            barrier(g, sub);
            sub.transient.push(tmp);
        }
    }
}

pub fn end(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    flush_ndve(g, sub, qs);
    end_pass(g, sub, qs);
}

/// The image of a texture snapshot for writing (render target, copy destination): made GPU-owned, prepared in the
/// upload command buffer when new. Returns (image, format info, extent, levels, layers, samples).
fn target_image(
    g: &Gpu,
    sub: &mut Sub,
    t: &tex_t,
) -> Option<(
    vk::Image,
    format::TexFormat,
    vk::Extent3D,
    vk::SampleCountFlags,
)> {
    let mut imgs = g.images.lock().unwrap();
    if !g.ensure_image(&mut imgs, t) {
        return None;
    }
    g.prepare_image(&mut imgs, t.serial, sub);
    g.tex_gpu_written(&mut imgs, t.serial);
    static WB: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *WB.get_or_init(|| std::env::var("NXGFX_TEX_WB").as_deref() == Ok("1"))
        && t.l.size <= 512 << 10
    {
        sub.tex_wb.push(t.serial);
    }
    let im = &imgs.by_serial[&t.serial];
    Some((im.image, im.tf, im.extent, im.samples))
}
fn source_image(
    g: &Gpu,
    sub: &mut Sub,
    t: &tex_t,
) -> Option<(vk::Image, format::TexFormat, vk::Extent3D)> {
    let mut imgs = g.images.lock().unwrap();
    if !g.ensure_image(&mut imgs, t) {
        return None;
    }
    g.prepare_image(&mut imgs, t.serial, sub);
    let im = &imgs.by_serial[&t.serial];
    Some((im.image, im.tf, im.extent))
}
/// The resolution scale of a texture's image (1 when nothing is scaled).
fn img_scale(g: &Gpu, serial: u64) -> f32 {
    if !super::scale::active() {
        return 1.0;
    }
    g.images
        .lock()
        .unwrap()
        .by_serial
        .get(&serial)
        .map(|im| im.scale)
        .unwrap_or(1.0)
}

/// The layers a render target's view covers (a layered pass draws into all of them).
fn attach_layers(im: &super::image::Image, v: Option<&view_t>) -> u32 {
    let lvl = v
        .filter(|v| v.bits & VW_LEVELS != 0)
        .map(|v| v.base_level as u32)
        .unwrap_or(0);
    let total = if im.ty == vk::ImageType::TYPE_3D {
        (im.extent.depth >> lvl).max(1)
    } else {
        im.layers
    };
    match v.filter(|v| v.bits & VW_LAYERS != 0) {
        Some(v) => (v.nlayers as u32)
            .max(1)
            .min(total.saturating_sub(v.min_layer as u32).max(1)),
        None => total,
    }
}

fn attach_view_layers(
    g: &Gpu,
    sub: &mut Sub,
    qs: &mut QState,
    t: &tex_t,
    v: Option<&view_t>,
) -> Option<(
    (
        vk::ImageView,
        vk::Format,
        vk::Extent2D,
        vk::SampleCountFlags,
        Kind,
    ),
    u32,
)> {
    let _ = target_image(g, sub, t)?;
    let mut imgs = g.images.lock().unwrap();
    let im = imgs.by_serial.get_mut(&t.serial)?;
    let lvl = v
        .filter(|v| v.bits & VW_LEVELS != 0)
        .map(|v| v.base_level as u32)
        .unwrap_or(0);
    let layer = v
        .filter(|v| v.bits & VW_LAYERS != 0)
        .map(|v| v.min_layer as u32)
        .unwrap_or(0);
    let e = vk::Extent2D {
        width: (im.extent.width >> lvl).max(1),
        height: (im.extent.height >> lvl).max(1),
    };
    let fmt = g.raw_view_format(im, v);
    let (samples, kind) = (im.samples, im.tf.kind);
    // a compressed texture rendered through an uncompressed view of its block size (a game encoding blocks on the
    // GPU): Vulkan cannot render into a compressed image, so the pass draws into an image of the view's format, one
    // texel per block, copied into the blocks at the end of the pass
    if !im.usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) && fmt != im.format {
        let (_, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
        if bw > 1 || bh > 1 || im.bc7 {
            let (bw, bh) = if im.bc7 { (4, 4) } else { (bw, bh) };
            let be = vk::Extent2D {
                width: e.width.div_ceil(bw),
                height: e.height.div_ceil(bh),
            };
            let dst = im.image;
            let im_bc7 = im.bc7;
            drop(imgs);
            let (img, view) = block_alias(g, sub, fmt, be)?;
            let bpb = crate::tex::layout::fmt_bpb(t.d.format)
                .0
                .max(if im_bc7 { 16 } else { 0 });
            qs.copyback.push((img, dst, lvl, layer, be, e, bpb));
            return Some(((view, fmt, be, samples, Kind::Float), 1));
        }
    }
    let fmt = g.view_format(im, v);
    let layers = attach_layers(im, v);
    let view = g.image_view(im, v, true)?;
    super::image::life_log(t, || {
        format!(
            "a pass's attachment: view {:#x} of image {:#x} ({} layers)",
            vk::Handle::as_raw(view),
            vk::Handle::as_raw(im.image),
            layers
        )
    });
    Some(((view, fmt, e, samples, kind), layers))
}

/// An uncompressed image standing in for a compressed render target (one texel per block), kept per format and size.
fn block_alias(
    g: &Gpu,
    sub: &mut Sub,
    fmt: vk::Format,
    e: vk::Extent2D,
) -> Option<(vk::Image, vk::ImageView)> {
    type Alias = (vk::Format, vk::Extent2D, vk::Image, vk::ImageView);
    static ALIASES: std::sync::Mutex<Vec<Alias>> = std::sync::Mutex::new(Vec::new());
    let mut a = ALIASES.lock().unwrap();
    if let Some(x) = a.iter().find(|x| x.0 == fmt && x.1 == e) {
        return Some((x.2, x.3));
    }
    let ci = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(fmt)
        .extent(vk::Extent3D {
            width: e.width,
            height: e.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .usage(
            vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
        )
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
    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let vci = vk::ImageViewCreateInfo::default()
        .image(img)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(fmt)
        .subresource_range(range);
    let view = unsafe { g.dev().create_image_view(&vci, None) }.ok()?;
    let b = [vk::ImageMemoryBarrier2::default()
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::GENERAL)
        .image(img)
        .subresource_range(range)];
    unsafe {
        g.dev().cmd_pipeline_barrier2(
            sub.pre,
            &vk::DependencyInfo::default().image_memory_barriers(&b),
        )
    };
    crate::vklog!(
        "compressed render target: an uncompressed {fmt:?} {}x{} stands in for its blocks",
        e.width,
        e.height
    );
    a.push((fmt, e, img, view));
    Some((img, view))
}

fn begin_pass(g: &Gpu, sub: &mut Sub, qs: &mut QState) -> bool {
    if qs.in_pass {
        return true;
    }
    let mut colors = Vec::new();
    let mut formats = [0i32; 8];
    let mut extent = vk::Extent2D {
        width: u32::MAX,
        height: u32::MAX,
    };
    let mut samples = vk::SampleCountFlags::TYPE_1;
    let mut layers = u32::MAX;
    for i in 0..qs.rt_n {
        let r = qs.rt[i];
        let a = r
            .tex
            .and_then(|t| attach_view_layers(g, sub, qs, &t, r.view.as_ref()));
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
            None => colors
                .push(vk::RenderingAttachmentInfo::default().image_view(vk::ImageView::null())),
        }
    }
    let mut depth = None;
    let mut dfmt = 0;
    if let Some(t) = qs.rt_depth.tex {
        let dv = qs.rt_depth.view;
        if let Some(((view, fmt, e, s, kind), nl)) = attach_view_layers(g, sub, qs, &t, dv.as_ref())
        {
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
    // the targets' resolution scale (render targets are scaled together; a pass mixing scales draws at the first's)
    qs.pass_scale = 1.0;
    if super::scale::active() {
        let mut ss: Vec<f32> = qs.rt[..qs.rt_n]
            .iter()
            .filter_map(|r| r.tex)
            .map(|t| img_scale(g, t.serial))
            .collect();
        if let Some(t) = qs.rt_depth.tex {
            ss.push(img_scale(g, t.serial));
        }
        if let Some(&s0) = ss.first() {
            qs.pass_scale = s0;
            if ss.iter().any(|&s| s != s0) {
                crate::log::red_once("resolution scale: a pass draws into targets of different scales (a target the game did not make compressible): drawn at the first one's");
            }
        }
    }
    let da = depth.map(|(v, _)| {
        vk::RenderingAttachmentInfo::default()
            .image_view(v)
            .image_layout(vk::ImageLayout::GENERAL)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
    });
    let mut ri = vk::RenderingInfo::default()
        .render_area(vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent,
        })
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
    // NXGFX_CHECKPOINTS=all: the pass's targets (texture serial, address, view)
    super::lost::checkpoint(g, sub.main, true, || {
        let mut d: Vec<String> = Vec::new();
        let imgs = g.images.lock().unwrap();
        let img = |t: &tex_t| match imgs.by_serial.get(&t.serial) {
            Some(im) => {
                let aliased = imgs
                    .by_serial
                    .values()
                    .filter(|a| a.alias_of == Some(t.serial))
                    .count();
                // the image's memory: its allocation's device memory (as the address binding report saw it) + offset
                let at = im
                    .alloc
                    .as_ref()
                    .and_then(|al| {
                        let mem = unsafe { al.memory() };
                        super::lost::bound_at(vk::Handle::as_raw(mem)).map(|(a, n)| {
                            format!(
                                " in memory {:#x} at {:#x} (+{:#x} of {:#x})",
                                vk::Handle::as_raw(mem),
                                a + al.offset(),
                                al.offset(),
                                n
                            )
                        })
                    })
                    .unwrap_or_default();
                format!(
                    "image {:#x} {}x{}x{} levels {} {:?}{}{}{}{}",
                    vk::Handle::as_raw(im.image),
                    im.extent.width,
                    im.extent.height,
                    im.layers,
                    im.levels,
                    im.format,
                    if im.alias_of.is_some() { " alias" } else { "" },
                    if aliased > 0 {
                        format!(", {aliased} alias image(s) over its memory")
                    } else {
                        String::new()
                    },
                    if im.sparse.is_some() { " sparse" } else { "" },
                    at
                )
            }
            None => "no image".to_string(),
        };
        for i in 0..qs.rt_n {
            if let Some(t) = qs.rt[i].tex {
                d.push(format!(
                    "c{i} ser{} at {:#x} view {:#x} {}",
                    t.serial,
                    t.mem as u64,
                    vk::Handle::as_raw(colors[i].image_view),
                    img(&t)
                ));
            }
        }
        if let (Some(t), Some((v, _))) = (qs.rt_depth.tex, depth) {
            d.push(format!(
                "depth ser{} at {:#x} view {:#x} {}",
                t.serial,
                t.mem as u64,
                vk::Handle::as_raw(v),
                img(&t)
            ));
        }
        drop(imgs);
        format!(
            "a pass {}x{} into [{}]",
            extent.width,
            extent.height,
            d.join(", ")
        )
    });
    if passdump::active() && render_chain::enabled() {
        qs.chain_views = [vk::ImageView::null(); 9];
        for (slot, attachment) in colors.iter().enumerate() {
            qs.chain_views[slot] = attachment.image_view;
        }
        qs.chain_views[8] = depth.map(|(view, _)| view).unwrap_or_default();
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

/// Vulkan's shader already represents OmapTargets as per-component stores. The ROP mask only needs to suppress a
/// physical target whose location the shader does not write; applying the component nibble again is Metal behavior
/// and can misassociate partial vector stores with attachment channels.
fn fragment_write_masks(chmask: u32, omap: u32, n: usize) -> Vec<vk::ColorComponentFlags> {
    (0..n)
        .map(|i| {
            let shader_writes_location = omap >> (4 * i) & 0xf != 0;
            vk::ColorComponentFlags::from_raw(if shader_writes_location {
                chmask >> (4 * i) & 0xf
            } else {
                0
            })
        })
        .collect()
}

/// SRC1 factors with no second source: SRC1_COLOR / SRC1_ALPHA as ZERO, their ONE_MINUS forms as ONE.
fn zero_src1(f: u32) -> u32 {
    match f {
        16 | 18 => 1,
        17 | 19 => 2,
        _ => f,
    }
}

/// The alpha test applied in the pixel shader (`NXGFX_ALPHA_TEST=0` off).
pub static A_TEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

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
static ZERO: std::sync::OnceLock<super::Buf> = std::sync::OnceLock::new();
/// (diagnosis, lane K2; with `NXGFX_ADDR_REPORT`) a buffer named "staging" or "scratch" that is not this submission's
/// own, about to be read by the GPU: the stale staged range MKW's early faults read (a red line with what binds it)
/// The size a storage buffer bound with size 0 gets at most (`NXGFX_SBUF0_MB`, default 1 MB).
fn sbuf0_cap() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_SBUF0_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1)
            << 20
    })
}

/// NXGFX_SBUF_TO_END=0: read-only storage buffers bound with the size the game gave (as before lane K3).
fn sbuf_to_end() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_SBUF_TO_END").as_deref() != Ok("0"))
}

/// The device range [buffer + o, + n) handed to the GPU, for the loss report (diagnostics, lane K3).
fn use_note(sub: &Sub, b: vk::Buffer, o: u64, n: u64, what: &'static str) {
    if super::lost::level() < 2 {
        return;
    }
    if let Some(a) = sub.addr_of(b).or_else(|| super::mem::buffer_address(b)) {
        super::lost::note_use(a + o, n, what);
    }
}

fn stale_check(sub: &Sub, b: vk::Buffer, what: &str) {
    if let Some(n) = super::lost::buf_name(vk::Handle::as_raw(b)) {
        if (n == "staging" || n == "scratch") && !sub.owns(b) {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 40 {
                crate::log::red(&format!(
                    "stale {n} buffer {:#x} bound by {what} in submission {} (not its own)",
                    vk::Handle::as_raw(b),
                    sub.serial
                ));
            }
        }
    }
}

fn zero_buffer(g: &Gpu, sub: &mut Sub) -> vk::Buffer {
    let mut fresh = false;
    let b = ZERO.get_or_init(|| {
        fresh = true;
        g.buffer(
            65536,
            vk::BufferUsageFlags::UNIFORM_BUFFER
                | vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::VERTEX_BUFFER
                | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            "zero",
        )
        .expect("zero buffer")
    });
    if fresh {
        unsafe {
            g.dev()
                .cmd_fill_buffer(sub.pre, b.buf, 0, vk::WHOLE_SIZE, 0)
        };
        super::queue::full_barrier(g, sub.pre);
    }
    b.buf
}

/// A writable missing binding must never alias ZERO (also used for uniforms and vertex attributes), or another
/// missing binding. DKB's unbound compute slots 14/15 contain atomics and stores. Use non-memoized staging so their
/// writes stay in this binding's submission-owned scratch; read-only fallbacks keep sharing the immutable zero.
fn missing_storage(
    g: &Gpu,
    sub: &mut Sub,
    zero: vk::Buffer,
    written: bool,
) -> vk::DescriptorBufferInfo {
    let (buffer, offset) = if written {
        static BYTES: [u8; 65536] = [0; 65536];
        sub.stage_bytes_fresh(g, &BYTES)
    } else {
        (zero, 0)
    };
    vk::DescriptorBufferInfo {
        buffer,
        offset,
        range: 65536,
    }
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
/// A texture's `cbuf_slot` as (the texture word's bank, the sampler word's bank): Ryujinx packs a separate sampler
/// in another bank as `texture slot | (sampler slot + 1) << 16` (Unity's NVN shaders: 0x30002, both words in c2).
fn tex_slots(s: i32) -> (i32, i32) {
    if s < 0 {
        return (s, s);
    }
    let (t, hi) = (s & 0xffff, (s as u32) >> 16);
    (t, if hi != 0 { hi as i32 - 1 } else { t })
}

fn bank_word(
    qs: &QState,
    st: usize,
    m: &Module,
    c0: &[u8],
    c2: &[u8],
    slot: i32,
    word: u32,
) -> u32 {
    let at = 4 * word as usize;
    let rd = |b: &[u8]| -> u32 {
        b.get(at..at + 4)
            .map(|x| u32::from_le_bytes(x.try_into().unwrap()))
            .unwrap_or(0)
    };
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

fn bank_u64(
    qs: &QState,
    st: usize,
    m: &Module,
    c0: &[u8],
    c2: &[u8],
    slot: i32,
    byte: u32,
) -> (u64, u64) {
    let lo = bank_word(qs, st, m, c0, c2, slot, byte / 4) as u64;
    let hi = bank_word(qs, st, m, c0, c2, slot, byte / 4 + 1) as u64;
    let n = bank_word(qs, st, m, c0, c2, slot, byte / 4 + 2) as u64;
    (lo | hi << 32, n)
}

/// The NVN target a shader's sampler type asks for (dimension | array 0x100 | multisample 0x400).
fn target_of(ty: u32) -> i32 {
    let arr = ty & 0x100 != 0;
    match ty & 0xff {
        1 => {
            if arr {
                target::T1D_ARRAY
            } else {
                target::T1D
            }
        }
        4 => target::T3D,
        5 => {
            if arr {
                target::CUBE_ARRAY
            } else {
                target::CUBE
            }
        }
        _ if ty & 0x400 != 0 => {
            if arr {
                target::T2D_MS_ARRAY
            } else {
                target::T2D_MS
            }
        }
        _ => {
            if arr {
                target::T2D_ARRAY
            } else {
                target::T2D
            }
        }
    }
}

/// The view of a registered texture ID with the type the shader declares; a stand-in when there is none.
/// `ty`: the shader's texture type, and in bits 16.. the format a storage image is declared with (Res::Img::fmt).
fn tex_view(g: &Gpu, sub: &mut Sub, id: i32, ty: u32, storage: bool) -> vk::ImageView {
    let gen = g.tables.generation();
    if let Some(&v) = sub.memo_views.get(&(id, ty, storage, gen)) {
        return v;
    }
    let v = tex_view_uncached(g, sub, id, ty, storage);
    if gen == g.tables.generation() {
        sub.memo_views.insert((id, ty, storage, gen), v);
    }
    v
}

fn note_storage_write(
    g: &Gpu,
    sub: &mut Sub,
    imgs: &mut super::image::Images,
    serial: u64,
    view: vk::ImageView,
) {
    let serial = imgs.live(serial);
    // A stand-in receives no writes to the registered texture.
    let real = imgs.by_serial.get(&serial).is_some_and(|im| {
        im.usage.contains(vk::ImageUsageFlags::STORAGE) && im.views.values().any(|&v| v == view)
    });
    if real {
        g.tex_gpu_written(imgs, serial);
        static WB: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *WB.get_or_init(|| std::env::var("NXGFX_TEX_WB").as_deref() == Ok("1"))
            && imgs.by_serial[&serial].tex.l.size <= 512 << 10
        {
            sub.tex_wb.push(serial);
        }
    }
}

/// Pool reflection omits individual Res::Img entries. Inspect its already-remapped bindings once per module;
/// only a shader declaring the storage-image pool can take ownership (a sampled-only pool shader cannot).
fn uses_storage_pool(m: &Module) -> bool {
    if !m.uses_pool {
        return false;
    }
    static CACHE: std::sync::Mutex<
        Option<std::collections::HashMap<(usize, crate::nvn::prog::ShaderKey), bool>>,
    > = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    *cache
        .get_or_insert_with(Default::default)
        .entry((m as *const Module as usize, m.key))
        .or_insert_with(|| {
            super::shader::reflect(&m.words).is_ok_and(|r| {
                r.vars
                    .iter()
                    .any(|&(_, set, binding)| set == L::POOL_SET && binding == L::B_IMG)
            })
        })
}

/// Resolve storage use before opening a pass or consulting descriptor caches. Ownership is a property of this
/// draw/dispatch, not of a cache miss or optional CPU readback. All upgrades precede resolution of sampled views
/// in every stage, so a shader sampling and storing the same backing cannot retain a pre-upgrade view.
fn prepare_storage_bindings(
    g: &Gpu,
    sub: &mut Sub,
    qs: &mut QState,
    mods: &[Arc<Module>],
    dispatch: [u32; 3],
    base_instance: u32,
) {
    for m in mods {
        if !m
            .res
            .iter()
            .any(|(_, r)| matches!(r, Res::Img { ty, .. } if *ty != 2))
        {
            continue;
        }
        let st = m.stage as usize;
        let (c0, c2, _, _) = qs.banks(g, sub, st, dispatch, base_instance);
        let tb = pool_bases(qs).0;
        for (_, r) in &m.res {
            let Res::Img {
                handle,
                cbuf_slot,
                ty,
                fmt,
            } = r
            else {
                continue;
            };
            if *ty == 2 {
                continue;
            }
            let word = bank_word(
                qs,
                st,
                m,
                &c0,
                &c2,
                tex_slots(*cbuf_slot).0,
                handle & 0x3fff,
            );
            let id = (word & 0xfffff) as i32 + tb;
            g.materialize(id);
            let upgrade = g.tables.tex_at(id).is_some_and(|(s, _)| {
                let imgs = g.images.lock().unwrap();
                imgs.by_serial
                    .get(&imgs.live(s))
                    .is_some_and(|im| !im.usage.contains(vk::ImageUsageFlags::STORAGE))
            });
            if upgrade {
                end_pass(g, sub, qs);
            }
            let view = tex_view(g, sub, id, *ty | (*fmt as u32) << 16, true);
            if let Some((serial, _)) = g.tables.tex_at(id) {
                let mut imgs = g.images.lock().unwrap();
                note_storage_write(g, sub, &mut imgs, serial, view);
            }
        }
    }
}

/// NXGFX_STORAGE_PROBE=<texture ID>: four uncached bindings during normal play and four during a pass dump.
/// This distinguishes a missing STORAGE usage from a target mismatch without readbacks or extra image creation.
fn storage_probe_due(id: i32) -> bool {
    use std::sync::atomic::{AtomicU32, Ordering};
    static ID: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    static NORMAL: AtomicU32 = AtomicU32::new(0);
    static DUMP: AtomicU32 = AtomicU32::new(0);
    if *ID.get_or_init(|| std::env::var("NXGFX_STORAGE_PROBE").ok()?.parse().ok()) != Some(id) {
        return false;
    }
    let n = if passdump::active() { &DUMP } else { &NORMAL };
    let used = n.load(Ordering::Relaxed);
    used < 4
        && n.compare_exchange(used, used + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

fn storage_probe_state(g: &Gpu, imgs: &super::image::Images, serial: u64) -> String {
    let Some(im) = imgs.by_serial.get(&serial) else {
        return format!("ser{serial} absent ({})", imgs.whereabouts(serial));
    };
    let owner = im.alias_of.unwrap_or(serial);
    let backing = imgs.by_serial.get(&owner).map(|o| {
        (
            o.alloc.is_some(),
            o.sparse.is_some(),
            o.format,
            o.extent,
            o.tex.d.flags,
        )
    });
    format!(
        "ser{serial} image {:#x} storage {:#x} NVN fmt {:#x} flags {:#x} target {} size {}x{}x{} Vulkan {:?} extent {:?} samples {:?} usage {:?} native-features {:?} alias {:?} owner {owner} backing {backing:?}",
        vk::Handle::as_raw(im.image), im.tex.mem as u64, im.tex.d.format, im.tex.d.flags,
        im.tex.d.target, im.tex.d.width, im.tex.d.height, im.tex.d.depth, im.format,
        im.extent, im.samples, im.usage, g.d.format_features(im.format), im.alias_of,
    )
}

/// Games use a tiny 2D image as an initialized fallback for optional shader slots of any image type. It cannot be a
/// real 3D/cube resource: it has one layer, one sample and no dimension over 16 texels. Bind the shader-typed zero
/// image for that sentinel while retaining target diagnostics for ordinary live textures.
fn generic_typed_placeholder(
    source: i32,
    extent: vk::Extent3D,
    layers: u32,
    samples: vk::SampleCountFlags,
    want: i32,
) -> bool {
    source == target::T2D
        && want != target::T2D
        && extent.width <= 16
        && extent.height <= 16
        && extent.depth == 1
        && layers == 1
        && samples == vk::SampleCountFlags::TYPE_1
}

fn tex_view_uncached(g: &Gpu, sub: &mut Sub, id: i32, ty: u32, storage: bool) -> vk::ImageView {
    let (ty, ifmt) = (ty & 0xffff, (ty >> 16) as u16);
    let want = target_of(ty);
    let probe = storage && storage_probe_due(id);
    if g.tables.placeholder_at(id) {
        return dummy_view(g, sub, want, storage);
    }
    g.materialize(id);
    if let Some((serial, view)) = g.tables.tex_at(id) {
        let mut imgs = g.images.lock().unwrap();
        let serial = imgs.live(serial);
        if probe {
            let declared = crate::tex::format::tex_format(ifmt as i32)
                .map(|f| (f.vk, g.d.format_features(f.vk)));
            crate::vklog!("storage probe ID {id} frame {} pass {} sub {} before: {}; want target {want} shader-format {ifmt:#x} {declared:?} registered-view {view:?} image_noflag {}",
                super::present::frames() + 1, passdump::pass_no(), sub.serial,
                storage_probe_state(g, &imgs, serial), super::image::image_noflag_on());
        }
        // bound as an image without the IMAGE flag (`image_noflag`): its image gets storage use now
        let mut upgraded = 0;
        if storage {
            if let Some(t) = imgs
                .by_serial
                .get(&serial)
                .filter(|im| !im.usage.contains(vk::ImageUsageFlags::STORAGE))
                .map(|im| im.tex)
            {
                if super::image::want_image(&t) || super::image::image_noflag_on() {
                    let cb = sub.main;
                    upgraded = g.upgrade_storage(&mut imgs, &[t.mem as u64], sub, cb);
                }
            }
        }
        if probe {
            crate::vklog!(
                "storage probe ID {id} after: {}; upgraded {upgraded}",
                storage_probe_state(g, &imgs, serial)
            );
        }
        g.prepare_image(&mut imgs, serial, sub);
        if let Some(im) = imgs.by_serial.get_mut(&serial) {
            if generic_typed_placeholder(im.tex.d.target, im.extent, im.layers, im.samples, want) {
                return dummy_view(g, sub, want, storage);
            }
            let mut v = view.unwrap_or_default();
            let base_target = if v.bits & VW_TARGET != 0 {
                v.target as i32
            } else {
                im.tex.d.target
            };
            // a square 2D array of whole cubes is made cube compatible (image.rs): viewed as a cube where the shader
            // samples one (environment maps kept as 2D arrays; Turbo 78ece7b)
            let first = if v.bits & VW_LAYERS != 0 {
                v.min_layer as u32
            } else {
                0
            };
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
                (
                    target::T2D
                    | target::T2D_ARRAY
                    | target::CUBE
                    | target::CUBE_ARRAY
                    | target::RECTANGLE,
                    target::T2D_ARRAY | target::T2D,
                ) => true,
                (target::CUBE_ARRAY, target::CUBE) | (target::CUBE, target::CUBE_ARRAY) => true,
                // a multisampled sampler type over a single-sample texture: the view type is the same in Vulkan
                (target::T2D | target::T2D_MS, target::T2D_MS | target::T2D) => true,
                (
                    target::T2D_ARRAY | target::T2D_MS_ARRAY,
                    target::T2D_MS_ARRAY | target::T2D_ARRAY,
                ) => true,
                _ => false,
            };
            if compatible && (!storage || im.usage.contains(vk::ImageUsageFlags::STORAGE)) {
                if base_target != want {
                    v.bits |= VW_TARGET;
                    v.target = want as u8;
                }
                // the shader's declared format, when the texel is the same size (a bit cast of a mutable-format image)
                static FMT_OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if storage
                    && ifmt != 0
                    && !*FMT_OFF
                        .get_or_init(|| std::env::var("NXGFX_IMG_FORMAT").as_deref() == Ok("0"))
                {
                    let cur = if v.bits & VW_FORMAT != 0 {
                        v.format as i32
                    } else {
                        im.tex.d.format
                    };
                    if cur != ifmt as i32
                        && crate::tex::layout::fmt_bpb(cur)
                            == crate::tex::layout::fmt_bpb(ifmt as i32)
                        && crate::tex::layout::fmt_bpb(cur).1 == 1
                    {
                        v.bits |= VW_FORMAT;
                        v.format = ifmt;
                    }
                }
                let view = if storage {
                    g.storage_view(im, Some(&v))
                } else {
                    g.image_view(im, Some(&v), false)
                };
                if let Some(view) = view {
                    if probe {
                        crate::vklog!(
                            "storage probe ID {id}: real storage view {:#x}",
                            vk::Handle::as_raw(view)
                        );
                    }
                    return view;
                }
            } else if !compatible {
                crate::log::red_once(&format!("texture ID {id}: target {base_target} bound where the shader samples target {want}"));
            } else {
                crate::log::red_once(&format!("texture ID {id} ser{serial}: storage image requested but Vulkan image lacks STORAGE usage ({:?}, {:?}); a zero storage stand-in is bound", im.format, im.usage));
            }
        }
    }
    if probe {
        crate::vklog!("storage probe ID {id}: ZERO storage stand-in, writes do not reach the registered texture");
    }
    {
        // each ID logged at most once every 600 frames, with what its slot holds (no registration, or a texture whose
        // image is gone: Mario Kart 8 Deluxe's black stretches are a tone-mapping input bound as the stand-in)
        static LAST: std::sync::Mutex<Option<std::collections::HashMap<i32, u64>>> =
            std::sync::Mutex::new(None);
        let f = super::present::frames();
        let mut l = LAST.lock().unwrap();
        let m = l.get_or_insert_with(Default::default);
        let due = m.get(&id).is_none_or(|&at| f >= at + 600);
        if due && m.len() < 4096 {
            m.insert(id, f);
            let what = match g.tables.tex_at(id) {
                Some((serial, _)) => format!(
                    "its slot names texture ser{serial}, which has no image now ({})",
                    g.images.lock().unwrap().whereabouts(serial)
                ),
                None => "nothing is registered there".to_string(),
            };
            crate::vklog!(
                "texture ID {id} (target {want}{}) at frame {f}: {what}: a zero stand-in is bound",
                if storage { ", image" } else { "" }
            );
        }
    }
    dummy_view(g, sub, want, storage)
}

/// 1x1 zero images standing in for unbound textures, one per view type.
static DUMMY: std::sync::Mutex<Option<(vk::Image, vk::Image, Vec<(i32, bool, vk::ImageView)>)>> =
    std::sync::Mutex::new(None);

/// The zero stand-in's view for `target` when the stand-in images exist already (made at the first unbound texture);
/// None before. For the texture pool's slots of an image that goes (`image.rs` `scrub_slots`).
pub fn dummy_view_ready(g: &Gpu, target: i32, storage: bool) -> Option<vk::ImageView> {
    if DUMMY.lock().unwrap().is_none() {
        return None;
    }
    let mut d = DUMMY.lock().unwrap();
    Some(dummy_view_of(g, d.as_mut()?, target, storage))
}

fn dummy_view(g: &Gpu, sub: &mut Sub, target: i32, storage: bool) -> vk::ImageView {
    let mut d = DUMMY.lock().unwrap();
    if d.is_none() {
        let mk = |ty: vk::ImageType, layers: u32, cube: bool| {
            let ci = vk::ImageCreateInfo::default()
                .flags(if cube {
                    vk::ImageCreateFlags::CUBE_COMPATIBLE
                } else {
                    vk::ImageCreateFlags::empty()
                })
                .image_type(ty)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D {
                    width: 1,
                    height: 1,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(layers)
                .samples(vk::SampleCountFlags::TYPE_1)
                .usage(
                    vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::STORAGE
                        | vk::ImageUsageFlags::TRANSFER_DST,
                );
            let i = unsafe { g.dev().create_image(&ci, None) }.unwrap();
            let req = unsafe { g.dev().get_image_memory_requirements(i) };
            let a = g
                .alloc
                .lock()
                .unwrap()
                .allocate(&gpu_allocator::vulkan::AllocationCreateDesc {
                    name: "dummy",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: gpu_allocator::vulkan::AllocationScheme::GpuAllocatorManaged,
                })
                .unwrap();
            unsafe { g.dev().bind_image_memory(i, a.memory(), a.offset()) }.unwrap();
            std::mem::forget(a);
            i
        };
        let i2 = mk(vk::ImageType::TYPE_2D, 6, true);
        let i3 = mk(vk::ImageType::TYPE_3D, 1, false);
        for (im, layers) in [(i2, 6), (i3, 1)] {
            let range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: layers,
            };
            let b = [vk::ImageMemoryBarrier2::default()
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(im)
                .subresource_range(range)];
            unsafe {
                g.dev().cmd_pipeline_barrier2(
                    sub.pre,
                    &vk::DependencyInfo::default().image_memory_barriers(&b),
                );
                g.dev().cmd_clear_color_image(
                    sub.pre,
                    im,
                    vk::ImageLayout::GENERAL,
                    &vk::ClearColorValue { float32: [0.0; 4] },
                    &[range],
                );
            }
        }
        super::queue::full_barrier(g, sub.pre);
        *d = Some((i2, i3, Vec::new()));
    }
    dummy_view_of(g, d.as_mut().unwrap(), target, storage)
}

fn dummy_view_of(
    g: &Gpu,
    d: &mut (vk::Image, vk::Image, Vec<(i32, bool, vk::ImageView)>),
    target: i32,
    storage: bool,
) -> vk::ImageView {
    let (i2, i3, views) = d;
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
    let mut usage = vk::ImageViewUsageCreateInfo::default().usage(if storage {
        vk::ImageUsageFlags::STORAGE
    } else {
        vk::ImageUsageFlags::SAMPLED
    });
    let ci = vk::ImageViewCreateInfo::default()
        .image(img)
        .view_type(vt)
        .format(vk::Format::R8G8B8A8_UNORM)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: layers,
        })
        .push_next(&mut usage);
    let v = unsafe { g.dev().create_image_view(&ci, None) }.unwrap();
    views.push((target, storage, v));
    v
}

fn dummy_sampler(g: &Gpu) -> vk::Sampler {
    let p = samp_par {
        minf: 1,
        magf: 1,
        ws: 1,
        wt: 1,
        wr: 1,
        cmp_func: 2,
        lod_max: 1000.0,
        aniso: 1.0,
        ..Default::default()
    };
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

/// The support buffer with render scales: [0] the pass's (pixel shaders), then one per texture / image binding.
fn support_bytes_scaled(w: f32, h: f32, scales: &[f32]) -> Vec<u8> {
    let mut v = support_bytes(w, h);
    for (i, s) in scales.iter().enumerate().take(73) {
        v[192 + 16 * i..196 + 16 * i].copy_from_slice(&s.to_le_bytes());
    }
    v
}

/// The render scales a stage's shader reads (the translator's TexelFetchScale): index 0 the pass's, 1 + k the k-th
/// texture binding's image, then the images'. A pixel shader gets them negative (its fetches keep their sub-texel).
fn binding_scales(
    g: &Gpu,
    qs: &QState,
    st: usize,
    m: &Module,
    c0: &[u8],
    c2: &[u8],
    pass: f32,
) -> Vec<f32> {
    let (tb, _) = pool_bases(qs);
    let ntex = m
        .res
        .iter()
        .filter(|(b, r)| {
            matches!(r, Res::Tex { .. }) && (L::TEX_BASE..L::TEX_BASE + L::TEX_N).contains(b)
        })
        .count();
    let mut v = vec![1.0f32; 1 + ntex + L::IMG_N as usize];
    v[0] = pass;
    let imgs = g.images.lock().unwrap();
    for (b, r) in &m.res {
        let (handle, cbuf_slot, ty, idx) = match r {
            Res::Tex {
                handle,
                cbuf_slot,
                ty,
            } if (L::TEX_BASE..L::TEX_BASE + L::TEX_N).contains(b) => {
                (*handle, *cbuf_slot, *ty, (b - L::TEX_BASE) as usize)
            }
            Res::Img {
                handle,
                cbuf_slot,
                ty,
                ..
            } if (L::IMG_BASE..L::IMG_BASE + L::IMG_N).contains(b) => {
                (*handle, *cbuf_slot, *ty, ntex + (b - L::IMG_BASE) as usize)
            }
            _ => continue,
        };
        if ty == 2 || 1 + idx >= v.len() {
            continue;
        }
        let word = bank_word(qs, st, m, c0, c2, tex_slots(cbuf_slot).0, handle & 0x3fff);
        let tid = (word & 0xfffff) as i32 + tb;
        if let Some((serial, _)) = g.tables.tex_at(tid) {
            if let Some(im) = imgs.by_serial.get(&imgs.live(serial)) {
                if im.scale != 1.0 {
                    v[1 + idx] = if st == 1 { -im.scale } else { im.scale };
                }
            }
        }
    }
    v
}

/// The descriptor set of one stage of the draw or dispatch.
fn stage_set(
    g: &Gpu,
    sub: &mut Sub,
    qs: &QState,
    m: &Module,
    dispatch: [u32; 3],
    base_instance: u32,
) -> (SetRef, Option<Arc<super::lost::bindings::Stage>>, Option<super::lost::bindings::RawVersion>, u64) {
    // Covers ordinary UBO/SSBO backing even when no raw-table resource is present.
    // Resolution may itself create backing. If this epoch changes, publication
    // declines caching this encoding rather than assigning it the later epoch.
    let memory_generation = super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire);
    super::queue::wait_diag::phase("descriptors: banks");
    let st = m.stage as usize;

    let zero = zero_buffer(g, sub);
    let max_ubo = (g.d.props.limits.max_uniform_buffer_range as u64).min(65536);
    let (c0, c2, c0s, c2s) = super::perf::timed_fine(super::perf::C::BankNs, || {
        qs.banks(g, sub, st, dispatch, base_instance)
    });
    // the handle bank's identity (its bytes are kept while they stay the same)
    let c2id = qs.c2id.get();
    let (c0, c2) = (&c0[..], &c2[..]);
    let (tb, sb) = pool_bases(qs);
    let mut bufs: Vec<(u32, vk::DescriptorType, vk::DescriptorBufferInfo)> = Vec::new();
    let mut raw_bindings = Vec::new();
    let mut imgs: Vec<(u32, vk::DescriptorType, vk::DescriptorImageInfo)> = Vec::new();
    let mut tbufs: Vec<(u32, vk::DescriptorType, vk::BufferView)> = Vec::new();
    // descriptor buffers: buffer textures by address
    let mut taddrs: Vec<(u32, vk::DescriptorType, vk::DescriptorAddressInfoEXT)> = Vec::new();
    let dumping = passdump::active();
    let mut said: Vec<String> = Vec::new();
    // the texture part of the last set of this stage, when nothing it depends on changed
    let cacheable = !dumping
        && m.res.iter().all(|(_, r)| match r {
            Res::Tex { cbuf_slot, ty, .. } | Res::Img { cbuf_slot, ty, .. } => {
                let (t, s) = tex_slots(*cbuf_slot);
                (t == 2 || t == -1) && (s == 2 || s == -1) && *ty != 2
            }
            _ => true,
        });
    let mkey = (m as *const Module as usize, m.key);
    let image_gen = g.tables.generation();
    let cached = cacheable
        && qs.tpart.borrow()[st].as_ref().is_some_and(|t| {
            t.module == mkey
                && t.gen == c2id
                && t.pools == (tb, sb)
                && t.serial == sub.serial
                && t.image_gen == image_gen
        });
    if cached {
        imgs.extend_from_slice(&qs.tpart.borrow()[st].as_ref().unwrap().imgs);
    }
    for (b, r) in &m.res {
        let b = *b;
        if cached && matches!(r, Res::Tex { .. } | Res::Img { .. }) {
            continue;
        }
        match r {
            Res::Support => {
                let (wi, hi) = (qs.pass_extent.width.max(1), qs.pass_extent.height.max(1));
                let (buf, off) = if super::scale::active() {
                    // the render scale of the pass and of each texture / image binding (resolution scale)
                    let ps = if st == 1 { qs.pass_scale } else { 1.0 };
                    let (wi, hi) = (
                        ((wi as f32 / qs.pass_scale).round() as u32).max(1),
                        ((hi as f32 / qs.pass_scale).round() as u32).max(1),
                    );
                    let sc = binding_scales(g, qs, st, m, &c0, &c2, ps);
                    let mut key = Vec::with_capacity(8 + 4 * sc.len());
                    key.extend_from_slice(&wi.to_le_bytes());
                    key.extend_from_slice(&hi.to_le_bytes());
                    for s in &sc {
                        key.extend_from_slice(&s.to_bits().to_le_bytes());
                    }
                    let k = super::queue::memo_hash(&key) as u64;
                    qs.staged(g, sub, 31, k, || {
                        support_bytes_scaled(wi as f32, hi as f32, &sc)
                    })
                } else {
                    qs.staged(g, sub, 0, (wi as u64) << 32 | hi as u64, || {
                        support_bytes(wi as f32, hi as f32).to_vec()
                    })
                };
                bufs.push((
                    b,
                    vk::DescriptorType::UNIFORM_BUFFER,
                    vk::DescriptorBufferInfo {
                        buffer: buf,
                        offset: off,
                        range: 1392,
                    },
                ));
            }
            Res::Cbuf { bank } => {
                let info = match *bank {
                    0 => vk::DescriptorBufferInfo {
                        buffer: c0s.0,
                        offset: c0s.1,
                        range: DRV_SIZE as u64,
                    },
                    1 => match &m.c1 {
                        Some(c1) if !c1.is_empty() => {
                            let (buf, off) = qs.staged(
                                g,
                                sub,
                                1,
                                c1.as_ptr() as u64 ^ (c1.len() as u64) << 48,
                                || c1.to_vec(),
                            );
                            vk::DescriptorBufferInfo {
                                buffer: buf,
                                offset: off,
                                range: (c1.len() as u64).min(max_ubo),
                            }
                        }
                        _ => vk::DescriptorBufferInfo {
                            buffer: zero,
                            offset: 0,
                            range: max_ubo,
                        },
                    },
                    2 => vk::DescriptorBufferInfo {
                        buffer: c2s.0,
                        offset: c2s.1,
                        range: DRV_SIZE as u64,
                    },
                    24 => {
                        let grp = (st == 5) as usize;
                        let (buf, off) =
                            qs.staged(g, sub, 2 + grp as u8, qs.pgen, || qs.push[grp][..].to_vec());
                        vk::DescriptorBufferInfo {
                            buffer: buf,
                            offset: off,
                            range: 0x800,
                        }
                    }
                    k @ 25..=29 => {
                        // graphics stage i reads its push block 1 + i through its own bank (NVN2)
                        let b = (k - 24) as usize * 0x100;
                        let (buf, off) = qs.staged(g, sub, k as u8, qs.pgen, || {
                            qs.push[0][b..b + 0x100].to_vec()
                        });
                        vk::DescriptorBufferInfo {
                            buffer: buf,
                            offset: off,
                            range: 0x100,
                        }
                    }
                    k @ 3..=18 => {
                        let (a, n) = qs.ubo[st][(k - 3) as usize];
                        let nf = (n.min(64) / 4) as usize;
                        if dumping && a != 0 && nf > 0 && super::mem::room(a) >= 4 * nf as u64 {
                            // SAFETY: guest memory inside a registered pool (identity mapped)
                            let f = unsafe { core::slice::from_raw_parts(a as *const f32, nf) };
                            said.push(format!(
                                "c{k}@{a:#x}+{n:#x}={:?}",
                                f.iter()
                                    .map(|v| (v * 1000.0).round() / 1000.0)
                                    .collect::<Vec<_>>()
                            ));
                        } else if dumping {
                            said.push(format!("c{k}@{a:#x}+{n:#x}"));
                        }
                        match if a != 0 && n != 0 {
                            super::mem::resolve(g, sub, a, n.min(max_ubo))
                        } else {
                            None
                        } {
                            Some((buf, off, _)) => {
                                if off % g.d.caps.uniform_align != 0 {
                                    crate::log::red_once(&format!(
                                        "uniform buffer at {a:#x}: offset not aligned to {}",
                                        g.d.caps.uniform_align
                                    ));
                                }
                                vk::DescriptorBufferInfo {
                                    buffer: buf,
                                    offset: off,
                                    range: n.min(max_ubo),
                                }
                            }
                            None => {
                                if a != 0 {
                                    let k: String =
                                        m.key.iter().map(|b| format!("{b:02x}")).collect();
                                    crate::log::red_once(&format!("shader {k} (stage {st}): uniform bank c{k2} at {a:#x} (+{n:#x}) is in no memory pool: zeros bound", k2 = *bank));
                                }
                                vk::DescriptorBufferInfo {
                                    buffer: zero,
                                    offset: 0,
                                    range: max_ubo,
                                }
                            }
                        }
                    }
                    _ => vk::DescriptorBufferInfo {
                        buffer: zero,
                        offset: 0,
                        range: max_ubo,
                    },
                };
                bufs.push((b, vk::DescriptorType::UNIFORM_BUFFER, info));
            }
            Res::Sbuf {
                cb_slot: super::gmem::CB_SLOT,
                cb_off,
                written,
            } => {
                // Delay the paired snapshot until all ordinary resources have backing.
                raw_bindings.push((b, *cb_off / 4, *written));
            }
            Res::Sbuf {
                cb_slot,
                cb_off,
                written,
            } => {
                let (a, mut n) = bank_u64(qs, st, m, &c0, &c2, *cb_slot, *cb_off);
                let bound_n = n;
                // A buffer bound with size 0 is still the memory at its address: an NVN shader holds the address and
                // reads and writes through it, the size is only what the driver tells it. Donkey Kong Bananza binds the
                // list its depth-sampling dispatch appends to this way. It runs to the end of its pool, at most 1 MB
                // (NXGFX_SBUF0_MB=<n> another cap; with robustBufferAccess2 an access past it is dropped, lane K3).
                if a != 0 && n == 0 {
                    n = super::mem::room(a).min(sbuf0_cap());
                    super::perf::inc(super::perf::C::SbufSize0);
                    static SAID: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if n > 0 && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        crate::vklog!("a storage buffer bound with size 0 ({a:#x}): bound to the end of its memory pool, {n} bytes (later ones are not logged)");
                    }
                }
                // A buffer the shader only reads runs to the end of its pool: an NVN shader's global loads are not held
                // to the size in c0 (MKW's text vertex shader reads one 64-byte glyph record per instance from a binding
                // of 0x100 bytes: past the fourth glyph it read zeros and the letters were dropped; mkw-recomp
                // vk_exec_bind.c binds read-only buffers to the end of their backing). NXGFX_SBUF_TO_END=0 off.
                if !*written
                    && a != 0
                    && n != 0
                    && sbuf_to_end()
                    && !crate::nvn::vpool::is_sparse(a)
                {
                    let r =
                        super::mem::room(a).min(g.d.props.limits.max_storage_buffer_range as u64);
                    if r > n {
                        n = r;
                    }
                }
                if *written && st < 5 && a != 0 {
                    append::bound(a);
                }
                let info = match if a != 0 && n != 0 {
                    super::mem::resolve(g, sub, a, n)
                } else {
                    None
                } {
                    Some((buf, off, _)) => {
                        if *written {
                            super::mem::note_gpu_write(g, sub, a, n);
                        }
                        if off % g.d.caps.storage_align != 0 {
                            crate::log::red_once(&format!(
                                "storage buffer at {a:#x}: offset not aligned to {}",
                                g.d.caps.storage_align
                            ));
                        }
                        vk::DescriptorBufferInfo {
                            buffer: buf,
                            offset: off,
                            range: n,
                        }
                    }
                    None => {
                        if a != 0 {
                            let k: String = m.key.iter().map(|b| format!("{b:02x}")).collect();
                            let fallback = if *written {
                                "private zero-initialized scratch"
                            } else {
                                "read-only zero buffer"
                            };
                            crate::log::red_once(&format!("shader {k} (stage {st}): storage buffer {cb_slot}:{cb_off:#x} at {a:#x} (+{n:#x}) is in no memory pool: 64 KiB {fallback} bound"));
                        }
                        missing_storage(g, sub, zero, *written)
                    }
                };
                if st == 3 && a != 0 {
                    // (diagnosis, lane K3: MKW's task shaders index a ring of 48-byte records in s0 with the game's
                    // counts) each task shader's storage buffers as finally bound, once per shader and slot
                    static SEEN: std::sync::Mutex<Vec<([u8; 8], u32)>> =
                        std::sync::Mutex::new(Vec::new());
                    let mut s = SEEN.lock().unwrap();
                    let id = (m.key, *cb_off);
                    if s.len() < 64 && !s.contains(&id) {
                        s.push(id);
                        let k: String = m.key.iter().map(|b| format!("{b:02x}")).collect();
                        crate::vklog!("task shader {k}: storage buffer c0+{cb_off:#x} at {a:#x}, size {bound_n:#x} as bound, final descriptor range {:#x}, {:#x} to its pool's end, written {written}", info.range, super::mem::room(a));
                    }
                }
                if st == 0
                    && *cb_slot == 0
                    && *cb_off == 0x110
                    && super::gmem::probe::matches(&m.key)
                {
                    qs.raw_probe_source
                        .set(Some((sub.serial, info, info.buffer == zero)));
                }
                bufs.push((b, vk::DescriptorType::STORAGE_BUFFER, info));
            }
            Res::Tex {
                handle,
                cbuf_slot,
                ty,
            }
            | Res::Img {
                handle,
                cbuf_slot,
                ty,
                ..
            } => {
                let is_img = matches!(r, Res::Img { .. });
                let ifmt = if let Res::Img { fmt, .. } = r {
                    *fmt
                } else {
                    0
                };
                let word = bank_word(
                    qs,
                    st,
                    m,
                    &c0,
                    &c2,
                    tex_slots(*cbuf_slot).0,
                    handle & 0x3fff,
                );
                let kind = handle >> 28;
                let tid = (word & 0xfffff) as i32 + tb;
                if *ty == 2 {
                    // a buffer texture / buffer image: a texel buffer view over its memory
                    let dt = if is_img {
                        vk::DescriptorType::STORAGE_TEXEL_BUFFER
                    } else {
                        vk::DescriptorType::UNIFORM_TEXEL_BUFFER
                    };
                    if let Some(t) = g.tables.buf_tex_at(tid) {
                        if g.d.caps.descbuf {
                            if let Some(ai) = g.texel_addr(sub, &t, is_img) {
                                if is_img {
                                    super::mem::note_gpu_write(
                                        g,
                                        sub,
                                        t.mem as u64,
                                        t.d.width as u64
                                            * crate::tex::layout::fmt_bpb(t.d.format).0 as u64,
                                    );
                                }
                                taddrs.push((b, dt, ai));
                            }
                            continue;
                        }
                        if let Some(v) = g.texel_view(sub, &t, is_img) {
                            if is_img {
                                super::mem::note_gpu_write(
                                    g,
                                    sub,
                                    t.mem as u64,
                                    t.d.width as u64
                                        * crate::tex::layout::fmt_bpb(t.d.format).0 as u64,
                                );
                            }
                            tbufs.push((b, dt, v));
                        }
                    }
                    continue;
                }
                let sid = match kind {
                    0 => (word >> 20) as i32,
                    1 => {
                        (bank_word(
                            qs,
                            st,
                            m,
                            &c0,
                            &c2,
                            tex_slots(*cbuf_slot).1,
                            handle >> 14 & 0x3fff,
                        ) >> 20) as i32
                    }
                    2 => (handle >> 14 & 0x3fff) as i32,
                    _ => {
                        crate::log::red_once("constant sampler handles are not bound yet");
                        0
                    }
                } + sb;
                let view = super::perf::timed_fine(super::perf::C::TexViewNs, || {
                    tex_view(g, sub, tid, *ty | (ifmt as u32) << 16, is_img)
                });
                if dumping && render_chain::enabled() {
                    if let Some((serial, _)) = g.tables.tex_at(tid) {
                        if let Some(resource) = chain_resource(g, serial, view) {
                            render_chain::consumer(
                                render_chain::Point {
                                    frame: super::present::frames() + 1,
                                    submission: sub.serial,
                                    pass: passdump::pass_no(),
                                },
                                m.stage,
                                m.key,
                                b,
                                is_img,
                                resource,
                            );
                        }
                    }
                }
                // NXGFX_LOG_SHADER=<key> (diagnosis, lane K): every texture / image this shader is bound to, with its image
                static LOGK: std::sync::OnceLock<String> = std::sync::OnceLock::new();
                static LOGN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let logk =
                    LOGK.get_or_init(|| std::env::var("NXGFX_LOG_SHADER").unwrap_or_default());
                if !logk.is_empty()
                    && m.key.iter().map(|b| format!("{b:02x}")).collect::<String>() == *logk
                    && LOGN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 60
                {
                    let what = g
                        .tables
                        .tex_at(tid)
                        .and_then(|(serial, v)| {
                            let imgs = g.images.lock().unwrap();
                            let serial = imgs.live(serial);
                            imgs.by_serial.get(&serial).map(|im| {
                                let d = &im.tex.d;
                                format!(
                                    "ser{serial} fmt{:#x} {}x{}x{} levels {} t{} at {:#x} v{:?}",
                                    d.format,
                                    d.width,
                                    d.height,
                                    d.depth,
                                    d.levels,
                                    d.target,
                                    im.tex.mem as u64,
                                    v.map(|v| (v.base_level, v.min_layer, v.format))
                                )
                            })
                        })
                        .unwrap_or_else(|| "no registered image".into());
                    crate::log::red(&format!(
                        "shader {logk}: {}{b} = handle word {word:#x} id {tid}: {what}; view {}",
                        if is_img { "img" } else { "tex" },
                        if view == vk::ImageView::null() {
                            "null"
                        } else {
                            "made"
                        }
                    ));
                }
                if dumping {
                    let what = g
                        .tables
                        .tex_at(tid)
                        .and_then(|(serial, v)| {
                            let imgs = g.images.lock().unwrap();
                            let serial = imgs.live(serial);
                            imgs.by_serial.get(&serial).map(|im| {
                                let d = &im.tex.d;
                                passdump::want_tex(serial, d.width, d.height);
                                format!(
                                    "ser{serial} fmt{:#x} {}x{}x{} t{} v{:?}",
                                    d.format,
                                    d.width,
                                    d.height,
                                    d.depth,
                                    d.target,
                                    v.map(|v| (
                                        v.bits,
                                        v.target,
                                        v.base_level,
                                        v.min_layer,
                                        v.format,
                                        v.swz,
                                        im.tex.swizzle(),
                                        im.format
                                    ))
                                )
                            })
                        })
                        .unwrap_or_else(|| "none".into());
                    let filt = g
                        .tables
                        .samp_filters(sid)
                        .map(|(mi, ma)| format!("(min {mi} mag {ma})"))
                        .unwrap_or_default();
                    said.push(format!(
                        "{}{b}=id{tid}/s{sid}{filt} ty{ty}: {what}",
                        if is_img { "img" } else { "tex" }
                    ));
                }
                if is_img {
                    imgs.push((
                        b,
                        vk::DescriptorType::STORAGE_IMAGE,
                        vk::DescriptorImageInfo {
                            sampler: vk::Sampler::null(),
                            image_view: view,
                            image_layout: vk::ImageLayout::GENERAL,
                        },
                    ));
                } else {
                    let s = g.tables.samp_at(sid).unwrap_or_else(|| dummy_sampler(g));
                    imgs.push((
                        b,
                        vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        vk::DescriptorImageInfo {
                            sampler: s,
                            image_view: view,
                            image_layout: vk::ImageLayout::GENERAL,
                        },
                    ));
                }
            }
            Res::Pool => {}
            Res::MeshDraw => {
                let info = match qs.mesh_rec {
                    Some((buf, off, range)) => vk::DescriptorBufferInfo {
                        buffer: buf,
                        offset: off,
                        range,
                    },
                    None => {
                        crate::log::red_once("a task / mesh shader's draw records are not bound (a draw that is not a mesh draw)");
                        vk::DescriptorBufferInfo {
                            buffer: zero,
                            offset: 0,
                            range: 65536,
                        }
                    }
                };
                bufs.push((b, vk::DescriptorType::STORAGE_BUFFER, info));
            }
        }
    }
    let mut raw_version = None;
    if !raw_bindings.is_empty() {
        let pair = super::gmem::bindings(g, sub);
        raw_version = pair.map(|p| p.token);
        for (binding, word, written) in raw_bindings {
            let info = pair.map(|p| p.descriptors[usize::from(word != 0)])
                .unwrap_or_else(|| missing_storage(g, sub, zero, written));
            bufs.push((binding, vk::DescriptorType::STORAGE_BUFFER, info));
        }
    }
    if cacheable && !cached {
        qs.tpart.borrow_mut()[st] = Some(TexPart {
            module: mkey,
            gen: c2id,
            pools: (tb, sb),
            serial: sub.serial,
            image_gen,
            imgs: imgs.clone(),
        });
    }
    if dumping {
        let k: String = m.key.iter().map(|b| format!("{b:02x}")).collect();
        crate::vklog!(
            "pass dump {}: stage {st} {k}: {}",
            passdump::pass_no(),
            said.join("; ")
        );
    }
    if g.d.caps.descbuf {
        let (off, evidence) = write_db_set(g, sub, m, &bufs, &imgs, &taddrs);
        return (SetRef::Buf(off), evidence, raw_version, memory_generation);
    }
    // the same writes for the same stage layout earlier in this submission: that set is bound again
    let tk = super::perf::fine().then(std::time::Instant::now);
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
    let tw = super::perf::fine().then(std::time::Instant::now);
    if let (Some(tk), Some(tw)) = (tk, tw) {
        super::perf::add(super::perf::C::SetKeyNs, (tw - tk).as_nanos() as u64);
    }
    if let Some(&set) = sub.memo_sets.get(&kh) {
        return (SetRef::Set(set), None, raw_version, memory_generation);
    }
    let set = sub.alloc_set(g, g.layout.stage_sets[st]);
    super::perf::inc(super::perf::C::DescSets);
    sub.memo_sets.insert(kh, set);
    let mut writes: Vec<vk::WriteDescriptorSet> = bufs
        .iter()
        .map(|(b, ty, i)| {
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(*b)
                .descriptor_type(*ty)
                .buffer_info(std::slice::from_ref(i))
        })
        .collect();
    writes.extend(imgs.iter().map(|(b, ty, i)| {
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(*b)
            .descriptor_type(*ty)
            .image_info(std::slice::from_ref(i))
    }));
    writes.extend(tbufs.iter().map(|(b, ty, v)| {
        vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(*b)
            .descriptor_type(*ty)
            .texel_buffer_view(std::slice::from_ref(v))
    }));
    unsafe { g.dev().update_descriptor_sets(&writes, &[]) };
    if let Some(tw) = tw {
        super::perf::add(super::perf::C::SetWriteNs, tw.elapsed().as_nanos() as u64);
    }
    (SetRef::Set(set), None, raw_version, memory_generation)
}

/// A stage's descriptors: a set (classic) or its offset in the submission's descriptor buffer.
#[derive(Clone, Copy)]
enum SetRef {
    Set(vk::DescriptorSet),
    Buf(u64),
}

/// Descriptor buffers: a stage set written into the submission's current chunk (vkGetDescriptorEXT: a few
/// nanoseconds a descriptor, against ~40 ns for a vkUpdateDescriptorSets write and a set allocation; 2,170 sets of
/// 15 uniform buffers took 0.12 ms against 1.8 ms on this machine's RTX 5090, tests/desc_bench.rs). Every set is
/// written: no key, no hash.
fn write_db_set(
    g: &Gpu,
    sub: &mut Sub,
    m: &Module,
    bufs: &[(u32, vk::DescriptorType, vk::DescriptorBufferInfo)],
    imgs: &[(u32, vk::DescriptorType, vk::DescriptorImageInfo)],
    taddrs: &[(u32, vk::DescriptorType, vk::DescriptorAddressInfoEXT)],
) -> (u64, Option<Arc<super::lost::bindings::Stage>>) {
    let tw = super::perf::fine().then(std::time::Instant::now);
    let l = &g.layout;
    let p = &g.d.db_props;
    let off = sub.db_alloc(l.db_stage_size);
    let ptr = sub.db[sub.db_cur].ptr;
    let mut evidence = super::lost::bindings::enabled().then(|| super::lost::bindings::Stage {
        serial: sub.serial,
        chunk: sub.db_cur,
        offset: off,
        stage: m.stage,
        shader: m.key,
        pool_epoch: super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire),
        ranges: Vec::new(),
        omitted: 0,
    });
    let zero = ZERO.get().map(|b| b.addr).unwrap_or(0);
    // the distinct buffers' addresses (a set names a few: a staging chunk, the zero buffer, some pools)
    let mut seen: [(vk::Buffer, u64); 8] = [(vk::Buffer::null(), 0); 8];
    let mut ns = 0;
    for (b, ty, info) in bufs {
        stale_check(sub, info.buffer, "a descriptor");
        let base = match seen[..ns].iter().find(|x| x.0 == info.buffer) {
            Some(x) => x.1,
            None => {
                let a = sub
                    .addr_of(info.buffer)
                    .or_else(|| (ZERO.get().map(|z| z.buf) == Some(info.buffer)).then_some(zero))
                    .or_else(|| super::mem::buffer_address(info.buffer))
                    .unwrap_or(0);
                if ns < seen.len() {
                    seen[ns] = (info.buffer, a);
                    ns += 1;
                }
                a
            }
        };
        if base == 0 {
            crate::log::red_once(
                "descriptor buffers: a buffer bound to a shader has no device address: zeros bound",
            );
        }
        let (a, range) = if base != 0 {
            (base + info.offset, info.range)
        } else {
            (zero, 65536)
        };
        if let Some(e) = evidence.as_mut() {
            let buffer = if base != 0 {
                info.buffer
            } else {
                ZERO.get().unwrap().buf
            };
            let handle = vk::Handle::as_raw(buffer);
            e.push(super::lost::bindings::Range {
                binding: *b,
                kind: ty.as_raw(),
                buffer: handle,
                offset: if base != 0 { info.offset } else { 0 },
                address: a,
                size: range,
                binding_id: super::lost::binding_id(handle, a),
                owned: sub.owns(buffer),
            });
        }
        super::lost::note_use(
            a,
            range,
            if *ty == vk::DescriptorType::UNIFORM_BUFFER {
                "a uniform buffer descriptor"
            } else {
                "a storage buffer descriptor"
            },
        );
        let ai = vk::DescriptorAddressInfoEXT::default()
            .address(a)
            .range(range);
        let (gi, size) = if *ty == vk::DescriptorType::UNIFORM_BUFFER {
            (
                vk::DescriptorGetInfoEXT::default()
                    .ty(*ty)
                    .data(vk::DescriptorDataEXT {
                        p_uniform_buffer: &ai,
                    }),
                super::device::desc_size(p, vk::DescriptorType::UNIFORM_BUFFER),
            )
        } else {
            (
                vk::DescriptorGetInfoEXT::default()
                    .ty(*ty)
                    .data(vk::DescriptorDataEXT {
                        p_storage_buffer: &ai,
                    }),
                super::device::desc_size(p, vk::DescriptorType::STORAGE_BUFFER),
            )
        };
        unsafe {
            super::tables::put_descriptor(&g.d, ptr, off + l.db_stage_off[*b as usize], &gi, size)
        };
    }
    for (b, ty, info) in imgs {
        let (gi, size) = if *ty == vk::DescriptorType::COMBINED_IMAGE_SAMPLER {
            (
                vk::DescriptorGetInfoEXT::default()
                    .ty(*ty)
                    .data(vk::DescriptorDataEXT {
                        p_combined_image_sampler: info,
                    }),
                p.combined_image_sampler_descriptor_size,
            )
        } else {
            (
                vk::DescriptorGetInfoEXT::default()
                    .ty(*ty)
                    .data(vk::DescriptorDataEXT {
                        p_storage_image: info,
                    }),
                p.storage_image_descriptor_size,
            )
        };
        unsafe {
            super::tables::put_descriptor(&g.d, ptr, off + l.db_stage_off[*b as usize], &gi, size)
        };
    }
    for (b, ty, ai) in taddrs {
        if let Some(e) = evidence.as_mut() {
            e.push(super::lost::bindings::Range {
                binding: *b,
                kind: ty.as_raw(),
                buffer: 0,
                offset: 0,
                address: ai.address,
                size: ai.range,
                binding_id: None,
                owned: false,
            });
        }
        super::lost::note_use(ai.address, ai.range, "a texel buffer descriptor");
        let (gi, size) = if *ty == vk::DescriptorType::UNIFORM_TEXEL_BUFFER {
            (
                vk::DescriptorGetInfoEXT::default()
                    .ty(*ty)
                    .data(vk::DescriptorDataEXT {
                        p_uniform_texel_buffer: ai,
                    }),
                super::device::desc_size(p, vk::DescriptorType::UNIFORM_TEXEL_BUFFER),
            )
        } else {
            (
                vk::DescriptorGetInfoEXT::default()
                    .ty(*ty)
                    .data(vk::DescriptorDataEXT {
                        p_storage_texel_buffer: ai,
                    }),
                super::device::desc_size(p, vk::DescriptorType::STORAGE_TEXEL_BUFFER),
            )
        };
        unsafe {
            super::tables::put_descriptor(&g.d, ptr, off + l.db_stage_off[*b as usize], &gi, size)
        };
    }
    super::perf::inc(super::perf::C::DescSets);
    if let Some(tw) = tw {
        super::perf::add(super::perf::C::SetWriteNs, tw.elapsed().as_nanos() as u64);
    }
    (off, evidence.map(Arc::new))
}

fn pool_bases(qs: &QState) -> (i32, i32) {
    let tb = if qs.tex_pool != 0 {
        crate::nvn::pool::dpool_base_of(qs.tex_pool as *const dpool_t, true)
    } else {
        0
    };
    let sb = if qs.samp_pool != 0 {
        crate::nvn::pool::dpool_base_of(qs.samp_pool as *const dpool_t, false)
    } else {
        0
    };
    (tb, sb)
}

fn push_constants(g: &Gpu, sub: &Sub, qs: &QState, flipped: bool) {
    let (tb, sb) = pool_bases(qs);
    // 16: the alpha test's function and reference (shaders::link::ALPHA_PUSH_OFFSET)
    let pc: [u32; 6] = [
        tb as u32,
        sb as u32,
        flipped as u32,
        qs.mesh_stride,
        qs.color.alpha_func(),
        qs.alpha_ref.to_bits(),
    ];
    let bytes = unsafe { core::slice::from_raw_parts(pc.as_ptr() as *const u8, 24) };
    unsafe {
        g.dev()
            .cmd_push_constants(sub.main, g.layout.pipeline, g.layout.push_stages, 0, bytes)
    };
}

fn bind_sets(
    g: &Gpu,
    sub: &mut Sub,
    qs: &QState,
    mods: &[Arc<Module>],
    bp: vk::PipelineBindPoint,
    dispatch: [u32; 3],
    base_instance: u32,
) {
    if super::lost::bindings::enabled() {
        qs.sets
            .borrow_mut()
            .begin(mods.iter().map(|m| m.stage as usize));
    }
    let pool = mods.iter().any(|m| m.uses_pool);
    if pool {
        let gen = g.tables.pool_generation();
        if sub.pool_snapshot.is_none_or(|(old, _, _)| old != gen)
            || super::image::lazy_waiting() != 0
            || g.tables.has_pending_images()
        {
            g.prepare_pool_images(sub);
            sub.pool_snapshot = Some(g.tables.snapshot(g));
            sub.db_bound = None;
        }
        if mods.iter().any(|m| uses_storage_pool(m)) {
            let views = g.tables.storage_views(pool_bases(qs).0);
            let mut imgs = g.images.lock().unwrap();
            for (serial, view) in views {
                note_storage_write(g, sub, &mut imgs, serial, view);
            }
        }
    }
    if g.d.caps.descbuf {
        // descriptor buffers: the draw's sets in one chunk, the pool set's buffer and that chunk bound, then offsets
        sub.db_reserve(g, g.layout.db_stage_size * mods.len().max(1) as u64);
        if sub.db_bound != Some(sub.db_cur) {
            let usage = vk::BufferUsageFlags::RESOURCE_DESCRIPTOR_BUFFER_EXT
                | vk::BufferUsageFlags::SAMPLER_DESCRIPTOR_BUFFER_EXT;
            let pool_addr = sub
                .pool_snapshot
                .map(|(_, _, addr)| addr)
                .unwrap_or_else(|| g.tables.db.as_ref().unwrap().addr);
            let infos = [
                vk::DescriptorBufferBindingInfoEXT::default()
                    .address(pool_addr)
                    .usage(usage),
                vk::DescriptorBufferBindingInfoEXT::default()
                    .address(sub.db[sub.db_cur].addr)
                    .usage(usage),
            ];
            unsafe { g.d.db.cmd_bind_descriptor_buffers(sub.main, &infos) };
            sub.db_bound = Some(sub.db_cur);
            super::lost::note_set(b'B', 0, sub.serial, sub.db_cur, 0);
        }
        for m in mods {
            let st = m.stage as usize;
            let uses_raw = m.res.iter().any(|(_, r)| matches!(r,
                Res::Sbuf { cb_slot: super::gmem::CB_SLOT, .. }));
            let raw = if uses_raw { sub.gmem.as_ref().map(|v| v.token()) } else { None };
            let key = set_key(g, sub, qs, m, dispatch, base_instance);
            let hit = key.filter(|_| !uses_raw || raw.is_some()).and_then(|k| {
                qs.sets.borrow().hit_version(st, k, raw,
                    super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire))
            });
            let reused = hit.is_some();
            let (set, evidence) = match hit {
                Some((off, evidence)) => {
                    super::perf::inc(super::perf::C::SetsReused);
                    (SetRef::Buf(off), evidence)
                }
                None => {
                    let (set, evidence, captured_raw, captured_memory) = super::perf::timed(super::perf::C::SetNs, || {
                        stage_set(g, sub, qs, m, dispatch, base_instance)
                    });
                    // the key again: building the set may have moved to a new descriptor-buffer chunk
                    if let (SetRef::Buf(off), Some(old)) = (set, key) {
                        let key = set_key(g, sub, qs, m, dispatch, base_instance)
                            .filter(|k| k.image_gen == old.image_gen && (!uses_raw || captured_raw.is_some()));
                        qs.sets.borrow_mut().store_version(st, key, off, evidence.clone(), captured_raw,
                            captured_memory,
                            super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire));
                    }
                    (set, evidence)
                }
            };
            if let SetRef::Buf(off) = set {
                if super::lost::bindings::enabled() {
                    qs.sets
                        .borrow_mut()
                        .select(st, sub.serial, sub.db_cur, off, m.key, evidence);
                }
                super::lost::note_set(
                    if reused { b'H' } else { b'W' },
                    st as u8,
                    sub.serial,
                    sub.db_cur,
                    off,
                );
                unsafe {
                    g.d.db.cmd_set_descriptor_buffer_offsets(
                        sub.main,
                        bp,
                        g.layout.pipeline,
                        m.stage,
                        &[1],
                        &[off],
                    )
                };
            }
        }
        if pool {
            unsafe {
                g.d.db.cmd_set_descriptor_buffer_offsets(
                    sub.main,
                    bp,
                    g.layout.pipeline,
                    L::POOL_SET,
                    &[0],
                    &[0],
                )
            };
        }
        return;
    }
    for m in mods {
        let (set, _, _, _) = super::perf::timed(super::perf::C::SetNs, || {
            stage_set(g, sub, qs, m, dispatch, base_instance)
        });
        if let SetRef::Set(set) = set {
            unsafe {
                g.dev().cmd_bind_descriptor_sets(
                    sub.main,
                    bp,
                    g.layout.pipeline,
                    m.stage,
                    &[set],
                    &[],
                )
            };
        }
    }
    if pool {
        unsafe {
            g.dev().cmd_bind_descriptor_sets(
                sub.main,
                bp,
                g.layout.pipeline,
                L::POOL_SET,
                &[sub.pool_snapshot.unwrap().1],
                &[],
            )
        };
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
        let n = nxconfig::frame_skip();
        if V.swap(n, Ordering::Relaxed) != n || g == 1 {
            crate::vklog!(
                "fps_target: frame skip {n} (every {} frame drawn, the game keeps its own rate)",
                n + 1
            );
        }
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

fn scissor_clamp() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_SCISSOR_CLAMP").as_deref() != Ok("0"))
}

/// `r` intersected with [0, e) (an empty rectangle when they do not meet).
fn clamp_rect(r: vk::Rect2D, e: vk::Extent2D) -> vk::Rect2D {
    let x0 = (r.offset.x.max(0) as u32).min(e.width);
    let y0 = (r.offset.y.max(0) as u32).min(e.height);
    let x1 = ((r.offset.x.max(0) as u64 + r.extent.width as u64).min(e.width as u64)) as u32;
    let y1 = ((r.offset.y.max(0) as u64 + r.extent.height as u64).min(e.height as u64)) as u32;
    let changed = x0 as i32 != r.offset.x
        || y0 as i32 != r.offset.y
        || x1 - x0.min(x1) != r.extent.width
        || y1 - y0.min(y1) != r.extent.height;
    if changed {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| crate::vklog!("a scissor past its pass's targets ({:?} in {}x{}): held to them (later ones are not logged)", r, e.width, e.height));
    }
    vk::Rect2D {
        offset: vk::Offset2D {
            x: x0 as i32,
            y: y0 as i32,
        },
        extent: vk::Extent2D {
            width: x1.saturating_sub(x0),
            height: y1.saturating_sub(y0),
        },
    }
}

/// Everything a draw needs bound; false when it cannot be drawn (it is skipped).
fn draw_setup(g: &Gpu, sub: &mut Sub, qs: &mut QState, prim_ty: i32, base_instance: u32) -> bool {
    draw_setup_ex(g, sub, qs, prim_ty, base_instance, false)
}

/// As `draw_setup`; `mesh`: a task / mesh draw (NVN2), its program's task and mesh shaders in places 3 and 4.
/// `NXGFX_SKIP_PS=<key>[,<key>...]` (diagnosis, with a replay): draws with one of these pixel shaders are skipped, to
/// find which draw puts something into a target.
fn skip_ps(qs: &QState) -> bool {
    static KEYS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let keys = KEYS.get_or_init(|| {
        std::env::var("NXGFX_SKIP_PS")
            .map(|v| {
                v.split(',')
                    .map(|x| x.trim().to_lowercase())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    });
    if keys.is_empty() {
        return false;
    }
    let Some(m) = qs.prog[1].as_ref() else {
        return false;
    };
    let k: String = m.key.iter().map(|b| format!("{b:02x}")).collect();
    keys.iter().any(|x| *x == k)
}

fn draw_setup_ex(
    g: &Gpu,
    sub: &mut Sub,
    qs: &mut QState,
    prim_ty: i32,
    base_instance: u32,
    mesh: bool,
) -> bool {
    if skip_ps(qs) {
        return false;
    }
    if !qs.missing.is_empty() {
        // shaders the build did not translate: wait for the live translator (once per set), then bind what came
        let keys: Vec<_> = qs.missing.iter().map(|m| m.0).collect();
        let dev = g.dev();
        // a shader that came but cannot be built (refused) ends the wait as well
        super::live::wait_for(&keys, || {
            keys.iter()
                .all(|k| g.shaders.get_ex(dev, k, true).is_some() || g.shaders.refused(k))
        });
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
    let first = if mesh {
        qs.prog[4].clone().filter(|m| m.mesh == 2)
    } else {
        qs.prog[0].clone()
    };
    let Some(mut vs) = first else {
        crate::log::red_once(if mesh {
            "a mesh draw with no mesh shader bound"
        } else {
            "draw with no vertex shader bound"
        });
        super::perf::inc(super::perf::C::DrawsSkipped);
        super::perf::inc(super::perf::C::SkipNoShader);
        return false;
    };
    if qs.prog[..5].iter().flatten().any(|m| {
        m.res
            .iter()
            .any(|(_, r)| matches!(r, Res::Img { ty, .. } if *ty != 2))
    }) {
        let image_mods: Vec<_> = qs.prog[..5]
            .iter()
            .flatten()
            .filter(|m| (m.mesh != 0) == mesh || m.stage == 1)
            .cloned()
            .collect();
        prepare_storage_bindings(g, sub, qs, &image_mods, [0; 3], base_instance);
    }
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
    if !shadows_on()
        && qs.pass_formats.0[..qs.pass_formats.1]
            .iter()
            .all(|&f| f == 0)
        && qs.pass_formats.2 != 0
        && qs.pass_extent != qs.last_color
    {
        super::perf::inc(super::perf::C::DrawsSkipped);
        return false;
    }
    if qs.vis_on && qs.occl_q.is_none() && sub.occl_n < 256 {
        let q = sub.occl_n;
        sub.occl_n += 1;
        let flags = if g.d.caps.occlusion_precise {
            vk::QueryControlFlags::PRECISE
        } else {
            vk::QueryControlFlags::empty()
        };
        unsafe { g.dev().cmd_begin_query(sub.main, sub.occl, q, flags) };
        qs.occl_q = Some(q);
    }
    // a mesh draw: the task (if any) and mesh shaders and the pixel shader; any other draw leaves task and mesh shaders out
    let mods: Vec<Arc<Module>> = if mesh {
        let mut v: Vec<Arc<Module>> = [3usize, 4, 1]
            .iter()
            .filter_map(|&s| qs.prog[s].clone())
            .filter(|m| m.stage == 1 || m.mesh != 0)
            .collect();
        // the pixel shader reads the mesh shader's per-primitive outputs per primitive
        let locs = v
            .iter()
            .find(|m| m.mesh == 2)
            .map(|m| m.per_primitive.clone())
            .unwrap_or_default();
        if !locs.is_empty() {
            if let Some(i) = v.iter().position(|m| m.stage == 1) {
                if let Some(pp) = g.shaders.per_primitive(g.dev(), &v[i].key, &locs) {
                    v[i] = pp;
                }
            }
        }
        v
    } else {
        [0usize, 1, 2, 3, 4]
            .iter()
            .filter_map(|&s| qs.prog[s].clone())
            .filter(|m| m.mesh == 0)
            .collect()
    };
    // Match the vertex shader's input classes to the actual fetch formats before choosing a pipeline variant.
    let mut mods = mods;
    if !mesh {
        let mut inputs = Vec::new();
        for &(loc, kind) in &vs.inputs {
            let Some(&word) = qs
                .vattr
                .get(loc as usize)
                .filter(|_| (loc as usize) < qs.nvattr)
            else {
                continue;
            };
            let attr = va_st(word);
            if attr.fmt() == 0 {
                continue;
            }
            if let Some(fetch) =
                super::shader::vertex_fetch_kind(format::vertex_format(attr.fmt() as i32))
            {
                if fetch != kind {
                    inputs.push((loc, fetch));
                }
            }
        }
        if !inputs.is_empty() {
            inputs.sort_by_key(|&(loc, _)| loc);
            let Some(typed) = g.shaders.with_vertex_inputs(g.dev(), &vs, &inputs) else {
                super::perf::inc(super::perf::C::DrawsSkipped);
                super::perf::inc(super::perf::C::SkipNoShader);
                return false;
            };
            if let Some(m) = mods.iter_mut().find(|m| m.stage == 0) {
                *m = typed.clone();
            }
            vs = typed;
        }
    }
    // Transform feedback uses the last vertex-processing stage's capture variant.
    let mut xfb_layout = None;
    if qs.tf_on && !mesh && g.d.caps.xfb {
        let last = if mods.iter().any(|m| m.stage == 2) {
            2
        } else if mods.iter().any(|m| m.stage == 4) {
            4
        } else {
            0
        };
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
    // the pixel shader for the blend and alpha state: target 0 blending with a second source (dual-source variant) and
    // NVN's alpha test (in the shader, as Turbo / nxnative do)
    let mut dual = false;
    if let Some(i) = mods.iter().position(|m| m.stage == 1) {
        let n = qs.pass_formats.1;
        let b0 = qs.blend[0];
        let src1 = |f: u32| (16..=19).contains(&f);
        let wants_dual = n > 0
            && qs.chmask & 0xf != 0
            && qs.color.blend_en() & 1 != 0
            && [
                blend_field::SRC_RGB,
                blend_field::DST_RGB,
                blend_field::SRC_A,
                blend_field::DST_A,
            ]
            .iter()
            .any(|&f| src1(b0.get(f)));
        let af = qs.color.alpha_func();
        let alpha =
            af != cmp::ALWAYS && af != 0 && A_TEST.load(std::sync::atomic::Ordering::Relaxed);
        let try_dual = wants_dual && g.d.caps.dual_src;
        if try_dual || alpha {
            match g
                .shaders
                .fragment_variant(g.dev(), &mods[i], try_dual, alpha)
            {
                Some(v) => {
                    mods[i] = v;
                    dual = try_dual;
                }
                // without the dual-source part, the alpha test alone
                None if try_dual && alpha => {
                    if let Some(v) = g.shaders.fragment_variant(g.dev(), &mods[i], false, true) {
                        mods[i] = v;
                    }
                }
                None => {}
            }
        }
    }
    qs.dual = dual;
    let mut key = super::pipe::GfxKey::default();
    for m in &mods {
        key.shaders[m.stage as usize] = m.key;
    }
    let (formats, n, dfmt, samples) = qs.pass_formats;
    key.colors = formats;
    key.ncolor = n as u8;
    key.depth = dfmt;
    key.samples = samples;
    let Some(pipe) = super::perf::timed(super::perf::C::PipeLookNs, || g.gfx_pipeline(&key, &mods))
    else {
        super::perf::inc(super::perf::C::DrawsSkipped);
        super::perf::inc(super::perf::C::SkipNoPipeline);
        return false;
    };
    super::perf::inc(super::perf::C::Draws);
    let dev = g.dev();
    let cb = sub.main;
    unsafe { dev.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipe) };
    bind_sets(
        g,
        sub,
        qs,
        &mods,
        vk::PipelineBindPoint::GRAPHICS,
        [0; 3],
        base_instance,
    );

    // The terrain geometry shader's append counter is its first storage binding (c0+0x410), not the index list
    // at c0+0x420. Observe draw setup rather than stage_set so descriptor-cache hits count as well.
    if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
        if let Some(m) = mods.iter().find(|m| {
            m.stage == 2
                && m.res.iter().any(|(_, r)| {
                    matches!(
                        r,
                        Res::Sbuf {
                            cb_slot: 0,
                            cb_off: 0x410,
                            written: true
                        }
                    )
                })
        }) {
            p.geometry_record(sub, qs.ssbo[2][0].0, &m.key);
        }
    }

    // viewport and origin
    let o = if qs.vp_origin != 0 {
        qs.vp_origin == 2
    } else {
        !qs.origin_ll
    };
    let upper_left = o;
    push_constants(g, sub, qs, upper_left);
    let [x, y, w, h] = qs.vp[0];
    let ps = qs.pass_scale;
    let (x, y, w, h) = if w == 0.0 || h == 0.0 {
        (
            x * ps,
            y * ps,
            qs.pass_extent.width as f32,
            qs.pass_extent.height as f32,
        )
    } else {
        (x * ps, y * ps, w * ps, h * ps)
    };
    let vp = if upper_left {
        vk::Viewport {
            x,
            y: y + h,
            width: w,
            height: -h,
            min_depth: qs.depth_range[0].clamp(0.0, 1.0),
            max_depth: qs.depth_range[1].clamp(0.0, 1.0),
        }
    } else {
        vk::Viewport {
            x,
            y,
            width: w,
            height: h,
            min_depth: qs.depth_range[0].clamp(0.0, 1.0),
            max_depth: qs.depth_range[1].clamp(0.0, 1.0),
        }
    };
    let sc = match qs.scissor {
        Some([sx, sy, sw, sh]) if ps != 1.0 => {
            let (x0, y0) = (
                super::scale::coord(sx.max(0), ps),
                super::scale::coord(sy.max(0), ps),
            );
            let (x1, y1) = (
                super::scale::coord((sx + sw).max(0), ps),
                super::scale::coord((sy + sh).max(0), ps),
            );
            vk::Rect2D {
                offset: vk::Offset2D { x: x0, y: y0 },
                extent: vk::Extent2D {
                    width: (x1 - x0).max(0) as u32,
                    height: (y1 - y0).max(0) as u32,
                },
            }
        }
        Some([sx, sy, sw, sh]) => vk::Rect2D {
            offset: vk::Offset2D {
                x: sx.max(0),
                y: sy.max(0),
            },
            extent: vk::Extent2D {
                width: sw.max(0) as u32,
                height: sh.max(0) as u32,
            },
        },
        None => vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: qs.pass_extent,
        },
    };
    // held inside the pass's render area: Vulkan leaves fragments outside it undefined, and NVIDIA writes them, past
    // the end of the target's image (Super Smash Bros. Ultimate's full-screen pass into a 1920x1080 target with a larger
    // scissor: "invalid write" just past a memory block, in no memory of ours, and the device lost; NXGFX_SCISSOR_CLAMP=0 off)
    let sc = if scissor_clamp() {
        clamp_rect(sc, qs.pass_extent)
    } else {
        sc
    };
    let ds = qs.ds;
    let poly = qs.poly;
    let raster = raster_probe(qs);
    let raster_mask = raster.map(|(_, mask)| mask).unwrap_or(0);
    if let Some((phase, mask)) = raster {
        static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let bit = 1 << phase;
        if SEEN.fetch_or(bit, std::sync::atomic::Ordering::Relaxed) & bit == 0 {
            crate::vklog!("raster probe phase {phase}: depth-off {} cull-off {}, native EQUAL/write=0, cull {} front {} upper-left {upper_left} discard {}, viewport {vp:?} scissor {sc:?}, depth target {:?}",
                mask & 1 != 0, mask & 2 != 0, poly.cull(), poly.front(), qs.rast_discard,
                qs.rt_depth.tex.map(|t| (t.serial, t.mem as u64, t.d.format)));
        }
    }
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
        dev.cmd_set_depth_bounds(
            cb,
            qs.depth_bounds.1.clamp(0.0, 1.0),
            qs.depth_bounds.2.clamp(0.0, 1.0),
        );
        dev.cmd_set_depth_bounds_test_enable(cb, qs.depth_bounds.0 && g.d.caps.depth_bounds);
        for (i, f) in [vk::StencilFaceFlags::FRONT, vk::StencilFaceFlags::BACK]
            .iter()
            .enumerate()
        {
            dev.cmd_set_stencil_compare_mask(cb, *f, qs.stencil[i][0]);
            dev.cmd_set_stencil_write_mask(cb, *f, qs.stencil[i][1]);
            dev.cmd_set_stencil_reference(cb, *f, qs.stencil[i][2]);
        }
        dev.cmd_set_cull_mode(
            cb,
            if raster_mask & 2 != 0 {
                vk::CullModeFlags::NONE
            } else {
                vk::CullModeFlags::from_raw(poly.cull())
            },
        );
        // NVN CW = 0 / CCW = 1; Vulkan COUNTER_CLOCKWISE = 0 / CLOCKWISE = 1. Unflipped, the winding inverts.
        let ccw = poly.front() == front_face::CCW;
        let vk_ccw = if upper_left { ccw } else { !ccw };
        dev.cmd_set_front_face(
            cb,
            if vk_ccw {
                vk::FrontFace::COUNTER_CLOCKWISE
            } else {
                vk::FrontFace::CLOCKWISE
            },
        );
        if !mesh {
            dev.cmd_set_primitive_topology(cb, topology(prim_ty));
        }
        if passdump::active() {
            crate::vklog!(
                "pass dump {}: depth test {} write {} func {} stencil test {}, cull {:#x}, color mask {:#x} output map {:#x}, blend enables {:#x}, target 0 blend rgb {}*src {} {}*dst, alpha {}*src {} {}*dst, logic op {:#x}, alpha to coverage {}, alpha test {}",
                passdump::pass_no(),
                ds.get(ds_field::DT),
                ds.get(ds_field::DW),
                ds.get(ds_field::DF),
                ds.get(ds_field::ST),
                poly.cull(),
                qs.chmask,
                qs.prog[1].as_ref().and_then(|m| m.omap).unwrap_or(u32::MAX),
                qs.color.blend_en(),
                qs.blend[0].get(blend_field::SRC_RGB),
                qs.blend[0].get(blend_field::EQ_RGB),
                qs.blend[0].get(blend_field::DST_RGB),
                qs.blend[0].get(blend_field::SRC_A),
                qs.blend[0].get(blend_field::EQ_A),
                qs.blend[0].get(blend_field::DST_A),
                qs.color.logic_op(),
                qs.ms.a2c,
                qs.color.alpha_func()
            );
        }
        dev.cmd_set_depth_test_enable(cb, ds.get(ds_field::DT) != 0 && raster_mask & 1 == 0);
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
            dev.cmd_set_primitive_restart_enable(
                cb,
                qs.prim_restart.0
                    && !matches!(
                        prim_ty,
                        prim::POINTS | prim::LINES | prim::TRIANGLES | prim::QUADS | prim::PATCHES
                    ),
            );
        }
        let caps = g.d.caps;
        if caps.logic_op_dyn {
            g.d.eds2
                .cmd_set_logic_op(cb, vk::LogicOp::from_raw(qs.color.logic_op() as i32));
        }
        if caps.patch_dyn && !mesh && mods.iter().any(|m| m.stage == 3 || m.stage == 4) {
            g.d.eds2.cmd_set_patch_control_points(cb, qs.patch.max(1));
        }
        let e3 = &g.d.eds3;
        if caps.eds3_poly {
            e3.cmd_set_polygon_mode(
                cb,
                match poly.mode() {
                    0 => vk::PolygonMode::POINT,
                    1 => vk::PolygonMode::LINE,
                    _ => vk::PolygonMode::FILL,
                },
            );
        }
        let sflag = vk::SampleCountFlags::from_raw(samples.max(1) as u32);
        if caps.eds3_samples {
            e3.cmd_set_rasterization_samples(cb, sflag);
        }
        if caps.eds3_sample_mask {
            e3.cmd_set_sample_mask(cb, sflag, &[qs.sample_mask]);
        }
        if caps.eds3_a2c {
            e3.cmd_set_alpha_to_coverage_enable(
                cb,
                qs.ms.a2c != 0 && A2C.load(std::sync::atomic::Ordering::Relaxed),
            );
        }
        if caps.eds3_logic_en {
            e3.cmd_set_logic_op_enable(cb, qs.color.logic_op() != logic_op::COPY);
        }
        if caps.eds3_clamp {
            e3.cmd_set_depth_clamp_enable(cb, qs.depth_clamp);
        }
        if caps.eds3_clip_neg {
            e3.cmd_set_depth_clip_negative_one_to_one(
                cb,
                !crate::nvn::device::DEPTH_ZERO.load(std::sync::atomic::Ordering::Relaxed),
            );
        }
        if caps.eds3_cons_raster {
            e3.cmd_set_conservative_rasterization_mode(
                cb,
                conservative_raster_mode(qs.cons_raster),
            );
        } else if qs.cons_raster {
            crate::log::red_once(
                "SetConservativeRasterEnable: VK_EXT_conservative_rasterization is unavailable",
            );
        }
        if n > 0 {
            let en: Vec<vk::Bool32> = (0..n)
                .map(|i| (qs.color.blend_en() >> i & 1) as vk::Bool32)
                .collect();
            let eq: Vec<vk::ColorBlendEquationEXT> = (0..n)
                .map(|i| {
                    let b = qs.blend[i];
                    // a second source the shader does not give counts as zero (Turbo / nvn_mtl.m blend_zero_src1)
                    let bf = |f: u32| {
                        if i == 0 && qs.dual {
                            blend_factor(f)
                        } else {
                            blend_factor(zero_src1(f))
                        }
                    };
                    vk::ColorBlendEquationEXT {
                        src_color_blend_factor: bf(b.get(blend_field::SRC_RGB)),
                        dst_color_blend_factor: bf(b.get(blend_field::DST_RGB)),
                        color_blend_op: blend_op(b.get(blend_field::EQ_RGB)),
                        src_alpha_blend_factor: bf(b.get(blend_field::SRC_A)),
                        dst_alpha_blend_factor: bf(b.get(blend_field::DST_A)),
                        alpha_blend_op: blend_op(b.get(blend_field::EQ_A)),
                    }
                })
                .collect();
            // The shader's scalar output stores select components. The dynamic mask is NVN's channel state, disabled
            // only for locations absent from the output map (Turbo's Vulkan path); do not apply OmapTargets twice.
            let omap = qs.prog[1].as_ref().and_then(|m| m.omap).unwrap_or(u32::MAX);
            let wm = fragment_write_masks(qs.chmask, omap, n);
            e3.cmd_set_color_blend_enable(cb, 0, &en);
            e3.cmd_set_color_blend_equation(cb, 0, &eq);
            e3.cmd_set_color_write_mask(cb, 0, &wm);
        }
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
    let from = (
        qs.vgen,
        vs.inputs.as_ptr() as usize ^ (vs.inputs.len() << 48),
        super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire),
    );
    if qs.vin_last.borrow().0 == sub.serial && qs.vin_from.get() == from && vin_skip_on() {
        super::perf::inc(super::perf::C::VinSkipped);
        return;
    }
    qs.vin_from.set(from);
    let mut binds: Vec<vk::VertexInputBindingDescription2EXT> = Vec::new();
    let mut attrs: Vec<vk::VertexInputAttributeDescription2EXT> = Vec::new();
    let mut streams_used = 0u32;
    let mut need_zero = false;
    for &(loc, kind) in &vs.inputs {
        let a = va_st(if (loc as usize) < qs.nvattr {
            qs.vattr[loc as usize]
        } else {
            0
        });
        if a.fmt() != 0 {
            let s = a.stream().min(15);
            streams_used |= 1 << s;
            attrs.push(
                vk::VertexInputAttributeDescription2EXT::default()
                    .location(loc)
                    .binding(s)
                    .format(format::vertex_format(a.fmt() as i32))
                    .offset(a.offset()),
            );
        } else {
            need_zero = true;
            let f = match kind {
                InKind::Float => vk::Format::R32G32B32A32_SFLOAT,
                InKind::Sint => vk::Format::R32G32B32A32_SINT,
                InKind::Uint => vk::Format::R32G32B32A32_UINT,
            };
            attrs.push(
                vk::VertexInputAttributeDescription2EXT::default()
                    .location(loc)
                    .binding(31)
                    .format(f)
                    .offset(0),
            );
        }
    }
    let mut bufs: Vec<(u32, vk::Buffer, u64)> = Vec::new();
    for s in 0..16u32 {
        if streams_used & (1 << s) == 0 {
            continue;
        }
        let st = qs.vstream[s as usize];
        let (rate, div) = if st.divisor == 0 {
            (vk::VertexInputRate::VERTEX, 1)
        } else {
            (vk::VertexInputRate::INSTANCE, st.divisor)
        };
        let div = if div > 1 && !g.d.caps.divisor {
            crate::log::red_once("vertex divisor > 1 without VK_EXT_vertex_attribute_divisor");
            1
        } else {
            div
        };
        binds.push(
            vk::VertexInputBindingDescription2EXT::default()
                .binding(s)
                .stride(st.stride)
                .input_rate(rate)
                .divisor(div),
        );
        let (a, n) = qs.vbuf[s as usize];
        match if a != 0 {
            super::mem::resolve(g, sub, a, n.max(1))
        } else {
            None
        } {
            Some((b, o, _)) => bufs.push((s, b, o)),
            None => {
                let z = zero_buffer(g, sub);
                bufs.push((s, z, 0));
            }
        }
    }
    if need_zero {
        binds.push(
            vk::VertexInputBindingDescription2EXT::default()
                .binding(31)
                .stride(0)
                .input_rate(vk::VertexInputRate::VERTEX)
                .divisor(1),
        );
        let z = zero_buffer(g, sub);
        bufs.push((31, z, 0));
    }
    // what this command buffer has set already is not set again
    // the layout as one hash (no allocation: this runs at every draw)
    let mut key = 0xcbf29ce484222325u64;
    let mut mix = |v: u32| key = (key ^ v as u64).wrapping_mul(0x100000001b3);
    for b in &binds {
        mix(b.binding);
        mix(b.stride);
        mix(b.input_rate.as_raw() as u32);
        mix(b.divisor);
    }
    mix(u32::MAX);
    for a in &attrs {
        mix(a.location);
        mix(a.binding);
        mix(a.format.as_raw() as u32);
        mix(a.offset);
    }
    let mut last = qs.vin_last.borrow_mut();
    if last.0 != sub.serial {
        *last = (sub.serial, 0, [(vk::Buffer::null(), u64::MAX); 32]);
    }
    unsafe {
        if last.1 != key {
            g.d.vin.cmd_set_vertex_input(sub.main, &binds, &attrs);
            last.1 = key;
        }
        for (s, b, o) in bufs {
            if last.2[s as usize] != (b, o) {
                stale_check(sub, b, "a vertex buffer");
                use_note(
                    sub,
                    b,
                    o,
                    1 << 24,
                    "a vertex buffer (16 MB from its start assumed)",
                );
                g.dev().cmd_bind_vertex_buffers(sub.main, s, &[b], &[o]);
                last.2[s as usize] = (b, o);
            }
        }
    }
}

/// NXGFX_VIN_SKIP=0: vertex input set up at every draw.
fn vin_skip_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_VIN_SKIP").as_deref() != Ok("0"))
}

/// The indirect count is GPU-owned: the residency lookahead cannot require bytes beyond the backing window.
/// vkCmdBindIndexBuffer itself binds the buffer tail, not a range of this size. Direct draws keep their old span.
fn index_binding_span(indirect: bool, count: i32, index_bytes: u64, room: u64) -> Option<u64> {
    if !matches!(index_bytes, 1 | 2 | 4) {
        return None;
    }
    if indirect {
        let n = room.min(1 << 20) / index_bytes * index_bytes;
        (n != 0).then_some(n)
    } else {
        Some((count.max(0) as u64 * index_bytes).max(1).min(1 << 20))
    }
}

fn conservative_raster_mode(on: bool) -> vk::ConservativeRasterizationModeEXT {
    if on {
        vk::ConservativeRasterizationModeEXT::OVERESTIMATE
    } else {
        vk::ConservativeRasterizationModeEXT::DISABLED
    }
}

/// One-run raster comparison for a selected VS's color-writing EQUAL pass only. Native until AT+STEP, then
/// depth off, culling off, both off, and native again (each phase STEP seconds). The prepass is not selected.
/// NXGFX_RASTER_PROBE_VS=<key>, NXGFX_RASTER_PROBE_AT_S=100, NXGFX_RASTER_PROBE_STEP_S=5.
fn raster_probe(qs: &QState) -> Option<(u8, u8)> {
    struct Config {
        key: [u8; 8],
        start: f64,
        step: f64,
    }
    static C: std::sync::OnceLock<Option<Config>> = std::sync::OnceLock::new();
    let c = C
        .get_or_init(|| {
            let text = std::env::var("NXGFX_RASTER_PROBE_VS").ok()?;
            if text.len() != 16 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let mut key = [0; 8];
            for (i, b) in key.iter_mut().enumerate() {
                *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
            }
            let start = std::env::var("NXGFX_RASTER_PROBE_AT_S")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(100.0);
            let step = std::env::var("NXGFX_RASTER_PROBE_STEP_S")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(5.0);
            if !start.is_finite() || start < 0.0 || !step.is_finite() || step < 1.0 {
                return None;
            }
            Some(Config { key, start, step })
        })
        .as_ref()?;
    if qs.prog[0].as_ref()?.key != c.key
        || qs.chmask & 0xf == 0
        || qs.pass_formats.1 == 0
        || qs.pass_formats.0[0] == 0
        || qs.ds.get(ds_field::DT) == 0
        || qs.ds.get(ds_field::DW) != 0
        || qs.ds.get(ds_field::DF) != cmp::EQUAL
    {
        return None;
    }
    Some(raster_probe_phase(crate::log::elapsed(), c.start, c.step))
}

fn raster_probe_phase(now: f64, start: f64, step: f64) -> (u8, u8) {
    let phase = (((now - start).max(0.0) / step).floor() as u8).min(4);
    (
        phase,
        match phase {
            1 => 1,
            2 => 2,
            3 => 3,
            _ => 0,
        },
    )
}

fn raster_probe_index(qs: &QState, d: &p_draw_idx, room: u64, span: Option<u64>, resolved: bool) {
    let Some((phase, _)) = raster_probe(qs) else {
        return;
    };
    static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let bit = 1 << phase;
    if SEEN.fetch_or(bit, std::sync::atomic::Ordering::Relaxed) & bit != 0 {
        return;
    }
    let vs = qs.prog[0].as_ref().unwrap();
    let bound = qs.vin_last.borrow();
    let attrs: Vec<_> = vs
        .inputs
        .iter()
        .map(|&(loc, kind)| {
            let a = va_st(
                qs.vattr
                    .get(loc as usize)
                    .filter(|_| (loc as usize) < qs.nvattr)
                    .copied()
                    .unwrap_or(0),
            );
            let s = a.stream().min(15) as usize;
            let binding = if a.fmt() == 0 { 31 } else { s };
            let (buffer, offset) = bound.2[binding];
            (
                loc,
                kind,
                a.fmt(),
                a.offset(),
                binding,
                if binding == 31 {
                    0
                } else {
                    qs.vstream[s].stride
                },
                qs.vbuf[s],
                vk::Handle::as_raw(buffer),
                offset,
                ZERO.get().is_some_and(|z| z.buf == buffer),
            )
        })
        .collect();
    crate::vklog!("raster probe phase {phase}: index resolved {resolved}, indirect {:#x}, ibuf {:#x} type {} room {room:#x} residency {span:x?}; attrs (location,source-kind,format,offset,binding,stride,(guest,size),VkBuffer,buffer-offset,zero-fallback) {attrs:x?}", d.ind, d.ibuf, d.itype);
}

/// Index data of a quads draw as triangles (u32), from a vertex range or from the game's index buffer.
fn quads_indices(n: u32, src: Option<(&[u8], i32)>) -> Vec<u32> {
    let get = |i: u32| -> u32 {
        match src {
            None => i,
            Some((b, ty)) => match ty {
                index_type::U8 => b.get(i as usize).copied().unwrap_or(0) as u32,
                index_type::U16 => {
                    u16::from_le_bytes([b[2 * i as usize], b[2 * i as usize + 1]]) as u32
                }
                _ => u32::from_le_bytes(b[4 * i as usize..4 * i as usize + 4].try_into().unwrap()),
            },
        }
    };
    let mut v = Vec::with_capacity((n / 4 * 6) as usize);
    for q in 0..n / 4 {
        let b = q * 4;
        v.extend_from_slice(&[
            get(b),
            get(b + 1),
            get(b + 2),
            get(b),
            get(b + 2),
            get(b + 3),
        ]);
    }
    v
}

fn draw(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_draw) {
    if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
        p.next_frame();
    }
    if d.ind == 0 && d.prim != prim::QUADS {
        if let Some(vs) = qs.prog[0].as_ref() {
            if let Some(p) = super::gmem::probe::arm(vs.key, d.first, d.count, qs.ssbo[0][0].0) {
                *qs.raw_probe.borrow_mut() = Some(p);
            }
        }
    }
    checkpoint(g, sub, qs, d.ind == 0, || {
        format!(
            "a draw of {} vertices x{}{}",
            d.count,
            d.instances,
            if d.ind != 0 {
                format!(", indirect at {:#x}", d.ind)
            } else {
                String::new()
            }
        )
    });
    let inst = if d.instances <= 0 {
        1
    } else {
        d.instances as u32
    };
    append::draw(g, sub, qs, d.ind, false);
    let rec = if d.ind != 0 {
        clamped_draw_record(g, sub, qs, d.ind, 16)
    } else {
        None
    };
    if !draw_setup(g, sub, qs, d.prim, d.base_instance as u32) {
        if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
            p.skipped();
        }
        return;
    }
    if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
        if let (Some(vs), Some((_, info, zero))) = (
            qs.prog[0].as_ref(),
            qs.raw_probe_source
                .get()
                .filter(|(serial, _, _)| *serial == sub.serial),
        ) {
            p.source_binding(&vs.key, info, zero);
        }
    }
    let dev = g.dev();
    if d.ind != 0 {
        if let Some((b, o)) = rec {
            stale_check(sub, b, "an indirect draw record");
            use_note(sub, b, o, 16, "an indirect draw record");
            unsafe { dev.cmd_draw_indirect(sub.main, b, o, 1, 16) };
        }
        return;
    }
    if d.prim == prim::QUADS {
        let idx = quads_indices(d.count as u32, None);
        let bytes =
            unsafe { core::slice::from_raw_parts(idx.as_ptr() as *const u8, idx.len() * 4) };
        let (b, o) = sub.stage_bytes(g, bytes);
        unsafe {
            dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT32);
            dev.cmd_draw_indexed(
                sub.main,
                idx.len() as u32,
                inst,
                0,
                d.first,
                d.base_instance as u32,
            );
        }
        return;
    }
    if d.prim == prim::LINE_LOOP {
        crate::log::red_once("LINE_LOOP drawn as a line strip (the closing segment is missing)");
    }
    unsafe {
        dev.cmd_draw(
            sub.main,
            d.count as u32,
            inst,
            d.first as u32,
            d.base_instance as u32,
        )
    };
    if let Some(p) = qs.raw_probe.borrow_mut().as_mut() {
        if let Some(vs) = qs.prog[0].as_ref() {
            p.issued(&vs.key);
        }
    }
}

fn draw_idx(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_draw_idx) {
    checkpoint(g, sub, qs, d.ind == 0, || {
        format!(
            "an indexed draw of {} indices x{}{}",
            d.count,
            d.instances,
            if d.ind != 0 {
                format!(", indirect at {:#x}", d.ind)
            } else {
                String::new()
            }
        )
    });
    let inst = if d.instances <= 0 {
        1
    } else {
        d.instances as u32
    };
    let isz = match d.itype {
        index_type::U8 => 1u64,
        index_type::U16 => 2,
        _ => 4,
    };
    append::draw(g, sub, qs, d.ind, true);
    let rec = if d.ind != 0 {
        clamped_draw_record(g, sub, qs, d.ind, 20)
    } else {
        None
    };
    if !draw_setup(g, sub, qs, d.prim, d.base_instance as u32) {
        return;
    }
    let dev = g.dev();
    if d.prim == prim::QUADS && d.ind == 0 {
        let src = unsafe {
            core::slice::from_raw_parts(d.ibuf as *const u8, (d.count as u64 * isz) as usize)
        };
        let idx = quads_indices(d.count as u32, Some((src, d.itype)));
        let bytes =
            unsafe { core::slice::from_raw_parts(idx.as_ptr() as *const u8, idx.len() * 4) };
        let (b, o) = sub.stage_bytes(g, bytes);
        unsafe {
            dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT32);
            dev.cmd_draw_indexed(
                sub.main,
                idx.len() as u32,
                inst,
                0,
                d.base_vertex,
                d.base_instance as u32,
            );
        }
        return;
    }
    let ity = match d.itype {
        index_type::U8 if g.d.caps.index_u8 => vk::IndexType::UINT8_EXT,
        index_type::U16 => vk::IndexType::UINT16,
        index_type::U32 => vk::IndexType::UINT32,
        _ => {
            // u8 indices widened to u16 on the CPU
            let src = unsafe {
                core::slice::from_raw_parts(d.ibuf as *const u8, d.count.max(0) as usize)
            };
            let w: Vec<u16> = src.iter().map(|&b| b as u16).collect();
            let bytes =
                unsafe { core::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2) };
            let (b, o) = sub.stage_bytes(g, bytes);
            unsafe {
                dev.cmd_bind_index_buffer(sub.main, b, o, vk::IndexType::UINT16);
                dev.cmd_draw_indexed(
                    sub.main,
                    d.count as u32,
                    inst,
                    0,
                    d.base_vertex,
                    d.base_instance as u32,
                );
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
    let room = if d.ind != 0 {
        super::mem::room(d.ibuf)
    } else {
        0
    };
    let span = index_binding_span(d.ind != 0, d.count, isz, room);
    let resolved = span.and_then(|n| super::mem::resolve(g, sub, d.ibuf, n));
    raster_probe_index(qs, &d, room, span, resolved.is_some());
    let Some((ib, io, _)) = resolved else {
        return;
    };
    let n = span.unwrap();
    unsafe {
        stale_check(sub, ib, "an index buffer");
        use_note(sub, ib, io, n.max(1).min(1 << 20), "an index buffer");
        dev.cmd_bind_index_buffer(sub.main, ib, io, ity);
        if d.ind != 0 {
            if let Some((b, o)) = rec {
                stale_check(sub, b, "an indexed indirect record");
                dev.cmd_draw_indexed_indirect(sub.main, b, o, 1, 20);
            }
        } else {
            dev.cmd_draw_indexed(
                sub.main,
                d.count as u32,
                inst,
                0,
                d.base_vertex,
                d.base_instance as u32,
            );
        }
    }
}

const DKB_NDSR: crate::nvn::prog::ShaderKey = 0x14a8aaf3e0a9edbdu64.to_be_bytes();

#[derive(Clone, Copy)]
struct NdsrImage {
    format: vk::Format,
    ty: vk::ImageType,
    extent: vk::Extent3D,
    samples: vk::SampleCountFlags,
    usage: vk::ImageUsageFlags,
}

fn ndsr_copy_plan(
    key: crate::nvn::prog::ShaderKey,
    source: NdsrImage,
    destination: NdsrImage,
) -> Option<vk::Filter> {
    (key == DKB_NDSR
        && source.format == vk::Format::B10G11R11_UFLOAT_PACK32
        && destination.format == source.format
        && source.ty == vk::ImageType::TYPE_2D
        && destination.ty == vk::ImageType::TYPE_2D
        && source.samples == vk::SampleCountFlags::TYPE_1
        && destination.samples == vk::SampleCountFlags::TYPE_1
        && source.extent.width > 0
        && source.extent.height > 0
        && destination.extent.width > 0
        && destination.extent.height > 0
        && source.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
        && destination
            .usage
            .contains(vk::ImageUsageFlags::TRANSFER_DST))
    .then_some(vk::Filter::LINEAR)
}

/// DKB's terminal NDSR shader is a tensor-core network. The translated kernel leaves its correctly bound output
/// black; nxnative's NDSR path likewise substitutes an ordered image blit for terminal network execution.
fn dkb_ndsr_copy(g: &Gpu, sub: &mut Sub, qs: &QState, cs: &Module, dispatch: [u32; 3]) -> bool {
    if cs.key != DKB_NDSR {
        return false;
    }
    let st = cs.stage as usize;
    let (c0, c2, _, _) = qs.banks(g, sub, st, dispatch, 0);
    let tb = pool_bases(qs).0;
    let id_of = |handle: u32, cbuf_slot: i32| {
        let word = bank_word(
            qs,
            st,
            cs,
            &c0,
            &c2,
            tex_slots(cbuf_slot).0,
            handle & 0x3fff,
        );
        (word & 0xfffff) as i32 + tb
    };
    let source_id = cs.res.iter().find_map(|(_, r)| match r {
        Res::Tex {
            handle,
            cbuf_slot,
            ty,
        } if *ty != 2 => Some(id_of(*handle, *cbuf_slot)),
        _ => None,
    });
    let destination_id = cs.res.iter().find_map(|(_, r)| match r {
        Res::Img {
            handle,
            cbuf_slot,
            ty,
            ..
        } if *ty != 2 => Some(id_of(*handle, *cbuf_slot)),
        _ => None,
    });
    let (Some(source_id), Some(destination_id)) = (source_id, destination_id) else {
        return false;
    };
    g.materialize(source_id);
    g.materialize(destination_id);
    let (Some((source_serial, _)), Some((destination_serial, _))) =
        (g.tables.tex_at(source_id), g.tables.tex_at(destination_id))
    else {
        return false;
    };
    let mut imgs = g.images.lock().unwrap();
    let source_serial = imgs.live(source_serial);
    let destination_serial = imgs.live(destination_serial);
    if source_serial == destination_serial {
        return false;
    }
    g.prepare_image(&mut imgs, source_serial, sub);
    g.prepare_image(&mut imgs, destination_serial, sub);
    let Some(source) = imgs.by_serial.get(&source_serial) else {
        return false;
    };
    let Some(destination) = imgs.by_serial.get(&destination_serial) else {
        return false;
    };
    let source_desc = NdsrImage {
        format: source.format,
        ty: source.ty,
        extent: source.extent,
        samples: source.samples,
        usage: source.usage,
    };
    let destination_desc = NdsrImage {
        format: destination.format,
        ty: destination.ty,
        extent: destination.extent,
        samples: destination.samples,
        usage: destination.usage,
    };
    let Some(mut filter) = ndsr_copy_plan(cs.key, source_desc, destination_desc) else {
        return false;
    };
    let features = g.d.format_features(source_desc.format);
    if !features.contains(vk::FormatFeatureFlags::BLIT_SRC | vk::FormatFeatureFlags::BLIT_DST) {
        return false;
    }
    if !features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR) {
        filter = vk::Filter::NEAREST;
    }
    let (source_image, destination_image) = (source.image, destination.image);
    let layer = vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    };
    let blit = vk::ImageBlit {
        src_subresource: layer,
        src_offsets: [
            vk::Offset3D::default(),
            vk::Offset3D {
                x: source_desc.extent.width as i32,
                y: source_desc.extent.height as i32,
                z: 1,
            },
        ],
        dst_subresource: layer,
        dst_offsets: [
            vk::Offset3D::default(),
            vk::Offset3D {
                x: destination_desc.extent.width as i32,
                y: destination_desc.extent.height as i32,
                z: 1,
            },
        ],
    };
    super::queue::full_barrier(g, sub.main);
    unsafe {
        g.dev().cmd_blit_image(
            sub.main,
            source_image,
            vk::ImageLayout::GENERAL,
            destination_image,
            vk::ImageLayout::GENERAL,
            &[blit],
            filter,
        )
    };
    super::queue::full_barrier(g, sub.main);
    g.tex_gpu_written(&mut imgs, destination_serial);
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        crate::vklog!(
            "DKB NDSR: terminal network replaced by ordered scene blit, ser{source_serial} {}x{} -> ser{destination_serial} {}x{}",
            source_desc.extent.width,
            source_desc.extent.height,
            destination_desc.extent.width,
            destination_desc.extent.height,
        )
    });
    true
}

#[cfg(test)]
mod ndsr_output_tests {
    use super::{ndsr_copy_plan, NdsrImage, DKB_NDSR};
    use ash::vk;

    fn image(w: u32, h: u32, usage: vk::ImageUsageFlags) -> NdsrImage {
        NdsrImage {
            format: vk::Format::B10G11R11_UFLOAT_PACK32,
            ty: vk::ImageType::TYPE_2D,
            extent: vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
            samples: vk::SampleCountFlags::TYPE_1,
            usage,
        }
    }

    #[test]
    fn dkb_terminal_ndsr_copies_exact_sampled_input_to_storage_output() {
        let source = image(
            1440,
            810,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC,
        );
        let destination = image(
            2880,
            1620,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST,
        );

        assert_eq!(
            ndsr_copy_plan(DKB_NDSR, source, destination),
            Some(vk::Filter::LINEAR)
        );
        assert_eq!(ndsr_copy_plan([0; 8], source, destination), None);
        assert_eq!(
            ndsr_copy_plan(
                DKB_NDSR,
                source,
                image(2880, 1620, vk::ImageUsageFlags::STORAGE),
            ),
            None,
            "an output that cannot receive the ordered write must run no host substitution"
        );
    }
}

fn compute_image_ids(
    g: &Gpu,
    sub: &mut Sub,
    qs: &QState,
    cs: &Module,
    dispatch: [u32; 3],
) -> (Vec<(i32, u32, i32)>, Vec<(i32, u32, i32)>) {
    let st = cs.stage as usize;
    let (c0, c2, _, _) = qs.banks(g, sub, st, dispatch, 0);
    let tb = pool_bases(qs).0;
    let id = |handle: u32, cbuf_slot: i32| {
        if !matches!(tex_slots(cbuf_slot).0, -1 | 2) { return 0; }
        let word = bank_word(
            qs,
            st,
            cs,
            &c0,
            &c2,
            tex_slots(cbuf_slot).0,
            handle & 0x3fff,
        );
        (word & 0xfffff) as i32 + tb
    };
    let mut textures = Vec::new();
    let mut images = Vec::new();
    for (_, resource) in &cs.res {
        match resource {
            Res::Tex {
                handle,
                cbuf_slot,
                ty,
            } => textures.push((*cbuf_slot, *handle, if *ty == 3 { id(*handle, *cbuf_slot) } else { 0 })),
            Res::Img {
                handle,
                cbuf_slot,
                ty,
                fmt,
            } => images.push((*cbuf_slot, *handle, if *ty == 3 && *fmt == 0 { id(*handle, *cbuf_slot) } else { 0 })),
            _ => {}
        }
    }
    (textures, images)
}

fn compute_snapshot(
    g: &Gpu,
    sub: &mut Sub,
    qs: &QState,
    shader: Arc<Module>,
    dispatch: p_dispatch,
) -> ComputeSnapshot {
    let dimensions = [
        dispatch.x.max(0) as u32,
        dispatch.y.max(0) as u32,
        dispatch.z.max(0) as u32,
    ];
    let (texture_ids, image_ids) = compute_image_ids(g, sub, qs, &shader, dimensions);
    let owner = |(bank, handle, id)| {
        let captured = g.tables.capture_texture(id, |serial| {
            let imgs = g.images.lock().unwrap();
            imgs.by_serial.get(&serial).and_then(|im| im.backing_creation()).map(|c| c.id)
        });
        (bank, handle, captured)
    };
    let textures = texture_ids.into_iter().map(owner).collect();
    let images = image_ids.into_iter().map(owner).collect();
    ComputeSnapshot {
        shader,
        dispatch,
        ubo: qs.ubo[5],
        ssbo: qs.ssbo[5],
        tex: qs.tex[5],
        samp: qs.samp[5],
        img: qs.img[5],
        tex_pool: qs.tex_pool,
        samp_pool: qs.samp_pool,
        push: qs.push[1],
        pool_generation: super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire),
        textures,
        images,
    }
}

fn replay_compute_snapshot(g: &Gpu, sub: &mut Sub, qs: &mut QState, s: &ComputeSnapshot) {
    let saved = (
        qs.prog[5].clone(),
        qs.ubo[5],
        qs.ssbo[5],
        qs.tex[5],
        qs.samp[5],
        qs.img[5],
        qs.tex_pool,
        qs.samp_pool,
        qs.push[1],
    );
    qs.prog[5] = Some(s.shader.clone());
    qs.ubo[5] = s.ubo;
    qs.ssbo[5] = s.ssbo;
    qs.tex[5] = s.tex;
    qs.samp[5] = s.samp;
    qs.img[5] = s.img;
    qs.tex_pool = s.tex_pool;
    qs.samp_pool = s.samp_pool;
    qs.push[1] = s.push;
    qs.bump0(5);
    qs.bump2(5);
    qs.pgen = new_bgen();
    dispatch_guest(g, sub, qs, s.dispatch);
    (
        qs.prog[5],
        qs.ubo[5],
        qs.ssbo[5],
        qs.tex[5],
        qs.samp[5],
        qs.img[5],
        qs.tex_pool,
        qs.samp_pool,
        qs.push[1],
    ) = saved;
    qs.bump0(5);
    qs.bump2(5);
    qs.pgen = new_bgen();
}

fn flush_ndve(g: &Gpu, sub: &mut Sub, qs: &mut QState) {
    let pending = std::mem::take(&mut qs.ndve.pending);
    super::newndve::prepare::replay_in_order(pending, |snapshot| {
        replay_compute_snapshot(g, sub, qs, snapshot);
    });
}

fn ndve_image_binding(
    g: &Gpu,
    _sub: &mut Sub,
    binding: super::tables::CapturedTexture,
    format: vk::Format,
    sampled: bool,
    storage: bool,
) -> Option<super::newndve::ImageBinding> {
    let captured = binding.registration;
    let (id, serial, registered, generation) = (captured.id, captured.serial, captured.view, binding.creation);
    if g.tables.registration_at(id) != Some(captured) || serial == 0 || generation == 0 { return None; }
    // Observation only: no materialization, upload, storage upgrade or write intent.
    // A cold/aliased/replaced backing has no proof and must use the ordinary guest path.
    let imgs = g.images.lock().unwrap();
    let image = imgs.by_serial.get(&serial)?;
    if image.format != format || image.needs_init || image.needs_upload
        || image.alias_of.is_some() || image.backing_creation()?.id != generation
        || image.sparse.is_some() || image.ty != vk::ImageType::TYPE_2D
        || image.extent.depth != 1 || image.samples != vk::SampleCountFlags::TYPE_1
        || image.scale != 1.0 { return None; }
    let view = |storage| {
        let key = super::image::ViewKey { view: registered.unwrap_or_default(),
            has: registered.is_some(), attach: false, storage };
        let v = image.prepared_view(&key)?;
        let want_format = if storage { super::image::unorm_twin(format).unwrap_or(format) } else { format };
        (v.format == want_format.as_raw() && v.view_type == vk::ImageViewType::TYPE_2D.as_raw()
            && v.aspect == vk::ImageAspectFlags::COLOR.as_raw()
            && v.base_level == 0 && v.levels == 1 && v.base_layer == 0 && v.layers == 1
            && v.components == [vk::ComponentSwizzle::IDENTITY.as_raw(); 4]).then_some(v.view)
    };
    let sampled_view = if sampled { view(false)? } else { 0 };
    let storage_view = if storage { view(true)? } else { 0 };
    let resolved = super::newndve::ImageBinding {
        id,
        serial,
        width: image.extent.width,
        height: image.extent.height,
        format: image.format.as_raw(),
        image: vk::Handle::as_raw(image.image),
        sampled_view,
        storage_view,
        usage: image.usage.as_raw(),
        generation,
        guest: super::newndve::Span { address: image.tex.mem as u64, size: image.tex.l.size },
    };
    drop(imgs);
    (g.tables.registration_at(id) == Some(captured)).then_some(resolved)
}

fn ndve_operand_image(
    g: &Gpu,
    sub: &mut Sub,
    ids: &[(i32, u32, Option<super::tables::CapturedTexture>)],
    register: u32,
    image: bool,
    width: u32,
    height: u32,
    format: vk::Format,
    sampled: bool,
    storage: bool,
) -> Option<super::newndve::ImageBinding> {
    let binding = super::newndve::prepare::operand(ids.iter().copied(), image, register).ok()??;
    ndve_image_binding(g, sub, binding, format, sampled, storage)
        .filter(|b| b.width == width && b.height == height)
}

fn ndve_registration_captures(network: &[ComputeSnapshot], resampler: &ComputeSnapshot)
    -> Result<[super::tables::CapturedTexture; 6], String> {
    let first = network.first().ok_or("missing first NDVE dispatch")?;
    let last = network.last().ok_or("missing terminal NDVE dispatch")?;
    let role = |s: &ComputeSnapshot, image, register| {
        let entries = if image { &s.images } else { &s.textures };
        super::newndve::prepare::operand(entries.iter().copied(), image, register)
            .map_err(str::to_string)?.ok_or_else(|| "uncaptured NDVE registration".to_string())
    };
    Ok([role(first, false, 1)?, role(last, false, 4)?, role(last, true, 0)?,
        role(resampler, false, 0)?, role(resampler, false, 3)?, role(resampler, true, 1)?])
}

fn ndve_frame_resources(
    g: &Gpu,
    sub: &mut Sub,
    network: &[ComputeSnapshot],
    resampler: &ComputeSnapshot,
) -> Result<super::newndve::FrameResources, &'static str> {
    if network.len() != super::newndve::NETWORK_DISPATCHES || resampler.dispatch.ind != 0
        || resampler.dispatch.x <= 0 || resampler.dispatch.y <= 0 || resampler.dispatch.z <= 0 {
        return Err("incomplete network or indirect/empty resampler");
    }
    let generation = super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire);
    if network.iter().chain(std::iter::once(resampler)).any(|s| s.pool_generation != generation) {
        return Err("NDVE pool backing changed during capture");
    }
    let first = &network[0];
    let terminal = &network[network.len() - 1];
    let picture = ndve_operand_image(
        g,
        sub,
        &first.textures,
        1, false,
        1920,
        1080,
        vk::Format::R8G8B8A8_UNORM,
        true,
        false,
    )
    .ok_or("first layer picture")?;
    let terminal_picture = ndve_operand_image(
        g,
        sub,
        &terminal.textures,
        4, false,
        1920,
        1080,
        vk::Format::R8G8B8A8_UNORM,
        true,
        false,
    )
    .ok_or("terminal picture")?;
    if terminal_picture.id != picture.id || terminal_picture.serial != picture.serial {
        return Err("picture identity changed");
    }
    let network_output = ndve_operand_image(
        g,
        sub,
        &terminal.images,
        0, true,
        1920,
        1080,
        vk::Format::A2B10G10R10_UNORM_PACK32,
        false,
        true,
    )
    .ok_or("network output")?;
    let resample_picture = ndve_operand_image(
        g,
        sub,
        &resampler.textures,
        0, false,
        1920,
        1080,
        vk::Format::A2B10G10R10_UNORM_PACK32,
        true,
        false,
    )
    .ok_or("resampler picture")?;
    if resample_picture.id != network_output.id
        || resample_picture.serial != network_output.serial
        || resample_picture.image != network_output.image
    {
        return Err("resampler input ownership");
    }
    let network_output = super::newndve::ImageBinding {
        sampled_view: resample_picture.sampled_view,
        ..network_output
    };
    let dither = ndve_operand_image(
        g,
        sub,
        &resampler.textures,
        3, false,
        512,
        512,
        vk::Format::A2B10G10R10_UNORM_PACK32,
        true,
        false,
    )
    .ok_or("dither image")?;
    let final_output = ndve_operand_image(
        g,
        sub,
        &resampler.images,
        1, true,
        2560,
        1440,
        vk::Format::R8G8B8A8_SRGB,
        false,
        true,
    )
    .ok_or("resampler output")?;
    let mut constants = [0; 96];
    constants.copy_from_slice(&resampler.push[..96]);
    let resources = super::newndve::FrameResources {
        picture,
        network_output,
        dither,
        final_output,
        resample_weights: super::newndve::Span {
            address: resampler.ssbo[2].0,
            size: resampler.ssbo[2].1,
        },
        constants,
    };
    if let Err(reason) = resources.validate(picture.id) {
        crate::log::red_once(&format!(
            "NDVE resolved resources rejected ({reason}): {resources:?}"
        ));
        return Err(reason);
    }
    Ok(resources)
}

fn ndve_info(
    g: &Gpu,
    sub: &mut Sub,
    address: u64,
    size: u64,
) -> Result<vk::DescriptorBufferInfo, String> {
    let _ = sub;
    let (buffer, offset) = super::mem::prepared_span(g, address, size)
        .ok_or_else(|| format!("guest range {address:#x}+{size:#x}"))?;
    Ok(vk::DescriptorBufferInfo {
        buffer,
        offset,
        range: size,
    })
}

fn ndve_scratch(g: &Gpu, sub: &mut Sub, size: u64) -> Result<vk::DescriptorBufferInfo, String> {
    let (buffer, offset) = sub
        .gpu_scratch(g, size)
        .ok_or_else(|| format!("scratch allocation {size:#x}"))?;
    Ok(vk::DescriptorBufferInfo {
        buffer,
        offset,
        range: size,
    })
}

fn ndve_copy_image_to_buffer(
    g: &Gpu,
    sub: &mut Sub,
    binding: super::newndve::ImageBinding,
    buffer: vk::DescriptorBufferInfo,
) {
    ndve_copy_raw_image(g, sub, vk::Image::from_raw(binding.image), binding.generation, binding.width, binding.height, buffer);
}

fn ndve_copy_raw_image(g: &Gpu, sub: &mut Sub, image: vk::Image, generation: u64, width: u32, height: u32,
    buffer: vk::DescriptorBufferInfo) {
    super::newndve::trace::record(super::newndve::trace::Record {
        serial: sub.serial, kind: 1, groups: [width, height, 1],
        buffers: [[vk::Handle::as_raw(buffer.buffer), buffer.offset, buffer.range], [0; 3], [0; 3], [0; 3], [0; 3]],
        views: [vk::Handle::as_raw(image), 0, 0],
        image_generations: [generation, 0, 0],
        backing_generation: super::mem::POOL_GEN.load(std::sync::atomic::Ordering::Acquire),
        ..Default::default()
    });
    let copy = vk::BufferImageCopy::default()
        .buffer_offset(buffer.offset)
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        });
    unsafe {
        g.dev().cmd_copy_image_to_buffer(
            sub.main,
            image,
            vk::ImageLayout::GENERAL,
            buffer.buffer,
            &[copy],
        )
    };
    super::queue::full_barrier(g, sub.main);
}

struct PreparedNdveFrame {
    resources: super::newndve::FrameResources,
    graph: super::newndve::kernels::PreparedGraph,
    private_network: super::newndve::private_image::PrivateImage,
    private_final: super::newndve::private_image::PrivateImage,
    picture: vk::DescriptorBufferInfo,
    dither: vk::DescriptorBufferInfo,
    network_output: vk::DescriptorBufferInfo,
    final_output: vk::DescriptorBufferInfo,
    comparison: PreparedNdveComparison,
}

impl PreparedNdveFrame {
    fn record(self, g: &Gpu, sub: &mut Sub) -> PreparedNdveComparison {
        use super::newndve::private_image::PrivateImage;
        let network_generation = self.private_network.generation;
        let final_generation = self.private_final.generation;
        let network_image = self.private_network.retain(sub);
        let final_image = self.private_final.retain(sub);
        PrivateImage::initialize(g, sub, network_image);
        PrivateImage::initialize(g, sub, final_image);
        ndve_copy_image_to_buffer(g, sub, self.resources.picture, self.picture);
        ndve_copy_image_to_buffer(g, sub, self.resources.dither, self.dither);
        self.graph.record(g, sub);
        ndve_copy_raw_image(g, sub, network_image, network_generation, 1920, 1080, self.network_output);
        ndve_copy_raw_image(g, sub, final_image, final_generation, 2560, 1440, self.final_output);
        self.comparison
    }
}

struct PreparedNdveComparison {
    kernels: &'static super::newndve::kernels::Kernels<'static>,
    compares: [super::newndve::kernels::PreparedDispatch; 2],
    guest_network: vk::DescriptorBufferInfo,
    guest_final: vk::DescriptorBufferInfo,
    count_buffer: vk::Buffer,
    ptr: usize,
}

fn ndve_host_graph(
    g: &Gpu,
    sub: &mut Sub,
    network: &[ComputeSnapshot],
    resources: super::newndve::FrameResources,
) -> Result<PreparedNdveFrame, String> {
    use super::newndve::kernels::{prepare_graph, GraphBuffers};
    use super::newndve::private_image::PrivateImage;
    // This unconditional gate is independent of recognition and preparation success.
    super::queue::submission_safety::live_ndve_permission()?;
    let picture = ndve_scratch(g, sub, 1920 * 1080 * 4)?;
    let dither = ndve_scratch(g, sub, 512 * 512 * 4)?;
    let network_output = ndve_scratch(g, sub, 1920 * 1080 * 4)?;
    let final_output = ndve_scratch(g, sub, 2560 * 1440 * 4)?;
    let private_network = PrivateImage::new(g, 1920, 1080, vk::Format::A2B10G10R10_UNORM_PACK32)?;
    let private_final = PrivateImage::new(g, 2560, 1440, vk::Format::R8G8B8A8_UNORM)?;
    let offsets = super::newndve::offsets();
    let mut weights = [vk::DescriptorBufferInfo::default(); 12];
    let mut bias = [vk::DescriptorBufferInfo::default(); 12];
    for i in 0..12 {
        let [t, k, n] = super::newndve::LAYERS[i + 1].kernel.shape();
        weights[i] = ndve_info(
            g,
            sub,
            network[0].ssbo[2].0 + offsets[i].0,
            u64::from(t * k * n * 2),
        )?;
        bias[i] = ndve_info(
            g,
            sub,
            network[0].ssbo[2].0 + offsets[i].1,
            u64::from(n * 2),
        )?;
    }
    let resample_weights = ndve_info(g, sub, resources.resample_weights.address, 0x3000)?;
    let graph = prepare_graph(
        g,
        sub,
        GraphBuffers {
            picture,
            network_output,
            final_output,
            dither,
            weights,
            bias,
            resample_weights,
            picture_view: vk::ImageView::from_raw(resources.picture.sampled_view),
            network_sampled_view: private_network.view,
            network_storage_view: private_network.view,
            dither_view: vk::ImageView::from_raw(resources.dither.sampled_view),
            final_storage_view: private_final.view,
            view_generations: [resources.picture.generation, private_network.generation,
                resources.dither.generation, private_final.generation],
        },
    )?;
    let prepared = ndve_prepare_compare(g, sub, network_output, final_output)?;
    // No fallible graph or comparison preparation follows this boundary. Every
    // private image owner transfers before its first use; scratch/pools already
    // belong to this submission. Guest outputs are never graph destinations.
    Ok(PreparedNdveFrame { resources, graph, private_network, private_final,
        picture, dither, network_output, final_output, comparison: prepared })
}

fn ndve_prepare_compare(
    g: &Gpu,
    sub: &mut Sub,
    network: vk::DescriptorBufferInfo,
    final_image: vk::DescriptorBufferInfo,
) -> Result<PreparedNdveComparison, String> {
    let guest_network = ndve_scratch(g, sub, 1920 * 1080 * 4)?;
    let guest_final = ndve_scratch(g, sub, 2560 * 1440 * 4)?;
    let count = g.buffer(
        512,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::GpuToCpu,
        "NDVE shadow counters",
    )?;
    let ptr = count.ptr as usize;
    let count_buffer = count.buf;
    // Ownership must precede even the fill, and survive either compare's error.
    // Recorded commands may still reference the buffer when this function returns Err.
    sub.transient.push(count);
    let counter = |offset| vk::DescriptorBufferInfo {
        buffer: count_buffer,
        offset,
        range: 8,
    };
    let a = super::newndve::kernels::prepare_image_compare(
        g,
        sub,
        1920,
        1080,
        false,
        network,
        guest_network,
        counter(0),
    )?;
    let b = super::newndve::kernels::prepare_image_compare(
        g,
        sub,
        2560,
        1440,
        true,
        final_image,
        guest_final,
        counter(256),
    )?;
    Ok(PreparedNdveComparison { kernels: super::newndve::kernels::shared(g)?, compares: [a, b],
        guest_network, guest_final, count_buffer, ptr })
}

fn ndve_shadow_compare(g: &Gpu, sub: &mut Sub, resources: super::newndve::FrameResources,
    prepared: PreparedNdveComparison) {
    ndve_copy_image_to_buffer(g, sub, resources.network_output, prepared.guest_network);
    ndve_copy_image_to_buffer(g, sub, resources.final_output, prepared.guest_final);
    unsafe { g.dev().cmd_fill_buffer(sub.main, prepared.count_buffer, 0, 512, 0) };
    super::queue::full_barrier(g, sub.main);
    for dispatch in prepared.compares { prepared.kernels.issue(sub, dispatch); }
    let ptr = prepared.ptr;
    sub.post.push(Post::Callback(Box::new(move |_| {
        let counts = unsafe { core::slice::from_raw_parts(ptr as *const u32, 66) };
        if counts[0] == 0 && counts[64] == 0 {
            crate::vklog!(
                "NDVE shadow observation: outputs match this frame only; live integration remains quarantined",
            );
        } else {
            crate::log::red(&format!(
                "NDVE guest shadow mismatch: network {} texels (max {}/1023), resampler {} texels (max {}/255)",
                counts[0], counts[1], counts[64], counts[65]
            ));
        }
    })));
}

fn ndve_dispatch(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_dispatch) -> bool {
    use super::newndve::{Dispatch, Mode, Span, KEYS, NETWORK_DISPATCHES, WEIGHT_BYTES};
    if super::newndve::mode() == Mode::Off {
        return false;
    }
    let Some(cs) = qs.prog[5].clone() else {
        flush_ndve(g, sub, qs);
        return false;
    };
    let key = u64::from_be_bytes(cs.key);
    if qs.ndve.pending.is_empty() {
        if key == KEYS[1] {
            let snapshot = compute_snapshot(g, sub, qs, cs, d);
            qs.ndve.pending.push(snapshot);
            return true;
        }
        return false;
    }
    let position = qs.ndve.pending.len() + 1;
    if position <= NETWORK_DISPATCHES && key == KEYS[position] {
        let snapshot = compute_snapshot(g, sub, qs, cs, d);
        qs.ndve.pending.push(snapshot);
        return true;
    }
    if position == NETWORK_DISPATCHES + 1 && key == KEYS[13] {
        let final_dispatch = compute_snapshot(g, sub, qs, cs, d);
        let table = Span {
            address: qs.ndve.pending[0].ssbo[2].0,
            size: WEIGHT_BYTES,
        };
        let observed: Vec<_> = qs
            .ndve
            .pending
            .iter()
            .map(|s| Dispatch {
                key: u64::from_be_bytes(s.shader.key),
                weights: Span {
                    address: s.ssbo[2].0,
                    size: s.ssbo[2].1,
                },
                bias: Span {
                    address: s.ssbo[3].0,
                    size: s.ssbo[3].1,
                },
                groups: [
                    s.dispatch.x.max(0) as u32,
                    s.dispatch.y.max(0) as u32,
                    s.dispatch.z.max(0) as u32,
                ],
                indirect: s.dispatch.ind != 0,
            })
            .collect();
        if let Err(reason) = super::newndve::validate_sequence(&observed, table) {
            crate::log::red_once(&format!(
                "NDVE frame preflight failed ({reason}); all guest dispatches replayed"
            ));
            flush_ndve(g, sub, qs);
            replay_compute_snapshot(g, sub, qs, &final_dispatch);
            return true;
        }
        let prepared = ndve_registration_captures(&qs.ndve.pending, &final_dispatch).and_then(|captures| {
            g.tables.prepare_captured(&captures, || {
                let resources = ndve_frame_resources(g, sub, &qs.ndve.pending, &final_dispatch)
                    .map_err(str::to_string)?;
                let host = ndve_host_graph(g, sub, &qs.ndve.pending, resources)?;
                Ok((resources, host))
            })
        });
        let ((resources, host), epoch) = match prepared {
            Ok(prepared) => prepared,
            Err(reason) => {
                crate::log::red_once(&format!(
                    "NDVE host graph preflight failed ({reason}); all guest dispatches replayed"
                ));
                flush_ndve(g, sub, qs);
                replay_compute_snapshot(g, sub, qs, &final_dispatch);
                return true;
            }
        };
        // A matching frame cannot certify later resources, weights or frames.
        // Replacement is quarantined; no global one-frame certification latch.
        if !g.tables.registration_unchanged(epoch) {
            flush_ndve(g, sub, qs);
            replay_compute_snapshot(g, sub, qs, &final_dispatch);
            return true;
        }
        let host = host.record(g, sub);
        flush_ndve(g, sub, qs);
        replay_compute_snapshot(g, sub, qs, &final_dispatch);
        // Registration can change concurrently while guest replay acquires images.
        // Never hold a table lock across replay. The monotonic epoch detects even
        // change-and-restore and withholds the entire shadow observation on change.
        if g.tables.registration_unchanged(epoch) {
            ndve_shadow_compare(g, sub, resources, host);
        }
        return true;
    }
    flush_ndve(g, sub, qs);
    if key == KEYS[1] {
        let snapshot = compute_snapshot(g, sub, qs, cs, d);
        qs.ndve.pending.push(snapshot);
        true
    } else {
        false
    }
}

fn dispatch(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_dispatch) {
    if ndve_dispatch(g, sub, qs, d) {
        return;
    }
    dispatch_guest(g, sub, qs, d);
}

fn dispatch_guest(g: &Gpu, sub: &mut Sub, qs: &mut QState, d: p_dispatch) {
    super::queue::wait_diag::phase("dispatch: end pass");
    checkpoint(g, sub, qs, false, || {
        format!(
            "a dispatch of {}x{}x{} groups{}",
            d.x,
            d.y,
            d.z,
            if d.ind != 0 {
                format!(", indirect at {:#x}", d.ind)
            } else {
                String::new()
            }
        )
    });
    end_pass(g, sub, qs);
    let Some(cs) = qs.prog[5].clone() else {
        crate::log::red_once("dispatch with no compute shader bound");
        return;
    };
    // This is also used to replay immutable NDVE snapshots after a failed preflight.
    super::newndve::before_dispatch(u64::from_be_bytes(cs.key));
    let dimensions = [d.x.max(0) as u32, d.y.max(0) as u32, d.z.max(0) as u32];
    if d.ind == 0 && dkb_ndsr_copy(g, sub, qs, &cs, dimensions) {
        super::perf::inc(super::perf::C::Dispatches);
        return;
    }
    super::queue::wait_diag::phase("dispatch: compute pipeline");
    let Some(pipe) = g.compute_pipeline(&cs) else {
        return;
    };
    // an indirect dispatch's group counts held to the device's limits, before the dispatch's own bindings
    super::queue::wait_diag::phase("dispatch: indirect/storage preparation");
    let ind = if d.ind != 0 {
        super::mem::resolve(g, sub, d.ind, 12)
            .map(|(b, o, _)| clamped_indirect(g, sub, b, o, 1, 12, 12, compute_limits(g)))
    } else {
        None
    };
    super::perf::inc(super::perf::C::Dispatches);
    prepare_storage_bindings(
        g,
        sub,
        qs,
        std::slice::from_ref(&cs),
        [d.x as u32, d.y as u32, d.z as u32],
        0,
    );
    unsafe {
        g.dev()
            .cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, pipe)
    };
    super::queue::wait_diag::phase("dispatch: descriptor sets");
    bind_sets(
        g,
        sub,
        qs,
        std::slice::from_ref(&cs),
        vk::PipelineBindPoint::COMPUTE,
        [d.x as u32, d.y as u32, d.z as u32],
        0,
    );
    super::queue::wait_diag::phase("dispatch: push/issue/barrier");
    push_constants(g, sub, qs, false);
    unsafe {
        if d.ind != 0 {
            if let Some((b, o)) = ind {
                stale_check(sub, b, "an indirect dispatch record");
                g.dev().cmd_dispatch_indirect(sub.main, b, o);
            }
        } else {
            g.dev().cmd_dispatch(
                sub.main,
                d.x.max(0) as u32,
                d.y.max(0) as u32,
                d.z.max(0) as u32,
            );
        }
    }
    barrier(g, sub);
}

fn clear_rect(qs: &QState) -> vk::ClearRect {
    let full = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: qs.pass_extent,
    };
    let ps = qs.pass_scale;
    let sc = qs.scissor.map(|[x, y, w, h]| {
        if ps == 1.0 {
            [x, y, w, h]
        } else {
            let (x0, y0) = (super::scale::coord(x, ps), super::scale::coord(y, ps));
            [
                x0,
                y0,
                super::scale::coord(x + w, ps) - x0,
                super::scale::coord(y + h, ps) - y0,
            ]
        }
    });
    let rect = match sc {
        Some([x, y, w, h]) => {
            let x0 = x.max(0) as u32;
            let y0 = y.max(0) as u32;
            let x1 = ((x + w).max(0) as u32).min(qs.pass_extent.width);
            let y1 = ((y + h).max(0) as u32).min(qs.pass_extent.height);
            vk::Rect2D {
                offset: vk::Offset2D {
                    x: x0 as i32,
                    y: y0 as i32,
                },
                extent: vk::Extent2D {
                    width: x1.saturating_sub(x0),
                    height: y1.saturating_sub(y0),
                },
            }
        }
        None => full,
    };
    vk::ClearRect {
        rect,
        base_array_layer: 0,
        layer_count: 1,
    }
}

fn region_copy(
    t: &tex_t,
    view: Option<&view_t>,
    r: &NVNcopyRegion,
    tf: &format::TexFormat,
) -> (vk::ImageSubresourceLayers, vk::Offset3D, vk::Extent3D) {
    let level = view
        .filter(|v| v.bits & VW_LEVELS != 0)
        .map(|v| v.base_level as u32)
        .unwrap_or(0);
    let layer0 = view
        .filter(|v| v.bits & VW_LAYERS != 0)
        .map(|v| v.min_layer as u32)
        .unwrap_or(0);
    let is3d = t.d.target == target::T3D;
    (
        vk::ImageSubresourceLayers {
            aspect_mask: super::image::upload_aspect(tf),
            mip_level: level,
            base_array_layer: if is3d {
                layer0
            } else {
                layer0 + r.zoffset.max(0) as u32
            },
            layer_count: if is3d { 1 } else { r.depth.max(1) as u32 },
        },
        vk::Offset3D {
            x: r.xoffset,
            y: r.yoffset,
            z: if is3d { r.zoffset } else { 0 },
        },
        vk::Extent3D {
            width: r.width.max(1) as u32,
            height: r.height.max(1) as u32,
            depth: if is3d { r.depth.max(1) as u32 } else { 1 },
        },
    )
}

/// A buffer <-> texture copy's buffer layout (texels per row, rows per image) and byte size.
fn buffer_layout(t: &tex_t, r: &NVNcopyRegion, strides: Option<p_copy_strides>) -> (u32, u32, u64) {
    let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
    let tight_row = (r.width.max(1) as u32).div_ceil(bw) as u64 * bpb as u64;
    let rows = (r.height.max(1) as u32).div_ceil(bh) as u64;
    let s = strides.unwrap_or_default();
    let row = if s.row > 0 { s.row as u64 } else { tight_row };
    let img = if s.image > 0 {
        s.image as u64
    } else {
        row * rows
    };
    let row_texels = if s.row > 0 && bpb > 0 {
        (row / bpb as u64) as u32 * bw
    } else {
        0
    };
    let img_rows = if s.image > 0 && row > 0 {
        (img / row) as u32 * bh
    } else {
        0
    };
    (row_texels, img_rows, img * r.depth.max(1) as u64)
}

// Texture snapshots travel with the record. Their original absolute pointers do not:
// flattening, async queueing and offline capture all move the record's bytes.
// An invalid snapshot is the recorder's explicit fallback to the guest object.
unsafe fn record_texture<T>(rec: &[u8], index: usize, fallback: *const tex_t) -> tex_t {
    let off = 8 + snap_off(core::mem::size_of::<T>()) + index * core::mem::size_of::<tex_t>();
    if off + core::mem::size_of::<tex_t>() <= rec.len() {
        let t = core::ptr::read_unaligned(rec.as_ptr().add(off) as *const tex_t);
        if t.magic == MAGIC_TEX {
            return t;
        }
    }
    *fallback
}

#[cfg(test)]
mod record_texture_tests {
    use super::*;

    #[test]
    fn copied_record_uses_owned_snapshot_instead_of_original_pointer() {
        let off = 8 + snap_off(core::mem::size_of::<p_set_rt>());
        let mut rec = vec![0u8; off + 2 * core::mem::size_of::<tex_t>()];
        for i in 0..2 {
            let mut t: tex_t = unsafe { core::mem::zeroed() };
            t.magic = MAGIC_TEX;
            t.serial = 100 + i as u64;
            unsafe {
                core::ptr::write_unaligned(
                    rec.as_mut_ptr()
                        .add(off + i * core::mem::size_of::<tex_t>())
                        as *mut tex_t,
                    t,
                );
            }
        }
        let moved = rec.clone();
        drop(rec);
        for i in 0..2 {
            let t = unsafe { record_texture::<p_set_rt>(&moved, i, core::ptr::null()) };
            assert_eq!(t.serial, 100 + i as u64);
        }
    }

    #[test]
    fn invalid_or_missing_snapshot_retains_guest_fallback() {
        let t: tex_t = unsafe { core::mem::zeroed() };
        let mut t = t;
        t.serial = 42;
        let rec = vec![
            0u8;
            8 + snap_off(core::mem::size_of::<p_clear_tex>())
                + core::mem::size_of::<tex_t>()
        ];
        for bytes in [&rec[..], &[][..]] {
            assert_eq!(
                unsafe { record_texture::<p_clear_tex>(bytes, 0, &t) }.serial,
                42
            );
        }
    }
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
    let (addr, mode, value): (u64, u32, u32) =
        unsafe { (rd_at(stream, 0), rd_at(stream, 8), rd_at(stream, 12)) };
    event_wait_passes(g, addr, mode, value)
}

/// What a parked stream waits for (its first record), for the log.
pub fn wait_desc(stream: &[u8]) -> String {
    if stream.len() < 24 {
        return "a sync of another parked queue".into();
    }
    let h = unsafe { rd::<op_hdr>(stream.as_ptr()) };
    if h.op != op::WAIT_EVENT {
        return format!("record {} first", h.op);
    }
    let (addr, mode, value): (u64, u32, u32) =
        unsafe { (rd_at(stream, 0), rd_at(stream, 8), rd_at(stream, 12)) };
    let cur = unsafe {
        (*(addr as *const std::sync::atomic::AtomicU32)).load(std::sync::atomic::Ordering::Acquire)
    };
    format!("event at {addr:#x} holds {cur:#x}, the wait wants mode {mode} value {value:#x} ({} bytes of records behind it)", stream.len())
}

fn event_wait_passes(g: &Gpu, addr: u64, mode: u32, value: u32) -> bool {
    let cur = unsafe {
        (*(addr as *const std::sync::atomic::AtomicU32)).load(std::sync::atomic::Ordering::Acquire)
    };
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
        super::queue::wait_diag::record(p, h.op, qs.prog[5].as_ref().map(|m| m.key));
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
        g.buffer(
            16,
            vk::BufferUsageFlags::TRANSFORM_FEEDBACK_COUNTER_BUFFER_EXT,
            MemoryLocation::GpuOnly,
            "transform feedback counters",
        )
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
        match if a != 0 && n != 0 {
            super::mem::resolve(g, sub, a, n)
        } else {
            None
        } {
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
    unsafe {
        (xfb.fp().cmd_bind_transform_feedback_buffers_ext)(
            cb,
            0,
            bufs.len() as u32,
            bufs.as_ptr(),
            offs.as_ptr(),
            sizes.as_ptr(),
        )
    };
    let cnt = tf_counters(g);
    match (qs.tf_cont, cnt) {
        (true, Some(c)) => {
            let cs: Vec<vk::Buffer> = (0..l.nbuf).map(|_| c).collect();
            let co: Vec<u64> = (0..l.nbuf as u64).map(|i| 4 * i).collect();
            unsafe {
                (xfb.fp().cmd_begin_transform_feedback_ext)(
                    cb,
                    0,
                    cs.len() as u32,
                    cs.as_ptr(),
                    co.as_ptr(),
                )
            };
        }
        _ => unsafe {
            (xfb.fp().cmd_begin_transform_feedback_ext)(
                cb,
                0,
                0,
                core::ptr::null(),
                core::ptr::null(),
            )
        },
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
            unsafe {
                (g.d.xfb.fp().cmd_end_transform_feedback_ext)(
                    cb,
                    0,
                    cs.len() as u32,
                    cs.as_ptr(),
                    co.as_ptr(),
                )
            };
        }
        None => unsafe {
            (g.d.xfb.fp().cmd_end_transform_feedback_ext)(
                cb,
                0,
                0,
                core::ptr::null(),
                core::ptr::null(),
            )
        },
    }
}

fn record_op(g: &Gpu, sub: &mut Sub, qs: &mut QState, opc: u32, rec: &[u8]) -> bool {
    let dev = g.dev();
    // Binding records may build a later immutable dispatch snapshot. Any command
    // with externally visible work first replays an incomplete transaction.
    if !matches!(
        opc,
        op::BIND_PROGRAM
            | op::BIND_UBO
            | op::BIND_UBO_SMALL
            | op::BIND_SSBO
            | op::BIND_TEX
            | op::BIND_SAMP
            | op::BIND_IMG
            | op::BIND_TEX1
            | op::SET_TEX_POOL
            | op::SET_SAMP_POOL
            | op::PUSH_CONST
            | op::BUFS_DEFERRED
            | op::BARRIER
            | op::DISPATCH
    ) {
        flush_ndve(g, sub, qs);
    }
    // a pass dump also names the work between draws that writes images or memory (clears, copies, dispatches, render
    // conditions), so a target's content that no draw explains can be traced to its op
    if passdump::active() {
        let name = match opc {
            op::CLEAR_COLOR => "clear colour",
            op::CLEAR_DS => "clear depth / stencil",
            op::CLEAR_TEX => "clear texture",
            op::CLEAR_BUF => "clear buffer",
            op::COPY_B2B => "copy buffer to buffer",
            op::COPY_B2T => "copy buffer to texture",
            op::COPY_T2B => "copy texture to buffer",
            op::COPY_T2T => "copy texture to texture",
            op::COPY_STRIDE => "strided copy",
            op::DISPATCH => "dispatch",
            op::RENDER_COND => "render condition",
            op::DOWNSAMPLE => "downsample",
            op::RESOLVE_DEPTH => "resolve depth",
            op::DRAW_TEX => "draw texture",
            _ => "",
        };
        if !name.is_empty() {
            let head: Vec<String> = rec
                .chunks_exact(4)
                .take(8)
                .map(|c| format!("{:x}", u32::from_le_bytes(c.try_into().unwrap())))
                .collect();
            let rect = if matches!(opc, op::CLEAR_COLOR | op::CLEAR_DS) {
                let r = clear_rect(qs).rect;
                format!(
                    " rect {},{} {}x{}",
                    r.offset.x, r.offset.y, r.extent.width, r.extent.height
                )
            } else {
                String::new()
            };
            crate::vklog!(
                "pass dump {}: op {name} [{}]{rect}{}",
                passdump::pass_no(),
                head.join(" "),
                if !qs.render_on {
                    " (render condition false)"
                } else {
                    ""
                }
            );
        }
    }
    match opc {
        op::END => {}
        op::SET_RT => {
            end_pass(g, sub, qs);
            let r: p_set_rt = rd_payload(rec);
            qs.rt_n = r.n.clamp(0, 8) as usize;
            for i in 0..8 {
                qs.rt[i] = Rt {
                    tex: None,
                    view: None,
                };
                if i < qs.rt_n && !r.color[i].is_null() {
                    let t = unsafe { record_texture::<p_set_rt>(rec, i, r.color[i]) };
                    if t.valid() {
                        qs.rt[i] = Rt {
                            tex: Some(t),
                            view: if r.has_cview[i] != 0 {
                                Some(r.cview[i])
                            } else {
                                None
                            },
                        };
                    }
                }
            }
            qs.rt_depth = Rt {
                tex: None,
                view: None,
            };
            if !r.depth.is_null() {
                let t = unsafe { record_texture::<p_set_rt>(rec, qs.rt_n, r.depth) };
                if t.valid() {
                    qs.rt_depth = Rt {
                        tex: Some(t),
                        view: if r.has_dview != 0 {
                            Some(r.dview)
                        } else {
                            None
                        },
                    };
                }
            }
        }
        op::CLEAR_COLOR => {
            // conditional rendering covers clears too (agl's clear-once records; Turbo render_skip)
            if !qs.render_on {
                return true;
            }
            super::perf::inc(super::perf::C::Clears);
            let c: p_clear_color = rd_payload(rec);
            let i = c.index.clamp(0, 7) as usize;
            if i >= qs.rt_n || qs.rt[i].tex.is_none() || !begin_pass(g, sub, qs) {
                return true;
            }
            if c.mask & 0xf != 0xf {
                crate::log::red_once("ClearColor with a partial channel mask clears every channel");
            }
            let kind = qs.rt[i]
                .tex
                .and_then(|t| format::tex_format(t.d.format))
                .map(|f| f.kind)
                .unwrap_or(Kind::Float);
            let v = match kind {
                Kind::Uint => vk::ClearColorValue {
                    uint32: c.rgba.map(|x| x as u32),
                },
                Kind::Sint => vk::ClearColorValue {
                    int32: c.rgba.map(|x| x as i32),
                },
                _ => vk::ClearColorValue { float32: c.rgba },
            };
            let a = [vk::ClearAttachment {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                color_attachment: i as u32,
                clear_value: vk::ClearValue { color: v },
            }];
            let r = clear_rect(qs);
            if r.rect.extent.width > 0 && r.rect.extent.height > 0 {
                unsafe { dev.cmd_clear_attachments(sub.main, &a, &[r]) };
            }
        }
        op::CLEAR_DS => {
            if !qs.render_on {
                return true;
            }
            super::perf::inc(super::perf::C::Clears);
            let c: p_clear_ds = rd_payload(rec);
            let Some(t) = qs.rt_depth.tex else {
                return true;
            };
            if !begin_pass(g, sub, qs) {
                return true;
            }
            let kind = format::tex_format(t.d.format)
                .map(|f| f.kind)
                .unwrap_or(Kind::Depth);
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
                    clear_value: vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: c.depth.clamp(0.0, 1.0),
                            stencil: c.stencil as u32,
                        },
                    },
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
            b.stages = (b.stages & 0x3f)
                | if b.stages & 0x40 != 0 { 8 } else { 0 }
                | if b.stages & 0x80 != 0 { 16 } else { 0 };
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
            qs.vgen += 1;
        }
        op::BIND_VSTREAM => {
            let n: i32 = unsafe { rd_at(rec, 4) };
            for i in 0..n.clamp(0, 16) as usize {
                qs.vstream[i] = unsafe { rd_at(rec, 8 + 8 * i) };
            }
            qs.vgen += 1;
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
            qs.vgen += 1;
        }
        op::BIND_VBUF1 => {
            let b: p_bind_vbuf1 = rd_payload(rec);
            if (b.index as usize) < 16 {
                qs.vbuf[b.index as usize] = (b.addr, b.size as u64);
            }
            qs.vgen += 1;
        }
        op::BIND_UBO | op::BIND_UBO_SMALL => {
            let b: p_bind_buf = if opc == op::BIND_UBO {
                rd_payload(rec)
            } else {
                let s: p_bind_ubo_small = rd_payload(rec);
                p_bind_buf {
                    stage: s.stage as i32,
                    index: s.index as i32,
                    addr: s.addr,
                    size: s.size as u64,
                }
            };
            if (b.stage as usize) < 7 && (b.index as usize) < 16 {
                qs.ubo[b.stage as usize][b.index as usize] = (b.addr, b.size);
                // only compute's c0 carries a uniform table
                if b.stage == 5 {
                    qs.bump0(5);
                }
            }
        }
        op::BIND_SSBO => {
            let b: p_bind_buf = rd_payload(rec);
            if (b.stage as usize) < 7 && (b.index as usize) < 16 {
                qs.ssbo[b.stage as usize][b.index as usize] = (b.addr, b.size);
                qs.bump0(b.stage as usize);
            }
        }
        op::BIND_TEX | op::BIND_SAMP | op::BIND_IMG => {
            let (stage, first, n): (i32, i32, i32) =
                unsafe { (rd_at(rec, 0), rd_at(rec, 4), rd_at(rec, 8)) };
            let (defer, src): (i32, u64) = unsafe { (rd_at(rec, 12), rd_at(rec, 16)) };
            if (stage as usize) < 7 {
                qs.bump2(stage as usize);
                for i in 0..n.clamp(0, 32) as usize {
                    // a deferred bind reads the game's handle array now, as the queue runs it
                    let h: u64 = if defer != 0 && src != 0 {
                        unsafe { core::ptr::read_unaligned((src + 8 * i as u64) as *const u64) }
                    } else {
                        unsafe { rd_at(rec, 24 + 8 * i) }
                    };
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
                qs.bump2(b.stage as usize);
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
        op::CONS_RASTER => qs.cons_raster = rd_payload::<p_rast_discard>(rec).on != 0,
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
            if let (Some((sb, so, _)), Some((db, doff, _))) = (
                super::mem::resolve(g, sub, src, n),
                super::mem::resolve(g, sub, dst, n),
            ) {
                unsafe {
                    dev.cmd_copy_buffer(
                        sub.main,
                        sb,
                        db,
                        &[vk::BufferCopy {
                            src_offset: so,
                            dst_offset: doff,
                            size: n,
                        }],
                    )
                };
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
                unsafe {
                    dev.cmd_copy_buffer(
                        sub.main,
                        sb,
                        db,
                        &[vk::BufferCopy {
                            src_offset: so,
                            dst_offset: doff,
                            size: n,
                        }],
                    )
                };
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
            let t = unsafe { record_texture::<p_copy_b2t>(rec, 0, c.tex) };
            let strides = (rec.len() - 8 >= COPY_STRIDES_OFF + 16)
                .then(|| unsafe { rd_at::<p_copy_strides>(rec, COPY_STRIDES_OFF) });
            let (row, img_rows, bytes) = buffer_layout(&t, &c.r, strides);
            let view = (c.has_view != 0).then_some(c.view);
            let img = if opc == op::COPY_B2T {
                target_image(g, sub, &t).map(|x| (x.0, x.1))
            } else {
                source_image(g, sub, &t).map(|x| (x.0, x.1))
            };
            let Some((image, tf)) = img else { return true };
            if tf.expand3 != 0 || tf.astc.is_some() {
                crate::log::red_once(
                    "buffer <-> texture copy of an RGB or ASTC texture is not converted yet",
                );
                return true;
            }
            if opc == op::COPY_B2T {
                super::mem::sync_range(g, sub, c.src, bytes);
            }
            let Some((b, o, _)) = super::mem::resolve(g, sub, c.src, bytes) else {
                return true;
            };
            let (sl, off, ext) = region_copy(&t, view.as_ref(), &c.r, &tf);
            let r = vk::BufferImageCopy {
                buffer_offset: o,
                buffer_row_length: row,
                buffer_image_height: img_rows,
                image_subresource: sl,
                image_offset: off,
                image_extent: ext,
            };
            if img_scale(g, t.serial) != 1.0 {
                let imgs = g.images.lock().unwrap();
                if let Some(im) = imgs.by_serial.get(&t.serial) {
                    super::image::copy_buf_img(g, sub.main, im, b, r, opc == op::COPY_B2T);
                }
            } else {
                unsafe {
                    if opc == op::COPY_B2T {
                        dev.cmd_copy_buffer_to_image(
                            sub.main,
                            b,
                            image,
                            vk::ImageLayout::GENERAL,
                            &[r],
                        );
                    } else {
                        dev.cmd_copy_image_to_buffer(
                            sub.main,
                            image,
                            vk::ImageLayout::GENERAL,
                            b,
                            &[r],
                        );
                    }
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
            let (s, d) = unsafe {
                (
                    record_texture::<p_copy_t2t>(rec, 0, c.src),
                    record_texture::<p_copy_t2t>(rec, 1, c.dst),
                )
            };
            let Some((si, stf, _)) = source_image(g, sub, &s) else {
                return true;
            };
            let Some((di, dtf, _, _)) = target_image(g, sub, &d) else {
                return true;
            };
            let (ssl, soff, sext) = region_copy(&s, (c.has_sv != 0).then_some(&c.sv), &c.sr, &stf);
            let (dsl, doff, dext) = region_copy(&d, (c.has_dv != 0).then_some(&c.dv), &c.dr, &dtf);
            let (ss, ds) = (img_scale(g, s.serial), img_scale(g, d.serial));
            if ss != 1.0 || ds != 1.0 {
                // resolution scale: a blit of the regions at each image's scale (the game's region sizes kept)
                let flt = if c.flags & 1 != 0 || ss != ds {
                    super::scale::filter(g, dtf.vk, dtf.kind)
                } else {
                    vk::Filter::NEAREST
                };
                let at = |o: vk::Offset3D, s: f32| vk::Offset3D {
                    x: super::scale::coord(o.x, s),
                    y: super::scale::coord(o.y, s),
                    z: o.z,
                };
                let end = |o: vk::Offset3D, e: vk::Extent3D, s: f32| vk::Offset3D {
                    x: super::scale::coord(o.x + e.width as i32, s),
                    y: super::scale::coord(o.y + e.height as i32, s),
                    z: o.z + e.depth as i32,
                };
                let r = vk::ImageBlit {
                    src_subresource: ssl,
                    src_offsets: [at(soff, ss), end(soff, sext, ss)],
                    dst_subresource: dsl,
                    dst_offsets: [at(doff, ds), end(doff, dext, ds)],
                };
                unsafe {
                    dev.cmd_blit_image(
                        sub.main,
                        si,
                        vk::ImageLayout::GENERAL,
                        di,
                        vk::ImageLayout::GENERAL,
                        &[r],
                        flt,
                    )
                };
                barrier(g, sub);
                return true;
            }
            unsafe {
                if sext == dext && stf.vk == dtf.vk {
                    let r = vk::ImageCopy {
                        src_subresource: ssl,
                        src_offset: soff,
                        dst_subresource: dsl,
                        dst_offset: doff,
                        extent: sext,
                    };
                    dev.cmd_copy_image(
                        sub.main,
                        si,
                        vk::ImageLayout::GENERAL,
                        di,
                        vk::ImageLayout::GENERAL,
                        &[r],
                    );
                } else {
                    let end = |o: vk::Offset3D, e: vk::Extent3D| vk::Offset3D {
                        x: o.x + e.width as i32,
                        y: o.y + e.height as i32,
                        z: o.z + e.depth as i32,
                    };
                    let r = vk::ImageBlit {
                        src_subresource: ssl,
                        src_offsets: [soff, end(soff, sext)],
                        dst_subresource: dsl,
                        dst_offsets: [doff, end(doff, dext)],
                    };
                    let filter = if c.flags & 1 != 0 {
                        vk::Filter::LINEAR
                    } else {
                        vk::Filter::NEAREST
                    };
                    dev.cmd_blit_image(
                        sub.main,
                        si,
                        vk::ImageLayout::GENERAL,
                        di,
                        vk::ImageLayout::GENERAL,
                        &[r],
                        filter,
                    );
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
            let t = unsafe { record_texture::<p_clear_tex>(rec, 0, c.tex) };
            let Some((image, tf, ext, _)) = target_image(g, sub, &t) else {
                return true;
            };
            let view = (c.has_view != 0).then_some(c.view);
            let (sl, off, e) = region_copy(&t, view.as_ref(), &c.r, &tf);
            if off.x != 0
                || off.y != 0
                || e.width < (ext.width >> sl.mip_level).max(1)
                || e.height < (ext.height >> sl.mip_level).max(1)
            {
                crate::log::red_once("ClearTexture of part of a level clears the whole level");
            }
            let range = vk::ImageSubresourceRange {
                aspect_mask: sl.aspect_mask,
                base_mip_level: sl.mip_level,
                level_count: 1,
                base_array_layer: sl.base_array_layer,
                layer_count: sl.layer_count,
            };
            unsafe {
                if matches!(tf.kind, Kind::Depth | Kind::DepthStencil | Kind::Stencil) {
                    let v = vk::ClearDepthStencilValue {
                        depth: f32::from_bits(c.c[0]),
                        stencil: c.c[1],
                    };
                    dev.cmd_clear_depth_stencil_image(
                        sub.main,
                        image,
                        vk::ImageLayout::GENERAL,
                        &v,
                        &[range],
                    );
                } else {
                    let v = if c.is_uint != 0 {
                        vk::ClearColorValue { uint32: c.c }
                    } else {
                        vk::ClearColorValue {
                            float32: c.c.map(f32::from_bits),
                        }
                    };
                    dev.cmd_clear_color_image(
                        sub.main,
                        image,
                        vk::ImageLayout::GENERAL,
                        &v,
                        &[range],
                    );
                }
            }
            barrier(g, sub);
        }
        op::SIGNAL_EVENT => {
            let (addr, mode, value): (u64, u32, u32) =
                unsafe { (rd_at(rec, 0), rd_at(rec, 8), rd_at(rec, 12)) };
            sub.post.push(Post::Event { addr, mode, value });
            let mut ps = g.q.pending_signals.lock().unwrap();
            if mode == event_signal::WRITE {
                ps.insert(addr, (mode, value, g.q.last_fence() + 1));
            } else {
                ps.remove(&addr);
            }
        }
        op::WAIT_EVENT => {
            let (addr, mode, value): (u64, u32, u32) =
                unsafe { (rd_at(rec, 0), rd_at(rec, 8), rd_at(rec, 12)) };
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
            super::counters::rc_note(
                addr,
                kind == counter::TIMESTAMP || kind == counter::TIMESTAMP_TOP,
            );
            if kind == counter::SAMPLES_PASSED {
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| crate::vklog!("samples-passed (occlusion) reports in use: each present now waits for the replay to catch up, so their results come back as without asynchronous replay (NXGFX_OCCL_SYNC=0 off)"));
                super::queue::OCCL_FRAME.store(
                    super::present::frames().max(1),
                    std::sync::atomic::Ordering::Relaxed,
                );
                end_query(g, sub, qs);
                sub.post.push(Post::OcclReport { addr });
            } else {
                if kind != counter::TIMESTAMP && kind != counter::TIMESTAMP_TOP {
                    crate::log::red_once(&format!("counter report of type {kind}: reported as 0 (timestamps and samples passed are counted)"));
                }
                let stage = if kind == counter::TIMESTAMP_TOP {
                    vk::PipelineStageFlags2::TOP_OF_PIPE
                } else {
                    vk::PipelineStageFlags2::ALL_COMMANDS
                };
                let q = if sub.queries < 256 {
                    sub.query()
                } else {
                    u32::MAX
                };
                if q != u32::MAX {
                    unsafe { dev.cmd_write_timestamp2(sub.main, stage, sub.query, q) };
                }
                sub.post.push(Post::Counter {
                    addr,
                    kind,
                    query: q,
                });
            }
        }
        op::PUSH_CONST => {
            let p: crate::nvn::cmd2::p_push_const = rd_payload(rec);
            let (o, n) = (p.offset as usize, p.size as usize);
            if o + n <= 0x800 {
                qs.pgen = new_bgen();
                let grp = &mut qs.push[(p.compute != 0) as usize][o..o + n];
                if p.deferred != 0 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(p.src as *const u8, grp.as_mut_ptr(), n)
                    };
                } else {
                    let at = core::mem::size_of::<crate::nvn::cmd2::p_push_const>();
                    grp.copy_from_slice(&rec[8 + at..8 + at + n]);
                }
            }
        }
        op::BUFS_DEFERRED => {
            let b: crate::nvn::cmd2::p_bufs_deferred = rd_payload(rec);
            if (b.stage as usize) < 7 {
                if b.ssbo != 0 || b.stage == 5 {
                    qs.bump0(b.stage as usize);
                }
                for i in 0..b.n.clamp(0, 16) {
                    let k = (b.first + i) as usize;
                    if k >= 16 {
                        break;
                    }
                    let r: [u64; 2] = unsafe {
                        core::ptr::read_unaligned((b.src + 16 * i as u64) as *const [u64; 2])
                    };
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
                    // the counters at addr and addr + 16 as the GPU will find them (counters.rs rc_value); not known: drawn
                    match (
                        super::counters::rc_value(r.addr),
                        super::counters::rc_value(r.addr + 16),
                    ) {
                        (Some(x), Some(y)) => {
                            if m == 0 {
                                x == y
                            } else {
                                x != y
                            }
                        }
                        _ => true,
                    }
                }
                _ => true,
            };
        }
        op::ALPHA_REF => qs.alpha_ref = rd_payload::<crate::nvn::cmd2::p_f32x4>(rec).v[0],
        op::DEPTH_BOUNDS => {
            let p: crate::nvn::cmd2::p_f32x4 = rd_payload(rec);
            qs.depth_bounds = (p.i[0] != 0, p.v[0], p.v[1]);
        }
        op::SAMPLE_MASK => {
            qs.sample_mask = rd_payload::<crate::nvn::cmd2::p_f32x4>(rec).i[0] as u32
        }
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
            sub.post
                .push(Post::Callback(Box::new(move |_g: &Gpu| unsafe {
                    core::ptr::write_unaligned(a as *mut u32, v)
                })));
        }
        op::DOWNSAMPLE => {
            end_pass(g, sub, qs);
            let t: crate::nvn::cmd2::p_tex2 = rd_payload(rec);
            let Some((si, stf, sext)) = source_image(g, sub, &t.src) else {
                return true;
            };
            let Some((di, _dtf, dext, _)) = target_image(g, sub, &t.dst) else {
                return true;
            };
            let asp = super::image::upload_aspect(&stf);
            let sl = |l: u32| vk::ImageSubresourceLayers {
                aspect_mask: asp,
                mip_level: l,
                base_array_layer: 0,
                layer_count: 1,
            };
            let sl0 = if t.has_sv != 0 && t.sv.bits & VW_LEVELS != 0 {
                t.sv.base_level as u32
            } else {
                0
            };
            let dl0 = if t.has_dv != 0 && t.dv.bits & VW_LEVELS != 0 {
                t.dv.base_level as u32
            } else {
                0
            };
            let e = |x: vk::Extent3D, l: u32| vk::Offset3D {
                x: (x.width >> l).max(1) as i32,
                y: (x.height >> l).max(1) as i32,
                z: 1,
            };
            let r = vk::ImageBlit {
                src_subresource: sl(sl0),
                src_offsets: [vk::Offset3D::default(), e(sext, sl0)],
                dst_subresource: sl(dl0),
                dst_offsets: [vk::Offset3D::default(), e(dext, dl0)],
            };
            unsafe {
                dev.cmd_blit_image(
                    sub.main,
                    si,
                    vk::ImageLayout::GENERAL,
                    di,
                    vk::ImageLayout::GENERAL,
                    &[r],
                    if stf.kind == Kind::Float {
                        vk::Filter::LINEAR
                    } else {
                        vk::Filter::NEAREST
                    },
                )
            };
            barrier(g, sub);
        }
        op::DLSS => {
            end_pass(g, sub, qs);
            let d: crate::nvn::dlss::p_dlss = rd_payload(rec);
            if d.has & 3 != 3 {
                crate::log::red_once(
                    "EvaluateDlss without its input colour or output texture: skipped",
                );
                return true;
            }
            let Some((si, stf, sext)) = source_image(g, sub, &d.color) else {
                return true;
            };
            let Some((di, _dtf, dext, _)) = target_image(g, sub, &d.out) else {
                return true;
            };
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::vklog!("DLSS evaluation: {}x{} -> {}x{} as a filtered upscale (no DLSS network on this GPU path)", d.iw, d.ih, d.ow, d.oh);
            }
            let (iw, ih) = (
                if d.iw > 0 { d.iw } else { sext.width as i32 },
                if d.ih > 0 { d.ih } else { sext.height as i32 },
            );
            let (ow, oh) = (
                if d.ow > 0 { d.ow } else { dext.width as i32 },
                if d.oh > 0 { d.oh } else { dext.height as i32 },
            );
            let asp = super::image::upload_aspect(&stf);
            let sl = vk::ImageSubresourceLayers {
                aspect_mask: asp,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            };
            // the game's rectangles at each image's resolution scale
            let (ss, ds) = (img_scale(g, d.color.serial), img_scale(g, d.out.serial));
            let (iw, ih) = if d.iw > 0 {
                (iw, ih)
            } else {
                ((iw as f32 / ss) as i32, (ih as f32 / ss) as i32)
            };
            let (ow, oh) = if d.ow > 0 {
                (ow, oh)
            } else {
                ((ow as f32 / ds) as i32, (oh as f32 / ds) as i32)
            };
            let c = |v: i32, s: f32| super::scale::coord(v, s);
            let r = vk::ImageBlit {
                src_subresource: sl,
                src_offsets: [
                    vk::Offset3D {
                        x: c(d.cbase[0], ss),
                        y: c(d.cbase[1], ss),
                        z: 0,
                    },
                    vk::Offset3D {
                        x: c(d.cbase[0] + iw, ss).min(sext.width as i32),
                        y: c(d.cbase[1] + ih, ss).min(sext.height as i32),
                        z: 1,
                    },
                ],
                dst_subresource: sl,
                dst_offsets: [
                    vk::Offset3D {
                        x: c(d.obase[0], ds),
                        y: c(d.obase[1], ds),
                        z: 0,
                    },
                    vk::Offset3D {
                        x: c(d.obase[0] + ow, ds).min(dext.width as i32),
                        y: c(d.obase[1] + oh, ds).min(dext.height as i32),
                        z: 1,
                    },
                ],
            };
            unsafe {
                dev.cmd_blit_image(
                    sub.main,
                    si,
                    vk::ImageLayout::GENERAL,
                    di,
                    vk::ImageLayout::GENERAL,
                    &[r],
                    vk::Filter::LINEAR,
                )
            };
            barrier(g, sub);
        }
        op::RESOLVE_DEPTH => {
            // NVN's depth resolve finishes the bound depth target's compressed / culled state so it can be read as a
            // texture (Turbo leaves it a stub). Vulkan keeps no such state: what remains is that its writes so far are
            // done and visible to what follows: the pass ends there and a barrier follows.
            end_pass(g, sub, qs);
            barrier(g, sub);
            static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::vklog!("ResolveDepthBuffer: the depth target's writes are made visible to later reads (end of pass, barrier; later ones are not logged)");
            }
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

#[cfg(test)]
mod downstream_tests {
    use super::*;

    #[test]
    fn indirect_index_binding_accepts_a_short_valid_backing_tail() {
        assert_eq!(index_binding_span(true, 0, 2, 2048), Some(2048));
        assert_eq!(index_binding_span(true, 0, 2, 21), Some(20));
        assert_eq!(index_binding_span(true, 0, 4, 3), None);
        assert_eq!(index_binding_span(true, 0, 2, 0), None);
        assert_eq!(index_binding_span(true, 0, 2, 2 << 20), Some(1 << 20));
        assert_eq!(index_binding_span(false, 54, 2, 0), Some(108));
        assert_eq!(index_binding_span(false, 0, 2, 0), Some(1));
    }

    #[test]
    fn raster_probe_separates_depth_and_culling_then_restores_native_state() {
        assert_eq!(raster_probe_phase(93.0, 100.0, 5.0), (0, 0));
        assert_eq!(raster_probe_phase(104.999, 100.0, 5.0), (0, 0));
        assert_eq!(raster_probe_phase(105.0, 100.0, 5.0), (1, 1));
        assert_eq!(raster_probe_phase(110.0, 100.0, 5.0), (2, 2));
        assert_eq!(raster_probe_phase(115.0, 100.0, 5.0), (3, 3));
        assert_eq!(raster_probe_phase(120.0, 100.0, 5.0), (4, 0));
        assert_eq!(raster_probe_phase(10000.0, 100.0, 5.0), (4, 0));
    }

    #[test]
    fn nvn_conservative_raster_maps_to_exact_vulkan_modes() {
        assert_eq!(
            conservative_raster_mode(false),
            vk::ConservativeRasterizationModeEXT::DISABLED
        );
        assert_eq!(
            conservative_raster_mode(true),
            vk::ConservativeRasterizationModeEXT::OVERESTIMATE
        );
    }

    #[test]
    fn typed_placeholder_is_narrower_than_a_live_target_mismatch() {
        let small = vk::Extent3D {
            width: 16,
            height: 16,
            depth: 1,
        };
        assert!(generic_typed_placeholder(
            target::T2D,
            small,
            1,
            vk::SampleCountFlags::TYPE_1,
            target::CUBE
        ));
        assert!(generic_typed_placeholder(
            target::T2D,
            small,
            1,
            vk::SampleCountFlags::TYPE_1,
            target::T3D
        ));
        let large = vk::Extent3D {
            width: 320,
            height: 180,
            depth: 1,
        };
        assert!(!generic_typed_placeholder(
            target::T2D,
            large,
            1,
            vk::SampleCountFlags::TYPE_1,
            target::CUBE
        ));
        assert!(!generic_typed_placeholder(
            target::T2D_ARRAY,
            small,
            6,
            vk::SampleCountFlags::TYPE_1,
            target::CUBE
        ));
    }

    #[test]
    fn fragment_output_map_gates_locations_not_channels() {
        let raw = |m: Vec<vk::ColorComponentFlags>| {
            m.into_iter()
                .map(vk::ColorComponentFlags::as_raw)
                .collect::<Vec<_>>()
        };
        // DKB: location 0 stores RGBA, location 1 stores only G/B in SPIR-V, and location 2 is absent. The SPIR-V
        // stores select G/B; the attachment mask remains the game's RGBA state for both written locations.
        assert_eq!(
            raw(fragment_write_masks(0xffff_ffff, 0x6f, 3)),
            [0xf, 0xf, 0]
        );
        assert_eq!(raw(fragment_write_masks(0x0000_0345, 0x6f, 3)), [5, 4, 0]);
        // Other title patterns: sparse locations and single-component outputs are location gates, not component masks.
        assert_eq!(
            raw(fragment_write_masks(0xffff_ffff, 0x00f0_000f, 8)),
            [0xf, 0, 0, 0, 0, 0xf, 0, 0]
        );
        assert_eq!(
            raw(fragment_write_masks(0x89ab_cdef, 0x0000_0101, 4)),
            [0xf, 0, 0xd, 0]
        );
        assert_eq!(
            raw(fragment_write_masks(0xffff_ffff, u32::MAX, 3)),
            [0xf, 0xf, 0xf]
        );
    }
}
