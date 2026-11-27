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
