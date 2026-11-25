# Handoff — 2026-11-25 (lead session -> next session)

Read this, then `AGENTS.md`, `docs/31-DELEGATION-PROTOCOL.md` (binding) and the **current** section of
`docs/32-REMAINING-WORK-PLAN.md` (the to-do list). Older sections of 32/33/34 and `docs/waves/*` are history.

## Repo and machine

- Repo `D:\Projects\RecompLauncherEngine`, branch `gameplay-harness`, tracking `origin/main` of
  **https://github.com/ChrisColeTech/nxrs** (public, full history). At handoff: HEAD `a4bb05a`; uncommitted only the
  launcher clippy fixes the lead session is finishing (`src/launcher/src/ui/chrome/caption.rs`,
  `src/launcher/src/model/model_tests.rs`).
- The owner has asked for commits throughout: commit accepted work with clear messages and push to `origin main`
  (`git push origin gameplay-harness:main`). Commit footer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- `dist/nxrs` = the HEAD engine + launcher (contract 2). Build: `pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4
  -SkipGnu -KeepCache` (~4 min). Rebuild a title after an engine change:
  `dist\nxrs\nxrs.exe --select-title "<library title>" --build` (relinks in ~30-60 s).
- **One compiler-heavy command and one game at a time on this machine**, never a build while a game runs. Kill by PID
  only. Run long commands in the foreground (< 10 min) or poll them yourself: **never end a turn waiting on a background
  notification** (an agent lost 4 hours that way).

## Split between sessions (avoid conflicts)

