# runtime (`nxrt`)

The engine's runtime: the library translated game code links against and runs on. The translator
(`src/translator`) turns the game's ARM64 code into C, one `void f_<addr>(Cpu *c)` per guest function; clang compiles
that C, and it is linked with this crate (built as `nxrt.lib`) into an executable whose `main` calls `nxrt_main`.

It provides the CPU-state ABI, indirect-branch dispatch, the host-call bridge, `svc` handling, TLS, guest
setjmp/longjmp, fibers, the crash reporter, guest memory on Windows, the module loader and a minimal Horizon kernel:
milestone M2 of the port plan. Sources and licenses: the "Credits and licenses" section of the repository README.

## The ABI it implements

The contract is the C the translator emits and the headers that C includes (`src/translator/runtime/cpu.h`,
`simd.h`, `simd_fp.h`, vendored there). This crate exports exactly the symbols they reference:

| Symbol | Declared in | What it does |
|---|---|---|
| `Cpu` | `cpu.h` | Guest state: `x[32]` at 0, `sp` 256, `pc` 264, `nzcv` 272, `tpidr` 280, `tpidrro` 288, `excl_addr` 296, `excl_val` 304, `thread` 312, `fpcr` 320, `fpsr` 328, `excl_val_hi` 336, `sj` 344, `v[32]` at 512; 1024 bytes, 16-aligned. Mirrored in `cpu.rs` |
| `rt_dispatch(Cpu *c)` | `cpu.h` | Jump to the function for `c->pc` (indirect branches, calls outside a chunk). A pure tail jump |
| `rt_sj_owner(c, body, lo, hi)` | `cpu.h` | The frame of a function a guest longjmp may re-enter (`sjlj.rs`) |
| `rt_svc(c, imm, pc)` | `cpu.h` | `SVC #imm` |
| `rt_brk(c, imm, pc)`, `rt_udf(c, pc)` | `cpu.h` | `BRK`, unlifted words: a fatal report with the guest state |
| `rt_ticks()`, `rt_tick_hz` | `cpu.h` | CNTPCT/CNTVCT at `RT_TICK_HZ` = `rt_tick_hz`: 19.2 MHz for a Switch program, 31.25 MHz for a Switch 2 one, set at start-up (`SWR_TICK_HZ`); CNTFRQ reads it |
| `rt_excl_mon` | `cpu.h` | The global exclusive monitor: 2^16 sequence numbers by 16-byte bucket. LDXR remembers its bucket's, STXR succeeds only if no other exclusive store went there since (and the value matches). On for Switch 1 programs, NULL (values only) for Switch 2 (`SWR_XMON=1` / `SWR_NO_XMON=1`). LDXR/LDAR/LDAPR are acquire loads, STLR a sequentially consistent store |
| `rt_spin_gate(c, head)` | `cpu.h` | One pass of a marked spin-wait loop: yields |
| `fnprof_hit(addr, lr, x0)` | `cpu.h` | Function-entry profiler hook (reached only while `g_fnprof` is set) |
| `g_fnprof`, `rt_patch_on[]` | `cpu.h` (hidden) | Defined beside the generated code, in the `modules.c` the build glue writes |
| `nxrt_main(argc, argv, mods, count)`, `RtModule` | `include/nxrt.h` | The executable's entry: the module table is the generator's `<name>_fn_addrs` / `_fn_ptrs` / `_fn_count` with the bases and sizes of `layout.json` |

Exclusive monitors, LSE atomics, barriers, flags and the FP/SIMD helpers are inline in the headers; the generated code
calls nothing else. `tests/abi_layout.rs` compiles a C probe against `cpu.h` and `nxrt.h` and fails on any size,
alignment or offset that differs from the Rust mirrors.

## Modules

