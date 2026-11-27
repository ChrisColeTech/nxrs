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

## 4. Results

(filled in as the wave lands)
