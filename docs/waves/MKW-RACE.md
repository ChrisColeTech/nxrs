# MKW-RACE — Mario Kart World: reach a race, its frame rate, the stray slot access

Owner: one delegated agent. Lead: main session. Opened 2026-11-24. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Goal / done (in order; each ends with a game result, not a diagnosis):**
1. **Race.** `py scripts/play.py mkw` reaches a controllable race: the sheet shows Mario racing on a track (HUD:
   position/lap) during the replay steps, and the window in `scripts/scenarios.toml` covers race seconds. Today the
   run lands on the attract loop: the title's "Press L+R" appears at ~141 s, after the scripted L+R at 49 s
   (`build/play/20261124-162441-plain2/sheet-1.png`). Fix the scenario (timing, a wait for the title, the menu
   path to a Grand Prix or free race). Fresh save each run (no save exists).
2. **Stray slot access.** Name the shader that reaches storage slot 33 of set 0 (a vertex shader's set,
   `1ee7ea5a03bdd780`) during task/mesh draws (task `6098504f5d12ea03`, mesh `1de24686294997d8`), and correct
   its binding/translation so it accesses its own resource. With the round-2 fill those writes now land in a sink
   (possibly lost rendering data). Evidence and tools: `docs/waves/MKW-LOSS.md` (round 2 report, the
   `NXGFX_DB_POISON_CANARY=1` diagnostic and the set-0 A/B). Done = canary reports 0 stray writes with
   `NXGFX_DB_FILL=0` in the same reproducer run, plus a CPU test of the correction.
