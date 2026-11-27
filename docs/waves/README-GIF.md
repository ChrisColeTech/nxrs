# README-GIF: the README's hero GIF (Mario Kart 8 Deluxe, library to both title screens)

Branch `readme-gif` (from `gameplay-harness` 5735fc5). Title: Mario Kart 8 Deluxe (its native and web build
folders). Native game on GPU 1 (`NXGFX_GPU=1`), Chrome on GPU 1 only (`--use-adapter-luid=0,113326`), one game at
a time. Raw captures and the tools below: `build/README-GIF/` (not in git).

## Plan

`docs/images/readme/hero.gif`, 960x540, 12 fps, about 15-20 s, looping, under 8 MB, showing in order: the
launcher's library (NXRS centred in the title bar) -> MK8D selected -> Build -> Native (progress, sped up) ->
Build -> Web (sped up) -> Play -> Native (the game window to MK8D's title screen) -> Play -> Web (the browser
loading the web build to MK8D's title screen). README: the placeholder comment replaced by the centred GIF.

## Built

- `docs/images/readme/hero.gif`: 960x540, 12 fps, 234 frames = 19.5 s, loops forever, 7.72 MB. Segments (output
  seconds / real seconds): library 1.5 / 2.1; Game menu > Build > Native 2.3 / 3.5; the native build 2.2 / 97
  (~44x); Game menu > Build > Web (Native disabled: "up to date") 1.9 / 3.7; the web build 2.4 / 272 (~113x);
  Play > Native 1.2 / 2.4; the game window, logo to "Press L+R to start" 2.6 / 12; Play > Web 1.6 / 3.6; Chrome:
  the page's loading list, the logo, the title screen 3.8 / 29. Hard cuts, no caption.
- `README.md`: the `<!-- hero.gif ... -->` comment replaced by a centred `<img ... width="100%">`; nothing else.

## How it was made (to remake it)

All real: `target\release\nxrs.exe` (the lead's 14:00 build; its nxrs-build 14:55) with
`NXRS_DATA_DIR=build\README-GIF\data` (a copy of `%LOCALAPPDATA%\nxrs\library.json`), `NXGFX_GPU=1`,
`NXOS_SAVE=build\README-GIF\save` (an empty folder: the game's save never touched the build's data folder),
`NXRS_BROWSER=build\README-GIF\stub\Chrome`, started `--select-title "Mario Kart 8 Deluxe"` at below-normal
priority (its builds and the game inherit it).

1. Back up the native folder's files (exe, `_code.dll`, `.exe.lib`, `.ini`, build log, `threads.txt`) and the web
   folder's top-level files (`game.wasm`, `game.js`, `build.json`, host files, gz copies) to
   `build\README-GIF\backup\`. Delete the native exe, or Build > Native is disabled ("The native build is up to
   date"). Both builds succeeded, so nothing was restored; the folders now hold this engine's builds.
2. The main window sized to exactly 16:9 (3648x2052 physical, `inp.py size <pid> 3678 2067 33 24`: MoveWindow
   counts the invisible borders).
3. `tools\wincap.py --pid <pid> --out raw\launcher.mkv --ctl ctl.txt`: PrintWindow (`PW_RENDERFULLCONTENT`) of the
   process's visible windows, the main window first and its dialogs (separate OS windows: egui viewports) over it
   at their screen offsets, GDI-halftone scaled to 1920x1080, piped raw to ffmpeg (FFV1); one unix time per frame
   in `<out>.times`. The control file sets the rate: 12 fps for the interactions, 2 during the native build, 1
   during the web build. The same tool on the game's PID captured the fullscreen Vulkan game window (3840x2160;
   PrintWindow works for it here, ~7.5 fps reached at that size).
4. Driving: `tools\uia.ps1 -ProcId <pid> -Name <button>` invokes main-window buttons through egui's AccessKit tree
   (UI Automation; no mouse or keyboard): `Game`, then `Build`; `Play Mario Kart 8 Deluxe` (the card). The
   Native/Web choice dialogs have no AccessKit tree, so `tools\inp.py click <pid> x y` clicks them (coordinates in
   the 1920-wide capture): it raises that dialog topmost, refuses unless the window under the point is the
   launcher's, clicks, puts the cursor back and drops topmost. Build dialog: Native (1113,696), Web (1245,696);
   Play dialog: Native (1090,708), Web (1210,708). Event times went to `raw\events.txt`.
5. Native play: the game (child of the launcher, GPU 1) reached the title screen ~13 s after Play; it was closed
   with WM_CLOSE by its PID before the web segment.
6. Web play: the launcher's Play > Web served the build on its own server (`http://127.0.0.1:8780/
   mario-kart-8-deluxe-1g1u1d/`) and started `NXRS_BROWSER` with the address. That is `stub\Chrome.exe`
   (`stub\chrome_stub.rs`, beside an empty file `Chrome`, so the launcher's browser name is "Chrome" and Rust's
   Command appends `.exe`): it only writes `url.txt`. `tools\cdpcast.mjs` waits for that file, refuses if another
   game Chrome runs, starts Chrome with its own profile, `--use-adapter-luid=0,113326` and a CDP port, sets a
   1280x720 viewport at 1.5x and records `Page.startScreencast` JPEGs with their times for 95 s, then closes
   Chrome by PID. The default browser never ran the game, and GPU 0 never ran it.
7. `ffmpeg -i launcher.mkv -vf scale=960:540:flags=lanczos lf\%05d.png` (likewise the game), then
   `tools\compose.py compose\spec.json`: each segment maps a real time span onto output seconds, takes the newest
   frame at or before each output time, fits it to 960x540 RGB, one palette for the clip
   (`palettegen stats_mode=full`), `paletteuse dither=bayer:bayer_scale=4` (sierra2_4a gave 10.7 MB), `-loop 0`.
   `tools\sheet.py` makes contact sheets of any source at given times (how the cut points were chosen).

## Verified

Read: contact sheets of the launcher capture at every event, of the game capture (black, logo, title fade,
"Press L+R to start" from ~13 s), of the screencast (loading list 0-11 s, logo 12-16 s, title screen from ~24 s),
and nine frames of the final GIF (library; Build dialog; native build output; web build; Play dialog; game title;
launcher "Opened in Chrome"; web title screen). Builds: native `@result ok` (96 s), web `@result ok` (271 s).
`%LOCALAPPDATA%\nxrs\library.json` unchanged (14:56); no `_data\save` created; no capture, Chrome or game process
left running.

## Unproven

- The web segment ends on the title screen ("Press L+R to start"); W10c's title menu (Single Player ...) needs
  input past it, which the recording does not send. Later in the screencast (t70, t90) the page shows the black
  loading screen with the logo and a spinner (the game's own title-idle step, not investigated).
- The game capture runs at ~7.5 fps (4K PrintWindow); its 12 s are shown in 2.6 s, so it is not visible.

## Slips

- Two early test clicks and one Ctrl+B landed in the owner's Edge window (the launcher was behind it): `inp.py` now
  refuses a click unless the window under the point is the launcher's and keys unless the launcher has the
  keyboard. During the session the launcher's window was also minimized once and maximized once by something
  other than the tools (probably the owner); both were undone with `inp.py size`, and the frames in between are
  not in the GIF.

## Questions

None.
