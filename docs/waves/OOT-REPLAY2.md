# OOT-REPLAY2 — Ocarina of Time: the replay thread under one vsync (target 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-28, worktree `D:\Projects\RecompLauncherEngine-ootreplay`,
branch `ootreplay` (base `gameplay-harness` `5c503d2`: OOT-WAIT merged, OOT-LOCK's code not merged). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 0. Deadline 5 h.

## Plan (written first)

Starting point: OOT-LOCK's baseline `D:\Projects\RecompLauncherEngine\build\play\20261128-024046-lock-base` (`oot`,
[845, 915] s): 50.0 FPS / 5 % low 39.0; the replay thread busy ~18-19.5 ms a frame against ~15.8 for one vsync; every
wait chain ends in it. Its host profile (`vk replay`, 55,493 samples over 850-890 s, ~2,000 frames: 1 % of samples
~0.2 ms a frame), split by callee before this plan:

| stage (replay thread) | ms a frame |
|---|---|
| pool sync `begin_submit_ex` | 7.5 |
| - `prehash` (page hashes on the 13 hash threads, memory-bound: the replay thread hashes its share for the whole job) | 1.64 |
| - per-pool pass (`upload_changed` 1.25: bookkeeping ~0.6, uploads 0.23, copy + hash of uploaded pages 0.3) | 1.58 |
| - `land_first` (waiting for the completion thread's landings) | 1.42 |
| - `watch::take` (GetWriteWatch with reset; ~14,000 rewritten pages a frame, two scanning syncs a frame) | 1.30 |
| translation `record` | 5.4 |
| - of it the replay thread's own share of parallel segments (`par::work`) | 4.1 |
| - streams in one piece (`exec::run`) | 1.06 |
| waiting at lane joins (`par::run`) | 1.6 |
| pre-pass and segment set-up (`run_segmented` without `par::run`) | 1.4 |
| `submit_sub` / presents | 1.1 / 0.7 |

The log's translate lines: 3 streams a frame in segments (16.9 segments, largest 115 draws), 7 in one piece; the lanes
translate 21 ms a frame together and are ~80 % idle; segments' wall 5.7 ms a frame. With ~5-6 segments a parallel run
for 6 lanes plus the replay thread, a run lasts as long as its largest segment and the replay thread translates a
seventh or more of every run itself.

0. **Measurement.** The `oot` walk varies run to run (1,072-1,522 draws in [845, 915] over OOT-LOCK's runs). Its
   standing stretch [660, 820] s (h2-h7: Link standing in his house, A every 1.5 s) holds 2,007-2,093 draws in every
   one of them and its frame time spreads 20.7-21.6 ms (`lock-base` 46.7 FPS, frame 21.43 ms, replay + prep 18.72).
   A scenario `oot-still` in `scripts/scenarios.toml`: `oot`'s steps and checks unchanged, its window that standing
   stretch, so the report gives FPS over the fixed view; the [845, 915] walk numbers come from the same run with the
   harness's own metrics over that window. The `oot` scenario and its route checks are untouched.
1. **Baseline** with diagnostics: this worktree's dist (`5c503d2`), OoT relinked from it with a PDB, `oot-still` on GPU 0
   with `NXRS_WAIT_ACCT=1` and the host profiler on `vk replay` with stacks over 850-890 s.
2. **(c) Overlap first, by the profile: translation's parallel runs.** Cut a stream into enough segments that every
   lane has work and the replay thread's own share and its join wait shrink (segment size, cuts inside passes, lane
   count; costs: whole sets at each segment start, gate give-ups translated again at the join). If the profile says a
   default moves it, the default changes in source (per title through its profile if other titles would change).
3. **(b) Page hashing and the write watch:** check whether the ~57 MB a frame of rewritten pages are the same pages
   every frame and whether their hashing can be finished off the replay thread before the sync needs it (OOT-FPS2's
   walker and scan-time hashing failed because the pages are written again after the walk: not redone).
4. **(a) Translation's per-draw cost** past SET-FAST's keys and copies: `tex_view` (lazy image making and its lock,
   0.5-0.9 ms), the bank and patch work, if the time allows.

Each change behind a switch (rows in `docs/env_switches.tsv`); a change that moves nothing visible (FPS, frame time in
the fixed window, its stage's ms) is removed. Excluded: Fault A (white house floor), skipped draws or hidden work,
faked completions, weaker checks; OOT-FIX's colours stay; live native NDVE off.

Expected files: `scripts/scenarios.toml`, `scripts/checkpoints.json` (the new scenario's references, written by
`play.py checkpoint`), `src/graphics/src/vk/exec.rs` (segmenting, `par`), maybe `queue.rs`, `mem.rs`, `upskip.rs`,
`image.rs`, `src/profiles` (a per-title default), `docs/env_switches.tsv`, this document.

Verification: one normal run after the work against the baseline and its diagnostics (no switch-off controls), every
sheet read, report.md's anomaly table and route checks (the `oot` checks too); one final `oot-house --faults` run
(Fault A count as an observation). `cargo test --release -p <crate>` for each changed crate with tests for new
behaviour; `py -m unittest scripts.test_play`. Numbers: FPS mean / 5 % low, frame ms in the fixed window, replay busy
and each targeted stage's ms, the game frame's median vsyncs (pacer line), peak memory, against the baseline and 59.

What would disprove the plan: segments that are smaller or more numerous leave the replay thread's busy ms where they
were (the per-segment costs eat the overlap), or the replay thread's frame is not in these stages in this wave's
baseline profile.

## Baseline (`build/play/20261128-044556-r2-base`, `5c503d2` + the scenario, OoT relinked with a PDB)

`oot-still` on GPU 0, `NXRS_WAIT_ACCT=1`, host profile of `vk replay` with stacks over 850-890 s. Completed, on route
(the `oot` references: 0.014-0.115 at x5 / h1 / h4 / h7 / walk1 / last; this run then became `oot-still`'s
references), every sheet read (OOT-FIX's colours, Link in his house at every check shot, the walk).

| window | FPS / 5 % low | frame ms | draws | replay + prep (p50) | replay busy / idle (pipelining line) | pacer median (fit 1) | peak |
|---|---|---|---|---|---|---|---|
| fixed [660, 820] | **48.1 / 44.0** | 20.80 | 2,024 | 18.22 (18.34) | 20.05 / - | 1.24 (5 %) | 28,469 MB |
| walk [845, 915] | **44.9 / 30.0** | 22.01 | 1,500 | 17.41 (17.33) | 19.85 / - | 1.28 (4 %) | |

Pool lines (fixed window): landings first 2.08 ms, lock wait 1.00, write-watch scan 2.28, per-pool pass 3.17, 57.5 MB
a frame of rewritten pages left out. Translate lines: lanes 24.7 ms a frame together, the replay thread waited 1.46
ms for them, 22 segments a frame (largest 174 draws), pre-pass 1.28 ms, segments' wall 6.66 ms.

Host profile (1,761 frames, ms a frame): pool sync 8.04 (`prehash` 1.66, `upload_changed` 1.35, `watch::take` 1.29,
`land_first` 1.57); translation `record` 6.05, of it the replay thread's own parallel segments (`par::work`) 4.85;
`par::run` 2.28 (with its join waits); pre-pass and set-up `run_segmented` 1.68; `submit_sub` 1.06; `tex_view` 1.39.

## Changes

### A. Frame threads on the most performant cores (`NXGFX_FAST_CORES`): built, measured, taken out (`d529ab4`)

This machine's CPU (Core Ultra 9 285K) has 8 performance cores (efficiency class 1: logical processors 0, 1, 10-13,
22, 23) and 16 efficient ones (class 0), and the OS is Windows 10, which places threads without regard to the class.
Nothing in the engine chose a core class: the replay thread (the frame's bound), its translation lanes and the
completion thread (whose landings the replay thread waits for) could each run on an efficient core. With the switch
on, `rtclass` reads the CPU sets (`GetSystemCpuSetInformation`) and, on a CPU with more than one class, gives those
threads the top class's sets (`SetThreadSelectedCpuSets`): the replay thread and the game's acquire thread when they
join the frame-thread class, each `vk translate` lane and `nvn-completion` when they start. Each frame thread samples
its core once a frame (`GetCurrentProcessorNumber`) and logs the share on the top class every 1,800 frames.

`build/play/20261128-050441-r2-v1` (`d529ab4`, same diagnostics as the baseline): the replay thread and the acquire
thread on a performance core in 100 % of their frames (44 lines), and nothing moved: fixed window 47.8 / 43.0, frame
20.91 ms (baseline 20.80), replay + prep 18.36 (18.22), replay busy 20.29 ms (20.05), lanes 22.97 ms a frame, the
replay thread's wait for them 1.93 ms (1.46); walk 43.0 / 29.5. The replay thread's frame is not bound by the speed
of the core it runs on (most of it waits on memory, locks and other threads). Taken out by the removal rule.

What the replay thread waits on, from the baseline profile (1 % of samples = 0.227 ms a frame): `land_first` 1.57 ms,
the lane joins 1.53, the pool lock at the sync 0.68, the lazy images' make lock in `materialize` 0.50, the images'
lock in `tex_view` 0.42: ~4.7 ms a frame of its ~20 ms busy is waiting on other threads.

### Lanes 6 -> 12: a diagnostic run, not adopted

`build/play/20261128-052114-r2-lanes12` (the `d529ab4` build with `NXGFX_FAST_CORES=0`, i.e. the base code, and
`NXGFX_TRANSLATE_THREADS=12`): 34 segments a frame (largest 87 draws), segments' wall 6.66 -> 5.89 ms, replay 9.42 ->
8.76 ms, but the lanes' CPU 24.7 -> 37.8 ms a frame, the join wait 1.46 -> 2.30 and the landings-first wait 2.08 ->
2.36: replay busy 20.05 -> 19.75, frame 20.80 -> 20.64 ms (inside the runs' spread), fixed window 48.5 / 45.0. Its
`17-h4` shot caught a Fault A frame (floor and shelves blown white with streaks; Link, the door and Navi right), which
the route check calls off route. Not adopted: half a lane's worth of the frame for 13 ms of CPU a frame.

### B. The pre-pass before the pool sync (`NXGFX_PLAN_AHEAD`): built, measured, taken out (`22e9b41`)

A stream translated in segments had its pre-pass (`plan`, ~1.3 ms a frame) made right after its submission's pool
sync, whose first step (`land_first`) waits up to 1.5 ms for the completion thread's landing. Made before the sync
instead, the landing would go on during the pre-pass and leave a shorter wait. `build/play/20261128-053900-r2-v2`: on
route, sheets read; only 1.0 stream a frame took that path (most segmented streams run on from the parked-queue chain,
which has no sync of its own), and the landings-first wait did not shrink (2.08 -> 2.18 ms). Fixed window 48.5 / 45.0,
frame 20.63 ms (20.80), replay busy 19.81 (20.05): inside the spread. Taken out.

### The landings-first wait off: a diagnostic run, not a change

`build/play/20261128-055429-r2-lf0` (base code; `NXGFX_LAND_FIRST=0`): the 2.08 ms a frame the replay thread waited
in `land_first` moved into its wait for the pool lock (1.00 -> 2.28 ms), and the game's fence wait grew (6.5 -> 7.4
ms a frame); replay busy 19.60, frame 20.75 ms, fixed window 48.2 / 45.0. The replay thread waits for the completion
thread's landings under the pool lock (~3 ms a frame of `apply_writebacks`, OOT-LOCK's completion profile: `land`
2.9 ms with `merge_changed_bytes` 1.9) whichever way it waits; neither switch setting shortens that.

### C. The upload skip's pages decided in one locked pass (`NXGFX_UPSKIP_ONE_PASS`): built, measured, taken out (`247c362`)

`upskip::upload_changed` takes its table's lock three times per 1 MB batch and walks the batch's candidates in each;
for a page `prehash` already hashed (nearly all of OoT's ~14,000 rewritten pages a frame) no guest read is needed
between the steps, so step 1's pass can decide it. Test `one_pass_decides_as_step_three` (a mixed batch decided the
same with the pass on and off). `build/play/20261128-061506-r2-v3`: on route, sheets read; `upload_changed` 1.35 ->
1.20 ms a frame in the profile, but the per-pool pass 3.17 -> 3.19 ms, replay busy 20.71, frame 21.27 ms (fixed
window 47.0 / 43.0): the stage did not move. Taken out.

### D. The sync's page hashing with the pool lock let go (`NXGFX_SYNC_HASH_UNLOCKED`, default on; `mem.rs`) - kept

The replay thread and the completion thread wait for each other at the pool lock: the replay thread ~2-3 ms a frame
for landings (landings first, then the lock), the completion thread 3.7-4.3 ms a frame ("completion: pool lock") for
syncs. Of a scanning sync's time under the lock, `prehash` (~1.7 ms a frame) is hashing on the hash threads that reads
guest memory only. The sync now gathers the pages to hash under the lock, lets the pool lock and the watch lock go
while they are hashed (256 pages or more), and takes both again for its per-pool pass (CPU flushes queued meanwhile
and the walker's pages drained, as at a sync's start; the re-take counts in the sync's lock wait). What the scan took
is in the sync's own `taken` map; a landing in between leaves its pages pending and re-arms hashes that the per-pool
pass reads under the lock, and a write-back that has landed meanwhile no longer holds pieces back. A skip still needs
the prehash to equal the kept hash, which then describes the GPU copy whichever came first. The pools line counts the
syncs that did it (1.0 a frame in OoT). No unit test: the path needs a device (`prehash`'s own tests stand).

`build/play/20261128-063536-r2-v4`: the frame times were wrecked by another wave's build on the machine (its target
and build folders written at 06:43-06:48 and 06:50-06:52, over this run's whole window: 300-950 ms frames with the
replay thread at ~14 ms and the game's threads 25-34 % runnable without running); its FPS is not used. Its sheets are
correct. Run again as `build/play/20261128-065234-r2-v4b` (the other wave wrote at 06:56-06:58 and 07:06-07:07, i.e.
before the fixed window and over the walk window's start):

| | baseline `r2-base` | `r2-v4b` |
|---|---|---|
| fixed window FPS / 5 % low | 48.1 / 44.0 | 48.9 / 44.0 |
| frame ms (draws) | 20.80 (2,024) | 20.47 (2,089) |
| replay busy (pipelining line) | 20.05 | 19.90 |
| landings first / sync lock wait | 2.08 / 1.00 | 0.97 / 1.04 |
| completion thread's wait for the pool lock | 3.7-4.3 | 1.5-1.8 |
| the game's fence wait (`the game waited`) / room in the replay queue | 6.50 / 1.56 | 3.96 / 3.65 |
| lanes' wait for the pool lock at resolves | 0.4-0.7 | 1.35 |
| replay thread's wait for the lanes | 1.46 | 2.04 |

Profile (850-890 s, ms a frame): pool sync 8.04 -> 7.09, `land_first` 1.57 -> 0.76; translation `record` 6.05 -> 6.47,
`par::run` 2.28 -> 3.05. So the stage it targeted moved (the sync's landing waits down ~1 ms, the completion thread's
lock wait halved, the game's fence wait down 2.5 ms a frame), but the landings now run during translation and the
lanes wait for them there: the replay thread's busy time and the frame moved within the runs' spread. This is the
pattern OOT-LOCK's change A showed (fence wait down, waits for queue room up); the lead left that one out.

## Final runs (`cfc016f`: base + D)

`build/play/20261128-070833-r2-after` (`oot-still`, GPU 0, `NXRS_WAIT_ACCT=1`, no profiler; the other wave wrote at
07:10-07:16 during the opening and a little at 07:21-07:23, the fixed window being 07:19:33-07:22:13): completed,
normal exit, on route (the `oot-still` references and the `oot` ones: 0.005-0.117), every sheet read (OOT-FIX's
colours, Link in his house at every check shot; the opening's shots differ in timing, the walk's camera stopped by
the bed's corner for walk5-9, as walks vary); anomaly kinds as the baseline's, one red line fewer.

| | baseline `r2-base` | after `r2-after` | target |
|---|---|---|---|
| fixed [660, 820] FPS / 5 % low | 48.1 / 44.0 | **49.6 / 47.0** | 59 |
| frame ms in the fixed window (draws) | 20.80 (2,024) | **20.14** (2,010) | <= 16.9 |
| walk [845, 915] FPS / 5 % low (draws) | 44.9 / 30.0 (1,500) | 45.0 / 30.0 (1,681) | 59 |
| replay busy (fixed / walk) | 20.05 / 19.85 | 19.66 / 20.29 | <= 15.8 |
| replay + prep, frames CSV (fixed) | 18.22 | 17.81 | |
| landings first / sync lock wait / per-pool pass | 2.08 / 1.00 / 3.17 | 0.94 / 0.98 / 3.24 | |
| completion thread's wait for the pool lock | 3.7-4.3 | 1.0-1.9 | |
| game's fence wait / room in the replay queue | 6.50 / 1.56 | 3.51 / 3.77 | |
| lanes' pool-lock wait at resolves / replay's wait for lanes | 0.4-0.7 / 1.46 | 1.1-1.4 / 2.08 | |
| pacer: the game frame's median vsyncs (fitting one) | 1.24 (5 %) | 1.19-1.20 (4-7 %) | 1.0 |
| peak memory | 28,469 MB | 29,654 MB | < 32,768 |

The base code's fixed-window frame over this wave's runs: 20.80 (base), 20.91 (A, nothing moved), 20.63 (B), 20.75
(land-first off), 21.27 (C); with D: 20.47 (`r2-v4b`) and 20.14. A ~0.3-0.7 ms shorter frame, at the edge of the
runs' spread; the FPS gain is 1.5 in the fixed window, none in the walk.

### `oot-house --faults` (`cfc016f`)

`build/play/20261128-072415-r2-house` (`NXGFX_FAULT_MAX=200`, `NXGFX_FAULT_PER_REASON=80`): completed, normal exit,
house window [610, 690] **47.6 / 44.0 / min 39.0** (OOT-LOCK's `lock-house` 46.9 / 43.0), peak 28,923 MB, on route
(0.019-0.042 at d01 / d10 / d20 / d30 / last). Captures: 101 `no-target` (the known empty pixel shader), 2 black-frame
captures, the opening's two white-light frames (145 s), and **33 one-frame white flashes at 385-689 s**:
`fault-136-p33124.png` is Fault A (floor, shelves and grass blown white with lightning streaks; Link, the door, the
lanterns and Navi right). The harness shot `15-d12` (659 s) caught the displaced-props form (the hanging herbs and
lights drawn out of place, OOT-REPLAY's description). Observation only, but more than before: OOT-LOCK's house run had
21 flash captures past the opening by the same count (its doc: 16 at 422-677 s), OOT-WAIT's 10, OOT-FPS2's 14. D
changes when landings run relative to the replay's syncs; whether that raises Fault A's rate is not shown (one run).

## What is left to 59 FPS

The replay thread is still ~19.7-20.3 ms busy a frame. What this wave measured: it is not bound by its core's speed
(A); ~4.7 ms of it is waiting on other threads (the completion thread's landings under the pool lock, the lanes, the
lazy images' and images' locks), and moving where the landings run moves those waits between stages (land-first off,
D) rather than removing them: the landings' own time under the pool lock (`apply_writebacks`, ~3 ms a frame, of it
`merge_changed_bytes` ~1.9) is what the replay thread and its lanes wait for wherever it falls. A landing that does not
need the whole pool table (its watch, its pool's pieces) would let syncs and resolves run beside it. The rest:
translation ~6 ms (a sixth of every parallel run, the join and the pre-pass ~1.3 ms serial), hashing ~57 MB a frame
(~1.7 ms, memory-bound), the write watch ~1.3 ms.

## Report (agent, protocol §5)

Branch `ootreplay` (base `5c503d2`): `2f2f960` (plan, `oot-still`), `26e5b38` (its references), `d529ab4` (A), `65c5b5a`
(A out), `22e9b41` (B), `697d21d` (B out), `247c362` (C), `3e1a7dd` (C out), `cfc016f` (D), docs. The final code is
`cfc016f`'s: `src/graphics/src/vk/mem.rs` (D), `docs/env_switches.tsv` (one row), `scripts/scenarios.toml` +
`scripts/checkpoints.json` (`oot-still`).

### 1. Built

- Step 0: `oot-still`, the `oot` scenario's steps and checks with its window on the standing stretch [660, 820] s
  (2,007-2,093 draws in every run; frame times within ~0.5 ms). `oot` is unchanged.
- Diagnosis (profile + lines): pool sync 8.0 ms, translation 6.0, joins 2.3, pre-pass 1.7; ~4.7 ms of waiting.
- D (kept, `NXGFX_SYNC_HASH_UNLOCKED`): a sync lets the pool and watch locks go while its pages are hashed. Cuts the
  landing waits (replay 2.08 -> 0.94 ms, completion 3.7-4.3 -> 1.0-1.9, game fence 6.5 -> 3.5 ms a frame).
- A (fast cores), B (pre-pass ahead), C (one-pass upload skip): built, measured, moved nothing, taken out. Lanes 12 and
  land-first off: diagnostic runs, not adopted.

### 2. Tests

- `cargo test --release -p graphics`: lib 325 passed, 6 ignored; other targets pass. `py -m unittest
  scripts.test_play`: 12 OK (the new scenario parses, its checks name its shots).
- D has no unit test (the path needs a device; `prehash`'s tests stand). A's and C's tests went out with them (A:
  CPU-set parsing and placement; C: one-pass decisions equal to step 3's).

### 3. Verified

Baseline `20261128-044556-r2-base` 48.1 / 44.0 fixed, 44.9 / 30.0 walk; after `20261128-070833-r2-after` 49.6 / 47.0
fixed, 45.0 / 30.0 walk; table above. All runs on route, every sheet read. Target 59 not reached. Machine: another
wave (`RecompLauncherEngine-3DS-W2`) built at times during my runs; `r2-v4`'s FPS is void for it.

### 4. Unproven

- D's frame gain (~0.3-0.7 ms) is two runs at the edge of the spread; a quiet-machine A/B is the lead's.
- Fault A: 33 flashes in the house run against 10-21 before; whether D raises the rate needs more house runs.
- Peak memory 29.7 GB in the after run (28.5 GB baseline): not traced.

### 5. Slips

- None of §4's editing rules broken: every edit and every take-out made with Edit/Write (rtclass.rs written back whole
  from its earlier content with Write). Diagnostic runs `r2-lanes12` and `r2-lf0` set switches on the base code to
  test a hypothesis (not change controls). `r2-v4`'s background shell ended early; the run itself completed.

### 6. Questions

- Merge D? Its targeted waits moved clearly and the frame by ~0.3-0.7 ms, but the waits partly moved into translation
  (OOT-LOCK A's pattern, which the lead left out) and the house run's Fault A count rose. Recommendation: merge only
  with a lead quiet-machine `oot-still` A/B and a second `oot-house --faults` run; otherwise leave it on the branch.
