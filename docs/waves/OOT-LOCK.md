# OOT-LOCK — Ocarina of Time: the replay thread and memory (47 -> 59 FPS)

Owner: one delegated agent. Lead: main session. Opened 2026-11-28, worktree `D:\Projects\RecompLauncherEngine-ootlock`,
branch `ootlock` (base `gameplay-harness` `013c5d4`: OOT-WAIT merged). Protocol `docs/31-DELEGATION-PROTOCOL.md`.
Title: Ocarina of Time only, GPU 0. Deadline 5 h.

## Plan (written first)

Starting point: OOT-WAIT's `D:\Projects\RecompLauncherEngine\build\play\20261128-014336-wait-fix1` (`oot`, [845, 915]
s): 47.1 FPS / 5 % low 30.0; house `-015941-wait-house` 44.4 / 41.0. Its run.log in the window: the replay thread busy
16-19 ms a frame and idle 1-2 ms (it sets the frame); pool sync ~7 ms a frame of it ("landings first" 0.9-1.4 ms,
lock wait 0.3-0.5 ms, write-watch scan 2.2 ms, per-pool pass 3.0 ms); the completion thread waits 2.9-3.5 ms a frame
for the pool lock and lands 1.9-2.6 ms a frame under it; the main thread waits ~5 ms a frame in `nvnSyncWait`;
memory 29.2 -> 30.7 GB over the window (textures ~6.2 GB and growing).

0. **Baseline** with the diagnostics on: this worktree's dist (`013c5d4`), OoT relinked from it with a PDB
   (`NXRS_LINK_LIBS=-Wl,/DEBUG`), `oot` on GPU 0 with `NXRS_WAIT_ACCT=1` and the host profiler on `vk replay` with
   stacks over 850-890 s (`NXRS_HOST_PROF=850,40`, `NXRS_HOST_PROF_STACK=1`). Split the replay thread's frame by
   callee (pool sync and its waits, set building, lanes) and read the `waits:` blocks, the pools / completion lines
   and the memory / image census lines over the window. Whichever stage is largest and cuttable goes first, said
   here with the numbers.
1. **(a) Landings and the pool lock.** The replay thread waits for the completion thread twice per sync: in
   `queue::land_first` (while a game thread waits for a fence) and at `g.mem`. Make the landings stop standing in
   the sync's way: candidates are landing outside `g.mem` (the guest-memory stores and the per-page work of
   `mem::apply_writebacks` / `land` need only the landed pools' state and the watch, not the whole table), or a sync
   that does not wait for landings and lets the next sync see them. Whatever the change, a write-back's bytes still
   land before its fence is published, and CPU writes the game makes meanwhile are still uploaded.
2. **(b) The main thread's fence wait** (`nvnSyncWait`, ~4.8 ms a frame): find from the wait accounting and the chain
   trace which fence it waits on and what holds that fence (replay latency, GPU, landing, publication), and cut that
   part if it is the engine's.
3. **(c) Memory** (peak 30.7 GB against the 32 GB cap): from the memory and image census lines, what grows over the
   window (textures, pool granules, write-back buffers, parked images, staging) and whether it is freed; fix a leak or
   unbounded cache if one is found.

Each change behind a switch (rows in `docs/env_switches.tsv`), default on once its pictures are right; a change that
leaves its stage's ms unchanged is removed. Excluded: Fault A (white house floor), skipped draws or hidden work,
faked completions, weaker checks; OOT-FIX's colours stay; live native NDVE off.

Expected files: `src/graphics/src/vk/mem.rs`, `queue.rs`, `lazy.rs`, maybe `upskip.rs`, `image.rs` (memory),
`src/graphics/src/nvn/sync.rs` (fence wait), `docs/env_switches.tsv`, this document.

Verification: one normal `oot` run after the work against the baseline and its diagnostics (no switch-off controls),
every sheet read, report.md's anomaly table and route checks; one final `oot-house --faults` run (Fault A count as an
observation). `cargo test --release -p <crate>` for each changed crate, with tests for new behaviour. Numbers: FPS mean
/ 5 % low, the game frame's median vsyncs (pacer line), replay + prep ms, each targeted wait before and after, peak
memory, against the baseline and 59.

What would disprove the plan: the profile puts the replay thread's frame elsewhere (then that is cut first), or the
landings' removal from the sync's path leaves the replay thread's busy ms where they were.

## Baseline (`build/play/20261128-024046-lock-base`, `013c5d4`, OoT relinked with a PDB)

