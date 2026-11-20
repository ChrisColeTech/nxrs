//! Frame generation on the presented frames (the built-in mode, needing nothing but the GPU).
//!
//! Each frame the game presents is copied (cropped, turned upright) into a history image. Between the previous and
//! the current one the motion is estimated on a luma pyramid (1/2, 1/4, 1/8 of the frame): a full search at the
//! coarsest level, then a small search around each finer level's doubled answer (`shaders/fg_flow.comp`). The
//! in-between frames are warped from both neighbours along that flow and blended, falling back to the nearer frame
//! where they disagree (`fg_interp.comp`). The generated frames are presented before the real one, spaced over the
//! game's frame interval by a presenting thread of their own, so the display shows (generations + 1) images per
//! game frame.
//!
//! How many to make: the pacer measures the game's frame interval (smoothed). With a target (`lsfg_target`, or 120
//! with `fps_target=120`) it makes enough frames to reach it, at most `lsfg_multiplier - 1` per game frame, carrying
//! the fraction over (credit) so a 45 FPS game at a 120 target alternates 2 and 1 generations. A game at or above the
//! target, below 10 FPS, or in a hitch (a frame longer than three times the average) gets none, and after a hitch
//! the pacer waits a second before making frames again. Without a target the multiplier applies as it is.
//!
//! Mode (`frame_gen_type`): `builtin` (this) or `lsfg` (the player's own Lossless Scaling, lane I's `front/lsfg.rs`);
//! LSFG falls back to this mode when its DLL is missing or unusable (said once).

use super::queue::Sub;
use super::Gpu;
use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static LUMA_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/fg_luma_comp.spv"));
static FLOW_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/fg_flow_comp.spv"));
static INTERP_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/fg_interp_comp.spv"));
static STATS_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/spv/fg_stats_comp.spv"));

const HIST_FMT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
const MAX_GEN: usize = 3;
const LEVELS: usize = 3; // 1/2, 1/4, 1/8

/// The frame-generation settings in force (nxconfig, read again when the settings change).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub on: bool,
    pub lsfg: bool,
    /// images per game frame at most (2..4)
    pub multiplier: u32,
    /// output frames a second aimed for (0: the multiplier as it is)
    pub target: f32,
}

pub fn settings() -> Settings {
    use std::sync::atomic::{AtomicU64, Ordering};
    static GEN: AtomicU64 = AtomicU64::new(u64::MAX);
    static S: Mutex<Option<Settings>> = Mutex::new(None);
    let g = nxconfig::generation();
    let mut s = S.lock().unwrap();
    if GEN.swap(g, Ordering::Relaxed) != g || s.is_none() {
        let (on, mult, target) = nxconfig::lsfg();
        let lsfg = nxconfig::string("frame_gen_type").eq_ignore_ascii_case("lsfg");
        *s = Some(Settings { on: on && std::env::var("NXGFX_FRAMEGEN").as_deref() != Ok("0"), lsfg, multiplier: mult.clamp(2, 4), target });
    }
    s.unwrap()
}

/// Whether the swap chain needs room for generated frames.
pub fn wanted() -> bool {
    settings().on
}

// ---------- pacer ----------
struct Pacer {
    last: Option<Instant>,
    avg: f64,
    credit: f64,
    hold_until: Option<Instant>,
}

impl Pacer {
    /// Generations for the frame presented now.
    fn plan(&mut self, s: &Settings) -> usize {
        let now = Instant::now();
        let Some(last) = self.last.replace(now) else { return 0 };
        let iv = now.duration_since(last).as_secs_f64();
        if iv <= 0.0 {
            return 0;
        }
        if self.avg > 0.0 && iv > self.avg * 3.0 || iv > 0.1 {
            // a hitch (or the game below 10 FPS): no frames, and a second to settle
            self.hold_until = Some(now + Duration::from_secs(1));
            self.credit = 0.0;
            self.avg = if iv > 0.1 { 0.0 } else { self.avg };
            return 0;
        }
        self.avg = if self.avg > 0.0 { self.avg + (iv - self.avg) * 0.2 } else { iv };
        if self.hold_until.is_some_and(|t| now < t) {
            return 0;
        }
        let cap = (s.multiplier as usize - 1).min(MAX_GEN);
        if s.target <= 0.0 {
            return cap;
        }
        let outputs = self.avg * s.target as f64; // images wanted per game frame
        if outputs <= 1.05 {
            self.credit = 0.0;
            return 0;
        }
        self.credit += outputs;
        let n = ((self.credit + 1e-4).floor() as usize).max(1);
        let gens = (n - 1).min(cap);
        self.credit = (self.credit - (gens + 1) as f64).clamp(0.0, 1.0);
        gens
    }
}

