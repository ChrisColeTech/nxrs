# WEB-W7 - The second title in Chrome: Beach Bounce Remastered's black frames

Wave of `docs/35-WEB-TARGET-PLAN.md` W7 (the second-title part). Base `3d3794c`, branch `web-w7`, worktree
`D:\Projects\RecompLauncherEngine-web-w7`. Title: Beach Bounce Remastered (01004060145C0000), web build
`<source folder>\nxrs-web\Beach Bounce Remastered` (`-o "Beach Bounce Remastered"`). GPU 0, port 8775. Protocol:
`docs/31-DELEGATION-PROTOCOL.md`. Parallel wave WEB-W6 owns cc.rs, pipeline.rs, webpack.rs, webhost.rs, boot.js,
io-worker.js, dev_server.py and the launcher server.

## 1. Evidence, re-read

The brief's facts (176 draws over 154 passes, "no render target: draw skipped", D24S8 and A4B4G4R4 red lines, "no
font found") come from `build/WEB/W5/run2-launcher-play/page.log`. That file is not Beach Bounce: it names title
`0100C6801E0FA000`, a 936 MiB / 76-file data manifest and SDK 16, which is **Anime Girls** (a stale page log left
in the folder). Beach Bounce's own log of that run is `log.txt` beside it (title `01004060145C0000`, 317.2 MiB /
941 files, SDK 10), and it says:

- `replay up to frame 360, per frame: 0.0 draws (4.0 skipped: no shader 699 ...)`: **every draw is skipped for a
  missing shader**;
- the 16 keys it misses (`live shader 0a56e5906042ca31: dmsh_xlate not found`, ... `shader 317416f793e1251a is not
  in the shader library`) are exactly the 16 keys in the native build's `shaders/live/` (and promoted into its
  `spv/`): `SwitchRecomp Builds\Beach Bounce Remastered nsw2u [nxrs]\..._data\shaders`;
- the web build's `shaders/` is empty: the build's prebuild finds 0 shaders in this title (`shaders: 0 found`),
  because Ren'Py's shaders are made by the game at run time.

So W5's reading was right and the brief's hypothesis (a web-device present/format fault) is disproved for the black
frames by the title's own log. Natively the live translator (`graphics::vk::live`) runs `dmsh_xlate` (a native
NativeAOT executable) on a thread; a browser cannot run it, so on the web a run-time shader never arrives.

## 2. Plan

1. Rebuild the web library and the Beach Bounce web build from this worktree; run in Chrome (port 8775) and confirm
   from its own log that draws are still skipped for "no shader" (and nothing else).
2. **The fix: the web build carries the title's run-time shaders.** A build for the web cannot translate them
   while the game runs, so it imports those an earlier native build of the same title translated live: `nxrs-build
   build <nsp> --target web --shaders-from <native data folder>\shaders`. Every complete record there (a key with its
   SPIR-V and reflection, in `live/<key>/spv/` or the library's `spv/` with a manifest row) that the web library
   lacks is copied into `<web data>\shaders` with its manifest and pair rows, the way `promote_live_shaders` promotes
   a native build's own live records. The web build's data folder is then served as is (shaders on first use, W4).
   The launcher's **Build for web** passes the library entry's native data folder when it has one.
3. On the web, the live translator does not try to start `dmsh_xlate`: a missing shader's red line names the cause
   (no translator in the browser; build natively and run once, then Build for web) instead of "dmsh_xlate not
   found". The draw still waits/skips exactly as before (nothing hidden).
4. Then whatever the first rendered frames show (formats such as A4B4G4R4, present, Y flip), fixed in the web device
   with unit tests for any conversion.
5. Validation: menu and the first story scenes against the native sheet
   (`build/play/20261126-182220-web-w3-native/sheet-1.png`), keyboard input (W3's block), FPS against 60 over a
   stated window, audio running, red lines before and after.

## 3. Files expected

| File | Change |
|---|---|
| `src/cli/src/shaderimport.rs` (new) | the import of another build's translated shaders, with tests |
| `src/cli/src/main.rs` | `--shaders-from <dir>` parsed into `BuildArgs` |
| `src/cli/src/pipeline.rs` (W6's: minimal) | `BuildArgs::shaders_from`; one call after the prebuild |
| `src/graphics/src/vk/live.rs` | web arm: the missing shader's red line, no translator thread |
| `src/launcher/src/model/args.rs` (+ the flow that calls it) | Build for web passes the native data folder's shaders |
| `src/graphics/src/web/*` | only if the first frames show a web-device fault |
| `docs/LAUNCHER_CONTRACT.md`, `docs/35-WEB-TARGET-PLAN.md` | the flag; the W7 row |

## 4. Results (2026-11-26)

Evidence in the main checkout's `build/WEB/W7/` (Chrome 158 over CDP, own profile per run, cold OPFS, dev_server.py on
port 8775; keyboard X = A, W3's input block).

| Item | State | Evidence |
|---|---|---|
| Cause of the black frames | Found: every draw skipped for a missing shader (16 run-time shaders, no translator in the browser) | W5's `log.txt` (the `page.log` beside it is Anime Girls'); `run1/page.log`: 0 draws skipped after the fix |
| `--shaders-from` import (`shaderimport.rs`) | Built (`3887da3`) | `shaders: 16 imported from ...nsw2u_data`; test `shaderimport::tests::imports_what_the_library_lacks` |
| Web live translator | Built (`3887da3`): a missing shader is marked failed at once with a red line naming the cause (no 3 s wait per draw on a translator that can never answer) | |
| Launcher Build for web | Built (`02b223f`): `--shaders-from <buildDir>\<name>_data` before `--target web` | `model::web_tests` updated; launcher tests and clippy clean |
| False "no translated shaders" red line on the web | Fixed (`02b223f`): manifest looked for on the page's server | `run7-validate/page.log`: 0 red lines |
| Menu, then story scenes | Shown, same as native | `run7-validate/01-boot..04-min3.png` against `build/play/20261126-182220-web-w3-native/sheet-1.png`: title menu; Grandma Umi with the same line at min_1; Tomoyo in the hospital at min_2 (a line later than native's); Yuuki at the Beach Bounce desk with the same line at min_3 |
| Audio | Running | `audio: output, last 5 s: RMS ... -20.8 dBFS`; underruns counted (260 by 210 s in the loaded run7) |
| FPS | **55.6 mean** over 60-230 s (30 windows, 42.8-59.8), below the 60 goal | `run8-measure` (Mario Kart World running from another wave); `run7-validate` 45.0 while another wave's wasm-opt held ~22 cores; slow frames have replay 1-11 ms, so the time is outside the replay (next step: where the game's own frame goes) |

**Not a bug (checked):** the first run's shots showed the dialogue cut after two letters. The shot was taken 0.9 s
after an A press (the typewriter mid-line); native shoots 4 s after. Shots 3-4 s after the press (runs 3, 5, 6, 7)
show the whole line with the default by-use sync, with `NXGFX_WEB_USE_SYNC=0` and with `NXGFX_UPLOAD_SKIP=0` alike.

**Rebuilds:** the prebuild rewrites the library's manifest, so a web rebuild without `--shaders-from` lists none of
the imported keys; their `spv/` files stay and the web reads them by key (`vk::shader` fetches `shaders/spv/<key>.spv`),
so drawing does not depend on the manifest. Build for web always passes the flag.
