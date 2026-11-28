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

**Done, on branch `3DS-W1`.** All six brief items landed (commits `36c0aec`, `9bf9718`, `da740c9`,
`84d8457`, plus the final hunk commit); suites green: `cargo test -p translator3ds` (74 lib + 1
real-title skip + 6 synthetic + 1 doc), `cargo test -p translator` (73 + integrations), root default
suite `cargo test --workspace --exclude renhdrum` (0 failures across every crate), launcher
`cargo test` (55 lib + integration crates) and `cargo clippy --all-targets -- -D warnings` clean.

**Real .3ds (Sonic Generations USA, Decrypted, 652 MB)**: `nxrs-build build ... --no-build` stages
`Sonic_data\`: `exefs\code` (4,214,784 bytes) + `exefs\exheader.bin` (1024), `romfs\` (22 files,
627,842,669 bytes), `control.smdh` (14,016 bytes, magic `SMDH`), `romfs.source` (`marker romfs.ds`).
`C generated: 178/178 chunks (8s)`, `C code: 178 files, 215.5 MB, 15283 functions`; a second run says
`(unchanged since the last build)` / `romfs/ is current` — both currency paths proven. **RomFS
cross-check: IDENTICAL** — the donor's own `rom.rs` (scratch binary path-depending on
`D:\Projects\3dsrecomp` at `d143f2c5`, outside the repo) yields the same 22 files path-for-path and
size-for-size as the staged tree (`sonic-romfs-donor.txt` vs `sonic-romfs-staged.txt`). The staged
SMDH decodes: Japanese short ` ジェネレーションズ`, English short `C GENERATIONS`, real RGB565 pixel
data at 0x2480. The launcher's service-layer test imports the same file (`valid_source` +
`game_from_source` + `add` into a temp library).

**Real .cia (Pokemon Moon USA RF Retail, 3.2 GB)**: the container walk runs exactly as the donor's
`cia_executable` does (header → cert/ticket sizes → TMD at 0x2DC0, signature 0x140, 2 content
records after the 64 content-chunk records; content 0 at 0x3940; NCCH magic found at +0x100), and
the dump **is retail-encrypted**: content 0's NCCH flags read `flags[3]=1, flags[7]=0` (no NoCrypto,
7.0+ key method), so the build refuses with the donor's exact line
`Cannot load the game: the dump is encrypted, it has to be decrypted first`, exit 1
(`moon-real-build.log`). That is the behaviour plan §2 specifies for encrypted dumps ("not
implemented in the first waves; nxrs says the same refusal line until the owner decides a decryption
lane") — the brief's named title cannot itself demonstrate staging (see Questions). The .cia path's
full staging is proven instead by: the synthetic decrypted .cia through the real CLI
(`moon-fixture-build.log`: translate 2 chunks, stages `exefs\`, `romfs\static.crs`, `control.smdh`,
`romfs.source`) and the crate tests (`a_cia_stages_like_a_cart`, plus the container-walk parity the
launcher's `cia()` reader now mirrors, TMD offsets byte-identical to the donor's formula).

**Findings**: (1) the ExeFS icon entry is named `icon` (no dot; 3dbrew's ExeFS names), not `.icon` —
rom.rs accepts both. (2) The donor's CIA reader walks 64×0x24 content-chunk records before the
content records; the launcher's `cia()` gate now walks them identically (its first version did not
and refused the real file).

**Deviations from the plan's letter** (each small, each recorded):
- `constants/engine.rs::BUILD_ICONS` gained `control.smdh` — not in the brief's file list, but the
  W1 row's "launcher imports … with SMDH card art" needs `build_cover` to reach the staged file.
- `services/sources3ds_tests.rs` is a new test file (the brief named `sources_tests.rs`, which sits
  at 234 of its 250-line cap; the launcher's style rules are hard limits — split, not bend).
- `Cargo.lock` (root) carries the `translator3ds → translator` and `nxrs-cli → translator3ds` edges.
- The gen key hashes the programs' bytes + hints; no patch text or profile inputs exist on the 3DS
  branch yet (W6 owns profiles).

**Slip (caught and corrected)**: `cargo fmt -p translator3ds` was run once to tidy the new test and
reflowed the whole crate, including W0's vendored donor files (whose hash-fidelity W0 records).
`git checkout HEAD -- src/translator3ds` restored the committed vendored state; only the intended
rom.rs hunks and the title.rs .cia staging test were re-applied (the current diff touches exactly
`rom.rs` and `title.rs`). The vendored files stay in the donor's style, as W0 shipped them; rustfmt
is not clean over the crate — the same state W0 merged.

**Unproven**: staging through a *real decrypted .cia* (the brief's file is encrypted; a decryption
lane is plan §10 D2, owner-owned); the staged data folder has not been *run* (W2 owns the runtime);
`discovered.tsv` hints reading has no real producer yet; SMDH titles are decoded and returned but
not wired into any launcher display (no surface owns them this wave); `romfs.bin` fallback does not
exist for 3DS titles whose RomFS names a Windows folder cannot hold — such a title refuses the
extraction with the extract message (a Switch build writes `romfs.bin` from its image backend; a
3DS build's `Stored` backend has none — reported per the brief's hypothesis clause, not forced).
