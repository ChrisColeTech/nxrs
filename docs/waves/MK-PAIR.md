# MK-PAIR — Mario Kart World v1.8.0 and Mario Kart 8 Deluxe on the current engine

Owner: one delegated agent. Lead: main session. Opened 2026-11-24. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Goal / done (each ends with a game result, sheets looked at):**
1. **MKW v1.8.0** (`[mkw180]`): it was blocked only by the base game's device-loss latch, now fixed
   (`docs/waves/MKW-LOSS.md`, `MKW-RACE.md`). Rebuild it (`dist\nxrs\nxrs.exe --select-title "Mario Kart World v1.8.0"
   --build`), bring `scripts/scenarios.toml [mkw180]` to the base game's working route (`[mkw]`: fresh save,
   unskippable opening, A through Grand Prix, drive replays; adjust if 1.8.0's menus differ) and show a driven race.
   Fix whatever stops it (device loss, red lines, a stall) at its cause.
2. **MK8D** (`[mk8d]`): rebuild ("Mario Kart 8 Deluxe"), run, reach a driven race on a fresh save (adjust the
   scenario), then work its recorded residuals (`docs/34-LIBRARY-VALIDATION.md` MK8D sections: resource diagnostics,
   stutters, memory scope): correct the largest one you can prove, with before/after numbers from the same scenario
   (window FPS, 5% low, worst pacing, red lines).

**Baseline:** branch `gameplay-harness`, HEAD `5bb3572` (+ this file), clean tree. `dist/nxrs` = HEAD engine
(CUBE-TARGET); neither title is rebuilt with it. Ledger: `docs/32-REMAINING-WORK-PLAN.md` current section.

**Exclusions:** no skipping draws, blacklisting or weakening robustness; native NDVE stays off; only these two titles
(MKW base runs only to check a shared change didn't break it); no commits (lead commits at acceptance).

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol §3
(one build or game at a time, never a build or cargo test while a game runs; `--jobs 4`). Engine:
`pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (background, ~4 min); rebuild each title
after an engine build. Run: `py scripts/play.py <scenario> [--env K=V] [--seconds N] [--diag render]`; read
`report.md`, look at every `sheet-*.png`. Tests: `cargo test --jobs 4 -p graphics --lib` (+ `-p shaders`/`-p os` if
touched); `py -m unittest scripts.test_play` if the harness is touched. A device loss stops that path: fix it,
never re-run it unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min and continue with the next item; no early hand-off.

**Files expected:** `scripts/scenarios.toml`; engine files wherever the causes live; this file for the report.

## Report (agent, protocol §5)

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with what is verified working now.)_
