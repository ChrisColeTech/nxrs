# MKW-SPIKE — the frames that miss the vsync: what is in them, and cutting their causes

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~07:00, worktree
`D:\Projects\RecompLauncherEngine-mkwspike`, branch `mkwspike` (base `gameplay-harness` `d3e4fc2`: every MKW wave of the
day merged). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0 (OOT-MEM owns OoT on GPU
1). Deadline 4 h (~11:00).

## Starting point

The lead's quiet-machine run on the merged build, `build/play/20261127-065536-mkw` (main checkout; console 60 Hz,
nothing else running): **42.7 FPS mean, 5 % low 29.4**, route 3/3 on. The frame is the replay of the game's per-frame
submit plus the pacer's whole-vsync hold (MKW-SYNC); 60 needs the replay op (replay + prep in the frames CSV) under
~15.8 ms on nearly every frame.

## Plan (written before code)

1. **Spike anatomy** from the baseline's frames CSV over [250, 325] s, then a profiled baseline run of this build
   (`NXRS_HOST_PROF` with stacks on `vk replay`, `nvn-completion`, `vk pool walker` over the race's slow stretches):
   which frames miss 15.8 ms, and what is in them (translation, texture uploads, the pool sync's lock wait or scan,
   pipeline compiles, the present).
2. **Pool-lock holders**: the completion thread's and the walker's samples under the pool lock while the replay
   thread's syncs wait.
3. **Cut the spikes' causes**, each behind a switch (`0` = the old path, rows in `docs/env_switches.tsv`).
4. **Verify**: `mkw`, `mkw-turn` on GPU 0, route checks, every sheet; `cargo test --release -p graphics` (and every
   other crate touched). The final run is compared with the baseline and its diagnostics (no switch-off controls).

Expected files: `src/graphics/src/vk/image.rs` (texture uploads), `mem.rs` / `queue.rs` (by the profile),
`docs/env_switches.tsv`, this doc. What would disprove a cause: the frames it names are not the frames over 15.8 ms,
or cutting it leaves the op's p95 and the 5 % low where they were.
