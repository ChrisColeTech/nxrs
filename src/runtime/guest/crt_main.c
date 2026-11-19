// The test program's start: SDK init (heap, the main thread's TLS and ThreadVars, unwind tables), main, exit.
#include "sdk.h"

#define HIDDEN __attribute__((visibility("hidden")))
extern char __tls_start[] HIDDEN, __tdata_end[] HIDDEN, __tls_end[] HIDDEN;
extern const char __eh_frame_hdr_start[] HIDDEN;
int main(void);

void __crt_start(uint64_t x0, Handle main_thread) {
    (void)x0;
    __sdk_init(main_thread, __tls_start, (size_t)(__tdata_end - __tls_start), (size_t)(__tls_end - __tls_start),
               __eh_frame_hdr_start);
    exit(main());
}
