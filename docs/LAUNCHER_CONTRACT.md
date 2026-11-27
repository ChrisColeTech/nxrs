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
| Built game | `<name>_data\romfs.source` (the record of the extracted RomFS: the text the old in-place marker had, so line 2 of `SWRNSP1`/`SWRNSP2` or line 1 with a `marker romfs.nup` row is the package, an `SWRNSP2` `update <path>` row the applied update; then `marker`, `extracted`, `stamp` rows), else, for a build made before 2026-11-26, `romfs.nsp` / `romfs.nup` read the same way; `control.nacp` (+0x3060 display version), `dlc.manifest` |
| Errors | anything on stderr is shown; the exit code decides |

Lines end in CRLF (Python's text mode); the launcher trims them. Exit codes: 0 done, 1 failed, 2 bad arguments.

### Output layout

```
<outdir>\<name>.exe              loader (loads <name>_code.dll beside it)
<outdir>\<name>_code.dll         the game's code + the runtime (nxrt.lib, static)
<outdir>\<name>_data\            exefs\, romfs\ (the game's RomFS extracted: decrypted, the update applied) or
                                 romfs.bin (its image, when its names cannot be files in a Windows folder),
                                 romfs.source, control.nacp, icon, dlc.manifest + dlc\<id>\romfs\
<outdir>\<name>_data\save, sdmc, cheats    the player's files (the game writes them; kept on Remove, below)
<outdir>\mods\                   the player's mods
<outdir>\<name>.ini              written when missing; the launcher then writes the game's settings into it
<outdir>\<name>_build[_v<N>|_base]\gen, obj   generated C and objects, kept for rebuilds
```

`<outdir>` defaults to `<the input's folder>\nxrs\<name>` (the launcher always passes it: the same folder, or
`<Build location>\nxrs\<name>` when Settings > Build location is set).

**A built game carries its own data** (owner, 2026-11-26): the build writes the RomFS into the data folder, so the
game plays with its package (and update and DLC packages) moved or deleted. A rebuild writes it again only when
`romfs.source` changed (another package, update, or package file). Disk use is accepted.

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
6. The game's RomFS, from any source (package, update, NUP, dumped folder, NRO), is extracted into `_data\romfs\`
   (`RomFS: extracting …`, `RomFS: N of M MB written` every few seconds, `RomFS: extracted into romfs/ (…)`; with
   `--machine` also `@progress\tdata\t<MB>\t<MB>`), or `RomFS: romfs is current` when `romfs.source` is unchanged.
   Builds made before 2026-11-26 have an in-place marker instead (`romfs.nsp` / `romfs.nup` / `romfs.dir`), which the
   runtime still reads until the game is built again.
7. Add-on content is extracted into `dlc\<id>\romfs\` (`Extracting <id> (… MB)...`, storage `folder` in
   `install.json`), decrypted by nxrs itself (no hactool); an NSZ / XCZ title is unpacked into `romfs.bin`. A title
   installed before as an in-place `romfs.nsp` marker (storage `package`) is extracted at its next `dlc sync`.
8. The first `<name>.ini` carries nxrs's own comment.
9. Argument errors are clap's text (exit 2 as argparse).
10. Without `--outdir`, the default is `nxrs\<name>` beside the input (the launcher's folder).
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
| Build > Web | the same with `--outdir <webDir>`, then `--shaders-from <buildDir>\<name>_data` (when the game has a build folder) and `--target web` last (`model::args::build_web`); below |
| Update probe | `update-info --game <source> [--keys <prod.keys>] <pkg>...` |
| DLC | `dlc sync --datadir <buildDir>\<name>_data [--keys <prod.keys>] [<pkg>...]` (Play, Install DLC, Remove, after a build) |
| Version | `contract` |
| Run-time shader (the web server) | `shader-xlate <webDir>\<name>_data\shaders`, the request on stdin, the key on stdout (Web builds, below) |
| Pad output (the web server) | `pad-bridge`, the page's pad lines on stdin, its log on stdout and stderr (Web builds, below; added 2026-11-27, WEB-PADBRIDGE) |

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
  another value or none loads as nxrs and is pointed at an nxrs folder of its own (`<base>\nxrs\<name>`, base = the
  folder above its old build folder, or above `SwitchRecomp Builds`), so it shows as needing a build until it is built
  there; the other engine's build is never touched. `nxrs.exe --engine` accepts only `nxrs`.
- An nxrs build goes to `<source folder>\nxrs\<name>\` (owner, 2026-11-26; `<Build location>\nxrs\<name>` when that
  setting is set); the entry reads what is built there (source and applied update from `romfs.source`).
- **Migration** of the layout before (`<source folder>\SwitchRecomp Builds\<name> [nxrs]`): `nxrs.exe
  --migrate-builds [--select-title <title>] [--library <file>]` (prints a line an entry, exit 1 when one was left),
  or Settings > Build defaults > Old build folders > Move. It runs only when asked, never at start (other tools and
  agents use build folders). Each such folder is renamed to `<source folder>\nxrs\<name>` and its entry's
  `buildDir` updated; a game running or building (process list, or its executable / module open), a folder whose new
  place exists, or a rename that fails is left and reported, and retried next time. `library.json` is copied to
  `library.pre-layout.json` before the first write (and to `library.qt-backup.json` by the Qt rule). A `SwitchRecomp
  Builds` folder is removed only when the moves leave it empty; other engines' builds in it stay.
- **A built game never needs its source to play**: "Source offline" applies only to a build with an in-place marker
  (made before 2026-11-26).
- **Remove from library** asks with a tick box, ticked: *Also delete the build (N GB)*. Ticked, the player's files
  (`<name>_data` and `<name>_p<N>_data`: `save\`, `sdmc\`, `cheats\`; the build's `mods\`) are moved to
  `<launcher data folder>\saves\<title id>\` (`data\…`, `p<N>\…`, `mods\`; the title id from `exefs\main.npdm`, else
  the output name; an older kept folder of the same game is renamed aside, never merged), then the build folder alone
  is deleted (refused while the game runs or builds, when the folder holds the source package or is not a build of the
  game). The package is never deleted. Unticked, no file changes. **Restoring** is the launcher's: after a successful
  build, a kept folder of that game is moved back into the build where the build has no such folder of its own (an
  empty one is replaced), then removed; the engine reads saves only from `<name>_data\save` as before.
- **Import build** / discover: a build whose `<name>_build*\gen\nxrs.gen.key` exists is an nxrs build (the entry gets
  `"engine": "nxrs"`).
- **Play**: no Python, .NET or `tools\nxshader` checks (nxrs translates shaders in the build). The DLC check before
  Play and the INI settings (renderer, button layout) apply.

### Web builds (`--target web`, docs/35 W5)

**One Play, one Build** (owner, 2026-11-27; WEB-W5b). The Game menu, the game's context menu, the card's primary
button, Enter / double-click, Ctrl+B and the Updates window's Build now each have one Play or one Build, and each
opens the Native / Web chooser: a small dialog titled Play or Build, its heading the game, a line per target saying
what it does or why it cannot run now, and two equal buttons, **Native** and **Web**, that act at once (the header
X or Escape dismisses it). Play is enabled when either target can run, Build when either can build (`App::why_not`
is the one rule). Play's reasons: no native build yet; no web build yet; no Chromium browser found; the game is
running or building. Build's: the native build is up to date (Native); another build is running; the source is
missing; nxrs-build cannot build. Native runs the existing build and play; Web the paths below.

A browser build of the same game (Chrome is the target), built by Build > **Web** (`nxrs.exe --select-title
<title> --build-web` headless) into the game's web folder, `<webDir>` =
`<base>\nxrs-web\<outputName>` beside its `<base>\nxrs\<outputName>`. The native build is not touched and the library
entry does not change (no `builtSource`, no INI, no DLC sync). The run's lines, `@progress` and `@result` are a
build's; its log is `<webDir>\<outputName>.build.log`. Success: exit 0 **and** `<webDir>\build.json` and
`<webDir>\game.wasm` exist.

`--shaders-from <data folder>` (WEB-W7): the translated shaders of another build of the title are imported into
the web build's library (every complete record its `shaders\spv\` and `shaders\live\<key>\` hold that the web library
lacks). A title that makes its shaders at run time (Ren'Py, Unity variants) has none for the build to find, and a
browser cannot run the translator, so its web build draws only the shaders its native build's runs translated. A
folder without a library is a line in the log, not a failure.

**Run-time shaders on request** (WEB-W7b). A shader the page's library lacks is asked for with
`POST /<slug>/shader-xlate/<key>` (`<key>`: 16 hex digits; body `u32 LE ctl length | ctl | code`, the blobs the
native live translator is given, at most 4 MiB). The launcher's server runs `nxrs-build shader-xlate <data>\shaders`
(contract v2: the body on stdin, fixed arguments, working directory nxrs-build's folder, 120 s limit) with the
nxrs-build it builds with (Play in browser: the toolchain's; `--serve-web`: `NXRS_BUILD`, the release's
`nxrs-engine\`, beside the launcher). nxrs-build translates it exactly as a native run's live translator does
(`translate_one`: dmsh_xlate, fixes, spirv-val, the profile's facts) into `<data>\shaders\live\<key>\`, prints the key
and exits 0, or prints the reason on stderr and exits 1. Answers: `200` `u32 LE spv length | spv | json` (also when
`live\<key>\spv\` already holds both files: nothing is run); `400` a key that is not 16 hex digits or a body that is
not a request; `404` no such build; `413` a body over 4 MiB, refused unread (connection closed); `422` the
translation failed (nxrs-build's reason, or a key other than the one asked for); `503` no nxrs-build. One
translation per key at a time. Nothing outside the mount's `shaders\live\<key>\` is written, and nothing but
nxrs-build is run. The page also looks in `data/shaders/live/<key>/spv/` before asking (an earlier page's
translation); the next build promotes those records into the library's `spv\`. A static host has no route: the page
says so in a red line and skips the draws, as before. `scripts/web/dev_server.py` has the same route.

**Pad rumble and lights through the launcher** (WEB-PADBRIDGE, contract 2, added 2026-11-27). Chrome cannot rumble
every pad (the DualSense has no `vibrationActuator` on Windows; WebHID needs a permission) and a page cannot set a
pad's lights, so a page the launcher serves sends its pads' output to the launcher, which drives the pads natively:

- The page opens a WebSocket (RFC 6455; text frames) to `/<slug>/pad-bridge` on its own origin and sends one line
  per change and at least every 250 ms: `pad <active> <low 1> <high 1> <low 2> <high 2> <low 3> <high 3> <low 4>
  <high 4>`. `active` is 1 while the game owns input (the page focused and visible, its menu closed: what a native
  run takes from its window's focus), else 0; the levels are the engine's input block words `[56..64)` (0..65535,
  already 0 when not active, the rumble settings applied). Zero at once on menu open, blur, hidden and `pagehide`.
- The launcher's server answers the upgrade `101` (`Sec-WebSocket-Accept`), `400` without a key, `404` for no such
  build, `503` when the mount has no nxrs-build (an imported web game) or it does not start. It runs `nxrs-build
  pad-bridge` (fixed arguments, working directory nxrs-build's folder, stdout and stderr into `<webDir>\pad-bridge.log`,
  written anew each connection), writes each well-formed line to its stdin (others are dropped), answers ping with
  pong, and ends on a close frame, a broken socket or 5 s without a frame: stdin is closed and the bridge has 2 s to
  end before it is killed.
- `nxrs-build pad-bridge` (Windows; elsewhere it says so and exits 1) runs the engine's input layer as a native run's
  input thread does (`src/os/src/input/bridge.rs`): its scan, player slots (the first pad is player 1), player lights
  while `active` (DualSense player LED and lightbar, DualShock 4 lightbar, Nintendo player LEDs), and each player's
  two levels through the native output (DualSense / DualShock 4 output reports, XInput, Nintendo standard rumble).
  It never takes a pad from the browser: devices are opened shared, Sony pads on Bluetooth (whose output would switch
  them to reports the browser cannot read) and Switch 2 pads (a BLE connection) are left alone, HD rumble is off (the
  page carries the two standard levels). No line for 1 s: motors 0 and lights off until the next line. End of stdin:
  motors 0, lights off, devices closed, exit 0. Each output report it writes is in its log with its bytes; a failed
  write is logged with the Windows error.
- Without the route (a static host, `dev_server.py`, a launcher or nxrs-build without the verb) the socket does not
  open or closes, and the page plays rumble itself as before (`vibrationActuator`, WebHID for the DualSense).

```
<webDir>\index.html, boot.js, input.js, saves.js, io-worker.js, audio-worklet.js   the page and its workers
                                 (embedded in nxrs-build: a release needs no source tree; the list is webhost.rs's
                                 HOST_FILES)
<webDir>\game.js, game.wasm      the Emscripten loader and the module (engine + game)
<webDir>\data.json, bootset.json the data manifest and the boot set the page prefetches (webpack.rs)
<webDir>\build.json              what the folder is (below)
<webDir>\<outputName>_data\      the game's data, as a native build's; served, never copied
<webDir>\<outputName>_build\     generated C and objects, kept for rebuilds
```

`build.json` (`format` 1): `{"format", "kind": "nxrs-web", "title", "titleId" (16 hex or null), "engine": {"name",
"contract", "version"}, "entry": "index.html", "data": "<outputName>_data", "files": [{"path", "size", "role"}]}`;
roles `host`, `loader`, `module`, `manifest`, `bootset` (later waves add others; readers ignore roles they do not
know, and refuse a newer `format`).

**Serving.** Any static server that sends `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp` and answers byte ranges exactly (206 + `Content-Range`) can serve the
folder, with `data/<path>` mapped to the data folder `build.json` names. The launcher's own server
(`services/web_serve.rs`, in its process, 127.0.0.1) mounts each web build at `/<slug>/` (the output name in
lower case, `-` between words) on port 8780 (`NXRS_WEB_PORT`; the next of ten when it is taken), one fixed port so
the page's origin, and with it the browser's data cache and saves, stays the same. `nxrs.exe --serve-web <webDir>`
serves one folder without a window; `scripts/web/dev_server.py` (engine development) does the same.

Play > **Web** can run only when the web folder holds a finished web build and a Chromium browser exists:
the default browser when its `http` ProgId is Chrome, Edge, Brave, Chromium, Vivaldi or Opera, else Chrome by path
(App Paths, then Program Files, Program Files (x86), the user's local data; Linux: `google-chrome`,
`chromium` on PATH). It starts the server (once), mounts the build (with the toolchain's nxrs-build for run-time
shaders) and opens its address in a tab; Chrome's
"Install as app" gives it an app window. The server lives as long as the launcher: closing the launcher stops the
game's data streaming.

### Release layout (`scripts/package-release.py --nxrs-engine <engine checkout>`)

```
nxrs.exe                         the launcher
nxrs-engine\nxrs-build.exe, engine.json
nxrs-engine\nxfront.lib, nxrt.lib                       MSVC engine (used when Visual Studio + a clang are installed)
nxrs-engine\gnu\libnxfront.a, libnxrt.a, libwindows.0.52.0.a   gnullvm engine
toolchain\llvm-mingw\                                   x86_64 part of llvm-mingw (no Visual Studio needed)
vcruntime140.dll, vcruntime140_1.dll, msvcp140.dll      beside nxrs.exe, nxrs-build.exe and tools\spirv-val.exe
legal\licenses\nxrs-engine\                             the engine's LICENSES\
```

nxrs-build chooses MSVC only when Visual Studio, a clang and `nxfront.lib` are all there (`cc::use_gnu`), else the
bundled llvm-mingw with `gnu\`: a release may ship only the gnullvm set. `.github/workflows/release.yml` builds both
(`scripts/build/build.ps1`), so the zip builds games with nothing installed.

Linux (`nxrs-linux-x64.tar.gz`): `nxrs`, `nxrs-engine/` (`libnxfront.a` with opus, libvpx, dav1d and FFmpeg's H.264 /
AAC decoders inside, `NXRS_CODECS`), and `toolchain/clang/` (LLVM's clang and lld, a glibc sysroot; nxrs-build's
`../toolchain/clang/bin/clang`), all from `scripts/build/linux-deps.sh`. A game links only ALSA and the C library
of the system.

### Not done

Nothing of v2 (2026-11-20). Done since: the launcher runs `doctor` (Settings > Engine, `nxrs --nxrs-doctor`; launcher
2d3ea0d); releases bundle `nxrs-engine/` for Windows and Linux by default (b3cd6a5); nxrs games are Vulkan-only
(b3cd6a5). An arm64 Linux engine bundle is built by the engine's CI (`nxrs-engine-linux-arm64`); the launcher has no
arm64 release.
