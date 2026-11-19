//! Pipelines.
//!
//! Every state NVN changes per draw is dynamic (extended dynamic state 1-3 and dynamic vertex input), so a graphics
//! pipeline's key is only its shaders, its attachment formats and its sample count; every program shares the one
//! layout (`layout`). That keeps the pipeline count close to the program count, which is what bounds the driver's
//! memory: NVIDIA's driver grew to 61 GB when one game's pipelines multiplied per state.
//!
//! New keys are appended to a key file; `prewarm` builds the keys of earlier runs on background threads, stopping at a
//! private-memory budget (`NXGFX_PREWARM_BUDGET_MB`, default 2048) or a count cap. A VkPipelineCache is kept on disk.

use super::shader::Module;
use super::Gpu;
use crate::nvn::prog::ShaderKey;
use ash::vk;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug, Default)]
pub struct GfxKey {
    /// vertex, fragment, geometry, tess control (or task), tess evaluation (or mesh); zero = none
    pub shaders: [ShaderKey; 5],
    pub colors: [i32; 8],
    pub ncolor: u8,
    pub depth: i32,
    pub samples: u8,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
pub enum Key {
    Gfx(GfxKey),
    Compute(ShaderKey),
}

impl Key {
    fn to_bytes(self) -> Vec<u8> {
        let mut v = Vec::new();
        match self {
            Key::Gfx(k) => {
                v.push(0u8);
                for s in k.shaders {
                    v.extend_from_slice(&s);
                }
                for c in k.colors {
                    v.extend_from_slice(&c.to_le_bytes());
                }
                v.push(k.ncolor);
                v.extend_from_slice(&k.depth.to_le_bytes());
                v.push(k.samples);
            }
            Key::Compute(s) => {
                v.push(1u8);
                v.extend_from_slice(&s);
            }
        }
        v
    }
    fn from_bytes(b: &[u8]) -> Option<(Key, usize)> {
        match *b.first()? {
            0 => {
                let n = 1 + 40 + 32 + 1 + 4 + 1;
                if b.len() < n {
                    return None;
                }
                let mut k = GfxKey::default();
                for i in 0..5 {
                    k.shaders[i].copy_from_slice(&b[1 + 8 * i..9 + 8 * i]);
                }
                for i in 0..8 {
                    k.colors[i] = i32::from_le_bytes(b[41 + 4 * i..45 + 4 * i].try_into().ok()?);
                }
                k.ncolor = b[73];
                k.depth = i32::from_le_bytes(b[74..78].try_into().ok()?);
                k.samples = b[78];
                Some((Key::Gfx(k), n))
            }
            1 => {
                if b.len() < 9 {
                    return None;
                }
                let mut s = [0u8; 8];
                s.copy_from_slice(&b[1..9]);
                Some((Key::Compute(s), 9))
            }
            _ => None,
        }
    }
}

pub struct Pipes {
    map: Mutex<HashMap<Key, vk::Pipeline>>,
    pub cache: vk::PipelineCache,
    dir: Mutex<Option<PathBuf>>,
    known: Mutex<std::collections::HashSet<Key>>,
    pub created_sync: AtomicU64,
    pub prewarmed: AtomicU64,
    stop: AtomicBool,
}

fn cache_dir() -> Option<PathBuf> {
    std::env::var("NXGFX_CACHE").ok().map(PathBuf::from)
}

impl Pipes {
    pub fn new(d: &super::device::Device) -> Pipes {
        let dir = cache_dir();
        let data = dir.as_ref().and_then(|p| std::fs::read(p.join("pipeline-cache.bin")).ok()).filter(|data| super::pcache::header_ok(data, &d.props)).unwrap_or_default();
        let ci = vk::PipelineCacheCreateInfo::default().initial_data(&data);
        let cache = unsafe { d.dev.create_pipeline_cache(&ci, None) }
            .or_else(|_| unsafe { d.dev.create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None) })
            .expect("pipeline cache");
        Pipes {
            map: Mutex::new(HashMap::new()),
            cache,
            dir: Mutex::new(dir),
            known: Mutex::new(Default::default()),
            created_sync: AtomicU64::new(0),
            prewarmed: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        }
    }

    /// Where the key file and the pipeline cache live (the game's `_data`).
    pub fn set_dir(&self, p: PathBuf) {
        *self.dir.lock().unwrap() = Some(p);
    }

    fn note_key(&self, k: Key) {
        if !self.known.lock().unwrap().insert(k) {
            return;
        }
        if let Some(d) = self.dir.lock().unwrap().clone() {
            let _ = std::fs::create_dir_all(&d);
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(d.join("pipeline-keys.bin")) {
                let _ = f.write_all(&k.to_bytes());
            }
        }
    }