// ---------- GPU resources ----------
struct Img {
    image: vk::Image,
    view: vk::ImageView,
    alloc: Option<Allocation>,
    w: u32,
    h: u32,
}

fn make_image(g: &Gpu, w: u32, h: u32, fmt: vk::Format, name: &str) -> Result<Img, String> {
    let dev = g.dev();
    let ci = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(fmt)
        .extent(vk::Extent3D { width: w, height: h, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe { dev.create_image(&ci, None) }.map_err(|e| format!("{name}: {e}"))?;
    let req = unsafe { dev.get_image_memory_requirements(image) };
    let a = g
        .alloc
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc { name, requirements: req, location: MemoryLocation::GpuOnly, linear: false, allocation_scheme: AllocationScheme::GpuAllocatorManaged })
        .map_err(|e| format!("{name}: {e}"))?;
    unsafe { dev.bind_image_memory(image, a.memory(), a.offset()) }.map_err(|e| format!("{name}: {e}"))?;
    let vci = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(fmt)
        .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 });
    let view = unsafe { dev.create_image_view(&vci, None) }.map_err(|e| format!("{name}: {e}"))?;
    Ok(Img { image, view, alloc: Some(a), w, h })
}

fn free_image(g: &Gpu, i: Img) {
    unsafe {
        g.dev().destroy_image_view(i.view, None);
        g.dev().destroy_image(i.image, None);
    }
    if let Some(a) = i.alloc {
        let _ = g.alloc.lock().unwrap().free(a);
    }
}

struct Frames {
    w: u32,
    h: u32,
    /// two history frames and their luma pyramids
    hist: [Img; 2],
    luma: [[Img; LEVELS]; 2],
    /// the flow at each level (finest first)
    flow: [Img; LEVELS],
    gen: Vec<Img>,
    /// the whole-frame check's state (fg_stats.comp): mode, warm-up frames left, changed and big-change counts
    state: super::Buf,
    cur: usize,
    have_prev: bool,
    fresh: bool,
}

#[derive(Clone, Copy)]
struct H {
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    luma: vk::Pipeline,
    flow: vk::Pipeline,
    interp: vk::Pipeline,
    stats: vk::Pipeline,
    sampler: vk::Sampler,
}

struct Fg {
    h: H,
    frames: Option<Frames>,
    pacer: Pacer,
    // statistics since the last line
    t_line: Instant,
    base: u64,
    out: u64,
    said_lsfg: bool,
    lsfg: Option<super::lsfg::Lsfg>,
    lsfg_failed: bool,
}

static FG: Mutex<Option<Fg>> = Mutex::new(None);

fn words(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

impl Fg {
    fn new(g: &Gpu) -> Result<Fg, String> {
        let dev = g.dev();
        let ty = |b: u32| match b {
            3 => vk::DescriptorType::STORAGE_IMAGE,
            4 => vk::DescriptorType::STORAGE_BUFFER,
            _ => vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        };
        let binds: Vec<_> = (0..5).map(|b| vk::DescriptorSetLayoutBinding::default().binding(b).descriptor_type(ty(b)).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE)).collect();
        let set_layout = unsafe { dev.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds), None) }.map_err(|e| e.to_string())?;
        let pcr = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::COMPUTE).size(32)];
        let sl = [set_layout];
        let layout = unsafe { dev.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&sl).push_constant_ranges(&pcr), None) }.map_err(|e| e.to_string())?;
        let mk = |spv: &[u8]| -> Result<vk::Pipeline, String> {
            let w = words(spv);
            let m = unsafe { dev.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&w), None) }.map_err(|e| e.to_string())?;
            let st = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::COMPUTE).module(m).name(c"main");
            let p = unsafe { dev.create_compute_pipelines(vk::PipelineCache::null(), &[vk::ComputePipelineCreateInfo::default().stage(st).layout(layout)], None) }.map_err(|(_, e)| e.to_string())?[0];
            unsafe { dev.destroy_shader_module(m, None) };
            Ok(p)
        };
        let sci = vk::SamplerCreateInfo::default()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .max_lod(0.0);
        let sampler = unsafe { dev.create_sampler(&sci, None) }.map_err(|e| e.to_string())?;
        Ok(Fg {
            h: H { set_layout, layout, luma: mk(LUMA_SPV)?, flow: mk(FLOW_SPV)?, interp: mk(INTERP_SPV)?, stats: mk(STATS_SPV)?, sampler },
            frames: None,
            pacer: Pacer { last: None, avg: 0.0, credit: 0.0, hold_until: None },
            t_line: Instant::now(),
            base: 0,
            out: 0,
            said_lsfg: false,
            lsfg: None,
            lsfg_failed: false,
        })
    }

}

