# MKW-RECORD — record-time command translation (Mario Kart World to 60 FPS)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md` (dist lock / machine lock; never two copies of one title). Title:
"Mario Kart World" (scenario `mkw`, GPU 0).

## Brief (lead)

**Problem (recorded, `docs/waves/MKW-PERF5.md` report, `build/play/20261125-101138-mkw-perf5-final`).** Race at
28.9-29.6 FPS against 60. The Vulkan replay thread does ~28 ms of serial work per frame: ~15 ms draw translation
(NVN command buffer -> Vulkan), ~7 ms write-watch scans, ~2.4 ms passes. Tuning cannot reach 16.7 ms; the
translation must move off the replay thread: translate each NVN command buffer when the game records/submits it, on
the game's threads (or a worker pool), so the replay thread only submits prepared work.

**Phase 1 — design (now, in parallel with wave OOT-NAN).** OOT-NAN is editing `vk/exec.rs`, `vk/image.rs`,
`vk/storemu.rs` and rebuilding `dist/`; the tree is uncommitted, so no worktree. Until the lead tells you OOT-NAN
has finished: **read and measure only — edit no engine source, do not build or relink, run no games.** Write the
design into this file under "## Design": the current call path (record -> submit -> replay -> Vulkan) with
file:line; which translation work depends on state only known at replay (memory contents, texture registration,
pipeline state, pool sync, write-watch) and how each is handled at record time (snapshot, deferred patch, or kept
on the replay thread); threading and ordering rules (queue order, fences, semaphores, cross-buffer state);
correctness risks (stale descriptors, memory written after record, coherence with lazy write-back); the expected
per-frame budget after the change; and an incremental plan where every step builds, runs MKW correctly, and can be
measured. Also how the write-watch ~7 ms is cut.

**Phase 2 — implementation (after the lead's go).** Step by step per the plan, each step built under the dist
lock, MKW relinked and run (`py scripts/play.py mkw --env NXGFX_GPU=0`), sheet read; perf-measurement runs under the
machine lock with nothing else running. Regression on one other title at the end (`smash` or `wonder`).

**Done:** MKW race at 60 FPS (5% low reported) with a correct sheet, red lines unchanged (known: 1), no device
loss; or, parked at the deadline, the measured gain per step and what remains.

**This changes descriptor/memory/coherence timing: the lead runs an independent audit.** Never skip draws; don't
weaken robustness; native NDVE stays off; keep `fps_hold`. **Exclusions:** no commits, don't edit docs/32, other
titles' routes.

**Deadline:** phase 1, 3 hours; phase 2 deadline set at the go. Report per protocol §5 under "## Report".
