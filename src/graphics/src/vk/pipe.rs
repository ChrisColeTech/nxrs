//! Pipelines.
//!
//! Every state NVN changes per draw is dynamic (extended dynamic state 1-3 and dynamic vertex input), so a graphics
//! pipeline's key is only its shaders, its attachment formats and its sample count; every program shares the one
//! layout (`layout`). That keeps the pipeline count close to the program count, which is what bounds the driver's
//! memory: NVIDIA's driver grew to 61 GB when one game's pipelines multiplied per state.
//!
//! A pipeline nothing built yet is linked from four parts (VK_EXT_graphics_pipeline_library, Turbo's NXRT_VK_GPL):
//! vertex input, pre-rasterization shaders, fragment shader, fragment output, each compiled once and kept by what it
//! depends on; a new combination of known parts is only linked (about a millisecond against 50-250 ms for a whole
//! pipeline: Super Mario Bros. Wonder's frames of 60-2,000 ms at every new place). The whole, optimized pipeline is
//! then built on a background thread and takes the linked one's place. `NXGFX_GPL=0`: whole pipelines only.
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
    /// pipeline-library parts by what they depend on (vk::Pipeline::null(): it could not be made)
    parts: Mutex<HashMap<Vec<u8>, vk::Pipeline>>,
    /// linked pipelines an optimized one has replaced (a recorded command buffer may still use them)
    retired: Mutex<Vec<vk::Pipeline>>,
    /// keys whose whole pipeline the background builder is to make
    optimize: Mutex<Option<std::sync::mpsc::Sender<(GfxKey, GfxKey, Vec<Arc<Module>>)>>>,
    pub linked: AtomicU64,
    pub optimized: AtomicU64,
    /// shader parts made ahead (`prebuild`)
    pub prebuilt: AtomicU64,
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
            parts: Mutex::new(HashMap::new()),
            retired: Mutex::new(Vec::new()),
            optimize: Mutex::new(None),
            linked: AtomicU64::new(0),
            optimized: AtomicU64::new(0),
            prebuilt: AtomicU64::new(0),
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
        for p in self.retired.lock().unwrap().drain(..) {
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
/// Pipelines the driver's compiler died on: a pipeline whose compile (whole or linked from parts) was under way when
/// the process ended (`<data>/pipeline-compiling.bin`, written before each compile and removed after) is recorded in
/// `<data>/pipeline-crashed.bin` at the next start and not compiled again: its draws are skipped (a red line says so).
/// Donkey Kong Bananza: the NVIDIA compiler (nvgpucomp64) faults on one vertex + pixel pair, whole or linked from
/// parts; each stage compiles with other partners. One session ends there, the next ones go on without that program.
static INFLIGHT: Mutex<Vec<GfxKey>> = Mutex::new(Vec::new());
static CRASHED: Mutex<Option<std::collections::HashSet<GfxKey>>> = Mutex::new(None);

fn crash_files(g: &Gpu) -> Option<(PathBuf, PathBuf)> {
    let d = g.pipes.dir.lock().unwrap().clone()?;
    Some((d.join("pipeline-compiling.bin"), d.join("pipeline-crashed.bin")))
}

fn read_keys(b: &[u8]) -> Vec<Key> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match Key::from_bytes(&b[i..]) {
            Some((k, n)) => {
                v.push(k);
                i += n;
            }
            None => break,
        }
    }
    v
}

/// Whether `k` crashed the compiler in an earlier session (the crash list is read once, the in-flight file of the
/// last session folded into it then).
fn crashed(g: &Gpu, k: &GfxKey) -> bool {
    let mut c = CRASHED.lock().unwrap();
    if c.is_none() {
        let mut set = std::collections::HashSet::new();
        if let Some((inflight, list)) = crash_files(g) {
            let old: Vec<Key> = read_keys(&std::fs::read(&list).unwrap_or_default());
            let new: Vec<Key> = read_keys(&std::fs::read(&inflight).unwrap_or_default());
            if !new.is_empty() {
                let mut all = std::fs::read(&list).unwrap_or_default();
                for k in &new {
                    all.extend_from_slice(&k.to_bytes());
                }
                let _ = std::fs::write(&list, all);
                crate::log::red(&format!(
                    "{} pipeline(s) were compiling when the last session ended (a compiler crash): not compiled again (a library part: its programs are compiled whole; a whole pipeline: its draws are skipped) ({})",
                    new.len(),
                    list.display()
                ));
            }
            let _ = std::fs::remove_file(&inflight);
            for k in old.into_iter().chain(new) {
                if let Key::Gfx(gk) = k {
                    set.insert(gk);
                }
            }
            if !set.is_empty() {
                crate::vklog!("pipelines not compiled (they crashed the compiler in an earlier session): {}", set.len());
            }
        }
        *c = Some(set);
    }
    c.as_ref().unwrap().contains(k)
}

