# Launcher contract

How the nxrs launcher (`nxrs.exe`, formerly RecompLauncher) drives the engine's build tool, `nxrs-build.exe`. v1 is
the legacy launcher's command line, run through Python; nxrs-build answers it unchanged. v2 is what the launcher does
for every game: it runs nxrs-build itself, without Python (implemented on both sides, 2026-11-19).

## v1 (the legacy launcher's command line)

### How the launcher starts the engine

The launcher runs `<python> -u <engine>\recomp_cli.py <args>` (working directory: the engine folder; stdout and
stderr merged; environment `PYTHONUTF8=1`, `PYTHONUNBUFFERED=1`, `NXRS_PROD_KEYS=<keys>`, its tool folders on `PATH`).
`recomp_cli.py` imports `switchrecomp` from beside itself and returns `switchrecomp.main()`. For nxrs, that module is
the shim `src/cli/launcher/switchrecomp.py` (the file name is the one `recomp_cli.py` imports): it runs `nxrs-build.exe` (`NXRS_BUILD`, `NXRS_EXE`, beside it, or
`bin\nxrs-build.exe`) with the same arguments and returns its exit code. Cancel kills the tree (`taskkill /PID <python>
/T /F`), nxrs-build with it. The launcher no longer needs this path (it speaks v2); it stays for tools that drive
the shim.

An engine folder the legacy launcher accepts (`Toolchain::problem()`) holds the shim, a Python, a file at
one of its gcc paths (`gcc\bin\gcc.exe`, …) and `bin\runtime_win64_win32.exe`. nxrs uses neither of the last two:
`--cc` and `--runtime` are ignored. A release puts the bundled clang and the loader there to satisfy the check.

### The three invocations

| What | Arguments (`buildmanager.cpp`, `library.cpp`) | nxrs |
|---|---|---|
| Build | `<source> --outdir <buildDir> -o <outputName> --target win64 --backend win32 --cc <gcc> --jobs <n> -O <0-3> --runtime <exe> [--no-update \| --update <pkg>]` | builds; `--target --backend --cc -O --runtime` (and `--native --chunk`) are said as ignored in one line |
| Update probe | `<source> --update-info <pkg>...` | `UPDATE` lines |
| DLC | `--sync-dlc [<pkg>...] --datadir <buildDir>\<name>_data` | the translator's DLC API |

Also accepted: `--install-dlc`, `--remove-dlc`, `--list-dlc`, `--datadir` / `--outdir -o`, `--no-build`,
`--no-shaders`, `-q`, `--build-runtime` (checks `nxrt.lib` and clang, builds nothing). `--interp-only` and `--target
linux` fail (no interpreter, Windows only). nxrs adds `--keys` and `--contract`.

### What the launcher reads

| Kind | Exactly |
|---|---|
| Build success | exit 0 **and** `<buildDir>\<name>.exe`, `<name>_code.dll`, `<name>_data\` all exist (`Game::hasBuild`) |
| Progress (`parseLine`) | `(?:C generated:\|compiled)\s+(\d+)/(\d+)`, lines starting `Analyzing`, `Linking`, `RomFS:` |
| Package DLC | a line starting `DLC: this package also holds add-on content` (the launcher then adds the package to the game's DLC) |
| Update probe | `UPDATE\t<abs path>\t<title id, 16 lowercase hex>\t<version>\t<display>` per accepted package, `UPDATE error: <reason>` per refused one; exit 1 if any was refused |
| DLC | each `DLC error: <reason>` line is one refused package; exit ≠ 0 fails the step |
| Built game | `<name>_data\romfs.nsp` / `romfs.nup` (line 2 / 1: the package; `SWRNSP2` `update <path>` row: the applied update), `control.nacp` (+0x3060 display version), `dlc.manifest` |
| Errors | anything on stderr is shown; the exit code decides |

Lines end in CRLF (Python's text mode); the launcher trims them. Exit codes: 0 done, 1 failed, 2 bad arguments.

### Output layout

```
<outdir>\<name>.exe              loader (loads <name>_code.dll beside it)
<outdir>\<name>_code.dll         the game's code + the runtime (nxrt.lib, static)
<outdir>\<name>_data\            exefs\, romfs.nsp | romfs.nup | romfs.dir, control.nacp, icon, dlc.manifest + dlc\
<outdir>\<name>.ini              written when missing; the launcher then writes the game's settings into it
<outdir>\<name>_build[_v<N>|_base]\gen, obj   generated C and objects, kept for rebuilds
```

`<outdir>` defaults to `<the input's folder>\SwitchRecomp Builds\<name>` (the launcher always passes it).

### Output notes

1. Banner `nxrs - use your own copy…`; one `nxrs-build: ignored (…)` line after it.
2. Module lines show the translator's layout (`7000000000`…) and each build id; a `Profile:` line; the translator's
   per-module discovery and C summaries, `C code: … functions`; `patch applied: …` for the profile's build-time
   patches.
3. `C generated: n/m chunks` counts the translator's chunks (124 for MK8D); an unchanged program says
   `(unchanged since the last build)` and is not translated again.
