# OOT-COMBO — Ocarina of Time on all of today's engine work combined (toward 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 03:20, worktree
`D:\Projects\RecompLauncherEngine-ootcombo`, branch `ootcombo` (base: the lead's integration merge `15f3606`:
USE-SYNC chunk tracking, SET-FAST dyn-state dedupe, MKW-PIPE park chain, MKW-AHEAD balanced segments + single image
walk, OOT-DESC, OOT-OCC occlusion lag, OOT-REPLAY snapshot keep + pcache try-alloc). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1 (MKW-SETS has MKW on GPU 0). Deadline 4 h
(07:20).

## Plan (written first)

1. **Baseline** the combined build: `oot` (walk [845, 915], house [600, 830]) and `oot-house` ([610, 690]): FPS /
   5 % low per window, replay busy ms (`occlusion, per frame` line), shown-vsync split. Same-build controls
   `NXGFX_USE_SYNC=0` and `NXGFX_TRANSLATE_BALANCE=0` (one `oot` each) to say what each brought to OoT.
2. **Views keyed per ID**: port TRANSLATE-REUSE change 7 (`reuse` `db758d6`, `NXGFX_VIEW_ID_KEY`) onto this base
   (`tables.rs`, `exec.rs`); keep it if the replay busy falls; then a host profile (`NXRS_HOST_PROF`) of the replay
   thread and cut the next biggest cost, behind a switch.
3. **Fault A** (displaced props at 676-679 s of `oot-house`): every-frame shots over [674, 682] with
   `NXGFX_SNAP_KEEP=256` and `=32`. If only 256 shows it, the default goes back to 32.
4. **Memory**: trim the pooled submissions (`queue.rs` `free_subs`) after the forest-load burst, their staging and
   scratch chunks, if straightforward.
5. **Verify**: route checks, every sheet and fault capture; `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/tables.rs`, `exec.rs`, `queue.rs`, `docs/env_switches.tsv`, this document.

## 0. The merged build hung OoT at the forest load (fixed first)

The first three `oot` runs on the integration build (`cb-base-oot` 03:34, `cb-base2-oot` 03:39, `cb-hang-prof`
03:44; all with the new levers off) **hung at Kokiri Forest's first frame** (presents stop at 213-248 s, the harness
kills the game ~45 s later; `report.md`: "hung"). The run log has the cause each time:

```
thread 'vk replay' panicked at src\graphics\src\vk\queue.rs:2529:46: called `Option::unwrap()` on a `None` value
thread 'nvn-parked' panicked at src\graphics\src\vk\queue.rs:2508:33: ... PoisonError
```

`retry_locked` collects the parked queues whose wait passes, then runs them one by one with
`st.parked.remove(&q).unwrap()`. Running one goes through `run_stream_due`, which since MKW-PIPE (park chain,
`NXGFX_PARK_CHAIN`, default on) runs on **other** parked queues whose wait a recorded GPU signal passes
(`chain_parked`, which removes them from `st.parked`). The next `q` of the collected list was then gone: the replay
thread panicked, the sub lock was poisoned (the retry thread panicked on it), and the game waited for the replay
forever. MKW never has two queues ready in one retry at that point; OoT's forest load does.

**Correction** (`queue.rs` `retry_locked`, commit `d19308b`): each collected queue is taken only while it is still
parked and its wait still passes (the chain may also have parked it again at a later wait); otherwise it is skipped
and the outer loop collects again. No switch: the old code is a crash. Every OoT run below is on this fix.

## 1. Runs and what the levers give OoT