    /// Writes the pipeline cache now (it is also saved incrementally, `pcache`).
    pub fn save(&self, g: &Gpu) {
        super::pcache::save_now(g);
    }

    pub fn count(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    /// Destroys every pipeline once the GPU is idle (tests of the prewarm; a device lost).
    pub fn clear(&self, g: &Gpu) {
        super::queue::finish();
        let mut m = self.map.lock().unwrap();
        for (_, p) in m.drain() {
            unsafe { g.dev().destroy_pipeline(p, None) };
        }
    }
}

fn dynamic_states(g: &Gpu, tess: bool, mesh: bool) -> Vec<vk::DynamicState> {
    use vk::DynamicState as D;
    let c = &g.d.caps;
    let mut v = vec![
        D::VIEWPORT_WITH_COUNT,
        D::SCISSOR_WITH_COUNT,
        D::LINE_WIDTH,
        D::DEPTH_BIAS,
        D::BLEND_CONSTANTS,
        D::DEPTH_BOUNDS,
        D::STENCIL_COMPARE_MASK,
        D::STENCIL_WRITE_MASK,
        D::STENCIL_REFERENCE,
        D::CULL_MODE,
        D::FRONT_FACE,
        D::PRIMITIVE_TOPOLOGY,
        D::DEPTH_TEST_ENABLE,
        D::DEPTH_WRITE_ENABLE,
        D::DEPTH_COMPARE_OP,
        D::DEPTH_BOUNDS_TEST_ENABLE,
        D::STENCIL_TEST_ENABLE,
        D::STENCIL_OP,
        D::RASTERIZER_DISCARD_ENABLE,
        D::DEPTH_BIAS_ENABLE,
        D::PRIMITIVE_RESTART_ENABLE,
        D::VERTEX_INPUT_EXT,
        D::COLOR_BLEND_ENABLE_EXT,
        D::COLOR_BLEND_EQUATION_EXT,
        D::COLOR_WRITE_MASK_EXT,
    ];
    if mesh {
        // a mesh pipeline has no vertex input or input assembly: those states may not be dynamic
        v.retain(|d| !matches!(*d, D::PRIMITIVE_TOPOLOGY | D::PRIMITIVE_RESTART_ENABLE | D::VERTEX_INPUT_EXT));
    }
    if c.logic_op_dyn {
        v.push(D::LOGIC_OP_EXT);
    }
    if c.patch_dyn && tess && !mesh {
        v.push(D::PATCH_CONTROL_POINTS_EXT);
    }
    if c.eds3_poly {
        v.push(D::POLYGON_MODE_EXT);
    }
    if c.eds3_samples {
        v.push(D::RASTERIZATION_SAMPLES_EXT);
    }
    if c.eds3_sample_mask {
        v.push(D::SAMPLE_MASK_EXT);
    }
    if c.eds3_a2c {
        v.push(D::ALPHA_TO_COVERAGE_ENABLE_EXT);
    }
    if c.eds3_logic_en {
        v.push(D::LOGIC_OP_ENABLE_EXT);
    }
    if c.eds3_clamp {
        v.push(D::DEPTH_CLAMP_ENABLE_EXT);
    }
    if c.eds3_clip_neg {
        v.push(D::DEPTH_CLIP_NEGATIVE_ONE_TO_ONE_EXT);
    }
    v
}

fn samples_flag(n: u8) -> vk::SampleCountFlags {
    match n {
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        _ => vk::SampleCountFlags::TYPE_1,
    }
}

fn depth_has_stencil(f: vk::Format) -> bool {
    matches!(f, vk::Format::D24_UNORM_S8_UINT | vk::Format::D32_SFLOAT_S8_UINT | vk::Format::S8_UINT | vk::Format::D16_UNORM_S8_UINT)
}
fn depth_has_depth(f: vk::Format) -> bool {
    f != vk::Format::UNDEFINED && f != vk::Format::S8_UINT
}

/// `driver_exclusive` (auto = on for Intel, whose driver crashes compiling and drawing at once): pipeline creation and
/// queue submissions take turns.
pub static EXCLUSIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub fn driver_exclusive(g: &Gpu) -> bool {
    nxconfig::tristate("driver_exclusive").unwrap_or(g.d.props.vendor_id == 0x8086)
}

fn build_gfx(g: &Gpu, k: &GfxKey, mods: &[Arc<Module>]) -> Result<vk::Pipeline, String> {
    let _x = driver_exclusive(g).then(|| EXCLUSIVE.lock().unwrap());
    let stages: Vec<vk::PipelineShaderStageCreateInfo> = mods
        .iter()
        .map(|m| vk::PipelineShaderStageCreateInfo::default().stage(m.vk_stage()).module(m.module).name(&m.entry))
        .collect();
    let mesh = mods.iter().any(|m| m.mesh != 0);
    let tess = !mesh && mods.iter().any(|m| m.stage == 3 || m.stage == 4);
    let vi = vk::PipelineVertexInputStateCreateInfo::default();
    let ia = vk::PipelineInputAssemblyStateCreateInfo::default().topology(if tess { vk::PrimitiveTopology::PATCH_LIST } else { vk::PrimitiveTopology::TRIANGLE_LIST });
    let ts = vk::PipelineTessellationStateCreateInfo::default().patch_control_points(3);
    let vp = vk::PipelineViewportStateCreateInfo::default();
    let rs = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).line_width(1.0);
    let mask = [u32::MAX];
    let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples_flag(k.samples)).sample_mask(&mask);
    let ds = vk::PipelineDepthStencilStateCreateInfo::default();
    let att: Vec<vk::PipelineColorBlendAttachmentState> =
        (0..k.ncolor).map(|_| vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)).collect();
    let cb = vk::PipelineColorBlendStateCreateInfo::default().attachments(&att);
    let dyns = dynamic_states(g, tess, mesh);
    let dy = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyns);
    let colors: Vec<vk::Format> = k.colors[..k.ncolor as usize].iter().map(|&f| vk::Format::from_raw(f)).collect();
    let df = vk::Format::from_raw(k.depth);
    let mut ri = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&colors)
        .depth_attachment_format(if depth_has_depth(df) { df } else { vk::Format::UNDEFINED })
        .stencil_attachment_format(if depth_has_stencil(df) { df } else { vk::Format::UNDEFINED });
    let mut ci = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .viewport_state(&vp)
        .rasterization_state(&rs)
        .multisample_state(&ms)
        .depth_stencil_state(&ds)
        .color_blend_state(&cb)
        .dynamic_state(&dy)
        .layout(g.layout.pipeline)
        .push_next(&mut ri);
    if !mesh {
        ci = ci.vertex_input_state(&vi).input_assembly_state(&ia);
    }
    if tess {
        ci = ci.tessellation_state(&ts);
    }
    let p = unsafe { g.dev().create_graphics_pipelines(g.pipes.cache, &[ci], None) }.map_err(|(_, e)| format!("vkCreateGraphicsPipelines: {e}"))?;
    super::pcache::note_created(1);
    Ok(p[0])
}

