# MKW-HEAVY — the heavy-draw frames, the frame's tail, and the pacer's trial back-off

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~08:37, worktree
`D:\Projects\RecompLauncherEngine-mkwheavy`, branch `mkwheavy` (base `gameplay-harness` `9628e36`: MKW-SPIKE merged,
`NXGFX_UPLOAD_UNLOCKED` opt-in and left off). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only,
GPU 0 (OOT-MEM owns OoT on GPU 1). Deadline 4 h (~12:37).

## Starting point (MKW-SPIKE)

Heavy-draw stretches (2,200-2,500 draws) make the replay op 14-16 ms and the frame shown op + ~3.2 ms = 19-20 ms: late,
so the pacer leaves 1 vsync; a failed 1-vsync trial doubles the hold-off before the next (1 → 8 s at 30 FPS), which is
the 5 % low of ~29. Quiet-machine numbers of one build: 42.7-52.8 FPS mean, 5 % low ~29.5.

## Plan (written before code)

1. **Baseline** (`mkw`, default pacer, GPU 0) with the diagnostics on: frames CSV (op = `replay_ms` + `prep_ms`, by
   draw count), the frame step's summaries, the `pipelining` lines, and a chain trace over a heavy stretch
   (`NXGFX_CHAIN_TRACE`) with one new note, `cpl_land` (the completion thread's write-backs and posts done, before its
   clean-up), to split op end → next op start into: submit, GPU + wake, landing, clean-up before publication, the game's
   wake, the game to its next submit, the replay queue.
2. **The tail (~3.2 ms)**: cut what is ours. Expected: the completion thread publishes a submission's fence only after
   freeing its resources (views, buffers, descriptor pools of the parent and every child submission, recycling); the
   publication can come right after the effects the game sees (write-backs, events, queries) and the clean-up after.
3. **Heavy-draw frames**: per-draw cost on the lanes against the replay thread's serial part (joins, stitching,
   submits), from the frames CSV by draw count and the `replay up to frame` / segment lines; cut the serial part.
4. **Pacer** (`src/os/src/fstep.rs`): a failed trial no longer doubles the hold-off to 8 s; trials short and frequent,
   behind a switch; game time against real time stays ~1.000 (the summaries), frames shown late are paid back.
5. **Verify**: `mkw`, `mkw-turn` on GPU 0, route checks, every sheet; `cargo test --release -p graphics -p os`. The final
   run compared with the baseline and its diagnostics (no switch-off controls).

Expected files: `src/graphics/src/vk/queue.rs` (completion), `src/os/src/fstep.rs`, maybe `exec.rs`,
`docs/env_switches.tsv`, this doc. What would disprove step 2: the tail is GPU time or the game's own time, not the
completion's work.

## 1. The tail: op end → the next frame's op (chain trace, 288-289.5 s)

