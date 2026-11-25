# Agent Guide — nxrs (engine and launcher)

`docs/ARCHITECTURE-AND-STYLE-GUIDE.md` holds the rules. The engine's plan and status are `docs/PORT_PLAN.md` and
`docs/INTERFACES.md` ("Current remaining work"), its decisions `docs/DECISIONS.md`; the launcher-engine interface is
`docs/LAUNCHER_CONTRACT.md`; the launcher's plan and parity checklist `src/launcher/PLAN.md`. Read the relevant ones
before changing behaviour or structure. The tree is often dirty: preserve unrelated work, and never reset, revert,
stage or commit another person's changes.

**Delegated work follows `docs/31-DELEGATION-PROTOCOL.md` (binding).** Read it before a wave. An agent owns a bug
end to end: it implements, builds, tests and verifies in the game; the lead may implement too. Builds and games may run
concurrently at full parallelism, except that a perf-measurement run (FPS/timing before/after) has the machine to
itself; two GPUs, `NXGFX_GPU=<index>` picks one. Use the normal CLI builds, the real
library and the configured build folders (owner-authorized); no per-run copies of games. Diagnostics, tests and
audits are steps, never milestones: work ends with a source correction and its game result. A device loss stops
that workload and becomes the top-priority bug: fix it and re-run that path with the fix, never unchanged.
Blacklists stay unless a corrected pipeline is proven safe.
Back up and restore saves. Audits are for genuinely dangerous changes, after a fix exists. Keep evidence compact.
The game-outcome roadmap is `docs/33-REFERENCE-DELTA-ROADMAP.md`; `docs/32-REMAINING-WORK-PLAN.md` is the ledger.
The lead commits as work lands (after every accepted wave; checkpoints before branch/worktree work); delegated
agents do not commit. Push when the owner asks.

**Gameplay harness (2026-11-24):** `scripts/play.py` runs one built title through a scenario from
`scripts/scenarios.toml` and writes `build/play/<run>/report.md` plus `sheet-N.png` contact sheets of the
screenshots. Read the sheets yourself; the harness does not judge pictures. It does gameplay troubleshooting only:
no thermal/HWiNFO feeds, supervisors, stop latches, leases or pre-run gates (the owner removed that incident
scaffolding; the thermal/incident wave docs under `docs/waves/` are history). Runs use a copy of the title's save
(`NXOS_SAVE`) unless `--real-save`. Live native NDVE stays off in the engine itself.

```powershell
py scripts/play.py --list                 # scenarios and their library titles
py scripts/play.py wonder                 # run, then read report.md and sheet-*.png
py scripts/play.py dkb --diag pipeline    # with compile/wait logging; --env/--option/--seconds/--exe too
py scripts/play.py report build/play/<run>   # regenerate a report
py -m unittest scripts.test_play          # the harness's own checks (under a second)
```

## Layout

- The engine workspace (root `Cargo.toml`): `src/translator` package input and the ARM64 → C translator;
  `src/runtime` the host runtime; `src/os` the OS services; `src/graphics` NVN on Vulkan; `src/shaders`;
  `src/front` the window, input and presentation; `src/profiles` game profiles; `src/config`; `src/cli` the
  `nxrs-cli` package, whose executable `nxrs-build` is what the launcher runs.
- `src/launcher` the launcher `nxrs` (egui), its own workspace and lock file, building into the root `target/`.
  Layers: `constants` → `model` (pure) → `services` (all I/O) → `flows` (commands, no egui) → `ui` → `cli`.
- `src/oracle` the T-lane oracle, its own workspace, run on demand. `scripts/` engine tools and `scripts/build/`;
  `build/` local tools and evidence (not committed).
- The launcher reaches the engine only through `nxrs-build`'s contract v2, as a child process, never a crate.

## Commands

```powershell
cargo build --release -p nxrs-cli -p front -p runtime   # the engine: nxrs-build + nxfront.lib + nxrt.lib
cargo build --release --manifest-path src/launcher/Cargo.toml   # the launcher: target/release/nxrs.exe
cargo test                                          # engine (seconds; GPU and game tests are opt-in)
cd src/launcher; cargo test                         # launcher (seconds: fake engine, kittest, CLI)
cd src/launcher; cargo clippy --all-targets -- -D warnings; cargo fmt   # max_width 100
scripts/build/build.ps1 -Run       # release build into dist/nxrs, then all of target/ removed; starts it
scripts/build/build.ps1 -KeepCache # keep target/: fast incremental rebuilds
scripts/build/prune.ps1            # shrink target/ without removing it (-BudgetGB, -KeepDays, -Lanes, -WhatIf)
```

