# 3DS-W2 — Guest host: `runtime3ds` runs the code (headless boot of the entry + first threads)

Plan: `docs/36-3DS-PORT-PLAN.md` §3 (the ABI kept verbatim), §4 (kernel/services mapping — this wave's rows
are the SVC/kernel ones only), §8 W2 row (the exit criterion), §9 (`src/runtime3ds` file table), §10 R7
(interpreter gate) and R10 (memory model). Brief: owner, 2026-11-27 (this wave's agent brief). Base commit
`7303a03`, worktree `D:\Projects\RecompLauncherEngine-3DS-W2`, branch `3DS-W2`. Donor read-only at the
seams: `abi/recomp.h` (the contract implemented), `abi/src/lib.rs` (the Context reference), `docs/interface.md`
(the semantics), `src/verify.rs` (the differential idea only). Unit: `src/runtime3ds` only, plus the root
`Cargo.toml` member line and this doc.

## Goal (the W2 row's exit criterion, measured)

`runtime3ds` runs the Entry tables (context loop, budget, exits, memory page tables, threads); an ARM11/
Thumb-1/VFP interpreter covers uncovered code; svc dispatch answers the kernel table; a title's entry and
first threads run headless with a svc/gap log; the interpreter differential vs its own golden set gives
0 mismatches on the tested classes. The title is Sonic Generations (the W1-staged dump), booted under a
budget to `build/DS/W2/`. Headless: CPU + memory + svc log only — no graphics, no audio, no window, no
`play.py`, no relink, `%LOCALAPPDATA%\nxrs` untouched.

## Expected files

| File | What |
|---|---|
| `src/runtime3ds/Cargo.toml` | the crate; deps: `translator3ds` (path), `windows-sys` (same 0.61 features the house crates use) |
| `src/runtime3ds/src/lib.rs` | crate root, `start()`/`Process3ds`, the boot options/report types |
| `src/runtime3ds/src/context.rs` | the Rust mirror of `recomp.h` v8 (Context, Host, Entry/Module/Origins shapes, Code fn type, stack_limit), the header's constants |
| `src/runtime3ds/src/dispatch.rs` | code library: DLL load (LoadLibraryW), `recomp_*` symbol tables, entry binary search, CRO module bases (`place`), stale filtering (`check`/`lookup`), natives switch |
| `src/runtime3ds/src/run.rs` | the run loop: budget slices, stack limit per call, exit handling (NONE/SVC/BUDGET/UNWIND), trail/ticks/mutex settlement at every host-loop boundary |
| `src/runtime3ds/src/mem.rs` | region table + `read_pages`/`write_pages` (2^20 entries each) over a 4 GiB-scale host reservation; lazy commit (64 KiB groups on first touch); GPU-visible pages stay null in `write_pages`; the read/write callbacks |
| `src/runtime3ds/src/thread.rs` | one host thread per guest thread; `Scheduler` trait (nxrt's shape); priorities, TLS slots, sleep/wake |
| `src/runtime3ds/src/svc.rs` | the svc dispatch registry + gap fallback (the `UNIMPL` discipline, one line per site) and W2's kernel table |
| `src/runtime3ds/src/hostfast.rs` | GetSystemTick answered in place (the `tick_*` fields) and the lent mutex (`wait_at`/`release_at`/`held`/`holding`/`held_floor`), lent/settled per interface.md §Ticks/§A mutex lent |
| `src/runtime3ds/src/interp/mod.rs` | the interpreter: single-instruction `interpret` for the callback, block mode for uncovered code |
| `src/runtime3ds/src/interp/arm.rs` | ARMv6K ARM decoder/executor |
| `src/runtime3ds/src/interp/thumb.rs` | Thumb-1 decoder/executor |
| `src/runtime3ds/src/interp/vfp.rs` | VFP2 (s0-s31, d0-d15, FPSCR incl. LEN/STRIDE short vectors, flush-to-zero) |
| `src/runtime3ds/src/prof.rs` | the miss recorder: `<data>\discovered.tsv` (hints, one address per line, bit 0 Thumb — the file W1's pipeline already reads) and `<data>\hotset.txt` (miss counts, the W7 websplit side) |
| `src/runtime3ds/src/bin/boot3ds.rs` | the headless harness: dump → translate → compile (donor flags) → load → boot under budget → `build/DS/W2/` logs |
| `src/runtime3ds/tests/*.rs` | context/mem/dispatch/thread/svc unit tests, interpreter golden cases, the differential, the host-run mirror proofs, the env-gated real boot |
| `Cargo.toml` (root) | one member line: `"src/runtime3ds"` |

Nothing outside that: no `cc.rs` change (the harness compiles the C directly through
`translator3ds::compile`, the way W0 did), no os3ds/gpu3ds/front, no Switch crate.

## Pinned facts (each with its source)

### The svc numbers (3dbrew `SVC`, revision 23981, and `Kernel_ABI`, revision 25030)

Kernel table this wave implements (number, name, actual register ABI):

| # | Name | Inputs | Outputs |
|---|---|---|---|
| 0x01 | ControlMemory | r0 op, r1 addr0, r2 addr1, r3 size, r4 perms | r0 Result, r1 addr_out |
| 0x02 | QueryMemory | r0 MemoryInfo*, r1 PageInfo*, r2 addr | r0 Result; struct + page written |
| 0x03 | ExitProcess | — | does not return |
| 0x08 | CreateThread | r0 priority, r1 entry, r2 arg, r3 stack_top, r4 processor | r0 Result, r1 handle |
| 0x09 | ExitThread | — | does not return |
| 0x0A | SleepThread | r0-r1 nanoseconds | — |
| 0x0B/0x0C | Get/SetThreadPriority | r1 handle (get) / r0 handle r1 prio (set) | r0 Result (+r1 prio on get) |
| 0x0F/0x10 | Get/SetThreadIdealProcessor | like 0x0B/0x0C | r0 Result (+r1 id on get) |
| 0x11 | GetCurrentProcessorNumber | — | r0 id |
| 0x13 | CreateMutex | r0 Handle* out, r1 initialLocked | r0 Result |
| 0x14 | ReleaseMutex | r0 handle | r0 Result (the donor's in-place release) |
| 0x15/0x16 | CreateSemaphore/ReleaseSemaphore | like the mutex pair | r0 Result (+r1 count out) |
| 0x17/0x18/0x19 | CreateEvent/SignalEvent/ClearEvent | r0 out*/handle, r1 resettype (create) | r0 Result |
| 0x1A-0x1D | CreateTimer/SetTimer/CancelTimer/ClearTimer | like the event pair | r0 Result |
| 0x1E-0x20 | CreateMemoryBlock/MapMemoryBlock/UnmapMemoryBlock | handle forms | r0 Result |
| 0x21/0x22 | CreateAddressArbiter/ArbitrateAddress | r0 handle, r1 addr, r2 type, r3 value, r4-r5 ns (the Kernel ABI's stack-args-in-registers rule) | r0 Result |
| 0x23 | CloseHandle | r0 handle | r0 Result |
| 0x24 | WaitSynchronization1 | r0 handle, r2-r3 timeout | r0 Result |
| 0x25 | WaitSynchronizationN | r0 s32* out, r1 handles*, r2 count, r3 waitAll, r4-r5 timeout | r0 Result, r1 index (the donor's in-place wait) |
| 0x27 | DuplicateHandle | r0 out*, r1 handle | r0 Result |
| 0x28 | GetSystemTick | — | r0 tick low, r1 tick high (the donor's in-place tick) |
| 0x2A | GetSystemInfo | r0 s64* out, r1 type, r2 param | r0 Result |
| 0x2D | ConnectToPort | r1 port name | gap-refused in W2 (named services are W3) |
| 0x32 | SendSyncRequest | r0 session | gap-refused in W2 |
| 0x3C | Break | r0 reason (+r1/r2 cro info on LOAD_RO) | logged, stops the boot |
| 0x3D | OutputDebugString | r0 str, r1 size | printed to the log |
| 0x52/0x53/0x54 | Invalidate/Store/FlushProcessDataCache | handle, addr, size | answered with success (no divergent caches yet) |
| 0x7C | KernelSetState | type, ... | answered with success for no-op types, else gap |

The three in-place numbers (0x28 tick, 0x25 wait, 0x14 release) are exactly the donor's
(`docs/interface.md` §Ticks/§A mutex lent, donor `codegen/arm.rs:65-67`), game-proven upstream.
The stubbed-svc result the kernel itself returns, `0xF8C007F4` (3dbrew `SVC`, "stubbed"), is this
wave's gap-refusal value; `0xD8E007EE`/`0xD8E007F0` (same page) are named error results where a
specific refusal shape is documented.

### The guest clock (GetSystemTick)

`SYSCLOCK_ARM11 = 268 111 856 Hz` — devkitPro/libctru `include/3ds/os.h`: "The clock rate for the Arm11
in CTR mode and in svcGetSystemTick" (`SYSCLOCK_SOC 16756991 × 2 × 4 × 2`). Decision recorded: the host
counts the budget in ticks 1:1 (one guest instruction = one tick), `tick_cost = 0` (a self-answered tick
is charged nothing beyond its block), `tick_floor = 0` (no host events come due mid-slice in W2),
`tick_end = slice start + slice budget`, so `tick_end - budget` is the guest tick now, as interface.md
specifies.

### The memory map (3dbrew `Memory_layout` rev 23986, `Memory_Management` rev 24597, libctru `os.h`)

Region table values (user-land NATIVE_FIRM application):

| Region | Guest range | Notes |
|---|---|---|
| ExeFS .code segments | from the exheader code-set info (text/rodata/data addresses; text is the entry point) | Sonic Generations: text 0x00100000 (0x39394C B), rodata 0x00494000 (0x36094 B), data 0x004CB000 (0x39AB4 B), bss 0xBBDD4 B — read from the staged `exefs/exheader.bin`, code from `exefs/code` (4,214,784 B = exactly the three page-rounded segments back to back) |
| Application heap | 0x08000000 up (MEMOP COMMIT; the app region is 0x04000000 for APPMEMTYPE 0) | allocated by the title's own `svcControlMemory` at boot |
| LINEAR heap | 0x14000000 up (MEMOP COMMIT with LINEAR 0x10000; phys = vaddr - 0x0C000000) | titles with kernel-release < 2.44 use this mapping (Sonic is a 2011 title) |
| Main thread stack | top 0x10000000, size from the exheader's stack field (Sonic: 0x4000 → bottom 0x0FFFC000) | 3dbrew: "StackSize from process exheader ... usually 0x4000, therefore the stack-bottom is usually 0x0FFFC000"; the stack field sits between the text and rodata code-set infos (Sonic's raw value 0x4000) |
| Shared memory mapping | 0x10000000 | 3dbrew `Memory_layout` user-land table |
| VRAM | 0x1F000000, 0x600000 | GPU-visible: `write_pages` entries stay null so writes hit the host callbacks (the explicit-write-watch the ABI designs for); reads direct |
| MMIO | 0x1EC00000, 0x400000 | reads answer 0 (logged once per page on first touch), writes logged and dropped in W2 |
| DSP RAM | 0x1FF00000, 0x80000 | plain RW (W5's dsp reads/writes it) |
| Kernel config page | 0x1FF80000, 4 KiB, read-only | app_memtype 0, memregion_sz[0] = 0x04000000 (APPMEMTYPE 0 per the FCRAM region table) |
| Shared config page | 0x1FF81000, 4 KiB | zeros + timeref fields in W2 |
| TLS | 0x1FF82000 + n*0x200 per thread | 3dbrew: "Number of threads * 0x1000 / 8" |
| Old LINEAR window | 0x14000000..0x1C000000 | 3dbrew: LINEAR ops "must be within the 0x14000000-0x1C000000 range when Addr0 is non-zero" |

`MemoryInfo` (QueryMemory) = {base, size, perm, state} (4 u32s); `PageInfo` = {flags u32}. MemoryState
enum per 3dbrew (FREE 0, RESERVED 1, IO 2, STATIC 3, CODE 4, PRIVATE 5, ...). MemoryOperation enum per
3dbrew: FREE 1, RESERVE 2, COMMIT 3, MAP 4, UNMAP 5, PROTECT 6, REGION_* 0x100..0x300, LINEAR 0x10000
(GSP heap = 0x10003 = COMMIT|LINEAR). MemoryPermission: R1 W2 X4 (write without read invalid).

### Host model decisions (recorded, per interface.md's host guidance)

- One host thread per guest thread, each with its own Context + VFP bank + page-table views; page tables
  shared. Budget in ticks, slices of 200k instructions; a blocked guest thread parks its host thread.
- The run loop settles the trail (mutex state, ticks, `done`/`ticks` reset) at every boundary into the
  host loop (the code calling svc/interpret or returning), as interface.md §A mutex lent requires.
- `tick_at`/`wait_at`/`release_at` are re-learned after each settle (set to 0 at settle, named again at
  the first in-place answer seen next slice). Lending resumes only when the mutex is free or the current
  thread's and nothing else waits on it — checked from the kernel state at lend time.
- EXIT_BUDGET with nothing spent: one instruction through the interpreter (the donor `verify.rs` discipline
  — "the block didn't fit, the interpreter takes a step").
- Stack: `recomp_abi::STACK` (1 MiB) below the caller's frame per call, `stack_limit` set before each entry.

## Plan

1. Wave doc (this file), root member line, crate skeleton.
2. `context.rs` + `dispatch.rs` + `mem.rs` with unit tests (layout vs the real header via a compiled
   probe; page-table/lazy-commit/watched-write behaviour; entry search/stale/module-base).
3. `interp/` (ARM + Thumb-1 + VFP) with the golden set; the differential harness (same sequences through
   recompiled C and through the interpreter, end-state compare, NaN-loose like `verify.rs`).
4. `svc.rs`/`thread.rs`/`run.rs`/`hostfast.rs` with unit tests (create/sleep/join, mutex lend/settle,
   arbitrate, events, gap refusals logged once per site).
5. `prof.rs` (miss recorder + file writes), `lib.rs` `start()`/`Process3ds`.
6. `boot3ds.rs` harness; boot Sonic Generations under a budget; coverage numbers (recompiled vs
   interpreted instructions) for the boot window; artifacts into `build/DS/W2/`.
7. Suites: `cargo test -p runtime3ds`; `cargo test --workspace --exclude renhdrum` (diff = the crate +
   the member line + the wave doc); evidence under `build/DS/W2/` (not committed).

## Verification steps (what the report will carry)

1. `cargo test -p runtime3ds` — every test listed with what it asserts; the differential's classes and
   counts; 0 mismatches.
2. The host-run mirror proofs (the W0 donor tests' Rust-side equivalent): generated C in a DLL, loaded
   through the mirror, state transitions asserted (tick answered in place, svc handed to the host, call
   and return, module base written and used, a stale function falling back to the interpreter).
3. The headless Sonic boot: `boot3ds` log lines (entry reached, threads created/run, svcs answered or
   gap-refused, clean stop), DLL/lib hashes, coverage numbers, under `build/DS/W2/`.
4. Root default suite green; the diff shows only `src/runtime3ds/`, the root member line, this doc.

Not opened this wave (later waves'/owner's D2 pending): `P:\Games\Nintendo\DUMPS\Star Fox 64 3D`,
`P:\Games\Nintendo\DUMPS\Pokemon Ultra Moon (3ds)`, `P:\Games\Nintendo\DUMPS\Pokemon Moon (cia)`.

## Outcome

**Built.** `src/runtime3ds` is in place as the plan's table names it: `context.rs` (the recomp.h v8
mirror, held to the real header by a compiled layout probe), `dispatch.rs` (DLL load, entry search,
CRO places, origins/stale, natives), `run.rs` (budget slices, the four exits, the settle, the
interpreter fallback), `mem.rs` (region table, 4 GiB reservation, 64 KiB lazy commits, page tables,
watched VRAM, MMIO, `load_bytes` for the loader), `thread.rs` (one host thread per guest thread,
the `Scheduler` shape), `svc.rs` (registry + gap fallback + the W2 kernel table), `hostfast.rs`
(the self-answered tick and the lent mutex), `interp/{mod,arm,thumb,vfp}.rs`, `prof.rs` (miss and
interpreted-block hints + the hot set), `src/bin/boot3ds.rs` (dump → C → clang → DLL → map → boot;
`--dll` reuses a build), and `tests/{interp_golden,host_run,differential}.rs` +
`tests/common/mod.rs`. The root `Cargo.toml` already carried the one member line.

Additions the boot needed beyond the first plan: the kernel config page's FCRAM-size word
(`0x1FF80040`, 64 MiB for APPMEMTYPE 0) is poked by the loader; `svcControlMemory` COMMIT succeeds
inside an existing mapping; LINEAR COMMIT with addr0 = 0 bumps in the `0x14000000..0x1C000000`
window and returns the chosen address; resource-limit svcs `0x38/0x39/0x3A` answer a handle and
plausible limits; the boot's svc ledger and interpreted-block starts are in the report/log. The
boot now waits for every host thread to leave its slice before it reports (the first try dropped
`Process3ds` under a running thread and crashed in `Library::entries`).

**Tests.** `cargo test -p runtime3ds`: 22 unit + 13 interpreter golden + 2 host-run + 1
differential = 38 passed (the differential skips without `NXRS_3DS_CC_TESTS`). The differential
runs ARM data-processing/multiply/load-store/shift plus VFP vldr/vadd/vmul/vcmp/vcvt through the
interpreter and through the recompiled C and compares r0–r12, nzcv, s0–s4, fpscr flags and a heap
word: **0 mismatches**, recompiled > 0. The interpreter golden cases are the classes the donor
leaves to `interpret` plus common ones (shifts/carry, multiplies, extra load/store, media,
exclusive monitor, Thumb-1 including pc-relative/BL, VFP arithmetic/compare/conversion/flush);
several first-pass test encodings were wrong and were replaced with clang-assembler-verified ones.
With `NXRS_3DS_CC_TESTS=1` the host-run proofs build DLLs and execute them through the mirror: the
entry answers ticks in place after learning the site, hands an unknown svc over with a gap line,
calls and returns; a patched function goes stale, its entry refuses and the interpreter runs it.

**Verified.** Headless Sonic (program `0004000000062300`, decrypted .3ds): 130 779 entry points,
DLL FNV1a `782A2BA22EF1144F` (92 938 240 bytes); the main thread enters at `0x00100000` with the
stack at `0x10000000`/0x4000 and runs 799 987 recompiled instructions, then 9 980 237 interpreted
(92.6% of the 10.3M-tick window) in a 20 s budget. The svc ledger: `GetResourceLimit`,
`GetResourceLimitCurrentValues`, `CreateAddressArbiter`, two `ControlMemory` (one LINEAR 0x3C80000
at 0x14000000), `CloseHandle`, all answered (no gaps); stop `Time`, clean, every thread joined.
Artifacts under `build/DS/W2/sonic/`: `boot.log`, `run-console.txt`, `discovered.tsv`, `hotset.txt`,
the DLL and its `.lib`/`.exp`. `cargo test --workspace --exclude renhdrum` is green; the diff is
`src/runtime3ds/`, the wave doc, and the already-committed member line.

**Unproven / blocker.** The exit criterion's *first threads* are not reached. After the linear
allocation the title returns into `0x00336854`, which is a branch target inside the function whose
entries are `0x00336850`/`0x0033683C`; `lookup` (entry-table binary search, the ABI's shape) has no
entry for it, so the interpreter starts **inside** the function at the memclr loop
(`stmhs/subshs/bhs`) without the prologue, and the loop then runs on a size/return state it did not
set up. `CreateThread` is never called, so the boot stops at the wall-clock cap instead. This is the
ABI assumption the brief said to report rather than force: the entry table covers function starts,
but real code resumes at internal labels after indirect calls/returns; the generated C's own
dispatch switch names those labels, so a host-side "containing function" lookup would be the
smallest fix, but the first attempt at it livelocked in the block-budget resume and was reverted to
the documented interpreter fallback. Next step (W3): either teach the host's lookup to resume a
containing function at an internal label with a fresh budget, or have W0/W1 discovery/exports seed
those targets as funcs; named services then carry the title to its thread creation.

**Slips.** One: the first boot attempt dropped `Process3ds` under a running host thread (fixed by
the join in `start`); the containing-function lookup experiment was reverted, not shipped. No files
outside the unit were changed.

