# MKW-PERF5 — Mario Kart World race to 60 FPS: cut the game-then-replay chain

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
parallel rules: `docs/waves/PARALLEL-RULES.md` (binding). **GPU 0.** Title: "Mario Kart World" only (scenario `mkw`).

## Brief (lead)

**Problem (recorded, `docs/waves/MKW-PERF4.md` report).** The race runs 29.1 of 60. The frame step holds each frame
for 2 vsyncs because a 1-vsync frame really costs 27.4-28.8 ms: the game's work, then the replay, in series — main
waits in `nvnSyncWait` (`aq_wait_until`, the replay publishing the fence) and the replay takes ~15 ms (pool sync /
write-watch scans 8-9 ms + descriptor sets ~7 ms). Game threads are mostly idle (guest code ~12% of each worker).
60 FPS needs the chain under ~18.3 ms. A scan-ahead thread was tried and reverted (no lead time: the game queues just
before the replay takes). Per-thread profiler: `--option profile=1`.

**Goal / done.** Race window FPS at or near 60 (frame step at hold 1 most of the window), the sheet showing a
correct race, no new red lines, no device loss. If 60 is not reached by the deadline, the best measured gain that
is a real source correction, with the numbers for what is left.

**How.** First establish which fence main waits on in `nvnSyncWait` (its own frame's, or an older one) — that decides
the route. Then cut the replay's latency on that path. Options in rough order of size: overlap the replay of frame N
with the game's work on N+1 where the fence allows; make the pool sync per submission cheaper (scan only pools the
frame's commands touch, or move scan work onto the submitting thread at submit time); translate command buffers
when the game records them on its worker threads (`CallCommands` on 0x105d) so the replay only submits — the big
one. Measure each step (perf pairs under the machine lock, rule 5). Keep `fps_hold` behaviour as it is (owner
decision pending; the lead recommends keeping it).

**This is a descriptor/memory/coherence change: the lead will run an independent audit.** Never skip draws, never
weaken robustness or the blacklist; device loss stops the work and becomes the bug.

**Exclusions:** other titles; translator changes; no commits; don't edit docs/32.

**Tests / runs:** `cargo test -p graphics --lib` (and crates you touch); `py scripts/play.py mkw --env NXGFX_GPU=0`;
read `report.md`, `frames-*.csv` and the sheets yourself.

**Deadline:** 5 hours wall clock. Report per protocol §5 in this file under "## Report".

## Report (agent, protocol §5)

**No FPS gain, and 60 cannot be reached by cutting the chain the brief describes.** That chain does not exist. Main does
not wait for its own frame's replay. The limit is the replay thread's own throughput: it does **~28 ms of serial work
per frame**, and that sets the period at hold 1 (perf4's measured 27.4-28.8 ms). I tried four cuts. Each one was
measured, gave no gain, and was reverted. The tree keeps only two opt-in diagnostics.

Final run (alone on the machine):
- Run: `build/play/20261125-101138-mkw-perf5-final`.
- Race window [250, 325]: **28.9 FPS** (5% low 25.1), against 28.7 / 24.3 in perf4's final run.
- Means in the window: frame 34.6 ms, replay 15.9 ms, prep 12.5 ms (pool sync 10.5 ms).

### Diagnosis: which fence main waits on (new chain trace, `build/play/20261125-084344-mkw-perf5-trace`, race at 270 s)
- **Each frame:**
  - Thread 0x1004 queues the whole frame right after its acquire: one submit op on queue 2, `nvnQueueFenceSync` on it, then the present.
  - At the next frame's start, main (0x1001) calls `nvnSyncWait` on **that op's fence**, i.e. the previous frame's submit.
  - Main never takes the publication path (`sw_pub` never fires): every wait is `wait_fence`.
  - At hold 2 the wait returns at once (0.001 ms).
- **So at hold 1** the frame period is the replay thread's time from taking frame N's op to finishing it, plus GPU and completion. The game's ~12 ms runs in parallel.
- **The replay thread is busy without a gap from 1.07 to 29.03 ms of a 33.3 ms frame** (28.0 ms; next frame 29.7 ms). The work:
  - **The op splits into 7 Vulkan submissions at event waits.** CPU event signals from 0x1015/0x1014 release the parts; GPU signals release others.
  - **Sub 1:** prep 2.3 ms (warm scan 1.24, pass 0.3, textures 0.74), execution 0.9 ms.
  - **Sub 2:** prep 2.8 ms. It must scan again (1.33 ms) because a CPU signal came after sub 1's scan. Execution 0.5 ms.
  - **Subs 3 and 5:** execution 5.1 and 7.3 ms.
  - **The other subs:** ~0.2 ms each.
  - **Present:** 5.8 ms. It is the frame's first sync, so it scans every pool: **4.4 ms**.
  - **Completion is delayed too.** It waits for the pool lock behind that scan for up to 4.8 ms (`wb_lock`).
- **Replay time by record** (`NXGFX_OP_TIMES=1`, `build/play/20261125-091827-mkw-perf5-optimes`, with `NXGFX_PROFILE=1` timers on):
  - **`DRAW_IDX` (op 26):** 14.8 ms a frame (2,200 × 6.7 µs).
  - **Other records:** `MESH` 1.6 ms, `DISPATCH` 1.5 ms (121 × 12.5 µs). Nothing else reaches 1 ms.
  - **The set path:** 10 ms, of which memory resolves 2.1, views 2.6, banks 1.3, writes 1.6.
- **Hold 1 needs the replay thread under ~16 ms a frame, from ~28.** No single safe item is larger than the present's 4.4 ms scan.
  - The pool sync floor is ~7 ms of write-watch walks (one walk over all pools, two over the warm ones; each walk is needed under the coverage rules) plus ~2.4 ms of per-pool passes.
  - Execution is ~15 ms, spread over ~2,200 draws.
  - Splitting the work across threads does not help:
    - Within the op, each sub waits for the event that the previous sub's end releases.
    - The present's prep can only overlap sub 7's 0.7 ms.
- **At hold 2 the replay thread is near saturation (28-33 ms of a 33.3 ms frame).** Its run-to-run swing decides how many frames are late, and so the FPS (26-29).
  - The write-watch scan alone varies 7.3-11.5 ms a frame between runs of the same tree.
  - This is why single runs of these small cuts cannot be told apart.

### Built
- **Kept (diagnostics, off by default; one `OnceLock` read per note when off):**
  - `NXGFX_CHAIN_TRACE=<start s>[:<length s>]`. New file `src/graphics/src/vk/chain_trace.rs` (included from `queue.rs` like `wait_diag`).
    - Notes in `queue.rs`: push, op, op_done, prep, vk_submit, gpu_done, cpl_done.
    - Notes in `mem.rs`: scan, wb_lock.
    - Notes in `nvn/sync.rs`: fence_sync, q_wait, sw_in/out/pub, cpu_sig.
    - Notes in `nvn/window.rs`: acq_in/out.
    - It writes them as `[vk] chain` lines once its window has passed.
  - `NXGFX_OP_TIMES=1` (`exec::op_times`, `exec::run`, a call in `queue::present_frame`): replay time by record kind, logged every 600 presents.
  - Both rows were added to `docs/env_switches.tsv`.
- **Tried, measured, reverted (no source left):**
  1. **Parallel write-watch walks** (4 threads, pieces of 64 MB).
     - A Python benchmark of the bare walk had shown 4.1 → 2.2 ms.
     - In the game it was worse: full scans 4.4-6.0 ms against 4.4, warm scans 1.6-1.9 against 1.2-1.3. Pool sync per frame 10.9 → 12.2 ms. The kernel walk serializes, and thread start costs come on top.
     - Run: `-085542-mkw-perf5-scan4`, 26.5 FPS.
  2. **The present syncing only its texture's pool**, with no scan of every pool.
     - The frame's scan of every pool then moved to the next sync that scans: the parked queue 0x13's run-on, 4.3 ms. Covered syncs fell from 7.1 to 6.0 a frame, so the total stayed the same: pool sync 11.2 ms, 28.0 FPS.
     - Run: `-091055-mkw-perf5-present`.
  3. **Skipping clean pools in the per-pool pass, and checking the cheap condition first.** Both kept the same behaviour. The per-pool pass stayed at 2.3-2.5 ms.
     - Runs: `-092601-…-skip-after`, `-095043-…-pass`.
     - The lead reports that `-092601` overlapped ROUTES-SS runs, so use its per-pool figure, not its FPS.
  4. **Keeping the replay, completion and acquire threads on the P-cores** (`SetThreadSelectedCpuSets`, EcoQoS off).
     - Without it, the replay thread already starts 97-98.5% of its work on a P-core.
     - Pair under the lock: 28.4 FPS with it (`-093443-mkw-perf5-pcores`) against 28.3 without (`-094030-…-nopcores`). Replay+prep 29.3 against 29.2 ms. No effect.
- **Probe:** `NXGFX_CHECKPOINTS=0` (the device-loss checkpoints off).
  - The first run (`-100035-…-nock`) may have overlapped ROUTES-SS, so I measured it again alone.
  - Final tree: 28.9 FPS / replay 15.89 ms on (`-101138-…-final`) against **29.7 / 15.19** off (`-101725-…-nock2`). Checkpoints cost about 0.5-0.7 ms of replay a frame.
  - 5. **Tried, measured, reverted: building the checkpoint description in one string.** It produced the same text (tested); before, it took a `format!` per key byte.
    - On: 29.1 / 15.75 (`-102504-…-ckfmt`). Same build, checkpoints off: 29.2 / 15.24 (`-103051-…-ckoff`).
    - At most ~0.15 ms of the gap, within the noise. Reverted.
    - The rest of the checkpoint cost is the ring and the `vkCmdSetCheckpointNV` call itself.
  - Turning checkpoints off is a diagnostics/robustness choice, not mine to make. I left the default on.

### Tests
- `cargo test -p graphics --lib`: 200 passed, 5 ignored, on the final tree.
- The reverted attempts had their own tests, removed with them:
  - split walks give the same pages as one call, and a failed piece fails its run;
  - pool selection for the present's sync;
  - the clean-pool skip is equivalent to `take_cpu_writes`;
  - the P-core table;
  - the checkpoint text matching the `format!` text.
- The two diagnostics have no test. They change no behaviour when off. I checked them by their output in the runs above.

### Verified
- **Before each run:** `build.ps1 -KeepCache -SkipGnu` under `build/locks/dist.lock`, then `nxrs.exe --select-title "Mario Kart World" --build`, then `py scripts/play.py mkw --env NXGFX_GPU=0 [--env …]`.
- **All runs:** 43/43 steps, normal exit, no device loss or hang. Only the known red line (pipeline-crashed skip) appeared; texture ID 337 no longer shows.
- **Final sheet** (`build/play/20261125-101138-mkw-perf5-final/sheet-1.png`): grid, then racing with HUD, coins 5 → 8 → 9, lap 1/3 → 2/3, Bullet Bill item, map, other racers. No missing or wrong surfaces.
- **Measured runs and the machine lock:**
  - The final run, the P-core pair and the checkpoint runs (`-101725`, `-102504`, `-103051`) ran under `build/locks/machine.lock` with no other game exe, cargo or nxrs-build running.
  - ROUTES-SS had finished by the time of the final and checkpoint runs; I was the only wave running.
  - The dist and MKW's build are now the final tree (the `-101138` build).
  - My wait loop checked only for game exes and builds, not `play.py` explore runs.
  - The lead reports ROUTES-SS overlap for `-092601-…-skip-after` and possibly `-095043-…-pass`/`-100035-…-nock`. Those numbers are not used for any claim above beyond "no change".

### Unproven / next step
- **60 FPS on MKW needs the replay work split across threads; no local cut gets there.**
  - The brief's "big one" fits: translating command buffers on the game's worker threads when they record (`CallCommands` on 0x105d), so the replay only submits.
  - The serial ~15 ms of draw translation is the part to move.
  - That is a multi-day architectural change: QState flows through the stream, and sets, views and resolves share the `mem`/`images` state. It needs its own design wave.
- **A cheaper, partial route: hold-2 smoothness.** The 30 FPS cap loses 1-4 FPS to late frames, because the replay thread runs 28-33 ms of a 33.3 ms frame.
  - Any steady ~3-5 ms cut would bring the race to a stable ~30.
  - Candidates by size: the frame's scan of every pool (4.4 ms; needs a cheaper way to detect writes to cold pools than walking 3.4 GB), the per-draw set path (~7.5 ms), `DISPATCH` at 12 µs each.
- **The scan cost's run-to-run swing (7.3-11.5 ms a frame on one tree) is unexplained.** It decides FPS at hold 2 more than any cut tried here.

### Slips
- My machine-lock wait loop did not look for `play.py` runs, only for game exes and builds. Some measured runs overlapped ROUTES-SS (lead's note). I re-measured the one probe that was still reproducible from env (checkpoints off). The rest belong to reverted no-gain attempts and are not used as evidence of a gain.
- The Python write-watch benchmark (3.4 GB of RAM, ~1 min) ran during my own diagnostic trace run, before its trace window.

### Questions
- None new. `fps_hold` is unchanged, as the brief asks. The lead's recommendation (keep the steady 30) still stands: at hold 1 the race would run at ~35 FPS with ~28 ms frames.