Conditions, all runs: GPU 1; MKW-SETS's Mario Kart World on GPU 0 and other waves' builds (rustc at ~200 % CPU)
through most of them; **the owner's session was on Remote Desktop (32 Hz virtual display, Remote Audio) for every
run up to 04:58**; the session moved to the console (60 Hz, 3840x2160) at ~05:01, which ended the running games
(`050052-cb-fa256-house` exited -1 at 46 s, MKW's game at the same moment). Runs from `cb-fa256c` on are on the 60 Hz
console display. Indicative only; the lead's quiet console numbers decide. The owner's rule change (docs/31 §4, `ec16da5`): no switch-off control runs; the
USE_SYNC=0 / TRANSLATE_BALANCE=0 controls of the brief were dropped.

Columns as OOT-REPLAY (FPS / 5 % low from `frames-*.csv`; "busy": the replay thread's busy ms a frame from the
`occlusion, per frame` lines; "shown": share of frames on screen 1 / 2 / 3 / 4+ vsyncs).

| Run (`build/play/`) | build / switches | walk [845, 915] FPS / 5 % low | walk busy | walk shown | house [600, 830] FPS / 5 % low | house busy | house shown | end | route |
|---|---|---|---|---|---|---|---|---|---|
| OOT-REPLAY `021206-or-v2-oot` (ref.) | its final | 28.2 / 20.0 (Link out of view) | 27-30 | 0/91/8/0 | 25.8 / 19.5 | 30-34 | 0/70/30/0 | 935 s | 6/6 |
| OOT-REPLAY `011621-or-v1-prof` (ref.) | v1, sampled, Link walking | 24.3 / 19.1 | 33 | 0/56/44/1 | 26.2 / 19.6 | 31-34 | 0/74/26/0 | 935 s | 6/6 |
| `033411-cb-base-oot`, `033938-cb-base2-oot`, `034447-cb-hang-prof`, `034918-cb-hang-prof2` | merge `15f3606` (+ inert view-ID port), `VIEW_ID_KEY=0` | - | - | - | - | - | - | **hung 213-248 s** (section 0) | - |
| `035702-cb-base3-oot` | hang fix, `VIEW_ID_KEY=0`, `SUB_KEEP=0` (= the merge) | - | - | - | - | - | - | **died 463 s**: `MapPhysicalMemory commit ... failed` (32 GB process cap; staging 861 chunks = 7.1 GB) | - |
| `040502-cb-base4-oot` | hang fix + trim (`SUB_KEEP` 64, lanes 64), `VIEW_ID_KEY=0` | - (died 838 s) | - | - | **26.7 / 19.6** | 24-30 | 0/78/21/0 | **died 838 s**: allocation failure at the 32 GB cap | 5/5 to 834 s |
| `042215-cb-v1b-oot` | all on (`3c78d49`) | - | - | - | invalid: machine stall 550-700 s (frames 235-740 ms with replay 14-17 ms; the harness and every python on the machine were killed at ~613 s) | - | - | killed | - |
| `043451-cb-v1c-oot` | all on | - | - | - | - | - | - | harness killed (as v1b) | - |
| `050159-cb-fa256c-house` (`oot-house`, console 60 Hz) | all on, Fault A capture (`SNAP_KEEP=256`) | - | - | - | **[610, 670]: 28.8 / 27.5** ([672, 684] slowed by the every-present shots) | 25-27 | 0/97/2/1 | completed | 5/5 |
| `051431-cb-fa32-house` (console 60 Hz) | all on, `SNAP_KEEP=32` (Fault A check) | - | - | - | [610, 670]: 13.4 / 4.7 | 57-59 | 0/42/29/29 | time limit (steps behind) | 3/3 |
| `044441-cb-v1d-oot` | all on | **27.4 / 19.8** (to 895 s; Link walking to `24-walk4`, out of view from `25-walk5`) | 22-39 | 0/83/17/1 | 14.9 / 8.7 (the opening ran ~60 s late under machine load: dialogue until ~640 s) | 34-47 | 0/5/80/15 | **died 895 s** (32 GB cap) | 5/5 |

What each lever gives OoT, as far as these runs show (no switch-off controls, per the rule change):

- **USE-SYNC chunk tracking:** OoT's pool sync is still 8.8-11.5 ms a frame in the house (frames CSV `pool_sync_ms`),
  the same as OOT-REPLAY's 9.4-9.6 without it. The pools line (`base4`, 836 s): write-watch scan 2.2 ms, per-pool
  pass 4.5 ms, lock wait 2.0 ms; 1,040 chunks open (29 MB), 3.4 opened / closed a frame, 0 missed. OoT's sync is the
  per-pool pass and the lock (the completion thread's landings: "re-arming 7.76 ms before the pool lock" a frame,
  54 MB a frame of same-byte CPU writes skipped), not the scan; chunk tracking has little to take here.
- **Balanced segments:** the lanes translate ~38 ms a frame together (house), the replay thread waits **1.5 ms a frame**
  for them (OOT-REPLAY: ~4 ms, 12 % of the thread). That is the gain the lane joins could give; it is in.
- **Views keyed per ID** (ported, below): the house replay busy went from 24-30 ms (`base4`, off, RDP) to **25-27**
  (`fa256c`, on, console) and the `prep`-less replay ms from 12.9 to 11.4 over [610, 670]; the two runs differ in
  display too, so this is not a clean A/B. The walk (`v1d`, Link walking) 27.4 / 19.8 against OOT-REPLAY's walking
  24.3 / 19.1.
- **SNAP_KEEP 256** (OOT-REPLAY's) confirmed again: with 32 the house replay busy is 57-59 ms and 12,544 snapshot
  buffers are made in a run (`fa32`), with 256 25-27 ms and 128.

## 2. Changes

1. **Retry of parked queues** (section 0, `queue.rs`, `d19308b`): the forest-load crash. Cherry-picked into
   `gameplay-harness` by the lead (`0765845`).
2. **Texture views keyed per texture ID** (`NXGFX_VIEW_ID_KEY`, default on; 0 = by the tables' generation, as
   before): TRANSLATE-REUSE change 7 (`reuse` `db758d6`) ported onto this base by hand (`tables.rs`: `view_epoch`,
   per-ID `id_ver` slots, `set_generation`; `pool_descriptors_changed` for the pool's own texture / texel-buffer
   descriptor writes; `exec.rs`: `tex_view` memo keyed by `view_version(id)`, stage sets' `image_gen` by
   `set_generation`). The tables' generation still moves at every site it moved before, so everything keyed by it
   (pool snapshots, NDVE captures) is unchanged. OoT moved the generation 40-60 times a frame, each emptying every
   view memo.
3. **Pooled submissions give back their memory** (`NXGFX_SUB_KEEP`, default 64; 0 = as before; `queue.rs`
   `trim_free`): a free submission keeps one 8 MB staging chunk and one 64 MB scratch chunk, and pooled submissions
   were never freed. Past the newest 64 on the replay thread's free list (16 on each translation lane's), the
   others' staging and scratch are freed. At the forest load the log shows `pooled submissions: 2592 MB ... given
   back` (`base4`, lanes at 64) and up to **6.2 GB** a run with the lanes at 16 (`v1d`, `fa256c`); staging held in the
   house 7.1 GB (`base3`) -> 4.9 GB (`base4`) -> **1.1 GB** (`v1d`).
4. `NXGFX_SHOT_SPAN=<from>-<to>` (diagnostic, `shot.rs`): every present between two log-clock seconds is a screenshot.

### Replay-thread profile, final build (house standing, console)

`052624-cb-fin-oot`, host sampler over [700, 715] s, replay thread + lane 1, stacks, PDB (`build/prof/housefin.txt`,
15,901 samples). Shares of the replay thread's wall time (inclusive):

| what | share |
|---|---|
| pool sync and image prep (`mem::begin_submit_ex`), all callers | **30.0 %** |
| - of it waiting for the pool lock (`g.mem`, held by the completion thread's landings and the walker) | 9.1 % |
| - prehash waiting for the hash threads (`upskip::prehash`) | 5.9 % |
| - write-watch take (`watch::take_one`) / watch lock | 2.7 / 1.6 % |
| translation (`exec::record`: `bind_sets` 13.4 = `stage_set` 11.0 + `patch_set` 4.1; `tex_view` 3.5) | 20.0 % |
| presents (`present_now`; 8.5 % of it is their own pool sync) | 13.5 % |
| waiting at lane joins (`par::run` condvar) | **4.5 %** (OOT-REPLAY: 12.3 %) |
| idle in the replay queue | 10.9 % |
| other contended locks (images / tables at `tex_view`, `note_gpu_write_version`, `target_image`) | ~3.5 % |

The next cut is the pool sync, and inside it the pool lock the completion thread holds while it lands write-backs (the
pools line: "completion: pool lock 1.49 ms, whole write-backs landed 2.57"; "re-arming 7.76 ms before the pool
lock" a frame on the completion thread) plus the prehash wait. Not changed in this wave (time; section 6).

## 3. Fault A (displaced props, 676-679 s of `oot-house`)

Every present over log-clock [672, 684] s captured (`NXGFX_SHOT_SPAN`; the harness's shots there are at 674-686 s),
with `--faults` and `NXGFX_FAULT_PER_REASON=40`, final build, console display:

- `SNAP_KEEP=256` (`050159-cb-fa256c-house`): presents 18437-18521, **85 consecutive frames, all correct** (herbs,
  vines, thatch, pots in place; contact sheets `build/span-sheets/k256-1..3.png`); 30 house shots correct.
- `SNAP_KEEP=32` (`051431-cb-fa32-house`): presents 16902-16970, **69 consecutive frames, all correct**
  (`build/span-sheets/k32-1..2.png`).
- Fault captures in both: the nightmare's lightning (2) and the forest load's no-target frames (30), the known set;
  none in the house.

**Verdict: Fault A did not occur with either setting in these runs**, so it is not shown to depend on
`NXGFX_SNAP_KEEP`; the default stays 256 (32 costs the house 2x the replay time). It remains an open, rare fault (one
frame, ~1 a run before; 0 in 154 frames captured here and 0 in 60 house shots).

## 4. Memory: what kills the walk window now

Every full `oot` run on this base died at the 32 GB process cap (`process.rs` `memory_cap`): `base3` at 463 s
(`MapPhysicalMemory commit failed`, staging 7.1 GB), `base4` at 838 s, `v1d` at 895 s (a 14.7 MB screenshot buffer
failed). OOT-REPLAY's runs peaked at 32.4 GB and finished; with the park chain the forest load leaves more pooled
submissions. Change 3 returns 6.2 GB of staging and scratch, but the run still reaches the cap in the walk:
**texture images keep growing in the house standing still** (`v1d`: 3,906 images / 4.0 GB at 509 s, 6,554 / 8.2 GB at
878 s, ~7 more a second), and scratch stays 3.5 GB (54 x 64 MB chunks: one kept per free submission, 144 kept). The
image growth is the next memory item (images made for OoT's ~46 texture objects a frame and not released).