/// The crash-list key of a program's pipeline-library part: its pre-rasterization shaders (`frag` false) or its pixel
/// shader, marked by an impossible colour count (never a real pipeline's key). DKB's vertex shader d87ccbede95b9f2c
/// crashes the compiler as a pre-rasterization part, with any pixel shader (the shader parts made ahead at
/// ProgramSetShaders died at 2.8 s in every session after the first, lane X2).
fn part_mark(k: &GfxKey, frag: bool) -> GfxKey {
    let mut m = GfxKey { ncolor: if frag { 254 } else { 255 }, ..Default::default() };
    for (i, s) in k.shaders.iter().enumerate() {
        if (i == 1) == frag {
            m.shaders[i] = *s;
        }
    }
    m
}

/// A whole-pipeline compile starts (`on`) or ends: the in-flight file names the ones under way.
fn note_inflight(g: &Gpu, k: &GfxKey, on: bool) {
    let Some((inflight, _)) = crash_files(g) else { return };
    let mut v = INFLIGHT.lock().unwrap();
    if on {
        v.push(*k);
    } else if let Some(i) = v.iter().position(|x| x == k) {
        v.swap_remove(i);
    }
    if v.is_empty() {
        let _ = std::fs::remove_file(&inflight);
    } else {
        let b: Vec<u8> = v.iter().flat_map(|k| Key::Gfx(*k).to_bytes()).collect();
        let _ = std::fs::write(&inflight, b);
    }
}

/// `NXGFX_PIPE_LOG=1`: every pipeline the driver compiles is logged as it starts and ends (the one a compiler crash
/// stopped in is the last started and not ended).
fn pipe_log(what: &str, mods: &[&Module], end: bool) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_PIPE_LOG").as_deref() == Ok("1")) {
        return;
    }
    let keys: Vec<String> = mods.iter().map(|m| m.key.iter().map(|b| format!("{b:02x}")).collect()).collect();
    let th = std::thread::current();
    crate::vklog!("pipeline {what} {} on {}: {}", if end { "made" } else { "compiling" }, th.name().unwrap_or("?"), keys.join(" "));
}

pub fn driver_exclusive(g: &Gpu) -> bool {
    driver_exclusive_d(&g.d)
}
pub fn driver_exclusive_d(d: &super::device::Device) -> bool {
    nxconfig::tristate("driver_exclusive").unwrap_or(d.props.vendor_id == 0x8086)
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
    static NOOPT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let noopt = if *NOOPT.get_or_init(|| std::env::var("NXGFX_PIPE_NOOPT").as_deref() == Ok("1")) { vk::PipelineCreateFlags::DISABLE_OPTIMIZATION } else { vk::PipelineCreateFlags::empty() };
    let mut ci = vk::GraphicsPipelineCreateInfo::default()
        .flags(db_flag(g) | noopt)
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
    let ml: Vec<&Module> = mods.iter().map(|m| &**m).collect();
    pipe_log("graphics", &ml, false);
    note_inflight(g, k, true);
    let p = unsafe { g.dev().create_graphics_pipelines(g.pipes.cache, &[ci], None) };
    note_inflight(g, k, false);
    let p = p.map_err(|(_, e)| format!("vkCreateGraphicsPipelines: {e}"))?;
    pipe_log("graphics", &ml, true);
    super::pcache::note_created(1);
    Ok(p[0])
}

/// Pipelines of the shared layout read descriptor buffers when the device has them (`caps.descbuf`).
fn db_flag(g: &Gpu) -> vk::PipelineCreateFlags {
    if g.d.caps.descbuf {
        vk::PipelineCreateFlags::DESCRIPTOR_BUFFER_EXT
    } else {
        vk::PipelineCreateFlags::empty()
    }
}

