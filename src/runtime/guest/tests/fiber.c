// Fibers through nn::os (bound to the runtime's host fibers by name): two fibers switch to each other and back to
// the thread; one finishes by returning the fiber to run next. Each runs on its own guest stack.
#include "sdk.h"

typedef struct FiberType { uint8_t storage[0x40]; } FiberType;
typedef FiberType *(*FiberFunction)(void *);

void InitializeFiber(FiberType *f, FiberFunction fn, void *arg, void *stack, size_t size, int flags)
    __asm__("_ZN2nn2os15InitializeFiberEPNS0_9FiberTypeEPFS2_PvES3_S3_mi");
void FinalizeFiber(FiberType *f) __asm__("_ZN2nn2os13FinalizeFiberEPNS0_9FiberTypeE");
void SwitchToFiber(FiberType *f) __asm__("_ZN2nn2os13SwitchToFiberEPNS0_9FiberTypeE");
FiberType *GetCurrentFiber(void) __asm__("_ZN2nn2os15GetCurrentFiberEv");

static FiberType f1, f2;
static uint8_t stack1[0x10000] __attribute__((aligned(16))), stack2[0x10000] __attribute__((aligned(16)));
static int reuse_calls;

// SDK inline queries (including Bezel's FiberLite) read this byte, not a host-side fiber registry.
static unsigned state(FiberType *f) { return *(volatile uint8_t *)&f->storage[0x20]; }

static int on_stack(void *p, uint8_t *stack) { return (uint8_t *)p >= stack && (uint8_t *)p < stack + sizeof stack1; }

static FiberType *fiber1(void *arg) {
    int here = 1;
    printf("fiber1: start (arg %d, on its stack: %d)\n", (int)(intptr_t)arg, on_stack(&here, stack1));
    printf("states: fiber1 running %u %u\n", state(&f1), state(&f2));
    SwitchToFiber(&f2);
    printf("fiber1: resumed, current is fiber1: %d, local %d\n", GetCurrentFiber() == &f1, here);
    return &f2;   // finish, switching to fiber2
}

static FiberType *fiber2(void *arg) {
    int here = 2;
    printf("fiber2: start (arg %d, on its stack: %d)\n", (int)(intptr_t)arg, on_stack(&here, stack2));
    printf("states: fiber1 suspended %u %u\n", state(&f1), state(&f2));
    SwitchToFiber(&f1);
    printf("fiber2: resumed after fiber1 finished\n");
    printf("states: fiber1 completed %u %u\n", state(&f1), state(&f2));
    SwitchToFiber(0);
    printf("fiber2: resumed from the thread, local %d\n", here);
    return 0;     // finish, back to the thread
}

static FiberType *reused(void *arg) {
    ++reuse_calls;
    printf("reused: arg %d state %u\n", (int)(intptr_t)arg, state(&f1));
    return 0;
}

int main(void) {
    int local = 42;
    InitializeFiber(&f1, fiber1, (void *)11, stack1, sizeof stack1, 0);
    InitializeFiber(&f2, fiber2, (void *)22, stack2, sizeof stack2, 0);
    printf("states: initialized %u %u\n", state(&f1), state(&f2));
    printf("main: switching to fiber1 (current %p)\n", (void *)GetCurrentFiber());
    SwitchToFiber(&f1);
    printf("main: back on the thread (current %p, local %d)\n", (void *)GetCurrentFiber(), local);
    printf("states: returned to thread %u %u\n", state(&f1), state(&f2));
    SwitchToFiber(&f2);
    printf("states: both completed %u %u\n", state(&f1), state(&f2));
    // Reinitialization permits reuse. A scheduler polling completion must invoke a returned callback only once.
    InitializeFiber(&f1, reused, (void *)33, stack1, sizeof stack1, 0);
    printf("states: reinitialized %u %u\n", state(&f1), state(&f2));
    for (int i = 0; i < 3 && !(state(&f1) & 4); ++i) SwitchToFiber(&f1);
    printf("main: callback calls %d, completed %u\n", reuse_calls, state(&f1) & 4);
    printf("main: finalizing\n");
    FinalizeFiber(&f1);
    FinalizeFiber(&f2);
    printf("states: finalized %u %u\n", state(&f1), state(&f2));
    printf("main: done\n");
    return 0;
}
