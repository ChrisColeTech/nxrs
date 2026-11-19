//! nn::os fibers, replaced by name through the bridge.
//!
//! The SDK switches fibers by swapping the register context, which breaks "RET = C return" in lifted code. So each
//! guest fiber gets a host fiber (its own reserved host stack) and its own `Cpu`: a suspended fiber's guest registers
//! and host frames stay untouched while others run. A switch carries the thread's state (TPIDR, TPIDRRO, FPCR/FPSR)
//! over to the target's `Cpu` and clears the exclusive monitor. A fiber's function returns the fiber to switch to
//! next (NULL: back to the thread's own context).

use crate::cpu::Cpu;
#[allow(unused_imports)]
use crate::boundary;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use windows_sys::Win32::System::Threading::{ConvertThreadToFiberEx, CreateFiberEx, DeleteFiber, SwitchToFiber};

const FIBER_FLAG_FLOAT_SWITCH: u32 = 1;
const HOST_STACK: usize = 256 << 20;

pub const SYM_INITIALIZE: &str = "_ZN2nn2os15InitializeFiberEPNS0_9FiberTypeEPFS2_PvES3_S3_mi";
pub const SYM_FINALIZE: &str = "_ZN2nn2os13FinalizeFiberEPNS0_9FiberTypeE";
pub const SYM_SWITCH: &str = "_ZN2nn2os13SwitchToFiberEPNS0_9FiberTypeE";
pub const SYM_CURRENT: &str = "_ZN2nn2os15GetCurrentFiberEv";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    New,
    Started,
    Done,
}

struct Fiber {
    addr: u64,
    func: u64,
    arg: u64,
    stack: u64,
    size: u64,
    cpu: *mut Cpu,
    host: usize,
    state: St,
    running: bool,
}

unsafe impl Send for Fiber {}

static FIBERS: Mutex<Option<HashMap<u64, Arc<Mutex<Fiber>>>>> = Mutex::new(None);

thread_local! {
    /// This host thread converted to a host fiber (after its first switch).
    static BASE: Cell<usize> = const { Cell::new(0) };
    /// The guest fiber running on this host thread (None: the thread's own context) and the thread's own Cpu.
    static CURRENT: RefCell<Option<Arc<Mutex<Fiber>>>> = const { RefCell::new(None) };
    static THREAD_CPU: Cell<*mut Cpu> = const { Cell::new(core::ptr::null_mut()) };
    /// The fiber just switched away from: released by whoever resumes next.
    static LEAVING: RefCell<Option<Arc<Mutex<Fiber>>>> = const { RefCell::new(None) };
}

pub fn on_fiber() -> bool {
    CURRENT.with(|c| c.borrow().is_some())
}

fn find(addr: u64) -> Option<Arc<Mutex<Fiber>>> {
    FIBERS.lock().unwrap().as_ref().and_then(|m| m.get(&addr).cloned())
}

fn resumed() {
    if let Some(f) = LEAVING.with(|l| l.borrow_mut().take()) {
        f.lock().unwrap().running = false;
    }
}

/// Runs a fiber's guest function (called by `nxrt_fiber_entry`, the fiber's first frame) and returns the host fiber
/// to switch to when it returns: the next fiber it names, or the thread's own context. Every Rust frame has returned
/// before the switch, so a finished fiber's stack holds none.
#[no_mangle]
unsafe extern "C" fn nxrt_fiber_run(p: *mut core::ffi::c_void) -> usize {
    crate::boundary::guard("a fiber entry", || {
        resumed();
        let f = Arc::from_raw(p as *const Mutex<Fiber>);
        let (c, func, arg, addr) = {
            let g = f.lock().unwrap();
            (g.cpu, g.func, g.arg, g.addr)
        };
        (*c).x[0] = arg;
        if crate::boundary::call_guest(c, func) != crate::boundary::GuestCall::Returned {
            crate::rt::guest_fatal(c, &format!("fiber {addr:#x}: a guest jump or ExitThread left the fiber's function"));
        }
        let next = (*c).x[0];
        f.lock().unwrap().state = St::Done;
        drop(f);
        let n = if next != 0 {
            Some(find(next).unwrap_or_else(|| {
                crate::rt::guest_fatal(c, &format!("fiber {addr:#x} returned {next:#x}, which is not an initialized fiber"))
            }))
        } else {
            None
        };
        prepare(c, n)
    })
}

#[no_mangle]
extern "C" fn nxrt_fiber_resumed_dead() {
    crate::rt::fatal("a finished fiber was resumed");
}

