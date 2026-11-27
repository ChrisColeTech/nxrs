# MKW-SETS — the lanes' per-draw cost (descriptor sets, locks) and the first part's prep

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~02:30, worktree
`D:\Projects\RecompLauncherEngine-mkwsets`, branch `mkwsets` (base `mkwahead` `08bddb3`: balanced segments, park chain,
SET-FAST step 1, USE-SYNC chunk tracking). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only,
GPU 0 (OOT-REPLAY owns OoT on GPU 1). Deadline 4 h.

## Starting point

MKW-AHEAD: with the pacer forced to 1 vsync (`NXOS_FPS_HOLD=off`) MKW runs 53-54 FPS, the frame op 11.5-15 ms; the
default pacer holds 1 vsync only after 30 frames ≤ 15.8 ms, so it sits at 30-40. Inside the op: A2 / A3 at ~20 µs a
draw on the lanes (~7.5 serial), the first part's prep ~2 ms. SET-FAST: ~700 whole sets a frame at segment start
(~9.3 µs, 95-97 % "no copy of the module": a segment's `QState` starts empty), ~2,300-2,600 patched at ~2.6 µs, half
of the set time unattributed.

## Plan (written before code)

1. **Stack profile** of the replay thread and the lanes over the race (`NXRS_HOST_PROF` with stacks, base build with
   PDB, hold 1): per-draw time split into set build (whole / patched), view lookups, banks, staging, and lock waits
   (which lock, its holder's path).
2. **Cut it**, each behind a switch (0 = old), chosen by step 1's split:
   - segment seeding: a segment starts with the sets its stage shaders had at the end of the segment before it (or
     the frame's last state), so it patches instead of building whole sets; per-submission side effects (staging,
     image preparation, GPU-write notes) redone for the classes that have one;
   - the hot lock(s): per-lane caches / read-mostly structures;
   - the first part's prep, if step 1 shows a cheap lever.
3. **Measure**: default pacer (FPS, 5 % low) and `NXOS_FPS_HOLD=off`; frame op median / p95 (frames CSV replay +
   prep, and the chain trace); same-build switch-off controls back to back. Target op ≤ ~12 ms, p95 < 15.8 ms.
4. **Verify**: `mkw`, `mkw-turn` (route checks, every sheet); `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/exec.rs`, maybe `queue.rs`, `tables.rs`, `image.rs`, `docs/env_switches.tsv`,
this doc. What would disprove it: the profile shows the lanes' time is not in sets or locks (e.g. driver calls, guest
memory reads), or seeded sets still miss (keys differ for another reason).
