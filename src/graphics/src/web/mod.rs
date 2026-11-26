//! The web build's GPU (docs/35 W4, wave WEB-W4): a Vulkan device implemented on WebGPU, in the page's GPU worker.
//!
//! The replay (`vk`) runs unchanged above it: ash loads its function tables from `icd::gipa` instead of a Vulkan
//! loader (`vk::device`'s web arm). Each Vulkan call is answered here; command buffers are kept as records (`cmd`) and
//! resolved at submission (`submit`) into WebGPU-shaped packets in the command ring (`ring`), which
//! `src/web/gpu-worker.js` executes. Shader modules become WGSL (`shaders::wgsl`). Native builds do not compile this
//! module.

pub mod cmd;
pub mod fmt;
pub mod icd;
pub mod obj;
pub mod ring;
pub mod submit;

pub use icd::{entry, surface};

/// A file of the build's data folder fetched from the page's server (`data/<rel>`), for data the web build does not
/// preload (the shader library: `vk::shader`).
pub fn fetch_data(rel: &str) -> Option<Vec<u8>> {
    let url: String = rel
        .split('/')
        .map(|s| s.replace('%', "%25").replace(' ', "%20").replace('#', "%23").replace('?', "%3F"))
        .collect::<Vec<_>>()
        .join("/");
    ring::fetch(&format!("data/{url}"))
}
