//! The machine-code boundary between generated code and Rust (x86-64, Windows x64 ABI). Every function here has SEH
//! unwind info, so host stack walks (crash reports, `fault.rs`) pass through.
//!
//! The rule this layer enforces: **no non-local jump ever skips a live Rust frame.** Rust code reached from generated
//! code is always entered through a shim here that *calls* it, lets it return, and only then performs whatever jump
//! it asked for; and Rust code calls guest code only through a barrier frame here. So every frame a jump discards is
//! generated C or one of these shims.
//!
//! * `rt_dispatch`: the indirect-branch entry of generated code, a pure tail jump (chains of indirect jumps keep the
//!   host stack flat). Misses go to `nxrt_dispatch_slow`, which calls Rust `nxrt_dispatch_resolve` and jumps to what
//!   it returns: a lifted function, `nxrt_host_entry` for a host function, or `nxrt_jump_pending` for a guest jump to
//!   a live owner or barrier.
//! * `rt_sj_owner`: the frame of a guest function a guest longjmp (or an unwinder's resume) may enter in the middle.
//!   Its slot in the thread's chain (`Cpu::sj`) holds the code range; a jump back re-runs the body, which continues at
//!   `c->pc`.
//! * `nxrt_call_guest`: the barrier a host -> guest call runs under, a slot with an empty range in the same chain. A
//!   guest jump aimed below it (or ExitThread) lands here first: the call returns a status to its Rust caller, which
//!   returns normally, and the shim it was called through carries the jump on.
//! * `rt_svc` and `nxrt_host_entry`: call `nxrt_svc_impl` / `nxrt_host_invoke` and act on their status: 0 return, 1
//!   jump to `c->pc` again (`rt_dispatch`), 2 take the pending jump (`c->sj` names the target slot).
//! * `nxrt_fiber_entry`: a fiber's first frame; Rust runs the fiber's guest function and returns the host fiber to
//!   switch to, so a finished fiber's stack holds no Rust frame when it is deleted.
//!
//! They restore the host's nonvolatile registers (rbx, rbp, rsi, rdi, r12-r15, xmm6-xmm15) of the frame jumped to;
//! guest registers live in `Cpu`.

use crate::cpu::Cpu;

macro_rules! frame_asm {
    // The common frame of rt_sj_owner and nxrt_call_guest: 8 pushes, 264 bytes: shadow space at rsp+0, the SjSlot
    // at rsp+32..96, xmm6-15 at rsp+96..256.
    (prologue $name:literal) => {
        concat!(
            ".seh_proc ", $name, "\n", $name, ":\n",
            "push rbp\n.seh_pushreg rbp\npush rbx\n.seh_pushreg rbx\npush rsi\n.seh_pushreg rsi\npush rdi\n.seh_pushreg rdi\n",
            "push r12\n.seh_pushreg r12\npush r13\n.seh_pushreg r13\npush r14\n.seh_pushreg r14\npush r15\n.seh_pushreg r15\n",
            "sub rsp, 264\n.seh_stackalloc 264\n",
            "movaps xmmword ptr [rsp + 96], xmm6\n.seh_savexmm xmm6, 96\n",
            "movaps xmmword ptr [rsp + 112], xmm7\n.seh_savexmm xmm7, 112\n",
            "movaps xmmword ptr [rsp + 128], xmm8\n.seh_savexmm xmm8, 128\n",
            "movaps xmmword ptr [rsp + 144], xmm9\n.seh_savexmm xmm9, 144\n",
            "movaps xmmword ptr [rsp + 160], xmm10\n.seh_savexmm xmm10, 160\n",
            "movaps xmmword ptr [rsp + 176], xmm11\n.seh_savexmm xmm11, 176\n",
            "movaps xmmword ptr [rsp + 192], xmm12\n.seh_savexmm xmm12, 192\n",
            "movaps xmmword ptr [rsp + 208], xmm13\n.seh_savexmm xmm13, 208\n",
            "movaps xmmword ptr [rsp + 224], xmm14\n.seh_savexmm xmm14, 224\n",
            "movaps xmmword ptr [rsp + 240], xmm15\n.seh_savexmm xmm15, 240\n",
            ".seh_endprologue\n",
        )
    };
    (epilogue) => {
        concat!(
            "movaps xmm6, xmmword ptr [rsp + 96]\nmovaps xmm7, xmmword ptr [rsp + 112]\n",
            "movaps xmm8, xmmword ptr [rsp + 128]\nmovaps xmm9, xmmword ptr [rsp + 144]\n",
            "movaps xmm10, xmmword ptr [rsp + 160]\nmovaps xmm11, xmmword ptr [rsp + 176]\n",
            "movaps xmm12, xmmword ptr [rsp + 192]\nmovaps xmm13, xmmword ptr [rsp + 208]\n",
            "movaps xmm14, xmmword ptr [rsp + 224]\nmovaps xmm15, xmmword ptr [rsp + 240]\n",
            "add rsp, 264\npop r15\npop r14\npop r13\npop r12\npop rdi\npop rsi\npop rbx\npop rbp\nret\n.seh_endproc\n",
        )
    };
}