/// One pipeline-library part, made once for its key.
fn gpl_part(g: &Gpu, key: Vec<u8>, what: vk::GraphicsPipelineLibraryFlagsEXT, ci: vk::GraphicsPipelineCreateInfo) -> Option<vk::Pipeline> {
    if let Some(&p) = g.pipes.parts.lock().unwrap().get(&key) {
        return (p != vk::Pipeline::null()).then_some(p);
    }
    let mut li = vk::GraphicsPipelineLibraryCreateInfoEXT::default().flags(what);
    // no RETAIN_LINK_TIME_OPTIMIZATION_INFO: nothing links these with LINK_TIME_OPTIMIZATION (the optimized pipeline is
    // built whole from the modules, `build_gfx`), and the retained information made each part bigger and slower to
    // make (NXGFX_PART_LTO=1: retained, as before)
    static LTO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let lto = *LTO.get_or_init(|| std::env::var("NXGFX_PART_LTO").as_deref() == Ok("1"));
    let f = vk::PipelineCreateFlags::LIBRARY_KHR | db_flag(g) | if lto { vk::PipelineCreateFlags::RETAIN_LINK_TIME_OPTIMIZATION_INFO_EXT } else { vk::PipelineCreateFlags::empty() };
    let ci = ci.flags(f).push_next(&mut li);
    let p = match unsafe { g.dev().create_graphics_pipelines(g.pipes.cache, &[ci], None) } {
        Ok(v) => v[0],
        Err((_, e)) => {
            static N: AtomicU64 = AtomicU64::new(0);
            if N.fetch_add(1, Ordering::Relaxed) < 8 {
                crate::log::red(&format!("pipeline part {what:?} could not be made ({e}): whole pipelines for it"));
            }
            vk::Pipeline::null()
        }
    };
    let mut m = g.pipes.parts.lock().unwrap();
    if let Some(&there) = m.get(&key) {
        if there != vk::Pipeline::null() && p != vk::Pipeline::null() {
            unsafe { g.dev().destroy_pipeline(p, None) };
            return Some(there);
        }
    }
    m.insert(key, p);
    (p != vk::Pipeline::null()).then_some(p)
}

/// A graphics pipeline linked from its four parts (the shared layout, no link-time optimization: fast).
fn link_gfx(g: &Gpu, k: &GfxKey, mods: &[Arc<Module>]) -> Option<vk::Pipeline> {
    let _x = driver_exclusive(g).then(|| EXCLUSIVE.lock().unwrap());
    let mesh = mods.iter().any(|m| m.mesh != 0);
    let tess = !mesh && mods.iter().any(|m| m.stage == 3 || m.stage == 4);
    let dyns = dynamic_states(g, tess, mesh);
    let dy = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyns);
    let colors: Vec<vk::Format> = k.colors[..k.ncolor as usize].iter().map(|&f| vk::Format::from_raw(f)).collect();
    let df = vk::Format::from_raw(k.depth);
    let ri = || {
        vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&colors)
            .depth_attachment_format(if depth_has_depth(df) { df } else { vk::Format::UNDEFINED })
            .stencil_attachment_format(if depth_has_stencil(df) { df } else { vk::Format::UNDEFINED })
    };
    let mask = [u32::MAX];
    let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples_flag(k.samples)).sample_mask(&mask);
    let flags = [tess as u8, mesh as u8];
    let mut parts = Vec::with_capacity(4);
    // vertex input: the input assembly (the vertex layout and topology are dynamic)
    if !mesh {
        let vi = vk::PipelineVertexInputStateCreateInfo::default();
        let ia = vk::PipelineInputAssemblyStateCreateInfo::default().topology(if tess { vk::PrimitiveTopology::PATCH_LIST } else { vk::PrimitiveTopology::TRIANGLE_LIST });
        let ci = vk::GraphicsPipelineCreateInfo::default().vertex_input_state(&vi).input_assembly_state(&ia).dynamic_state(&dy);
        parts.push(gpl_part(g, vec![1, flags[0], flags[1]], vk::GraphicsPipelineLibraryFlagsEXT::VERTEX_INPUT_INTERFACE, ci)?);
    }
    // the shaders' parts: pre-rasterization and fragment shader (made ahead when the game sets a program: `prebuild`)
    let [pre, frag] = shader_parts(g, k, mods)?;
    parts.push(pre);
    parts.push(frag);
    // fragment output: the targets' formats, blending, multisampling
    {
        let mut key = vec![4, flags[0], flags[1], k.samples, k.ncolor];
        for c in &k.colors[..k.ncolor as usize] {
            key.extend_from_slice(&c.to_le_bytes());
        }
        key.extend_from_slice(&k.depth.to_le_bytes());
        let att: Vec<vk::PipelineColorBlendAttachmentState> =
            (0..k.ncolor).map(|_| vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)).collect();
        let cb = vk::PipelineColorBlendStateCreateInfo::default().attachments(&att);
        let mut r = ri();
        let ci = vk::GraphicsPipelineCreateInfo::default().multisample_state(&ms).color_blend_state(&cb).dynamic_state(&dy).push_next(&mut r);
        parts.push(gpl_part(g, key, vk::GraphicsPipelineLibraryFlagsEXT::FRAGMENT_OUTPUT_INTERFACE, ci)?);
    }
    let mut li = vk::PipelineLibraryCreateInfoKHR::default().libraries(&parts);
    let ci = vk::GraphicsPipelineCreateInfo::default().flags(db_flag(g)).layout(g.layout.pipeline).push_next(&mut li);
    match unsafe { g.dev().create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None) } {
        Ok(v) => Some(v[0]),
        Err((_, e)) => {
            static N: AtomicU64 = AtomicU64::new(0);
            if N.fetch_add(1, Ordering::Relaxed) < 8 {
                crate::log::red(&format!("a pipeline could not be linked from its parts ({e}): made whole"));
            }
            None
        }
    }
}

