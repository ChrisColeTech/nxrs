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
| `044441-cb-v1d-oot` | all on | **27.4 / 19.8** (to 895 s; Link walking to `24-walk4`, out of view from `25-walk5`) | 22-39 | 0/83/17/1 | 14.9 / 8.7 (the opening ran ~60 s late under machine load: dialogue until ~640 s) | 34-47 | 0/5/80/15 | **died 895 s** (32 GB cap) | 5/5 |
