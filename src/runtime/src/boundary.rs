//! The Rust side of the generated-code boundary (`asm.rs` is the other side).
//!
//! 1. **No panic crosses into C or guest frames.** Every Rust function generated code or a shim calls runs its body
//!    under `guard`: a panic is caught, reported, and ends the process with exit code 70 (the same deterministic exit
//!    as any fatal runtime error). The policy is applied by `guard` everywhere rather than by `panic = "abort"`, so the
//!    report names the boundary it happened at and `cargo test` keeps unwinding test panics.
//! 2. **No non-local jump skips a live Rust frame.** Rust calls guest code only through `call_guest` (the
//!    `nxrt_call_guest` barrier). A guest longjmp or ExitThread aimed below the barrier returns `Escaped`/`Exit` from
//!    `call_guest` instead; the host function must then return at once (its Rust frames unwind normally, Drop runs),
//!    and `nxrt_host_invoke` hands the jump back to the shim, which continues it from generated-code frames only.

use crate::asm::{self, ST_JUMP, ST_REDISPATCH, ST_RETURN};
use crate::cpu::Cpu;
use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Runs `f`; a panic in it ends the process with a report instead of unwinding further.
pub fn guard<R>(what: &str, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(e) => {
            let msg = e
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| e.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(no message)".into());
            crate::rt::fatal(&format!("panic in {what}: {msg}"));
        }
    }
}

/// How a host -> guest call ended.
#[must_use = "a host function must return at once when a guest call does not return"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestCall {
    /// The guest function returned.
    Returned,
    /// A guest jump (longjmp, unwinder resume) left the call for an owner below this host function.
    Escaped,
    /// The guest thread called ExitThread.
    Exit,
}

thread_local! {
    /// A jump the last `call_guest` on this thread left pending: 0 none, 1 escape, 2 exit.
    static PENDING: Cell<u8> = const { Cell::new(0) };
}

/// Calls the guest function at `addr` on `c` from the host (thread entry, callbacks, fibers): x30 is the host-return
/// address, MXCSR follows FPCR. `c->pc` is overwritten.
///
/// # Safety
/// `c` must be the calling thread's current guest state.
pub unsafe fn call_guest(c: *mut Cpu, addr: u64) -> GuestCall {
    (*c).x[30] = crate::hostfn::RET_HOST;
    (*c).pc = addr;
    (*c).sync_mxcsr();
    match asm::nxrt_call_guest(c) {
        0 => GuestCall::Returned,
        1 => {
            PENDING.set(1);
            GuestCall::Escaped
        }
        _ => {
            PENDING.set(2);
            GuestCall::Exit
        }
    }
}

/// Clears what a `call_guest` left pending (for callers that consume the result themselves: thread entry).
pub fn take_pending() -> u8 {
    PENDING.replace(0)
}

/// ExitThread from generated code: the next barrier down (status exit), as a shim status.
///
/// # Safety
/// `c` is the calling thread's guest state.
pub unsafe fn exit_jump(c: *mut Cpu) -> u32 {
    match crate::sjlj::exit_target(c) {
        Some(s) => {
            (*c).sj = s as *mut _;
            ST_JUMP
        }
        None => crate::rt::guest_fatal(c, "ExitThread: no host frame to return to"),
    }
}

