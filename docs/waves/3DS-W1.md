# 3DS-W1 — Input: `.3ds`/`.cia` accepted end to end (translate-only)

Plan: `docs/36-3DS-PORT-PLAN.md` §2 (formats/staging), §7 (pipeline changes), §8 W1 row, §9 (file list).
Brief: owner, 2026-11-27 (this wave's agent brief). Base: `2d50c8b` (W0 merged). Worktree
`D:\Projects\RecompLauncherEngine-3DS-W1`, branch `3DS-W1`. Donor pin `d143f2c5` (read-only).

## Goal (the W1 row's exit criterion, measured)

`nxrs-build build <game>.3ds --no-build` stages the 3DS data folder (programs, `romfs/`, `control.smdh`,
`romfs.source`); a real decrypted title's RomFS listing equals the donor's (`rom.rs` cross-check); the launcher
imports `.3ds`/`.cia` with SMDH card art (launcher `cargo test`). No compile, no link, no engine-lib lookup:
a full 3DS build says what is not built yet (the 3DS engine runtime, W2/W3) and exits non-fake.

## Plan

1. **`package::io` public** (the one Switch-crate touch): `mod io;` → `pub mod io;` in
   `src/translator/src/package/mod.rs`. translator3ds's rom.rs drops its local `Blob` twin for the real
   import (W0's vendor-delta hunk 1 named this swap; W0's wave doc §deviations 1 records it), gaining
   `From<PackageError> for Error` so donor error text stays verbatim.
2. **`translator3ds/src/title.rs` full W1 form** (plan §2 "The precise changes", `Game3ds::marker` row):
   - `Game3ds::write_data(_with)` mirroring the Switch flow's layout and words:
     - `<data>/exefs/` — the exe's program bytes as the loader/runtime read them: `code` (the decompressed
       `.code`, segments back to back as `Title::code()` yields) and `exheader.bin` (the 0x400 exheader whose
       code-set infos say where text/rodata/data sit). The CROs stay RomFS files (that is where a 3DS title
       carries them) and reach the data folder through `romfs/` below; the translated C for exe + CROs lands
       in the build folder's `gen` as W0's generate already does.
     - `romfs/` extraction through the **existing** `translator::package::romfs` machinery: title.rs walks the
       3DS RomFS tables into `(path, size, id)` triples and serves the bytes through a `StoredFiles` impl that
       reads the dump positionally (fresh `Blob`, `Send + Sync`); `RomFs::from_stored` builds the Switch-shaped
       `RomFs`, so `extract::tree`, the currency check and the record text are the Switch code paths' shape.
       (The Switch `image_tree` cannot parse a 3DS image: its header is 0x50 bytes of u64 fields, the 3DS
       level-3 header is 0x28 bytes of u32 fields — `from_stored` is the adapter shape that needs no Switch
       change, and it is the same backend the web lane's manifest uses.)
     - `control.smdh` — the ExeFS `.icon` entry raw (SMDH: icons + titles).
     - `romfs.source` — the Switch record's shape naming this wave's marker: line 1 the dump path,
       `marker romfs.ds`, `extracted romfs <files> <bytes>`, `stamp <size> <ns> <path>`. Currency: the record
       matching means the copy is current (same rule as Switch).
   - rom.rs gains the `.icon` accessor (the ExeFS table entry, like `.code`) and keeps its refusals verbatim.
   - gen.rs's `Event::Generated` carries `functions` so the pipeline prints `C code: … functions` without a
     second discovery pass.
3. **`src/cli/src/pipeline.rs`**: `build()` sniffs the container magic (NCSD at 0x100 / NCCH at 0x100 / CIA
   header size 0x2020 at 0) before the Switch flow touches anything. The 3DS branch `build_3ds`:
   open (`translator3ds::title::open`) → module lines (exe + CROs: name, base, MB) → gen key over the programs'
   bytes + options (+ hints), reuse says `(unchanged since the last build)` → translate
   (`translator3ds::gen::generate`, events printed as the launcher parses them: `Analyzing the code...`,
   `C generated: n/m chunks`, `C code: … functions`) → data folder (`write_data_with`, `RomFS:` lines +
   `@progress data` in MB) → `<name>.ini` when missing → `--no-build` returns; without it a clear refusal
   naming the 3DS engine runtime (W2/W3) as not built, exit non-fake. No Switch path behaviour changes.
   `--update/--no-update` and `--target web` refused on the 3DS branch (update CIA merging is a later wave,
   the web lane is W7). Hints read `<data>/discovered.tsv` when present (the file W2's interpreter writes;
   absent today, so the read is empty).
4. **`src/cli/src/main.rs`**: the build input's help/doc text names `.3ds .cia .cxi`; the crate doc comments
   gain the 3DS line; one doctor information line (decrypted 3DS dumps need no keys) — trivial only.
5. **Launcher** (`src/launcher`, style rules are hard limits — a new test file rather than a bent one):
   - `services/sources.rs::valid_source` by magic: `.3ds` NCSD at 0x100 (+ its partition 0 NCCH),
     `.cia` header size 0x2020 at 0 (+ its TMD content record 0), `.cxi`/`.app` NCCH at 0x100.
     Encrypted-look inputs refuse (NCCH crypto flags / CIA record's encrypted bit) with their own words.
   - `services/artwork_decode.rs`: SMDH beside the image path — `decode()` dispatches on the `SMDH` magic,
     decoding the 48×48 RGB565 large icon; `smdh()` also returns the UTF-16 titles (short, long, publisher;
     consumed by a later wave's version/name wiring, exposed here).
   - `constants/words_library.rs`: the new source words + `GAME_FILTER` gains the 3DS suffixes.
   - `constants/engine.rs::BUILD_ICONS` gains `control.smdh` (one entry; `build_cover` finds the staged card
     art through it — noted as a deviation: the file is not in the brief's list but the W1 row's "launcher
     imports ... with SMDH card art" needs the cover path to reach the staged file).
   - `services/sources_tests.rs` is at 234 lines of its 250 cap: the 3DS accept/refuse cases go into a new
     `services/sources3ds_tests.rs` (split, not bend). Service-layer tests over fixtures; the two real dumps
     exercised when present (skip with a line when the paths do not exist, keeping the default suite green
     everywhere), always against a temp data dir — never `%LOCALAPPDATA%\nxrs`.
6. **`docs/LAUNCHER_CONTRACT.md`**: the 3DS input line, `control.smdh`, the decrypted-dumps/no-keys note.
   Nothing else in docs.

## Titles (owned by this wave)

- `P:\Games\Nintendo\DUMPS\Sonic Generations (3ds)\Sonic Generations (USA) Decrypted.3ds` (.3ds, 652 MB).
- `P:\Games\Nintendo\DUMPS\Pokemon Moon (cia)\Pokemon Moon-USA-RF-Retail.cia` (.cia, 3.2 GB, content 0 on
  demand — never loaded whole).
Not opened (later waves own them): Star Fox 64 3D, Pokemon Ultra Moon.

## Verification

1. `cargo test -p translator3ds` (new: the Blob swap, `.icon`, `write_data` staging, the adapter), then
   `cargo test -p translator`, then the root default suite `cargo test --workspace --exclude renhdrum`.
2. Real `.3ds`: `nxrs-build build "<path>" --no-build --outdir <worktree>\build\DS\W1\sonic -o Sonic` —
   staged tree listed; RomFS cross-check: a scratch cargo binary (outside the repo, opencode temp dir)
   path-depending on the donor prints the donor's `rom.rs` walk for the same dump; the two listings compare
   equal (folder-for-folder, file-for-file, size-for-size). Runs are logged.
3. Real `.cia`: the same `--no-build` proof through the CIA path (content index 0), staged tree listed.
4. Launcher: `cargo test` and `cargo clippy --all-targets -- -D warnings` in `src/launcher`.
5. Evidence under `build/DS/W1/` (logs, staged trees, hashes); `build/` is not committed.

GPU: n/a (no game runs, no play.py, no relink; `%LOCALAPPDATA%\nxrs` untouched).

## Outcome

(filled at the end of the wave)
