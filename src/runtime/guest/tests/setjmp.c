// setjmp/longjmp: longjmp from five frames down back into main, three times; longjmp(b, 0) returns 1; and a guest
// callback, called by a host function, longjmps past that host function back into main (the host function's Rust
// frame is unwound normally first: its destructor runs once).
#include "sdk.h"

void nxrt_test_callback(void (*fn)(void *), void *arg);
uint32_t nxrt_test_drops(void);

static jmp_buf jb, jb_cb;

__attribute__((noinline)) static void deep(int n, int v) {
    if (n == 0) longjmp(jb, v);
    deep(n - 1, v);
    __asm__ volatile("");
}

static void callback(void *arg) {
    printf("callback runs (arg %d), longjmps out\n", (int)(intptr_t)arg);
    longjmp(jb_cb, 5);
}

int main(void) {
    volatile int count = 0;
    int local = 7;
    int r = setjmp(jb);
    count++;
    printf("setjmp returned %d (pass %d, local %d)\n", r, count, local);
    if (r < 3) deep(5, r + 1);

    jmp_buf b2;
    if (!setjmp(b2)) longjmp(b2, 0);
    else printf("longjmp(b2, 0) made setjmp return 1\n");

    int v = setjmp(jb_cb);
    if (v == 0) {
        nxrt_test_callback(callback, (void *)(intptr_t)99);
        printf("not reached\n");
    }
    printf("back in main with %d; host frames dropped: %u\n", v, nxrt_test_drops());
    return 0;
}
