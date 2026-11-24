# Architecture and style guide

The rules for this repository: how it is put together, how code is written, and how the launcher's interface looks
and behaves. The launcher follows the Giga family's rules (`D:\Projects\giga-cut-rust\docs\01-ARCHITECTURE-AND-STYLE-GUIDE.md`,
`24-POPUP-VISUAL-REAUDIT.md`, `28-CHILD-WINDOW-PATTERN.md`); where this file and the family disagree, this file says
why. `AGENTS.md` is the short version.

## 1. Architecture

Two Cargo workspaces, one `target/`:

| Workspace | What | Edition |
|---|---|---|
| root (`Cargo.toml`) | the engine: `translator`, `runtime`, `graphics`, `shaders`, `os`, `front`, `profiles`, `config`, `nxrs-cli` (`src/cli`, the `nxrs-build` executable) | 2021 |
| `src/launcher` | `nxrs`, the launcher (egui); its own `Cargo.lock`; builds into the root `target/` (`src/launcher/.cargo/config.toml`) | 2024 |
| `src/oracle` | the T-lane oracle (dynarmic, C++), run on demand | 2021 |

`target/release` holds `nxrs.exe` (the launcher) and `nxrs-build.exe` with `nxfront.lib` and `nxrt.lib` (the engine).
No engine binary is named `nxrs`. Lane folders (`target/L2`, `target/K`, ...) are other work's `CARGO_TARGET_DIR`s.

**The one boundary.** The launcher runs the engine only as a child process, through `nxrs-build`'s launcher
contract v2 (`docs/LAUNCHER_CONTRACT.md`): `build`, `update-info`, `dlc`, `doctor`, `contract`. It never links an
engine crate. It finds `nxrs-build` in this order: `NXRS_BUILD`, the one chosen in Settings, `nxrs-engine/` beside
the launcher (a release), `nxrs-build` beside the launcher (a development build in `target/release`).

**The launcher's layers** (`src/launcher/src`), each depending only on the ones above it:

