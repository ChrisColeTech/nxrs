# OOT-OCC — Ocarina of Time's presents no longer wait for the replay to catch up (occlusion results)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~22:10, worktree
`D:\Projects\RecompLauncherEngine-ootocc`, branch `ootocc` (base `ootdesc` `6f7413f`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1 (SET-FAST had MKW on GPU 0 throughout; the web
wave's Chrome ran at ~300 % CPU during v2). Deadline 4 h.

## Goal and starting point

OOT-DESC: OoT reports samples-passed counters, so every present waited in `aq_wait_room` until the replay thread had
replayed everything queued (`occl_sync`, MK8D's rule); the game's render thread spent 83 % of its time there. Target 59.

## Plan (written first)

1. What OoT asks (diagnosis): its samples-passed resets and reports, addresses, draws between a reset and its report,
   render conditions on report addresses; the results; how many presents after submission each lands; whether the
   game rewrites report memory itself. New `vk/occl.rs`.
2. References: Turbo, nxnative, kirikae.
3. The non-stalling rule behind a switch (`NXGFX_OCCL_LAG`, 0 = old).
4. The frame step with the wait gone.
5. Verify: `oot` (route checks, every sheet), `oot-house --faults`, same-build switch-off controls.

## 1. What OoT asks (measured, `occlusion, per frame` line, every 600 presents)

- No samples-passed counters before Kokiri Forest (the first at 206-360 s, run to run). From there **every report has
  its own ResetCounter: 1.0-1.1 draws between a reset and its report**, i.e. one bounding-box draw per object test.
- **~120 tests a frame in the forest cutscenes, 440-920 on the village paths, ~1,380 a frame in Link's house standing,
  1,300-2,140 on the house walk.** The records go to a ring of ~50,600 distinct addresses (each test a fresh slot).
- **~45 % of them are read back by the GPU itself**: 200-990 `SetRenderEnableConditional` a frame name a report
  address (predicated draws: the object is drawn only if its box passed). Those conditions are not known before the
  GPU runs, so the replay keeps drawing (`counters::rc_value` is None for a samples-passed address, as in Turbo and
  nxnative); they do not depend on when the result lands.
- Results: 51-92 % zero (a box fully hidden). The game **never rewrites report memory** before resubmitting or before
  a landing (0 of ~10^6 a run in v2/v3/v4/v5; 3-6 in one v2 window): it reads whatever record is there, it does not
  clear and poll. So the most recent completed record at each address is what it sees, whatever the lag.
- With `NXGFX_OCCL_LAG=1` every result landed 1 present after its submission (0 % at 0, 0 % at 2+); with 0 the game
  waited for them.
- **Queries were refused**: with 256 occlusion queries a submission, ~1,000 queries a frame in the house (600,000 per
  600 frames) found the pool used up, so those boxes' samples were not counted (their reports summed only what came
  before). Zero share in the house: 89-91 % with 256, 74-79 % with 4,096 (v2).

## 2. How the references answer (source read)

- **Turbo** (`D:\Projects\Switch2RECOMP-main\runtime\nvn\vk\vk_replay.c` 288-312, 2266-2285): real queries, a pool of
  NQ slots allocated round-robin and freed at completion; the result is summed and the record written by a
  completion callback (`vks_post_fn` → `q_post_run`). It replays at submit on the game's thread, so no extra lag;
  nothing waits for the result: the game reads whatever has landed.
- **nxnative** (`D:\Projects\nxnative\dm\gfx\nvn\nvn_mtl.m` 1436-1460, 2770, 17045-17063; `nvn_geometry.inc` 1222):
  Metal visibility buffers, one slot per render pass while counting, folded into the record at the submission's
  completion; also draws nothing reads are not counted (`DM_NVN_VIS_NEED`). No wait at present.
- **kirikae** (`D:\Projects\kirikae\crates\rt\src\gpu\threed.rs` 56, 340-364): no query at all: a samples-passed
  report writes the constant `OCCLUSION_VISIBLE` (1 << 24) at once ("always visible").
- All three: render conditions on an unknown result draw.

## 3. Design (built)

- **`NXGFX_OCCL_LAG` (default 1; `vk/occl.rs` `lag`, `present_waits`; `queue.rs` `aq_wait_room`)**: while occlusion
  results are in use (`occl_sync`), a present waits only while one present is still queued, instead of until every
  op queued before it has been replayed. The game builds frame n + 1 while the replay translates frame n; the results
  it reads are the most recent completed ones (one frame later than with 0 — on the console a box test is read a
  frame or more later too, which the game is written for; nothing it does depends on same-frame results, see 1).
  0 = the old wait (MK8D). `NXGFX_OCCL_SYNC=0` still turns the wait off altogether.
- **`NXGFX_OCCL_EARLY` (default 1)**: where the game rewrote a report's memory since its last landing, the previous
  answer is put back at submit, so a game that clears and polls reads the most recent completed result instead of
  waiting. OoT never does this (0 rewrites), so it is a no-op here; kept for games that do.
- **`NXGFX_OCCL_POOL` (default 4096; `queue.rs` `occl_pool`)**: occlusion queries a submission has (was 256), so OoT's
  ~1,400 box tests a frame are all counted. 256 = as before.
- Diagnosis: the `occlusion, per frame` line (above), including the presents' wait for the replay and the replay
  thread's busy time a frame (`PRESENT_WAIT_NS`, `aq_ns`).

Not chosen: kirikae's constant "visible" (would make every predicated or CPU-culled object count as seen: wrong for
lens flares, sun, lights); answering from a CPU-side guess at submit (no way to know the box's result before the GPU).

## 4. Runs (GPU 1, `oot`, window [845, 915] s = the house walk; [600, 830] = Link standing in his house)

FPS / 5 % low are the report's (window); the rest from `frames-*.csv` and the log. "wait" / "busy": the presents'
wait for the replay and the replay thread's busy time, ms a frame, house. Shown vsyncs: share of frames on screen
1 / 2 / 3 / 4+ vsyncs. Frame-step: frames held 2 of each 1,800 in the house, cost median.

| Run (`build/play/`) | LAG / POOL | FPS / 5 % low [845, 915] | FPS [600, 830] | house shown 1/2/3/4+ (%) | wait / busy | held 2 of 1,800 | cost median | route |
|---|---|---|---|---|---|---|---|---|
| OOT-DESC `200158-od-v8-oot` (base) | old | 21.0 / 10.0 | 22.2 | 0 / 46 / 42 / 12 | (83 % of the render thread) | 540-1,111 | 1.76-1.85 | 6/6 |
| `223847-oc-v1-oot` | 1 / 256 | **invalid** (0.2: machine stall 690-930 s) | - | - | - | - | - | 5/5 |
| `225932-oc-v2-oot` | 1 / 4096 | 14.5 / 8.0 | 16.5 | 0 / 3 / 55 / 42 | 0.0-0.2 / 33-55 | 89-552 | 1.80-2.65 | 6/6 |
| `231450-oc-v2-off-oot` (control) | 0 / 256 | 15.5 / 8.0 | 22.0 | 0 / 27 / 64 / 9 | **29-41** / 34-48 | 156-880 | 1.97-2.39 | 6/6 |
| `233030-oc-v3-lag1-p256-oot` | 1 / 256 | 16.2 / 8.8 | **27.4** | 0 / 64 / 34 / 2 | **0.0-0.2** / 31-33 | **1,476-1,653** | 1.33-1.85 | 6/6 |
| `234539-oc-v4-lag0-p4096-oot` | 0 / 4096 | 15.8 / 6.1 | 23.3 | 0 / 33 / 64 / 3 | 28-30 / 33-36 | 616-1,148 | 1.94-1.96 | 6/6 |
| `000105-oc-v5-oot` (final, defaults) | 1 / 4096 | 16.3 / 6.2 (to 884 s) | **28.4** | 0 / 73 / 26 / 2 | **0.0-0.2** / 31-35 | **1,525-1,770** | 1.41-1.88 | 5/5 * |

House runs (`oot-house --faults`, window [610, 690] = Link standing, A every 1.5 s):

HOUSE_TABLE

Notes:

- The machine was shared throughout: SET-FAST's MKW (~370-440 % CPU) and builds, and the web wave's Chrome (~300 %
  CPU during v1-v2). v1's frames from 690 s on took 1-9 s with the replay at 13-18 ms: the guest's cores held by
  six worker threads at ~1 % CPU and "waiting" climbing in the heartbeat (host CPU starvation, as OOT-DESC's v3); its
  window is invalid. v2 ran under Chrome's load; its house numbers (replay busy 39-55 ms against 31-35 in v3 and v5
  with the same rule) are the machine's, not the rule's: v5 is v2's build and switches.
- **The pool costs nothing** (v4 against the control at LAG 0: 23.3 vs 22.0 FPS, busy 33-36 vs 34-48).
- **The lag rule is the gain**: the presents' wait for the replay goes from 28-41 ms a frame to 0.2, and in the house
  the frame step now holds 2 vsyncs (30 FPS) for 85-98 % of frames instead of 9-49 %: 27.4-28.4 FPS against
  22.0-23.3 in the same session's LAG 0 runs (+22-29 %), 22.2 in OOT-DESC's final.
- The window [845, 915] (the walk) is not improved: there OoT makes 1,300-2,140 box tests a frame and the replay
  thread is busy 45-67 ms a frame in every run, so frames take 3-4 vsyncs whatever the wait.
- \* v5 ended at 884 s (`memory allocation of 391528417 bytes failed`, exit fast-fail): that is exactly the size of
  OoT's `pipeline-cache.bin` loaded at start, i.e. `pcache::save_now`'s `get_pipeline_cache_data` buffer failing to
  allocate under the machine's memory pressure (OoT's process at ~32.5 GB, MKW ~18 GB beside it). Not this wave's
  code; no other run of any worktree has the line. Reported, not fixed.
- Sheets read: every sheet of v1 (to its stall), v2, v2-off, v3, v4, v5, and the house runs; all correct. Objects
  occlusion decides (Navi's glow, the door's light, the lamps and pots, the herbs, the window's light shafts) are
  present and steady in every house shot; no popping or flicker in the 30 house shots a run, nothing missing in the
  walk shots. The forest-load black frame (`05`/`06`, no-target draws) and the nightmare's lightning are the known
  ones (OOT-DESC). Red lines: the known two (no-target draw, crashed pipeline) and, with POOL 256, the query pool's.
  Fault captures (house): the same 16 kinds as OOT-DESC's: lightning (1-2), forest-load no-target (3-10), scene
  cuts in the forest cutscenes (11-16, e.g. `fault-14-p13192.png`: a camera cut to the fence); none in the house
  stretch.

## 5. What's left between this and 59

- **The game no longer waits for the replay at its presents** (0.2 ms a frame). The frame is now the replay thread's
  own work: busy 31-35 ms a frame in the house standing (~1,380 box tests a frame), 45-67 ms on the walk. 30 FPS held
  needs it under ~31.7 ms (0.95 x 2 vsyncs) in nearly every frame; 59 needs under ~15.8 ms, i.e. roughly half of
  today's house cost and a quarter of the walk's.
- **The frame step is not what blocks 59.** It holds whole vsyncs (1 / 2 / 3); with the cost median at 1.4-1.9 vsyncs
  it holds 2 for 85-98 % of house frames (v5). Its quantization does cost the frames between 30 and 60 (a cost of
  1.3 vsyncs is shown for 2), but it drops to 1 vsync as soon as 30 frames in a row fit 0.95 vsync, so with a 15 ms
  frame it gives 60. Not changed: an in-between hold (e.g. 40 FPS on a 120 Hz display) is a pacing-design choice,
  not a fix.
- Where the replay thread's time goes is OOT-DESC's list (translation ~25 %, pool sync ~20 %, presents, image prep)
  plus, on the walk, the box tests themselves: each of the 1,300-2,140 a frame is a reset, one draw and a report,
  ~45 % of them read back by a render condition. Next: TRANSLATE-REUSE (the same draws every frame), and making the
  box tests cheap to translate (one draw each, the same few pipelines).
- MK8D and other games that use samples-passed results now also run one frame behind with the default
  (`NXGFX_OCCL_LAG=1`); MK8D's black stretches were the reason for the old wait (1 black lap shot of 14 with it, 3-5
  without, INTERFACES.md). Not run here (not this wave's title); `NXGFX_OCCL_LAG=0` restores the old rule. The lead
  should run `mk8d` once with the default before merging, or the lag default can be limited to OoT's profile.
