//! The host-call bridge: host (Rust) functions at synthetic guest addresses.
//! Registration is by Switch symbol name; an import nobody provides gets a stub that reports it by name.
//!
//! Every host function is a `HostFn` (a Rust `unsafe fn(*mut Cpu)`) at `HOSTFN_BASE + 16 * i`. The address is what
//! the loader writes into the GOT of an import (`bind`), what the host hands the guest as a function pointer
//! (vtables, NVN procs: `register`), and what `rt_dispatch` resolves on its slow path. The function reads its
//! arguments from x0-x7/v0-v7 and the guest stack itself (`Cpu::arg_*`), so narrow integers and variadics need no
//! thunks. It runs on the calling guest thread, may clobber x0-x18 and v0-v7/v16-v31, must preserve x19-x29, sp and
//! v8-v15, and returns to the guest call site with a C return. On entry `c->pc` holds its own address. It is entered
//! through `nxrt_host_entry` (asm.rs) and runs under `boundary::guard`; to call guest code it uses
//! `boundary::call_guest` and returns at once when that reports a jump past it.
//!
//! Registration may happen at any time (slots are published with release stores and read lock-free).

use crate::cpu::Cpu;

/// A host function: plain Rust ABI, called only by `boundary::nxrt_host_invoke` (so a panic in it unwinds into
/// that guard, never into generated code).
pub type HostFn = unsafe fn(*mut Cpu);
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

pub const HOSTFN_BASE: u64 = 0x7F_1000_0000;
pub const HOSTFN_SIZE: u64 = 0x10_0000;
pub const STRIDE: u64 = 16;
pub const MAX: usize = (HOSTFN_SIZE / STRIDE) as usize;
/// The return address a host -> guest call leaves in x30: a guest jump there is a bug, reported by name.
pub const RET_HOST: u64 = HOSTFN_BASE + HOSTFN_SIZE - STRIDE;

#[allow(clippy::declare_interior_mutable_const)] // array initialisers only
const ZERO: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO64: AtomicU64 = AtomicU64::new(0);
static SLOTS: [AtomicUsize; MAX] = [ZERO; MAX];
static CALLS: [AtomicU64; MAX] = [ZERO64; MAX];

struct Registry {
    names: Vec<String>,
    by_key: HashMap<(String, usize), u64>,
    overrides: HashMap<String, HostFn>,
    unresolved: HashMap<String, u64>,
}

static REG: Mutex<Option<Registry>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut g = REG.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let r = g.get_or_insert_with(|| Registry {
        names: vec![],
        by_key: HashMap::new(),
        overrides: HashMap::new(),
        unresolved: HashMap::new(),
    });
    f(r)
}

/// A synthetic guest address for `f`. The same (name, f) pair always gets the same address.
pub fn register(name: &str, f: HostFn) -> u64 {
    with(|r| {
        let key = (name.to_string(), f as usize);
        if let Some(&a) = r.by_key.get(&key) {
            return a;
        }
        let i = r.names.len();
        if i >= MAX - 1 {
            crate::rt::fatal(&format!("hostfn: more than {} host functions", MAX - 1));
        }
        r.names.push(name.to_string());
        SLOTS[i].store(f as usize, Ordering::Release);
        let a = HOSTFN_BASE + STRIDE * i as u64;
        r.by_key.insert(key, a);
        a
    })
}

/// Guest addresses outside the host-function window that run a host function (a game patch's `bl` into the patch
/// page after `main`, `aot/patches.rs`): `rt_dispatch`'s slow path finds the host function here.
static ALIASES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// Makes guest address `at` call host function `f` (registered under `name`).
pub fn alias(at: u64, name: &str, f: HostFn) {
    let a = register(name, f);
    let mut v = ALIASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    v.retain(|e| e.0 != at);
    v.push((at, a));
}

/// The host-function address standing in for guest address `pc`, if `alias` made one.
pub fn aliased(pc: u64) -> Option<u64> {
    let v = ALIASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    v.iter().find(|e| e.0 == pc).map(|e| e.1)
}

/// `HOST_OVERRIDE`: every import of `name` (GOT, GLOB_DAT, ABS64 in every module) binds to `f`, also when a module
/// defines the symbol. A later override of the same name replaces an earlier one.
pub fn add_override(name: &str, f: HostFn) {
    with(|r| r.overrides.insert(name.to_string(), f));
}

