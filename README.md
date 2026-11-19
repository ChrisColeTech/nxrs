# RecompLauncherEngine

A Rust engine that runs Switch games as native programs: their ARM64 code is translated to C ahead of time, and the
game's data is read in place from its own packages.

| Part | What |
|---|---|
| [`src/translator`](src/translator/README.md) | package input (folder, NSP, NUP, XCI, NCA; updates and DLC in place) and the ARM64 → C translator, with the `nxrs` CLI |
| `src/runtime` | the host runtime |

Build and test: `cargo build --release`, `cargo test` (seconds). The CLI is `target/release/nxrs`.

## Credits and licenses

The engine builds on these projects. Their license texts are in [`LICENSES/`](LICENSES/).

| Source | Copyright and license | What it contributed |
|---|---|---|
| mkw-recomp | © 2026 Adrià, MIT ([text](LICENSES/mkw-recomp.txt)) | the translator, ported to Rust: NSO loading, module seeds and layout (`loader/`), the lifter, NEON/FP lowering, local flags and registers, jump tables (`xlate/`), function discovery, chunked emission and the patch table (`aot/`); the runtime headers the generated C includes (`src/translator/runtime/`, vendored; only their attribution line moved here) |
| totk-pc-recomp | © 2026 Valiantsin Dzerakh, MIT ([text](LICENSES/totk-pc-recomp.txt)) | the translator code mkw-recomp took from it (its `recomp/`) |
| LAS Recomp | © 2026 Peiikai, MIT ([text](LICENSES/las-recomp.txt)) | the original NSO loader, lifter, NEON/FP lowering, generator and `cpu.h` / `simd.h` / `simd_fp.h` that totk-pc-recomp and mkw-recomp built on |
| nxnative | © 2026 seedds, MIT ([text](LICENSES/nxnative.txt)) | the engine's design: this is a Rust port of nxnative, which runs ARM code natively; the translator fills that gap |
| the owner's code | ChrisColeTech | package input: NSP / NCA / ticket / key handling, BKTR updates, NUP, DLC install and the RomFS markers, ported from the owner's readers written for the Turbo fork |
| mkw-recomp (runtime) | © 2026 Adrià, MIT ([text](LICENSES/mkw-recomp.txt)) | the runtime's mechanisms, ported to Rust in `src/runtime/`: the flat PC → function dispatch table (`dispatch.rs`, `asm.rs`), host functions at synthetic guest addresses and the `HOST_OVERRIDE` binding rule (`hostfn.rs`, `loader.rs`), guest setjmp/longjmp owners (`sjlj.rs`, `asm.rs`), nn::os fibers on host fibers (`fiber.rs`), the Windows guest memory model (`mem.rs`: fixed reservations below 2^39, write watch, GetInfo ids, QueryMemory states), the `rt_*` services and the crash report's guest backtrace (`rt.rs`, `fault.rs`); `tools/mkw_gen.py` runs its unmodified `recomp/gen.py` |
| LAS Recomp (runtime) | © 2026 Peiikai, MIT ([text](LICENSES/las-recomp.txt)) | through mkw-recomp: the host-function registry and dispatch it started from, and the GetWriteWatch use in the memory layer |
| nxnative (runtime) | © 2026 seedds, MIT ([text](LICENSES/nxnative.txt)) | the minimal kernel (`kernel.rs`: threads, ArbitrateLock/Unlock, the process-wide-key condition variable, address arbitration, events, WaitSynchronization) ported from `dm/host/hle_thread.c`; import binding by symbol name with an UNIMPL report from `hle.c` / `loader.c` |
