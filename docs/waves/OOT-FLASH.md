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

## Diagnosis (agent, as it lands)

**Fault B, two causes, both found with `NXGFX_TEX_TRACE=1`** (run `build/play/20261126-102501-of-trace1-oot-house`):

1. *Forest load (~336 s), parallel translation only.* The stand-in IDs (2084, 2089, ... 746) were registered lazily at
   frames 0-792 and never taken: at frame 10144 a pool-indexing draw on one translation lane called
   `materialize_all`, which takes **every** waiting registration off the list at once (3,834) and then makes them for
   ~0.5 s (`image 98 ... image 3597` lines 336.64-337.12 s, then `lazy images: ... made 3834` at 337.13 s). A bound use
   of one of them on another lane meanwhile (`materialize` → `lazy::take`) found it neither waiting nor made, and
   bound the zero stand-in. With `NXGFX_TRANSLATE_THREADS=0` (`080111-af-tt0`) this burst is absent, as expected.
2. *Every ~80 s (454 s, 536 s, 624 s; tt0 run: 441/519/603/689 s), house and forest alike.* Trace of e.g. ser85718:
   registered lazily at ID 16290 (f12100), made for the pool (f12100), then **its image taken over by ser85764**
   (f12101: the game initializes a new texture object over the same render-target storage each frame). The slot still
   names ser85718, which reaches the image through `Images::forward` (old → new). The stand-in line says the serial
   is not forwarded: `forward` and `released` were **cleared whole** when they reached 65,536 entries. OoT makes ~46
   texture objects a frame (serials 85,718 → 156,096 in 82 s), so every ~80 s the clear dropped the forwards of the
   frame in flight: the 5-20 slots of the main scene's targets bound the zero stand-in for one frame (black frame:
   `faults/fault-07-p12102.png`, only particles over black; the red frame of `074311-af` is the same with another
   target).

Corrections (`vk/image.rs`): (1) `lazy::make_lock` / `making`: a batch holds the lock while it makes what it took,
`materialize` of a registration that is neither waiting nor made while a batch runs waits for it, so the image exists
when the use samples it; (2) `Recent<V>`: `forward` and `released` keep their newest 65,536 entries and drop the oldest
one at a time, never the frame in flight.

**Fault A** (white props) is not either of these: it recurred in the fixed run `103850-of-fix1-oot-house` at `23-d20`
(675 s, present 16150) with no stand-in, no anomaly; the fault capture missed it because the white-flash reason had used
its 4 captures on the forest's fog (`NXGFX_FAULT_PER_REASON` added). The white objects are whole meshes (hanging
herbs, pots, baskets, vines; in `064040-hd/16-d13` the same props displaced), so per-object data (transforms / material
constants) wrong for one frame is the working hypothesis; texture stand-ins are ruled out by the trace (none logged).

**Fault A, measured.** With `NXGFX_FAULT_PER_REASON=40` the white-flash check catches it on its own: run
`110401-of-fix2-oot-house` has 6 one-frame white flashes in the house (presents 14660, 15188, 15296, 15840, 16128,
16698; every one "100% of the white area gone again" at the next present), all the same props, Link standing still.
Nothing else is logged near them. Ruled out by reading: inline uniform updates (ordered GPU copies), compute → draw
ordering (a full barrier after every dispatch), the pool descriptor snapshots (recycled after use, refreshed with
every edit since their generation), the texture view memo (keyed by the table generation).

**Working hypothesis: the cold-pool rule (`mem.rs` `begin_submit`).** A pool with no CPU write for 60 frames is
asked only at a frame's first sync; "a write to a cold pool reaches the GPU at the next frame's first submission". OoT
makes 11 submissions a frame, so a CPU write into a cold pool after the frame's first sync is not uploaded before the
later submissions of that frame: their draws read the pool's older bytes (per-object transforms / constants: displaced
or white props). A/B `114437-of-ring-cold0-oot-house` (`NXGFX_POOL_COLD=0`): **INVALID** — UPSYNC2's `114517-s2-oot2` ran at the
same time from 11:45 (two OoT copies; see Provenance), which also explains its 13 FPS; its numbers and its 0 flashes
count for nothing.

