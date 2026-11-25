# POOL-ORDER — memory-pool finalize in replay order, corrected against its audit

Owner: one delegated agent. Lead: main session. Opened 2026-11-24. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Context.** MK8D's 49 red lines `uniform bank c3 is in no memory pool: zeros bound` (menu to race) come from
`mem::pool_del` (nvnMemoryPoolFinalize) removing a pool from the address map at once on the game thread while replay
ops the game queued earlier still resolve addresses in it. MK-PAIR wrote a correction: an uncommitted 54-line change in
`src/graphics/src/vk/mem.rs` (in the tree now; `git diff`) that defers the removal through `queue::after_queued`
(`pool_del_now`, `take_same` identity check, `NXGFX_POOL_DEL_ORDER=0` off). It removed the 49 lines
(`build/play/20261124-190217-mk8d-after` vs `-185525-mk8d-before`; control `-190521-mk8d-control-order0`). An
independent audit found no use-after-free but three defects — read "Lead review" in `docs/waves/MK-PAIR.md`:
1. Same-base reuse defeats it: `pool_add` inserts the new pool and releases the old one at once, so queued work resolves
   the old pool's addresses to the new buffer.
2. Work in parked queues is not "queued before" (the deferred call can run before a parked queue's appended stream).
3. While a finalized pool stays mapped, an overlapping new pool at a lower base is shadowed for game-thread lookups
   (`find` -> `addr_known` in `nvn/prog.rs`, `room`), and both may be write-watched over the same pages so a CPU write
   can be handed to the old one and the new one left stale.

**Goal / done.** A pool-finalize ordering that is correct for all three cases (design it so replay-time lookups see
the pool as it was when the op was queued, while game-thread lookups and write-watch ownership see the current map —
e.g. a replay-side view of retired pools instead of keeping them in the live map; your design), with CPU tests for
each case (each failing on the current uncommitted change where it applies), then: engine rebuilt, MK8D and MKW
rebuilt, `py scripts/play.py mk8d` showing 0 "in no memory pool" lines and ≥ 59 FPS in its window, and
`py scripts/play.py mkw` still racing with no new red lines; sheets looked at. If time remains: the lazy-keep cost
(MK-PAIR review: `materialize_all` makes images for every kept finalized texture at once; `take`'s fast path lost).

**Baseline:** branch `gameplay-harness`, HEAD `4316ad2` (+ this file) with the uncommitted `vk/mem.rs` change. `dist/nxrs`
= that tree's engine. Ledger: `docs/32-REMAINING-WORK-PLAN.md` "Pool finalize ordering".

**Exclusions:** no skipping draws, blacklisting or weakening robustness; native NDVE off; only MK8D and MKW runs;
no commits.

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol §3
(one build or game at a time, never a build or cargo test while a game runs; `--jobs 4`). Engine:
`pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (background, ~4 min); rebuild each title
after an engine build (`dist\nxrs\nxrs.exe --select-title "<title>" --build`). Run: `py scripts/play.py <scenario>`;
read `report.md`, look at every `sheet-*.png`. Tests: `cargo test --jobs 4 -p graphics --lib`.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min; no early hand-off.

**Files expected:** `src/graphics/src/vk/{mem,queue,image}.rs`, `src/graphics/src/nvn/prog.rs`; this file.

## Report (agent, protocol §5)

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with what is verified working now.)_
