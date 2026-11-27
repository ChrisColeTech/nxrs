# MKW-SYNC — the replay thread's per-frame prep, and a pacer that holds 1 vsync on a mostly-fitting window

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~04:54, worktree
`D:\Projects\RecompLauncherEngine-mkwsync`, branch `mkwsync` (base `gameplay-harness` `0765845`: every MKW and OoT wave
of the day merged). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0 (OOT-COMBO owns OoT
on GPU 1). Deadline 4 h (08:54).

## Starting point (MKW-SETS)

At hold 1 MKW runs 53-56 FPS; the frame op ~12 ms median, replay + prep p50 13.4-14.0 / p95 17.4-18.1 ms, 74-87 % of
frames ≤ 15.8 ms. The default pacer (`src/os/src/fstep.rs`) tries 1 vsync only after 30 frames in a row fit 15.8 ms and
steps back up on three late frames when the window's rate is short, so it averages ~45 FPS with a 5 % low of ~29.

## Plan (written before code)

1. **Baseline** (default pacer, the build as merged, after the owner's session moved to the 60 Hz console display):
   FPS / 5 % low, replay + prep p50 / p95 and share ≤ 15.8 ms from the frames CSV over [250, 325] s, the `pools` /
   `pipelining` lines, the frame step's summaries (held 1 / 2, failed trials). A hold-1 host profile of the replay
   thread (`NXRS_HOST_PROF` with stacks) says where its p95 frames go.
2. **Replay thread −2 ms at p95**: USE-SYNC's use-driven sync if the profile still shows the first part's pool sync at
   ~1.4 ms; then whatever the profile names next (prep, image walk, present). Each behind a switch (`0` = old path).
3. **Pacer** (`NXOS_FSTEP_SHARE`): a share rule instead of "30 in a row" — try the shorter hold when ≥ N % of a window
   of frames fits it, and leave it only when fewer than N % of the window are on time; frames over budget are shown late
   and paid back with the existing step debt, so game time stays real time (the summaries' `of real time` figure).
4. **Verify**: `mkw`, `mkw-turn` on GPU 0, route checks, every sheet; `cargo test --release -p graphics` and `-p os`.

Expected files: `src/os/src/fstep.rs`, `src/graphics/src/vk/mem.rs` / `exec.rs` / `queue.rs` (by the profile),
`docs/env_switches.tsv`, this doc. What would disprove step 2: the profile's p95 frames are not prep-bound (e.g. lane
waits or the driver); for step 3: game time drifting from real time, or the 5 % low not moving.

## Display

The present mode is MAILBOX (`forced_vsync=0`); the frame step and `pace_period` pace on a fixed 1/60 s
(`nvn::window::VSYNC_NS`), not on the display's refresh: nothing in the pacer or the present path measures or assumes
the display's rate, so a 32 Hz Remote Desktop display does not change the pacer's arithmetic (the compositor may still
show fewer frames). The owner's session moved to the console at ~05:01 (`query session`: session 1 `console`, Active;
`Win32_VideoController`: RTX 5090 at 60 Hz, 3840 wide). Every run below was started after the move, on the 60 Hz
display; the window is now 2637x1483 (swapchain line), where earlier waves' runs had ~960x540.
`20261127-050054-ms-prof-hold1` (started 05:00:54, before the move) died at 42 s without a log line when the session
moved (exit -1) and is not used.

Machine during the runs: OOT-COMBO's OoT runs (GPU 1) overlapped every run of this wave (theirs started 05:01,
05:14, 05:26, 05:43; OoT at 0.8 core at 05:10, **3.7 cores at 05:53**, with an OoT relink's `clang`s at 05:44 and
05:53); a Chrome process used ~2.9 cores from 04:52 (measured 05:10); a web wave's nightly wasm build ran ~05:31-05:36
(during `v2`). On this CPU (8 P-cores, 16 E-cores) that moves MKW's absolute times by about a third from run to run:
the lanes' and the replay thread's translation time, which this wave does not touch, went 12.0 → 17.8 ms a frame
between two runs of the same build (`v2`, `final`). The within-run ratios in §3 are the comparable figures.

## 1. Baseline (`20261127-050202-ms-base`)

Build as merged (`0765845`), default pacer, the replay-thread host profile on (`NXRS_HOST_PROF=265,40`, stacks, MKW DLL
linked with `/DEBUG`), window [250, 325] s, frames CSV (replay + prep = `replay_ms` + `prep_ms` per presented frame).

