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

## Results

(filled in at the end)