- **Lead session (this one) owns:** CI and the release — `.github/workflows/*`, the launcher clippy fixes, the Linux
  CI test failures (`src/runtime/tests/abi_layout.rs`: x86-only `-march=x86-64-v3` breaks arm64; `os` crate unit tests
  SIGSEGV on x86-64 Linux CI, crashing test not yet named), and the first release `v0.1.0` (`release.yml`, run started
  07:41; README's Install section links to it). It does only short local checks (launcher clippy/tests).
- **Next session owns:** everything in the queue below, the machine's build/game slot, `docs/32` updates. Do not edit
  `.github/workflows/*`, `src/launcher/src/ui/chrome/caption.rs`, `src/launcher/src/model/model_tests.rs`,
  `src/runtime/tests/abi_layout.rs` until the lead session reports CI green. Pull before starting (`git pull`).

## Game status (current engine unless noted)

| Title | State |
|---|---|
| Wonder, Anime Girls, Beach Bounce | play, 60 FPS |
| Mario Kart 8 Deluxe | plays, 59.9 FPS, 0 red lines |
| Pokémon Z-A | plays (fresh save to free movement), 59.6 FPS |
| Donkey Kong Bananza | plays, 59.4 FPS, 0 audio underruns |
| Jamboree | plays, 56 FPS avg / 36.5 5% low; scenario stops in the fresh-save rules tutorial |
| Arceus (1G+1U), Arceus base | native 30 FPS; scenarios stop in the fresh-save opening; 15 s freeze (see engine rows) |
| Mario Kart World (+ "v1.8.0" entry, same package) | races, **28.8 FPS of 60** |
| SONIC X SHADOW GENERATIONS, Smash | not worked on this engine yet |
| Ocarina of Time | blocked: native NDVE stays off (reboot incident) |
| Pokémon Violet | blocked: display reset in the opening |

Caveat: Wonder/Anime Girls/Beach Bounce/DKB/Z-A numbers predate the descriptor-set and scan changes (MKW-PERF,
MKW-PERF2) that touch every title. Owner direction: work broken titles; rebuild/re-verify a passing title only when a
change you make touches it, or when the owner asks.

## Queue (in order)

1. **Arceus base repoint.** Its package was copied (verified identical) to
   `P:\Games\Nintendo\DUMPS\Pokemon Legends Arceus base [01001F5010DFA000][v0]\`. Point the library entry at it and
   rebuild: `dist\nxrs\nxrs.exe --select-title "Pokemon Legends Arceus base" --source "<that .nsp>" --build`; confirm
   the new buildDir and one `py scripts/play.py arceus-base` run. Then tell the owner `D:\Projects\Switch2RECOMP-main`
   can be deleted (owner deletes it). P: is ~96% full (~25 GB free).
2. **CLEANUP-S2R** (`docs/waves/CLEANUP-S2R.md`, brief ready, owner decisions in it): remove Switch2RECOMP/Turbo from
   code and living docs (rename `SWR_*` -> `NXRS_*` with no aliases, drop the `swr_ctl` pipe and the launcher's "turbo"
   engine choice, rename `turbo.toml`; leave dated history docs). Wait until the lead session's launcher edits are
   pushed (it touches `src/launcher`).
3. **RenHDrum phase 1** (`docs/waves/RENHDRUM.md`: verdict integrate, vendored crate, Pro/Joy-Con HD rumble first;
   owner must feel-test with real controllers, the doc lists games/scenes).
4. **MKW performance round 3** (`docs/waves/MKW-PERF2.md` "Unproven"): per-set fixed cost (~2.5 µs: `stage_set` common
   path, 1.5 KB copy per set), write-watch scans ~11 ms (frame's first scan walks every mirror), replay.
5. **Sonic/Shadow and Smash**: rewrite `docs/waves/SONIC-SMASH.md` fix-first (owner: start from the recorded defects
   in `docs/34-LIBRARY-VALIDATION.md` — Shadow's unaligned vpool requests `SXSG-B01`, Smash stutters/memory — change
   code, rebuild only to measure). The first attempt was stopped by the owner for rebuild-first churn.
6. Engine rows in docs/32: no preemption inside long guest loops (Arceus freeze; back-edge checkpoint, needs full
   retranslation), lazy-keep `materialize_all` spike, per-stage descriptor limits check (non-NVIDIA), MKW
   `texture ID 337` (3D read as 2D), single-sample view where SPIR-V declares MS (non-NVIDIA risk), descriptor-patch
   audit limits (MKW-PERF2 review), shelved `POOL-ORDER.patch` (hangs MKW; revisit only with the hang explained).
7. Fresh-save scenarios for Arceus / Jamboree to reach first gameplay.
8. OoT and Violet (blocked; need their own investigation).

## Owner decisions pending

- "Mario Kart World v1.8.0" builds the same base package as "Mario Kart World" (no 1.8.0 update attached/on disk):
  attach a real update or merge the entries.
- Delete the old public repo `ChrisColeTech/nxrs-engine`: the owner chose delete; needs
  `gh auth refresh -h github.com -s delete_repo` run by the owner first.

## How the owner wants work done (from this session)

- Delegation protocol: one fresh agent per wave with a brief in `docs/waves/<ID>.md` (goal/done verified in the game,
  baseline, evidence, exclusions, rules, 2-hour deadline, no early hand-off); the lead reviews, re-runs tests, reads the
  screenshot sheets, and commits. Independent read-only audit (fresh agent) for memory-lifetime, descriptor and
  coherence changes before accepting; fix audit HIGHs before committing.
- **Fix first, rebuild only to measure**; no exploratory rebuild/re-run loops; no re-verifying working titles.
- Report a title's real state (FPS vs 60 target, what's wrong) — never "races/passes" without the number.
- Harness = `scripts/play.py` (gameplay only: no thermal/latch scaffolding). `py scripts/play.py <scenario>`;
  read `build/play/<run>/report.md` and look at `sheet-*.png` yourself. Scenarios in `scripts/scenarios.toml`.
- No skipping draws, blacklisting, frame skipping or weakening robustness to "fix" something; blacklists stay unless the
  exact pipeline is proven safe.
- README is for players: short, screenshots of the library and running games with FPS, no internal detail.
