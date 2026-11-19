//! nxgfx: NVN on Vulkan.
//!
//! - `nvn`: the record layer, the `nvn*` entry points on the game's own objects, commands recorded into the game's
//!   command memory (`nvn::procs()` lists the entry points as bridge host functions).
//! - `vk`: the back end that replays them on a discrete GPU: memory pool placement, images, the shared pipeline
//!   layout, pipelines, submissions and their CPU-visible effects, present and pacing.
//! - `tex`: NVN's texture layouts, formats and ASTC on the GPU.
//!
//! The interfaces other parts of the engine use are listed in docs/INTERFACES.md ("graphics").
#![allow(clippy::missing_safety_doc, clippy::too_many_arguments)]

pub mod host_window;
pub mod log;
pub mod nvn;
pub mod tex;
pub mod vk;

/// Where the game's translated shaders are (`<key>.spv` + `<key>.json`, lane C's output).
pub fn set_shader_dir(p: std::path::PathBuf) {
    vk::live::set_dir(p.join("live"));
    if let Ok(g) = vk::Gpu::get() {
        g.shaders.set_dir(p);
    }
}

/// Where the pipeline key file and pipeline cache live (the game's `_data`).
pub fn set_cache_dir(p: std::path::PathBuf) {
    if let Ok(g) = vk::Gpu::get() {
        vk::pcache::attach(g, &p);
        g.pipes.set_dir(p);
    }
}

/// Builds the pipelines earlier runs used, on `threads` threads, within the memory budget. `shader_thread=0` builds
/// none ahead (every pipeline on the replay's thread, at its first draw).
pub fn prewarm(threads: usize) -> vk::pipe::PrewarmReport {
    if !nxconfig::flag("shader_thread") {
        crate::vklog!("shader_thread=0: no pipelines built ahead on threads of their own");
        return Default::default();
    }
    match vk::Gpu::get() {
        Ok(g) => vk::pipe::prewarm(g, threads),
        Err(_) => Default::default(),
    }
}

/// Writes the pipeline cache (at exit, or when the game goes to the background).
pub fn save_caches() {
    if let Some(g) = vk::Gpu::try_get() {
        g.pipes.save(g);
    }
}

pub use vk::present::{close_window, set_window};

/// The graphics layer's control commands (served by the front end's control pipe, `nxconfig::ctl`): `shot [path]`
/// (a screenshot of the next present), `a2c 0|1` (alpha to coverage live), `pcache` (save the pipeline cache now),
/// `gfx` (the replay's counters).
pub fn register_controls() {
    if std::env::var("NXGFX_A2C").as_deref() == Ok("0") {
        vk::exec::A2C.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    nxconfig::ctl::register("shot", "[path]: a screenshot of the next present (PNG; default captures/shot-<n>.png)", |a| {
        vk::shot::request(a.first().copied().unwrap_or(""));
        "ok".into()
    });
    nxconfig::ctl::register("a2c", "0|1: alpha to coverage off / as the game sets it (live)", |a| {
        let on = a.first().is_none_or(|v| *v != "0");
        vk::exec::A2C.store(on, std::sync::atomic::Ordering::Relaxed);
        format!("a2c {}", on as u8)
    });
    nxconfig::ctl::register("pcache", ": write the pipeline cache now", |_| match vk::Gpu::try_get() {
        Some(g) => format!("{}", vk::pcache::save_now(g)),
        None => "no device".into(),
    });
    nxconfig::ctl::register("passdump", ": the colour targets of the next frame's passes as PNGs (captures/)", |_| {
        vk::exec::passdump::request_now();
        "ok".into()
    });
    nxconfig::ctl::register("gfx", ": the replay's counters", |_| vk::perf::snapshot().iter().map(|(n, v)| format!("{n} {v}")).collect::<Vec<_>>().join("
"));
}