`oot` on GPU 0, `NXRS_WAIT_ACCT=1`, host profile of `vk replay` with stacks over 850-890 s, chain trace at 870 s
(`NXGFX_CHAIN_TRACE=870:0.6`). On route, sheets read. **50.0 / 5 % low 39.0**, frame 19.78 ms, replay + prep 16.04 ms
(p50 16.00), 1,141 draws, pacer median 1.31 / 1.16 vsyncs (fitting one: 5 / 7 %), peak 28,458 MB.

- **Replay thread** (profile, ~2,000 frames, ms a frame): pool sync `begin_submit_ex` 7.5 (`prehash` 1.64,
  per-pool pass 1.58 of which `upload_changed` 1.25, `land_first` 1.42, `watch::take` 1.30, the pool lock 0.40),
  translation `record` 5.4, lane joins 1.6, `submit_sub` 1.1. Pools lines: landings first 1.36 ms, lock wait 0.49.
- **Completion thread** (pools lines): waits 3.42 ms a frame for the pool lock, lands 2.82 ms under it.
- **Main thread** (`waits:` 850-910 s, ms a second): jobs 333, `nvnSyncWait` 272 (~5.4 ms a frame), submits 44.
- **The chain** (`chain.py` / `gpusim.py` over the trace, GPU finish times simulated from submit times and
  `gpu_ts`): the main thread waits at each frame's start for the previous frame's last submission. Its GPU work is
  done 0.6-2.5 ms into the wait, its landing 5-7 ms in. Example at trace 165.7 ms: the replay thread's `land_first`
  waited 2.2 ms for the submissions finished when it began, timed out behind a 2.3 ms landing of 75 write-backs, then
  the next frame's first sync held the pool lock 4.6 ms; the completion thread's next landing waited all of it, and
  the game's fence with it (7.56 ms wait). Over the 0.6 s trace: 143 ms of fence waits, 62 of them GPU still working.

## Changes

### A. Landings first wait for the waited fence (`NXGFX_LAND_FIRST`, default; `any`: OOT-FIX's rule) - kept

`GuestWait` now records the fence each waiting game thread waits for (eight slots; a wait without a slot falls back to
the old rule). `land_first` waits only for the oldest waited fence that is submitted and not landed, until that one
lands: 1.5 ms at most when the GPU has finished it, 4 ms when it is still on the GPU (so the next sync no longer takes
the pool lock just before the landing the game waits for). A waited fence not submitted yet gives no wait. The pools
line counts the syncs of each kind. `src/graphics/src/vk/queue.rs`, `nvn/sync.rs`, `nvn/window.rs`, `vk/mem.rs` (line).

### B. Landings merged on the page-hash threads (`NXGFX_LAND_PAR`) - built, measured, taken out

The completion profile (`lock-v1`, `nvn-completion`) put `merge_changed_bytes` at 9.5 % of its samples (~1.9 ms a frame
under the pool lock). Landings of 4+ runs went to the page-hash threads, each run's take, merge and reset on one
thread; partial words stored by their set bits (`aeb71c9`). Then 2+ runs and single runs' pages (`b55481d`, taken out
in `807e5c5`). The landed time over the window did not move clearly (2.82 base, 2.51 with B, 2.92 with the wider B,
3.21 after both were out), so both were taken out (`4641f8b`).

### C. Memory - observation, no change