pub fn override_of(name: &str) -> Option<HostFn> {
    with(|r| r.overrides.get(name).copied())
}

/// The address an import of `name` binds to when an override exists.
pub fn bind(name: &str) -> Option<u64> {
    override_of(name).map(|f| register(name, f))
}

/// The address of a stub that reports `name` (an import nothing provides) when called.
pub fn unresolved(name: &str) -> u64 {
    if let Some(a) = with(|r| r.unresolved.get(name).copied()) {
        return a;
    }
    let a = register(name, unresolved_import);
    with(|r| r.unresolved.insert(name.to_string(), a));
    a
}

unsafe fn unresolved_import(c: *mut Cpu) {
    let name = name_of((*c).pc).unwrap_or_default();
    crate::rt::guest_fatal(c, &format!("UNIMPL: the guest called {name}, an import no module or host function provides"));
}

unsafe fn ret_host(c: *mut Cpu) {
    crate::rt::guest_fatal(c, "guest code jumped to the host-return address (a host -> guest call's x30)");
}

fn index(pc: u64) -> Option<usize> {
    let d = pc.wrapping_sub(HOSTFN_BASE);
    (d < HOSTFN_SIZE && d.is_multiple_of(STRIDE)).then_some((d / STRIDE) as usize)
}

/// Counts a call of the host function at `pc` (and traces the first one).
pub fn count_call(pc: u64) {
    if let Some(i) = index(pc) {
        if CALLS[i].fetch_add(1, Ordering::Relaxed) == 0 && crate::rt::trace("host") {
            crate::rt::log(&format!("host: first call of {} at {pc:#x}", name_of(pc).unwrap_or_default()));
        }
    }
}

/// The host function at `pc`.
pub fn lookup(pc: u64) -> Option<HostFn> {
    let i = index(pc)?;
    if pc == RET_HOST {
        return Some(ret_host);
    }
    let f = SLOTS[i].load(Ordering::Acquire);
    if f == 0 {
        return None;
    }
    // SAFETY: only HostFn values are stored.
    Some(unsafe { core::mem::transmute::<usize, HostFn>(f) })
}

pub fn name_of(pc: u64) -> Option<String> {
    if pc == RET_HOST {
        return Some("<return to host>".into());
    }
    let i = index(pc)?;
    with(|r| r.names.get(i).cloned())
}

pub fn calls(pc: u64) -> u64 {
    index(pc).map_or(0, |i| CALLS[i].load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn add(c: *mut Cpu) {
        let c = &mut *c;
        let s = c.arg_x(0) + c.arg_x(1) + c.arg_x(8);
        c.ret_x(s);
    }
    unsafe fn other(c: *mut Cpu) {
        (*c).x[0] = 0;
    }

    #[test]
    fn registry() {
        let a = register("test_add", add);
        assert_eq!(register("test_add", add), a, "same pair, same address");
        let b = register("test_other", other);
        assert_ne!(a, b);
        assert_eq!(a % STRIDE, 0);
        assert!(a >= HOSTFN_BASE && a < HOSTFN_BASE + HOSTFN_SIZE);
        assert_eq!(name_of(b).as_deref(), Some("test_other"));
        assert!(lookup(a + 4).is_none());
        let stack = [0u64, 100];
        let mut c = Cpu::new_boxed();
        c.x[0] = 2;
        c.x[1] = 3;
        c.sp = stack.as_ptr() as u64 + 8;
        unsafe { lookup(a).unwrap()(&mut *c) };
        assert_eq!(c.x[0], 105);
        count_call(a);
        assert!(calls(a) >= 1);
    }

    #[test]
    fn overrides_and_unresolved() {
        assert!(bind("_ZN2nn2os4TestEv").is_none());
        add_override("_ZN2nn2os4TestEv", add);
        let a = bind("_ZN2nn2os4TestEv").unwrap();
        assert_eq!(bind("_ZN2nn2os4TestEv"), Some(a));
        add_override("_ZN2nn2os4TestEv", other);
        assert_ne!(bind("_ZN2nn2os4TestEv"), Some(a), "a later override wins");
        let u = unresolved("missing_fn");
        assert_eq!(unresolved("missing_fn"), u);
        assert_eq!(name_of(u).as_deref(), Some("missing_fn"));
        assert!(lookup(RET_HOST).is_some());
    }
}
