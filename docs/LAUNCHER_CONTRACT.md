# Launcher contract

How RecompLauncher drives the engine. v1 is what the launcher does today with Turbo's `switchrecomp.py`; nxrs
answers it unchanged. v2 is the proposal for a launcher that runs nxrs directly.

## v1 (today)

### How the launcher starts the engine

The launcher runs `<python> -u <engine>\recomp_cli.py <args>` (working directory: the engine folder; stdout and
stderr merged; environment `PYTHONUTF8=1`, `PYTHONUNBUFFERED=1`, `SWR_PROD_KEYS=<keys>`, its tool folders on `PATH`).
`recomp_cli.py` imports `switchrecomp` from beside itself and returns `switchrecomp.main()`. For nxrs, that module is
`src/cli/launcher/switchrecomp.py`: it runs `nxrs.exe` (beside it, `bin\nxrs.exe`, or `NXRS_EXE`) with the same
arguments and returns its exit code. Cancel kills the tree (`taskkill /PID <python> /T /F`), nxrs with it.

An engine folder the launcher accepts (`Toolchain::problem()`) holds `switchrecomp.py` (the shim), a Python, a file at
one of its gcc paths (`gcc\bin\gcc.exe`, …) and `bin\runtime_win64_win32.exe`. nxrs uses neither of the last two:
`--cc` and `--runtime` are ignored. A release puts the bundled clang and the loader there to satisfy the check.

### The three invocations

| What | Arguments (`buildmanager.cpp`, `library.cpp`) | nxrs |
|---|---|---|
| Build | `<source> --outdir <buildDir> -o <outputName> --target win64 --backend win32 --cc <gcc> --jobs <n> -O <0-3> --runtime <exe> [--no-update \| --update <pkg>]` | builds; `--target --backend --cc -O --runtime` (and `--native --chunk`) are said as ignored in one line |
| Update probe | `<source> --update-info <pkg>...` | `UPDATE` lines |
| DLC | `--sync-dlc [<pkg>...] --datadir <buildDir>\<name>_data` | the translator's DLC API |

Also accepted, as Turbo: `--install-dlc`, `--remove-dlc`, `--list-dlc`, `--datadir` / `--outdir -o`, `--no-build`,
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

### Verified against Turbo (2026-11-19)

The launcher's own argument lists were run against Turbo (`py switchrecomp.py`, Switch2RECOMP-main) and nxrs, each
into `build\cli\` (never the owner's build folders), and through the launcher's `recomp_cli.py` + the shim:

| Case | Result |
|---|---|
| `--update-info`: an update of the game; a package that is no update; both together; no game | byte-identical output and exit codes (0, 1, 1, 0) |
| `--sync-dlc` / `--remove-dlc` / `--list-dlc` / `--sync-dlc` with no package / no `--datadir` | identical, except the install lines (below) and the argument-error text |
| Build MK8D (1G+1U+1D NSP, `--jobs 8`) | exit 0 both; the launcher-read lines all present; `_data` byte-identical (marker, control.nacp, icon, every ExeFS file); `dlc.manifest` identical after a sync |
| Build errors: an update of another game, a missing input | exit 1 both; the package refusal is the same line |
| Build of a homebrew folder (the runtime's `hello` test) | exit 0 both; the launcher-read files present |
| Through `recomp_cli.py` + `switchrecomp.py` shim | same output, same exit codes |

Intentional differences:

1. Banner `nxrs - use your own copy…` instead of Turbo's; one `nxrs: ignored (…)` line after it.
2. Module lines show the translator's layout (`7000000000`…) and each build id, not `.eh_frame functions`; a
   `Profile:` line; the translator's per-module discovery and C summaries, `C code: … functions` (not `entries`);
   `patch applied: …` for the profile's build-time patches (Turbo printed the same words for its built-in ones).
3. `C generated: n/m chunks` counts the translator's chunks (124 for MK8D, Turbo 160); an unchanged program says
   `(unchanged since the last build)` and is not translated again.
4. `Compiling with clang.exe 21.1.8 (-O1, N at a time)` (mkw-recomp's flag set; `-O` is ignored), plus a
   `compile: k of n units compiled, the rest reused` line and a `time:` line.
5. Shaders: a `shaders:` line (the shader lane's prebuild is called once it is published); no `tools\nxshader\`,
   no `<name>_nvn\` pipeline seed, no `.hints` / `.hints.for` (nxrs has no interpreter fallback).
6. A dumped folder's RomFS is read in place (`romfs.dir` marker), not copied into `_data\romfs\`.
7. Add-on content encrypted with AES-CTR is read in place (`dlc\<id>\romfs.nsp` marker, `Reading … in place`) instead
   of decrypted by hactool into `romfs.bin` (`Decrypting …`); nxrs needs no hactool.
8. The first `<name>.ini` has a different comment; Turbo's runtime rewrote it at first start anyway.
9. Argument errors are clap's text (exit 2 as argparse).
10. Without `--outdir`, the default is the launcher's `SwitchRecomp Builds\<name>` beside the input (Turbo: the
    working directory).
11. A missing input says `Cannot load the game: <path> does not exist`, before the banner (Turbo: `Cannot find
    exefs/main in <path>` after it); the launcher checks the source before it starts a build. Package errors are the
    same lines (e.g. `Cannot load the game: <update>: this update is for <id>, not for this game (<id>)`).

## v2 (proposed: versioned, no Python)

The launcher runs `nxrs.exe` itself; Python leaves the build path.

- **Version.** `nxrs contract` (and `nxrs --contract`) prints `nxrs-contract 2`. A launcher that finds no such line
  falls back to v1. The number only grows; a v2 launcher reads every 2.x.
- **Subcommands** (implemented): `nxrs build <input> -o <name> --outdir <dir> [--jobs N] [--update <pkg> |
  --no-update] [--keys <file>] [--machine]`, `nxrs update-info [--game <input>] <pkg>...`, `nxrs dlc install | sync
  | remove | list --datadir <dir> [...]`, `nxrs profile`, `nxrs doctor`.
- **Machine lines** (with `--machine`, implemented for `build`): `@progress\t<stage>\t<done>\t<total>` with stages
  `open`, `analyze`, `translate`, `compile`, `link`; one final `@result\tok\t<exe>` or `@result\terror\t<reason>`. Human
  lines stay as they are, so the log reads the same.
- **`doctor`** (implemented): AVX2/FMA/BMI2, clang, `nxrt.lib`, the Vulkan loader, keys present; exit 1 on a failure.
  To add: the Vulkan 1.3 device and driver version, free disk space.
- **`engine.json`** beside `nxrs.exe`: `{ "contract": 2, "engine": "nxrs", "version": "…" }`, so the launcher finds the
  engine without starting it; `Toolchain::problem()` then checks `nxrs.exe`, clang and `nxrt.lib` instead of
  `switchrecomp.py`, Python, gcc and the runtime exe.
- **Per-game engine.** A `library.json` game field `"engine": "turbo" | "nxrs"`; an nxrs build of a game Turbo also
  builds goes to `<dump>\SwitchRecomp Builds\<name> [nxrs]\` so both can exist side by side.
- **Unchanged from v1:** the output layout, the data folder, the `UPDATE` and `DLC` lines, exit codes 0 / 1 / 2.
