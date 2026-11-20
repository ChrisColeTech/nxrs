# Packing

How a game package becomes a build: the package readers (`src/translator/src/package/`), the build command the
launcher runs (`nxrs-build`, `src/cli/`), and the folders and marker files a build leaves for the run. Packages are
only ever read in place; nothing is extracted except the ExeFS and a few small files.

## Inputs

`translator::package::open_game` (`src/translator/src/package/mod.rs`) takes any of these and answers the same
`Game` (ExeFS, RomFS, version, control data, add-on hint):

| Input | Reader | Notes |
|---|---|---|
| Dumped folder | `src/translator/src/package/folder.rs` | `exefs/` + `romfs/`, or a flat folder with `main` in it; one level deeper is searched; RomFS optional; `control.nacp` and the icon from the folder or its `control/` |
| NSP / NSZ | `container.rs`, `pfs0.rs`, `app.rs` | plain, encrypted, or combined 1G+1U(+1D) |
| XCI / XCZ | `container.rs` | the `secure` HFS0 partition, with or without the 0x1000-byte key area |
| Bare NCA | `container.rs` | with its ticket beside it (updates) |
| NUP (Switch 2) | `src/translator/src/package/nup.rs` | one indexed file; codec-1 entries are whole-file zstd frames, kept in a 256 MiB LRU cache |
| Homebrew NRO | `src/translator/src/package/nro.rs` | runs on the runtime alone (no SDK) |

`program_indices` lists the programs a multi-program package holds; the cli builds program 0 unless `--program`
(`NXRS_PROGRAM_INDEX`) names another, and other programs get build folders of their own
(`cli::main::other_programs`).

## Keys and decryption

- `keys::keys_path` / `default_keys_path` (`src/translator/src/package/keys.rs`): an explicit `--keys`, else
  `SWR_PROD_KEYS`, else the launcher's copy: `%LOCALAPPDATA%\nxrs\keys\prod.keys`, then the older
  `%LOCALAPPDATA%\RecompLauncher\RecompLauncher\keys\prod.keys` (`keys_candidates`). On Linux, `XDG_DATA_HOME` (or
  `~/.local/share`) then `~/.switch/prod.keys`. Key values are never printed.
- NCA headers: AES-XTS with `header_key` and Nintendo's big-endian sector tweak (`nca.rs`, `crypto.rs`). Content keys
  come from the ticket's title key (`titlekek_XX`) for rights-id NCAs, else the key area
  (`key_area_key_<application|ocean|system>_XX`).
- Sections are AES-CTR; a type 4 (AesCtrEx) section takes each range's counter generation from the update's
  AesCtrEx table.

Missing keys fail with "Nintendo keys are not installed ..."; there is no other key source.

## Updates and BKTR

The update is chosen by `UpdateChoice` (`--update <pkg>`, `--no-update`, or the one a combined package embeds). The
update's ExeFS replaces the game's. Its RomFS is a BKTR patch composed over the base section on every read
(`src/translator/src/package/bktr.rs`): the indirect table maps virtual ranges to the base or the patch, and the
AesCtrEx table gives the counter generation of each patch range. Nothing is written out.

`update_info` (`app.rs`) answers the launcher's `--update-info` with Turbo's `UPDATE\t...` line (title, version, and,
with the game given, whether the update belongs to it). PORT_PLAN M0 records it byte-identical to Turbo in 8 cases.

## Compressed and sparse storage

- NCA compressed storage (`src/translator/src/package/compressed.rs`, FS header CompressionInfo at +0x178): the data
  level is LZ4 blocks, zero runs and plain runs behind a bucket tree. Newer card images (Super Mario Party Jamboree)
  hold their Program NCA this way. A bad table reads "the compression table is damaged (wrong keys?)".
