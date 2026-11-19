// Built with +lse: LDADDAL, CASAL, SWPAL.
#include <stdint.h>

void lse_work(uint64_t *add, uint32_t *cas, uint64_t *swp, int n) {
    for (int i = 0; i < n; i++) {
        __atomic_fetch_add(add, 3, __ATOMIC_SEQ_CST);
        uint32_t v = __atomic_load_n(cas, __ATOMIC_RELAXED);
        while (!__atomic_compare_exchange_n(cas, &v, v + 1, 0, __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) {}
    }
    // every thread swaps its count in and adds back what it swapped out: the total is the sum of all counts
    uint64_t old = __atomic_exchange_n(swp, 0, __ATOMIC_SEQ_CST);
    __atomic_fetch_add(swp, old + (uint64_t)n, __ATOMIC_SEQ_CST);
}