/// The pre-rasterization part (every stage but the fragment shader, viewport, rasterization, tessellation) and the
/// fragment shader part (its stage, depth / stencil, multisampling) of a pipeline: what costs the shader compiles.
fn shader_parts(g: &Gpu, k: &GfxKey, mods: &[Arc<Module>]) -> Option<[vk::Pipeline; 2]> {
    // a pre-rasterization part needs its vertex (or mesh) shader: a program made ahead whose vertex shader is not
    // translated yet had none, and an empty pre-rasterization library is invalid (llvmpipe crashed on it: lane L)
    if !mods.iter().any(|m| m.stage != 1) {
        return None;
    }
    let mesh = mods.iter().any(|m| m.mesh != 0);
    let tess = !mesh && mods.iter().any(|m| m.stage == 3 || m.stage == 4);
    let dyns = dynamic_states(g, tess, mesh);
    let dy = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dyns);
    let df = vk::Format::from_raw(k.depth);
    let colors: Vec<vk::Format> = k.colors[..k.ncolor as usize].iter().map(|&f| vk::Format::from_raw(f)).collect();
    let ri = || {
        vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&colors)
            .depth_attachment_format(if depth_has_depth(df) { df } else { vk::Format::UNDEFINED })
            .stencil_attachment_format(if depth_has_stencil(df) { df } else { vk::Format::UNDEFINED })
    };
    let mask = [u32::MAX];
    let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples_flag(k.samples)).sample_mask(&mask);
    let flags = [tess as u8, mesh as u8];
    let pre = {
        let stages: Vec<vk::PipelineShaderStageCreateInfo> = mods
            .iter()
            .filter(|m| m.stage != 1)
            .map(|m| vk::PipelineShaderStageCreateInfo::default().stage(m.vk_stage()).module(m.module).name(&m.entry))
            .collect();
        let mut key = vec![2, flags[0], flags[1]];
        for (i, s) in k.shaders.iter().enumerate() {
            if i != 1 {
                key.extend_from_slice(s);
            }
        }
        let vp = vk::PipelineViewportStateCreateInfo::default();
        let rs = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).line_width(1.0);
        let ts = vk::PipelineTessellationStateCreateInfo::default().patch_control_points(3);
        let mut r = ri();
        let mut ci = vk::GraphicsPipelineCreateInfo::default().stages(&stages).viewport_state(&vp).rasterization_state(&rs).dynamic_state(&dy).layout(g.layout.pipeline).push_next(&mut r);
        if tess {
            ci = ci.tessellation_state(&ts);
        }
        // a part the compiler died on in an earlier session is not made again: the program is compiled whole
        let mark = part_mark(k, false);
        if crashed(g, &mark) {
            return None;
        }
        note_inflight(g, &mark, true);
        let p = gpl_part(g, key, vk::GraphicsPipelineLibraryFlagsEXT::PRE_RASTERIZATION_SHADERS, ci);
        note_inflight(g, &mark, false);
        p?
    };
    let frag = {
        let stages: Vec<vk::PipelineShaderStageCreateInfo> = mods
            .iter()
            .filter(|m| m.stage == 1)
            .map(|m| vk::PipelineShaderStageCreateInfo::default().stage(m.vk_stage()).module(m.module).name(&m.entry))
            .collect();
        let mut key = vec![3, flags[0], flags[1], k.samples];
        key.extend_from_slice(&k.shaders[1]);
        key.extend_from_slice(&k.depth.to_le_bytes());
        let ds = vk::PipelineDepthStencilStateCreateInfo::default();
        let mut r = ri();
        let ci = vk::GraphicsPipelineCreateInfo::default().stages(&stages).multisample_state(&ms).depth_stencil_state(&ds).dynamic_state(&dy).layout(g.layout.pipeline).push_next(&mut r);
        let mark = part_mark(k, true);
        if crashed(g, &mark) {
            return None;
        }
        note_inflight(g, &mark, true);
        let p = gpl_part(g, key, vk::GraphicsPipelineLibraryFlagsEXT::FRAGMENT_SHADER, ci);
        note_inflight(g, &mark, false);
        p?
    };
    Some([pre, frag])
}

