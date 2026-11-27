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
(`nvn::window::VSYNC_NS`), not on the display's refresh, so a 32 Hz Remote Desktop display does not change the pacer's
arithmetic, but the compositor at 32 Hz may delay presents. Every run below names its display.