Analysis by frame: the frame's op = the longest submit op between two presents; its fence = the timeline value after
it. Segments: `submit` (op end → its last `vk_submit`), `gpu` (→ the completion thread's `gpu_done` of that value),
`gpu_last` (that submission's own GPU time, new note `gpu_ts`), `land` (→ `cpl_land`, new note: write-backs, events,
queries landed), `clean` (→ `cpl_done`: the clean-up before publication), `wake` (→ the game's `sw_out`; includes the
pacer's hold at 2 vsyncs), `game` (→ the push of the next frame's op), `queue` (→ that op's start). Medians, ms:

| run | frames | op | gpu | gpu_last | land | clean | game | tail | period |
|---|---|---|---|---|---|---|---|---|---|
| `hv-base` (hold 2, OoT on GPU 1) | 43 | 16.1 | 2.93 | - | 0.04 | 0.004 | 1.97 | 16.7 | 33.3 |
| `hv-v1` (hold 1 part) | 61 | 15.3 | 2.41 | 0.31 | 0.04 | 0.004 | 1.75 | 4.8 | 20.5 |
| `hv-v2` (final code) | 88 | 10.4 | **1.69** | 0.30 | 0.06 | 0.003 | 1.08 | 6.2 (of it pacer wait 2.9) | 16.66 |
| `hv-final2` (final code) | 82 | 11.9 | **1.45** | 0.30 | 0.05 | 0.002 | 1.45 | 4.0 | 16.53 |

- **The clean-up is not in it** (0.003-0.006 ms): the planned change (publish the fence before freeing the
  submission's resources) was built and taken out without a run; it could not move anything.
- **The GPU is not in it either**: the frame's last submission runs 0.30 ms on the GPU, yet its fence is published
  1.5-2.9 ms after its `vk_submit`. One frame of `hv-v1` read in full: the op ended at 45.27 ms; submission `1b782`
  (1.18 ms of GPU, submitted 43.86) was done by ~45.3; the completion thread landed it 45.32-46.27 (write-back
  pre-pass 0.37, pool lock 0.09, under the lock 0.46), then **waited 0.86 ms for the pool lock** to land `1b783`
  (0.2 ms of GPU) while the replay thread's pool sync of the next frame's early work (the second queue's submit, pushed
  at 39.2 and replayed at 45.97) held it; the frame's fence `1b784` was published at 47.57. The completion thread
  lands every submission in order (7 a frame), so one sync holding the lock delays the frame's fence by its whole hold.
- **The game** (fence → its next submit) is 1.1-2.0 ms: the game's own CPU (main thread release, render thread
  recording), not ours.

## 2. Heavy-draw frames

Frames CSV, [250, 325] s, op = `replay_ms` + `prep_ms`, by draws (frames with a texture upload > 0.5 ms left out):

| run | draws < 1,800 | 1,800-2,200 | ≥ 2,200 | lanes' CPU ≥ 2,200 | replay ≥ 2,200 | prep ≥ 2,200 |
|---|---|---|---|---|---|---|
| lead's quiet run `20261127-065536-mkw` | 12.71 | 13.83 | 13.83 | 16.94 | 11.57 | 2.96 |
| `hv-base` (OoT running) | 16.63 | 17.70 | 18.23 | 20.97 | 14.73 | 3.93 |
| `hv-v2` | 12.11 | 13.40 | 13.59 | 15.69 | 10.77 | 3.10 |
| `hv-final2` | 12.90 | 14.22 | 14.98 | 16.10 | 11.73 | 3.37 |

What +600 draws cost (`hv-final2`): lanes' CPU +4.0 ms (≈0.7 ms of wall on 6 lanes), replay thread +1.6 ms, prep +0.2:
the serial part grows more than the lanes' share. In the replay thread per frame (`translate` lines): pre-pass
1.3-1.7 ms, the replay thread's own share of segments (~19 %), and **its wait for the slowest lane at the join, 1.5-3.4
ms**; the lanes wait 1.8-5.3 ms (CPU, all lanes) for the images' lock.

- **Tried, rejected: 8 lanes** (`NXGFX_TRANSLATE_THREADS=8`, env only, `hv-v3-l8`): lanes' CPU 26 → 31-34 ms a frame,
  images-lock wait 1.8-2.5 → 4.5-4.7 ms, join wait no lower (2.5-3.3), op p50 / p95 14.98 / 20.60 against `hv-v2`'s
  12.93 / 16.72, 48.8 / 34.8 FPS. More lanes buy contention on the images' lock, not wall time. Default stays 6.
- **Tried, taken out: the lanes and the completion thread in the "Games" MMCSS class** (`hv-v1`): join wait 2.3-2.9 ms
  against 2.5-3.4 in the (loaded) baseline and 2.48 in the lead's quiet run; the landings' lock waits unchanged (mean
  242 µs against 266). Nothing moved that the class explains: removed.
- **Not built**: the lever is the images' lock in the lanes' view lookups and uploads (`tex_view_uncached`,
  `prepare_image`), i.e. MKW-SPIKE's `NXGFX_UPLOAD_UNLOCKED` path, opt-in since the TDR. Left for a wave that can
  re-validate it.

## 3. Built (each behind a switch, default on, `0` = the old path; rows in `docs/env_switches.tsv`)

### Landings first (`NXGFX_LAND_FIRST`; `queue.rs` `land_first`, `mem.rs` `begin_submit_ex`)

Before a pool sync on the replay thread takes the pool lock, it reads the timeline semaphore's counter
(`vkGetSemaphoreCounterValue`); when the GPU has finished submissions the completion thread has not published yet, it
waits for their publication (`wait_fence`, at most 1.5 ms) and then takes the lock. The landings no longer stand
behind a sync of the next frame's early work, so the frame's fence (what the game's next frame waits for) comes out as
soon as its submissions are done. Off the replay thread, or with nothing finished-and-unlanded, nothing changes.
Counted in the `pools` line (`landings first N syncs, ms`) and as chain note `land_first`.

Measured: the syncs waited for landings 0.64-0.80 ms a frame, ~1.1 syncs a frame, all of them in the small ops after
the present (the second queue's early submit; chain trace: none inside the frame's op); the syncs' own pool-lock wait
0.66-0.72 → **0.10-0.18 ms** a frame; the landings' pool-lock wait (chain `wb_lock`) mean 242-266 → **130-161 µs**
(max 1,432-2,725 → 936 µs); the tail's `gpu` segment 2.41-2.93 → **1.45-1.69 ms**; the completion's write-back time
4.1-4.5 → 3.1 ms a frame.

### Retry rule (`NXOS_FSTEP_RETRY`; `src/os/src/fstep.rs`)

- A failed trial no longer doubles the hold-off (1 → 8 s): every trial waits 0.5 s (`RETRY_HOLD`) and, with the share
  rule, 20 frames at the longer hold (`RETRY_WIN`) instead of 60.
- A hold is left for a longer one only when its frames run **half a vsync** past it on average over the last 30
  (`RETRY_SHORT`, instead of `LATE` 0.1): frames of 17-25 ms stay at 1 vsync, shown late and paid back by the step
  (game time = real time), instead of 30 FPS. Three late frames in a row and the share rule's "fewer than 75 % on time"
  still have to hold too.

Measured (`hv-v1` had the first half only, `hv-v2` both): `hv-v1` made 138 failed trials in its last summary period
(the cheap retry alone turned the 8 s waits into constant flapping: 40 hold changes in the window, 5 % low 28.5);
with the half-vsync rule `hv-v2` made 8, held 1 vsync for 1,445-1,800 of every 1,800 frames, 26 hold changes. Game
time against real time in the race summaries: 0.999-1.001 (one summary each at 0.938-0.972 that holds a load or a
stall past 0.25 s, dropped as before; the baseline has the same).

## 4. Measured

Window [250, 325] s, default pacer, GPU 0, chain trace 288-289.5 s in every `mkw` run. FPS / 5 % low from the
report (per-second metrics) and, in brackets, from the frames CSV's presents per second (the metrics rows stopped
in `hv-final2`, see below).

| run (`build/play/`) | build | what else ran | FPS / 5 % low | op p50 / p95 | op ≤ 15.8 | frames at 1 vsync | route |
|---|---|---|---|---|---|---|---|
| lead's `20261127-065536-mkw` (main) | `9628e36`-equivalent | quiet | 42.7 / 29.4 | 13.33 / 18.32 | 80 % | 2,038 of 3,214 | 3/3 |
| **`20261127-084735-hv-base`** | base + notes | OoT `om-fin-house` (GPU 1) whole window | 29.8 / 24.7 (29.6 / 20) | 17.66 / 25.65 | 24 % | 173 of 1,755 | 3/3 |
| `20261127-085949-hv-v1` | + cheap retry, MMCSS (out) | OoT ended 09:02:50, before the window | 40.6 / 28.5 (38.8 / 24) | 15.59 / 20.69 | 54 % | 1,799 of 2,912 | 3/3 |
| **`20261127-091104-hv-v2`** | final code (`efffdc9`) | nothing of ours known | **56.1 / 46.0** (56.1 / 43) | **12.93 / 16.72** | **91 %** | 4,030 of 4,206 | 3/3 |
| `20261127-091706-hv-v3-l8` | final + 8 lanes (env) | - | 48.8 / 34.8 | 14.98 / 20.60 | 63 % | 3,296 of 3,654 | 3/3 |
| `20261127-092811-hv-final` | final | - | 50.2 / 28.7 | 13.88 / 19.07 | 77 % | 3,528 of 3,770 | 3/3, **wrong course** |
| `20261127-093420-hv-final2` | final | the owner at the machine from ~154 s | (51.4 / 24) | 13.80 / 17.56 | 86 % | 3,589 of 3,852 | 3/3 |
| `20261127-092306-hv-final-turn` (`mkw-turn`, [250, 290]) | final | - | **58.4 / 52.8** | - | - | - | 4/4 |

- **`hv-final` raced another course** (the race shots show a seaside town and a staircase, not Mario Bros. Circuit):
  the menu presses landed on another cup. The route checks look at the HUD only, so they passed. 81 pipelines were
  compiled in its window (others: 8-36), slow frames of 130-560 ms are pipeline builds (`pipelines 568.6 ms (6
  made)`): its FPS is not comparable. Its pictures are correct.
- **`hv-final2`**: the per-second metrics rows stop at 154 s (the front loop's once-a-second block did not run; the
  game, its log and its frames CSV went on), about when the owner wrote to the session; a 1.9 s gap without a present
  at 315.5 s (frame 16038, `frame_ms` 1,900, replay 11.6 ms) is not in the engine's replay. The report's 31.8 / 0.0
  is the 6 metrics rows; the frames CSV gives 51.4 / 24 (the 24 is the second holding the 1.9 s gap).
- **Final against baseline**: the baseline ran beside an OoT run and is ~4 ms slower in every stage than the lead's
  quiet run of the same code, so the lead's run is the fairer reference. Against it: `hv-v2` op p50 / p95 13.33 /
  18.32 → 12.93 / 16.72, frames at 1 vsync 63 % → 96 %, FPS 42.7 → 56.1, 5 % low 29.4 → 46.0; `hv-final2` 13.80 /
  17.56, 93 % at 1 vsync, 51.4 FPS. `mkw-turn` 58.4 / 52.8 (MKW-SPIKE's 55.0 / 31.2).
- **60 at the default pacer: not reached** (56 at best; frames of 17-20 ms are shown at 1 vsync late and paid back,
  which the 60 FPS mean does not count as 60).
- No exit 71 / 72 or device loss in any run of this wave.

## 5. Pictures

Every sheet read (`sheet-1.png` of every run, `sheet-2.png` of `hv-final-turn`): boot (highway, Mario's kart and the
bus), title (Mario Kart arch with the field at the start line; in `hv-final-turn` the opening-to-title cut's emblem on
black, as earlier waves), menus, grid, the desert slope and lap 2 by the water / the steel deck; `hv-final`'s other
course (town, beach, staircase) also correct. Lit and textured, HUD, coins, items, minimap and opponents in every run.
The sheets show no difference from the baseline. The roadside shrubs render as dark silhouettes in every run,
baseline included (known since MKW-SYNC).

**This wave did not fix MKW's graphical issues.** The owner reports (2026-11-27, during this wave), playing the game:
**missing textures, severe pop-in, and a broken character screen**. All of them are still there. This wave worked on
frame time and pacing only and changed no rendering. A few still shots per run cannot show pop-in, and the scenario
has no shot of the character screen. So "the sheets show no difference" does not mean the picture is right. These
are open rendering bugs for a wave of their own. Anomaly tables: the
baseline's set only (zero stand-ins before 165 s, none in the window; crashed-pipeline and timestamp-query red lines;
gate give-ups at loads; held-back frames 0-70 in the window).

## 6. What is left

- **Graphics (owner-reported, not fixed here)**: missing textures, severe pop-in, the character screen not displaying
  properly. These need a rendering wave with shots at the character-select screen and frame-by-frame captures in
  the race (`--faults`) to catch the pop-in.
- **60 FPS**: the frame is op + ~4-5 ms of tail at 1 vsync, so the op needs ~12 ms; it is 12.9-13.9 ms p50. The
  biggest remaining serial items: the join wait for the slowest lane (1.5-3.4 ms a frame, driven by the images' lock in
  the lanes, §2), the pre-pass (1.3-1.7 ms), the pool sync (~3 ms).
- **The tail's game part** (1.1-2.0 ms) is the game's; the landings still cost ~0.5 ms each under the lock for big
  submissions.
- **Big uploads / first pipelines**: single frames of 100-560 ms (texture streaming, pipeline builds on a new course)
  still set the 5 % low when they come in a burst.
- **Harness**: `mkw`'s route checks cannot tell the course (HUD only); `hv-final` raced another cup and passed.

## Report (agent, protocol §5)

Branch `mkwheavy` (base `9628e36`): `93a28a4` (plan; retry rule), `efffdc9` (landings first; half-vsync rule; chain
notes), this commit (doc).

### 1. Built
- **Diagnosis**: the ~3.2 ms between op end and the next frame is not GPU work (the last submission 0.3 ms) nor the
  completion's clean-up (0.004 ms): it is the completion thread landing the frame's 7 submissions in order while the
  replay thread's pool sync of the next frame's early work holds the pool lock (0.86 ms in the frame read in full),
  then the game's own ~1.5-2 ms. Heavy frames: the replay thread's serial part (join wait, pre-pass) grows faster than
  the lanes' share; the lanes wait on the images' lock.
- **Corrections**: `NXGFX_LAND_FIRST` (finished landings go before a sync takes the pool lock); `NXOS_FSTEP_RETRY` (no
  doubled hold-off; a hold is left only at half a vsync late on average).
- **Taken out** (no movement): fence publication before clean-up; MMCSS for lanes and completion thread; 8 lanes.

### 2. Tests (`cargo test --release -p graphics -p os`: graphics lib 306 passed, 6 ignored; os lib 172 passed, 1 ignored)
- `fstep::tests::the_retry_rule_keeps_near_misses_and_tries_again_soon`: 19.7 ms frames keep hold 1 with the retry
  rule and pay the late frames back (old rule: 30 FPS); 28 ms frames go to hold 2 and try hold 1 every ≤ 100 vsyncs
  (old: gaps doubling to ≥ 480); hold 1 is back within 60 vsyncs when the stretch ends; game time = real time. New
  rule, so no failing-before state; the old-rule halves assert the old behaviour.
- `land_first` has no unit test (it needs a device and a completion thread); the runs show it (§3).

### 3. Verified
Runs §4, pictures §5. Route checks on in every run (`hv-final`: on, but another course); `mkw-turn` 4/4 on, 58.4 /
52.8. Best final `mkw` 56.1 / 46.0 (`hv-v2`), op p50 / p95 12.93 / 16.72. Target 60: not reached.

### 4. Unproven
- **Graphics are not fixed**: the owner reports missing textures, severe pop-in and a broken character screen in MKW.
  This wave did not address them (§5).
- How much of `hv-v2` → `hv-final2` (56.1 → 51.4) is the owner's activity in `hv-final2` and how much run-to-run spread;
  the lead's quiet-machine run decides.
- Other titles (OoT, DKB, PLZA) not run; both switches apply to every title (`NXGFX_LAND_FIRST` only waits when a sync
  meets finished, unlanded work; the retry rule changes every title's pacer).
- The visual cost of late frames at hold 1 (frames of 17-25 ms shown as they come on a 60 Hz display) is not judged
  by still sheets.

### 5. Slips
- `git checkout src/graphics/src/vk/queue.rs` (and `rtclass.rs`) to take out my own uncommitted changes (the publish-
  early change and the MMCSS lines); only my own edits were in those files. No sed/script edits of source.
- `hv-final` ran another course, noticed only from its sheets after the run; its numbers are not used.

### 6. Questions
None.
