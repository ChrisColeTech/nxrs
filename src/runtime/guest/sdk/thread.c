// Threads, mutexes and condition variables shaped like libnx's: a mutex is a tag word (owner handle, 0x40000000
// when others wait) that the kernel arbitrates; a condition variable is a key word for WaitProcessWideKeyAtomic.
// C TLS (_Thread_local in the main module, local-exec) is a per-thread copy of the main module's TLS template
// behind TPIDR_EL0 (AArch64 variant 1: a 16-byte TCB, then the data).
#include "sdk.h"

#define HAS_LISTENERS 0x40000000u
#define THREADVARS_MAGIC 0x21545624u   // "!TV$"

void __heap_init(void);
extern const char __eh_frame_hdr_start[] __attribute__((visibility("hidden")));
void __eh_register(const void *eh_frame_hdr);

static void *tls_image;
static size_t tls_data, tls_size;

static void *tls_block_new(void) {
    if (!tls_size) return 0;
    uint8_t *b = malloc(16 + tls_size);
    memset(b, 0, 16 + tls_size);
    memcpy(b + 16, tls_image, tls_data);
    return b;
}

static void thread_vars_init(Handle h, void *thread, void *tp) {
    ThreadVars *tv = getThreadVars();
    tv->magic = THREADVARS_MAGIC;
    tv->handle = h;
    tv->thread_ptr = thread;
    tv->tls_tp = tp;
    if (tp) armSetTp(tp);
}

void __sdk_init(Handle main_thread, void *tls_start, size_t tdata_size, size_t total_tls, const void *eh_frame_hdr) {
    __heap_init();
    tls_image = tls_start;
    tls_data = tdata_size;
    tls_size = total_tls;
    thread_vars_init(main_thread, 0, tls_block_new());
    __eh_register(__eh_frame_hdr_start);
    if (eh_frame_hdr) __eh_register(eh_frame_hdr);
}

Handle threadGetCurHandle(void) { return getThreadVars()->handle; }

static void thread_entry(void *arg) {
    Thread *t = arg;
    thread_vars_init(t->handle, t, t->tls);
    t->fn(t->arg);
    svcExitThread();
}

int threadCreate(Thread *t, void (*fn)(void *), void *arg, size_t stack_size, int prio) {
    t->fn = fn;
    t->arg = arg;
    t->stack = malloc(stack_size);
    t->tls = tls_block_new();
    return (int)svcCreateThread(&t->handle, (void *)thread_entry, t, (uint8_t *)t->stack + stack_size, prio, -2);
}

int threadStart(Thread *t) { return (int)svcStartThread(t->handle); }

int threadWaitForExit(Thread *t) {
    int32_t idx;
    return (int)svcWaitSynchronization(&idx, &t->handle, 1, UINT64_MAX);
}

int threadClose(Thread *t) { return (int)svcCloseHandle(t->handle); }

void mutexLock(Mutex *m) {
    uint32_t self = getThreadVars()->handle;
    for (;;) {
        uint32_t cur = 0;
        if (__atomic_compare_exchange_n(m, &cur, self, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) return;
        if (!(cur & HAS_LISTENERS)) {
            // announce a waiter first; the owner then hands the lock over through the kernel
            if (!__atomic_compare_exchange_n(m, &cur, cur | HAS_LISTENERS, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED))
                continue;
        }
        svcArbitrateLock(cur & ~HAS_LISTENERS, m, self);
        if ((__atomic_load_n(m, __ATOMIC_ACQUIRE) & ~HAS_LISTENERS) == self) return;
    }
}

void mutexUnlock(Mutex *m) {
    uint32_t self = getThreadVars()->handle;
    uint32_t cur = self;
    if (!__atomic_compare_exchange_n(m, &cur, 0, 0, __ATOMIC_RELEASE, __ATOMIC_RELAXED)) svcArbitrateUnlock(m);
}

Result condvarWaitTimeout(CondVar *c, Mutex *m, uint64_t ns) {
    Result r = svcWaitProcessWideKeyAtomic(m, c, getThreadVars()->handle, ns);
    if (r == 0xEA01) mutexLock(m);   // timed out: the kernel did not hand the lock back
    return r;
}

Result condvarWait(CondVar *c, Mutex *m) { return condvarWaitTimeout(c, m, UINT64_MAX); }
void condvarWakeOne(CondVar *c) { svcSignalProcessWideKey(c, 1); }
void condvarWakeAll(CondVar *c) { svcSignalProcessWideKey(c, -1); }
