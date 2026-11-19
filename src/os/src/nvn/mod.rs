//! NVN as the os layer binds it: `nvnBootstrapLoader` and `nvnDeviceGetProcAddress` hand out host functions by
//! name, the graphics layer's (NVN on Vulkan, `bind_graphics`: every game run is on the real device, rule 1.1.6),
//! or a logging stub for a name graphics does not implement, which counts the call, records an `UNIMPL Nvn` gap on
//! its first call and answers 0. Every proc's calls are counted; `report` prints the table. The first
//! `nvnQueueSubmitCommands` is logged with the time since boot. `nv.rs` is the driver glue under NVN.

pub mod nv;

use nxrt::cpu::Cpu;
use nxrt::hostfn::HostFn;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static GRAPHICS: Mutex<Vec<(String, HostFn)>> = Mutex::new(Vec::new());
static CALLS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
static TOTAL: AtomicU64 = AtomicU64::new(0);
static SUBMITS: AtomicU64 = AtomicU64::new(0);
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static FIRST_SUBMIT_NS: AtomicU64 = AtomicU64::new(0);

/// The graphics layer's NVN procs (by NVN name), registered before the boot: they replace the os layer's.
pub fn register(name: &str, f: HostFn) {
    let mut g = GRAPHICS.lock().unwrap();
    g.retain(|(n, _)| n != name);
    g.push((name.to_string(), f));
}

pub fn calls() -> u64 {
    TOTAL.load(Ordering::Relaxed)
}
/// Frames presented so far (vi's frame number).
pub fn presents() -> u64 {
    PRESENTS.load(Ordering::Relaxed)
}

/// Called by whichever `nvnQueuePresentTexture` runs.
pub fn note_present(window: u64, index: u32) {
    if PRESENTS.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::log!("nvn: first present (window {window:#x}, texture {index}), {:.3} s after start", crate::log::secs());
    }
}

pub fn submits() -> u64 {
    SUBMITS.load(Ordering::Relaxed)
}
/// Seconds since the first queue submit, if there was one.
pub fn first_submit_age() -> Option<f64> {
    let t = FIRST_SUBMIT_NS.load(Ordering::Relaxed);
    (t != 0).then(|| (nxrt::rt::now_ns() - t) as f64 / 1e9)
}

pub(crate) fn count(name: &str) {
    TOTAL.fetch_add(1, Ordering::Relaxed);
    let mut g = CALLS.lock().unwrap();
    *g.get_or_insert_with(HashMap::new).entry(name.to_string()).or_insert(0) += 1;
}

/// Called by whichever `nvnQueueSubmitCommands` runs (the null device's, or the graphics layer's through
/// `note_submit`).
pub fn note_submit(queue: u64, n: u64) {
    if SUBMITS.fetch_add(1, Ordering::Relaxed) == 0 {
        FIRST_SUBMIT_NS.store(nxrt::rt::now_ns(), Ordering::Relaxed);
        crate::log!("FIRST NVN QUEUE SUBMIT: queue {queue:#x}, {n} command handle(s), {:.3} s after start", crate::log::secs());
        report();
        crate::log::report_gaps("first submit");
    }
}

/// The per-proc call table.
pub fn report() {
    let g = CALLS.lock().unwrap();
    let Some(m) = g.as_ref() else { return };
    let mut v: Vec<(&String, &u64)> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    crate::log!("NVN calls: {} procs, {} calls", v.len(), calls());
    for (n, c) in v {
        let have = GRAPHICS.lock().unwrap().iter().any(|(g, _)| g == n);
        crate::log!("  nvn {c:8} {n}{}", if have { "" } else { "  (stub)" });
    }
}

/// The logging stub of a proc nothing implements.
unsafe fn stub(c: *mut Cpu) {
    let name = nxrt::hostfn::name_of((*c).pc).unwrap_or_default();
    let name = name.strip_prefix("nvn:").unwrap_or(&name).to_string();
    count(&name);
    if !recorded(&name) {
        crate::log::gap(crate::log::Gap::Nvn, &name, &format!("x0={:#x} x1={:#x} x2={:#x}, answered 0", (*c).x[0], (*c).x[1], (*c).x[2]));
    }
    (*c).x[0] = 0;
}

/// Calls whose whole effect is GPU state or GPU work: a null device records nothing for them, and nothing the
/// game reads back depends on them (command recording, the state objects, debug labels). They are counted, not
/// listed as gaps.
pub fn recorded(name: &str) -> bool {
    const STATE: [&str; 8] = [
        "nvnBlendState",
        "nvnColorState",
        "nvnChannelMaskState",
        "nvnDepthStencilState",
        "nvnMultisampleState",
        "nvnPolygonState",
        "nvnVertexAttribState",
        "nvnVertexStreamState",
    ];
    let set_or_default = |n: &str| n.contains("Set") || n.ends_with("SetDefaults");
    (name.starts_with("nvnCommandBuffer") && !name.contains("Get"))
        || STATE.iter().any(|p| name.starts_with(p) && set_or_default(&name[p.len()..]))
        || name.ends_with("SetDebugLabel")
        || name.starts_with("nvnTextureView")
        || name.starts_with("nvnSamplerBuilderSet")
}

