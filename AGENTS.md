# Agent Guide — nxrs (engine and launcher)

`docs/ARCHITECTURE-AND-STYLE-GUIDE.md` holds the rules. The engine's plan and status are `docs/PORT_PLAN.md` and
`docs/INTERFACES.md` ("Current remaining work"), its decisions `docs/DECISIONS.md`; the launcher-engine interface is
`docs/LAUNCHER_CONTRACT.md`; the launcher's plan and parity checklist `src/launcher/PLAN.md`. Read the relevant ones
before changing behaviour or structure. The tree is often dirty: preserve unrelated work, and never reset, revert,
stage or commit another person's changes.

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
cd src/launcher; cargo test --jobs 4                # launcher (seconds: fake engine, kittest, CLI)
cd src/launcher; cargo clippy --all-targets --jobs 4 -- -D warnings; cargo fmt   # max_width 100
scripts/build/build.ps1 -Run       # release build into dist/nxrs, target/ left lean, starts dist/nxrs/nxrs.exe
scripts/build/build.ps1 -KeepCache # keep the release cache: fast rebuilds, large target/
scripts/build/prune.ps1            # just shrink target/ (-BudgetGB, -KeepDays, -Lanes, -WhatIf)
```

`dist/nxrs` is a release laid out as it ships (`nxrs.exe`, `nxrs-engine/`, `legal/`) and runs on its own.
`target/` is large mostly because of `debug/` and the lane folders (`target/L2`, `target/K`, ...) that parallel
work builds into with `CARGO_TARGET_DIR`; `prune.ps1` never touches lanes unless given `-Lanes`. Cargo uses
`--jobs 4` on this machine (heavy parallel builds crash its compilers).

## Rules that bite (full text: docs/ARCHITECTURE-AND-STYLE-GUIDE.md)

- **In `src/launcher`: 200 lines per source file, 250 per test**, measured wrapped at 100 columns; no
  `unwrap`/`expect`/`panic!`/`todo!` in production; `unsafe` only in `services/platform` with a `// SAFETY:`
  line; no `utils`/`helpers`/`misc`/`common` modules; words and sizes in `constants/`. The engine crates do not
  meet these yet; do not refactor engine code to meet them as a side task.
- **Never read or write the owner's `%LOCALAPPDATA%\nxrs`.** Development and tests use `NXRS_DATA_DIR=<temp>` or
  `--library <temp file>`. Never open or drive the owner's running launcher; run your own instance.
- `library.json` keeps the Qt launcher's format exactly, backed up to `library.qt-backup.json` before the first
  write.
- **Never kill by image name** (`taskkill /IM nxrs.exe`): kill by PID.
- Don't run games except an acceptance build-and-play the owner asked for; close it by PID.
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
- Verify the interface visibly: your own instance on a temporary data folder, its windows captured with
  `PrintWindow` (3x scaling here; never a screen copy), closed by PID afterwards.
- Use `py` (with `PYTHONDONTWRITEBYTECODE=1`) for Python. Don't commit unless asked.

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
