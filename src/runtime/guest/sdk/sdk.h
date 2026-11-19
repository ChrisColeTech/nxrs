// The test programs' SDK: a libnx-shaped subset (svc wrappers, threads, mutexes, condition variables, TLS) and the
// libc pieces they use. Lives in the `sdk` module; the tests (`main`) import it through the PLT, as a game imports
// nnSdk.
#pragma once
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef uint32_t Handle;
typedef uint32_t Result;

Result svcSetHeapSize(uint64_t *out, uint64_t size);
void svcExitProcess(void) __attribute__((noreturn));
Result svcCreateThread(Handle *out, void *entry, void *arg, void *stack_top, int prio, int cpuid);
Result svcStartThread(Handle h);
void svcExitThread(void) __attribute__((noreturn));
void svcSleepThread(int64_t ns);
Result svcCloseHandle(Handle h);
Result svcWaitSynchronization(int32_t *index, const Handle *handles, int32_t count, uint64_t timeout);
Result svcArbitrateLock(Handle owner, uint32_t *addr, uint32_t tag);
Result svcArbitrateUnlock(uint32_t *addr);
Result svcWaitProcessWideKeyAtomic(uint32_t *addr, uint32_t *key, uint32_t tag, uint64_t timeout);
Result svcSignalProcessWideKey(uint32_t *key, int32_t count);
uint64_t svcGetSystemTick(void);
Result svcGetThreadId(uint64_t *out, Handle h);
Result svcOutputDebugString(const char *s, uint64_t len);
Result svcGetInfo(uint64_t *out, uint32_t id0, Handle h, uint64_t id1);
void *armGetTls(void);
void armSetTp(void *tp);
uint64_t armGetSystemTickFreq(void);
uint64_t armGetSystemTick(void);

// libnx's ThreadVars at the end of the 0x200-byte TLS region.
typedef struct {
    uint32_t magic;
    Handle handle;
    void *thread_ptr;
    void *reent;
    void *tls_tp;
} ThreadVars;
static inline ThreadVars *getThreadVars(void) { return (ThreadVars *)((uint8_t *)armGetTls() + 0x1E0); }

// libc
void *memcpy(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);
void *memmove(void *d, const void *s, size_t n);
int memcmp(const void *a, const void *b, size_t n);
size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
void *malloc(size_t n);
void free(void *p);
int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap);
int snprintf(char *buf, size_t size, const char *fmt, ...) __attribute__((format(printf, 3, 4)));
int printf(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
int puts(const char *s);
void abort(void) __attribute__((noreturn));
void exit(int code) __attribute__((noreturn));

typedef uint64_t jmp_buf[22];
int setjmp(jmp_buf b) __attribute__((returns_twice));
void longjmp(jmp_buf b, int v) __attribute__((noreturn));

// threads (libnx-shaped)
typedef struct Thread {
    Handle handle;
    void (*fn)(void *);
    void *arg;
    void *stack;
    void *tls;
} Thread;
int threadCreate(Thread *t, void (*fn)(void *), void *arg, size_t stack_size, int prio);
int threadStart(Thread *t);
int threadWaitForExit(Thread *t);
int threadClose(Thread *t);
Handle threadGetCurHandle(void);

typedef uint32_t Mutex;
void mutexLock(Mutex *m);
void mutexUnlock(Mutex *m);
typedef uint32_t CondVar;
Result condvarWait(CondVar *c, Mutex *m);
Result condvarWaitTimeout(CondVar *c, Mutex *m, uint64_t ns);
void condvarWakeOne(CondVar *c);
void condvarWakeAll(CondVar *c);

// Called by the test's crt: the main thread's handle and the main module's TLS template.
void __sdk_init(Handle main_thread, void *tls_start, size_t tdata_size, size_t tls_size, const void *eh_frame_hdr);

#ifdef __cplusplus
}
#endif
