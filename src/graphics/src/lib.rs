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

pub mod log;
pub mod nvn;
pub mod tex;
pub mod vk;

/// Where the game's translated shaders are (`<key>.spv` + `<key>.json`, lane C's output).
pub fn set_shader_dir(p: std::path::PathBuf) {
    if let Ok(g) = vk::Gpu::get() {
        g.shaders.set_dir(p);
    }
}

/// Where the pipeline key file and pipeline cache live (the game's `_data`).
pub fn set_cache_dir(p: std::path::PathBuf) {
    if let Ok(g) = vk::Gpu::get() {
        g.pipes.set_dir(p);
    }
}

/// Builds the pipelines earlier runs used, on `threads` threads, within the memory budget.
pub fn prewarm(threads: usize) -> vk::pipe::PrewarmReport {
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