/// The guest address of the proc `name` (null for a name that is not NVN's).
pub fn proc_address(name: &str) -> u64 {
    if !name.starts_with("nvn") {
        crate::log!("[RED] NVN proc lookup of {name:?}");
        return 0;
    }
    let key = format!("nvn:{name}");
    if let Some((_, f)) = GRAPHICS.lock().unwrap().iter().find(|(n, _)| n == name) {
        if std::env::var("NXOS_NVN_BT").is_ok_and(|v| v == name) {
            let _ = BT_TARGET.set(*f);
            return crate::hle::addr(&format!("{key}#bt"), bt_proc);
        }
        return crate::hle::addr(&key, *f);
    }
    crate::hle::addr(&key, stub)
}

static BT_TARGET: std::sync::OnceLock<HostFn> = std::sync::OnceLock::new();

/// `NXOS_NVN_BT=<proc>` (Turbo's SWR_NVN_INTERVAL_BT): the first 64 calls of that proc log their arguments and the
/// guest's return chain (x30, then the x29 frames), e.g. where a game sets its present interval.
unsafe fn bt_proc(c: *mut Cpu) {
    static N: AtomicU64 = AtomicU64::new(0);
    if N.fetch_add(1, Ordering::Relaxed) < 64 {
        let r = &*c;
        let mut s = format!("x0 {:#x} x1 {:#x} x2 {:#x}; lr {:#x}", r.x[0], r.x[1], r.x[2], r.x[30]);
        let mut fp = r.x[29];
        for _ in 0..8 {
            if fp == 0 || fp & 7 != 0 || !nxrt::mem::readable(fp, 16) {
                break;
            }
            s.push_str(&format!(" {:#x}", *((fp + 8) as *const u64)));
            fp = *(fp as *const u64);
        }
        crate::log!("NVN bt {}: {s}", std::env::var("NXOS_NVN_BT").unwrap_or_default());
    }
    if let Some(f) = BT_TARGET.get() {
        f(c);
    }
}

/// `nvnBootstrapLoader(const char *name)`: the entry to NVN; imported by `main`, defined by no module on SDK 4.
unsafe fn bootstrap_loader(c: *mut Cpu) {
    let name = crate::guest::cstr((*c).x[0], 128);
    crate::log_once!(format!("bootstrap {name}"), "nvnBootstrapLoader({name})");
    (*c).x[0] = proc_address(&name);
}

/// The game's NintendoSDK major from its `sdk` module's "NintendoSdk_nnSdk-<major>_<minor>_<micro>-Release" (Turbo
/// 2f8f44e): it picks the NVN API the device reports (SDK 10 and older are API 53; Mario Kart 8 Deluxe, SDK 4.4.0,
/// creates no graphics system on another). 19 when the string is absent. (Read from the module file before the load: the parsed image, or the raw file.)
fn sdk_major(data: &std::path::Path) -> i32 {
    fn scan(b: &[u8]) -> Option<i32> {
        let pat = b"NintendoSdk_nnSdk-";
        let i = b.windows(pat.len()).position(|w| w == pat)?;
        // Switch 2 SDKs name a variant first: "nnSdk-Generic_21_4_0" (Mario Kart World), as nxnative's dm_sdk_major
        let mut s = &b[i + pat.len()..];
        let alpha = s.iter().take(16).take_while(|c| c.is_ascii_alphabetic()).count();
        if alpha > 0 && s.get(alpha) == Some(&b'_') {
            s = &s[alpha + 1..];
        }
        let digits: String = s.iter().take(4).map(|&c| c as char).take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    }
    let v = std::fs::read(data.join("exefs").join("sdk")).ok().and_then(|raw| match nxrt::loader::parse(&raw) {
        Ok(img) => scan(&img.bytes),
        Err(_) => scan(&raw),
    });
    let major = v.unwrap_or(19);
    crate::log!("sdk: NintendoSDK {major}{}", if v.is_none() { " (no version string: taken as 19)" } else { "" });
    major
}