fn ensure_frames<'a>(g: &Gpu, frames: &'a mut Option<Frames>, w: u32, h: u32) -> Result<&'a mut Frames, String> {
    {
        let this = frames;
        if this.as_ref().is_some_and(|f| f.w != w || f.h != h) {
            // the old images may be in use by submissions in flight
            super::queue::wait_fence(g.q.last_fence(), u64::MAX);
            let f = this.take().unwrap();
            for i in f.hist.into_iter().chain(f.luma.into_iter().flatten()).chain(f.flow).chain(f.gen) {
                free_image(g, i);
            }
            g.free_buffer(f.state);
        }
        if this.is_none() {
            let lvl = |k: usize| ((w >> (k + 1)).max(1), (h >> (k + 1)).max(1));
            let mk_luma = |s: usize| -> Result<[Img; LEVELS], String> {
                let a = make_image(g, lvl(0).0, lvl(0).1, vk::Format::R16_SFLOAT, &format!("fg luma {s} 1/2"))?;
                let b = make_image(g, lvl(1).0, lvl(1).1, vk::Format::R16_SFLOAT, &format!("fg luma {s} 1/4"))?;
                let c = make_image(g, lvl(2).0, lvl(2).1, vk::Format::R16_SFLOAT, &format!("fg luma {s} 1/8"))?;
                Ok([a, b, c])
            };
            let fl = |k: usize| make_image(g, lvl(k).0, lvl(k).1, vk::Format::R16G16_SFLOAT, "fg flow");
            let f = Frames {
                w,
                h,
                hist: [make_image(g, w, h, HIST_FMT, "fg history 0")?, make_image(g, w, h, HIST_FMT, "fg history 1")?],
                luma: [mk_luma(0)?, mk_luma(1)?],
                flow: [fl(0)?, fl(1)?, fl(2)?],
                gen: (0..MAX_GEN).map(|i| make_image(g, w, h, HIST_FMT, &format!("fg generated {i}"))).collect::<Result<_, _>>()?,
                state: {
                    let b = g.buffer(64, vk::BufferUsageFlags::STORAGE_BUFFER, MemoryLocation::GpuToCpu, "fg state")?;
                    if !b.ptr.is_null() {
                        unsafe { core::ptr::write_bytes(b.ptr, 0, 64) };
                    }
                    b
                },
                cur: 0,
                have_prev: false,
                fresh: true,
            };
            crate::vklog!("frame generation: built-in, {w}x{h} history, flow at {}x{} .. {}x{}", lvl(0).0, lvl(0).1, lvl(2).0, lvl(2).1);
            *this = Some(f);
        }
        Ok(this.as_mut().unwrap())
    }
}

fn set_for(g: &Gpu, sub: &mut Sub, fg: &H, inputs: [vk::ImageView; 3], out: vk::ImageView, state: vk::Buffer) -> vk::DescriptorSet {
    let set = sub.alloc_set(g, fg.set_layout);
    let ii: Vec<[vk::DescriptorImageInfo; 1]> = inputs.iter().map(|&v| [vk::DescriptorImageInfo { sampler: fg.sampler, image_view: v, image_layout: vk::ImageLayout::GENERAL }]).collect();
    let oi = [vk::DescriptorImageInfo { sampler: vk::Sampler::null(), image_view: out, image_layout: vk::ImageLayout::GENERAL }];
    let mut w: Vec<vk::WriteDescriptorSet> = (0..3).map(|b| vk::WriteDescriptorSet::default().dst_set(set).dst_binding(b as u32).descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&ii[b])).collect();
    w.push(vk::WriteDescriptorSet::default().dst_set(set).dst_binding(3).descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(&oi));
    let bi = [vk::DescriptorBufferInfo { buffer: state, offset: 0, range: 64 }];
    w.push(vk::WriteDescriptorSet::default().dst_set(set).dst_binding(4).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&bi));
    unsafe { g.dev().update_descriptor_sets(&w, &[]) };
    set
}

