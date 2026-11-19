# translator

The engine's first piece, as one crate: Switch package input (the game's code, and its RomFS, updates and DLC read
in place) and the ARM64 → C translator, driven by the `nxrs` binary. The C it writes is mkw-recomp's generator's,
byte for byte, apart from three additions the runtime needs (below). Credits and licenses: the repository's [README](../../README.md#credits-and-licenses).

## CLI

    nxrs translate <input> --out <dir> [--data <dir>] [--update <pkg> | --no-update] [--keys prod.keys] [--jobs 4]
                   [--modules main,sdk] [--limit N] [--patches file]
    nxrs extract   <input> --out <dir> [--update <pkg> | --no-update]
    nxrs info      <input>
    nxrs update-info [--game <game.nsp>] <package>...
    nxrs romfs ls [-r] | cat | sha256 [--all] <input> [path...] [--update <pkg> | --no-update] [--dlc <title id>]
    nxrs dlc install | sync | remove | list --datadir <dir> [package or title id...]

- `<input>`: a dumped folder (`exefs/` + `romfs/`), an `.nsp` (plain or encrypted; a combined 1G+1U(+1D) package
  applies its own update unless `--no-update`), or a `.nup`. For `romfs`, also a built game's data folder.
- Keys: `--keys`, else `SWR_PROD_KEYS`, else `%LOCALAPPDATA%\RecompLauncher\RecompLauncher\keys\prod.keys`. Inputs
  are only read; key values are never printed or written (markers carry content keys still wrapped, as the package
  holds them).
- `translate` writes the generated C (`layout.h/json`, `patches.h`, `<module>/<module>_NNNN.c`, `<module>_table.c`,
  `manifest.json`, `include/`) and stages the data folder (`--data`, default `<out>/data`): `exefs/`, the RomFS
  marker, `control.nacp` and the icon.
- `update-info` prints `UPDATE<tab>path<tab>title id<tab>version<tab>display version` or `UPDATE error: <reason>`
  per package and exits 1 when any failed; with `--game` the update is opened over the game, which proves it
  belongs to it. Lines end in CRLF on Windows, as Turbo's Python prints them.
- `dlc` keeps Turbo's `dlc.manifest`, `install.json`, messages (`DLC: installed ...`, `DLC error: <reason>`) and
  exit codes.

## Modules

| Module | What |
|---|---|
| `package/` | package input, see below |
| `loader/` | NSO: LZ4 segments, MOD0, symbols (DT_HASH / DT_GNU_HASH), RELA/JMPREL/RELR, function seeds, guest layout |
| `xlate/` | the lifter (integer, branch, load/store, system, exclusives, LSE atomics, PAC as identity), FP / AdvSIMD / crypto / CRC32 as portable C, NZCV and registers in function locals, symbolic jump tables |
| `aot/` | function discovery, chunking, parallel emission (threads), tables, manifests, the patch table (sites, spans, spin waits) |
| `runtime/` | `cpu.h`, `simd.h`, `simd_fp.h`: the headers the generated C includes |
| `bin/nxrs.rs` | the CLI |

### Additions to mkw-recomp's generator

