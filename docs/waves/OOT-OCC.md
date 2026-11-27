# OOT-OCC — Ocarina of Time's presents no longer wait for the replay to catch up (occlusion results)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~22:10, worktree
`D:\Projects\RecompLauncherEngine-ootocc`, branch `ootocc` (base `ootdesc` `6f7413f`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1 (SET-FAST has MKW on GPU 0). Deadline 4 h.

## Goal and starting point

OOT-DESC (`docs/waves/OOT-DESC.md`): OoT reports samples-passed counters, so every present waits in `aq_wait_room`
until the replay thread has replayed everything queued (`occl_sync`, MK8D's rule); the game's render thread spends
83 % of its time there, and the frame is the game's work plus the replay's, in turn. The frame step then holds 2 or 3
vsyncs (cost median ~1.8 vsyncs). Target 59.

## Plan

1. What OoT asks (diagnosis, no behaviour change): count, on the game's thread at submit, its samples-passed resets
   and reports, the addresses, the draws between a reset and its report, render conditions on report addresses; at
   landing the results (zero share, zero/non-zero changes), how many presents after submission each lands, and
   whether the game rewrites report memory itself (a "pending" marker it would poll). New `vk/occl.rs`.
2. References: how Turbo, nxnative and kirikae answer reports.
3. The non-stalling rule behind a switch (`NXGFX_OCCL_LAG`, 0 = the old wait): presents wait only while one present
   is still queued, so the game builds frame n + 1 while the replay translates frame n; results come from the most
   recent completed frame. Where the game rewrote report memory, the previous answer is put back at submit
   (`NXGFX_OCCL_EARLY`).
4. The frame step with the wait gone: frames at 2 vsyncs or better, or the numbers that show its quantization blocks.
5. Verify: `oot` (route checks, every sheet, objects occlusion would hide or show: flicker, popping, lens flare, the
   sun, lights), `oot-house --faults`, same-build switch-off control back to back; FPS / 5 % low, frame cost in
   vsyncs, replay ms; `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/occl.rs` (new), `vk/queue.rs` (the wait, the submit walk, the landing),
`vk/mod.rs`, `nvn/window.rs` (the present count), `docs/env_switches.tsv`, this doc.
