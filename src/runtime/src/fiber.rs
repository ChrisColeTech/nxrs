//! nn::os fibers, replaced by name through the bridge.
//!
//! The SDK switches fibers by swapping the register context, which breaks "RET = C return" in lifted code. So each
//! guest fiber gets a host fiber (its own reserved host stack) and its own `Cpu`: a suspended fiber's guest registers
//! and host frames stay untouched while others run. A switch carries the thread's state (TPIDR, TPIDRRO, FPCR/FPSR)
//! over to the target's `Cpu` and clears the exclusive monitor. A fiber's function returns the fiber to switch to
//! next (NULL: back to the thread's own context). Returning completes the fiber; it must be initialized again
//! before another switch to it. The SDK's inline queries read that lifecycle in FiberType, not our side table.

#[allow(unused_imports)]
use crate::boundary;
use crate::cpu::Cpu;
#[cfg(any(target_os = "linux", target_os = "emscripten"))]
use host::{ConvertThreadToFiberEx, CreateFiberEx, DeleteFiber, SwitchToFiber};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    ConvertThreadToFiberEx, CreateFiberEx, DeleteFiber, SwitchToFiber,
};

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
    Completed,
}

/// Horizon FiberType's state byte: 0 uninitialized, 1 ready/suspended, 2 running, 4 completed. SDK InitializeFiber,
/// SwitchToFiber and the fiber entry wrapper maintain it at +0x20; clients can read it inline (Bezel's IsCompleted).
unsafe fn guest_state(addr: u64, state: u8) {
    // SAFETY: the caller supplied this writable FiberType to InitializeFiber and has not finalized it yet.
    core::ptr::write_volatile((addr + 0x20) as *mut u8, state);
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
    FIBERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|m| m.get(&addr).cloned())
}

fn resumed() {
    if let Some(f) = LEAVING.with(|l| l.borrow_mut().take()) {
        f.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running = false;
    }
}

