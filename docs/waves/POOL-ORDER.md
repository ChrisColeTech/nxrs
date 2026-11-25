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

**Verified now:** MK8D is correct with the corrected design: 0 "in no memory pool" lines (47 before the first
attempt), 59.9 FPS mean, 59.0 5% low, 58 min in [95, 175] s, 23/23 steps (`build/play/20261124-195919-mk8d`).
**MKW is not cleared:** three runs tonight all stopped presenting a few frames after a harness screenshot, and the
control run with this change switched off did the same (below).

### Built
The first attempt is replaced (`take_same` and the `after_queued` removal are gone). The change is
`src/graphics/src/vk/{mem,queue}.rs`.
- **Live map vs. replay view.** `pool_del` takes the pool out of `by_base` at once. So does `pool_add` for a pool it
  replaces at the same base. The pool goes to `Pools::retired` with `until = aq_tail()`, and each pool keeps
  `since = aq_tail()` from when it was registered. Lookups on the game's threads (`find`, `addr_known`, `room`,
  `cpu_flushed`) and the write-watch scan in `begin_submit` see only the live map, which fixes defect (3).
- **Lookups during replay.** `find_at` serves `resolve`, `room` and `prepared_span`, and `note_gpu_write` uses it
  too. It sees the pools as they were when the op was queued: live pools with `since < view`, and retired pools with
  `since < view <= until`. If nothing matches, it falls back to the live map and then to the retired pools.
  - The replay thread sets the view per op (`mem::view_scope`, the first seq of a merged run).
  - `resolve`'s 16-entry cache is keyed by view while any pool is retired.
  - This fixes defect (1): an op queued before a same-base reuse gets the old pool.
- **Parked queues.** `Parked` now carries `oldest`, the oldest op whose work it holds. A parked stream replays with
  that view. `mem::reap` releases a retired pool only when
  `until <= queue::replayed_through() = min(done, oldest parked - 1)`. This fixes defect (2).
- **When reap runs.** `reap` runs after every replay op, and on the game thread after `pool_add`/`pool_del`. The game
  thread uses `try_lock`, so it never waits on a replay.
- **CPU writes at retirement.** When a pool is retired, its pages dirty in the write watch are read without resetting
  the watch and kept as its `pending` ranges. `begin_submit` uploads them to the retired buffer. The watch stays with
  the live pools.
- **Write-backs.** GPU writes that resolve to a retired pool get no write-back to guest memory.
- `NXGFX_POOL_DEL_ORDER=0` turns all of this off: pools are released at once, as originally.

### Tests
`cargo test --jobs 4 -p graphics --lib`: 186 passed, 0 failed. It ran before the game runs and again after the last
MKW run. Three new tests are in `vk::mem::pool_del_tests`:
- `same_base_reuse_keeps_the_old_pool_for_the_work_queued_before`: op 5 gets the old pool, op 6 and the game thread
  get the new one. A control in the test (`order` false, the first attempt's `pool_add`) shows op 5 getting the new
  pool.
- `a_parked_queues_work_keeps_a_retired_pool`: a queue parked with op 4's work keeps a pool retired at 5, even with
  `done` at 10. The pool is released once the parked work is newer. The first attempt released it at `done`.
- `a_retired_pool_does_not_shadow_a_new_lower_pool`: the game's `find` gets the new, lower pool, and the retired pool
  is not in the watched map. Old work gets the retired pool. A parked stream's later records still reach a pool
  registered after its view. A control (both pools in the live map) shows the shadowing.

These tests were not run against the first attempt's code. Its behaviour is reproduced inside each test as the
control.

### Verified
- Engine built (`build.ps1 -Jobs 4 -SkipGnu -KeepCache`), then MK8D and MKW rebuilt.
- **MK8D** `20261124-195919-mk8d`:
  - Sheet: title, grid, driving in 1st, lap 2/3 at 161 s and 169 s, rendering normal.
  - The metrics memory column is flat over the window (4719 to 4730 MB), so retired pools do not pile up. The
    report's 4935 MB is its last sample.
  - One red line, `audio: no output device (... Cannot change thread mode after it is set)`. This is environmental,
    not this change: it is a COM apartment-mode error (RPC_E_CHANGED_MODE) when the audio client opens on an OS
    thread, and this change touches only graphics pool bookkeeping.
- **MKW**:
  - `20261125-000131-mkw`: hung at about 250 s, a few presents after the `04-grid` screenshot. The main thread sat in
    `nvnSyncWait`; submits stopped at 20726.
  - Control `20261125-000658-mkw` (`--env NXGFX_POOL_DEL_ORDER=0`): hung even earlier, at about 40 s, after the
    `01-boot` screenshot.
  - Re-run `20261125-000839-mkw`: the sheet shows the map, desert, the Grand Prix menu, the grid and the race
    countdown at 324 s. It hung about 7 presents after the `05-race` screenshot (42/43 steps, 20.9 FPS mean in the
    window).
  - No new red lines apart from the hang's own `graceful exit` line. The `texture ID 337` and `pipeline-crashed`
    lines are known.

### Unproven
- **The MKW hang.** Each hang follows a harness screenshot. It happens with this change switched off (control) and
  did not happen in the baseline `-193353-mkw-lazykeep`.
  - The control still has the parts that do nothing when no pool is retired: the per-op view scope, the `reap` call
    and `Parked.oldest`. So this change is not cleared.
  - Next step: run MKW on HEAD's committed engine (stash `mem.rs`/`queue.rs`, then rebuild). If that hangs too, the
    cause is machine or driver state. If not, bisect the view scope and reap.
- **Parked and merged streams use one view.** A parked stream, or a merged submit run, replays with its oldest op's
  view. A same-base reuse inside one parked stream therefore resolves the newer records to the old pool. It falls back
  correctly for pools at new addresses.
- **Not covered here:** raw global memory (`gmem`) lists only live pools. `tex_release` has the same parked-queue gap
  (from the audit).
- **Lazy pages come back late.** `lazy::touch` at release now runs at reap time, so it can write device-held pages
  over memory the game has reused since. The first attempt had the same timing.
- The lazy-keep cost item was not reached.

### Slips
- I stalled about 4 hours waiting on a background notification and missed the 21:52 deadline. The lead re-scoped
  the work after midnight.
- Two MKW runs were added beyond the one requested: the `NXGFX_POOL_DEL_ORDER=0` control, and a re-run of the same
  path after a hang (a hang, not a device loss).
- No device loss. Nothing is committed. Nothing is left running. Changed files: `vk/mem.rs` and `vk/queue.rs`.

### Questions
None.

## Lead decision (2026-11-25 00:30): shelved

Discriminator run by the lead: HEAD's committed engine (this change removed from the tree), engine + MKW rebuilt,
`build/play/20261125-001723-mkw-head-control`: completed 43/43, normal exit, 17.7 FPS, only the two known red
lines. So the machine is fine and this change causes MKW's hang (3/3 hung with it, including the
`NXGFX_POOL_DEL_ORDER=0` control, whose view/reap/parked-oldest parts stay live; 0/1 without). The change is not
accepted: 340 lines of lifetime machinery that hang a title which races without it, to remove MK8D's transition red
lines (MK8D passes without it: `20261125-002334-mk8d-head`, 59.8 FPS, 23/23, 86 "in no memory pool" lines). The full
diff is kept as `docs/waves/POOL-ORDER.patch` (applies to `77b2f5e`). Revisit only with the hang explained (bisect
the per-op view scope, `reap` after each op, `Parked.oldest`).