core::arch::global_asm!(
    ".text",
    // void rt_dispatch(Cpu *c): jump to the lifted function for c->pc.
    ".globl rt_dispatch",
    ".p2align 4",
    "rt_dispatch:",
    "mov rax, qword ptr [rcx + 264]",
    "sub rax, qword ptr [rip + NXRT_DISPATCH]",
    "ror rax, 2",
    "cmp rax, qword ptr [rip + NXRT_DISPATCH + 8]",
    "jae 2f",
    "mov rdx, qword ptr [rip + NXRT_DISPATCH + 16]",
    "mov eax, dword ptr [rdx + rax * 4]",
    "test eax, eax",
    "jz 2f",
    "add rax, qword ptr [rip + NXRT_DISPATCH + 24]",
    "jmp rax",
    "2:",
    "jmp nxrt_dispatch_slow",
    "",
    ".globl nxrt_dispatch_slow",
    ".p2align 4",
    ".seh_proc nxrt_dispatch_slow",
    "nxrt_dispatch_slow:",
    "push rcx",
    ".seh_pushreg rcx",
    "sub rsp, 32",
    ".seh_stackalloc 32",
    ".seh_endprologue",
    "call nxrt_dispatch_resolve",
    "add rsp, 32",
    "pop rcx",
    "jmp rax",
    ".seh_endproc",
    "",
    // void rt_sj_owner(Cpu *c, GuestFn body, uint64_t lo, uint64_t hi)
    ".globl rt_sj_owner",
    ".p2align 4",
    frame_asm!(prologue "rt_sj_owner"),
    "mov rax, qword ptr [rcx + 344]",
    "mov qword ptr [rsp + 32], rax",     // up
    "mov qword ptr [rsp + 40], r8",      // lo
    "mov qword ptr [rsp + 48], r9",      // hi
    "mov qword ptr [rsp + 56], rsp",     // rsp
    "mov qword ptr [rsp + 64], rcx",     // c
    "mov qword ptr [rsp + 72], rdx",     // body
    "lea rax, [rip + 3f]",
    "mov qword ptr [rsp + 80], rax",     // resume
    "mov qword ptr [rsp + 88], 0",       // status
    "lea rax, [rsp + 32]",
    "mov qword ptr [rcx + 344], rax",
    "3:",
    "mov rcx, qword ptr [rsp + 64]",
    "call qword ptr [rsp + 72]",
    "mov rcx, qword ptr [rsp + 64]",
    "mov rax, qword ptr [rsp + 32]",
    "mov qword ptr [rcx + 344], rax",
    frame_asm!(epilogue),
    "",
    // uint64_t nxrt_call_guest(Cpu *c): rt_dispatch(c) (c->pc, x30 set by the caller) under a barrier slot.
    // Returns 0 when the guest function returns, else the status a jump to the barrier carried (1 escape, 2 exit).
    ".globl nxrt_call_guest",
    ".p2align 4",
    frame_asm!(prologue "nxrt_call_guest"),
    "mov rax, qword ptr [rcx + 344]",
    "mov qword ptr [rsp + 32], rax",
    "mov qword ptr [rsp + 40], 0",
    "mov qword ptr [rsp + 48], 0",
    "mov qword ptr [rsp + 56], rsp",
    "mov qword ptr [rsp + 64], rcx",
    "mov qword ptr [rsp + 72], 0",
    "lea rax, [rip + 5f]",
    "mov qword ptr [rsp + 80], rax",
    "mov qword ptr [rsp + 88], 0",
    "lea rax, [rsp + 32]",
    "mov qword ptr [rcx + 344], rax",
    "call rt_dispatch",
    "xor eax, eax",
    "5:",
    "mov rcx, qword ptr [rsp + 64]",
    "mov rdx, qword ptr [rsp + 32]",
    "mov qword ptr [rcx + 344], rdx",
    frame_asm!(epilogue),
    "",
    // noreturn void nxrt_sj_jump(SjSlot *slot): into the slot's frame at its resume address, rax = its status.
    // Only generated code and these shims lie above the slot (Rust frames above it have returned).
    ".globl nxrt_sj_jump",
    ".p2align 4",
    "nxrt_sj_jump:",
    "mov rsp, qword ptr [rcx + 24]",
    "mov rax, qword ptr [rcx + 56]",
    "jmp qword ptr [rcx + 48]",
    "",
    // The pending jump: c->sj names the target slot (set by Rust, which has returned).
    ".globl nxrt_jump_pending",
    ".p2align 4",
    "nxrt_jump_pending:",
    "mov rcx, qword ptr [rcx + 344]",
    "jmp nxrt_sj_jump",
    "",
    // void rt_svc(Cpu *c, uint32_t imm, uint64_t pc)
    ".globl rt_svc",
    ".p2align 4",
    ".seh_proc rt_svc",
    "rt_svc:",
    "push rcx",
    ".seh_pushreg rcx",
    "sub rsp, 32",
    ".seh_stackalloc 32",
    ".seh_endprologue",
    "call nxrt_svc_impl",
    "add rsp, 32",
    "pop rcx",
    "test eax, eax",
    "jnz 6f",
    "ret",
    "6:",
    "cmp eax, 1",
    "je rt_dispatch",
    "jmp nxrt_jump_pending",
    ".seh_endproc",
    "",
    // A host function at c->pc, called like a lifted function.
    ".globl nxrt_host_entry",
    ".p2align 4",
    ".seh_proc nxrt_host_entry",
    "nxrt_host_entry:",
    "push rcx",
    ".seh_pushreg rcx",
    "sub rsp, 32",
    ".seh_stackalloc 32",
    ".seh_endprologue",
    "call nxrt_host_invoke",
    "add rsp, 32",
    "pop rcx",
    "test eax, eax",
    "jnz 7f",
    "ret",
    "7:",
    "cmp eax, 1",
    "je rt_dispatch",
    "jmp nxrt_jump_pending",
    ".seh_endproc",
    "",
    // void WINAPI nxrt_fiber_entry(void *param): the first frame of a guest fiber's host fiber.
    ".globl nxrt_fiber_entry",
    ".p2align 4",
    ".seh_proc nxrt_fiber_entry",
    "nxrt_fiber_entry:",
    "sub rsp, 40",
    ".seh_stackalloc 40",
    ".seh_endprologue",
    "call nxrt_fiber_run",
    "mov rcx, rax",
    "call qword ptr [rip + NXRT_SWITCH_TO_FIBER]",
    "call nxrt_fiber_resumed_dead",
    "int3",
    ".seh_endproc",
);

