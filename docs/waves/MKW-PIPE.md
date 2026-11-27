# MKW-PIPE — Mario Kart World's next frame overlapping the replay of the previous one

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~23:00, worktree
`D:\Projects\RecompLauncherEngine-mkwpipe`, branch `mkwpipe` (base `setfast` `70373e2`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0 (OOT-OCC owns OoT on GPU 1). Deadline 4 h.

## Starting point

SET-FAST: in every MKW run the frame = replay thread (replay + prep) + ~13 ms. Turbo runs MKW at 57-59 FPS with a
9.5-11.3 ms translation thread on this machine (REF-ARCH), so its game overlaps translation.

Read before measuring (code): MKW reports no samples-passed counters (no "samples-passed ... in use" line in any MKW
log), so OoT's `occl_sync` wait does not apply to it. The throttles left on the game's side are `aq_wait_room`
(63 ops or 2 presents queued: the same numbers as Turbo's `AQ_N` / `AQ_PRESENTS`), `nvnSyncWait` (publication wait
and the fence), the acquire's `present_wait` of the texture's previous present, and the frame step (`os/fstep.rs`),
which holds 1 or 2 vsyncs: the race frames of `214047-sf-s2-on2-mkw` are 1 vsync (636) or 2 vsyncs (1,763), and the
trials at 1 vsync fail at a cost of 1.13-1.17 vsyncs (18.8-19.5 ms).

## Plan

1. Host sampler (`NXRS_HOST_PROF`, stacks, DLL relinked with a PDB) over a race stretch on `vk replay`, the game's
   render thread (`guest 0x1004`, the acquiring thread), its main thread (`guest 0x1001`) and `nvn-completion`;
   plus a run with the hold kept at 1 vsync (`NXOS_FPS_HOLD=off`) so the frame is not quantized. From them: what the
   game's threads block on each frame, how long, the call site, and whether the replay thread is busy (throughput
   bound) or idle (serialized with the game) while they wait.
2. Remove the serialization found, behind a switch (0 = old), answering what the game needs as late as NVN allows.
3. The frame step with frames overlapping: 60 reachable, or the numbers that show what holds it.
4. Verify: `mkw`, `mkw-turn` (routes, every sheet), same-build switch-off control back to back; FPS / 5 % low, frame,
   replay ms, game-thread wait ms; `cargo test --release` for touched crates.

Expected files: `src/graphics/src/vk/queue.rs`, `src/graphics/src/nvn/{window,sync,queue}.rs`, maybe
`src/os/src/fstep.rs`, `docs/env_switches.tsv`, this doc.
