// Link-time stand-ins for the functions the runtime provides by name (the bridge): main is linked against this
// library so the names become dynamic imports; at run time the loader binds them to host functions. Never loaded.
#include <stdint.h>
int64_t nxrt_test_sum9(uint8_t a, int16_t b, int32_t c, int64_t d, int64_t e, int64_t f, int64_t g, int64_t h, int64_t i) { return 0; }
double nxrt_test_scale(double x, int32_t k) { return 0; }
void nxrt_test_callback(void (*fn)(void *), void *arg) {}
uint32_t nxrt_test_drops(void) { return 0; }
void _ZN2nn2os15InitializeFiberEPNS0_9FiberTypeEPFS2_PvES3_S3_mi(void) {}
void _ZN2nn2os13FinalizeFiberEPNS0_9FiberTypeE(void) {}
void _ZN2nn2os13SwitchToFiberEPNS0_9FiberTypeE(void) {}
void _ZN2nn2os15GetCurrentFiberEv(void) {}
