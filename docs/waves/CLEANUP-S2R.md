# CLEANUP-S2R — remove Switch2RECOMP / Turbo from the code and living docs

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Owner decision (2026-11-25):** nxrs no longer uses Switch2RECOMP. Its fork is called "Turbo" throughout the repo
(`D:\Projects\Switch2RECOMP-main`, branch `turbo`). Neither name should appear in the code or the living docs.
- **Behaviour: remove all, no aliases.** Rename every `SWR_*` environment variable the engine/launcher reads to
  `NXRS_*` (e.g. `SWR_PROD_KEYS`, `SWR_CFLAGS`, `SWR_TICK_HZ`, `SWR_XMON` / `SWR_NO_XMON`, `SWR_MAP_LIMIT`,
  `SWR_MEMORY_CAP_GB`; find them all: `rg -n "SWR_" src scripts`), keeping no old names (if an `NXRS_*` name already
  exists for the same thing, keep that one); drop the second control pipe/socket `swr_ctl_<pid>` (`src/front/src/ctl.rs`);
  drop the launcher's "turbo" engine choice (`src/launcher`, its tests — `tests/cli_verbs.rs` lists `"turbo"`; the
  library format must still read old `library.json` files: an entry saved with `engine: "turbo"` must load as nxrs,
  not fail); rename `src/profiles/games/0100F43008C44000/turbo.toml` (Z-A's 60 FPS patch) to a neutral name and fix
  every reference (README row, profiles loader/build.rs). Rename tests whose names say turbo (`*_as_turbo`, etc.).
- **Text: code + living docs only.** Rewrite source comments that cite Turbo ("as Turbo did", "Turbo's X") so they
  state the behaviour on its own (keep the technical content; drop the attribution — credits live only in the
  README). Living docs: `AGENTS.md`, `README.md`, every crate README, `docs/ARCHITECTURE-AND-STYLE-GUIDE.md`,
  `docs/INTERFACES.md`, `docs/LAUNCHER_CONTRACT.md`, `docs/settings.md`, `docs/31-*.md`, `src/launcher/PLAN.md`, and the
  current sections of `docs/PORT_PLAN.md` / `docs/32-*.md` / `docs/33-*.md`. **Leave dated history alone:**
  `docs/waves/*` (except this file), `docs/AUDIT.md`, `docs/DECISIONS.md`, `docs/34-LIBRARY-VALIDATION.md`, and the
  historical/dated sections of PORT_PLAN/32/33. `PORT_MAP.tsv` / `scripts/port_map.py` (a Turbo comparison tool):
  remove the Turbo column/option. `src/translator/tests/real_packages.rs` names a package under
  `Switch2RECOMP-main`: point it at `P:\Games\Nintendo\DUMPS\Pokemon Legends Arceus base [01001F5010DFA000][v0]\`
  (the lead moved the package there).
- Keep `mkw-recomp`, `nxnative`, `LAS Recomp`, `totk-pc-recomp` and other credited projects as they are; do not
  touch "SwitchRecomp Builds" folder names that come from the user's library data.

**Done:** `rg -i -n "switch2recomp|\bturbo\b|swr_|switchrecomp\.py"` over `src scripts` and the living docs returns
nothing (list any deliberate exception with its reason); `cargo build --release` of the engine and the launcher
(`src/launcher`) succeed; `cargo test --jobs 4` passes for every touched crate, the launcher's `cargo test --jobs 4`
and `cargo clippy --all-targets --jobs 4 -- -D warnings` pass (AGENTS.md: required for launcher work);
`py -m unittest scripts.test_play` passes; one game smoke (`py scripts/play.py smoke`, Wonder, already built) shows
the renamed paths and the control pipe still work. No behaviour change beyond the renames.

**Rules:** `AGENTS.md`: edit with the editor tools, one file at a time — **no scripts or multi-file replace batches**
(`cargo fmt` is the only bulk rewrite); `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID; protocol §3 (one build or
game at a time, `--jobs 4`). Foreground runs with timeouts under 10 minutes, never end a turn waiting on a background
notification. No commits.

**Deadline:** 2 hours wall clock (it is mostly mechanical editing). Park anything ambiguous with a note; no early
hand-off.

## Report (agent, protocol §5)

_(Built / Tests / Verified / Unproven / Slips / Questions.)_