| File | Part |
|---|---|
| `cpu.rs` | `Cpu` mirror, AAPCS64 argument helpers for host functions, MXCSR ← FPCR sync |
| `asm.rs` | The machine-code boundary: `rt_dispatch`, `rt_sj_owner`, the host → guest barrier `nxrt_call_guest`, the `rt_svc` and host-function shims, the fiber entry frame |
| `boundary.rs` | The Rust side of that boundary: `guard` (panics), `call_guest`, `nxrt_host_invoke` |
| `dispatch.rs` | The flat guest-PC → function table (one `u32` per guest instruction, sealed read-only), the slow path, host-PC → guest-function mapping |
| `hostfn.rs` | The bridge: host functions at synthetic guest addresses (`0x7F_1000_0000 + 16*i`), overrides by symbol name, UNIMPL stubs for imports nobody provides |
| `sjlj.rs` | Which owner or barrier a non-local guest jump goes to |
| `fiber.rs` | nn::os fibers (`InitializeFiber`, `SwitchToFiber`, `GetCurrentFiber`, `FinalizeFiber`) bound by name to host fibers, each with its own `Cpu` |
| `mem.rs` | Guest address space: module window from `0x70_0000_0000`, kernel region `0x7F_0000_0000`, a write-watched stack+heap block and alias region found from 4 GiB up, all below 2^39; heap (lazily committed on demand: `set_lazy_heap`, `NXRT_LAZY_HEAP=1`), physical memory in the alias region counted against the pool (a map past it fails with ResultLimitReached when the map limit is on: Z-A, Ocarina, `SWR_MAP_LIMIT`), stacks, TLS regions, QueryMemory, GetInfo ids, `written_pages` (GetWriteWatch) |
| `loader.rs` | NSO (lz4) and NRO images, MOD0/.dynamic, mapping at the translator's bases, RELATIVE/ABS64/GLOB_DAT/JUMP_SLOT/RELR relocation, imports bound across modules by name with host overrides first, `main.npdm` main-thread parameters |
| `kernel.rs` | Threads (one host thread per guest thread, 256 MiB reserved host stack; the thread ID at TLS+0x110 for SDK 22), ArbitrateLock/Unlock, WaitProcessWideKeyAtomic/SignalProcessWideKey, WaitForAddress (incl. WaitIfEqual64)/SignalToAddress, events (also from host code: `create_event`, `signal_event`, ...), WaitSynchronization, CancelSynchronization, CloseHandle, priorities and core masks; a `Scheduler` (`set_scheduler`) is told thread start and exit, blocking SVCs, priority and affinity changes and answers GetCurrentProcessorNumber |
| `svc.rs` | `rt_svc`: handlers registered by number first (`set_handler`), memory (SetHeapSize, QueryMemory, Map/UnmapPhysicalMemory), ExitProcess, ExitThread, Break, OutputDebugString (stdout), GetInfo, GetSystemTick, then the kernel, then the fallback (`set_fallback`); anything else is a named fatal |
| `fault.rs` | Vectored exception handler: commits an untouched lazy-heap page and retries; a host fault in lifted code (the image the runtime is linked into: the game's code DLL) is reported as the guest function it was lifted from, with the host stack walked and mapped the same way, and the guest registers and frame chain |
| `process.rs` | `nxrt_main`; `load` (reserve, load, bind, seal the dispatch table) and `start` (the main thread at `Entry::{Main, Module, Address, Host}` with x1 = its handle); `startup`: from main.npdm's program id the counter rate, the exclusive monitor, the map limit, and the process memory cap (a job object: half of RAM within 12..32 GB, at least the pool + 8 GB, `SWR_MEMORY_CAP_GB`); the ini's (`nxconfig`) `memory_gb` (the pool: 0 automatic, 4 console, 6 / 8 extended), `null_trap` (page 0 always traps here), `log` (2: every trace), `profile` |
| `profile.rs` | `profile=1`: samples every guest thread's host pc about once a millisecond and writes `profile.txt` (the guest functions with the most samples) every 10 s |
| `rt.rs` | Logging, fatal errors, the guest report, `rt_ticks`, `rt_brk`, `rt_udf` |
| `bin/nxrt_dev.rs` | Build glue: guest test build, `modules.c`, compile + link, the end-to-end test runner |

## Rules at the Rust/C boundary

1. **No non-local jump skips a live Rust frame.** Rust that generated code reaches is always entered through a shim in
   `asm.rs` that calls it, lets it return, and only then performs a jump it asked for (an owner resume, ExitThread).
   Rust calls guest code only through `boundary::call_guest`, which runs it under a barrier slot in the same chain as
   setjmp owners. A guest longjmp aimed below a host function lands on the barrier first: `call_guest` returns
   `Escaped` (or `Exit` for ExitThread), the host function returns at once and its frames unwind normally (Drop
   runs), and the shim continues the jump from generated-code frames only. A fiber switch suspends the Rust frames of
   the switching host function (they continue when switched back); a finished fiber's stack holds only the asm entry
   frame, and a fiber suspended mid-function is never deleted under its frames. Tested by
   `boundary::tests::longjmp_out_of_a_host_callback_keeps_rust_frames` and the `setjmp` homebrew test.
2. **No panic unwinds into C or guest frames, and none cascades.** Every entry point C or generated code calls, and
   every thread the runtime starts (guest threads' host threads, fibers), runs under `boundary::guard`, which reports
   the panic and exits with code 70: the first panic ends the process with its own message. Host functions are plain
   Rust functions (`HostFn`) called only from inside that guard. The runtime's locks recover instead of poisoning, so
   no later caller can turn one panic into a `PoisonError` elsewhere. Tested by
   `boundary::tests::panic_in_host_function_exits_cleanly`.
3. **Shared layouts match.** `tests/abi_layout.rs` (needs clang; `NXRT_CLANG` names it).

## Host functions

```rust
unsafe fn sum(c: *mut Cpu) {          // reads x0-x7, v0-v7, then the guest stack (Cpu::arg_x / arg_d ...)
    let c = &mut *c;
    c.ret_x(c.arg_x(0) + c.arg_x(1));
}
hostfn::add_override("_ZN2nn4test3sumEll", sum);   // every import of that name binds here
let addr = hostfn::register("vtable slot", sum);   // or an address to hand the guest
```

A host function may clobber x0-x18 and v0-v7/v16-v31 and must keep x19-x29, sp and v8-v15. To call guest code it uses
`boundary::call_guest` and returns as soon as that does not return `Returned`.

## Tests

`cargo test -p runtime` (about a second): unit tests for the dispatch table, the bridge registry, the loader
(relocation, binding, overrides, UNIMPL stubs, NRO), guest memory (reservation below 2^39, heap, QueryMemory,
write watch reporting exactly the written pages), the kernel primitives (handles, lock handoff order, condition
variables retaking the lock, sync waits, timed blocking), the asm boundary, and the C/Rust layout probe.

The M2 homebrew tests (`guest/tests/*.c`, expected output in `*.expected`) run end to end:

```
cargo build -p runtime -p translator
target/debug/nxrt-dev e2e --generator rust      # Rust translator (src/translator): all 8
target/debug/nxrt-dev e2e --generator mkw       # mkw-recomp gen.py + landing-pad owners (tools/mkw_gen.py): all 8
```

There is no devkitA64 or libnx here, so the tests are homebrew of their own: a small libnx-shaped SDK in
`guest/sdk/` (svc wrappers, a libc subset with printf, threads, mutexes and condition variables over the kernel's
arbitration SVCs, setjmp/longjmp, and a minimal Itanium C++ exception runtime with a DWARF unwinder), built with
clang `--target=aarch64-none-elf` and ld.lld (`guest/link.ld`) into two modules, `main` (the test, a PIE) and `sdk`
(a shared library main imports through its PLT, as a game imports nnSdk), each converted to an lz4 NSO with MOD0.
Imports named after host functions are linked against stand-ins (`guest/hostlib.c`) and bound to the runtime by the
loader. The expected output of `float_printf` is the output of the same source and printf run natively on x86.

The mkw generator path needs numpy and lz4: `py -m venv build/venv-mkw` and
`build/venv-mkw/Scripts/python -m pip install numpy lz4` (`NXRT_MKW_PYTHON` names another interpreter). It runs with
`-B` and `PYTHONDONTWRITEBYTECODE=1`, so nothing is written into the mkw-recomp checkout.

## Translator support it relies on

- **Landing pads as owners (C++ exceptions):** functions holding a landing pad are resumable owners like setjmp
  callers, so an unwinder's resume into the middle of one is taken by `rt_sj_owner`. Done in the translator;
  `tools/mkw_gen.py` gives gen.py the same owners.
- **libunwind's `ret x30` after a context restore** is lowered as a dispatch to x30 (a lifted `ret` is otherwise a C
  return). Done in the translator. The test SDK's unwinder uses `br x16` and does not depend on it.
- **The module entry as a seed:** the test crt0 gives the entry an FDE, so it is a function start either way.

## Not done (later milestones)

- Scheduling itself (virtual cores, preemption) is the os layer's, through the `Scheduler` hooks.
- Memory: MapMemory/stack moves, transfer and shared memory, read traps; QueryMemory reports the heap, stacks and
  module segments only.
- ExitThread from inside a guest fiber; a fiber resuming on another thread is untested.
- IPC/services (ConnectToNamedPort, SendSyncRequest): named fatals until nxnative's HLE layer comes.
- A `cdylib` (`nxrt.dll`): the game code links the engine statically into its DLL instead (src/cli).

Verified in a real run (2026-11-19, MK8D built by `nxrs build`, linked with `nxfront.lib`, real Vulkan): `program
0100152000022000: system tick 19.20 MHz; exclusive monitor on`, `process memory cap: 32768 MB`, 60 FPS on the title
for a minute, no crash; the eight homebrew tests pass with the monitor on (both toolchains).