| | baseline |
|---|---|
| FPS / 5 % low (report) | **28.7 / 16.0** |
| replay + prep p50 / p95 | **18.42 / 27.83** ms |
| frames with replay + prep ≤ 15.8 ms | 18 % |
| prep / pool sync mean | 5.76 / 4.35 ms |
| lanes' descriptor CPU mean | 20.66 ms |
| frame step (last summaries) | held 1 / 2: 191 / 1,609 and 128 / 1,672; 18-25 failed trials a summary; game time 0.98-1.0 of real |
| hold-1 frames (under 25 ms) | 201 of 2,159; frame p50 22.0 ms, 13 % ≤ 18.3 ms |
| route | 3/3 on |

These are well below MKW-SETS's figures for the same build (replay + prep p50 13.4-14.0 at hold 1): the profiler,
the larger window and the machine's load (above) all differ; this wave compares only with its own baseline.

The replay thread's profile (29,769 busy samples of 51,276; waits on condition variables left out, lock waits kept):

| where | share of busy samples |
|---|---|
| `mem::begin_submit` (pool syncs) | **22.0** (≈ 4.2 ms a frame) |
| — the per-pool pass and its uploads (`upload_changed` copy + hash 3.8) | 5.3 |
| — `watch::take` (GetWriteWatch) | 3.7 |
| — pool-lock wait | 2.5-3.1 |
| — walks over all ~4,600 pools (lists, prehash, pass, busy set) | ~5 |
| `image_begin_submit`, of it the span list made again (`image.rs` 4043-4050) | **4.9**, ~4 (≈ 1.2 ms) |
| copy of a parking stream's rest (`stream[off..].to_vec()`, `queue.rs` 2310) and its free | **~2.8 + 1** |
| translation on the replay thread (`emit_op`, `par::work`), driver (`nvoglv64`) | 35, 10.6 |
| pre-pass (`plan`) | 4.9 |

USE-SYNC's use-driven half was not the cheapest lever: the first part's scan is ~0.7 ms, and the profile showed ~2.5 ms
a frame of bookkeeping around it (walks, the span list, the stream copy) that a narrower change removes.

## 2. Built (each behind a switch, default on, `0` = the old path; rows in `docs/env_switches.tsv`)

### Image span list kept by edits (`NXGFX_IMG_SPAN_EDITS`, `image.rs`)

MKW-SETS's span list (every CPU-addressed image's storage, by start, walked beside a sync's sorted dirty ranges) was
made again from `by_addr` and `by_serial` whenever `addr_gen` moved, and MKW makes and releases images every frame, so
it was remade at nearly every scanning sync (~1.2 ms a frame). Now the three places that change `by_addr` (an image
made, taken over, released) record the change (`Images::span_edit`), and the next use applies them in order
(`spans_apply`: removals by start and serial, additions merged in by start); the list is made again only when it was
never made or the changes outnumber half of it. A removal that finds nothing leaves a stale entry, which only makes the
check look at an image `by_serial` no longer has; an image is never left out.

### Lean scanning sync (`NXGFX_SYNC_LEAN`, `mem.rs`)

