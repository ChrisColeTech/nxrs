// Horizon SVC wrappers (the libnx calling convention: outputs written through pointers passed in x0).
    .text
    .macro SVC_BEGIN name
    .global \name
    .type \name, %function
    .p2align 2
    .cfi_startproc
\name:
    .endm
    .macro SVC_END
    .cfi_endproc
    .endm

SVC_BEGIN svcSetHeapSize          // (u64 *out_addr, u64 size)
    str x0, [sp, #-16]!
    .cfi_def_cfa_offset 16
    svc 0x1
    ldr x2, [sp], #16
    .cfi_def_cfa_offset 0
    str x1, [x2]
    ret
SVC_END
SVC_BEGIN svcExitProcess
    svc 0x7
    ret
SVC_END
SVC_BEGIN svcCreateThread         // (u32 *out, void *entry, void *arg, void *stack_top, int prio, int cpuid)
    str x0, [sp, #-16]!
    .cfi_def_cfa_offset 16
    svc 0x8
    ldr x2, [sp], #16
    .cfi_def_cfa_offset 0
    str w1, [x2]
    ret
SVC_END
SVC_BEGIN svcStartThread
    svc 0x9
    ret
SVC_END
SVC_BEGIN svcExitThread
    svc 0xA
    ret
SVC_END
SVC_BEGIN svcSleepThread
    svc 0xB
    ret
SVC_END
SVC_BEGIN svcCloseHandle
    svc 0x16
    ret
SVC_END
SVC_BEGIN svcWaitSynchronization  // (s32 *index, const u32 *handles, s32 count, u64 timeout)
    str x0, [sp, #-16]!
    .cfi_def_cfa_offset 16
    svc 0x18
    ldr x2, [sp], #16
    .cfi_def_cfa_offset 0
    str w1, [x2]
    ret
SVC_END
SVC_BEGIN svcArbitrateLock        // (u32 owner, u32 *addr, u32 tag)
    svc 0x1A
    ret
SVC_END
SVC_BEGIN svcArbitrateUnlock      // (u32 *addr)
    svc 0x1B
    ret
SVC_END
SVC_BEGIN svcWaitProcessWideKeyAtomic   // (u32 *addr, u32 *key, u32 tag, u64 timeout)
    svc 0x1C
    ret
SVC_END
SVC_BEGIN svcSignalProcessWideKey // (u32 *key, s32 count)
    svc 0x1D
    ret
SVC_END
SVC_BEGIN svcGetSystemTick
    svc 0x1E
    ret
SVC_END
SVC_BEGIN svcGetThreadId          // (u64 *out, u32 handle)
    str x0, [sp, #-16]!
    .cfi_def_cfa_offset 16
    svc 0x25
    ldr x2, [sp], #16
    .cfi_def_cfa_offset 0
    str x1, [x2]
    ret
SVC_END
SVC_BEGIN svcOutputDebugString    // (const char *s, u64 len)
    svc 0x27
    ret
SVC_END
SVC_BEGIN svcGetInfo              // (u64 *out, u32 id0, u32 handle, u64 id1)
    str x0, [sp, #-16]!
    .cfi_def_cfa_offset 16
    svc 0x29
    ldr x2, [sp], #16
    .cfi_def_cfa_offset 0
    str x1, [x2]
    ret
SVC_END

// The current thread's TLS region (TPIDRRO_EL0) and C TLS pointer (TPIDR_EL0).
SVC_BEGIN armGetTls
    mrs x0, tpidrro_el0
    ret
SVC_END
SVC_BEGIN armSetTp
    msr tpidr_el0, x0
    ret
SVC_END
SVC_BEGIN armGetSystemTickFreq
    mrs x0, cntfrq_el0
    ret
SVC_END
SVC_BEGIN armGetSystemTick
    mrs x0, cntpct_el0
    ret
SVC_END

// setjmp/longjmp: callee-saved registers, sp and lr; longjmp returns with BR to the saved lr (as musl does).
SVC_BEGIN setjmp
    stp x19, x20, [x0, #0]
    stp x21, x22, [x0, #16]
    stp x23, x24, [x0, #32]
    stp x25, x26, [x0, #48]
    stp x27, x28, [x0, #64]
    stp x29, x30, [x0, #80]
    mov x2, sp
    str x2, [x0, #104]
    stp d8, d9, [x0, #112]
    stp d10, d11, [x0, #128]
    stp d12, d13, [x0, #144]
    stp d14, d15, [x0, #160]
    mov x0, #0
    ret
SVC_END
SVC_BEGIN longjmp
    ldp x19, x20, [x0, #0]
    ldp x21, x22, [x0, #16]
    ldp x23, x24, [x0, #32]
    ldp x25, x26, [x0, #48]
    ldp x27, x28, [x0, #64]
    ldp x29, x30, [x0, #80]
    ldr x2, [x0, #104]
    mov sp, x2
    ldp d8, d9, [x0, #112]
    ldp d10, d11, [x0, #128]
    ldp d12, d13, [x0, #144]
    ldp d14, d15, [x0, #160]
    cmp w1, #0
    csinc w0, w1, wzr, ne
    br x30
SVC_END

// The unwinder's register file: x0-x30 at 0, sp at 248, pc at 256, d8-d15 at 264.
// __unw_getcontext(regs): the caller's state at its call (pc = the return address).
SVC_BEGIN __unw_getcontext
    stp x0, x1, [x0, #0]
    stp x2, x3, [x0, #16]
    stp x4, x5, [x0, #32]
    stp x6, x7, [x0, #48]
    stp x8, x9, [x0, #64]
    stp x10, x11, [x0, #80]
    stp x12, x13, [x0, #96]
    stp x14, x15, [x0, #112]
    stp x16, x17, [x0, #128]
    stp x18, x19, [x0, #144]
    stp x20, x21, [x0, #160]
    stp x22, x23, [x0, #176]
    stp x24, x25, [x0, #192]
    stp x26, x27, [x0, #208]
    stp x28, x29, [x0, #224]
    str x30, [x0, #240]
    mov x1, sp
    str x1, [x0, #248]
    str x30, [x0, #256]
    stp d8, d9, [x0, #264]
    stp d10, d11, [x0, #280]
    stp d12, d13, [x0, #296]
    stp d14, d15, [x0, #312]
    mov x0, #0
    ret
SVC_END
// __unw_resume(regs): load every register and jump to pc (a landing pad). BR, so lifted code dispatches it.
SVC_BEGIN __unw_resume
    ldp d8, d9, [x0, #264]
    ldp d10, d11, [x0, #280]
    ldp d12, d13, [x0, #296]
    ldp d14, d15, [x0, #312]
    ldp x2, x3, [x0, #16]
    ldp x4, x5, [x0, #32]
    ldp x6, x7, [x0, #48]
    ldp x8, x9, [x0, #64]
    ldp x10, x11, [x0, #80]
    ldp x12, x13, [x0, #96]
    ldp x14, x15, [x0, #112]
    ldp x18, x19, [x0, #144]
    ldp x20, x21, [x0, #160]
    ldp x22, x23, [x0, #176]
    ldp x24, x25, [x0, #192]
    ldp x26, x27, [x0, #208]
    ldp x28, x29, [x0, #224]
    ldr x30, [x0, #240]
    ldr x16, [x0, #248]
    mov sp, x16
    ldr x16, [x0, #256]
    ldp x0, x1, [x0, #0]
    br x16
SVC_END