Peak 28.2-29.2 GB in every run of this wave (OOT-WAIT's 30.7 GB was a different walk). Private memory is ~16 GB plus
the Vulkan allocator's blocks; the blocks track texture memory. Over the walk the live image count stays ~6,700 while
images are made and destroyed (~15 a second each) and texture bytes grow 5.3 -> 6.6 GB as aliases (no memory) are
replaced by images with their own memory. Freed, not leaked; no cap reached.

## Runs (`oot`, GPU 0, this worktree's `build/play/`, window [845, 915] s, all on route, sheets read)

| run | build | FPS / 5 % low | draws | frame ms | replay+prep (p50) | pacer median (fit 1) | landings first / sync lock wait | completion lock / landed | main `nvnSyncWait` / submit (ms a s) | peak |
|---|---|---|---|---|---|---|---|---|---|---|
| **`20261128-024046-lock-base`** | `013c5d4` | **50.0 / 39.0** | 1,141 | 19.78 | 16.04 (16.00) | 1.31, 1.16 (5-7 %) | 1.36 / 0.49 | 3.42 / 2.82 | 272 / 44 | 28,458 MB |
| `20261128-030353-lock-v1` | `2b13425` (A), completion profiled | 49.0 / 35.0 | 1,507 | 20.13 | 16.82 (19.13) | 1.29, 1.11 (16-31 %) | 1.24 / 0.72 | 1.92 / 3.26 | 156 / 130 | 29,069 MB |
| `20261128-032511-lock-v2` | `aeb71c9` (A + B) | 53.1 / 42.9 | 1,072 | 18.63 | 15.44 (15.97) | 1.23, 1.07 (17-36 %) | 0.96 / 0.70 | 1.80 / 2.51 | 152 / 121 | 29,191 MB |
| `20261128-034515-lock-final` | `b55481d` (A + wider B) | 50.8 / 33.0 | 1,094 | 19.49 | 16.06 (17.24) | 1.12 (20 %) | 0.99 / 0.74 | 1.88 / 2.92 | 139 / 131 | 29,037 MB |
| **`20261128-040748-lock-after`** | **`4641f8b` (A)** | **46.9 / 30.0** | 1,373 | 20.99 | 17.14 (18.86) | 1.26, 1.10 (9-30 %) | 1.36 / 0.71 | 2.00 / 3.21 | 152 / 124 | 28,418 MB |

The walk's draws vary run to run (1,072-1,507 in the window), so frame times by draws (600-915 s, `bins.py`):

| draws | base | after (A) | v2 (A + B) |
|---|---|---|---|
| < 900 | 18.10 | 17.87 | 17.57 |
| 900-1,100 | 19.06 | 18.83 | 18.20 |
| 1,100-1,300 | 20.16 | 20.44 | 19.09 |
| 1,800+ (most frames: standing in the house) | 21.75 | 21.68 | 21.53 |

## What A did, and why the FPS did not follow

The main thread's fence wait fell 272 -> 152 ms a second (~5.4 -> ~3.0 ms a frame); the completion thread's wait for
the pool lock 3.42 -> 2.00 ms a frame. The chain trace of the after run: 98 ms of fence waits in 0.6 s (143 before),
69 of them with the GPU still working: the landing part of the wait is ~2.6 ms a frame shorter. But the main thread now
waits that time in `nvnQueueSubmitCommands` instead (44 -> 124 ms a second: room in the replay queue), and the other
fence waits are for ops the replay thread has not replayed yet (+8 to +10 ms). The replay thread is the bound: busy
18.4 ms a frame before, 19.5 after (its landing waits unchanged: landings first 1.36 ms, lock 0.49 -> 0.71). At equal
draws the frame time is the same as the baseline's within run-to-run noise (+-1 ms a bin). The FPS means differ by the
walk (after: 1,373 draws against 1,141).

## `oot-house --faults` (final build `4641f8b`)

`build/play/20261128-042313-lock-house` (`NXGFX_FAULT_MAX=200`, `NXGFX_FAULT_PER_REASON=80`): completed, normal exit,
house window [610, 690] **46.9 / 43.0 / min 42.0** (OOT-WAIT's `wait-house` 44.4 / 41.0), peak 28,183 MB, on route
(0.019-0.044 at d01/d10/d20/d30/last). Captures: 100 `no-target` (the known empty pixel shader), 2 black-frame
captures (scene cuts), the opening's two white-light frames (143 s), and **16 one-frame white flashes at 422-677 s**;
`fault-121-p31735.png` is Fault A (floor, shelves and grass blown white with lightning streaks; Link, door, lanterns
right), and the harness shot `25-d22.png` (677 s) caught one. Observation only (OOT-WAIT: 10).

Two earlier `lock-house` folders (`-040033`, `-040402`) are runs stopped by PID within minutes because the build was
about to change; their numbers are not used.

## Pictures

Every sheet of the baseline, v1, v2, the after run and the house run read: OOT-FIX's colours (dark interiors lit by
the lanterns, Navi's glow, the title card), Link in his house at every check shot, the walk with each run's own camera
path. No picture changed with these changes. The opening differs in timing only.

## What is left to 59 FPS

The replay thread (~18-19.5 ms busy a frame at ~2,000 draws standing in the house): translation ~5.4 ms, pool sync
~7.4 ms (page hashing ~2.9 ms of it, the write watch 1.2 ms, landing waits ~2 ms). Until it is under ~15.8 ms, cuts
elsewhere move waits between the game's threads (as A did) without moving the frame.