fn build_compute(g: &Gpu, m: &Module) -> Result<vk::Pipeline, String> {
    let _x = driver_exclusive(g).then(|| EXCLUSIVE.lock().unwrap());
    let st = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::COMPUTE).module(m.module).name(&m.entry);
    let ci = vk::ComputePipelineCreateInfo::default().stage(st).layout(g.layout.pipeline);
    let p = unsafe { g.dev().create_compute_pipelines(g.pipes.cache, &[ci], None) }.map_err(|(_, e)| format!("vkCreateComputePipelines: {e}"))?;
    super::pcache::note_created(1);
    Ok(p[0])
}

fn modules_of(g: &Gpu, k: &GfxKey) -> Option<Vec<Arc<Module>>> {
    let mut v = Vec::new();
    for s in k.shaders {
        if s != [0; 8] {
            v.push(g.shaders.get(g.dev(), &s)?);
        }
    }
    Some(v)
}

impl Gpu {
    /// The graphics pipeline of a key, built now if no prewarm built it.
    pub fn gfx_pipeline(&self, k: &GfxKey, mods: &[Arc<Module>]) -> Option<vk::Pipeline> {
        let key = Key::Gfx(*k);
        if let Some(p) = self.pipes.map.lock().unwrap().get(&key) {
            return Some(*p);
        }
        let t0 = std::time::Instant::now();
        let r = build_gfx(self, k, mods);
        let ns = t0.elapsed().as_nanos() as u64;
        super::perf::add(super::perf::C::PipeNs, ns);
        super::perf::max(super::perf::C::PipeMaxNs, ns);
        match r {
            Ok(p) => {
                super::perf::inc(super::perf::C::PipesGfx);
                self.pipes.created_sync.fetch_add(1, Ordering::Relaxed);
                self.pipes.map.lock().unwrap().insert(key, p);
                self.pipes.note_key(key);
                Some(p)
            }
            Err(e) => {
                crate::log::red(&e);
                None
            }
        }
    }
    pub fn compute_pipeline(&self, m: &Module) -> Option<vk::Pipeline> {
        let key = Key::Compute(m.key);
        if let Some(p) = self.pipes.map.lock().unwrap().get(&key) {
            return Some(*p);
        }
        let t0 = std::time::Instant::now();
        let r = build_compute(self, m);
        let ns = t0.elapsed().as_nanos() as u64;
        super::perf::add(super::perf::C::PipeNs, ns);
        super::perf::max(super::perf::C::PipeMaxNs, ns);
        match r {
            Ok(p) => {
                super::perf::inc(super::perf::C::PipesCompute);
                self.pipes.created_sync.fetch_add(1, Ordering::Relaxed);
                self.pipes.map.lock().unwrap().insert(key, p);
                self.pipes.note_key(key);
                Some(p)
            }
            Err(e) => {
                crate::log::red(&e);
                None
            }
        }
    }
}

