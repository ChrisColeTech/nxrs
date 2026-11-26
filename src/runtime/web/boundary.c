// The web build's boundary between generated code and Rust (src/runtime/src/asm.rs on the other hosts), in C over two
// wasm features: guest <-> host jumps are setjmp / longjmp (wasm exception handling, -sSUPPORT_LONGJMP=wasm), and the
// dispatch is a guaranteed tail call (wasm tail calls, -mtail-call), so chains of guest indirect branches keep the host
// stack flat as the native tail jump does. The rule of asm.rs holds: no jump skips a live Rust frame. Rust is entered
// through a shim that calls it and acts on its status after it returns; Rust calls guest code only under a barrier.
//
// SjSlot is asm.rs's (same layout): `rsp` is unused here and `resume` holds the slot's jmp_buf.
#include <setjmp.h>
#include <stdint.h>
#include "cpu.h"

typedef struct SjSlot {
    struct SjSlot *up;
    uint64_t lo, hi, rsp;
    Cpu *c;
    uint64_t body, resume, status;
} SjSlot;

typedef struct {
    uint64_t lo, slots;
    const uint32_t *slot;
    uint64_t anchor;
} DispatchHot;

extern DispatchHot NXRT_DISPATCH;
GuestFn nxrt_dispatch_resolve(Cpu *c);
uint32_t nxrt_svc_impl(Cpu *c, uint32_t imm, uint64_t pc);
uint32_t nxrt_host_invoke(Cpu *c);

#define SLOT(c) ((SjSlot *)(uintptr_t)(c)->sj)

// The lifted function for c->pc: the flat table (asm.rs's fast path: (pc - lo) rotated right by 2 indexes it), else
// Rust's resolve (a host function's entry, a pending guest jump, or the fatal miss). Both are tail calls.
void rt_dispatch(Cpu *c) {
    uint64_t i = c->pc - NXRT_DISPATCH.lo;
    i = (i >> 2) | (i << 62);
    if (i < NXRT_DISPATCH.slots) {
        uint32_t w = NXRT_DISPATCH.slot[i];
        if (w) {
            GuestFn f = (GuestFn)(uintptr_t)(w + NXRT_DISPATCH.anchor);
            __attribute__((musttail)) return f(c);
        }
    }
    GuestFn f = nxrt_dispatch_resolve(c);
    __attribute__((musttail)) return f(c);
}

// Into the slot's frame, which reads its status from the slot.
__attribute__((noreturn)) void nxrt_sj_jump(SjSlot *s) {
    longjmp(*(jmp_buf *)(uintptr_t)s->resume, 1);
}

// The jump Rust set up: c->sj names the target slot.
__attribute__((noreturn)) void nxrt_jump_pending(Cpu *c) {
    nxrt_sj_jump(SLOT(c));
}

// A setjmp caller's body: a jump back into it runs the body again, which continues at c->pc.
void rt_sj_owner(Cpu *c, GuestFn body, uint64_t lo, uint64_t hi) {
    jmp_buf jb;
    volatile SjSlot s = {SLOT(c), lo, hi, 0, c, (uint64_t)(uintptr_t)body, (uint64_t)(uintptr_t)&jb, 0};
    c->sj = (uint64_t)(uintptr_t)&s;
    setjmp(jb);
    body(c);
    c->sj = (uint64_t)(uintptr_t)s.up;
}

// A host -> guest call under a barrier: 0 when the guest function returns, else the status a jump to the barrier
// carried (asm.rs BARRIER_ESCAPE / BARRIER_EXIT).
uint64_t nxrt_call_guest(Cpu *c) {
    jmp_buf jb;
    volatile SjSlot s = {SLOT(c), 0, 0, 0, c, 0, (uint64_t)(uintptr_t)&jb, 0};
    volatile uint64_t r = 0;
    c->sj = (uint64_t)(uintptr_t)&s;
    if (setjmp(jb) == 0) {
        rt_dispatch(c);
    } else {
        r = s.status;
    }
    c->sj = (uint64_t)(uintptr_t)s.up;
    return r;
}

// Status of the Rust shims: 0 return, 1 dispatch c->pc again, 2 take the pending jump.
void rt_svc(Cpu *c, uint32_t imm, uint64_t pc) {
    uint32_t st = nxrt_svc_impl(c, imm, pc);
    if (st == 1) {
        rt_dispatch(c);
    } else if (st == 2) {
        nxrt_jump_pending(c);
    }
}

void nxrt_host_entry(Cpu *c) {
    uint32_t st = nxrt_host_invoke(c);
    if (st == 1) {
        rt_dispatch(c);
    } else if (st == 2) {
        nxrt_jump_pending(c);
    }
}
