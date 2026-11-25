# SONIC-SMASH — SONIC X SHADOW GENERATIONS and Super Smash Bros. Ultimate on the current engine

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Goal / done (each ends with a game result, sheets looked at):**
1. **Where they stand.** Rebuild "SONIC X SHADOW GENERATIONS" and "Super Smash Bros Ultimate"
   (`dist\nxrs\nxrs.exe --select-title "<title>" --build`) and run `py scripts/play.py shadow`, `sonic` and `smash`.
   Adjust `scripts/scenarios.toml` so each reaches controllable gameplay on a fresh save (no save exists for any
   title; fresh saves ask for names: the harness answers with `NXOS_SWKBD=auto`). Sonic Generations is a successor
   process (program 1); the harness follows it.
2. **Their recorded blockers** (`docs/34-LIBRARY-VALIDATION.md`: Sonic/Shadow `SXSG-B01`, Smash runtime): several were
   "2D placeholder sampled as a cube gets a zero view" (Sonic ID 519; Smash's 16x16 fallback) — the cube twin landed
   since (`docs/waves/CUBE-TARGET.md`, `MK-PAIR.md`): check them first. Then correct the largest remaining proven
   defect per title at its cause: Shadow's unaligned vpool mapping requests (`SXSG-B01`: offsets 1..0x15 with
   64-256 KiB sizes; NVN documents 4 KiB alignment and the MIT reference refuses them — decide from evidence what the
   offsets mean, do not round blindly), Smash's pacing (58.4 FPS, ~53 stutters; check whether stutters line up with
   the harness's own screenshots as MK8D's did) and memory over the match. Before/after numbers from the same
   scenario (window FPS, 5% low, worst pacing, red lines).

**Baseline:** branch `gameplay-harness`, HEAD `7174ef2` (+ this file), clean tree. `dist/nxrs` = HEAD engine. Neither
title is rebuilt with it. Ledger: `docs/32-REMAINING-WORK-PLAN.md` current section.

**Exclusions:** blacklisted pipelines stay (remove an entry only by proving that exact pipeline safe, protocol §3);
no skipping draws or weakening robustness; native NDVE off; only these two titles; no commits.

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol §3
(one build or game at a time, never a build or cargo test while a game runs; `--jobs 4`). **Run builds and games in
the foreground with a timeout under 10 minutes (title builds and scenario runs fit), or, if something must run in the
background, check on it yourself with bounded foreground polls: never end a turn waiting for a background
notification** (the previous wave stalled 4 hours that way). Engine: `pwsh -NoProfile -File scripts/build/build.ps1
-Jobs 4 -SkipGnu -KeepCache` (~4 min); rebuild each title after an engine build. Run: `py scripts/play.py <scenario>`;
read `report.md`, look at every `sheet-*.png`. Tests: `cargo test --jobs 4 -p graphics --lib` (+ `-p os` if touched).
A device loss stops that path: fix it, never re-run it unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min; no early hand-off.

**Files expected:** `scripts/scenarios.toml`; engine files wherever the causes live (`src/graphics/src/nvn/vpool.rs`,
`src/os/*` for Shadow's mapping); this file for the report.

## Report (agent, protocol §5)

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with what is verified working now.)_
