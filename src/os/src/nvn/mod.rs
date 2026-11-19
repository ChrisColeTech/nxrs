//! NVN as the os layer binds it until the graphics layer takes over: `nvnBootstrapLoader` and
//! `nvnDeviceGetProcAddress` hand out host functions by name, from three sources in order:
//!
//! 1. procs the graphics layer registered (`register`; see docs/INTERFACES.md, graphics);
//! 2. the os layer's null device (`null.rs`): the calls whose answers the game reads before it can render
//!    (device integers, memory pools and buffer addresses, texture storage sizes, queue and command memory sizes,
//!    window texture indices, syncs that are signalled), with the objects kept in the game's own NVN memory;
//! 3. a logging stub per name, which counts the call, records an `UNIMPL Nvn` gap on its first call and answers 0
//!    (command recording calls, state setters and the rest).
//!
//! Every proc's calls are counted; `report` prints the table. The first `nvnQueueSubmitCommands` is logged with
//! the time since boot: milestone M3's mark.

pub mod null;
pub mod tex;

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

/// A present waits for the display's next 60 Hz slot, as a swap interval of 1 does (the null device has no
/// display to pace it); the core is given up while it waits.
pub fn pace_present() {
    static NEXT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let period = std::time::Duration::from_nanos(16_666_667);
    let wait = {
        let mut g = NEXT.lock().unwrap();
        let now = std::time::Instant::now();
        let at = match *g {
            Some(t) if t > now => t,
            Some(t) if now - t < period => t,
            _ => now,
        };
        *g = Some(at + period);
        at.saturating_duration_since(now)
    };
    if !wait.is_zero() {
        crate::sched::without_core(|| std::thread::sleep(wait));
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
        crate::log!("  nvn {c:8} {n}{}", if null::implements(n) { "" } else { "  (stub)" });
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
        return crate::hle::addr(&key, *f);
    }
    if let Some(f) = null::lookup(name) {
        return crate::hle::addr(&key, f);
    }
    crate::hle::addr(&key, stub)
}

/// `nvnBootstrapLoader(const char *name)`: the entry to NVN; imported by `main`, defined by no module on SDK 4.
unsafe fn bootstrap_loader(c: *mut Cpu) {
    let name = crate::guest::cstr((*c).x[0], 128);
    crate::log_once!(format!("bootstrap {name}"), "nvnBootstrapLoader({name})");
    (*c).x[0] = proc_address(&name);
}

/// Called by the boot after the services registered: binds the NVN entry and the nv:: driver glue.
pub fn finish_registration(reg: &mut crate::hle::Registry) {
    reg.force("nvnBootstrapLoader", "the NVN entry: the host's procs (graphics layer, null device, stubs)", bootstrap_loader);
    null::register_nv(reg);
    let n = GRAPHICS.lock().unwrap().len();
    crate::log!("NVN: {n} procs from the graphics layer, {} from the null device, the rest logging stubs", null::count());
}
