//! The SDK's nn::os objects the services hand out, and host entropy.
//!
//! A service-layer `nn::os::SystemEventType` a host service signals (an async context's completion, the oe
//! notification event, hid's style-set events): bound on its first use to a kernel event the SDK creates
//! itself (`CreateSystemEvent(ev, clear mode, interprocess)` then `GetWritableHandleOfSystemEvent`, run as guest
//! code on the calling guest thread); the host keeps the write end and signals or clears it from any thread.
//! A CloseHandle of that write end (the game destroyed the event) ends the binding.

use crate::guest::Args;
use crate::hle::Registry;
use nxrt::cpu::Cpu;
use std::sync::Mutex;

static BOUND: Mutex<Vec<(u64, u32)>> = Mutex::new(Vec::new());

fn find(ev: u64) -> Option<u32> {
    BOUND.lock().unwrap().iter().find(|(e, _)| *e == ev).map(|(_, h)| *h)
}

fn ksvc(imm: u32, h: u32) {
    let mut c = Cpu::new_boxed();
    c.x[0] = h as u64;
    nxrt::kernel::svc(&mut c, imm);
}

/// Binds `ev` (a guest SystemEventType) to a new kernel event on the calling guest thread `c`.
///
/// # Safety
/// `c` is the calling guest thread's state.
pub unsafe fn bind_system_event(c: *mut Cpu, ev: u64, autoclear: bool) -> Option<u32> {
    if let Some(h) = find(ev) {
        return Some(h);
    }
    let create = crate::hle::sym("_ZN2nn2os17CreateSystemEventEPNS0_15SystemEventTypeENS0_14EventClearModeEb")?;
    let getw = crate::hle::sym("_ZN2nn2os30GetWritableHandleOfSystemEventEPKNS0_15SystemEventTypeE")?;
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
        ksvc(0x11, h);
    }
}

/// Signals a bound SystemEvent from any thread; false when it is not bound.
pub fn signal_bound(ev: u64) -> bool {
    match find(ev) {
        Some(h) => {
            ksvc(0x11, h);
            true
        }
        None => false,
    }
}

/// Clears a bound SystemEvent from any thread.
pub fn clear_system_event(ev: u64) {
    if let Some(h) = find(ev) {
        ksvc(0x12, h);
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

pub fn register(r: &mut Registry) {
    r.hle("getentropy", getentropy);
    r.force("_ZN2nn6crypto36GenerateCryptographicallyRandomBytesEPvm", "csrng service: host entropy", csrng);
}
