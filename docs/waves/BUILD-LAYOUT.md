# BUILD-LAYOUT - built games carry their data; builds go to `<source folder>\nxrs\<name>`

Owner decisions, 2026-11-26. Branch `layout` (worktree `D:\Projects\RecompLauncherEngine-layout`), base
`gameplay-harness` 3b690b2.

## Decisions

1. **Built games carry their own game data.** A build extracts the game's RomFS (decrypted, with the applied update's
   BKTR patch or the NUP's tree already resolved) into `<name>_data\romfs\`, so the game runs with its source package
   moved or deleted. Add-on content installed from a package (`dlc\<id>\romfs.nsp` markers today) is extracted into
   `dlc\<id>\romfs\` the same way. Disk use is accepted.
2. **Build folder `<source folder>\nxrs\<name>`** replaces `<source folder>\SwitchRecomp Builds\<name> [nxrs]`.
   Library entries in the old default layout are migrated: the folder is renamed to the new path and `buildDir`
   updated. Running games, and folders that cannot be moved, are skipped and reported. Old non-nxrs build folders and
   a non-empty `SwitchRecomp Builds` folder are left alone.

## Plan

### Engine (translator `package`, cli, os)

- `package/extract.rs` (new): `tree(romfs, dest, progress)` writes every file of a `RomFs` into `dest` through a
  `dest.partial` folder renamed into place when complete (an interrupted build never leaves a half tree the runtime
  would serve), on several threads (AES-CTR / BKTR decryption is CPU work); `unsafe_name(romfs)` finds a path a
  Windows folder cannot hold faithfully (reserved names, `<>:"|?*\`, trailing dot / space, two names equal but for
  case); `image(romfs, file)` writes the RomFS image as one file for that case.
- `Game::write_data`: no in-place marker any more. The RomFS goes to `romfs/` (or, for a tree a folder cannot hold,
  `romfs.bin`, the decrypted image); the old markers (`romfs.nsp`, `romfs.nup`, `romfs.dir`) are removed. A record
  `romfs.source` keeps what the old marker said (the source package and the applied update, for the launcher's
  import and update adoption) plus an `extracted ...` row (file count, bytes, size and modification time of each
  package read). While the record matches, a rebuild does not extract again.
- `marker::open_data_dir`: `romfs.bin` (a whole image in the data folder) before `romfs/`; legacy markers still
  first so builds made before this wave keep working until rebuilt.
- `RomFs` folder backend: reads keep a small cache of open files and read positionally (today each read opens the
  file: the game's data is now always a folder, so its read path must be as cheap as the package's).
- `dlc::extract`: plaintext (crypto 1) and AES-CTR (crypto 3) titles are extracted into `romfs/` (storage `folder`
  in `install.json`); an installed title with storage `package` counts as not installed, so the next sync (every
  build and Play) extracts it when its package is there. NCZ titles stay `romfs.bin` (already self-contained).
- cli: `default_outdir` = `<input folder>\nxrs\<name>`; `--outdir` doc; progress `@progress data <MB done> <MB
  total>` and `RomFS: extracting ...` lines during extraction.

### Launcher (`src/launcher`)

- `constants/engine.rs`: `NXRS_BUILDS = "nxrs"`, the old `DEFAULT_BUILDS` and `[nxrs]` suffix kept only as the
  migration's recognisers; `ROMFS_SOURCE = "romfs.source"`; legacy marker names.
- `model/names.rs` `build_folder_for` replaced by the layout rule (`<base>\nxrs\<name>`, base = source folder or the
  Build location setting) and `migrated_build_dir` (old default -> new); `game_json` moves a non-nxrs entry to the
  new layout instead of the `[nxrs]` suffix; `sources::game_from_source` uses it.
- `services/markers.rs`: source and applied update read from `romfs.source` first, the legacy markers second.
- `services/build_facts.rs`: `in_place` = the build's data folder still has a legacy in-place marker (a build from
  before this wave). A build that carries its data is never "Source offline".
- `services/build_move.rs` (new): the migration: running games (process list) and locked folders skipped, rename
  on the same drive, `buildDir` updated, empty `SwitchRecomp Builds` removed, report lines; the library saved
  through `library_file::save` (Qt backup) after a `library.pre-layout.json` copy.
- CLI `--migrate-builds` (headless: prints one line per entry, exit 0 / 1); the window runs the same migration once
  at start.
- Tests: model rules (new layout, migration recogniser, non-nxrs entry), markers from `romfs.source`, offline rule,
  migration on a temp library with fake build folders (moved, running skipped, target exists skipped, old folder
  left with other content, non-nxrs folder untouched, library format and backup).

### Docs

`docs/LAUNCHER_CONTRACT.md` (layout table, "Built game" row, output notes 6 / 7 / 10, v2 build folder), PORT_PLAN.md
rows that say the RomFS is "read in place", `src/launcher/PLAN.md` G-02 / G-08 / D-05, `docs/env_switches.tsv`
NXOS_SAVE row, `src/os/src/boot.rs` doc, README of os if it says "in place".