/// The hand-off to the graphics layer (docs/INTERFACES.md, graphics): every NVN proc the game asks for is the
/// graphics layer's, on the real Vulkan device; its waits give the virtual core up; its present and submit marks
/// come here. The game's host window opens now (until the front end owns it).
fn bind_graphics() {
    for e in nxgfx::nvn::procs() {
        register(e.name, e.f);
    }
    nxgfx::nvn::set_proc_resolver(proc_address);
    nxgfx::nvn::set_guest_caller(crate::guest::call_current);
    nxgfx::vk::counters::set_guest_clock(crate::svc::guest_ns);
    nxgfx::register_controls();
    nxgfx::nvn::set_hooks(nxgfx::nvn::Hooks { submit: note_submit, present: note_present, blocking: |f| crate::sched::without_core(f) });
    let prof = crate::profile::get();
    let data = crate::boot::process().data_dir.clone();
    let major = sdk_major(&data);
    nxgfx::nvn::device::SWITCH2.store(prof.switch2, Ordering::Relaxed);
    nxgfx::nvn::device::SDK_MAJOR.store(major, Ordering::Relaxed);
    nxgfx::nvn::vpool::APP_POOL.store(prof.pool, Ordering::Relaxed);
    let shaders = std::env::var("NXGFX_SHADERS").map(std::path::PathBuf::from).unwrap_or_else(|_| data.join("shaders"));
    if !shaders.join("manifest.tsv").is_file() {
        crate::log!("[RED] graphics: no translated shaders at {} (nxshaders prebuild-data): draws with a missing shader wait for none", shaders.display());
    }
    nxgfx::set_shader_dir(shaders.clone());
    nxgfx::set_cache_dir(data.clone());
    crate::log!("graphics: NVN on Vulkan, {} procs, SDK {major}{}, shaders {}", nxgfx::nvn::procs().len(), if prof.switch2 { ", NVN2" } else { "" }, shaders.display());
    // the front end (lane I, src/front) owns the game window when it is linked: it set FRONT_WINDOW and gave graphics its window
    if std::env::var("NXGFX_HEADLESS").is_err() && !FRONT_WINDOW.load(Ordering::Acquire) {
        nxgfx::host_window::set_key_handler(window_key);
        nxgfx::host_window::open(&prof.name, 1920, 1080);
    }
    let threads = std::env::var("NXGFX_PREWARM_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    std::thread::spawn(move || {
        let r = nxgfx::prewarm(threads);
        crate::log!("graphics: pipeline prewarm {r:?}");
    });
}

/// The keyboard as player 1's controller while the graphics layer's host window is the game window (Turbo's
/// layout: X / Space A, Z B, C X, V Y, Q L, E R, 1 ZL, 3 ZR, Enter +, Backspace -, arrows the d-pad, WASD the left
/// stick, IJKL the right stick). The front end's input layer replaces it when it owns the window.
fn window_key(vk: u32, down: bool) {
    static HELD: AtomicU64 = AtomicU64::new(0);
    let bit = match vk {
        0x58 | 0x20 => 1u64 << 0,
        0x5A => 1 << 1,
        0x43 => 1 << 2,
        0x56 => 1 << 3,
        0x51 => 1 << 6,
        0x45 => 1 << 7,
        0x31 => 1 << 8,
        0x33 => 1 << 9,
        0x0D => 1 << 10,
        0x08 => 1 << 11,
        0x25 => 1 << 12,
        0x26 => 1 << 13,
        0x27 => 1 << 14,
        0x28 => 1 << 15,
        0x41 => 1 << 40,
        0x57 => 1 << 41,
        0x44 => 1 << 42,
        0x53 => 1 << 43,
        0x4A => 1 << 44,
        0x49 => 1 << 45,
        0x4C => 1 << 46,
        0x4B => 1 << 47,
        _ => return,
    };
    let h = if down { HELD.fetch_or(bit, Ordering::Relaxed) | bit } else { HELD.fetch_and(!bit, Ordering::Relaxed) & !bit };
    let axis = |neg: u64, pos: u64| -> i16 { (if h & pos != 0 { 0x7fff } else { 0 }) - (if h & neg != 0 { 0x7fff } else { 0 }) };
    crate::input::set_keyboard(h & 0xffff, [axis(1 << 40, 1 << 42) as i32, axis(1 << 43, 1 << 41) as i32, axis(1 << 44, 1 << 46) as i32, axis(1 << 47, 1 << 45) as i32]);
}

/// Set by the front end before the boot: it owns the game window (graphics' host window is not opened).
pub static FRONT_WINDOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called by the boot after the services registered: binds the NVN entry and the nv:: driver glue.
pub fn finish_registration(reg: &mut crate::hle::Registry) {
    bind_graphics();
    // NVN2 native images (decoded video pictures as textures): sdk symbols, the graphics layer's over the decoders' pictures
    nxgfx::nvn::native::set_source(crate::services::codec_video::native_image);
    for (name, f) in nxgfx::nvn::native::takeovers() {
        reg.force(name, "native images are the graphics layer's, over the video decoders' pictures", f);
    }
    reg.force("nvnBootstrapLoader", "the NVN entry: the graphics layer's procs (logging stubs for names it lacks)", bootstrap_loader);
    nv::register_nv(reg);
    let n = GRAPHICS.lock().unwrap().len();
    crate::log!("NVN: {n} procs from the graphics layer, the rest logging stubs");
}
