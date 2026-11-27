# WEB-W8e - Chrome: the level-load GPU completion abort, then the main thread's waits

**Brief (2026-11-27):** Anime Girls Sun of a Beach (0100C6801E0FA000), Chrome on GPU 1, beach ~52-55 FPS, goal 60.
(1) About one run in nine stops at the level load (~50-60 s) with W8c's red line "submission N not complete 5.0 s
after the GPU worker handed it to the GPU" (W8c `after3`, W8d `prof1`; no TDR). Find why a submission takes > 5 s on
the web and fix the cause (raise the budget only if the work is shown to be legitimately long on a healthy GPU).
(2) W8d: the main thread is ~60% busy (10.9 ms CPU a frame), ~40% waiting: a per-thread wait timeline over a few
frames, then fix the biggest avoidable wait; several runs a side against the current build. Branch `web-w8e`;
deadline 2 h. Library web folder as W8c/W8d.

## Plan

1. **The abort.** Suspects: Dawn compiling the level's new pipelines in Chrome's GPU process ahead of the submission
   (`createRenderPipeline` is synchronous on the worker but compiled in the GPU process, after the worker has already
   stored the "queued" word that starts the engine's 5 s budget); a missed wake between `gpu-worker.js` and the
   completion loop (`ring::wait_until` re-checks every 100 ms, so a missed wake alone cannot reach 5 s);
   `onSubmittedWorkDone` never resolving. Evidence: the worker logs every submission the GPU finishes >= 1 s after
   the hand-over, and any still pending at 3 s, with the pipelines made before it; `tools/gpuproc.ps1` samples the GPU
   process's CPU and GPU 1's utilization each second. Then fix the cause in the worker (`src/web/gpu-worker.js`).
2. **The waits.** The CPU profiles of every worker (W8d `runs/prof2`, names kept) are wall-clock samples with stacks:
   a blocked thread is sampled at its wait. `tools/waits.mjs` classifies each main-thread sample as running or waiting
   and names the wait by the first engine frame above the futex/sleep primitive, per frame. Fix the largest
   avoidable wait; measure several runs a side.

## Expected files

- `src/web/gpu-worker.js` (submission timing, pipeline creation), possibly `src/graphics/src/web/*.rs`,
  `src/graphics/src/vk/queue.rs` (web arms only), whatever the wait timeline names.

## Results (2026-11-27)

Evidence: `build/WEB/W8e/` (`tools/drive.mjs`: W8d's driver plus `--profile <dir>` (a new Chrome profile: Dawn's
pipeline cache and the OPFS data cache cold), `tools/gpuproc.ps1` (each second: the CPU the game Chrome's GPU process
used, its busiest thread, GPU 1's utilization -> `gpuproc.tsv`), `--cycles <s>` (`tools/cycles.ps1`: per-thread CPU
from `QueryThreadCycleTime` next to `TotalProcessorTime`); `tools/waits.mjs`, `episodes.mjs`, `strip.mjs`,
`callers.mjs`: a thread's waits from a `--cpu` profile). Chrome on GPU 1 (`--use-adapter-luid=0,113326`); dev servers
on 8795 (`--live`: this worktree's scripts) and 8796 (the library build's own scripts, = `gameplay-harness`).

### (1) The level-load abort: the pipelines were compiled inside the GPU budget

**Diagnosis.** The worker now logs every submission the GPU finishes >= 1 s after the hand-over (with the pipelines
and shader modules made since the previous submission), and any still pending at 3 s.

| run (cold profile unless said) | slow submissions (>= 1 s) | GPU process during the level load (46-51 s) | GPU 1 |
|---|---|---|---|
| `l1`, scripts as `gameplay-harness` + the log lines | **12**: 1.03-1.09 s at boot (25.9 s), **1.81-1.88 s** (48.9 s, after 13 pipelines + 14 modules), **1.46-1.47 s** (50.6 s, after 12 pipelines + 18 modules) | **1,016-1,188 ms of CPU a second, the busiest thread 891-1,016 ms** (one thread saturated) | **0%** in those seconds |
| `a1`, async pipelines, warm profile | 0 | no second above 900 ms | - |
| `c1`, async pipelines, cold profile | **0**; instead `pipelines: 41 compiled by Chrome's GPU process in 1.09 s` (20.7 s) and `1 ... in 1.44 s` (22.6 s), before the submission | 49 s: **4,250 ms of CPU in one second, the busiest thread 609 ms** (compiled on ~7 threads at once) | 0% |

So a slow submission is Chrome's GPU process compiling pipelines on its one GPU thread while the GPU is idle - not
GPU work, not a lost wake (`ring::wait_until` re-checks every 100 ms) and not an `onSubmittedWorkDone` that never
resolves (every slow one did resolve). `createRenderPipeline` returns at once on the worker; Dawn compiles the
pipeline when the GPU process reaches the command, after the worker has already stored the next submission's
"queued" word, which starts the engine's 5 s GPU completion budget (`queue.rs` `completion_loop`, W4c). A level load
puts 12-13 new pipelines (cold) ahead of one submission at up to ~1.4 s each; on a loaded machine that passed 5 s:
the abort of W8c `after3` and W8d `prof1`. No TDR, no device loss, GPU healthy.

**Correction** (`src/web/gpu-worker.js`, JS only): pipelines are made with `createRenderPipelineAsync` /
`createComputePipelineAsync`, and the ring loop waits for every pending one before the next submission (or a destroy).
The compile is then CPU-side work before the hand-over, as a native driver's `vkCreate*Pipelines` returns compiled:
outside the GPU budget, which stays 5 s (nothing raised). An invalid descriptor (the async call rejects) falls back to
the synchronous call, so the same invalid object and validation message as before. Side effect: Chrome compiles the
async pipelines on several GPU-process threads, so the level load is shorter: frames around it 37.0 / 57.5 FPS
(`c1`) against 31.3 (`l1`) in the window after the load.

### (2) The main thread's waits

**Per-thread wait timeline** (W8d `runs/prof2`, the library build relinked with names, 7 s of beach at ~43 FPS under
the profiler; V8 samples a blocked thread at its wait, so the samples are wall time). Main game thread (`w15`):

| | ms in 7.0 s | a frame (~300) | called from (guest) |
|---|---|---|---|
| running | 5,631 (80.4%) | ~18.8 | - |
| SleepThread (`svc 0x0B` -> `std::thread::sleep`) | ~550 | 1.8 | **one call site**: `f_1d0858ab0 < f_1d0846240 < f_1d0670e80` |
| kernel waits (`kernel::block`: events, condition variables, address waits) | ~580 | 1.9 | `f_1d65377c4` / `f_1d6537e24` (the SDK's wait wrappers) from `f_1d04caf20`, `f_1d04ca4c0`, `f_1d03eb924` |
| `nvnSyncWait` (GPU fence) | 134 (11 waits, 3 of ~40 ms) | 0.45 | `f_1d08c8750 < f_1d0676ccc < f_1d0670e80` |
| a virtual core (`nxos::sched::acquire`) | ~110 | 0.37 | after the waits above |

A 70 ms strip (`tools/strip.mjs`): each ~23 ms frame is one long run (~15 ms), one sleep of 1.1-1.7 ms, then
several 0.5-1 ms kernel waits between 1-7 ms runs. `f_1d0858ab0` (generated C read in the build folder) is the
game's job-wait loop: `while (!job->done /* +0x24 */) { if (!try_run_a_job()) yield_sleep(); }` - the main thread
helps with jobs and otherwise sleeps until its job threads finish; the kernel waits are the same kind (waiting for job
groups). `f_1d0670e80`, the parent of both the sleep loop and the fence wait, is the frame loop. So the main thread's
waits are the game's own waits for its job threads, the same calls a native run makes; the job threads (`w7`, `w9`:
16% running, 80% blocked waiting for work) are not starved. None of these is a web-only wait that could be removed
without changing what the game waits for.

**Real busy time, exact** (`tools/cycles.ps1`, `QueryThreadCycleTime` of every renderer thread over 10 s from 100 s,
no profiler): the busiest thread (the game's main thread) used **80.8-82.9%** of a core in every run
(8,114-8,328 ms of 10.04 s; `TotalProcessorTime` within 5% of the cycles, so the tick accounting is not what
made W8d's 59.5%), the second 52.7-60.6%. At 50-56 FPS that is **~15-16 ms of CPU a frame**: on this machine today
the main thread is CPU-bound, and its waits (~18% of wall time, above) are the game's job waits. W8d's ~40% waiting
(one run, 59.5% busy) did not reproduce in five runs; the lever for 60 is the main thread's own code (and the
job threads it waits for), not a wait.

No FPS change was made in this wave: there was no avoidable wait to remove.

### Runs against the current build (warm profile, beach 95-135 s, alternated)

Base = the library build's own scripts (`gameplay-harness`, served on 8796); new = this branch's (8795). Same wasm.
A shared machine: CPU 44% busy from other sessions (a `cmd` job and `pkg-to-anyps5`), another session's game Chrome
ran between `n2` and `b3` (the driver waited for it).

| run | FPS 95-135 s | main thread busy (cycles) | slow submissions / GPU budget red lines |
|---|---|---|---|
| `b1` | 34.7 (52.1, 50.2, then 44.6 / 21.9 / 27.3 from 110 s) | 82.3% | 0 / 0 |
| `n1` | **56.5** | 82.9% | 0 / 0 |
| `b2` | 54.8 | 81.8% | 0 / 0 |
| `n2` | 49.8 | 80.8% | 0 / 0 |
| `b3` | 50.0 | 79.3% | 0 / 0 |
| `n3` | 39.1 (45.7, 45.6, then 33.7-37.8) | 75.9% | 0 / 0 |

Means 46.5 (base) and 48.5 (new): inside the spread, as expected for a change that only touches pipeline creation
(no new pipelines in steady beach play). Both sides have one run that sinks after ~110 s (more spiders on screen,
"334 cuts inside a pass" in `b1`'s log, and the machine's other load). Screenshots read: `b1/t140.png`,
`n2/t100.png`: Kayla on the beach, spiders, HUD, sharp, right colours; 0 WebGPU errors, no lost device in any run.

### Tests

`src/web/gpu-worker.test.js` (run with node; the worker script in a vm context with a fake device): a render pipeline
is made with `createRenderPipelineAsync`, no synchronous one, and the next submission waits until it resolves; a
compute pipeline likewise; a rejected (invalid) descriptor falls back to the synchronous call with its validation
message logged and the error scope popped; settling with nothing pending returns at once. The first two fail on the
previous worker (it called `createRenderPipeline`). 4 passed.

### Unproven / next

- The abort itself is 1 in ~9 runs; 9 runs here (3 cold-or-load, 6 FPS) had no slow submission with the new worker,
  and the cold run shows the compile now waited for before the hand-over. More runs, cold, will show it holds.
- The library web build was not relinked (JS-only change); the lead's dist rebuild and relink carry the new
  `gpu-worker.js` into the library folder.
- 60 FPS not reached. The main thread is ~80% busy (~15-16 ms CPU a frame); next is its code and the job threads it
  waits for (their per-shape costs: W8d), on a quiet machine with several runs a side.

## Safety

Chrome WebGPU on GPU 1 only (`--use-adapter-luid=0,113326`); one game Chrome at a time (the driver refused while
another session's ran and retried); screenshots >= 20 s apart (60/100/140 s); no builds or relinks were needed; my
dev servers and Chromes stopped by PID; no TDR, no device loss.
