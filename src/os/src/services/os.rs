//! The SDK's nn::os objects the services hand out, and host entropy.
//!
//! A service-layer `nn::os::SystemEventType` a host service signals (an async context's completion, the oe
//! notification event, hid's style-set events): bound on its first use to a kernel event the SDK creates
//! itself (`CreateSystemEvent(ev, clear mode, interprocess)` then `GetWritableHandleOfSystemEvent`, run as guest
//! code on the calling guest thread); the host keeps the write end and signals or clears it from any thread.
//! A CloseHandle of that write end (the game destroyed the event) ends the binding.
//!
//! Two pass-throughs a game's settings turn on: `game_env` (`KEY=VALUE` words; `NXOS_GAME_ENV`) answers getenv
//! for its names (Horizon gives a program no environment, and runtimes that read settings from it run on
//! defaults sized for the console otherwise: a .NET GC's region range), every other name going to the SDK's own
//! getenv; `alloc_big_align` (`NXOS_ALLOC_BIG_ALIGN=1`) makes nn::mem::StandardAllocator::Allocate align blocks
//! of 64 KB or more to 64 KB (an engine that cuts its GPU memory pools at 64 KB alignment relative to the pool's
//! GPU address, which is a big page on the console and only a 4 KB page here). Neither is taken over without its
//! setting.

use crate::guest::Args;
use crate::hle::Registry;
use nxrt::cpu::Cpu;
use std::sync::Mutex;

static BOUND: Mutex<Vec<(u64, u32)>> = Mutex::new(Vec::new());

fn find(ev: u64) -> Option<u32> {
    BOUND
        .lock()
        .unwrap()
        .iter()
        .find(|(e, _)| *e == ev)
        .map(|(_, h)| *h)
}

fn signal(h: u32) {
    nxrt::kernel::get().signal_event(h);
}

fn clear(h: u32) {
    nxrt::kernel::get().clear_event(h);
}

/// Binds `ev` (a guest SystemEventType) to a new kernel event on the calling guest thread `c`.
///
/// # Safety
/// `c` is the calling guest thread's state.
pub unsafe fn bind_system_event(c: *mut Cpu, ev: u64, autoclear: bool) -> Option<u32> {
    if let Some(h) = find(ev) {
        return Some(h);
    }
    let create = crate::hle::sym(
        "_ZN2nn2os17CreateSystemEventEPNS0_15SystemEventTypeENS0_14EventClearModeEb",
    )?;
    let getw =
        crate::hle::sym("_ZN2nn2os30GetWritableHandleOfSystemEventEPKNS0_15SystemEventTypeE")?;
    let r = crate::guest::call(c, create, &[ev, autoclear as u64, 1])?;
    if r as u32 != 0 {
        crate::log!("[RED] os: CreateSystemEvent for a host event failed {r:#x}");
        return None;
    }
    let wh = crate::guest::call(c, getw, &[ev])? as u32;
    BOUND.lock().unwrap().push((ev, wh));
    Some(wh)
}

/// Signals a service-layer SystemEvent (binding it first, on the calling guest thread, when it is new).
///
/// # Safety
/// `c` is the calling guest thread's state.
pub unsafe fn signal_system_event(c: *mut Cpu, ev: u64) {
    let h = match find(ev) {
        Some(h) => Some(h),
        None => bind_system_event(c, ev, false),
    };
    if let Some(h) = h {
        signal(h);
    }
}

/// Signals a bound SystemEvent from any thread; false when it is not bound.
pub fn signal_bound(ev: u64) -> bool {
    match find(ev) {
        Some(h) => {
            signal(h);
            true
        }
        None => false,
    }
}

/// Clears a bound SystemEvent from any thread.
pub fn clear_system_event(ev: u64) {
    if let Some(h) = find(ev) {
        clear(h);
    }
}

/// CloseHandle of a kernel handle: a bound event's write end ends its binding.
pub fn handle_closed(h: u32) {
    BOUND.lock().unwrap().retain(|(_, w)| *w != h);
}

fn fill_entropy(p: u64, n: u64) {
    let mut buf = vec![0u8; n as usize];
    let mut x = nxrt::rt::now_ns() ^ 0xD1B5_4A32_D192_ED03;
    for chunk in buf.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let b = x.to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
    crate::guest::write_bytes(p, &buf);
}

unsafe fn getentropy(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    fill_entropy(a.x(0), a.x(1).min(256));
    a.ret(0);
}

unsafe fn csrng(c: *mut Cpu) {
    let a = Args(&mut *c);
    fill_entropy(a.x(0), a.x(1));
}

/// The game's environment: (name, guest address of its value), from `game_env`.
fn game_env() -> &'static [(String, u64)] {
    static E: std::sync::OnceLock<Vec<(String, u64)>> = std::sync::OnceLock::new();
    E.get_or_init(|| {
        let spec = crate::options::game("game_env", "");
        let mut v = vec![];
        for word in spec.split_whitespace() {
            let Some((k, val)) = word.split_once('=') else {
                continue;
            };
            let p = nxrt::kernel::get()
                .space
                .kernel_alloc(val.len() as u64 + 1, 16);
            crate::guest::put_cstr(p, val.len() as u64 + 1, val);
            v.push((k.to_string(), p));
        }
        if !v.is_empty() {
            crate::log!("game environment (game_env): {spec}");
        }
        v
    })
}

unsafe fn getenv(c: *mut Cpu) {
    let p = (*c).x[0];
    let name = if p != 0 {
        crate::guest::cstr(p, 256)
    } else {
        String::new()
    };
    if let Some((_, v)) = game_env()
        .iter()
        .find(|(k, _)| !name.is_empty() && *k == name)
    {
        (*c).x[0] = *v;
        return;
    }
    let Some(orig) = crate::hle::sym("getenv") else {
        (*c).x[0] = 0;
        return;
    };
    if let Some(r) = crate::guest::call(c, orig, &[p]) {
        (*c).x[0] = r;
    }
}

/// StandardAllocator::Allocate(self, size): blocks of 64 KB or more through Allocate(self, size, 64 KB).
unsafe fn sa_allocate(c: *mut Cpu) {
    let (this, size) = ((*c).x[0], (*c).x[1]);
    let sym = |n: &str| {
        crate::hle::sym(n).unwrap_or_else(|| nxrt::rt::fatal(&format!("os: the SDK has no {n}")))
    };
    let r = if size >= 0x10000 {
        crate::guest::call(
            c,
            sym("_ZN2nn3mem17StandardAllocator8AllocateEmm"),
            &[this, size, 0x10000],
        )
    } else {
        crate::guest::call(
            c,
            sym("_ZN2nn3mem17StandardAllocator8AllocateEm"),
            &[this, size],
        )
    };
    if let Some(r) = r {
        (*c).x[0] = r;
    }
}

pub fn register(r: &mut Registry) {
    if !crate::options::game("game_env", "").trim().is_empty() {
        r.force(
            "getenv",
            "the game's environment (game_env), the SDK's getenv for the rest",
            getenv,
        );
    }
    if crate::options::game_flag("alloc_big_align", false) {
        crate::log!("StandardAllocator::Allocate: allocations of 64 KB or more are 64 KB aligned (alloc_big_align)");
        r.force(
            "_ZN2nn3mem17StandardAllocator8AllocateEm",
            "blocks of 64 KB or more 64 KB aligned (alloc_big_align)",
            sa_allocate,
        );
    }
    r.hle("getentropy", getentropy);
    r.force(
        "_ZN2nn6crypto36GenerateCryptographicallyRandomBytesEPvm",
        "csrng service: host entropy",
        csrng,
    );
}
