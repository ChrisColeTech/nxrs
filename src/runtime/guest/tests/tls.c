// TLS: _Thread_local variables (TPIDR_EL0, local-exec) are per thread; each thread's TLS region (TPIDRRO_EL0) is its
// own and holds its ThreadVars with its handle.
#include "sdk.h"

static _Thread_local int tl = 100;
static _Thread_local char tl_name[16];
static int results[3];
static int handle_ok[3];
static void *regions[4];

static void worker(void *arg) {
    int id = (int)(intptr_t)arg;
    for (int i = 0; i < 1000; i++) tl += id + 1;
    tl_name[0] = (char)('a' + id);
    svcSleepThread(1000000);   // the others run meanwhile
    results[id] = tl * 10 + (tl_name[0] == 'a' + id);
    uint64_t tid = 0;
    handle_ok[id] = svcGetThreadId(&tid, threadGetCurHandle()) == 0 && tid != 0 && getThreadVars()->magic == 0x21545624u;
    regions[id] = armGetTls();
}

int main(void) {
    Thread t[3];
    for (int i = 0; i < 3; i++) threadCreate(&t[i], worker, (void *)(intptr_t)i, 0x10000, 44);
    for (int i = 0; i < 3; i++) threadStart(&t[i]);
    for (int i = 0; i < 3; i++) threadWaitForExit(&t[i]);
    regions[3] = armGetTls();
    printf("main: tl = %d, name '%s'\n", tl, tl_name);
    for (int i = 0; i < 3; i++) printf("thread %d: tl*10+name_ok = %d, handle ok %d\n", i, results[i], handle_ok[i]);
    int distinct = regions[0] != regions[1] && regions[1] != regions[2] && regions[0] != regions[2] &&
                   regions[3] != regions[0] && regions[3] != regions[1] && regions[3] != regions[2];
    printf("TLS regions distinct: %s\n", distinct ? "yes" : "no");
    return 0;
}