**Correction (built, `63a439b`; not yet run: the lead held OoT runs).** `mem.rs` `is_cold` / `cold_rule`: a pool found
written after a quiet stretch (more than 60 frames without a CPU write) gets `Pool::sporadic` and is never cold
again: every sync asks it, not only a frame's first. Pools that stay unwritten after their load keep the cheap rule
(DKB's ~1,300 never-written mirrors). The same rule decides the walker's warm walks and `sync_range`.
`NXGFX_POOL_STICKY=0` restores the old rule. A diagnostic line logs each cold pool found written (pool, frame,
first-sync or later sync, bytes; first 400). With the fix on, "a later sync of the frame" lines are writes the old
rule left for the next frame's first sync. Residual by construction: the **first** write after a pool's first quiet
stretch is still found only at the next frame's first sync (one frame per such pool). Test:
`cold_rule_tests::a_pool_written_after_going_quiet_is_never_cold_again`.

**Provenance of the OoT runs (lead's check).** UPSYNC2 relinks OoT into the same library folder from its own dist
(without rewriting the build log); my relinks write the build log. Runs and the build they used:

| Run | relink before it | build | valid |
|---|---|---|---|
| `102501-of-trace1-oot-house` | mine 10:23-10:24 (UPSYNC2's last run ended 10:17) | ootflash (trace) | yes |
| `103850-of-fix1-oot-house` | mine 10:37:34-10:38:22; UPSYNC2 relinked next at 10:50:51, after it ended | ootflash (B fixes) | yes |
| `110401-of-fix2-oot-house` | mine 11:03:27 (UPSYNC2's `105129` ended 11:03:07) | ootflash (B fixes) | yes |
| `114437-of-ring-cold0-oot-house` | mine 11:41 | ootflash, but a second OoT copy (`s2-oot2`) ran from 11:45 | **no** |

UPSYNC2's `114517-s2-oot2` logged no stand-in line in 935 s (its build logs a burst every ~80 s), so it ran on the
ootflash 11:41 build. My relink at 12:00:30-12:00:59 (sticky fix) left OoT on the ootflash build with no run started;
the lead was told.

**Verification:** `cargo test --release -p graphics`; dense `py scripts/play.py oot-house --faults --env NXGFX_GPU=1`
runs totalling >= 90 house shots with zero faults of either kind; every sheet, report.md and capture read.

## Report (agent, protocol §5)

Branch `ootflash`: `1fd1ac6` (Fault B fixes, trace), `d0730f3`, `c4bcbb9` (diagnostics), `63a439b` + `66d0ecb` (cold-pool
rule, doc), then `gameplay-harness` (`6453c76`, UPSYNC2 merged) merged in cleanly. All verification runs below are on
that merge, relinked once at 12:22:14 (`_code.dll` 337,867,264 bytes, build log 12:21:54) and checked before each run.

### 1. Built

**Fault B: fixed.** Diagnosis (above, with `NXGFX_TEX_TRACE=1`) found two causes, both in `vk/image.rs`:
- *Lazy-image batch race.* `materialize_all` takes every waiting registration off `image::lazy` at once and makes
  them one by one (3,834 at OoT's forest load, ~0.5 s). A bound use on another translation lane meanwhile found its
  registration neither waiting nor made, and bound the zero stand-in. Correction: `lazy::make_lock` / `making`. A
  batch holds the lock until everything it took is made. `materialize` of an ID that is not waiting while a batch
  runs takes the lock, so the image exists (made and uploaded from guest memory by `reg_tex_now`) before the use
  samples it. No draw is skipped.
- *Takeover forwards cleared whole.* A slot that names an object whose image a newer object over the same storage took
  over reaches the image through `Images::forward`. That map (and `released`) was cleared in full at 65,536 entries.
  OoT makes ~46 objects a frame, so about every 80 s the clear dropped the in-flight frame's forwards, and 5-20 slots
  of the scene's targets bound the stand-in (the black and red frames). Correction: `Recent<V>` keeps the newest
  65,536 entries and drops the oldest one at a time.

**Fault A: not fixed; root cause not proven.** What was established:
- It is one frame and a fixed set of props (hanging herbs, baskets, pots, vines; the same props displaced in
  `064040-hd/16-d13`). Nothing in the engine log is near it. The white-flash check catches it on its own once its
  per-reason cap is raised (`NXGFX_FAULT_PER_REASON`, new): 6 flashes in the house in pre-merge run `110401-of-fix2`.
- Ruled out by reading: zero stand-ins (the trace logs none near it), inline uniform updates (ordered GPU copies),
  compute→draw ordering (full barrier after each dispatch), pool descriptor snapshots, the texture view memo.
- Hypothesis tested: the cold-pool rule in `mem.rs` `begin_submit`. Pools quiet for 60 frames are asked only at a
  frame's first sync, so a later submission of the same frame reads older bytes. The new log shows OoT writes quiet
  pools often: 249 of the first 400 such writes in `122243-of-v1` were taken by a later sync of their frame, and a run
  of 1 MB pools (0x171393000 + k·0x111000) is written again after quiet stretches ~32 times a run. Correction kept
  (`63a439b`, default on, `NXGFX_POOL_STICKY=0` off): a pool found written after a quiet stretch is never cold again.
  This closes a documented coherence gap for repeat writers, but **it is not shown to remove Fault A**. On the merged
  build the flash rate is ~1 a run with the fix on (0, 1) and with it off (1), against 6 before the merge. The drop
  came with the UPSYNC2 merge, not with this change.
- Diagnostics added: `NXGFX_TEX_TRACE`, `NXGFX_DRAW_RING` (stage sets around a capture: `fault-NN-draws.txt`),
  `NXGFX_FAULT_PER_REASON`, the "cold pool written" line (first 400).

### 2. Tests (`cargo test --release -p graphics`: lib 261 passed, 6 ignored; other targets pass, on the merged tree)

- `image::recent_tests::a_full_map_drops_only_its_oldest_entries`: a full map keeps its newest entries; re-insert and
  remove behave. Fails on the old code by construction (cleared whole at the cap).
- `image::lazy::tests::a_use_waits_for_a_batch_that_took_its_registration`: a registration taken by a batch reads as
  "being made", and the make lock is granted only after the batch finished. Fails by construction before (no lock).
- `mem::cold_rule_tests::a_pool_written_after_going_quiet_is_never_cold_again`: the 60-frame rule, the sticky mark,
  and the switch.

### 3. Verified (GPU 1; OoT alone on the title, DLL checked before each run)

| Run (`build/play/`) | build | route | house shots wrong | white flashes in the house (detector) | Fault B bursts | window FPS / 5% low |
|---|---|---|---|---|---|---|
| `102501-of-trace1-oot-house` | B fixes not yet (trace only) | 5/5 | 0 / 32 | (capped) | 4 bursts (336, 454, 536, 624 s) | 18.6 / 16.5 |
| `103850-of-fix1-oot-house` | B fixes | 4/5 (`23-d20` = the fault) | **1 / 32** (`23-d20`) | (capped) | 0 | invalid (off route) |
| `110401-of-fix2-oot-house` | B fixes | 5/5 | 0 / 32 | **6** (589-694 s) | 0 | 19.3 / 18.0 |
| `122243-of-v1-oot-house` | merged, sticky on | 5/5 | 0 / 32 | **0** | 0 | 15.8 / 10.0 |
| `123451-of-v2-oot-house` | merged, sticky on | 5/5 | 0 / 32 | **1** (660 s) | 0 | 14.9 / 7.0 |
| `124706-of-sticky0-oot-house` | merged, sticky off (control) | 5/5 | 0 / 32 | **1** (598 s) | 0 | 12.2 / 1.0 |
| `114437-of-ring-cold0-oot-house` | — | — | — | — | — | **invalid** (two OoT copies) |

- **Fault B:** no stand-in burst and no black or red stand-in frame in any run with the fixes (5 runs, ~160 house
  shots and every frame of the scene changes checked by the detector). The only stand-in line left is the known
  `texture ID 0 ... nothing is registered there` once at the forest load.
- **Fault A:** still present, ~1 a run on the merged build. 160 house shots are 159 correct (`fix1/23-d20` wrong).
- Sheets read: all three sheets of trace1, fix1, fix2, v1, v2 and the sticky-off control. Captures read: trace1 05/06/07, fix2 18/22, v1 71,
  v2 71, sticky0 74. The black captures in the house window are the game's own dark cutscene (Link waking up).
- FPS: indicative only. The merged build's window is lower (15-12 vs 19); the sticky-off control is as slow, so this
  is not the sticky rule. Write-watch scan 9-13 ms vs 6.6 ms; lock waits up to 6.8 ms (other agents' games and builds
  ran meanwhile).

### 4. Unproven

- Fault A's cause. Next: (1) reproduce on the merged build with `NXGFX_DRAW_RING=1` and analyze the dump of a caught
  flash (`fault-NN-draws.txt`, compare the white frame's sets with its neighbours'); (2) A/B the UPSYNC2 lazy
  write-back paths (`NXGFX_WB_LAZY=0`), since the rate fell with that merge; (3) at ~1 a run, an A/B needs ~5 runs a
  side.
- The sticky rule's cost on titles with many mirrors (DKB): not run (OoT only). The residual it leaves (first write
  after a pool's first quiet stretch) is not covered.
- The fix's effect on the lazy race with translation threads at 0: not run separately (that burst never occurred with
  tt0 anyway).

### 5. Slips

- One stray Windows PowerShell 5 call in a background command (hangs on this machine). It was stopped at once and
  left no process.
- My relink at 11:41 was followed by UPSYNC2's run on my build, overlapping my cold-off run (two OoT copies); my relink
  at 12:00 left OoT on my build without a run. Both reported to the lead; the overlapped runs are marked invalid.
- Several of my early title-free checks used `tasklist | grep ocarina`, which misses the truncated image name.
- The investigation went past the 2-hour deadline (Fault A's tests waited on the shared title).
- `mem.rs` edited (lead allowed it once the cause was tested there; it is not proven there).

### 6. Questions

- Keep `NXGFX_POOL_STICKY` on by default? It closes a real gap (later-sync writes into quiet pools are taken in their
  own frame), costs nothing measurable in OoT, but is not shown to fix Fault A. Recommendation: keep it on, and have
  DIRECT-MEM's rework decide the cold rule.