/// Runs a fiber's guest function (called by `nxrt_fiber_entry`, the fiber's first frame). When the function returns,
/// the fiber switches to the fiber it names (NULL: the thread's own context), as nn::os does. The SDK wrapper marks
/// it completed before that switch. Restarting it here replays a finished callback and leaves IsCompleted false.
#[no_mangle]
unsafe extern "C" fn nxrt_fiber_run(p: *mut core::ffi::c_void) -> usize {
    crate::boundary::guard("a fiber entry", || {
        resumed();
        let f = Arc::from_raw(p as *const Mutex<Fiber>);
        let (c, func, arg, addr, top) = {
            let g = f.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            (g.cpu, g.func, g.arg, g.addr, (g.stack + g.size) & !15)
        };
        (*c).x[0] = arg;
        (*c).sp = top;
        if crate::boundary::call_guest(c, func) != crate::boundary::GuestCall::Returned {
            crate::rt::guest_fatal(
                c,
                &format!("fiber {addr:#x}: a guest jump or ExitThread left the fiber's function"),
            );
        }
        let next = (*c).x[0];
        if crate::rt::trace("fiber") {
            crate::rt::log(&format!(
                "fiber {addr:#x}: its function returned, next {next:#x}"
            ));
        }
        let n = if next != 0 {
            Some(find(next).unwrap_or_else(|| {
                crate::rt::guest_fatal(
                    c,
                    &format!(
                        "fiber {addr:#x} returned {next:#x}, which is not an initialized fiber"
                    ),
                )
            }))
        } else {
            None
        };
        f.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state = St::Completed;
        guest_state(addr, 4);
        switch_to(c, n);
        crate::rt::guest_fatal(
            c,
            &format!("fiber {addr:#x}: a completed fiber was resumed"),
        );
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
            let mut g = f.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if g.running {
                crate::rt::fatal(&format!(
                    "SwitchToFiber({:#x}): the fiber is running",
                    g.addr
                ));
            }
            if g.state == St::Completed {
                crate::rt::fatal(&format!(
                    "SwitchToFiber({:#x}): the fiber is completed; initialize it before reuse",
                    g.addr
                ));
            }
            g.running = true;
            if g.state == St::New {
                (*g.cpu).sp = (g.stack + g.size) & !15;
                g.state = St::Started;
            }
            guest_state(g.addr, 2);
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
    if let Some(f) = &prev {
        let g = f.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.state != St::Completed {
            guest_state(g.addr, 1);
        }
    }
    LEAVING.with(|l| *l.borrow_mut() = prev);
    host
}

/// A switch from inside a host function: the calling Rust frames are suspended with the fiber, not skipped, and
/// continue when something switches back.
unsafe fn switch_to(from: *const Cpu, to: Option<Arc<Mutex<Fiber>>>) {
    let host = prepare(from, to);
    SwitchToFiber(host as *const _);
    resumed();
    let me = CURRENT
        .with(|c| {
            c.borrow().as_ref().map(|f| {
                f.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .cpu
            })
        })
        .unwrap_or(THREAD_CPU.get());
    (*me).sync_mxcsr();
}

/// InitializeFiber(x0 FiberType*, x1 function, x2 argument, x3 stack, x4 stack size, w5 flags)
unsafe fn initialize(c: *mut Cpu) {
    let c = &mut *c;
    let addr = c.x[0];
    if crate::rt::trace("fiber") {
        crate::rt::log(&format!("fiber {addr:#x}: InitializeFiber function {:#x} argument {:#x} stack {:#x}+{:#x} flags {:#x}", c.x[1], c.x[2], c.x[3], c.x[4], c.x[5]));
    }
    let mut map = FIBERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = map.get_or_insert_with(HashMap::new);
    if let Some(old) = map.get(&addr) {
        let g = old
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.running {
            crate::rt::guest_fatal(
                c,
                &format!("InitializeFiber({addr:#x}): the fiber is running"),
            );
        }
    }
    let cpu = Box::into_raw(Cpu::new_boxed());
    let f = Arc::new(Mutex::new(Fiber {
        addr,
        func: c.x[1],
        arg: c.x[2],
        stack: c.x[3],
        size: c.x[4],
        cpu,
        host: 0,
        state: St::New,
        running: false,
    }));
    #[cfg(windows)]
    let entry: unsafe extern "system" fn(*mut core::ffi::c_void) =
        core::mem::transmute(crate::asm::nxrt_fiber_entry as *const () as usize);
    #[cfg(target_os = "linux")]
    let entry: unsafe extern "C" fn(*mut core::ffi::c_void) = crate::asm::nxrt_fiber_entry;
    #[cfg(target_os = "emscripten")]
    let entry: unsafe extern "C" fn(*mut core::ffi::c_void) = host::never_entered;
    let host = CreateFiberEx(
        64 << 10,
        HOST_STACK,
        FIBER_FLAG_FLOAT_SWITCH,
        Some(entry),
        Arc::into_raw(f.clone()) as *const _,
    );
    if host.is_null() {
        crate::rt::fatal("fiber: cannot create a host fiber");
    }
    f.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .host = host as usize;
    if let Some(old) = map.insert(addr, f) {
        release(
            &old.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
    guest_state(addr, 1);
}

/// FinalizeFiber(x0 FiberType*)
unsafe fn finalize(c: *mut Cpu) {
    let addr = (*c).x[0];
    let f = FIBERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .and_then(|m| m.remove(&addr));
    let Some(f) = f else {
        crate::rt::guest_fatal(
            c,
            &format!("FinalizeFiber({addr:#x}): not an initialized fiber"),
        )
    };
    let g = f.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if g.running {
        crate::rt::guest_fatal(
            c,
            &format!("FinalizeFiber({addr:#x}): the fiber is running"),
        );
    }
    release(&g);
    guest_state(addr, 0);
}

/// Deletes a fiber's host fiber (its host stack). A fiber finalized while suspended inside its function (as Horizon
/// allows: its stack is simply abandoned) still has frames of ours on that stack: the SwitchToFiber host call, the
/// guest call and `nxrt_fiber_run`. None of them holds a lock while suspended (`switch_to` takes none across the
/// switch) and nothing can resume them once the fiber left the table, so the stack is freed under them; what they
/// own (the fiber's `Arc`, its `Cpu`, which a thread record may still name) is leaked, not freed. Keeping every such
/// host fiber cost Super Smash Bros. Ultimate ~100 MB of committed stack a minute (11,471 fibers in 14 min).
unsafe fn release(g: &Fiber) {
    if g.state == St::Started && crate::rt::trace("fiber") {
        crate::rt::log(&format!(
            "fiber {:#x}: finalized while suspended; its host stack is freed",
            g.addr
        ));
    }
    DeleteFiber(g.host as *const _);
}

/// SwitchToFiber(x0 FiberType*; NULL: the thread's own context)
unsafe fn switch(c: *mut Cpu) {
    let addr = (*c).x[0];
    if crate::rt::trace("fiber") {
        let cur = CURRENT.with(|f| {
            f.borrow().as_ref().map_or(0, |f| {
                f.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .addr
            })
        });
        crate::rt::log(&format!(
            "fiber: SwitchToFiber {cur:#x} -> {addr:#x} (lr {:#x})",
            (*c).x[30]
        ));
    }
    if !on_fiber() {
        THREAD_CPU.set(c);
    }
    let to = if addr != 0 {
        Some(find(addr).unwrap_or_else(|| {
            crate::rt::guest_fatal(
                c,
                &format!("SwitchToFiber({addr:#x}): not an initialized fiber"),
            )
        }))
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
    (*c).x[0] = CURRENT.with(|f| {
        f.borrow().as_ref().map_or(0, |f| {
            f.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .addr
        })
    });
}

pub fn register() {
    // SAFETY: written before any fiber exists.
    #[cfg(windows)]
    unsafe {
        crate::asm::NXRT_SWITCH_TO_FIBER = SwitchToFiber as *const () as usize
    };
    crate::hostfn::add_override(SYM_INITIALIZE, initialize);
    crate::hostfn::add_override(SYM_FINALIZE, finalize);
    crate::hostfn::add_override(SYM_SWITCH, switch);
    crate::hostfn::add_override(SYM_CURRENT, current);
}

/// Host fibers on the web: wasm cannot switch stacks yet (no stack-switching in the shipped engines), so a game that
/// starts a guest fiber stops with a clear message (docs/35: refused per title until then). The calls keep the
/// Win32 shape so the rest of this file is shared.
#[cfg(target_os = "emscripten")]
#[allow(non_snake_case)]
mod host {
    fn refuse() -> ! {
        crate::rt::fatal("this game uses nn::os fibers, which web builds cannot run yet (docs/35)")
    }
    pub unsafe fn ConvertThreadToFiberEx(_p: *const core::ffi::c_void, _flags: u32) -> *mut core::ffi::c_void {
        refuse()
    }
    pub unsafe fn CreateFiberEx(
        _commit: usize,
        _reserve: usize,
        _flags: u32,
        _entry: Option<unsafe extern "C" fn(*mut core::ffi::c_void)>,
        _param: *const core::ffi::c_void,
    ) -> *mut core::ffi::c_void {
        refuse()
    }
    pub unsafe fn DeleteFiber(_f: *const core::ffi::c_void) {
        refuse()
    }
    pub unsafe fn SwitchToFiber(_to: *const core::ffi::c_void) {
        refuse()
    }
    pub unsafe extern "C" fn never_entered(_p: *mut core::ffi::c_void) {
        refuse()
    }
}

/// Host fibers on Linux, with the Win32 calls' shape: a fiber is a stack of its own (mmap, a guard page below it) and
/// the stack pointer it was left at; `nxrt_ctx_switch` (asm.rs) saves the callee-saved registers and the FP control
/// state (FIBER_FLAG_FLOAT_SWITCH's: x86-64 MXCSR and the x87 control word, AArch64 FPCR) on the stack it leaves.
#[cfg(target_os = "linux")]
#[allow(non_snake_case)]
mod host {
    use std::cell::Cell;

    pub struct HostFiber {
        sp: usize,
        stack: usize,
        size: usize,
    }

    thread_local! {
        /// The host fiber running on this thread.
        static CUR: Cell<*mut HostFiber> = const { Cell::new(core::ptr::null_mut()) };
    }

    pub unsafe fn ConvertThreadToFiberEx(
        _p: *const core::ffi::c_void,
        _flags: u32,
    ) -> *mut core::ffi::c_void {
        let f = Box::into_raw(Box::new(HostFiber {
            sp: 0,
            stack: 0,
            size: 0,
        }));
        CUR.set(f);
        f as *mut _
    }

    pub unsafe fn CreateFiberEx(
        _commit: usize,
        reserve: usize,
        _flags: u32,
        entry: Option<unsafe extern "C" fn(*mut core::ffi::c_void)>,
        param: *const core::ffi::c_void,
    ) -> *mut core::ffi::c_void {
        const GUARD: usize = 0x1000;
        let size = reserve + GUARD;
        let p = libc::mmap(
            core::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE | libc::MAP_STACK,
            -1,
            0,
        );
        if p == libc::MAP_FAILED {
            return core::ptr::null_mut();
        }
        libc::mprotect(p, GUARD, libc::PROT_NONE);
        let top = (p as usize + size) & !15;
        let sp = initial_frame(top, entry.map_or(0, |e| e as usize as u64), param as u64);
        let f = Box::into_raw(Box::new(HostFiber {
            sp,
            stack: p as usize,
            size,
        }));
        f as *mut _
    }

    /// Writes the frame `nxrt_ctx_switch` resumes a new fiber from below `top` (16-aligned) and returns its sp.
    /// x86-64: the first switch "returns" into the entry with the parameter in rbx (asm.rs nxrt_fiber_entry): top-8 a
    /// null return address, top-16 the entry, then rbp rbx r12 r13 r14 r15 and MXCSR / x87 control word.
    #[cfg(target_arch = "x86_64")]
    pub(super) unsafe fn initial_frame(top: usize, entry: u64, param: u64) -> usize {
        let w = |off: usize, v: u64| *((top - off) as *mut u64) = v;
        w(8, 0);
        w(16, entry);
        w(24, 0); // rbp
        w(32, param); // rbx
        for off in [40, 48, 56, 64] {
            w(off, 0); // r12-r15
        }
        w(72, 0x1F80 | (0x037F << 32)); // MXCSR default, x87 control word default
        top - 72
    }

    /// AArch64: nxrt_ctx_switch's 176-byte frame (CTX_FRAME words), all zero (x29 = 0 ends the frame chain, FPCR 0 is
    /// the default) but x19 = the parameter and x30 = the entry, which its `ret` goes to with sp = `top`.
    #[cfg(target_arch = "aarch64")]
    pub(super) unsafe fn initial_frame(top: usize, entry: u64, param: u64) -> usize {
        const CTX_FRAME: usize = 22;
        let sp = top - CTX_FRAME * 8;
        let f = core::slice::from_raw_parts_mut(sp as *mut u64, CTX_FRAME);
        f.fill(0);
        f[0] = param; // x19
        f[11] = entry; // x30
        sp
    }

    pub unsafe fn DeleteFiber(f: *const core::ffi::c_void) {
        let f = Box::from_raw(f as *mut HostFiber);
        if f.stack != 0 {
            libc::munmap(f.stack as *mut _, f.size);
        }
    }

    pub unsafe fn SwitchToFiber(to: *const core::ffi::c_void) {
        let from = CUR.get();
        let to = to as *mut HostFiber;
        if from.is_null() || from == to {
            return;
        }
        CUR.set(to);
        crate::asm::nxrt_ctx_switch(&mut (*from).sp, (*to).sp);
    }

    /// A fiber's function returned for good (nxrt_fiber_entry): on to the host fiber it named.
    #[no_mangle]
    unsafe extern "C" fn nxrt_fiber_leave(to: usize) {
        SwitchToFiber(to as *const _);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::cpu::Cpu;
        use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

        // The test fiber's first frame, as nxrt_fiber_entry: the parameter from the frame's register to the argument.
        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".globl nxrt_test_fiber_tramp",
            ".p2align 4",
            "nxrt_test_fiber_tramp:",
            "sub rsp, 8",
            "mov rdi, rbx",
            "call nxrt_test_fiber_body",
            "ud2",
        );
        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".globl nxrt_test_fiber_tramp",
            ".p2align 4",
            "nxrt_test_fiber_tramp:",
            "mov x0, x19",
            "bl nxrt_test_fiber_body",
            "brk #0",
        );
        extern "C" {
            fn nxrt_test_fiber_tramp(p: *mut core::ffi::c_void);
        }

        /// Flush-to-zero in the host FP control (MXCSR.FTZ / FPCR.FZ).
        fn host_fz() -> bool {
            #[cfg(target_arch = "x86_64")]
            #[allow(deprecated)]
            // SAFETY: reads MXCSR
            return unsafe { core::arch::x86_64::_mm_getcsr() } & 0x8000 != 0;
            #[cfg(target_arch = "aarch64")]
            {
                let v: u64;
                // SAFETY: reads FPCR
                unsafe { core::arch::asm!("mrs {}, fpcr", out(reg) v, options(nomem, nostack)) };
                v & (1 << 24) != 0
            }
        }

        static BASE_FIBER: AtomicUsize = AtomicUsize::new(0);
        static STEPS: AtomicU32 = AtomicU32::new(0);
        static FZ_KEPT: AtomicU32 = AtomicU32::new(0);

        /// The fiber: turns flush-to-zero on (as a guest with FPCR.FZ does, through `sync_mxcsr`), switches back, and
        /// checks its FP control survived the base context's turn. Never returns (a fiber's first frame has no caller).
        #[no_mangle]
        unsafe extern "C" fn nxrt_test_fiber_body(p: *mut core::ffi::c_void) -> ! {
            let mut c = Cpu::new_boxed();
            c.fpcr = 1 << 24;
            c.sync_mxcsr();
            (*(p as *const AtomicU32)).fetch_add(1, Ordering::SeqCst);
            loop {
                SwitchToFiber(BASE_FIBER.load(Ordering::SeqCst) as *const _);
                FZ_KEPT.store(host_fz() as u32, Ordering::SeqCst);
                STEPS.fetch_add(1, Ordering::SeqCst);
            }
        }

        /// nxrt_ctx_switch and the new fiber's frame: a fiber starts at its entry with its parameter, the switch
        /// keeps each context's callee-saved registers and FP control (MXCSR / FPCR) apart, and a switched-out fiber
        /// resumes where it left off.
        #[test]
        fn host_fiber_switch_keeps_fp_control_apart() {
            std::thread::spawn(|| unsafe {
                let started = AtomicU32::new(0);
                let base = ConvertThreadToFiberEx(core::ptr::null(), 1);
                BASE_FIBER.store(base as usize, Ordering::SeqCst);
                let f = CreateFiberEx(
                    64 << 10,
                    1 << 20,
                    1,
                    Some(nxrt_test_fiber_tramp),
                    &started as *const _ as *const _,
                );
                assert!(!f.is_null());
                assert!(!host_fz());
                let kept = core::hint::black_box(0x1234_5678_u64);
                SwitchToFiber(f);
                assert_eq!(
                    started.load(Ordering::SeqCst),
                    1,
                    "the fiber ran from its entry"
                );
                assert!(
                    !host_fz(),
                    "the fiber's FP control did not leak into the base context"
                );
                assert_eq!(core::hint::black_box(kept), 0x1234_5678);
                for i in 1..=3 {
                    SwitchToFiber(f);
                    assert_eq!(STEPS.load(Ordering::SeqCst), i);
                    assert_eq!(
                        FZ_KEPT.load(Ordering::SeqCst),
                        1,
                        "the fiber's own FP control came back with it"
                    );
                    assert!(!host_fz());
                }
                DeleteFiber(f);
            })
            .join()
            .unwrap();
        }
    }
}
