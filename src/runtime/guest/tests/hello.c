// Hello world, plus the bridge: two imports bound to host (Rust) functions return their values.
#include "sdk.h"

int64_t nxrt_test_sum9(uint8_t a, int16_t b, int32_t c, int64_t d, int64_t e, int64_t f, int64_t g, int64_t h, int64_t i);
double nxrt_test_scale(double x, int32_t k);

int main(void) {
    printf("Hello, world!\n");
    printf("host sum9: %lld\n", (long long)nxrt_test_sum9(250, -3, 100000, 4, 5, 6, 7, 8, 9));
    printf("host scale: %f\n", nxrt_test_scale(1.5, 3));
    uint64_t heap = 0;
    svcGetInfo(&heap, 4, 0, 0);
    printf("heap region reported: %s\n", heap ? "yes" : "no");
    return 0;
}