/// The (sample count, depth format) pairs pipelines were made for lately (the fragment parts made ahead use them).
static TARGETS: Mutex<Vec<(u8, i32)>> = Mutex::new(Vec::new());

fn note_targets(k: &GfxKey) {
    let mut t = TARGETS.lock().unwrap();
    if let Some(i) = t.iter().position(|&x| x == (k.samples, k.depth)) {
        let x = t.remove(i);
        t.insert(0, x);
    } else {
        t.insert(0, (k.samples, k.depth));
        t.truncate(4);
    }
}

/// nvnProgramSetShaders: the program's shader parts are made ahead on a background thread (a pipeline of it is then
/// only linked at its first draw: a new place in a game no longer waits for its shaders to compile there). The
/// fragment part for the depth formats and sample counts pipelines were made for lately.
pub fn prebuild(g: &'static Gpu, keys: Vec<ShaderKey>) {
    // shader_thread=0: nothing made on threads of its own (every pipeline at its first draw)
    if !g.d.caps.gpl || keys.is_empty() || !nxconfig::flag("shader_thread") {
        return;
    }
    static TX: Mutex<Option<std::sync::mpsc::Sender<Vec<ShaderKey>>>> = Mutex::new(None);
    let mut tx = TX.lock().unwrap();
    if tx.is_none() {
        let (s, r) = std::sync::mpsc::channel::<Vec<ShaderKey>>();
        let r = Arc::new(Mutex::new(r));
        // two (four on the 16-core test machine made Super Mario Bros. Wonder's first run worse: multi-second frames)
        for i in 0..2 {
            let r = r.clone();
            let _ = std::thread::Builder::new().name(format!("vk shader parts {i}")).spawn(move || {
                low_priority();
                // the driver's memory bounds it: the private memory the parts' own jobs grew it by (the game's loading
                // around them no longer counts), at most parts_budget()
                let budget = parts_budget();
                loop {
                    let job = r.lock().unwrap().recv();
                    let Ok(keys) = job else { return };
                    if g.pipes.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if PARTS_GROWTH.load(Ordering::Relaxed) > budget {
                        static SAID: AtomicBool = AtomicBool::new(false);
                        if !SAID.swap(true, Ordering::Relaxed) {
                            crate::vklog!("shader parts are no longer made ahead: they grew private memory by {} MB (NXGFX_PREWARM_BUDGET_MB)", budget >> 20);
                        }
                        continue;
                    }
                    let _grew = Grew::start();
                    let _x = driver_exclusive(g).then(|| EXCLUSIVE.lock().unwrap());
                    let mut k = GfxKey::default();
                    let mut mods = Vec::new();
                    for key in keys {
                        // a shader not translated yet is made when its draw comes (the live translator)
                        let Some(m) = g.shaders.get(g.dev(), &key) else { continue };
                        if (m.stage as usize) < 5 {
                            k.shaders[m.stage as usize] = m.key;
                            mods.push(m);
                        }
                    }
                    if mods.is_empty() {
                        continue;
                    }
                    link_io(g, &mut k, &mut mods);
                    let targets = TARGETS.lock().unwrap().clone();
                    for (samples, depth) in if targets.is_empty() { vec![(1, 0)] } else { targets } {
                        k.samples = samples;
                        k.depth = depth;
                        if shader_parts(g, &k, &mods).is_some() {
                            g.pipes.prebuilt.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
        *tx = Some(s);
    }
    let _ = tx.as_ref().unwrap().send(keys);
}

/// Private memory the shader-part jobs grew the process by (each job's own growth, summed).
static PARTS_GROWTH: AtomicU64 = AtomicU64::new(0);

static PARTS_JOBS: AtomicU64 = AtomicU64::new(0);

/// Adds a job's growth to PARTS_GROWTH when it ends (shared among the jobs that ran beside it: each one's window sees
/// the others' growth too).
struct Grew(u64, u64);
impl Grew {
    fn start() -> Grew {
        let n = PARTS_JOBS.fetch_add(1, Ordering::Relaxed) + 1;
        Grew(private_bytes(), n)
    }
}
impl Drop for Grew {
    fn drop(&mut self) {
        let n = PARTS_JOBS.fetch_sub(1, Ordering::Relaxed).max(self.1).max(1);
        PARTS_GROWTH.fetch_add(private_bytes().saturating_sub(self.0) / n, Ordering::Relaxed);
    }
}

/// The shader parts' memory budget: NXGFX_PREWARM_BUDGET_MB, default 2 GB of the parts' own growth. (6 GB with 4
/// threads, `-G4-wonder-first2`: 19,441 parts by 102 s, still a 2.3 s frame at the level entry and many multi-second
/// frames: worse than 2 GB of growth counted with the game's loading, `-G4-wonder-first`.)
fn parts_budget() -> u64 {
    std::env::var("NXGFX_PREWARM_BUDGET_MB").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(2048) << 20
}

/// A pipeline replaced (its optimized build took its place, or another thread made the same one first): destroyed once
/// the work recorded with it has run. They were kept to the end of the session: the driver's memory for each linked
/// pipeline stayed (Mario Kart World's private memory grew with every pipeline made in play).
fn retire(g: &Gpu, p: vk::Pipeline) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_RETIRE_KEEP").as_deref() != Ok("1")) {
        g.pipes.retired.lock().unwrap().push(p);
        return;
    }
    g.q.defer_after_use(Box::new(move |g: &Gpu| unsafe { g.dev().destroy_pipeline(p, None) }));
}

/// The background builders of the whole pipelines that replace linked ones (below normal priority).
/// `at`: the key the pipeline is found by; `k`: the one it is built from (its linked variant, `link_io`).
fn queue_optimize(g: &'static Gpu, at: GfxKey, k: GfxKey, mods: Vec<Arc<Module>>) {
    let mut tx = g.pipes.optimize.lock().unwrap();
    if tx.is_none() {
        let (s, r) = std::sync::mpsc::channel::<(GfxKey, GfxKey, Vec<Arc<Module>>)>();
        let r = Arc::new(Mutex::new(r));
        for i in 0..2 {
            let r = r.clone();
            let _ = std::thread::Builder::new().name(format!("vk pipeline optimizer {i}")).spawn(move || {
                low_priority();
                loop {
                    let job = r.lock().unwrap().recv();
                    let Ok((at, k, mods)) = job else { return };
                    if g.pipes.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if crashed(g, &k) {
                        continue;
                    }
                    if let Ok(p) = build_gfx(g, &k, &mods) {
                        let old = g.pipes.map.lock().unwrap().insert(Key::Gfx(at), p);
                        if let Some(o) = old {
                            retire(g, o);
                        }
                        g.pipes.optimized.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
        *tx = Some(s);
    }
    let _ = tx.as_ref().unwrap().send((at, k, mods));
}

fn build_compute(g: &Gpu, m: &Module) -> Result<vk::Pipeline, String> {
    let _x = driver_exclusive(g).then(|| EXCLUSIVE.lock().unwrap());
    let st = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::COMPUTE).module(m.module).name(&m.entry);
    let ci = vk::ComputePipelineCreateInfo::default().flags(db_flag(g)).stage(st).layout(g.layout.pipeline);
    pipe_log("compute", &[m], false);
    let p = unsafe { g.dev().create_compute_pipelines(g.pipes.cache, &[ci], None) }.map_err(|(_, e)| format!("vkCreateComputePipelines: {e}"))?;
    pipe_log("compute", &[m], true);
    super::pcache::note_created(1);
    Ok(p[0])
}

/// 1 an unsigned integer colour format, 2 a signed one, 0 otherwise.
fn int_class(f: vk::Format) -> u8 {
    let n = format!("{f:?}");
    if n.contains("UINT") {
        1
    } else if n.contains("SINT") {
        2
    } else {
        0
    }
}

/// A program's stage before the pixel shader with outputs for the varyings the pixel shader reads and it never writes
/// (undefined on the console, an interface error on Vulkan): the variant module in `mods` and its key in `k`.
/// `NXGFX_LINK_IO=0` off.
fn link_io(g: &Gpu, k: &mut GfxKey, mods: &mut [Arc<Module>]) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("NXGFX_LINK_IO").as_deref() != Ok("0")) || mods.iter().any(|m| m.mesh != 0) {
        return;
    }
    let Some(ps) = mods.iter().position(|m| m.stage == 1) else { return };
    // integer colour targets: the pixel shader's float outputs there retyped (Turbo's integer output variant)
    let outs = shaders::link::output_kinds(&mods[ps].words);
    let mut kinds = [0u8; 8];
    for (l, kind) in kinds.iter_mut().enumerate().take(k.ncolor as usize) {
        let want = int_class(vk::Format::from_raw(k.colors[l]));
        if want != 0 && outs[l] == Some(0) {
            *kind = want;
        }
    }
    if kinds.iter().any(|&x| x != 0) {
        if let Some(v) = g.shaders.with_int_outputs(g.dev(), &mods[ps], &kinds) {
            k.shaders[1] = v.key;
            mods[ps] = v;
        }
    }
    let Some(prev) = [2u32, 4, 0].iter().find_map(|&s| mods.iter().position(|m| m.stage == s)) else { return };
    let inputs = shaders::link::io_locations(&mods[ps].words, false);
    if inputs.is_empty() {
        return;
    }
    let add = shaders::link::missing(&shaders::link::io_locations(&mods[prev].words, true), &inputs);
    if add.is_empty() {
        return;
    }
    if let Some(v) = g.shaders.with_outputs(g.dev(), &mods[prev], &add) {
        k.shaders[mods[prev].stage as usize] = v.key;
        mods[prev] = v;
    }
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
        let at = *k;
        let (k, mods) = {
            let (mut k2, mut m2) = (*k, mods.to_vec());
            link_io(self, &mut k2, &mut m2);
            (k2, m2)
        };
        let (k, mods) = (&k, &mods[..]);
        // it crashed the driver's compiler in an earlier session: not compiled again, its draws are skipped
        if crashed(self, k) {
            crate::log::red_once("a pipeline that crashed the driver's compiler in an earlier session is not compiled again: its draws are skipped (pipeline-crashed.bin)");
            return None;
        }
        let t0 = std::time::Instant::now();
        note_targets(k);
        let linked = if self.d.caps.gpl {
            note_inflight(self, k, true);
            let l = link_gfx(self, k, mods);
            note_inflight(self, k, false);
            l
        } else {
            None
        };
        let r = match linked {
            Some(p) => Ok(p),
            None => build_gfx(self, k, mods),
        };
        let ns = t0.elapsed().as_nanos() as u64;
        super::perf::add(super::perf::C::PipeNs, ns);
        super::perf::max(super::perf::C::PipeMaxNs, ns);
        match r {
            Ok(p) => {
                super::perf::inc(super::perf::C::PipesGfx);
                self.pipes.created_sync.fetch_add(1, Ordering::Relaxed);
                {
                    let mut m = self.pipes.map.lock().unwrap();
                    // made meanwhile by another thread (the prewarm): that one stays
                    if let Some(&there) = m.get(&key) {
                        drop(m);
                        retire(self, p);
                        return Some(there);
                    }
                    m.insert(key, p);
                }
                self.pipes.note_key(key);
                if linked.is_some() {
                    self.pipes.linked.fetch_add(1, Ordering::Relaxed);
                    // shader_thread=0: the linked pipeline stays (no optimizer thread)
                    if let Some(g) = Gpu::try_get().filter(|_| nxconfig::flag("shader_thread")) {
                        queue_optimize(g, at, *k, mods.to_vec());
                    }
                }
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
    // Linux: a nice value of its own for the calling thread (Linux threads carry their own niceness)
    #[cfg(target_os = "linux")]
    if nxconfig::flag("low_priority_shader_compiler") {
        unsafe { libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 5) };
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
    #[cfg(target_os = "linux")]
    {
        let (rss, swap) = nxrt::linux::process_memory();
        return rss + swap;
    }
    #[allow(unreachable_code)]
    0
}

/// A first run (the pipeline key file holds fewer than `NXGFX_PROFILE_PREWARM_MIN` keys, 64): the shader parts of the
/// programs the profile's tables name (`<shaders>/profile/`, written by the build: nxnative's prewarm seed
/// `pso_manifest.tsv` in first-use order, then the vertex / pixel pairs of `shader_pairs.tsv`) are made ahead on the
/// shader-part threads (`prebuild`, within its memory budget), so a level's first entry links its pipelines from
/// parts instead of compiling them at the first draw. `NXGFX_PROFILE_PREWARM=0` off. Returns the programs queued.
pub fn prewarm_profile(g: &'static Gpu, keys_known: usize) -> usize {
    if std::env::var("NXGFX_PROFILE_PREWARM").as_deref() == Ok("0") || !g.d.caps.gpl || !nxconfig::flag("shader_thread") {
        return 0;
    }
    let min = std::env::var("NXGFX_PROFILE_PREWARM_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(64usize);
    if keys_known >= min {
        return 0;
    }
    let Some(dir) = g.shaders.dir().map(|d| d.join("profile")) else { return 0 };
    let key = |h: &str| -> Option<ShaderKey> {
        let h = h.split('_').next()?;
        if h.len() != 16 {
            return None;
        }
        let mut k = [0u8; 8];
        for i in 0..8 {
            k[i] = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(k)
    };
    let pairs: Vec<(ShaderKey, ShaderKey)> = std::fs::read_to_string(dir.join("shader_pairs.tsv"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split(char::from(9u8));
            Some((key(f.next()?)?, key(f.next()?)?))
        })
        .collect();
    // the seed's shaders in first-use order: their pairs first
    let mut order: Vec<ShaderKey> = Vec::new();
    for l in std::fs::read_to_string(dir.join("pso_manifest.tsv")).unwrap_or_default().lines() {
        let f: Vec<&str> = l.split(char::from(9u8)).collect();
        if let ["S", k, ..] = f[..] {
            if let Some(k) = key(k) {
                order.push(k);
            }
        }
    }
    let mut queued = std::collections::HashSet::new();
    let mut programs: Vec<Vec<ShaderKey>> = Vec::new();
    let pos: std::collections::HashMap<ShaderKey, usize> = order.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let mut sorted = pairs;
    sorted.sort_by_key(|(v, p)| pos.get(v).copied().unwrap_or(usize::MAX).min(pos.get(p).copied().unwrap_or(usize::MAX)));
    for (v, p) in sorted {
        if queued.insert((v, p)) {
            programs.push(vec![v, p]);
        }
    }
    let n = programs.len();
    if n == 0 {
        return 0;
    }
    crate::vklog!("profile prewarm: a first run ({keys_known} pipeline keys known): the shader parts of {n} programs from {} made ahead ({} seeded shaders first)", dir.display(), order.len());
    for p in programs {
        prebuild(g, p);
    }
    n
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
                Key::Gfx(gk) if crashed(g, &gk) => None,
                Key::Gfx(gk) => modules_of(g, &gk).and_then(|mut m| {
                    let mut k2 = gk;
                    link_io(g, &mut k2, &mut m);
                    build_gfx(g, &k2, &m).ok()
                }),
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