A scanning `begin_submit_ex` walked all ~4,600 pools about seven times: the cold set, every watched pool, the scan
list, the tracked pools, the prehash's pending pages, the per-pool pass and the busy set made again. Now one walk
(`scan_lists`) makes the four lists, the prehash takes the busy pools' pending pages (as a covered sync's does; a page it
misses is only hashed inline), and the per-pool pass visits the busy set plus the pools the scan found written or could
not read, as a covered sync already did. Every other pool returned at once in the old pass (asked and found unwritten,
sealed, or a cold one left out, with nothing pending), and the busy set is kept by every place that gives a pool work
(pool creation of an unwatched mirror, `sync_range`, landings, the walker's drain).

### A parking stream keeps its buffer (`NXGFX_PARK_IN_PLACE`, `queue.rs`)

When a stream parked at a wait, its rest was copied into a new buffer (`stream[off..].to_vec()`, three places): up to
~1 MB a few times a frame, a fresh allocation whose pages faulted in, and the old buffer freed (~2.8 % + ~1 % of the
replay thread). `park_rest` drains the head off the stream's own buffer instead.

### Pacer share rule (`NXOS_FSTEP_SHARE`, default 75; `src/os/src/fstep.rs`)

`Fstep` keeps, for the hold in force, a 60-frame window (two bitmasks): which frames fit `k − 1` (cost ≤ 0.95 vsync ×
(k − 1)) and which were on time (not late: shown and cost ≤ k + 0.1 vsync). In share mode a shorter hold is tried when
the window is full and ≥ N % of it fits (instead of 30 frames in a row), after the same hold-off; the hold is left for a
longer one only when, besides three late frames in a row and the rate short of `k` (the old rule), fewer than N % of the
window's frames were on time. Late frames under the kept hold are shown late, and the step pays the debt back one vsync
a frame as before, so game time stays real time (`DEBT_MAX` drops only stalls past 0.25 s, as before). The window
starts again at every change of hold. `NXOS_FSTEP_SHARE=0`: 30 in a row, as before. The frame step logs the rule at
boot.

### Built, measured, taken out: landing yield (`c1a2920`, reverted in `bc2dba8`)

A sync that found the pool lock taken marked itself waiting, and a landing of several write-backs released the pool and
watch locks between two of them until the sync had them. It stepped aside ~1.3 times a frame, but the replay thread's
pool-lock wait did not move (0.73-1.04 ms a frame in `v2`'s summaries, 0.62-1.04 in `v3`'s): most of the wait is not
behind landing batches. What holds the lock then is not established (the completion thread holds it ~2.6 ms a frame
within single write-backs; the walker's sealing pass ~0.3 ms).

## 3. Measured

All default pacer, `mkw`, window [250, 325] s, GPU 0, 60 Hz console display. `base`, `v1`, `v2`, `v3` with the
replay-thread profiler on (as the baseline); `final` without it.

| Run (`build/play/`) | build | FPS / 5 % low | replay + prep p50 / p95 | ≤ 15.8 | prep / pool sync mean | translation (`replay_ms`) mean | lanes mean | route |
|---|---|---|---|---|---|---|---|---|
| `20261127-050202-ms-base` | `0765845` | 28.7 / 16.0 | 18.42 / 27.83 | 18 % | 5.76 / 4.35 | 14.10 | 20.66 | 3/3 on |
| `20261127-051815-ms-v1` | spans, lean sync, share rule (`151ea00`) | 31.2 / 28.7 | 17.07 / 23.04 | 30 % | 3.93 / 3.72 | 13.86 | 20.29 | 3/3 on |
| `20261127-052803-ms-v2` | + park in place (`6628fb3` = final code) | **33.3 / 29.4** | **15.02 / 19.62** | **64 %** | 3.43 / 3.32 | 12.04 | 17.60 | 3/3 on |
| `20261127-053841-ms-v3` | + landing yield (taken out) | 28.9 / 23.0 | 21.67 / 32.01 | 2 % | 5.02 / 4.52 | 17.89 | 22.74 | 3/3 on |
| `20261127-054716-ms-final` | final (`bc2dba8`), no profiler | 29.6 / 26.0 | 21.92 / 28.27 | 1 % | 4.90 / 4.69 | 17.81 | 23.35 | 3/3 on |

`v2` and `final` are the same code: their translation and lanes, which no change here touches, differ by ~48 % and ~33 %
(the machine, §Display). Within each run the replay thread's own parts scale together, so their ratio to translation is
the comparable figure:

| Run | prep ÷ translation | pool sync ÷ translation | lanes ÷ translation |
|---|---|---|---|
| base | **0.409** | **0.309** | 1.47 |
| v1 | 0.283 | 0.268 | 1.46 |
| v2 | 0.285 | 0.276 | 1.46 |
| v3 | 0.280 | 0.252 | 1.27 |
| final | **0.275** | **0.264** | 1.31 |

Prep fell from 41 % to 28 % of the translation time in every run of the new build: at the baseline's translation speed
(14.1 ms) that is **5.8 → 3.9 ms of prep a frame (−1.9 ms)**, of it the pool sync −0.6 ms and the image walk −1.3 ms.
The parked-stream copy is in neither column: in the profile, `run_stream_due`'s memcpy and free went from ~3.8 % of the
replay thread's busy samples (1,392 samples in `v1`) to ~0.2 % (58 in `v2`), ≈ −0.5 ms a frame. Together ≈ **−2.4 ms a
frame on the replay thread** at the baseline's speed, on every frame (the walks and copies do not depend on the scene).
At p95 the like-for-like pair (`base` → `v2`, both profiled, `v2` on a lighter machine) gives 27.8 → 19.6 ms; how much
of that −8.2 ms is the machine is not separable here, and the protocol rules out a switch-off control.

### The pacer

| Run | frame step summaries in the race (held 1 / 2 of 1,800; failed trials; game time ÷ real time) | hold-1 frames: p50, ≤ 18.3 ms |
|---|---|---|
| base (30 in a row) | 191 / 1,609, 128 / 1,672; 18, 25 failed (cumulative); 0.91, 0.98 | 22.0 ms, 13 % |
| v1 (share 75) | 179 / 1,621, 195 / 1,605; 19, 26; 1.00, 1.00 | 21.2 ms, 15 % |
| v2 (share 75) | 1,105 / 695, 631 / 1,169; 13, 19; 1.00, 1.00 | **18.0 ms, 53 %** |
| final (share 75) | 77 / 1,723, 85 / 1,715; 19, 26; 0.98, 1.00 | 23.6 ms, 0 % |

On this machine today the frames at hold 1 cost 18-24 ms (median), so at most about half of them are on time and the
share rule, like the old one, falls back to two vsyncs; it holds 1 vsync for longer stretches when the machine allows
(`v2`: 1,105 of 1,800 frames in one summary). With the new build game time stayed real time in the race's summaries
(0.98-1.00; earlier summaries of 0.91-0.96 hold the loads, where stalls past 0.25 s are dropped by design); the
baseline's were 0.91 and 0.98, with ~2,600 vsyncs dropped by then against ~600-750. What it looks like: the sheets are stills, so they show only that the
picture is right in both holds; motion at a kept hold with late frames is the uneven 1-or-2-vsync cadence of a
`NXOS_FPS_HOLD=off` run, with the late frames' time paid back so the race does not slow down.

### `mkw-turn` (`20261127-055341-ms-final-turn`, final build, window [250, 290] s)

29.1 / 20.0 FPS (5 % low), replay + prep p50 22.2 / p95 30.2 ms, prep 5.58 ms mean against translation 17.70
(ratio 0.32: the window holds the turn's streaming); route checks **4/4 on** (grid, t37, t41, t46). Ran beside OoT at
~3.7 cores and an OoT relink. MKW-AHEAD's `mkw-turn` was 41.4 / 30.0 and MKW-SETS's 30.7 / 25.0 on other machines'
days: not comparable.

## 4. Pictures

Every sheet read: `base`, `v1`, `v2`, `final` (`sheet-1.png`), `final-turn` (`sheet-1.png`, `sheet-2.png`; `v3`'s
build is out). Boot, title (course flyover; the baseline's opening ran late, its title and menu shots are the flyovers),
grid (already racing), race on the desert slope and lap 2 by the water / steel deck: lit and textured, HUD, coins,
items, minimap, opponents in every run; no pop-in, stale or wrong textures, flicker or missing geometry. `mkw-turn`
t37-t46: up the banked slope, through the turn onto the steel deck, all correct. Route checks on in every run (3/3,
turn 4/4).

One thing the lead may want looked at, **not from this wave**: on the slope the roadside shrubs on the left render as
near-black silhouettes while one by the item boxes is green and lit (`final-turn/05-t37.png`); the baseline build draws
the same slope's shrubs the same way (`base/05-race.png`, top edge).

Anomalies: the known set only (zero stand-ins at boot and loads, none in the race windows; the crashed-pipeline and
timestamp-query red lines with the crashed pipeline's skipped draws; gate give-ups at the loads, 4 in the final
window). `held-back-frame` lines rose with the new build in every run (`base` 40 in the window, 1.0 sync a frame
holding pages under in-flight write-backs; `v1` 120 / 1.8, `v2` 140 / 1.8, `final` 90 / 1.5-1.6, turn 20), ~0.01 MB
each. The lean sync visits the same pools under the same rules (test below), so the reading here is timing: syncs come
sooner after the previous submission and find its write-backs still in flight, and the designed hold-back takes those
pages to a later sync. Not proven.

## 5. What is left

- **60 at the default pacer is not reached** (final 29.6 / 26.0; `v2` 33.3 / 29.4). The hold-1 frames on this machine
  today cost 18-24 ms median; the replay thread's whole frame (`pipelining` busy 17.5-24 ms a frame in these runs) has
  to fit 16.7 ms every frame for the share rule (or the old one) to keep 1 vsync.
- Replay thread, by `v2`'s profile: translation on it as a lane (`emit_op` 42 % of busy samples; lanes' image-lock
  waits at view misses, `tex_view_uncached` 6.3 %), pool sync 22.8 % (pool-lock wait 3.4 %, `watch::take` 4.9 %, the
  per-pool pass and uploads 5.6 %, prehash 1.4 %), driver calls (`nvoglv64` 12 %: submits in `chain_parked`, `new_sub`),
  pre-pass `plan` 5.3 %, present 2.9 %.
- The pool-lock wait (~0.7-0.9 ms a frame): not behind landing batches (§2); its holder needs a completion / walker
  thread profile (`NXRS_HOST_PROF_THREAD=completion,vk pool walker`).
- USE-SYNC's use-driven sync would take the first sync's `watch::take` (~1 ms a frame now), with the risks USE-SYNC
  lists.
- Quiet-machine numbers: every run here shared the CPU with OoT runs and builds (§Display).

## Report (agent, protocol §5)

Branch `mkwsync` (base `gameplay-harness` `0765845`): `151ea00` (plan; span edits, lean sync, share rule), `6628fb3`
(park in place, switch rows), `c1a2920` (landing yield), `bc2dba8` (landing yield taken out), this commit (doc).

### 1. Built
- **Diagnosis** (§1): a baseline profile of the replay thread put ~2.5 ms a frame in bookkeeping, not in the scan:
  seven walks over ~4,600 pools at each scanning sync, the image span list made again at nearly every sync (MKW makes
  and releases images every frame), and a parking stream's rest copied into a fresh buffer.
- **Corrections** (§2), default on, `0` = old: `NXGFX_IMG_SPAN_EDITS` (span list kept by edits), `NXGFX_SYNC_LEAN`
  (one walk for the scan lists; per-pool pass over the busy and written pools), `NXGFX_PARK_IN_PLACE` (no copy when a
  stream parks), `NXOS_FSTEP_SHARE` (pacer share rule, 75 %).
- **Effect** (§3): prep from 41 % to 27.5-28.5 % of translation time in every new-build run (≈ −1.9 ms a frame at the
  baseline's speed) plus ≈ −0.5 ms of parked-stream copy: ≈ −2.4 ms a frame on the replay thread. Like-for-like profiled
  runs: replay + prep p50 / p95 18.4 / 27.8 → 15.0 / 19.6 ms, FPS 28.7 / 16.0 → 33.3 / 29.4 (`v2`, lighter machine).
- Rejected with numbers: landing yield (lock wait did not move), taken out.

### 2. Tests (`cargo test --release -p graphics`: lib 300 passed, 6 ignored; `-p os`: 172 passed, 1 ignored)
- `image::spans_tests::edits_bring_the_list_to_what_a_rebuild_makes`: additions, removals, remove-then-add and
  add-then-remove in one batch give the rebuilt list, ordered by start; the walk finds the added images.
- `mem::seal_tests::one_walk_makes_the_scan_lists`: `scan_lists` equals `watched_pools`, `scan_list(!skip)`, the cold
  filter and the tracked filter over sealed, unwatched, VRAM, tracked, tracked-and-sealed and cold pools.
- `fstep::tests::the_share_rule_holds_1_on_a_mostly_fitting_window`: frames of which 4 in 20 run over: the old rule
  never tries hold 1 from hold 2, the share rule tries it once, keeps it, pays the late frames back (game = real time,
  nothing dropped); 23 ms frames still step to hold 2. The old rule's tests run unchanged (share 0).
- New functions and field: no failing-before state. Park in place has no unit test (`Parked` needs the queue state);
  covered by every run.

### 3. Verified
Runs, numbers and sheets in §3-§4; route checks on in every run (`mkw` 3/3 ×5, `mkw-turn` 4/4). Target: replay thread
−2 ms a frame met by the within-run ratios (≈ −2.4 ms); 60 FPS at the default pacer not reached (§5).

### 4. Unproven
- Absolute FPS and p95 gains: every run shared the CPU with OoT runs (up to 3.7 cores) and relinks; the lead's quiet
  run decides. The p95 effect is shown only by the profiled pair `base` / `v2`.
- How the share rule looks in motion at a kept hold: the sheets are stills, and on this machine it rarely kept hold 1.
- The `held-back-frame` rise (§4): timing, by my reading.
- Other titles (OoT, DKB, PLZA) not run (not owned); all four switches apply to every title.

### 5. Slips
- The landing yield was taken out with `git revert` (then `--amend` to give the revert commit its message and
  attribution), not by editor edits; the reverted files are `mem.rs`, `lazy.rs`, `env_switches.tsv`.
- Relinks set `NXRS_LINK_LIBS=-Wl,/DEBUG` (a PDB for the profiler) on the normal CLI relink. The MKW DLL in the library
  is this branch's `bc2dba8` build.
- `20261127-050054-ms-prof-hold1` died at the session move (not mine to prevent); its game exited on its own.

### 6. Questions
None.
