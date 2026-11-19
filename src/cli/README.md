# cli (`nxrs-build`)

The engine's command line: the build (package → data folder → C → shaders → code DLL + loader), updates, DLC, and
the launcher contract (v1 drop-in for Turbo's `switchrecomp.py`, v2 subcommands): [docs/LAUNCHER_CONTRACT.md](../../docs/LAUNCHER_CONTRACT.md).

    nxrs-build build <input> [-o <name>] [--outdir <dir>] [--jobs 4] [--update <pkg> | --no-update] [--keys <file>]
                     [--no-build] [--no-shaders] [--chunk N] [--native] [-q] [--machine]
    nxrs-build update-info [--game <input>] <package>...
    nxrs-build dlc install | sync | remove | list --datadir <dir> [package or title id...]
    nxrs-build profile [<title id> [<main build id>]] [--package <input>]
    nxrs-build doctor | contract
    nxrs-build translate <input> --out <dir>                      (development: C + data folder, for nxrt-dev e2e)
    nxrs-build <input> --outdir <dir> -o <name> ...                (contract v1: Turbo's options)

Build and test: `cargo build --release -p cli -p runtime -p front` (`nxrs-build.exe` finds `nxfront.lib` and `nxrt.lib` beside
itself), `cargo test -p cli`. Input: a folder, NSP, XCI, NUP or a homebrew NRO.

## The build step

| Step | What |
|---|---|
| Package | `translator::package::open_game` (folder, NSP, XCI, NUP, NRO; the update chosen); one line per module (layout base, text size, build id) |
| Profile | `profiles::lookup(title id, main build id)`; its build-time patches become the translator's patch table |
| Translate | `translator::aot::generate` into `<name>_build\gen`, `jobs` threads; skipped when nothing it reads changed (`gen\nxrs.gen.key`: this executable, the ExeFS, the patch table, the options) |
| Data | `Game::write_data` into `<name>_data` (the RomFS marker, ExeFS, control.nacp, icon); `<name>.ini` when missing |
| Shaders | `shaders::prebuild_data(<name>_data)`: every shader found and translated into `<name>_data\shaders\` (an unchanged one is reused); the profile's nxnative shader tables into `shaders\profile\` |
| Compile | every chunk, table and `modules.c` with clang, `jobs` at a time (default 4), the largest first |
| Link | `<name>_code.dll` (objects + `nxfront.lib`, the whole engine, for a retail title; `nxrt.lib` for the runtime's homebrew) and `<name>.exe` (the loader) |

**Compiler.** With Visual Studio's C++ build tools installed: clang for `x86_64-pc-windows-msvc` (the engine's ABI),
found as `NXRS_CLANG`, `--cc` when it is a clang, the bundled `clang\bin` beside nxrs (or `..\toolchain\clang\bin`),
`NXRT_LLVM` (as `nxrt-dev`), `C:\Program Files\LLVM\bin`, then PATH. **Without Visual Studio** (a player's machine):
the bundled llvm-mingw (`llvm-mingw\bin` beside nxrs, `NXRS_LLVM_MINGW`) for `x86_64-w64-windows-gnu`, linking the
engine built for `x86_64-pc-windows-gnullvm` (`gnu\libnxfront.a`, `gnu\libnxrt.a`, `gnu\libwindows.0.52.0.a`;
`NXRS_GNU_LIBS`) with compiler-rt and libunwind statically: the DLL imports only system DLLs and the UCRT.
`NXRS_TOOLCHAIN=msvc|gnu` forces one; by default MSVC is used only when Visual Studio, a clang and
`nxfront.lib` are all there, else llvm-mingw (a release may ship only the gnullvm engine). Build the gnullvm engine with llvm-mingw on PATH:
`rustup target add x86_64-pc-windows-gnullvm`, then `CC_x86_64_pc_windows_gnullvm=x86_64-w64-mingw32-clang
CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-L native=build\tools\gnu-libs" cargo build --release --target
x86_64-pc-windows-gnullvm -p front -p runtime` (`gnu-libs`: os's codec archives as `lib<name>.a`, and the windows
crate's import library). Flags: mkw-recomp's game-code set, `-std=gnu11 -O1 -fno-strict-aliasing -Wno-return-type
-Wno-unused-label -fvisibility=hidden -march=x86-64-v3 -fno-vectorize -fno-slp-vectorize -fomit-frame-pointer -g0 -w`,
plus `-fms-runtime-lib=dll` on MSVC; `--native` builds for `-march=native` instead. A retail title's code is compiled
with os's `cpu.h` overlay first (SVCs, the counter and spin waits to the os layer).

**Turbo's options (contract v1).** Every `switchrecomp.py` option is accepted: `--chunk N` (ARM instructions per C
file; the default lets the translator size chunks), `--native` (`-march=native`), `--no-build`, `--no-shaders`,
`--keep-c` (the C is always kept), `--dlc-stub`, the DLC, update and `--update-info` options, `-q`, `--build-runtime`
(checks the engine libraries and the compiler). Launcher-agreed replacements (docs/LAUNCHER_CONTRACT.md): `-O`
(one measured flag set), `--cc` when it is not a clang, `--runtime` (the engine is linked into the DLL),
`--backend` (Windows only) are said as ignored in one line; `--target linux` (M10) and `--interp-only` (no
interpreter: every function is translated) fail with the reason. `nxrs-build doctor`: CPU, toolchain, compiler, Visual
Studio, engine libraries, the Vulkan loader, every Vulkan device with its API and driver version (1.3 needed), free
disk for builds and temporary files, keys.

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
(exported by `modules.c`), with `NXRS_DATA` and `NXRT_EXEFS` set to its data folder unless set already. It is a
windowed program (`/SUBSYSTEM:WINDOWS`): no console opens; a failure to load shows a message box (or goes to stderr
when it is redirected). The same 11 KB file serves every game.

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
`--jobs 8`. The eight runtime homebrew tests (`src/runtime/guest/tests`) built by `nxrs-build build` and run through the
loader all pass.

## Mods

The player's mods go in `<build folder>\mods\<mod>\` (`NXRS_MODS=<folder>` names another), in the layouts Atmosphère,
Ryujinx and yuzu use: `exefs\<build id>.ips` (IPS or IPS32), `exefs\*.pchtxt` (IPSwitch, `@nsobid-...`),
`exefs_patches\<x>\...`, loose patch files in the mod folder, `romfs\...` (files that replace or join the game's), and an
optional `mod.json` (`{"name": "...", "enabled": false}`). The build (`profiles::mods` parses, `mods.rs` plans):
- every changed instruction word in a module's `.text` becomes a site of the translator's patch table, one set per mod
  (`mod_<folder>`, switch `NXRS_MOD_<FOLDER>`): both instructions are lifted, so a mod is turned off or on without a
  rebuild: `NXRS_MOD_<FOLDER>=0` / `=1`, or a file named `disabled` in the mod's folder (modules.c checks it at start);
- bytes outside `.text` (data, constants) and the RomFS folder go to `<data>\mods.tsv`; the run writes an enabled mod's
  bytes into the loaded modules before the game starts and layers its RomFS files over the game's (src/os `mods.rs`);
- a word the profile (or an earlier mod) patches already is left out with a line; a patch for a build id no module
  has is left out with a line. Adding or removing a mod's code patches retranslates the game once.

Mods are the player's files: nxrs never ships or downloads one.

## Modules

| File | What |
|---|---|
| `main.rs` | argument parsing: contract v1 (Turbo's options) and v2 (subcommands); update-info, DLC, doctor, translate |
| `pipeline.rs` | `build`: the steps above, the output layout, `modules.c` |
| `cc.rs` | clang discovery, flags, the chunk cache and compile pool, the DLL and loader links, the loader's C |
| `profile.rs` | the build's profile line and patch table; `nxrs-build profile` |
| `mods.rs` | the player's mods (below) |
| `out.rs` | console lines (CRLF, flushed), `-q`, the v2 `@progress` / `@result` lines |
| `launcher/switchrecomp.py` | the v1 shim the launcher's `recomp_cli.py` imports: runs `nxrs-build.exe` |

## Not done

- Release packaging lives in the launcher (`scripts/package-release.py --nxrs-engine <this checkout>`: `nxrs-engine\` with
  nxrs-build, `engine.json`, the MSVC and gnullvm engine libraries, and `toolchain\llvm-mingw\`); CI cannot build
  this engine until it has a remote.
- `--target linux` (M10).
