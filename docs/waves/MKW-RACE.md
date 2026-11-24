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

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with what is verified working now.)_
