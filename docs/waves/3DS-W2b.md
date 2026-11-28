# 3DS-W2b — Memory services: the LINEAR heap end, and the threads it unblocks

Plan: `docs/36-3DS-PORT-PLAN.md` §4 (kernel rows), §8 (W2 row, split note), §10 R7/R10.
Brief: owner, 2026-11-28. Base `7c2ebba`, worktree `D:\Projects\RecompLauncherEngine-3DS-W2b`,
branch `3DS-W2b`. Donor pin `d143f2c5` read-only. Unit: `src/runtime3ds` (the kernel memory-service
answers and the memory/layout facts they expose) plus this doc. Translator3ds, os3ds, gpu3ds and the
launcher are out of scope; a defect found there is reported with evidence, not fixed.

## Goal

Sonic Generations' early heap clear walks `r0` from `0x1400F7F0` by `0x100C80` per call past the
mapped LINEAR window (`0x14000000..0x1C000000`); the first store fault is at `0x1C00ECB0`, and after
60 s `r0 = 0x2B5A00B0` (~374 MB cleared). The size the walk trusts comes from a kernel answer this
unit gives. Find the wrong answer, fix it against documented kernel behaviour, prove the clear
finishes, and then prove `svcCreateThread` and the created threads run headless. Deliverable: the
fix + regression tests + the thread log, committed on `3DS-W2b`.

Titles: Sonic Generations only (decrypted .3ds, staged from
`D:\Projects\RecompLauncherEngine\build\DS\W1\sonic\Sonic_data`, read-only). Headless: no GPU, no
audio, no window, no `play.py`, no relink, `%LOCALAPPDATA%\nxrs` untouched.

## Expected files

| File | What |
|---|---|
| `src/runtime3ds/src/svc.rs` | the kernel answers that are wrong (resource limit values, the kernel-ABI output registers) and their regression tests |
| `src/runtime3ds/src/lib.rs` | only if the heap/used-memory accounting the answers need lives here |
| `src/runtime3ds/src/mem.rs` | only if the region/page facts the answers expose are wrong |
| `src/runtime3ds/src/bin/boot3ds.rs` | the bounded `RUNTIME3DS_PROBE` svc dump (temporary evidence, gated) |
| `src/runtime3ds/tests/*.rs` | the handler regression tests |
| `docs/waves/3DS-W2b.md` | this plan, then the outcome |
| `build/DS/W2b/` | boot logs, DLL hash, thread log (not committed) |

## Pinned expectations (with sources)

### Exheader (staged `exefs/exheader.bin`, parsed per 3dbrew `NCCH/Extended Header` rev 24348)

- Program ID `0004000000062300`; text `0x00100000` / `0x39394C`; rodata `0x00494000` /
  `0x36094`; data `0x004CB000` / `0x39AB4`; bss `0x0BBDD4`; stack size `0x4000`.
- ACI starts at exheader `0x200`; resource limit descriptors at ACI `0x10` = exheader `0x210`
  (`16 * 2` bytes), all zero in this title.
- ACI `0x16F` (exheader `0x36F`) resource limit category = `0` (APPLICATION).

### Resource limits (Azahar `src/core/hle/kernel/resource_limit.cpp`, `<sources>/ResourceLimitList`)

For `ResourceLimitCategory::Application`, the kernel object holds:

| LimitableResource | Value |
|---|---|
| Priority (0) | `0x18` |
| Commit (1) | `APPMEMALLOC` = `0x04000000` (64 MiB, APPMEMTYPE 0) |
| Thread (2) | `0x20` |
| Event (3) | `0x20` |
| Mutex (4) | `0x20` |
| Semaphore (5) | `0x8` |
| Timer (6) | `0x8` |
| SharedMemory (7) | `0x10` |
| AddressArbiter (8) | `0x2` |
| CpuTime (9) | preemption disabled (this title's descriptor is legacy; PM sets a value) |

The title asks only for `RESLIMIT_COMMIT` (1) with `GetResourceLimitCurrentValues` (disassembly:
wrapper at `0x0043C770` passes `names[0] = 1`, `count = 1`, process handle `0xFFFF8001`,
`0x0043C7A8: svc 0x3A`). `GetResourceLimitLimitValues` (0x39) is never called by this title.

### Kernel ABI vs the official wrapper (3dbrew `Kernel ABIs` rev 25030, libctru `source/svc.s`)

- The wrappers in the title (`0x0043C9A8` for 0x38, `0x0043C7A8` for 0x3A, `0x00182A74` for 0x01)
  are the official shapes: the kernel's real outputs are `r0 = Result`, `r1..` = outputs.
  `svcGetResourceLimit` stores `r1` (the handle) to the caller's pointer; a handler that writes the
  handle to memory at `r0` and leaves `r1` untouched is wrong.
- `GetResourceLimitCurrentValues` writes each 64-bit value at the address in `r0` and returns the
  result in `r0`; current commit for this title is the bytes it has committed (code set + stack +
  heap), not 0.
- `ControlMemory` returns `addr_out` in `r1`; `QueryMemory` returns base/size/perm/state/page in
  `r1..r5`. The current `QueryMemory` handler writes the official-wrapper struct at the pointers in
  `r0`/`r1`, which is not the kernel ABI the SVC instruction delivers (this title never calls 0x02,
  so it is not the walk; it is fixed only if the dump shows it and it stays inside this unit).

### Memory layout (3dbrew `Memory layout` rev 23986, `Configuration Memory` rev 24074)

- APPLICATION region `0x04000000` (APPMEMTYPE 0), LINEAR window `0x14000000..0x1C000000`
  (`0x08000000`, phys `vaddr - 0x0C000000`), main stack top `0x10000000`.
- Config page: APPMEMTYPE `0x1FF80030`, APPMEMALLOC `0x1FF80040`, SYSMEMALLOC `0x1FF80044`,
  BASEMEMALLOC `0x1FF80048`. The W2 harness pokes `0x1FF80040 = 0x04000000` and wrongly writes
  `0x04000000/0x02C00000/0x01400000` at `0x1FF80058..0x60` while leaving `0x44`/`0x48` zero.

The expected heap arithmetic for this title is therefore built from `APPMEMALLOC = 0x04000000` and
the current commit (a few MiB), so the LINEAR allocation stays inside
`0x14000000..0x1C000000` and the clear must end before `0x1C000000`. A walk to `0x2B5A00B0` is not
legitimate under the documented map.

## Plan

1. Write this doc.
2. Dump the answers (bounded, `RUNTIME3DS_PROBE`): `0x38`/`0x3A`, each `0x01`, and `0x30`/`0x02` if
   they occur: arguments, buffer addresses, counts, bytes written, caller PC.
3. Identify the arithmetic that produces the walk's size; fix the wrong kernel answer in `svc.rs`
   (or the region/config values in `lib.rs`/`mem.rs`), with the documented value.
4. Regression test first: the handler's written values against the APPLICATION resource-limit table
   (fails before the fix).
5. Re-run the headless boot with the same harness: the clear finishes, `CreateThread` appears, every
   created thread is logged (handle, entry, priority, stack top) and runs (its own svc lines / a
   known guest function / instruction count), then a clean stop. Logs go under `build/DS/W2b/`.
6. If the defect is outside this unit, stop and report the cross-unit finding with addresses and the
   smallest repro.

## Verification

- `cargo test -p runtime3ds` green, new handler/regression tests included; `cargo test --workspace
  --exclude renhdrum` green.
- The boot: the fault count collapses after the clear; `CreateThread` (0x08) svc lines; created
  threads run; clean stop; before/after numbers (faults, ticks, interpreted %, the walk's end).
- Diff limited to `src/runtime3ds/` + this doc.

## Outcome

### Diagnosis (apart from the correction)

The runaway walk was **not** a memory-service answer. The bounded dump (`build/DS/W2b/sonic-probe/`,
`sonic-fix1/`) showed the title asking the right questions:

- `0x38` at site `0x43C9B0`: `r0=0xFFFFFE0`, process `0xFFFF8001`; our answer left `r1=0xFFFF8001`.
- `0x3A` at site `0x43C7AC`: `out=0xFFFFFD8`, handle `0xFFFF8001`, `names[0]=1` (RESLIMIT_COMMIT),
  count 1; our answer wrote `0` to the value.
- `0x01` LINEAR: op `0x10003`, size `0x3C80000`, chosen base `0x14000000`;
- `0x01` COMMIT: op `3`, addr `0x08000000`, size `0x380000` (the APPMEMALLOC split: 0x380000 +
  0x3C80000 = 0x04000000).

The clear is the libctru-style `memset` at `0x00336838` (`stmhs r0!,{r2,r3,r12,lr}` twice,
`subshs r1,#32`, `bhs`). Executing it through `interp::step` per instruction showed PC stuck on
`0x00336854` while `r0` advanced 16 bytes and `r1` stayed `0x100C80`: the ARM interpreter's
`LDM/STM` handler returned `Step::Next` **without advancing r15** (`multiple_transfer`,
`src/runtime3ds/src/interp/arm.rs`). `Step::Next` means "r15 is already at the next instruction"; the
store therefore repeated forever, which is the whole "390 MB walk". `r1` never decremented, so no
service supplied that size. The condition flags and the store itself were correct; the W2
differential missed it because its driver advanced `r15` itself on `Next`.

After the PC fix the clear finished (`probe return: from 0x336850 to pc 0x28104C`, `r1=0`) and the
boot reached object/service setup. The dumps then exposed the second class of defect: the kernel's
object-creating svcs must answer the handle in **r1** (Kernel ABI), not by writing through the
wrapper's `r0` pointer. Our `CreateAddressArbiter` left `r1` stale, so the title's arbiter handle
was `0`, and its `ArbitrateAddress` wait parked on handle 0 forever. `DuplicateHandle(0xFFFF8000)`
also failed (pseudo handle not in the table), taking the title into the error path.

### Correction

- `interp/arm.rs`: `multiple_transfer` sets `r15 = address + 4` on every non-PC `Step::Next` (both
  load and store). Other ARM/Thumb/VFP handlers were audited; this was the only omission.
- `svc.rs`: kernel-ABI outputs in registers for `0x02 QueryMemory` (`r1..r5`), `0x13/0x15/0x17/0x1A/
  0x1E/0x21` (handle in `r1`), `0x27 DuplicateHandle` (copy in `r1`), `0x37 GetThreadId`, `0x38
  GetResourceLimit` (limit handle in `r1`), `0x16 ReleaseSemaphore`, `0x29/0x2A/0x2B/0x2C`
  (`r1`-`r2`).
- `svc.rs`: `resource_limit` matches the APPLICATION category table (priority `0x18`, commit
  `APPMEMALLOC=0x04000000`, thread/event/mutex `0x20`, semaphore/timer `0x8`, shared memory `0x10`,
  arbiter `0x2`, CPU time preemption disabled); `CloseHandle` accepts the limit handle.
- `svc.rs`/`lib.rs`: `ArbitrateAddress` types per 3dbrew (1 wait, 2 decrement+wait, 3 wait+timeout,
  4 decrement+wait+timeout), comparing the word signed against `value`; the wait no longer
  misreads type 1 as a decrement.
- `lib.rs`: `DuplicateHandle` accepts `0xFFFF8000`/`0xFFFF8001` (active thread/process pseudo
  handles); `create_thread` starts the guest with `r0 = arg`; an exited thread returns `Parked` so
  its host thread stops; `report` recognises "every guest thread ended" as `Stop::Idle`.
- `bin/boot3ds.rs`: config page at the documented offsets (`APPMEMTYPE 0x1FF80030`, `APPMEMALLOC
  0x40`, `SYSMEMALLOC 0x44`, `BASEMEMALLOC 0x48`), replacing the stale `0x58/0x5C/0x60` pokes.

### Tests

`cargo test -p runtime3ds`: 29 unit + 3 differential + 2 host-run + 13 interpreter golden + 1
thread proof = 48 passed. New tests:

- `interp::tests::a_store_multiple_advances_pc` — `stmhs r0!` stores the four words, writes back
  `r0`, and leaves `r15=address+4`; a load multiple without pc does the same. Fails on the old
  interpreter (`r15` stays).
- `svc::tests::resource_limits_match_the_application_category` — the APPLICATION table. Fails on
  the old `2..=8 => 0x40`.
- `svc::tests::create_address_arbiter_answers_the_handle_in_r1` — handle in `r1`, the wrapper's
  `r0` pointer untouched, and the handle works in `ArbitrateAddress`. Fails on the old handler
  (`r1=0`).
- `svc::tests::get_resource_limit_answers_the_handle_in_r1` — `r1=0x2000`, and `CloseHandle`
  accepts it. Fails on the old handler (`r1=0xFFFF8001`).
- `svc::tests::info_svcs_answer_in_registers` — `QueryMemory` in `r1..r5`, no struct written.
- `svc::tests::resource_limit_values_are_written_to_the_out_array` — the 0x39 table.
- `tests/thread_proof.rs::create_thread_runs_the_created_thread` — `svcCreateThread` spawns a guest
  thread, its entry sees `r0=arg` and its stack top, both threads exit, `Stop::Idle`,
  `threads_ran=2`; log at `build/DS/W2b/thread-proof.txt`.

`cargo test --workspace --exclude renhdrum` is green.

### Verified — the boot before/after

Same harness, decrypted Sonic, same DLL (`fnv1a:782A2BA22EF1144F`, 92,938,240 bytes). Before numbers
are W2's 60 s run; after is `build/DS/W2b/sonic/` (20 s, evidence log committed beside this doc's
report).

| | W2 (before) | W2b (after) |
|---|---|---|
| stop | Time (spinning in the clear) | Time (parked on a sticky event waiting for the srv reply) |
| ticks | 25.8M | 2.8M |
| recompiled | 799,987 | 2,672,008 |
| interpreted | 24,857,357 (96.88%) | 75 (0.00%) |
| faults | 252,291,248 | 0 |
| clear | `r0` `0x1400F7F0 -> 0x2B5A00B0`, never returns | `r1` `0x100C80 -> 0`, returns to `0x28104C`, `r0=0x14110480` |
| svcs | 6 | 11 (0x21, 0x38, 0x3A, 0x23, 2x0x01, 0x27, 2x0x2D, 0x32, 0x22) |
| threads | 1 | 1 |

The title now allocates (`0x3C80000` LINEAR at `0x14000000`, `0x380000` COMMIT at `0x08000000`),
clears, initialises the CRT/locks, duplicates the active-thread handle, and reaches
`ConnectToPort("srv:")` / `SendSyncRequest` — then parks on the event the srv reply would signal.
The two gaps are exactly W3's named services:

```
gap [1x] svc 0x2D ConnectToPort("srv:") at 0x45c830
gap [1x] svc 0x32 SendSyncRequest(session 0x4a3d6c) at 0x449da0
gap [1x] svc 0x2D ConnectToPort("err:f") at 0x45c830
```

### Unproven / next step

- The title's **own** `svcCreateThread` is not reached: its main thread waits for the `srv:` reply
  (W3). The kernel thread path itself is proven by `tests/thread_proof.rs` (CreateThread, argument,
  stack top, both threads ran, clean `Stop::Idle`).
- `GetResourceLimitCurrentValues` still answers current commit `0`; runtime3ds has no per-process
  commit accounting yet. The title reads it once before allocating and its split equals
  APPMEMALLOC, so this run does not depend on it, but the value is incomplete against the kernel.
- No graphics/audio/window; the next step is W3's `srv`/`err:f` (and the rest of the named services)
  so the title can finish service init and create its threads.

