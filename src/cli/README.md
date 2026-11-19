# cli (`nxrs`)

The engine's command line: the build (package → data folder → C → shaders → code DLL + loader), updates, DLC, and
the launcher contract (v1 drop-in for Turbo's `switchrecomp.py`, v2 subcommands): [docs/LAUNCHER_CONTRACT.md](../../docs/LAUNCHER_CONTRACT.md).

    nxrs build <input> [-o <name>] [--outdir <dir>] [--jobs 4] [--update <pkg> | --no-update] [--keys <file>]
               [--no-build] [--no-shaders] [-q] [--machine]
    nxrs update-info [--game <input>] <package>...
    nxrs dlc install | sync | remove | list --datadir <dir> [package or title id...]
    nxrs profile [<title id> [<main build id>]] [--package <input>]
    nxrs doctor | contract
    nxrs translate <input> --out <dir>                      (development: C + data folder, for nxrt-dev e2e)
    nxrs <input> --outdir <dir> -o <name> ...                (contract v1: Turbo's options)

Build and test: `cargo build --release -p cli -p runtime` (nxrs finds `nxrt.lib` beside itself), `cargo test -p cli`.

## The build step

| Step | What |
|---|---|
| Package | `translator::package::open_game` (folder, NSP, NUP; the update chosen); one line per module (layout base, text size, build id) |
| Profile | `profiles::lookup(title id, main build id)`; its build-time patches become the translator's patch table |
| Translate | `translator::aot::generate` into `<name>_build\gen`, `jobs` threads; skipped when nothing it reads changed (`gen\nxrs.gen.key`: this executable, the ExeFS, the patch table, the options) |
| Data | `Game::write_data` into `<name>_data` (the RomFS marker, ExeFS, control.nacp, icon); `<name>.ini` when missing |
| Shaders | lane C's `shaders::prebuild`, once published in docs/INTERFACES.md; until then a logged skip |
| Compile | every chunk, table and `modules.c` with clang, `jobs` at a time (default 4), the largest first |
| Link | `<name>_code.dll` (objects + `nxrt.lib`) and `<name>.exe` (the loader) |

**Compiler.** clang for `x86_64-pc-windows-msvc` (the Rust runtime's ABI), found as `NXRS_CLANG`, `--cc` when it is a
clang, the bundled `clang\bin` beside nxrs (or `..\toolchain\clang\bin`), `NXRT_LLVM` (as `nxrt-dev`), `C:\Program
Files\LLVM\bin`, then PATH. Flags: mkw-recomp's game-code set, `-std=gnu11 -O1 -fno-strict-aliasing -Wno-return-type
-Wno-unused-label -fvisibility=hidden -march=x86-64-v3 -fno-vectorize -fno-slp-vectorize -fomit-frame-pointer -g0`,
plus `-fms-runtime-lib=dll -w`.

**Chunk cache.** One object per unit in `<name>_build\obj`, with a `.key` beside it: sha256 of the compiler's id
(`--version` without its folder), every header the units can include (`gen\include\*.h`, `layout.h`, `patches.h`),
the flags with the build folder's path taken out, and the C. A unit whose key matches is not compiled; a changed
chunk recompiles alone; objects no unit produces any more are deleted. A compiler crash (as opposed to an error) is
retried twice: this machine has crashed compilers at random under parallel load.

**The runtime is linked statically into `<name>_code.dll`, not built as `nxrt.dll`:**
- generated code calls `rt_dispatch` on every indirect branch and every call outside its chunk: a direct call inside
  one image, an import-table call across two;
- code and runtime can never disagree on the ABI (no load-time check needed);
- the dispatch table's 32-bit slots need all lifted code within 4 GiB, and the fault handler maps host PCs to guest
  functions: one PE image guarantees both;
- a runtime change relinks only (every object is reused: about a second for MK8D).

`<name>.exe` holds no runtime: it loads `<its name>_code.dll` from its folder and calls `nxrs_game_main(argc, argv)`
(exported by `modules.c`), with `NXRS_DATA` and `NXRT_EXEFS` set to its data folder unless set already. The same
11 KB file serves every game.

**MK8D** (Mario Kart 8 Deluxe 1G+1U+1D NSP, update 4.0.1: 98,570 functions, 124 chunks, 339 MB of C; Core Ultra 9
285K, clang 21.1.8):

| Build | Time |
|---|---|
| cold, `--jobs 4` (the default) | 121.8 s (translate 7.0, compile 114.1, link 0.5) |
| cold, `--jobs 8` (what the launcher passes) | 70.5 s (translate 4.9, compile 64.9, link 0.6) |
| one chunk changed | 6.5 s (1 of 125 units compiled in 5.8 s, link 0.5) |
| nothing changed | 0.75 s |
| Turbo, same launcher command (gcc -O2, `--jobs 8`) | 353 s |

The DLL is 80 MB and imports only system DLLs and the MSVC runtime (`VCRUNTIME140.dll`, the UCRT). Pokémon Legends:
Z-A (Switch 2 Edition NUP, 270,366 functions, 1.05 GB of C, with the profile's 60 FPS patch) builds cold in 233.7 s at
`--jobs 8`. The eight runtime homebrew tests (`src/runtime/guest/tests`) built by `nxrs build` and run through the
loader all pass.

## Modules

| File | What |
|---|---|
| `main.rs` | argument parsing: contract v1 (Turbo's options) and v2 (subcommands); update-info, DLC, doctor, translate |
| `pipeline.rs` | `build`: the steps above, the output layout, `modules.c` |
| `cc.rs` | clang discovery, flags, the chunk cache and compile pool, the DLL and loader links, the loader's C |
| `profile.rs` | the build's profile line and patch table; `nxrs profile` |
| `out.rs` | console lines (CRLF, flushed), `-q`, the v2 `@progress` / `@result` lines |
| `launcher/switchrecomp.py` | the v1 shim the launcher's `recomp_cli.py` imports: runs `nxrs.exe` |

## Not done

- Linking needs the MSVC libraries (Visual Studio Build Tools) because the runtime is an MSVC-ABI Rust library. A
  player's machine without them needs the runtime built for `x86_64-pc-windows-gnullvm` and llvm-mingw bundled.
- The shader step waits for lane C's published `shaders::prebuild`.
- `doctor` does not check the Vulkan device version or free disk space.
- The loader is a console program (the runtime logs to stdout); a windowed one waits for the front end.
