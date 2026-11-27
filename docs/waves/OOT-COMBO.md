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