/// Bookkeeping of a switch from the guest context `from` to fiber `to` (None: the thread's own context); returns the
/// host fiber to switch to.
unsafe fn prepare(from: *const Cpu, to: Option<Arc<Mutex<Fiber>>>) -> usize {
    if BASE.get() == 0 {
        let b = ConvertThreadToFiberEx(core::ptr::null(), FIBER_FLAG_FLOAT_SWITCH);
        if b.is_null() {
            crate::rt::fatal("fiber: cannot make the thread a fiber");
        }
        BASE.set(b as usize);
    }
    let (dst, host) = match &to {
        Some(f) => {
            let mut g = f.lock().unwrap();
            if g.state == St::Done {
                crate::rt::fatal(&format!("SwitchToFiber({:#x}): the fiber has finished", g.addr));
            }
            if g.running {
                crate::rt::fatal(&format!("SwitchToFiber({:#x}): the fiber is running", g.addr));
            }
            g.running = true;
            if g.state == St::New {
                (*g.cpu).sp = (g.stack + g.size) & !15;
                g.state = St::Started;
            }
            (g.cpu, g.host)
        }
        None => (THREAD_CPU.get(), BASE.get()),
    };
    let d = &mut *dst;
    d.tpidr = (*from).tpidr;
    d.tpidrro = (*from).tpidrro;
    d.fpcr = (*from).fpcr;
    d.fpsr = (*from).fpsr;
    d.excl_addr = !0;
    d.thread = (*from).thread;
    let prev = CURRENT.with(|c| core::mem::replace(&mut *c.borrow_mut(), to));
    LEAVING.with(|l| *l.borrow_mut() = prev);
    host
}

/// A switch from inside a host function: the calling Rust frames are suspended with the fiber, not skipped, and
/// continue when something switches back.
unsafe fn switch_to(from: *const Cpu, to: Option<Arc<Mutex<Fiber>>>) {
    let host = prepare(from, to);
    SwitchToFiber(host as *const _);
    resumed();
    let me = CURRENT.with(|c| c.borrow().as_ref().map(|f| f.lock().unwrap().cpu)).unwrap_or(THREAD_CPU.get());
    (*me).sync_mxcsr();
}

/// InitializeFiber(x0 FiberType*, x1 function, x2 argument, x3 stack, x4 stack size, w5 flags)
unsafe fn initialize(c: *mut Cpu) {
    let c = &mut *c;
    let addr = c.x[0];
    let mut map = FIBERS.lock().unwrap();
    let map = map.get_or_insert_with(HashMap::new);
    if let Some(old) = map.get(&addr) {
        let g = old.lock().unwrap();
        if g.running {
            crate::rt::guest_fatal(c, &format!("InitializeFiber({addr:#x}): the fiber is running"));
        }
    }
    let cpu = Box::into_raw(Cpu::new_boxed());
    let f = Arc::new(Mutex::new(Fiber { addr, func: c.x[1], arg: c.x[2], stack: c.x[3], size: c.x[4], cpu, host: 0, state: St::New, running: false }));
    let entry: unsafe extern "system" fn(*mut core::ffi::c_void) = core::mem::transmute(crate::asm::nxrt_fiber_entry as *const () as usize);
    let host = CreateFiberEx(64 << 10, HOST_STACK, FIBER_FLAG_FLOAT_SWITCH, Some(entry), Arc::into_raw(f.clone()) as *const _);
    if host.is_null() {
        crate::rt::fatal("fiber: cannot create a host fiber");
    }
    f.lock().unwrap().host = host as usize;
    if let Some(old) = map.insert(addr, f) {
        release(&old.lock().unwrap());
    }
}

/// FinalizeFiber(x0 FiberType*)
unsafe fn finalize(c: *mut Cpu) {
    let addr = (*c).x[0];
    let f = FIBERS.lock().unwrap().as_mut().and_then(|m| m.remove(&addr));
    let Some(f) = f else { crate::rt::guest_fatal(c, &format!("FinalizeFiber({addr:#x}): not an initialized fiber")) };
    let g = f.lock().unwrap();
    if g.running {
        crate::rt::guest_fatal(c, &format!("FinalizeFiber({addr:#x}): the fiber is running"));
    }
    release(&g);
}

/// Deletes a fiber's host fiber when its stack holds no live Rust frame (never started, or finished: only
/// `nxrt_fiber_entry` and SwitchToFiber remain). A fiber suspended inside its function keeps host frames of ours
/// (the SwitchToFiber host call); it is left allocated rather than deleted under them.
unsafe fn release(g: &Fiber) {
    if g.state == St::Started {
        crate::rt::log(&format!("fiber {:#x}: finalized while suspended; its host fiber is kept", g.addr));
        return;
    }
    DeleteFiber(g.host as *const _);
}

/// SwitchToFiber(x0 FiberType*; NULL: the thread's own context)
unsafe fn switch(c: *mut Cpu) {
    let addr = (*c).x[0];
    if !on_fiber() {
        THREAD_CPU.set(c);
    }
    let to = if addr != 0 {
        Some(find(addr).unwrap_or_else(|| crate::rt::guest_fatal(c, &format!("SwitchToFiber({addr:#x}): not an initialized fiber"))))
    } else {
        None
    };
    let same = CURRENT.with(|cur| match (&*cur.borrow(), &to) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    });
    if !same {
        switch_to(c, to);
    }
}

/// GetCurrentFiber() -> x0 (NULL on the thread's own context)
unsafe fn current(c: *mut Cpu) {
    (*c).x[0] = CURRENT.with(|f| f.borrow().as_ref().map_or(0, |f| f.lock().unwrap().addr));
}

pub fn register() {
    // SAFETY: written before any fiber exists.
    unsafe { crate::asm::NXRT_SWITCH_TO_FIBER = SwitchToFiber as *const () as usize };
    crate::hostfn::add_override(SYM_INITIALIZE, initialize);
    crate::hostfn::add_override(SYM_FINALIZE, finalize);
    crate::hostfn::add_override(SYM_SWITCH, switch);
    crate::hostfn::add_override(SYM_CURRENT, current);
}