| Layer | Holds | Never |
|---|---|---|
| `constants/` | every size, interval, name and sentence (`words_*.rs`), the pane table, the theme tokens | logic |
| `model/` | pure types and rules: the game, the library and its JSON, card state, filters, names, the engine's output lines | I/O, egui |
| `services/` | everything that touches the disk, processes or the OS: the library file, markers, keys, jobs, `platform/` | egui |
| `flows/` | the command layer: `App`, its `Action`s and surfaces (Draft windows), builds, play, DLC, updates, settings | egui |
| `ui/` | egui: the shell, chrome, library views, windows, popups, widgets, theme | I/O except through `flows` |
| `cli/` | the flags (the Qt launcher's names, values and exit codes), headless runs, the window | |

`main.rs` only attaches the console, installs the panic report and runs `cli`.

## 2. Code rules

Enforced in `src/launcher` (CI: `.github/workflows/launcher.yml`). The engine crates predate them and do not meet
them today (about 190 files over 250 lines, `unwrap` common, `unsafe` needed throughout the runtime and graphics);
bringing the engine to them is an owner decision, never a side task of other work.

- **200 lines per source file, 250 per test file**, counted wrapped at 100 columns. Split by concern, not by count.
- `rustfmt` `max_width = 100`; `cargo clippy --all-targets -- -D warnings` clean.
- **No `unwrap`, `expect`, `panic!`, `todo!` in production code.** Tests may (`clippy.toml`).
- **`unsafe` only in `services/platform`** (Win32: the caption, job objects, process lookup, the message box), each
  block with a `// SAFETY:` line.
- No `utils`, `helpers`, `misc` or `common` modules (the test harness's `tests/common` is cargo's convention).
  `snake_case` file names.
- Words and sizes live in `constants/`; code never spells a user-facing sentence inline.
- **No fake features.** Every control is wired to real behaviour, or it is not shown. No stubs.
- No per-file copyright, SPDX or "ported from" headers: credits live in the README's Credits table and `LICENSES/`.
- A failure the user can see says why: errors in the semantic red with the reason; a crash or a window that cannot
  open shows a message box (`platform::fatal`) and writes `%TEMP%\nxrs-startup.log`.

## 3. Data rules

- The library is `%LOCALAPPDATA%\nxrs\library.json` in the Qt launcher's format, read and written unchanged
  (unknown keys kept, sorted keys, 4-space indent). Before the first write it is copied to `library.qt-backup.json`.
  The launcher's own settings go under the `consolePanel` key the Qt launcher ignores.
- On first start an older data folder (`RecompLauncher/RecompLauncher`, then `RecompLauncher/SwitchRecompLauncher`)
  is copied in once; the old folder is left as it was.
- `NXRS_DATA_DIR` (the data folder) and `--library` (the library file) point the launcher elsewhere; the tests use
  temporary ones. The launcher checks that keys exist; it never reads them.

## 4. Processes

- An engine run starts suspended in a job object, so Cancel ends it and everything it started.
- Games start detached; a running game is found by its executable's full path.
- **Never kill by image name** (`taskkill /IM`): kill by PID.

## 5. The interface (the family grammar)

**Windows.** A dialog is a child window (its own OS window, modal over a scrim) with the shared header: the title
and a trailing close button named `Close <title>`. Embedded in tests and `--screenshot`. Escape and the close button
discard a Draft. Sizes come from `constants/child_window.rs`.

**Work windows** (Settings, Properties, Add game) have a **rail** down the left (150 px; 32 px entries with an icon
and a label; the chosen one has the selection tint, a 2 px accent bar and accent ink) and the chosen section's form
scrolling beside it, above the footer (`ui/windows/rail_window.rs`).

**Forms use the page grammar** (`ui/widgets/page.rs`):

- a **Group** is a heading: a 32 px accent-tinted icon tile, a bold 16 pt title, an info mark;
- a **Row** is a label in the 190 px column with its info mark, and the control after it; a label that does not fit
  stacks above its control;
- **explanations live in the info marks' tooltips**, never as grey hint text under controls;
- an accent-cored hairline closes a Group; read-only values are boxed, selectable readouts;
- a list's *state* (empty, locked while a game runs, an update waiting for a build) is a status line under it.

**Buttons** (family: "Buttons and surfaces"):

| Kind | Where |
|---|---|
| Ghost (`button::ghost`) | the default everywhere: Cancel, Close, row and panel verbs. No border at rest, the hover fill under the pointer |
| Primary (`button::primary`) | the one filled accent action of a surface, last and rightmost (Apply, Add, Done) |
| Icon (`icon_button`) | icon-only controls; the tooltip is the accessible name. Borderless, except a title header's close, which is bordered. Every close in the app is this button with the same X |
| Outline | the title bar only |

Footer language: **Apply** commits a Draft, **Cancel** discards it, **Close** dismisses a window that has nothing
to commit. Never OK, Submit or Done for a commit. A question box is No and a filled Yes, Yes rightmost.

**Panels.** The build output docks at the bottom (default) or the right, or pops out into its own tool window. One
owner sets its geometry (`ui/chrome/dock.rs`, `constants/panes.rs`): limits per side, a 5 px sash with a hairline
(invisible at rest, accent on hover, 2 px while dragging), drag 1:1, double click for the first size, a 100 ms fold;
the saved size survives hiding it, and a small window refits only the paint.

**The library.** Cards show the artwork to the card's foot with the title on a dark fade; the chosen card has a
2 px accent outline. A game's status shows in the status bar, never on the card.

**Accessibility.** Every control has an accessible name (tests find controls by name and role).

## 6. Tests and verification

- The default test run takes seconds: `cargo test` in `src/launcher`. A fake engine (`fake-engine`) stands in for
  `nxrs-build`; UI tests drive the real shell with `egui_kittest` by accessible names; CLI tests run the real binary.
  Heavy checks are opt-in (`NXRS_DOCTOR_EXE` runs the real doctor).
- Verify the real thing: run the launcher and capture its windows with `PrintWindow` (this machine is at 3x
  scaling; never a screen copy), then close it by PID.
- A release is checked by `src/launcher/scripts/smoke_release.py`: extracted away from the checkout, no
  `NXRS_BUILD`, a disposable profile, a build of a game folder through the launcher, and an import twice.

## 7. Build and release

- `scripts/build/build.ps1` builds the engine and the launcher in release and assembles `dist/nxrs` (the launcher,
  `nxrs-engine/`, `toolchain/llvm-mingw` with the gnullvm engine, `legal/`), then removes all of `target/` so
  `dist/nxrs` is what remains; `-KeepCache` keeps `target/` for fast rebuilds, `-Run` starts the result.
  `scripts/build/prune.ps1` shrinks `target/` without removing it.
- `src/launcher/scripts/package_launcher.py` makes the release archive with its SHA-256 manifest from an engine
  bundle (`scripts/package_engine.py`); `smoke_release.py` checks it.
