# front (`nxfront`) and config (`nxconfig`)

The front end and the game's entry. A game links `nxfront.lib` (front + os + graphics + runtime) and its `main`
calls `nxfront_main(argc, argv, mods, count)`: the ini is opened, `--key=value` arguments apply for this run, the
window opens, the controllers, the control pipe, metrics and frame generation start, then the os layer boots the game.
The interfaces for other parts: `docs/INTERFACES.md`, "front". The settings live in `src/config` (`nxconfig`), a crate
with no dependencies every part can read.

## Layout

| File | Part |
|---|---|
| `lib.rs` | `nxfront_main`, `setup` / `start`, the 60 Hz front-end loop (input, ini reload once a second, metrics, the timed exit `seconds=` + `shot=`), `quit` (controllers quiet, ini and caches written, recording finished, TerminateProcess), `<name>.log` when the exe has no redirected output |
| `window.rs` | the one game window (Win32, own thread, per-monitor DPI): windowed / borderless fullscreen (`fullscreen`, F11, Alt+Enter, kept in the ini), title with state (FPS limit, RECORDING, LSFG), hotkeys, raw mouse, focus, device-change rescans |
| `keys.rs` | keyboard as player 1 (the default keys: arrows d-pad, WASD left stick, IJKL right stick, X/Space Z C V = A B X Y, Q E 1 3 = L R ZL ZR, Enter Backspace = + -), the mouse as the right stick (`mouse_right_stick`, `mouse_sensitivity` 0.1..5 on a speed curve, `mouse_invert`) |
| `help.rs` | F1: the controls page (right stick source, mouse direction, sensitivity, audio slowdown; the key tables), saved to the ini |
| `capture.rs` | F3 window screenshot (`captures\screenshot_<time>.png`; screen copy when in front, PrintWindow otherwise), F8 the game's picture at its own resolution through graphics (`captures\fullres_<time>.png`) |
| `rec.rs` | F7: H.264 MP4 through Media Foundation's sink writer at `recording_fps`, constant frame rate (`recordings\recording_<time>.mp4`) |
| `lsfg.rs` | LSFG 3.1 from the player's Lossless.dll (`lsfg_dll`, beside the game, Steam libraries): PE resources, SPIR-V, the pass set identified by pass order and bindings; `lossless_scaling=1` starts the Lossless Scaling app |
| `ctl.rs` | the control pipe `\\.\pipe\nxrs_ctl_<pid>`, commands from `nxconfig::ctl` |
| `metrics.rs` | `metrics=1`: `metrics-<time>.csv` each second (frames, FPS, CPU, private MB, every counter), `frames-<time>.csv` per frame rows graphics pushes |
| `../config/src/options.rs` | every ini option: default, comment, section, range; the Spanish aliases |
| `../config/src/lib.rs` | the ini: read, settings_version migrations (2: docked=1, 3: game_dynamic_resolution=auto), written back with comments, live reload, `--key=value`, the typed getters |

## Hotkeys

F1 controls page, F3 screenshot, F4 FPS limit 60 / unlimited (kept in the ini), F5 diagnostic dump (`captures\threads.txt`
and the gap list in the log), F7 record video, F8 full-resolution capture, F11 / Alt+Enter fullscreen, Esc quit.

## Build, test, run

    set CARGO_TARGET_DIR=target\I
    cargo test -p config -p front                         # well under a second
    cargo build -p front -p os                            # nxfront.lib and nxos-dev.exe
    target\I\debug\nxos-dev build --gen <gen> --obj <obj> --out <game>.exe --front
    py scripts\play.py smoke --exe <game>.exe             # boot, two screenshots, report.md + sheet-1.png

## Verified (MK8D, 2026-11-19, real Vulkan)

Window 2637x1483 on the 3x-DPI monitor, the ini written with every section and comment, `--fullscreen=0 --seconds=90`
for the run only; F4 wrote `fps_limit=0` and the title said "FPS limit: unlimited"; F3 wrote a 2637x1483 PNG of the
game's picture; F7 wrote a 3.5 s H.264 MP4; F11 twice went fullscreen and back and the ini kept `fullscreen=0`; the
control pipe answered `status`, `pads`, `press A~0.2`; the timed exit at 90 s (4,050 frames) wrote "Exit.". F7 with
`smooth_recording=1`: the clock slowed to 0.30, 118 of 118 frames distinct, H.264 1.97 s + AAC 2.07 s (ffprobe).
Keyboard X / W and NXRS_INPUT_REPLAY states were read by the game (hid trace); F1 opened the controls page; Esc quit.

## Not done

- Frame generation runs on graphics' present path: the frame-generation hook is requested from lane G
  (docs/INTERFACES.md); the set is identified and held (`lsfg::set()`). No Lossless.dll is on this machine.
- Window grabs for F7 take 55-100 ms while the window is not in front (PrintWindow); `smooth_recording` hides it by
  slowing the game, a graphics-side frame readback would make recordings real-time.
- The keyboard counts out of focus only with `NXRS_CONTROLLER_BACKGROUND=1` (test scripts).