## Verification

- `cargo test --release -p translator` (extract, marker, dlc), `-p nxrs-cli`, `-p os` (fs folder reads).
- Launcher `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- Real game: one small title (not OoT / MKW / DKB / Smash) built with this worktree's `nxrs-build` into a scratch
  folder; `<name>_data\romfs\` present; its source package renamed away; the game run to its first screens; package
  restored.
- Migration: a copy of the library in a temp data folder with fake build folders.

## Added mid-wave (lead, owner 2026-11-26)

Remove from library with a ticked-by-default "Also delete the build (N GB)": saves, sdmc, cheats and mods kept in
`<data>\saves\<title id>` (title id from `exefs\main.npdm`, else the output name), the build folder deleted (refused
while running / building, or when it holds the source or is not this game's build); the next successful build puts
the kept folders back (launcher-side; the engine still reads only `<name>_data\save`). LAUNCHER_CONTRACT.md v2 and
PLAN.md X-05.

**Owner verdict on the dialog (2026-11-26): rejected, to be rolled back.** The question box was first extended with
the tick box under the old QMessageBox icon, then reworked onto the Settings grammar (Group heading, info marks,
shared footer). The owner: it looks bad, formatted after Settings, and the family's dialogs and docs were not read
first. The agent read `docs/ARCHITECTURE-AND-STYLE-GUIDE.md` and the giga-cut popup docs only after the owner asked.
Commit 384de45 (the message/question box rework and `--show-remove`) is the part to revert; the remove-with-delete
behaviour itself is in 6c9656f.

## Send-back 1 (lead, 2026-11-26): the remove dialog in the existing style, migration explicit

Reference first (PrintWindow, 3x, a test data folder): the Remove from library box and About of a launcher built
from `gameplay-harness` (`build/dialogs/ref-remove-kart-0.png`, `ref-about-0.png`). 384de45 reverted; the tick box
is the question text's last paragraph, in the text column beside the `?` mark, info mark after it, No / filled Yes
unchanged (02b234a). The window no longer migrates at start; `--migrate-builds` and Settings > Build defaults > Old
build folders > Move (a readout and a ghost button, the Engine > Checks pattern) are the only ways.

## Send-back 2 (owner via lead): one dialog grammar app-wide

**Owner verdict (2026-11-26): rejected; superseded by `docs/waves/DIALOGS.md`.** Footers kept redundant
Close / Cancel / No beside the header's X; message and question boxes had a bare ringed mark and no heading; and
this inventory counted popovers (Search & Filter, Build tools), menus and the build output as dialogs. They are not
dialogs (guide §5): their captures moved to `docs/images/popovers/`. Rules 2, 3 and 8 below are replaced by the
guide's current §5.

### Inventory (before; captures in `build/dialogs/before/<name>-0.png`, launcher at 02b234a + capture flags)

Checked against `docs/ARCHITECTURE-AND-STYLE-GUIDE.md` §5 and the AGENTS.md family grammar. "Shared header" =
`authored::header` (12 pt semibold title, 28 pt, hairline under it, bordered 22 pt X named `Close <title>`).
"Shared footer" = `authored::footer` (hairline, ghost then the one filled primary, right).

| Surface (capture) | Kind | Header | Body | Footer / buttons | Deviations from the guide |
|---|---|---|---|---|---|
| Message, info: About (`about`) | child window | shared | ringed `i` mark, text | **own strip, no hairline**; filled **Close** (label drawn off-centre) | footer not shared; Close filled (guide: Close is ghost, a surface with nothing to commit has no primary) |
| Message, error: Cannot add game (`error-add`) | child window | shared | red `!` mark, text | as About | as About |
| Question: Remove from library, not built (`remove-unbuilt`) | child window | shared | `?` mark, text | own strip, no hairline; ghost No, filled Yes | footer not shared |
| Question + tick box: Remove, built (`remove-built`) | child window | shared | `?` mark, text, tick box + info mark as last paragraph | as above | footer not shared |
| Question: close during a build (not capturable without a build; same `message::question`) | child window | shared | as Remove | as Remove | as Remove |
| Install keys | native file picker, then the info message box | - | - | - | as About |
| Properties, 6 tabs (`props-0..5`) | rail work window, 820 pt | shared | Groups and Rows | shared: Cancel, Apply | General's **About** Group and Settings' **Library** Group have no info mark (every other Group has one); width 820 vs Settings 840 |
| Add game (`add-game`) | rail work window, 820 pt | shared | as Properties (4 tabs) | shared: Cancel, Add game | as Properties |
| Settings, 5 tabs (`settings-0..4`) | rail work window, **840 pt** | shared | Groups and Rows; Checks / Old build folders: readout + ghost verb | shared: Cancel, Apply | Library Group without info mark; rail windows differ in width |
| DLC (`dlc`) | work window, no rail | shared | Group heading, list | **own row, no hairline**: ghost Remove (leading), ghost Close, filled Add DLC… | footer not shared; **empty list has no status line** (guide: a list's state is a status line under it; Properties > DLC has one) |
| Updates (`updates`) | work window, no rail | shared | Group heading, **"Version to build" label inline, not a Row** (no 190 pt column, no info mark), list | own row, no hairline: ghost Remove, ghost Build now, ghost Close, filled Add update… | footer not shared; the select is not a Row; empty list has no status line |
| Build tools popup (`tool`) | in-window popover | **own title, 17 pt, no hairline**, X | readout text | filled **Open Settings** | header not shared; a navigation verb filled (guide: primary = the commit) |
| Search & Filter (`filters`) | in-window popover (Live) | **own title, 17 pt, no hairline**, X | labels above controls (stacked rows), segmented Gallery/List | ghost Clear filters, filled **Done** | header not shared; **Done** (guide: never Done); a Live surface has no commit, so no primary |
| Build output dock / pop-out (`console`) | panel | panel bar: bold title, ghost verbs, borderless icons | log | - | none (panel grammar, guide §5 Panels) |
| Menus (`menu`) | menu | - | - | - | none |

### Rules chosen (written into the guide §5)

1. **One header**: every titled surface, child window or popover, uses `authored::header`.
2. **One footer**: every child window ends with `authored::footer`: a hairline; the surface's own verbs as ghost
   buttons at the leading side (Remove, Build now); then Cancel / No / Close as ghost; then the one filled
   primary, last. A surface with nothing to commit (a message, a Live popover) has no filled button.
3. **Messages and questions keep their shape** (the mark, the text, a tick box as the text's last paragraph with its
   info mark) and use rule 2: a message's footer is a ghost Close; a question's is ghost No and filled Yes.
4. **Every Group heading has an info mark** (the common form: 17 of 19 Groups); **Rows**: an info mark when there is
   an explanation (the common form: Color mode, Format, Status have none).
5. **A list's state is a status line under it** in every window that shows the list (DLC, Updates, Properties).
6. **A control with a label in a form is a Row** (190 pt label column), also outside rail windows.
7. **Rail work windows share one size** (`RAIL_WINDOW`).
8. **Popovers (Live)**: the shared header; no Done, no filled button; the X closes.

### Applied (after; `docs/images/dialogs/<name>.png`, all captures in `build/dialogs/after/`)

- `authored::footer_bar` / `footer_at_bottom` (leading ghost verbs, ghost dismissal, optional primary with its
  tooltip) is the one footer: message boxes (ghost Close), questions (ghost No, filled Yes), DLC and Updates
  (Remove / Build now leading, Close, filled Add…), and through `footer` the rail windows.
- `authored::popover_frame` + `authored::header` for Search & Filter (Done removed) and Build tools (Open Settings
  ghost); both had their own 17 pt titles and frames.
- DLC and Updates: "No DLC / No update is installed for this game." status line; Version to build is a Row.
- Properties > General > About and Settings > Appearance > Library got their info marks; the rail windows share one
  size (840 x 440, least 730 x 360).
- Capture openers: `--show-window remove|about|build-settings|tool`, `--show-add <package>` (flags moved to
  `cli/flags.rs`, options.rs was at the 200-line limit).
- Not changed: the console dock and the menus (panel and menu grammar, no deviation); native file pickers (OS
  dialogs); the close-during-build question was not captured (needs a running build) but is the same
  `message::question` as Remove.

- Tests: translator 166 + package 35 (extract, synthetic NSP/update/DLC moved-away reads), nxrs-cli 29, os all pass;
  launcher `cargo test` all pass (new: model `layout_tests`, `tests/migrate.rs` through the real CLI,
  `tests/remove_build.rs` through the fake engine), clippy `-D warnings` and fmt clean.
- Real library (owner-authorized): `nxrs --migrate-builds --select-title "Anime Girls Sun of a Beach"` moved
  `...\SwitchRecomp Builds\Anime Girls Sun of a Beach [nxrs]` -> `...\nxrs\Anime Girls Sun of a Beach`; the old
  Switch2RECOMP folder `SwitchRecomp Builds\Anime Girls Sun of a Beach` left. Backups: `library.pre-layout.json`,
  `library.pre-build-layout-20261126.json`.
- Built with `nxrs --select-title ... --build`: `RomFS: extracted into romfs/ (66 files, 886.3 MB, 2.6 s)` from the
  AES-CTR + compressed NSP, `@result ok`, 356 s. Source NSP renamed away, `play.py agsob --env NXGFX_GPU=0`
  (`build/play/20261126-121200-agsob`): completed 14/14, `fs: RomFS from folder ..._data\romfs`, 60.7 FPS mean
  (target 60), 0 red lines, sheet read (menu, swim, beach, fire). NSP restored.
- Not run for real: remove-with-delete (tests only; the dialog is to be rolled back), DLC extraction on a real DLC
  package, a migration of the whole library.
