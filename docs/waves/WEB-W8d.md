# WEB-W8d - Chrome FPS: why the game's code is ~3x slower as wasm

**Brief (2026-11-27):** Anime Girls Sun of a Beach (0100C6801E0FA000) beach gameplay in Chrome from 51.7 FPS (WEB-W8c
`final`) toward 60. W8c: the main game thread runs 17.6 ms of guest code a frame as wasm, native 5.7 ms for all its
work. Find why the translated code is ~3x slower as wasm and fix the biggest cause. Ruled out before: GPU worker GC
(W8c), FZ fast path + relaxed SIMD (W8b), whole-module wasm-opt -O2 (W8c), the lanes' pools lock. Branch `web-w8d`;
deadline 2 h. Library web folder as W8c (relinked with the launcher's arguments, backed up first).

## Plan

1. A per-function CPU profile of the main game thread (the library build relinked with `--profiling-funcs`,
   `build/WEB/W8d/tools/drive.mjs --cpu 100,110`) to name the hot guest functions and what else the thread runs.
2. For the hottest: their generated C, the wasm, and the cost candidates with evidence (memory64 access path,
   the Cpu struct traffic, i64/i128 helpers, indirect dispatch, -O2 vs -O3).
3. Fix the biggest cause in source (translator runtime headers / `cc.rs` web flags), rebuild, measure against the
   W8c final setup with several runs a side.

## Expected files

- `src/translator/runtime/*.h` or `src/cli/src/cc.rs` (web arms only), their tests.

## Results (2026-11-27)

Evidence: `build/WEB/W8d/` (`tools/drive.mjs`: CDP driver, refuses to start while another Chrome with
`--use-adapter-luid` / `--remote-debugging-port` runs, X every 6 s from 40 s, PNGs at 60/100/140 s, `--cpu a,b` a
CPU profile of every worker, every CDP call bounded at 8 s; `tools/fps.mjs`, `tools/prof.mjs` self time per function,
`tools/cnt.py` the operation mix of a list of generated functions, `tools/threads.ps1` per-thread CPU time of the
renderer; `bench/` the microbenchmark). Chrome on GPU 1 (`--use-adapter-luid=0,113326`), dev server on port 8794.
W8c's own tools and run folders were gone with its worktree; these are rewritten.

### No change kept

No source change in this wave: the measurements below do not support the premise that the translated code runs ~3x
slower as wasm, so there was no code-generation cause to fix. The library web build is relinked from this branch
without names (the same code as W8c's `final`, from the merged commit).

### The profile (`runs/prof2`, library build relinked with `--profiling-funcs`)

Main game thread (`w15`), 7.06 s profiled from ~106 s (38-48 FPS under the profiler, ~300 frames):

| | ms in the window | share |
|---|---|---|
| guest functions `f_*` / `fb_*` | 4,847 | 69% (~16 ms a frame) |
| `_do_futex_wait` | 1,374 | 19.5% |
| `rt_dispatch` | 187 | 2.7% |
| `emscripten_futex_wake`, `_emscripten_get_now`, `svc_inner`, `rt_sj_owner` | 143 | 2% |

Flat, as W8b found: the hottest guest function is 1.4% (`f_1d10bab30`), the top 280 functions make 4.5 s. Note that
`_do_futex_wait` (a thread blocked in `Atomics.wait`) is counted as busy: V8's sampler records a thread's last
position whether it runs or not, so the profile's "ms" are **wall time**, not CPU time.

### What the hot code is made of (`tools/cnt.py`, 255 of the 280 hottest main-thread functions)

78,559 lines of generated C: 51,435 `c->x[...]` and 36,262 `c->v[...]`/`VREG`/`VS*` accesses (guest registers in the
`Cpu` struct), 24,481 guest loads/stores, 6,488 scalar FP ops (`fp_op_s`), 951 `cmp_s`, 2,565 direct guest calls,
527 `rt_dispatch`. The wasm of `f_1d10bab30` (`llvm-objdump` of `main_0107.o`): a 1,216-byte shadow-stack frame,
vector lanes written through it (v128 store, f32 store into a lane, v128 load: a store-forwarding stall on x86), every
guest address formed by a separate `i64.add` (the memarg offset is never used: `ea = x + off` is a plain 64-bit add,
which LLVM may not fold without `nuw`).

### Microbenchmark: the same shapes natively and under V8 (`bench/bench.c`)

The generated code's shapes - eight scalar FP ops through `c->v` (`fp_op_s`, FZ off and on), guest register and
memory traffic as gen.py writes it, a guest caller with two guest calls (`FN_ENTER`, write-back) - compiled with the
native game flags (llvm-mingw clang -O1, x86-64-v3) and with the web flags (emcc -O2, memory64, pthreads, relaxed
SIMD, tail calls; and the same as wasm32), run under emsdk's Node 24 (V8), best of 5, two rounds:

| ns an iteration | native | wasm64 (V8) | wasm32 (V8) |
|---|---|---|---|
| 8 FP ops, FZ off | 26.5 | 11.6 | 11.6 |
| 8 FP ops, FZ on | 26.6 | 12.7 | 12.7 |
| guest register + memory traffic | 1.37 | 1.80 | 1.73 |
| caller + 2 guest calls | 7.69 | 8.21 | - |

So, per shape: integer guest code 1.3x native, guest calls 1.07x, scalar FP *faster* as wasm here (the native path
pays its own FREDO checks), the FZ software fast path costs +10%, memory64 against memory32 4% (bounds checks are
trap-handled, as W8b found). Liftoff-only (`--liftoff --no-wasm-tier-up`) did not change the numbers (Node tiers
these tiny loops anyway). Nothing here is 3x.

### Named cause: the main thread is not CPU-bound at 3x; it runs ~11 ms a frame and waits the rest

W8c's 17.6 ms of "guest code a frame" is sampled wall time; native's 5.7 ms is CPU time from the metrics CSV. They do
not measure the same thing. `runs/cpu1` (the library build relinked without names, no profiler, 54.6 FPS over
93-121 s, beach gameplay with two spiders at `t100.png`) measured the renderer's threads' real CPU time
(`TotalProcessorTime`) over 10.03 s from 103 s:

| renderer thread (busiest first) | CPU in 10.03 s | busy | a frame at ~55 FPS |
|---|---|---|---|
| 1st (the game's main thread by its profile share) | 5,969 ms | 59.5% | ~10.9 ms |
| 2nd | 4,734 ms | 47% | ~8.6 ms |
| 3rd | 3,625 ms | 36% | ~6.6 ms |
| 4th / 5th | 1,828 / 1,688 ms | 18% / 17% | ~3.3 / 3.1 ms |

(Threads are told apart by CPU rank only; which thread is which was not recorded.) No renderer thread is busy more
than 60% of the time. The main thread's ~10.9 ms a frame of CPU (all its work, not only guest code) is ~1.9x native's
5.7 ms, in line with the microbenchmark's 1.0-1.3x per shape plus the web's own engine work on that thread; the rest of
its ~18 ms frame is waiting (the profile's futex waits: 19.5%; the game's SleepThread, kernel waits and GPU fence,
W8b/W8c) or not scheduled. So the FPS limit is how the main thread's frame is serialized with other threads
(handoffs, waits for the GPU worker and the replay thread), not V8's code: making the guest code 2x faster would save
~5 ms only where the frame is not already waiting.

### Other findings

- **The `after3` abort reproduced and is now named** (`runs/prof1`, 52 s, during the level load right after
  `A4B4G4R4_UNORM_PACK16 is not supported`): `[RED] web: submission 787 not complete 5.0 s after the GPU worker handed
  it to the GPU; the game stops (GPU completion budget)` - W8c's red line working. The boot had already stalled
  ~10 s at 3 presents with the main thread in `nvnSyncWait` (`warning: 6 s without a new image` at 33 s). Not this
  wave's scope; no TDR in the System log. Two of 11 runs across W8c/W8d.
- Profiling every worker through CDP needs bounded calls: a worker blocked in `Atomics.wait` or a stopped page never
  answers (`prof1`'s driver hung on one); `drive.mjs` gives up on a call after 8 s.

### Unproven / next

- Next: what the main thread waits for in each frame (a wait timeline per thread: futex waits with who wakes them,
  the GPU fence, SleepThread), since its CPU is ~11 ms of an ~18 ms frame. One run, one window, on a busy machine:
  repeat on a quieter one. Name the threads (the per-thread CPU table by thread id, matched with the profile's
  targets) before acting on the 2nd/3rd rows.
- Smaller code costs seen in the wasm but not measured in the game: the shadow-stack lane writes of the SIMD
  helpers (store-forward stalls), unfolded `ea = x + off` adds.
- The GPU-completion abort at level load (1 in ~5-9 runs).

## Safety

Chrome WebGPU on GPU 1 only; one game Chrome at a time (two of mine, sequential; none started while another
session's ran); screenshots >= 20 s apart; builds and relinks at below normal with `EM_CACHE=target\emcache`; only my
own processes stopped, by PID; no native game runs; no TDR (System log checked after the abort). Library web build
backed up (`build/WEB/W8d/backup-w8c-final`) before the first relink.