extern "C" {
    pub fn rt_dispatch(c: *mut Cpu);
    pub fn rt_sj_owner(c: *mut Cpu, body: crate::cpu::GuestFn, lo: u64, hi: u64);
    pub fn nxrt_call_guest(c: *mut Cpu) -> u64;
    pub fn nxrt_host_entry(c: *mut Cpu);
    pub fn nxrt_jump_pending(c: *mut Cpu);
    pub fn rt_svc(c: *mut Cpu, imm: u32, pc: u64);
    pub fn nxrt_fiber_entry(param: *mut core::ffi::c_void);
}

/// Status codes of the shims (`nxrt_svc_impl`, `nxrt_host_invoke`).
pub const ST_RETURN: u32 = 0;
pub const ST_REDISPATCH: u32 = 1;
pub const ST_JUMP: u32 = 2;
/// A barrier's status after a jump: a guest jump escaped past the host function above it, or the thread exits.
pub const BARRIER_ESCAPE: u64 = 1;
pub const BARRIER_EXIT: u64 = 2;

/// A frame in the thread's chain (`Cpu::sj`): a setjmp owner (lo < hi) or a host -> guest barrier (lo = hi = 0).
#[repr(C)]
pub struct SjSlot {
    pub up: *mut SjSlot,
    pub lo: u64,
    pub hi: u64,
    pub rsp: u64,
    pub c: *mut Cpu,
    pub body: usize,
    pub resume: usize,
    pub status: u64,
}

impl SjSlot {
    pub fn is_barrier(&self) -> bool {
        self.lo == 0 && self.hi == 0
    }
}

/// The hot dispatch data `rt_dispatch` reads: one cache line, written once before any guest thread runs.
#[repr(C, align(64))]
pub struct DispatchHot {
    pub lo: u64,
    pub slots: u64,
    pub slot: *const u32,
    pub anchor: u64,
}

#[no_mangle]
pub static mut NXRT_DISPATCH: DispatchHot = DispatchHot { lo: 0, slots: 0, slot: core::ptr::null(), anchor: 0 };

/// SwitchToFiber, for `nxrt_fiber_entry` (set by `fiber.rs`).
#[no_mangle]
pub static mut NXRT_SWITCH_TO_FIBER: usize = 0;

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn slot_layout() {
        assert_eq!(offset_of!(SjSlot, rsp), 24);
        assert_eq!(offset_of!(SjSlot, resume), 48);
        assert_eq!(offset_of!(SjSlot, status), 56);
        assert_eq!(core::mem::size_of::<SjSlot>(), 64);
        assert_eq!(offset_of!(DispatchHot, slots), 8);
        assert_eq!(offset_of!(DispatchHot, slot), 16);
        assert_eq!(offset_of!(DispatchHot, anchor), 24);
    }
}