4. `Compiling with clang.exe 21.1.8 (-O1, N at a time)` (mkw-recomp's flag set; `-O` is ignored), plus a
   `compile: k of n units compiled, the rest reused` line and a `time:` line.
5. Shaders: a `shaders:` line (the shader lane's prebuild is called once it is published); no `tools\nxshader\`,
   no `<name>_nvn\` pipeline seed, no `.hints` / `.hints.for` (nxrs has no interpreter fallback).
6. A dumped folder's RomFS is read in place (`romfs.dir` marker), not copied into `_data\romfs\`.
7. Add-on content encrypted with AES-CTR is read in place (`dlc\<id>\romfs.nsp` marker, `Reading … in place`) instead
   of decrypted by hactool into `romfs.bin` (`Decrypting …`); nxrs needs no hactool.
8. The first `<name>.ini` carries nxrs's own comment.
9. Argument errors are clap's text (exit 2 as argparse).
10. Without `--outdir`, the default is the launcher's `SwitchRecomp Builds\<name>` beside the input.
11. A missing input says `Cannot load the game: <path> does not exist`, before the banner; the launcher checks the
    source before it starts a build. Package errors are lines like `Cannot load the game: <update>: this update is for
    <id>, not for this game (<id>)`.

## v2 (the launcher runs nxrs-build; no Python)

Implemented in the launcher (D:\Projects\RecompLauncher, `src/launcher/library.cpp` + `buildmanager.cpp`) and in
nxrs-build (`src/cli`). Windows and Linux (`nxrs-engine/nxrs-build`, no `.exe`).

### Finding nxrs-build

The launcher looks, in order, at `NXRS_BUILD` (a file or its folder), the path chosen in **Settings > Engine >
nxrs-build** (`library.json` `"nxrsBuild"`, also `nxrs.exe --nxrs-build <exe>`; for development: the engine's
`target\<dir>\release\nxrs-build.exe`), then the release's `nxrs-engine\nxrs-build.exe` beside `nxrs.exe`.

**Version.** `engine.json` beside nxrs-build (a release writes it: `{"contract": 2, "engine": "nxrs", "version":
"<engine commit>"}`) is read without starting the engine; without it the launcher runs `nxrs-build contract` (10 s
limit) and reads `nxrs-contract <n>`. It needs contract 2; less is shown as the problem. The number only grows; a v2
launcher reads every 2.x.

**Ready to build** (`Toolchain::nxrsProblem()`): nxrs-build found and contract ≥ 2. The compiler and the engine
libraries are nxrs-build's to find (`nxrs-build doctor` says what is missing); the build fails with its reason when one
is.

### The invocations

The program is nxrs-build itself; working directory: its folder; stdout and stderr merged; environment as in v1
(`NXRS_PROD_KEYS` included); keys are also passed as `--keys` when the installed file exists.

| What | Arguments (`nxrsBuildArguments`, `nxrsUpdateInfoArguments`, `nxrsDlcSyncArguments`) |
|---|---|
| Build | `build <source> --outdir <buildDir> -o <outputName> --jobs <n> --machine [--no-update \| --update <pkg>] [--keys <prod.keys>]` |
| Update probe | `update-info --game <source> [--keys <prod.keys>] <pkg>...` |
| DLC | `dlc sync --datadir <buildDir>\<name>_data [--keys <prod.keys>] [<pkg>...]` (Play, Install DLC, Remove, after a build) |
| Version | `contract` |

Other subcommands (not run by the launcher): `dlc install | remove | list`, `profile`, `doctor`, `translate`.

### What the launcher reads

Everything v1 lists (the progress lines, the `DLC:` package line, `UPDATE` / `UPDATE error:` / `DLC error:` lines,
exit codes 0 / 1 / 2, the output layout and the data folder), plus, with `--machine`:

- `@progress\t<stage>\t<done>\t<total>`, stages `open`, `analyze`, `translate`, `compile`, `link` (the launcher
  shows `shaders` and `data` too if they come). Machine lines are not shown in the build console; the log file keeps
  them.
- One final `@result\tok\t<exe>` or `@result\terror\t<reason>`; the exit code still decides.

### Engine and the build folder

- Every game builds with nxrs. `library.json`'s game field `"engine"` is written as `"nxrs"`; an entry saved with
  another value or none loads as nxrs and is moved to its `<build folder> [nxrs]` folder, so it shows as needing a
  build until it is built there. `nxrs.exe --engine` accepts only `nxrs`.
- An nxrs build goes to `<build folder> [nxrs]` (e.g. `<dump>\SwitchRecomp Builds\<name> [nxrs]\`); the entry reads
  what is built there (source and applied update from the data folder's marker).
- **Import build** / discover: a build whose `<name>_build*\gen\nxrs.gen.key` exists is an nxrs build (the entry gets
  `"engine": "nxrs"`).
- **Play**: no Python, .NET or `tools\nxshader` checks (nxrs translates shaders in the build). The DLC check before
  Play and the INI settings (renderer, button layout) apply.

### Release layout (`scripts/package-release.py --nxrs-engine <engine checkout>`)

```
nxrs.exe                         the launcher
nxrs-engine\nxrs-build.exe, engine.json
nxrs-engine\nxfront.lib, nxrt.lib                       MSVC engine (used when Visual Studio + a clang are installed)
nxrs-engine\gnu\libnxfront.a, libnxrt.a, libwindows.0.52.0.a   gnullvm engine
toolchain\llvm-mingw\                                   x86_64 part of llvm-mingw (no Visual Studio needed)
legal\licenses\nxrs-engine\                             the engine's LICENSES\
```

nxrs-build chooses MSVC only when Visual Studio, a clang and `nxfront.lib` are all there (`cc::use_gnu`), else the
bundled llvm-mingw with `gnu\`: a release may ship only the gnullvm set.

### Not done

Nothing of v2 (2026-11-20). Done since: the launcher runs `doctor` (Settings > Engine, `nxrs --nxrs-doctor`; launcher
2d3ea0d); releases bundle `nxrs-engine/` for Windows and Linux by default (b3cd6a5); nxrs games are Vulkan-only
(b3cd6a5). An arm64 Linux engine bundle is built by the engine's CI (`nxrs-engine-linux-arm64`); the launcher has no
arm64 release.
