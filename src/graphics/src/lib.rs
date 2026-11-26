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

#[cfg(not(target_os = "emscripten"))]
pub mod host_window;
pub mod log;
pub mod nvn;
pub mod tex;
pub mod vk;
#[cfg(target_os = "emscripten")]
pub mod web;

/// Where the game's translated shaders are (`<key>.spv` + `<key>.json`, lane C's output).
pub fn set_shader_dir(p: std::path::PathBuf) {
    vk::live::set_dir(p.join("live"));
    if let Ok(g) = vk::Gpu::get() {
        g.shaders.set_dir(p);
    }
}

/// The profile's `shader_key_skip` (control bytes shader keys hash as zeros), set before any program is keyed.
pub fn set_shader_key_skip(skip: Option<(u32, u32)>) {
    if let Some((o, l)) = skip {
        crate::nvnlog!(
            "shader keys hash control bytes {o:#x}+{l:#x} as zeros (profile shader_key_skip)"
        );
    }
    shaders::nvn::set_key_skip(skip);
}

/// F5's graphics part: the render targets that exist now, as text, and a pass dump of
/// the next frame (each pass's colour targets as PNGs in `captures`, `vk::exec::passdump`).
pub fn diag_dump() -> String {
    let Some(g) = vk::Gpu::try_get() else {
        return String::new();
    };
    vk::exec::passdump::request_now();
    let rts = g.images.lock().unwrap().render_targets();
    let mut s = format!(
        "# render targets ({}; the next frame's passes are dumped as PNGs into captures)\n",
        rts.len()
    );
    for l in rts {
        s += &l;
        s.push('\n');
    }
    s
}

/// Where the pipeline key file and pipeline cache live (the game's `_data`).
pub fn set_cache_dir(p: std::path::PathBuf) {
    tex::astc_cache::set_dir(&p);
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
        Ok(g) => {
            let r = vk::pipe::prewarm(g, threads);
            vk::pipe::prewarm_profile(g, r.keys);
            r
        }
        Err(_) => Default::default(),
    }
}

/// Writes the pipeline cache (at exit, or when the game goes to the background).
pub fn save_caches() {
    if let Some(g) = vk::Gpu::try_get() {
        g.pipes.save(g);
    }
}

pub use vk::present::{close_window, set_window, set_window_raw};
#[cfg(target_os = "linux")]
pub use vk::present::{set_window_unix, WsKind};

/// The graphics layer's control commands (served by the front end's control pipe, `nxconfig::ctl`): `shot [path]`
/// (a screenshot of the next present), `a2c 0|1` (alpha to coverage live), `pcache` (save the pipeline cache now),
/// `gfx` (the replay's counters).
pub fn register_controls() {
    if std::env::var("NXGFX_A2C").as_deref() == Ok("0") {
        vk::exec::A2C.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    nxconfig::ctl::register(
        "shot",
        "[path]: a screenshot of the next present (PNG; default captures/shot-<n>.png)",
        |a| {
            vk::shot::request(a.first().copied().unwrap_or(""));
            "ok".into()
        },
    );
    nxconfig::ctl::register(
        "a2c",
        "0|1: alpha to coverage off / as the game sets it (live)",
        |a| {
            let on = a.first().is_none_or(|v| *v != "0");
            vk::exec::A2C.store(on, std::sync::atomic::Ordering::Relaxed);
            format!("a2c {}", on as u8)
        },
    );
    if std::env::var("NXGFX_ALPHA_TEST").as_deref() == Ok("0") {
        vk::exec::A_TEST.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    nxconfig::ctl::register(
        "alpha_test",
        "0|1: NVN's alpha test off / applied in the pixel shader (live)",
        |a| {
            let on = a.first().is_none_or(|v| *v != "0");
            vk::exec::A_TEST.store(on, std::sync::atomic::Ordering::Relaxed);
            format!("alpha_test {}", on as u8)
        },
    );
    nxconfig::ctl::register(
        "image_noflag",
        "0|1: storage use for textures used as images without the IMAGE flag (new textures)",
        |a| {
            let on = a.first().is_none_or(|v| *v != "0");
            std::env::set_var("NXGFX_IMAGE_NOFLAG", if on { "1" } else { "0" });
            format!("image_noflag {}", on as u8)
        },
    );
    nxconfig::ctl::register(
        "pcache",
        ": write the pipeline cache now",
        |_| match vk::Gpu::try_get() {
            Some(g) => format!("{}", vk::pcache::save_now(g)),
            None => "no device".into(),
        },
    );
    nxconfig::ctl::register(
        "passdump",
        ": the colour targets of the next frame's passes as PNGs (captures/)",
        |_| {
            vk::exec::passdump::request_now();
            "ok".into()
        },
    );
    nxconfig::ctl::register("gfx", ": the replay's counters", |_| {
        vk::perf::snapshot()
            .iter()
            .map(|(n, v)| format!("{n} {v}"))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    });
}
