// The runtime's C interface for the executable that links translated code (nxrt.lib). The generated code itself
// uses only cpu.h (vendored by the translator); this adds the module table and the entry point.
#pragma once
#include <stdint.h>

#include "cpu.h"

// One translated module: its guest base and image size (layout.h MOD_<X>_BASE / _SIZE) and the generator's
// function table (<name>_fn_addrs, <name>_fn_ptrs, <name>_fn_count).
typedef struct RtModule {
    const char *name;
    uint64_t base, size;
    const uint64_t *addrs;
    const GuestFn *fns;
    uint64_t count;
} RtModule;

// Loads <exefs>/<name> for every module (argv[1], or NXRT_EXEFS), runs the guest until it exits.
int nxrt_main(int argc, char **argv, const RtModule *mods, int count);
