//! The NVN record layer: the `nvn*` functions the game calls, on objects stored in place in the game's own object
//! memory (`obj`).
//!
//! - Functions with CPU-visible semantics (device, memory pools, textures and samplers, queues, syncs, events,
//!   command memory, window) are implemented as NVN defines them.
//! - Functions that only change GPU state (state objects, bindings, draws, clears, copies) are recorded as commands
//!   into the game's command memory and replayed on Vulkan at submit (`vk::exec`).
//! - GPU address = CPU address: a pool's storage address is its GPU address, as on the console's unified memory; the
//!   back end gives every address its placement on the discrete GPU (`vk::mem`).

pub mod cmd;
pub mod cmd2;
pub mod native;
pub mod device;
pub mod entry;
pub mod nops;
pub mod obj;
pub mod pool;
pub mod prog;
pub mod prog2;
pub mod query;
pub mod dlss;
pub mod devbuf;
pub mod queue;
pub mod state;
pub mod sync;
pub mod tex;
pub mod types;
pub mod vpool;
pub mod window;

pub use entry::{find, procs, resolve, set_proc_resolver, Entry, HostFn};

/// What the OS layer wants to know of, and do around, the NVN calls (all optional).
#[derive(Clone, Copy)]
pub struct Hooks {
    /// nvnQueueSubmitCommands(queue object address, handle count)
    pub submit: fn(queue: u64, n: u64),
    /// nvnQueuePresentTexture(window object address, texture index)
    pub present: fn(window: u64, index: u32),
    /// runs a wait that blocks the calling guest thread (fence and vsync waits): the OS gives its core up meanwhile
    pub blocking: fn(&mut dyn FnMut()),
}
static HOOKS: std::sync::OnceLock<Hooks> = std::sync::OnceLock::new();
pub fn set_hooks(h: Hooks) {
    let _ = HOOKS.set(h);
}
pub(crate) fn hooks() -> Option<&'static Hooks> {
    HOOKS.get()
}
/// Runs `f`, a wait that blocks the guest thread, through the OS's hook.
pub fn blocking<R>(f: impl FnOnce() -> R) -> R {
    match HOOKS.get() {
        Some(h) => {
            let mut f = Some(f);
            let mut r = None;
            (h.blocking)(&mut || r = Some((f.take().unwrap())()));
            r.expect("the blocking hook ran its wait")
        }
        None => f(),
    }
}

use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);
/// A new serial (> 0, increasing): textures, programs, queues, pools.
pub fn new_serial() -> u64 {
    SERIAL.fetch_add(1, Ordering::Relaxed) + 1
}

/// NVN booleans: callers do not extend them; only the low 8 bits count.
#[inline]
pub fn b(x: u32) -> bool {
    x & 0xff != 0
}

/// # Safety
/// `p` is null or a NUL-terminated string.
pub unsafe fn cstr(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(p as *const std::ffi::c_char).to_string_lossy().into_owned()
}

/// How the record layer calls back into the game (command memory callbacks): lane A installs a caller that runs the
/// guest function at `addr` with integer arguments; without one, `addr` is taken as a host function
/// `extern "C" fn(u64, u64, u64, u64)` (tests and tools).
pub type GuestCaller = fn(addr: u64, args: &[u64]) -> u64;
static CALLER: std::sync::OnceLock<GuestCaller> = std::sync::OnceLock::new();
pub fn set_guest_caller(f: GuestCaller) {
    let _ = CALLER.set(f);
}
pub(crate) fn call_guest(addr: u64, args: &[u64]) -> u64 {
    if let Some(f) = CALLER.get() {
        return f(addr, args);
    }
    let mut a = [0u64; 4];
    for (i, v) in args.iter().take(4).enumerate() {
        a[i] = *v;
    }
    // SAFETY: without a guest caller, callbacks are host functions of this signature (documented above).
    let f: extern "C" fn(u64, u64, u64, u64) -> u64 = unsafe { core::mem::transmute(addr as usize) };
    f(a[0], a[1], a[2], a[3])
}