`dist/nxrs` is a release laid out as it ships (`nxrs.exe`, `nxrs-engine/`, `legal/`) and runs on its own; the
default build leaves only it. Don't keep stale lane folders (`target/L2`, `target/K`, ...): build in the normal
`target/` and remove a lane when its work is done. Cargo and `build.ps1` use every core (the old `--jobs 4` cap
belonged to the previous hardware's compiler crashes; the machine was replaced).

## Rules that bite (full text: docs/ARCHITECTURE-AND-STYLE-GUIDE.md)

- **In `src/launcher`: 200 lines per source file, 250 per test**, measured wrapped at 100 columns; no
  `unwrap`/`expect`/`panic!`/`todo!` in production; `unsafe` only in `services/platform` with a `// SAFETY:`
  line; no `utils`/`helpers`/`misc`/`common` modules; words and sizes in `constants/`. The engine crates do not
  meet these yet; do not refactor engine code to meet them as a side task.
- `library.json` keeps the Qt launcher's format exactly, backed up to `library.qt-backup.json` before the first
  write.
- **Never kill by image name** (`taskkill /IM nxrs.exe`): kill by PID.
- No fake features: every control works, or it is not shown. No per-file attribution headers (credits: README).
- The interface follows the family grammar: child windows with the shared header and close; rail work windows;
  Groups and Rows with explanations in info-mark tooltips; ghost buttons by default, one filled primary per
  surface, last; one icon button for every close. Match it; don't invent a variant.
- Run clippy and the launcher's tests before saying launcher work is done.

## How to edit this repository

- Change files with the editor tools (Edit for existing, Write for new), one file at a time, so every change is a
  readable diff.
- **Never write a script to write a file** (no Python/Node/shell program that rewrites `.rs`, `.toml` or `.md`; no
  multi-file replace batches). `cargo fmt` is the only bulk rewrite.
- The shell runs things: builds, clippy, tests, the CLI, audits.
- Verify by running the real thing. Keep any default test run seconds-fast; heavy suites are opt-in
  (`NXRS_DOCTOR_EXE` for the real doctor).
- Verify the interface visibly: capture the launcher's windows with `PrintWindow` (3x scaling here; never a
  screen copy) and close it by PID afterwards. `NXRS_DATA_DIR` or `--library` points it at another data folder.
- Use `py` (with `PYTHONDONTWRITEBYTECODE=1`) for Python.

## Command-line tools available on this machine

Prefer these over their slower or plainer equivalents. All are installed system-wide (Chocolatey) except `http`
(a `uv tool` install) and `cargo-sweep` (cargo). Open a new terminal after installing so PATH refreshes.

| Tool | Use it for | Examples |
|---|---|---|
| `rg` (ripgrep) | Searching file contents; the default search tool | `rg -n "Command::new" src/launcher`, `rg -l TODO -g "*.rs"` |
| `fd` | Finding files by name; respects `.gitignore` | `fd -e rs dock`, `fd -t d gnu` |
| `bat` | Reading a file with line numbers | `bat -n src/launcher/src/main.rs`, `bat -r 40:80 file.rs`, `bat -pp file` |
| `eza` | Directory listings, tree views | `eza -la`, `eza -T -L 2 src/launcher/src` |
| `duf` | Disk free overview | `duf`; check before big builds, `target/` is hundreds of GB |
| `cargo sweep` | Old build artifacts (`prune.ps1` runs it) | `cargo sweep --time 14` |
| `http` (HTTPie) | Calling HTTP APIs with readable JSON | `http GET :8080/health` |
| `fzf`, `nvim`, `zoxide` | Interactive, for the human at a terminal; agents don't run them | `fd -e rs \| fzf` |

This machine is Windows; the shell may be PowerShell or Git Bash, so quote paths with spaces and mind the syntax
of whichever is in use.