| Addition | Why | Effect on Mario Kart World 1.8.0 |
|---|---|---|
| Functions holding a C++ landing pad (`.eh_frame` FDE → LSDA → call-site table) are built resumable, like setjmp callers (`rt_sj_owner`, every instruction a label) | an unwinder resumes in the middle of the function | 3,638 functions (main 3,070, sdk 568) |
| A `ret` that ends a context restore (x30 loaded through a base other than sp, sp set from a register other than x29: libunwind's `jumpto`) is lowered as `c->pc = x30` + dispatch | it resumes into a landing pad, not a caller | 1 function (sdk `f_7004c172d4`) |
| Each module's first instruction is a seed | the runtime starts it even with no unwind entry or symbol | none (already a start in every module) |

Every other function and file is identical (242,014 functions; the tables, layout and patch headers).

## Package input (`package/`)

The runtime's file system calls this API on every read; everything it returns is `Send + Sync`.

| Call | What |
|---|---|
| `open_game(input, &OpenOptions { keys, update })` | `Game`: `exefs` (the update's code when one is applied), `romfs`, `display_version`, `control`, `addons`, `applied` (`None` / `Embedded` / `Separate(path)` / `LeftOut`), `marker` |
| `Game::write_data(datadir, say)` | stage `exefs/`, the marker, `control.nacp` + icon; says what was applied in Turbo's words |
| `open_data_dir(datadir, keys)` | what the runtime calls at start: the game's `RomFs` from `romfs.nup`, `romfs.nsp`, `romfs.dir` or `romfs/` |
| `RomFs` | `list(dir)`, `walk()`, `find(path)`, `read(path, offset, len)`, `read_file(path)`, `image()` (the raw RomFS image for NSP sources) |
| `update_info(package, keys, game)` | `UpdateInfo` (`line()` is the `UPDATE` line), or Turbo's refusal |
| `dlc::{install, sync, remove, describe, read_manifest, title_romfs}` | add-on content; `title_romfs` is a title's RomFS as `OpenDataStorageByDataId` mounts it |

How each source is read, with nothing extracted:

| Source | RomFS |
|---|---|
| NSP, plaintext | a byte range of the package |
| NSP, encrypted | AES-CTR per read; content key from the ticket (`titlekek_XX`) or the NCA key area |
| NSP with an update (combined, or `--update` NSP / XCI / bare NCA + `.tik`) | BKTR per read: the indirect (relocation) table over the game's section and the update's, the update's ranges decrypted with the counter generation of its AesCtrEx table |
| NUP | its file tree; raw entries are ranges, zstd entries are decoded once (256 MiB LRU cache) |
| folder | its `romfs/` |
| DLC | each title's own package: NSP, XCI, bare NCA (plaintext or AES-CTR, read in place); NSZ / XCZ unpacked once into `romfs.bin` |

Refused with Turbo's reasons: an update for another game (`this update is for <id>, not for this game (<id>)`), a
package with no update, an update on its own as the game, a console-bound (personalized) ticket, a missing ticket,
keys without the content's generation, compressed NSZ / XCZ updates, sparse or compressed NCA storage, a separate
update for a NUP or folder game.

### Markers (the build records what the runtime reads)

Turbo's formats, unchanged:

```text
romfs.nsp  SWRNSP1 / <package> / <offset hex> / <size hex>              a plaintext RomFS
romfs.nsp  SWRNSP2                                                       encrypted and/or update-patched
           <the game's package>
           keys <prod.keys>
           romfs <offset of the RomFS image in the (virtual) section> <size>
           base <file offset> <size> <offset in NCA> <crypto> <counter> <key name>:<wrapped key>|-
           update <the update's package>                                 only for a separate update
           patch <file offset> <size> <offset in NCA> 4 <counter> <key name>:<wrapped key> <indirect off> <size> <AesCtrEx off> <size>
romfs.nup  <package path>
```

This engine adds `romfs.dir` (`<folder path>`): a dumped folder's `romfs/`, read where it is instead of copied.

Two deliberate differences from Turbo: an AES-CTR DLC title is read in place through a version 2 marker instead
of being decrypted into `romfs.bin` with hactool (Turbo's runtime reads that marker too), and paths are printed as
UTF-8 (Turbo's Python prints them in the console code page).

## Verification

- `cargo test`: 54 fast tests (well under a second to run): lifter classes, SIMD, flags, registers, the patch table,
  partitioning, an end-to-end generate, PFS0 / HFS0 (XCI with and without the key area), keys, AES / XTS / CTR,
  tickets, cnmt, NUP (zstd, the RomFS cache), BKTR tables, the RomFS image parser, the DLC manifest and
  `install.json`, and a synthetic package suite with test keys: a combined game + update + DLC (two counter
  generations changing mid-block), separate updates as NSP / XCI / bare NCA, `--no-update`, the markers read back,
  `update-info`, every refusal, DLC from NSP / XCI / NCA / NSZ (solid and block).
- `cargo test --release -- --ignored`: differential checks on real data (SIMD words, jump tables, packages).
- `scripts/pkg/`: reference listings and hashes from the owner's Turbo readers, and `compare.sh` for nxrs.
- `scripts/compare_gen.py <ref> <ours>`, `scripts/cc_check.sh <gen dir> <chunks...>`: generated C against
  mkw-recomp, and a clang compile check.

## Not done yet

- XCI or NSZ as the game's own package (only as update or DLC), compressed (NSZ / XCZ) updates, sparse or
  compressed NCA storage: refused (Turbo accepts XCI as the game package; to be ported, PORT_PLAN 5A.1).
- Code patches shipped by mods (`mods/<mod>/mod.json`) and enabled mods' image bytes.
- The inline-asm SIMD path (AArch64 hosts only).
- Compiling and linking into `<name>_code.dll`.
- Differential testing against dynarmic.