3. **Frame rate in the race.** Measure FPS over the race window (target 60). Round 2 saw 7.1-7.4 in 150-185 s vs
   round 1's 12.5 with pool sync 48 vs 20 ms/frame after ~60 s. Find what costs the frame (frame CSV, `--diag
   render`, the `[vk] slow frame` lines) and correct the largest cost you can prove. Report before/after numbers
   from the same scenario.

**Baseline:** branch `gameplay-harness`, HEAD `780624c` (+ this file's commit), clean tree. `dist/nxrs` = HEAD
engine; MKW rebuilt with it. Ledger rows: `docs/32-REMAINING-WORK-PLAN.md` "mkw follow-ups".

**Exclusions:** keep the fill on by default (it fixed the device loss). No skipping draws, blacklisting or
weakening robustness. Native NDVE stays off. No other titles. No commits (lead commits at acceptance).

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID),
protocol §3 (one build/game at a time, `--jobs 4`). Engine: `pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4
-SkipGnu -KeepCache` (background; ~4 min). Title: `dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build`.
Run: `py scripts/play.py mkw [--env K=V] [--seconds N] [--diag render]`; read `report.md` and look at every
`sheet-*.png`. Tests: `cargo test --jobs 4 -p graphics --lib` (and `-p shaders` if you touch it);
`py -m unittest scripts.test_play` if you touch the harness. A device loss stops that path: fix, never re-run it
unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min and move to the next item; no early hand-off.

**Files expected:** `scripts/scenarios.toml`, `src/shaders/src/*` or `src/graphics/src/vk/{shader,exec}.rs` for (2),
wherever the frame cost lives for (3); this file for the report.

## Report (agent, protocol §5)

**Verified working now (final tree, engine + MKW rebuilt 17:31):** `py scripts/play.py mkw` reaches a Grand Prix race
on Mario Bros. Circuit and drives it (HUD: position, coins, lap 1/3, items) through the replay steps; the poison
canary reports 0 stray writes with `NXGFX_DB_FILL=0`; race-window FPS is **14.1-16.1, was 7.2-9.9** (window
[250, 325] s, same scenario, warm caches).

### Built
1. **Race** (`scripts/scenarios.toml` `[mkw]`). The opening cinematic is unskippable on a fresh save (L+R every 5 s
   from 20 s changed nothing; title at ~138 s, `build/play/20261124-164514-explore1`). A starts at the title too, and A
   every 6 s takes the defaults: Single Player, Grand Prix, Mario, kart, Mushroom Cup 50cc, OK. New steps: wait 40,
   A x16, A x3, A x15, then the two drive replays; 360 s, window [250, 325]. Extra A presses are harmless.
2. **Stray slot** (`vk/layout.rs`). Set 3 named only TESSELLATION_CONTROL|TASK and set 4 TESSELLATION_EVALUATION|MESH.
   The task/mesh draws (task `6098504f5d12ea03`, mesh `1de24686294997d8`, pixel `1e3e40ea0432fbd4`) wrote storage
   binding 33 through set 0. Both modules decorate binding 33 (cp_s1) in their own set, so the driver mapped it to
   the wrong set; the SPIR-V is correct. Correction: `stage_set_flags`, so every graphics stage set names every
   graphics stage, plus TASK|MESH when the device has them (compute keeps COMPUTE). A startup check logs `[RED]` if a
   set's descriptor-buffer size or offsets differ from set 0's; none differ.
3. **Frame rate.** Two costs found with new always-on counters in the `pools:` line (lock wait / scan / per-pool
   pass, syncs by kind, scans skipped):
   - `vk/pipe.rs`: the 18 draws a frame of the pipeline on `pipeline-crashed.bin` re-ran IO linking (SPIR-V parse,
     variant lookups) and the list lookup at every draw: **16-17 ms of `pipeline lookups` a frame**. `CRASHED_SEEN`
     remembers the draw key, and they are skipped at once: 0.27 ms. The blacklist is unchanged and those draws are
     still skipped.
   - `vk/mem.rs` + `vk/queue.rs`: every submission (~10 replays + 1 present a frame) ran a write-watch scan of ~2.8 GB
     of mirrors (~4 ms each, **~47 ms a frame**). `scan_covers`: a replay op or present needs no scan when a scan
     (a full one at a frame's first submission) started after the game queued it (the queue tail read at scan start).
     Parked-queue resumes and one-offs always scan. `NXGFX_SCAN_COVER=0` turns it off. About 2 of 11 are covered
     (pool sync 50.7 to 40.8 ms on the same build).

### Tests
- `layout::tests::graphics_stage_sets_name_every_graphics_stage`: every graphics set has the same mask with all
  graphics stages, TASK|MESH exactly when mesh is on, and compute is separate. It fails on the old per-stage masks.
- `mem::scan_cover_tests::a_scan_after_the_op_was_queued_covers_it`: covered only by a later-started scan, a full
  one for a frame's first submission, never for `None`/0.
- `pipe::crashed_seen_tests::a_crashed_key_is_remembered_and_only_it`.
- `cargo test --jobs 4 -p graphics --lib`: 179 passed, 0 failed.

### Verified
- Race sheets read: `build/play/20261124-165356-race1` (first run, cold pipeline cache), `-170929-before`, `-171642-after`,
  `-173103-after2`, `-173812-after3`: Grand Prix menus, "GO!"/countdown, Mario racing with position/coins/lap 1/3
  and items (Bullet Bill, Golden Mushroom) at 292/335 s. All 43/43 steps, normal exit. With the fixes the race starts at
  ~158 s (was ~232 s).
- Stray slot (`--seconds 70 --env NXGFX_DB_FILL=0 NXGFX_DB_POISON=1 NXGFX_DB_POISON_ONLY=32-64
  NXGFX_DB_POISON_CANARY=1`): `-165945-canary-flags2` 0 strays / 723 frames, `-173656-canary-final` 0 / 998. Round 2's
  identical control was 6 / 554 (`-163058-set0-control`), and `-160009-canary7` 5.
- FPS in [250, 325] s (mean; frame / pool sync / pipeline lookups ms):

  | run | FPS | frame | pool sync | lookups |
  |---|---|---|---|---|
  | `-170929-before` (layout fix only) | 7.2 | 139 | 48.5 | ~17 |
  | `-172234-after-control-cover0` (cover off) | 9.9 | 100 | 50.7 | ~17 |
  | `-171642-after` (cover on) | 10.4 | 96 | 40.8 | ~17 |
  | `-173103-after2` (final) | 16.1 | 61 | 33.7 | 0.27 |
  | `-173812-after3` (final, repeat) | 14.1 | 71 | 41.2 | ~0.3 |

  Run-to-run spread is ~±1.5 FPS (before vs control differ by 2.7 with nearly the same code).

### Unproven
- Which of task/mesh shader makes the access, and why NVIDIA maps it to set 0 (the correction works for either).
- Still 14-16 FPS, not 60. Remaining costs in a 61-71 ms frame: pool sync 34-41 ms (the ~6 parked-queue resumes a
  frame each scan; covering them is only safe if every CPU signal goes through `event_signalled`, which direct
  memory writes to an event word would not). Next are sets (12 ms) and replay (21 ms).
- The layout change invalidates the driver pipeline cache once: the first run recompiles everything (`race1`: 9.4 s
  hitch, 6.4 FPS).

### Slips
- `cargo test` compiled during the `before` run (~150-230 s of it, outside its FPS window): broke "no builds while a
  game runs".
- The layout comment was edited after the last build (comment only).
- `scenarios.toml` had a temporary `mkw-explore` scenario, now removed.
- Superseded screenshots and the cold-cache canary run (`-165241`) were deleted; sheets were kept.
- `pipeline-crashed.bin` is unchanged (237 bytes). No device loss.

### Questions
- None.

## Lead review (2026-11-24 17:58)

Accepted. Lead re-ran graphics lib (179 passed) and harness tests; read `-173812-after3`'s sheet (countdown, racing,
lap 1/3, items). Independent audit (protocol §4, read-only fresh session) of the scan skip and the layout change:
- **HIGH, fixed by the lead:** a GPU event wait whose event the CPU signals after the covering scan already passes at
  replay, so nothing parks and its draws could read data the CPU wrote before signalling without an upload. Fix:
  `mem::note_cpu_signal()` (counter bumped in `nvn::sync::event_publish` before the event word changes); a scan
  records the count at its start and `scan_covers` refuses when it moved. Test extended in
  `scan_cover_tests` (the audit's case). Limit: a game writing an event word directly (not via nvnEventSignal)
  bypasses the counter.
- **LOW, fixed:** a scan with any failed write-watch batch no longer records coverage.
- **Layout:** valid and correct by construction on NVIDIA (identical set layouts, so set 0's offsets hold), but each
  graphics stage now sees ~880 descriptors and no `maxPerStageDescriptor*` / `maxPerStageResources` limit is
  checked: a startup check is a ledger row (matters for non-NVIDIA devices).
Verified after the fix: engine + MKW rebuilt, `build/play/20261124-175216-mkw-audit-fix`: completed 43/43, normal
exit, 14.0 FPS in [250, 325] s (within the agent's 14.1-16.1 spread), sheet read (world map, "1st Race", racing,
coins 5 -> 7). New in race runs (also in the agent's: 369/452 lines): `texture ID 58700: target 1 bound where the
shader samples target 6` (2D bound where a cube is sampled), ~415 lines a run: ledger row.
