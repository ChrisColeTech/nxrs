// Atomics: exclusive load/store loops (LDXR/STXR, LDAXR/STLXR, LDXP/STXP for 128 bits) and LSE (LDADD, CAS, SWP;
// atomics_lse.c), four threads at once.
#include "sdk.h"

void lse_work(uint64_t *add, uint32_t *cas, uint64_t *swp, int n);

static uint64_t ll_counter;
static uint32_t cas_counter;
static unsigned __int128 wide __attribute__((aligned(16)));
static uint64_t lse_add, lse_swp;
static uint32_t lse_cas;

static void worker(void *arg) {
    int n = (int)(intptr_t)arg;
    for (int i = 0; i < n; i++) {
        __atomic_fetch_add(&ll_counter, 1, __ATOMIC_SEQ_CST);
        uint32_t v = __atomic_load_n(&cas_counter, __ATOMIC_RELAXED);
        while (!__atomic_compare_exchange_n(&cas_counter, &v, v + 2, 1, __ATOMIC_ACQ_REL, __ATOMIC_RELAXED)) {}
        unsigned __int128 w = __atomic_load_n(&wide, __ATOMIC_ACQUIRE);
        while (!__atomic_compare_exchange_n(&wide, &w, w + ((unsigned __int128)1 << 64) + 1, 0, __ATOMIC_SEQ_CST,
                                            __ATOMIC_ACQUIRE)) {}
    }
    lse_work(&lse_add, &lse_cas, &lse_swp, n);
}

int main(void) {
    Thread t[4];
    for (int i = 0; i < 4; i++) threadCreate(&t[i], worker, (void *)(intptr_t)50000, 0x10000, 44);
    for (int i = 0; i < 4; i++) threadStart(&t[i]);
    for (int i = 0; i < 4; i++) threadWaitForExit(&t[i]);
    printf("ldxr/stxr add: %llu\n", (unsigned long long)ll_counter);
    printf("cas loop: %u\n", cas_counter);
    printf("128-bit cas: hi %llu lo %llu\n", (unsigned long long)(wide >> 64), (unsigned long long)wide);
    printf("lse ldadd: %llu, cas: %u, swp total: %llu\n", (unsigned long long)lse_add, lse_cas, (unsigned long long)lse_swp);
    return 0;
}