- Sparse storage in a Program NCA is refused: "sparse storage in the Program NCA is not supported" (`app.rs`). A
  sparse base game (nxnative's "sparse base games") cannot be built; no PORT_PLAN row tracks it.
- ZBIC NSO segments (Switch 2): `src/translator/src/loader/zbic.rs` rewrites ZBIC's interpolative-coded FSE headers
  into standard zstd and decodes with stock zstd. The build stages these modules unpacked into `<data>\exefs` (the
  runtime reads LZ4 only). The shaders crate re-exports the same decoder.

## RomFS markers

The build never copies the RomFS. It writes a marker into the data folder (`src/translator/src/package/marker.rs`),
and the runtime opens the RomFS from it at start (`open_data_dir`):

| Marker | Meaning |
|---|---|
| `romfs.nsp` `SWRNSP1` | a plaintext RomFS: package path, offset, size |
| `romfs.nsp` `SWRNSP2` | encrypted and/or update-patched: package, keys path, base / update / patch rows with wrapped keys, plus this engine's `compressed` rows |
| `romfs.nup` | a NUP's RomFS tree read in place |
| `romfs.dir` | a dumped folder's `romfs/` read in place (this engine only) |

Content keys stay wrapped in the marker; they are unwrapped with prod.keys when it is opened (explicit keys, else
`SWR_PROD_KEYS`, else the marker's `keys` path). A moved or renamed package breaks the marker; rebuild or re-run.

## Add-on content

`src/translator/src/package/dlc.rs` (Turbo's manifest and messages): `install`, `sync`, `remove`, `describe`,
`title_romfs`. The data folder gets `dlc.manifest` (one line a title: id, version, `data|stub`, package) and per
title `dlc/<title id>/romfs.nsp` (read in place) or `romfs.bin` (an NSZ / XCZ title unpacked once), with
`install.json` to skip repeated work. AES-CTR titles are read in place through an `SWRNSP2` marker rather than
decrypted. At run time `fs::aoc_indices` (`src/os/src/services/fs.rs`) offers only titles whose RomFS opens, so a
title with missing data is not offered (MK8D hung otherwise; PORT_PLAN 5A.1).

## NROs loaded at run time

`package::nro::collect` finds every NRO the RomFS's NRR files register (by SHA-256) and the build translates them
with the ExeFS at fixed bases after it (`<data>\nro.tsv`). An online SDK (Epic Online Services, export
`EOS_Platform_Create`) is listed as refused, so the game plays offline. An NRO no file holds (decoded by the game at
run time) is captured by a capture boot (`cli::pipeline::capture_boot`, `NXRS_NRO_CAPTURE`; `NXRS_NO_CAPTURE_BOOT=1`
skips it) and the build runs again with it. PORT_PLAN 5A.1 marks NRO capture Partial.

## The build

`nxrs-build build <input>` (`src/cli/src/main.rs`, `cli::pipeline::build` in `src/cli/src/pipeline.rs`), in order:

1. Check the compiler and runtime library first (`cc::find_clang`, `cc::find_engine`).
2. Open the package, print each module (`main`, `subsdk*`, `sdk`: base, code size, build id) and the `Profile:` line.
3. Apply the profile's patch table, `NXRS_PROBE` probe lines, the player's mods (`src/cli/src/mods.rs`) and code
   cheats (`src/cli/src/cheats.rs`); see [rewriter.md](rewriter.md) and [cheats.md](cheats.md).
4. Translate ARM64 to C into `<name>_build\gen`, skipped when the stamp key (ExeFS, patch text, options) is unchanged.
   `discovered.tsv` hints from earlier runs become function starts.
5. Stage the data folder (`Game::write_data`): `exefs/`, the RomFS marker, `control.nacp`, the icon, the manual's
   HTML, plus `frame_step.txt`, `fps_lock.txt` and `nro.tsv`. The ini is written once, when missing.
6. Shaders on a thread beside the compile (`shaders_step`); never fatal: a failure leaves the live translator to
   cover them ([shaders.md](shaders.md)).
7. Compile one object per chunk under a content key (`cc::object_key`; a changed chunk recompiles alone, stale ones
   are removed) and link `<name>_code.dll` with the runtime statically, plus the small loader `<name>.exe`
   (`cc::STUB_C`), which loads the DLL and calls its one export `nxrs_game_main`.

Output layout (default `<input folder>\SwitchRecomp Builds\<name>`):

```text
<name>.exe                      loader
<name>_code.dll                 game code + runtime (Linux: <name> and <name>_code.so)
<name>_data\                    exefs\, RomFS marker, control.nacp, icon, DLC, saves, caches
<name>.ini                      settings (docs/settings.md)
<name>_build[_v<N>|_base]\gen   generated C, kept for incremental rebuilds
                         \obj   objects under content keys, link.rsp
```

### Toolchain

`cc::find_clang`: `NXRS_CLANG`, `--cc`, the bundled clang, `NXRT_LLVM`, LLVM's installer folder. With Visual Studio
the MSVC target links `nxrt.lib` (`NXRS_RUNTIME_LIB`); without it, or with `NXRS_TOOLCHAIN=gnu`, the bundled
llvm-mingw (`NXRS_LLVM_MINGW`) and the gnullvm libraries (`NXRS_GNU_LIBS`). `SWR_CFLAGS` adds compiler flags;
`--jobs` defaults to the CPU count. The game-code flag set is fixed (`cc::GEN_FLAGS`, `-march=x86-64-v3`); players
get no optimisation options (DECISIONS.md section 7). A "file is in use" error at link means the game is still
running.

## Commands and the launcher contract

| Command | What |
|---|---|
| `nxrs-build build <input> [-o name] [--outdir] [--jobs] [--update pkg / --no-update] [--keys] [--machine]` | the build above; `--machine` adds `@progress` / `@result` lines (`src/cli/src/out.rs`) |
| `nxrs-build update-info [--game g] <pkg>...` | one `UPDATE` line a package |
| `nxrs-build dlc install / sync / remove / list --datadir <dir>` | add-on content |
| `nxrs-build profile <title id> [<build id>]` | the profile and its patch table |
| `nxrs-build doctor` / `contract` | toolchain, Vulkan device (1.3 needed), free disk / the contract version |
| `nxrs-build translate <input> --out <dir>` | development: C and data folder only |
| `nxrs-translate info / extract / romfs ls\|cat\|sha256` | package inspection (`src/translator/src/bin/nxrs.rs`) |

Contract v1 (Turbo's `switchrecomp.py` arguments, unknown ones ignored) and v2 are in
[docs/LAUNCHER_CONTRACT.md](../LAUNCHER_CONTRACT.md). Exit codes: 0 done, 1 failed (reason on stderr or the `UPDATE` /
DLC error lines), 2 bad arguments. Every output line is flushed, CRLF on Windows.

## Identification and build ids

A module's build id is read from its NSO header (`package::nso_build_id`); the main module's first 8 bytes name the
build everywhere (cheat files, profile rows, `nso_match`). Profiles are found by title id and main build id
(`profiles::lookup`, `src/profiles/src/lib.rs`); a build no profile registers still gets its title's profile and
the function-matched patch rules. `cargo run -p translator --example exefs_dump`, `symat` and `symgrep`
(`src/translator/examples/`) help when a build id or symbol is in question.

## Problems found on games

### DLC with missing data offered to the game

- **Symptom:** Mario Kart 8 Deluxe hangs when an installed DLC title's data is missing.
- **Cause:** every manifest title was offered, including ones whose RomFS does not open.
- **Fix:** `fs::aoc_indices` offers only titles whose RomFS opens (PORT_PLAN 5A.1, Turbo 2cdeabd).

### Link fails with "is in use"

- **Symptom:** the build ends with `<name>_code.dll is in use. Close the game and try again.`
- **Cause:** the game from the previous build is still running and holds the DLL.
- **Fix:** close the game (or end its process by PID) and build again; the objects are reused, so only the link reruns.

## Known limits

- Sparse Program NCAs are refused (above).
- XCI as the game's own package has been built and run once (Jamboree, PORT_PLAN 5A.1); other card images are untested.
- nxnative's iOS / IPA packaging and external-drive handling have no counterpart. Its diagnostic bundle is
  `scripts/bug_report.py` (a game folder's log, ini, F5 dump, newest metrics, shader problems, redacted, into one zip).
- Package tests without game data: `src/translator/src/package/synthetic.rs` builds test-key NSPs, updates, XCIs,
  markers and DLC; `src/translator/tests/real_packages.rs` runs against real dumps when present.
