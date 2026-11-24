//! The machine-code boundary between generated code and Rust: x86-64 (Windows x64 ABI, System V on Linux) and AArch64
//! Linux (AAPCS64). The Windows functions have SEH unwind info and the Linux ones CFI, so host stack walks (crash
//! reports, `fault.rs`) pass through.
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

#[cfg(windows)]
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

#[cfg(windows)]
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
    "mov qword ptr [rsp + 32], rax", // up
    "mov qword ptr [rsp + 40], r8",  // lo
    "mov qword ptr [rsp + 48], r9",  // hi
    "mov qword ptr [rsp + 56], rsp", // rsp
    "mov qword ptr [rsp + 64], rcx", // c
    "mov qword ptr [rsp + 72], rdx", // body
    "lea rax, [rip + 3f]",
    "mov qword ptr [rsp + 80], rax", // resume
    "mov qword ptr [rsp + 88], 0",   // status
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

// The same boundary for the System V ABI (Linux): arguments in rdi, rsi, rdx, rcx; no shadow space is needed, but the
// frame of rt_sj_owner / nxrt_call_guest keeps the Windows layout (the SjSlot at rsp+32) so `SjSlot` is one type.
// The frames save rbx, rbp, r12-r15 (the System V callee-saved set) and, harmlessly, rsi, rdi and xmm6-15 as well.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
macro_rules! frame_asm_sysv {
    (prologue $name:literal) => {
        concat!(
            ".type ", $name, ",@function\n", $name, ":\n",
            ".cfi_startproc\n",
            "push rbp\n.cfi_adjust_cfa_offset 8\n.cfi_offset rbp, -16\n",
            "push rbx\n.cfi_adjust_cfa_offset 8\n.cfi_offset rbx, -24\n",
            "push rsi\n.cfi_adjust_cfa_offset 8\npush rdi\n.cfi_adjust_cfa_offset 8\n",
            "push r12\n.cfi_adjust_cfa_offset 8\n.cfi_offset r12, -48\n",
            "push r13\n.cfi_adjust_cfa_offset 8\n.cfi_offset r13, -56\n",
            "push r14\n.cfi_adjust_cfa_offset 8\n.cfi_offset r14, -64\n",
            "push r15\n.cfi_adjust_cfa_offset 8\n.cfi_offset r15, -72\n",
            "sub rsp, 264\n.cfi_adjust_cfa_offset 264\n",
            "movaps xmmword ptr [rsp + 96], xmm6\nmovaps xmmword ptr [rsp + 112], xmm7\n",
            "movaps xmmword ptr [rsp + 128], xmm8\nmovaps xmmword ptr [rsp + 144], xmm9\n",
            "movaps xmmword ptr [rsp + 160], xmm10\nmovaps xmmword ptr [rsp + 176], xmm11\n",
            "movaps xmmword ptr [rsp + 192], xmm12\nmovaps xmmword ptr [rsp + 208], xmm13\n",
            "movaps xmmword ptr [rsp + 224], xmm14\nmovaps xmmword ptr [rsp + 240], xmm15\n",
        )
    };
    (epilogue $name:literal) => {
        concat!(
            "movaps xmm6, xmmword ptr [rsp + 96]\nmovaps xmm7, xmmword ptr [rsp + 112]\n",
            "movaps xmm8, xmmword ptr [rsp + 128]\nmovaps xmm9, xmmword ptr [rsp + 144]\n",
            "movaps xmm10, xmmword ptr [rsp + 160]\nmovaps xmm11, xmmword ptr [rsp + 176]\n",
            "movaps xmm12, xmmword ptr [rsp + 192]\nmovaps xmm13, xmmword ptr [rsp + 208]\n",
            "movaps xmm14, xmmword ptr [rsp + 224]\nmovaps xmm15, xmmword ptr [rsp + 240]\n",
            "add rsp, 264\npop r15\npop r14\npop r13\npop r12\npop rdi\npop rsi\npop rbx\npop rbp\nret\n",
            ".cfi_endproc\n.size ", $name, ", .-", $name, "\n",
        )
    };
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
core::arch::global_asm!(
    ".text",
    // void rt_dispatch(Cpu *c): jump to the lifted function for c->pc.
    ".globl rt_dispatch",
    ".type rt_dispatch,@function",
    ".p2align 4",
    "rt_dispatch:",
    "mov rax, qword ptr [rdi + 264]",
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
    ".size rt_dispatch, .-rt_dispatch",
    "",
    ".globl nxrt_dispatch_slow",
    ".type nxrt_dispatch_slow,@function",
    ".p2align 4",
    "nxrt_dispatch_slow:",
    ".cfi_startproc",
    "push rdi",
    ".cfi_adjust_cfa_offset 8",
    "call nxrt_dispatch_resolve",
    "pop rdi",
    ".cfi_adjust_cfa_offset -8",
    "jmp rax",
    ".cfi_endproc",
    ".size nxrt_dispatch_slow, .-nxrt_dispatch_slow",
    "",
    // void rt_sj_owner(Cpu *c, GuestFn body, uint64_t lo, uint64_t hi)
    ".globl rt_sj_owner",
    ".p2align 4",
    frame_asm_sysv!(prologue "rt_sj_owner"),
    "mov rax, qword ptr [rdi + 344]",
    "mov qword ptr [rsp + 32], rax", // up
    "mov qword ptr [rsp + 40], rdx", // lo
    "mov qword ptr [rsp + 48], rcx", // hi
    "mov qword ptr [rsp + 56], rsp", // rsp
    "mov qword ptr [rsp + 64], rdi", // c
    "mov qword ptr [rsp + 72], rsi", // body
    "lea rax, [rip + 3f]",
    "mov qword ptr [rsp + 80], rax", // resume
    "mov qword ptr [rsp + 88], 0",   // status
    "lea rax, [rsp + 32]",
    "mov qword ptr [rdi + 344], rax",
    "3:",
    "mov rdi, qword ptr [rsp + 64]",
    "call qword ptr [rsp + 72]",
    "mov rdi, qword ptr [rsp + 64]",
    "mov rax, qword ptr [rsp + 32]",
    "mov qword ptr [rdi + 344], rax",
    frame_asm_sysv!(epilogue "rt_sj_owner"),
    "",
    // uint64_t nxrt_call_guest(Cpu *c)
    ".globl nxrt_call_guest",
    ".p2align 4",
    frame_asm_sysv!(prologue "nxrt_call_guest"),
    "mov rax, qword ptr [rdi + 344]",
    "mov qword ptr [rsp + 32], rax",
    "mov qword ptr [rsp + 40], 0",
    "mov qword ptr [rsp + 48], 0",
    "mov qword ptr [rsp + 56], rsp",
    "mov qword ptr [rsp + 64], rdi",
    "mov qword ptr [rsp + 72], 0",
    "lea rax, [rip + 5f]",
    "mov qword ptr [rsp + 80], rax",
    "mov qword ptr [rsp + 88], 0",
    "lea rax, [rsp + 32]",
    "mov qword ptr [rdi + 344], rax",
    "call rt_dispatch",
    "xor eax, eax",
    "5:",
    "mov rdi, qword ptr [rsp + 64]",
    "mov rdx, qword ptr [rsp + 32]",
    "mov qword ptr [rdi + 344], rdx",
    frame_asm_sysv!(epilogue "nxrt_call_guest"),
    "",
    // noreturn void nxrt_sj_jump(SjSlot *slot)
    ".globl nxrt_sj_jump",
    ".type nxrt_sj_jump,@function",
    ".p2align 4",
    "nxrt_sj_jump:",
    "mov rsp, qword ptr [rdi + 24]",
    "mov rax, qword ptr [rdi + 56]",
    "jmp qword ptr [rdi + 48]",
    ".size nxrt_sj_jump, .-nxrt_sj_jump",
    "",
    ".globl nxrt_jump_pending",
    ".type nxrt_jump_pending,@function",
    ".p2align 4",
    "nxrt_jump_pending:",
    "mov rdi, qword ptr [rdi + 344]",
    "jmp nxrt_sj_jump",
    ".size nxrt_jump_pending, .-nxrt_jump_pending",
    "",
    // void rt_svc(Cpu *c, uint32_t imm, uint64_t pc): its arguments are nxrt_svc_impl's already
    ".globl rt_svc",
    ".type rt_svc,@function",
    ".p2align 4",
    "rt_svc:",
    ".cfi_startproc",
    "push rdi",
    ".cfi_adjust_cfa_offset 8",
    "call nxrt_svc_impl",
    "pop rdi",
    ".cfi_adjust_cfa_offset -8",
    "test eax, eax",
    "jnz 6f",
    "ret",
    "6:",
    "cmp eax, 1",
    "je rt_dispatch",
    "jmp nxrt_jump_pending",
    ".cfi_endproc",
    ".size rt_svc, .-rt_svc",
    "",
    ".globl nxrt_host_entry",
    ".type nxrt_host_entry,@function",
    ".p2align 4",
    "nxrt_host_entry:",
    ".cfi_startproc",
    "push rdi",
    ".cfi_adjust_cfa_offset 8",
    "call nxrt_host_invoke",
    "pop rdi",
    ".cfi_adjust_cfa_offset -8",
    "test eax, eax",
    "jnz 7f",
    "ret",
    "7:",
    "cmp eax, 1",
    "je rt_dispatch",
    "jmp nxrt_jump_pending",
    ".cfi_endproc",
    ".size nxrt_host_entry, .-nxrt_host_entry",
    "",
    // A host fiber's first frame (fiber.rs builds its stack so that nxrt_ctx_switch returns here with the fiber's
    // parameter in rbx): runs the fiber's guest function, then switches to the host fiber it returns for good.
    ".globl nxrt_fiber_entry",
    ".type nxrt_fiber_entry,@function",
    ".p2align 4",
    "nxrt_fiber_entry:",
    "sub rsp, 8",
    "mov rdi, rbx",
    "call nxrt_fiber_run",
    "mov rdi, rax",
    "call nxrt_fiber_leave",
    "call nxrt_fiber_resumed_dead",
    "ud2",
    ".size nxrt_fiber_entry, .-nxrt_fiber_entry",
    "",
    // void nxrt_ctx_switch(size_t *save, size_t to)
    ".globl nxrt_ctx_switch",
    ".type nxrt_ctx_switch,@function",
    ".p2align 4",
    "nxrt_ctx_switch:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "sub rsp, 8",
    "stmxcsr dword ptr [rsp]",
    "fnstcw word ptr [rsp + 4]",
    "mov qword ptr [rdi], rsp",
    "mov rsp, rsi",
    "ldmxcsr dword ptr [rsp]",
    "fldcw word ptr [rsp + 4]",
    "add rsp, 8",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
    ".size nxrt_ctx_switch, .-nxrt_ctx_switch",
);

// The same boundary for AArch64 Linux (AAPCS64): `Cpu *` in x0, the return address in x30, x9-x15 free scratch at
// every entry here (each is a call or a tail jump from generated code). The frame of rt_sj_owner / nxrt_call_guest
// (224 bytes) saves the callee-saved set and holds the SjSlot: 0 x29 x30 (the frame record), 16 x19-x28, 96 d8-d15,
// 160 the SjSlot (`rsp` is the frame's sp). A jump to a slot loads its sp and branches to its resume address with the
// status in x0; the epilogue then restores x19-x30 and d8-d15 from the frame, so whatever the generated code between
// the jump and the slot left in them does not matter.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
macro_rules! frame_asm_a64 {
    (prologue $name:literal) => {
        concat!(
            ".type ", $name, ",%function\n", $name, ":\n",
            ".cfi_startproc\n",
            "sub sp, sp, #224\n.cfi_def_cfa_offset 224\n",
            "stp x29, x30, [sp]\n",
            "stp x19, x20, [sp, #16]\nstp x21, x22, [sp, #32]\nstp x23, x24, [sp, #48]\n",
            "stp x25, x26, [sp, #64]\nstp x27, x28, [sp, #80]\n",
            "stp d8, d9, [sp, #96]\nstp d10, d11, [sp, #112]\nstp d12, d13, [sp, #128]\nstp d14, d15, [sp, #144]\n",
            ".cfi_offset w29, -224\n.cfi_offset w30, -216\n",
            ".cfi_offset w19, -208\n.cfi_offset w20, -200\n.cfi_offset w21, -192\n.cfi_offset w22, -184\n",
            ".cfi_offset w23, -176\n.cfi_offset w24, -168\n.cfi_offset w25, -160\n.cfi_offset w26, -152\n",
            ".cfi_offset w27, -144\n.cfi_offset w28, -136\n",
            ".cfi_offset b8, -128\n.cfi_offset b9, -120\n.cfi_offset b10, -112\n.cfi_offset b11, -104\n",
            ".cfi_offset b12, -96\n.cfi_offset b13, -88\n.cfi_offset b14, -80\n.cfi_offset b15, -72\n",
            "mov x29, sp\n",
        )
    };
    (epilogue $name:literal) => {
        concat!(
            "ldp x19, x20, [sp, #16]\nldp x21, x22, [sp, #32]\nldp x23, x24, [sp, #48]\n",
            "ldp x25, x26, [sp, #64]\nldp x27, x28, [sp, #80]\n",
            "ldp d8, d9, [sp, #96]\nldp d10, d11, [sp, #112]\nldp d12, d13, [sp, #128]\nldp d14, d15, [sp, #144]\n",
            "ldp x29, x30, [sp]\n",
            "add sp, sp, #224\n.cfi_def_cfa_offset 0\n",
            "ret\n",
            ".cfi_endproc\n.size ", $name, ", .-", $name, "\n",
        )
    };
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
core::arch::global_asm!(
    ".text",
    // void rt_dispatch(Cpu *c): jump to the lifted function for c->pc (x30 untouched: it is the target's return).
    ".globl rt_dispatch",
    ".type rt_dispatch,%function",
    ".p2align 4",
    "rt_dispatch:",
    "adrp x9, NXRT_DISPATCH",
    "add x9, x9, :lo12:NXRT_DISPATCH",
    "ldr x10, [x0, #264]",
    "ldr x11, [x9]",
    "sub x10, x10, x11",
    "ror x10, x10, #2",
    "ldr x11, [x9, #8]",
    "cmp x10, x11",
    "b.hs 2f",
    "ldr x11, [x9, #16]",
    "ldr w12, [x11, x10, lsl #2]",
    "cbz w12, 2f",
    "ldr x11, [x9, #24]",
    "add x12, x12, x11",
    "br x12",
    "2:",
    "b nxrt_dispatch_slow",
    ".size rt_dispatch, .-rt_dispatch",
    "",
    ".globl nxrt_dispatch_slow",
    ".type nxrt_dispatch_slow,%function",
    ".p2align 4",
    "nxrt_dispatch_slow:",
    ".cfi_startproc",
    "stp x0, x30, [sp, #-16]!",
    ".cfi_def_cfa_offset 16",
    ".cfi_offset w30, -8",
    "bl nxrt_dispatch_resolve",
    "mov x9, x0",
    "ldp x0, x30, [sp], #16",
    ".cfi_def_cfa_offset 0",
    ".cfi_restore w30",
    "br x9",
    ".cfi_endproc",
    ".size nxrt_dispatch_slow, .-nxrt_dispatch_slow",
    "",
    // void rt_sj_owner(Cpu *c, GuestFn body, uint64_t lo, uint64_t hi)
    ".globl rt_sj_owner",
    ".p2align 4",
    frame_asm_a64!(prologue "rt_sj_owner"),
    "ldr x9, [x0, #344]",
    "str x9, [sp, #160]",     // up
    "stp x2, x3, [sp, #168]", // lo, hi
    "mov x9, sp",
    "str x9, [sp, #184]",     // rsp (the frame's sp)
    "stp x0, x1, [sp, #192]", // c, body
    "adr x9, 3f",
    "str x9, [sp, #208]",  // resume
    "str xzr, [sp, #216]", // status
    "add x9, sp, #160",
    "str x9, [x0, #344]",
    "3:",
    "mov x29, sp",
    "ldp x0, x9, [sp, #192]",
    "blr x9",
    "ldr x0, [sp, #192]",
    "ldr x9, [sp, #160]",
    "str x9, [x0, #344]",
    frame_asm_a64!(epilogue "rt_sj_owner"),
    "",
    // uint64_t nxrt_call_guest(Cpu *c)
    ".globl nxrt_call_guest",
    ".p2align 4",
    frame_asm_a64!(prologue "nxrt_call_guest"),
    "ldr x9, [x0, #344]",
    "str x9, [sp, #160]",
    "stp xzr, xzr, [sp, #168]",
    "mov x9, sp",
    "str x9, [sp, #184]",
    "stp x0, xzr, [sp, #192]",
    "adr x9, 5f",
    "str x9, [sp, #208]",
    "str xzr, [sp, #216]",
    "add x9, sp, #160",
    "str x9, [x0, #344]",
    "bl rt_dispatch",
    "mov x0, #0",
    "5:",
    "ldr x1, [sp, #192]",
    "ldr x9, [sp, #160]",
    "str x9, [x1, #344]",
    frame_asm_a64!(epilogue "nxrt_call_guest"),
    "",
    // noreturn void nxrt_sj_jump(SjSlot *slot): into the slot's frame at its resume address, x0 = its status.
    ".globl nxrt_sj_jump",
    ".type nxrt_sj_jump,%function",
    ".p2align 4",
    "nxrt_sj_jump:",
    "ldr x9, [x0, #24]",
    "mov sp, x9",
    "ldr x10, [x0, #48]",
    "ldr x0, [x0, #56]",
    "br x10",
    ".size nxrt_sj_jump, .-nxrt_sj_jump",
    "",
    ".globl nxrt_jump_pending",
    ".type nxrt_jump_pending,%function",
    ".p2align 4",
    "nxrt_jump_pending:",
    "ldr x0, [x0, #344]",
    "b nxrt_sj_jump",
    ".size nxrt_jump_pending, .-nxrt_jump_pending",
    "",
    // void rt_svc(Cpu *c, uint32_t imm, uint64_t pc): its arguments are nxrt_svc_impl's already
    ".globl rt_svc",
    ".type rt_svc,%function",
    ".p2align 4",
    "rt_svc:",
    ".cfi_startproc",
    "stp x0, x30, [sp, #-16]!",
    ".cfi_def_cfa_offset 16",
    ".cfi_offset w30, -8",
    "bl nxrt_svc_impl",
    "mov w9, w0",
    "ldp x0, x30, [sp], #16",
    ".cfi_def_cfa_offset 0",
    ".cfi_restore w30",
    "cbnz w9, 6f",
    "ret",
    "6:",
    "cmp w9, #1",
    "b.eq rt_dispatch",
    "b nxrt_jump_pending",
    ".cfi_endproc",
    ".size rt_svc, .-rt_svc",
    "",
    ".globl nxrt_host_entry",
    ".type nxrt_host_entry,%function",
    ".p2align 4",
    "nxrt_host_entry:",
    ".cfi_startproc",
    "stp x0, x30, [sp, #-16]!",
    ".cfi_def_cfa_offset 16",
    ".cfi_offset w30, -8",
    "bl nxrt_host_invoke",
    "mov w9, w0",
    "ldp x0, x30, [sp], #16",
    ".cfi_def_cfa_offset 0",
    ".cfi_restore w30",
    "cbnz w9, 7f",
    "ret",
    "7:",
    "cmp w9, #1",
    "b.eq rt_dispatch",
    "b nxrt_jump_pending",
    ".cfi_endproc",
    ".size nxrt_host_entry, .-nxrt_host_entry",
    "",
    // A host fiber's first frame (fiber.rs builds its stack so that nxrt_ctx_switch returns here with the fiber's
    // parameter in x19 and x29 = 0, the end of the frame chain): runs the fiber's guest function, then switches to
    // the host fiber it returns for good.
    ".globl nxrt_fiber_entry",
    ".type nxrt_fiber_entry,%function",
    ".p2align 4",
    "nxrt_fiber_entry:",
    "mov x0, x19",
    "bl nxrt_fiber_run",
    "bl nxrt_fiber_leave",
    "bl nxrt_fiber_resumed_dead",
    "brk #0",
    ".size nxrt_fiber_entry, .-nxrt_fiber_entry",
    "",
    // void nxrt_ctx_switch(size_t *save, size_t to): x19-x30, d8-d15 and FPCR in a 176-byte frame (CTX_FRAME in
    // fiber.rs): 0 x19 x20, 16 x21 x22, 32 x23 x24, 48 x25 x26, 64 x27 x28, 80 x29 x30, 96 d8-d15, 160 FPCR.
    ".globl nxrt_ctx_switch",
    ".type nxrt_ctx_switch,%function",
    ".p2align 4",
    "nxrt_ctx_switch:",
    "sub sp, sp, #176",
    "stp x19, x20, [sp, #0]",
    "stp x21, x22, [sp, #16]",
    "stp x23, x24, [sp, #32]",
    "stp x25, x26, [sp, #48]",
    "stp x27, x28, [sp, #64]",
    "stp x29, x30, [sp, #80]",
    "stp d8, d9, [sp, #96]",
    "stp d10, d11, [sp, #112]",
    "stp d12, d13, [sp, #128]",
    "stp d14, d15, [sp, #144]",
    "mrs x9, fpcr",
    "str x9, [sp, #160]",
    "mov x9, sp",
    "str x9, [x0]",
    "mov sp, x1",
    "ldr x9, [sp, #160]",
    "msr fpcr, x9",
    "ldp x19, x20, [sp, #0]",
    "ldp x21, x22, [sp, #16]",
    "ldp x23, x24, [sp, #32]",
    "ldp x25, x26, [sp, #48]",
    "ldp x27, x28, [sp, #64]",
    "ldp x29, x30, [sp, #80]",
    "ldp d8, d9, [sp, #96]",
    "ldp d10, d11, [sp, #112]",
    "ldp d12, d13, [sp, #128]",
    "ldp d14, d15, [sp, #144]",
    "add sp, sp, #176",
    "ret",
    ".size nxrt_ctx_switch, .-nxrt_ctx_switch",
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

#[cfg(target_os = "linux")]
extern "C" {
    /// Saves the callee-saved registers and the FP control state (MXCSR and the x87 control word; AArch64: FPCR) on
    /// the current stack, stores the stack pointer in `*save`, and resumes the context whose stack pointer is `to` (a
    /// host fiber switch).
    pub fn nxrt_ctx_switch(save: *mut usize, to: usize);
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
pub static mut NXRT_DISPATCH: DispatchHot = DispatchHot {
    lo: 0,
    slots: 0,
    slot: core::ptr::null(),
    anchor: 0,
};

/// SwitchToFiber, for `nxrt_fiber_entry` (set by `fiber.rs`).
#[cfg(windows)]
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