fn dispatch(g: &Gpu, sub: &Sub, fg: &H, pipe: vk::Pipeline, set: vk::DescriptorSet, push: &[u8], w: u32, h: u32) {
    let dev = g.dev();
    unsafe {
        dev.cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, pipe);
        dev.cmd_bind_descriptor_sets(sub.main, vk::PipelineBindPoint::COMPUTE, fg.layout, 0, &[set], &[]);
        dev.cmd_push_constants(sub.main, fg.layout, vk::ShaderStageFlags::COMPUTE, 0, push);
        dev.cmd_dispatch(sub.main, w.div_ceil(8), h.div_ceil(8), 1);
    }
    super::queue::full_barrier(g, sub.main);
}

fn bytes<T: Copy>(v: &T) -> Vec<u8> {
    // SAFETY: plain push-constant structs
    unsafe { core::slice::from_raw_parts(v as *const T as *const u8, core::mem::size_of::<T>()) }.to_vec()
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PLuma {
    size: [i32; 2],
    inv_src: [f32; 2],
    from_rgb: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct PFlow {
    size: [i32; 2],
    radius: i32,
    has_coarse: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct PStats {
    size: [i32; 2],
    same: f32,
    cut_d: f32,
    cut_f: f32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct PInterp {
    size: [i32; 2],
    inv_size: [f32; 2],
    t: f32,
    flow_scale: f32,
}

/// What the present path shows for this game frame: the generated frames (oldest first), then the real one, as
/// images of `w` x `h` in GENERAL layout (upright, cropped).
pub struct Output {
    pub gens: Vec<vk::Image>,
    pub real: vk::Image,
    pub w: u32,
    pub h: u32,
}

/// Records, for a presented frame (`src` and the source region: `y0 > y1` turns it over), the copy into the history
/// and the generated frames the pacer asks for. None: frame generation is off or failed (the caller presents as
/// usual).
pub fn process(g: &Gpu, sub: &mut Sub, src: vk::Image, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<Output> {
    let s = settings();
    if !s.on {
        return None;
    }
    let mut lock = FG.lock().unwrap();
    if lock.is_none() {
        match Fg::new(g) {
            Ok(f) => *lock = Some(f),
            Err(e) => {
                crate::log::red_once(&format!("frame generation: {e}; off"));
                return None;
            }
        }
    }
    let fg = lock.as_mut().unwrap();
    if s.lsfg && !fg.said_lsfg {
        fg.said_lsfg = true;
        crate::vklog!("frame generation: frame_gen_type=lsfg: {}", lsfg_state());
    }
    let (w, h) = ((x1 - x0).unsigned_abs(), (y1 - y0).unsigned_abs());
    if w < 16 || h < 16 {
        return None;
    }
    let gens_wanted = fg.pacer.plan(&s);
    // the LSFG mode, with the player's Lossless.dll; the built-in mode when it cannot run
    if s.lsfg && !fg.lsfg_failed && super::lsfg::shaders_ready() {
        if fg.lsfg.is_none() {
            match super::lsfg::shaders().map(|sh| super::lsfg::Lsfg::new(g, &sh)) {
                Some(Ok(l)) => {
                    crate::vklog!("frame generation: LSFG mode (the player's Lossless.dll)");
                    fg.lsfg = Some(l);
                }
                Some(Err(e)) => {
                    crate::log::red(&format!("frame generation: LSFG cannot run ({e}); the built-in mode is used"));
                    fg.lsfg_failed = true;
                }
                None => {}
            }
        }
        if let Some(l) = fg.lsfg.as_mut() {
            let flow = nxconfig::f32("lsfg_flow").clamp(0.25, 1.0);
            match l.process(g, sub, src, [x0, y0, x1, y1], gens_wanted, flow) {
                Ok((gens, real, w, h)) => {
                    if let Some(&g0) = gens.first() {
                        capture(g, sub, g0, vk::Format::R8G8B8A8_UNORM, w, h, "generated");
                        capture(g, sub, real, vk::Format::R8G8B8A8_UNORM, w, h, "current");
                        CAPTURE_TAKEN.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    note_stats(fg, &s, gens.len(), "LSFG");
                    return Some(Output { gens, real, w, h });
                }
                Err(e) => {
                    crate::log::red(&format!("frame generation: LSFG failed ({e}); the built-in mode is used"));
                    fg.lsfg = None;
                    fg.lsfg_failed = true;
                }
            }
        }
    }
    let hh = fg.h;
    let fg_ro = &hh;
    let frames = match ensure_frames(g, &mut fg.frames, w, h) {
        Ok(f) => f,
        Err(e) => {
            crate::log::red_once(&format!("frame generation images: {e}; off"));
            return None;
        }
    };
    let dev = g.dev();
    if frames.fresh {
        let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
        let all: Vec<vk::Image> =
            frames.hist.iter().chain(frames.luma.iter().flatten()).chain(frames.flow.iter()).chain(frames.gen.iter()).map(|i| i.image).collect();
        let b: Vec<vk::ImageMemoryBarrier2> = all
            .iter()
            .map(|&im| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::NONE)
                    .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                    .dst_access_mask(vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .image(im)
                    .subresource_range(range)
            })
            .collect();
        unsafe { dev.cmd_pipeline_barrier2(sub.main, &vk::DependencyInfo::default().image_memory_barriers(&b)) };
        frames.fresh = false;
    }
    // the history: this frame upright and cropped
    let cur = frames.cur ^ 1;
    let blit = vk::ImageBlit {
        src_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
        src_offsets: [vk::Offset3D { x: x0, y: y0, z: 0 }, vk::Offset3D { x: x1, y: y1, z: 1 }],
        dst_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
        dst_offsets: [vk::Offset3D::default(), vk::Offset3D { x: w as i32, y: h as i32, z: 1 }],
    };
    unsafe { dev.cmd_blit_image(sub.main, src, vk::ImageLayout::GENERAL, frames.hist[cur].image, vk::ImageLayout::GENERAL, &[blit], vk::Filter::NEAREST) };
    super::queue::full_barrier(g, sub.main);
    // its luma pyramid
    for k in 0..LEVELS {
        let (src_view, sw, sh, rgb) = if k == 0 { (frames.hist[cur].view, w, h, 1) } else { (frames.luma[cur][k - 1].view, frames.luma[cur][k - 1].w, frames.luma[cur][k - 1].h, 0) };
        let d = &frames.luma[cur][k];
        let set = set_for(g, sub, fg_ro, [src_view; 3], d.view, frames.state.buf);
        let p = PLuma { size: [d.w as i32, d.h as i32], inv_src: [1.0 / sw as f32, 1.0 / sh as f32], from_rgb: rgb };
        dispatch(g, sub, fg_ro, fg_ro.luma, set, &bytes(&p), d.w, d.h);
    }
    let real = frames.hist[cur].image;
    frames.cur = cur;
    let had_prev = std::mem::replace(&mut frames.have_prev, true);
    let gens = if had_prev { gens_wanted } else { 0 };
    let prev = cur ^ 1;
    if had_prev {
        // the whole-frame check on the coarsest luma (a duplicate, a scene cut and its warm-up show the previous frame
        // in between instead of a warped guess: Eglantine's duplicate skip and warm-up; the cut test is new)
        let k = LEVELS - 1;
        let d = &frames.luma[cur][k];
        let set = set_for(g, sub, fg_ro, [frames.luma[cur][k].view, frames.luma[prev][k].view, d.view], d.view, frames.state.buf);
        let p = PStats { size: [d.w as i32, d.h as i32], same: 1.0 / 512.0, cut_d: 0.2, cut_f: 0.5 };
        let dev = g.dev();
        unsafe {
            dev.cmd_bind_pipeline(sub.main, vk::PipelineBindPoint::COMPUTE, fg_ro.stats);
            dev.cmd_bind_descriptor_sets(sub.main, vk::PipelineBindPoint::COMPUTE, fg_ro.layout, 0, &[set], &[]);
            dev.cmd_push_constants(sub.main, fg_ro.layout, vk::ShaderStageFlags::COMPUTE, 0, &bytes(&p));
            dev.cmd_dispatch(sub.main, 1, 1, 1);
        }
        super::queue::full_barrier(g, sub.main);
    }
    if gens > 0 {
        // the flow, coarse to fine
        for k in (0..LEVELS).rev() {
            let d = &frames.flow[k];
            let coarse = if k + 1 < LEVELS { frames.flow[k + 1].view } else { frames.flow[k].view };
            let set = set_for(g, sub, fg_ro, [frames.luma[cur][k].view, frames.luma[prev][k].view, coarse], d.view, frames.state.buf);
            let p = PFlow { size: [d.w as i32, d.h as i32], radius: if k + 1 == LEVELS { 4 } else { 1 }, has_coarse: (k + 1 < LEVELS) as i32 };
            dispatch(g, sub, fg_ro, fg_ro.flow, set, &bytes(&p), d.w, d.h);
        }
        for i in 0..gens {
            let t = (i + 1) as f32 / (gens + 1) as f32;
            let o = &frames.gen[i];
            let set = set_for(g, sub, fg_ro, [frames.hist[cur].view, frames.hist[prev].view, frames.flow[0].view], o.view, frames.state.buf);
            let p = PInterp { size: [w as i32, h as i32], inv_size: [1.0 / w as f32, 1.0 / h as f32], t, flow_scale: 2.0 };
            dispatch(g, sub, fg_ro, fg_ro.interp, set, &bytes(&p), w, h);
        }
    }
    if gens > 0 {
        capture(g, sub, frames.gen[0].image, HIST_FMT, w, h, "generated");
        capture(g, sub, frames.hist[prev].image, HIST_FMT, w, h, "previous");
        capture(g, sub, real, HIST_FMT, w, h, "current");
        CAPTURE_TAKEN.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let out = Output { gens: frames.gen[..gens].iter().map(|i| i.image).collect(), real, w, h };
    note_stats(fg, &s, gens, if s.lsfg { "built-in (LSFG not available)" } else { "built-in" });
    Some(out)
}

/// The whole-frame check's last answer (fg_stats.comp: 0 interpolate, 1 duplicate, 2 scene cut, 3 warming up) and
/// its counts (changed, big changes), once the submission that made it has completed. Tests and diagnosis.
pub fn last_check() -> Option<(u32, u32, u32)> {
    let l = FG.lock().unwrap();
    let f = l.as_ref()?.frames.as_ref()?;
    if f.state.ptr.is_null() {
        return None;
    }
    // SAFETY: the state buffer's mapped words (written by the GPU, read after its fence)
    let w = unsafe { core::slice::from_raw_parts(f.state.ptr as *const u32, 4) };
    Some((w[0], w[2], w[3]))
}

/// Counts a game frame and its generated frames; a line every 5 s.
fn note_stats(fg: &mut Fg, s: &Settings, gens: usize, mode: &str) {
    fg.base += 1;
    fg.out += gens as u64 + 1;
    let el = fg.t_line.elapsed().as_secs_f64();
    if el >= 5.0 {
        crate::vklog!(
            "frame generation: {mode}, base {:.1} FPS, output {:.1} FPS ({:.2} generated per game frame), target {}, x{} at most",
            fg.base as f64 / el,
            fg.out as f64 / el,
            (fg.out - fg.base) as f64 / fg.base.max(1) as f64,
            s.target,
            s.multiplier
        );
        fg.t_line = Instant::now();
        fg.base = 0;
        fg.out = 0;
    }
}

static CAPTURE_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `NXGFX_FG_SHOT=<s>`: once, after that many seconds, the first generated frame of a game frame and its two
/// neighbours as PNG (`fg-generated.png`, `fg-previous.png`, `fg-current.png` in `NXGFX_SHOT_DIR`).
fn capture(g: &Gpu, sub: &mut Sub, img: vk::Image, fmt: vk::Format, w: u32, h: u32, name: &'static str) {
    use std::sync::atomic::Ordering;
    let Ok(at) = std::env::var("NXGFX_FG_SHOT") else { return };
    if CAPTURE_TAKEN.load(Ordering::Relaxed) || crate::log::elapsed() < at.parse::<f64>().unwrap_or(0.0) {
        return;
    }
    let n = w as u64 * h as u64 * 8;
    let Ok(buf) = g.buffer(n, vk::BufferUsageFlags::TRANSFER_DST, MemoryLocation::GpuToCpu, "fg shot") else { return };
    let r = vk::BufferImageCopy {
        buffer_offset: 0,
        buffer_row_length: 0,
        buffer_image_height: 0,
        image_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
        image_offset: vk::Offset3D::default(),
        image_extent: vk::Extent3D { width: w, height: h, depth: 1 },
    };
    unsafe { g.dev().cmd_copy_image_to_buffer(sub.main, img, vk::ImageLayout::GENERAL, buf.buf, &[r]) };
    super::queue::full_barrier(g, sub.main);
    let ptr = buf.ptr as usize;
    sub.transient.push(buf);
    let dir = std::env::var("NXGFX_SHOT_DIR").unwrap_or_else(|_| "captures".into());
    sub.post.push(super::queue::Post::Callback(Box::new(move |_g: &Gpu| {
        let np = (w * h) as usize;
        // SAFETY: the read-back buffer, kept until after this callback
        let src = unsafe { core::slice::from_raw_parts(ptr as *const u8, n as usize) };
        let mut out = vec![0u8; np * 4];
        if super::shot::to_rgba8(fmt, src, np, &mut out) {
            let _ = std::fs::create_dir_all(&dir);
            let p = std::path::Path::new(&dir).join(format!("fg-{name}.png"));
            let _ = super::shot::png_write(&p, &out, w, h);
            crate::vklog!("frame generation: {} written", p.display());
        }
    })));
}

/// The game frame interval the pacer measured (s), for spacing the presents.
pub fn interval() -> f64 {
    FG.lock().unwrap().as_ref().map(|f| f.pacer.avg).unwrap_or(0.0)
}

// ---------- the LSFG mode's state (lane I's front/lsfg.rs) ----------
static LSFG_STATE: Mutex<Option<fn() -> String>> = Mutex::new(None);

/// The front end tells graphics how to read the LSFG state (`front::lsfg::state`).
pub fn set_lsfg_state(f: fn() -> String) {
    *LSFG_STATE.lock().unwrap() = Some(f);
}

fn lsfg_state() -> String {
    match *LSFG_STATE.lock().unwrap() {
        Some(f) => {
            let s = f();
            format!("Lossless.dll {s}; its passes are not run by graphics yet, the built-in mode generates the frames")
        }
        None => "no Lossless.dll state from the front end; the built-in mode generates the frames".into(),
    }
}

// ---------- paced presenting ----------
struct Paced {
    presents: Vec<super::present::PendingPresent>,
    spacing: Duration,
}

static PACED: Mutex<Option<std::sync::mpsc::Sender<Paced>>> = Mutex::new(None);

/// After the submission that rendered them: the generated frames' presents then the real one's, spaced over the
/// game's frame interval on a thread of their own (the queue lock is taken per present).
pub fn schedule(g: &'static Gpu, presents: Vec<super::present::PendingPresent>) {
    let n = presents.len().max(1) as f64;
    let iv = interval().clamp(0.0, 0.1);
    let spacing = Duration::from_secs_f64(iv / n);
    let mut tx = PACED.lock().unwrap();
    if tx.is_none() {
        let (s, r) = std::sync::mpsc::channel::<Paced>();
        std::thread::Builder::new()
            .name("vk frame-gen present".into())
            .spawn(move || {
                while let Ok(p) = r.recv() {
                    let mut at = Instant::now();
                    for (i, pp) in p.presents.into_iter().enumerate() {
                        if i > 0 {
                            at += p.spacing;
                            let now = Instant::now();
                            if at > now {
                                super::present::precise_sleep((at - now).as_nanos() as u64);
                            }
                        }
                        let _vq = g.q.vkq_lock();
                        super::present::queue_present(g, pp);
                    }
                }
            })
            .ok();
        *tx = Some(s);
    }
    let _ = tx.as_ref().unwrap().send(Paced { presents, spacing });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_fills_to_the_target_with_credit() {
        let s = Settings { on: true, lsfg: false, multiplier: 4, target: 120.0 };
        let mut p = Pacer { last: None, avg: 1.0 / 45.0, credit: 0.0, hold_until: None };
        // 45 FPS to 120: 2.67 images a frame, so 1 or 2 generations alternate, about 1.67 on average
        let mut total = 0;
        for _ in 0..30 {
            p.last = Some(Instant::now() - Duration::from_secs_f64(1.0 / 45.0));
            total += p.plan(&s);
        }
        assert!((45..=55).contains(&total), "{total}");
        // at the target already: none
        let mut p = Pacer { last: None, avg: 1.0 / 120.0, credit: 0.0, hold_until: None };
        p.last = Some(Instant::now() - Duration::from_secs_f64(1.0 / 120.0));
        assert_eq!(p.plan(&s), 0);
    }
}
