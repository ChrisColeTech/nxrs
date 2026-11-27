# OOT-REPLAY — Ocarina of Time: cut the replay thread's own work (toward 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 00:41, worktree
`D:\Projects\RecompLauncherEngine-ootbox`, branch `ootbox` (base `ootocc` `c32f047`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1 (MKW-AHEAD has MKW on GPU 0). Deadline 4 h.

## Goal and starting point

OOT-OCC removed the presents' wait for the replay (`NXGFX_OCCL_LAG`). The frame is now the replay thread's own work:
busy 31-35 ms a frame in Link's house standing (28.4 FPS over [600, 830]), 45-67 ms on the walk [845, 915] (~16 FPS).
59 needs under ~15.8 ms. OoT makes 1,300-2,140 occlusion box tests a frame (reset, one box draw, report each).

## Plan (written first)

1. **Profile** (no behaviour change): host sampler (`NXRS_HOST_PROF`, stacks, PDB) of the replay thread and the
   lanes over the walk [855, 880] s of an `oot` run on the base build; the house from a second sample. Split the busy
   time by function; name the top costs with numbers (below, before any code).
2. **Cut the biggest**, behind switches (0 = old): the box tests if they show (state churn per test, per-draw set
   and dynamic-state work of the box draws), and whatever else the profile puts on top. MKW-AHEAD's translate-ahead
   (lanes) is not duplicated here.
3. **The 391 MB allocation failure** (`pcache::save_now`): why one allocation, and why OoT holds ~32.5 GB; fix if
   small.
4. **Verify**: `oot` and `oot-house --faults`, route checks, every sheet; same-build switch-off controls back to back;
   FPS / 5 % low per window, replay busy ms, shown-vsync split; `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/exec.rs`, `tables.rs`, `pcache.rs`, `docs/env_switches.tsv`, this document.