/// Called by `nxrt_host_entry` for the host function at `c->pc`.
#[no_mangle]
pub unsafe extern "C" fn nxrt_host_invoke(c: *mut Cpu) -> u32 {
    guard("a host function", || {
        let pc = (*c).pc;
        let Some(f) = crate::hostfn::lookup(pc) else {
            crate::rt::guest_fatal(c, &format!("no host function at {pc:#x}"));
        };
        crate::hostfn::count_call(pc);
        PENDING.set(0);
        f(c);
        match PENDING.replace(0) {
            0 => ST_RETURN,
            1 => ST_REDISPATCH, // c->pc still holds the jump's target
            _ => exit_jump(c),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::GuestFn;
    use crate::dispatch::{Module, Table};
    use std::sync::atomic::{AtomicU32, Ordering};

    // "Lifted code" for the test, in assembly as the real thing is C: no Rust frame between an owner and a jump.
    //   test_owner_body: on re-entry (c->pc == TARGET) sets x0 = 42 and returns; first time it calls the host
    //                    function at c->x[1] through rt_dispatch (as an import's PLT stub does).
    //   test_longjmp:    a guest callback that longjmps: c->pc = TARGET, then BR (rt_dispatch).
    core::arch::global_asm!(
        ".globl nxrt_test_owner_body",
        ".p2align 4",
        ".seh_proc nxrt_test_owner_body",
        "nxrt_test_owner_body:",
        "sub rsp, 40",
        ".seh_stackalloc 40",
        ".seh_endprologue",
        "mov rax, 0x7A0000000108",
        "cmp qword ptr [rcx + 264], rax",
        "jne 2f",
        "mov qword ptr [rcx], 42",
        "add rsp, 40",
        "ret",
        "2:",
        "mov qword ptr [rsp + 32], rcx",
        "mov rax, qword ptr [rcx + 8]",
        "mov qword ptr [rcx + 264], rax",
        "call rt_dispatch",
        "mov rcx, qword ptr [rsp + 32]",
        "mov qword ptr [rcx], 7",   // only if the host function returned: wrong
        "add rsp, 40",
        "ret",
        ".seh_endproc",
        ".globl nxrt_test_longjmp",
        ".p2align 4",
        "nxrt_test_longjmp:",
        "mov rax, 0x7A0000000108",
        "mov qword ptr [rcx + 264], rax",
        "jmp rt_dispatch",
    );
    extern "C" {
        fn nxrt_test_owner_body(c: *mut Cpu);
        fn nxrt_test_longjmp(c: *mut Cpu);
    }

    static DROPS: AtomicU32 = AtomicU32::new(0);
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    static RESULT: std::sync::Mutex<Option<GuestCall>> = std::sync::Mutex::new(None);

    /// A host function that calls a guest callback, holding a value with a destructor across the call.
    unsafe fn host_calls_back(c: *mut Cpu) {
        let _held = Counted;
        let r = call_guest(c, 0x7A00_0000_0200);
        *RESULT.lock().unwrap() = Some(r);
        if r != GuestCall::Returned {
            return;
        }
        panic!("the callback returned");
    }

    /// A guest callback invoked from a host function longjmps past it to a setjmp owner below. The host function's
    /// frame is not skipped: its destructor runs, it sees `Escaped`, and the jump then lands in the owner.
    #[test]
    fn longjmp_out_of_a_host_callback_keeps_rust_frames() {
        let leak = |v: Vec<u64>| -> &'static [u64] { Box::leak(v.into_boxed_slice()) };
        let fns: &'static [GuestFn] = Box::leak(vec![nxrt_test_longjmp as GuestFn].into_boxed_slice());
        let t = Table::build(vec![Module { name: "test".into(), base: 0x7A00_0000_0000, size: 0x1000, addrs: leak(vec![0x7A00_0000_0200]), fns }]).unwrap();
        crate::dispatch::install(t).unwrap();
        let host = crate::hostfn::register("test_host_calls_back", host_calls_back);
        let mut c = Cpu::new_boxed();
        c.x[1] = host;
        c.pc = 0x7A00_0000_0100;
        unsafe { asm::rt_sj_owner(&mut *c, nxrt_test_owner_body, 0x7A00_0000_0100, 0x7A00_0000_0180) };
        assert_eq!(c.x[0], 42, "the jump landed in the owner");
        assert_eq!(DROPS.load(Ordering::SeqCst), 1, "the host function's frame unwound normally");
        assert_eq!(*RESULT.lock().unwrap(), Some(GuestCall::Escaped));
        assert!(c.sj.is_null(), "owner and barrier both unlinked");
    }

    unsafe fn host_panics(_c: *mut Cpu) {
        panic!("deliberate test panic");
    }

    /// A panic inside a host function never unwinds into the caller's (generated) frames: the process ends with a
    /// report and exit code 70. Run in a child process, since it ends the process.
    #[test]
    fn panic_in_host_function_exits_cleanly() {
        if std::env::var("NXRT_PANIC_CHILD").is_ok() {
            let a = crate::hostfn::register("test_host_panics", host_panics);
            let mut c = Cpu::new_boxed();
            c.pc = a;
            unsafe { asm::nxrt_host_entry(&mut *c) };
            std::process::exit(0); // not reached
        }
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args(["--exact", "boundary::tests::panic_in_host_function_exits_cleanly", "--nocapture"])
            .env("NXRT_PANIC_CHILD", "1")
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(70), "{err}");
        assert!(err.contains("panic in a host function: deliberate test panic"), "{err}");
    }
}
