# OOT-FLASH — two one-frame render faults in Ocarina of Time

Owner: one delegated agent. Lead: main session. Opened 2026-11-26, worktree `D:\Projects\RecompLauncherEngine-ootflash`,
branch `ootflash` (base `upsync2` head `2a31679`). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time
only; GPU 1. Investigation deadline 2 h to a root cause, then fix and verify.

## Plan (agent, before coding)

**Fault B (zero stand-ins at scene changes; the pure red / black frames) first**: it is logged, so it can be traced.
The stand-in line says the slot names a serial with "no image now (released or never made)". Only two places remove
an image (`tex_release_now`, the takeover in `ensure_image`), and a lazily registered texture is made by
`materialize` / `materialize_all` from `image::lazy`. Registration (`reg_tex`) runs on the game thread at call time,
while draws and releases replay later on the replay thread, so the order of "registered / taken / made / finalized /
released / evicted" per serial is the question. Step 1: an opt-in per-serial event trace (`NXGFX_TEX_TRACE=1`,
image.rs) printed with the stand-in line, and "released" told apart from "never made" in `whereabouts`. Step 2: one
`oot-house --faults` run with it; read the traces of the burst. Step 3: fix the cause so the image exists (uploaded
from guest memory) when first sampled; never skip the draw.

**Fault A (white props in Link's house)**: working hypothesis to test, not assume: the props' shader reads a texture
slot (bound or pool-indexed) that holds the zero stand-in for one frame (a lazily registered texture whose slot names
the stand-in until it is made, or a stand-in not logged because the per-ID log is rate-limited to once per 600 frames
and the parallel-gate give-up returns the stand-in silently). A zero normal map normalised gives NaN, which the HDR
chain blooms into white. Test: catch the frame with `--faults` and the trace; check which IDs were stand-ins that
frame (log every stand-in near a capture, not rate-limited, when tracing). If disproved, go down the brief's list
(stale set reuse, inline uniform ordering, pool slot reuse, read-before-write of shadow/bloom inputs).

**Files expected:** `src/graphics/src/vk/image.rs` (lazy registration, materialize, trace), `src/graphics/src/vk/exec.rs`
(`tex_view_uncached`), maybe `vk/tables.rs`; this doc. Not `vk/upskip.rs`, `mem.rs`, `lazy.rs` unless proven there.

**Verification:** `cargo test --release -p graphics`; dense `py scripts/play.py oot-house --faults --env NXGFX_GPU=1`
runs totalling >= 90 house shots with zero faults of either kind; every sheet, report.md and capture read.