/// `low_priority_shader_compiler`: the pipeline builder threads run below normal priority.
fn low_priority() {
    #[cfg(windows)]
    if nxconfig::flag("low_priority_shader_compiler") {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
    }
}

/// The process's private bytes (committed memory the driver's compiler shows up in).
pub fn private_bytes() -> u64 {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS_EX = core::mem::zeroed();
        c.cb = core::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb) != 0 {
            return c.PrivateUsage as u64;
        }
    }
    0
}

#[derive(Debug, Default, Clone)]
pub struct PrewarmReport {
    pub keys: usize,
    pub built: usize,
    pub skipped: usize,
    pub private_growth: u64,
    pub stopped_by_budget: bool,
    pub secs: f64,
}

/// Builds the pipelines of the key file on `threads` background threads until done or over the memory budget, and
/// returns when they have finished.
pub fn prewarm(g: &'static Gpu, threads: usize) -> PrewarmReport {
    let t0 = std::time::Instant::now();
    let Some(dir) = g.pipes.dir.lock().unwrap().clone() else { return PrewarmReport::default() };
    let data = std::fs::read(dir.join("pipeline-keys.bin")).unwrap_or_default();
    let mut keys = Vec::new();
    let mut i = 0;
    while i < data.len() {
        match Key::from_bytes(&data[i..]) {
            Some((k, n)) => {
                keys.push(k);
                i += n;
            }
            None => break,
        }
    }
    keys.sort_by_key(|k| format!("{k:?}"));
    keys.dedup();
    let budget = std::env::var("NXGFX_PREWARM_BUDGET_MB").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(2048) << 20;
    let cap = std::env::var("NXGFX_PREWARM_MAX").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(50_000);
    let base = private_bytes();
    let queue = Arc::new(Mutex::new(keys.clone()));
    let built = Arc::new(AtomicU64::new(0));
    let skipped = Arc::new(AtomicU64::new(0));
    let over = Arc::new(AtomicBool::new(false));
    let mut hs = Vec::new();
    for _ in 0..threads.max(1) {
        let (queue, built, skipped, over) = (queue.clone(), built.clone(), skipped.clone(), over.clone());
        hs.push(std::thread::spawn(move || loop {
            low_priority();
            if g.pipes.stop.load(Ordering::Relaxed) || over.load(Ordering::Relaxed) {
                return;
            }
            let Some(k) = queue.lock().unwrap().pop() else { return };
            if built.load(Ordering::Relaxed) as usize >= cap || private_bytes().saturating_sub(base) > budget {
                over.store(true, Ordering::Relaxed);
                return;
            }
            if g.pipes.map.lock().unwrap().contains_key(&k) {
                continue;
            }
            let p = match k {
                Key::Gfx(gk) => modules_of(g, &gk).and_then(|m| build_gfx(g, &gk, &m).ok()),
                Key::Compute(s) => g.shaders.get(g.dev(), &s).and_then(|m| build_compute(g, &m).ok()),
            };
            match p {
                Some(p) => {
                    g.pipes.map.lock().unwrap().insert(k, p);
                    g.pipes.known.lock().unwrap().insert(k);
                    built.fetch_add(1, Ordering::Relaxed);
                    g.pipes.prewarmed.fetch_add(1, Ordering::Relaxed);
                }
                None => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for h in hs {
        let _ = h.join();
    }
    let r = PrewarmReport {
        keys: keys.len(),
        built: built.load(Ordering::Relaxed) as usize,
        skipped: skipped.load(Ordering::Relaxed) as usize,
        private_growth: private_bytes().saturating_sub(base),
        stopped_by_budget: over.load(Ordering::Relaxed),
        secs: t0.elapsed().as_secs_f64(),
    };
    crate::vklog!(
        "prewarm: {} of {} pipelines in {:.2} s ({} without their shaders), private memory +{} MB{}",
        r.built,
        r.keys,
        r.secs,
        r.skipped,
        r.private_growth >> 20,
        if r.stopped_by_budget { ", stopped at the budget" } else { "" }
    );
    r
}
