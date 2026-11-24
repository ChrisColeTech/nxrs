# Settings

The game's ini (`<game>.ini` beside the executable; `NXRS_INI` names another) holds these keys. A missing key takes its
default; values outside a key's range are brought into it when read. Turbo's Spanish key names are still read and written
back under these names. Environment switches for diagnostics and tests are listed in [env_switches.tsv](env_switches.tsv).

This page is generated from `src/config/src/options.rs` (`cargo test -p config settings_page_is_current` checks it).

## Display

| Key | Default | What it does |
|---|---|---|
| `docked` | `1` | 1 = TV mode: the game draws for a television (Switch: up to 1920x1080; Switch 2: up to 2560x1440 and more), 0 = handheld mode: the console's own screen (Switch 1280x720, Switch 2 1920x1080), softer on a monitor |
| `fullscreen` | `1` | 1 = start in fullscreen (F11 or Alt+Enter switches while playing; the choice is kept here) |
| `language` | `1` | Game language: 0 Japanese, 1 English (US), 2 French, 3 German, 4 Italian, 5 Spanish, 6 Chinese, 7 Korean, 8 Dutch, 9 Portuguese, 10 Russian, 11 Taiwanese, 12 English (UK), 13 French (Canada), 14 Spanish (Latin America) |
| `keyboard_text` | `Player` | The text the on-screen keyboard answers when the game asks for one (a name...) |
| `error_notice` | `1` | 1 = when the game shows an error (its error screen on the console) a message box shows it beside the game (the game goes on); 0 = logged only |
| `offline_pages_in_browser` | `1` | 1 = the game's own HTML pages (its manual, offline web pages) open in the PC's browser; 0 = not opened |
| `keyboard_box` | `1` | 1 = when the game asks for text a box opens over it to type in (keyboard_text already in it); 0 = keyboard_text is answered at once |

## Controls

| Key | Default | What it does |
|---|---|---|
| `mouse_right_stick` | `0` | Right stick: 0 = the I J K L keys, 1 = the mouse (F1 changes it) |
| `mouse_sensitivity` | `1.00` | Mouse speed, 0.10 to 5.00 (F1: 10% to 500%, on a curve: 10% = 5x, 100% about 14x, 500% = 1250x) |
| `mouse_invert` | `0` | Mouse direction: 0 = normal, 1 = left / right inverted, 2 = up / down inverted, 3 = both |
| `button_layout` | `switch` | Face buttons: switch = by position as on a Switch (right = A, bottom = B, top = X, left = Y; Circle = A, Cross = B on PlayStation), xbox = by the Xbox labels (bottom = A, right = B, left = X, top = Y; Cross = A, Square = X on PlayStation). Applies live |
| `button_map` | (empty) | Optional changes on top of button_layout, physical:guest pairs, e.g. south:a east:b. Physical: south east west north (or cross circle square triangle), l r zl zr lstick rstick plus minus up down left right. Guest: a b x y l r zl zr lstick rstick plus minus up down left right none |
| `controller_prompt` | `1` | 1 = when a game wants more players than are connected, it waits with a prompt until they join (A or Return: done, B or Esc: cancel); 0 = it gets the players there are at once |
| `pc_user` | `1` | The local profile (1..8) the game runs as: picked at launch and by any in-game user picker (a PC app has its player, not a console user list). Others exist when a game made them (their save folder markers) |
| `pc_user_name` | `Player` | The name the game shows for that profile (NXOS_NICK overrides) |
| `ask_user` | `0` | 1 = when the game asks who is playing and more than one local profile exists, a picker opens over it (Up / Down to choose, A / Return: select, B / Esc: cancel); 0 = the game gets pc_user without asking |
| `home_quit` | `1` | 1 = holding a controller's Home (logo) button 2 s asks whether to quit; 0 = off |
| `rumble` | `1` | 1 = the game's rumble reaches the controllers; 0 = off. Applies live |
| `rumble_gain` | `1.00` | Rumble strength, 0 to 3 (0 = silent, 1 = as the game sends it). Applies live |
| `rumble_min` | `0` | Rumble cutoff, 0 to 0.9: levels below it stay silent and the rest is stretched over the full range (for controllers with rotating motors that a game's quiet rumble keeps running: Mario Kart 8 Deluxe 0.45). Applies live |
| `pad_press_to_join` | `0` | 1 = a controller is no player until one of its buttons is pressed, then it is the lowest player free; 0 = each controller connected is a player at once (the one it had before, else player 1 first) |
| `pad_merge` | `0` | 1 = every controller is player 1 together; 0 = each controller is a player of its own |
| `keyboard_player` | `shared` | shared = the keyboard is player 1 together with the first controller; own = the keyboard alone is player 1 and the controllers are players 2 and on |
| `pause_on_focus_loss` | `0` | 0 = the game runs on when its window is behind others (a PC app); 1 = the game is told it lost the focus (many pause, as on a console) when the window is in the background or a prompt is open over it |
| `mute_on_focus_loss` | `0` | 1 = the game's sound is muted while its window is behind others or minimised (it comes back with the focus); 0 = it plays on (a PC app). Applies live |

## Graphics

| Key | Default | What it does |
|---|---|---|
| `renderer` | `vulkan` | Graphics back end: vulkan (NVN on Vulkan; the only renderer of this engine) |
| `resolution` | `1` | Internal 3D resolution: 1 = native, 2 = double, 0.75 / 0.5 = faster (applies live) |
| `dynamic_resolution` | `0` | 1 = lower the resolution by itself when the GPU cannot keep up (down to dynamic_resolution_min) |
| `dynamic_resolution_min` | `0.6` | The lowest internal resolution dynamic_resolution goes to |
| `game_dynamic_resolution` | `auto` | The game's own dynamic resolution and frame pacing, as on the console: auto = on (the game is told the GPU's real times), 1 = on, 0 = off (the game is told the GPU takes no time: always its largest size, for a fast graphics card) |
| `shadows` | `1` | 0 = no shadow maps (more FPS) |
| `image_noflag` | `1` | 1 = allow shader image writes to textures without the IMAGE flag (required by some games); 0 = diagnostic override |

## Performance

| Key | Default | What it does |
|---|---|---|
| `fps_target` | `60` | Experimental: 30 or 20 = draw only every 2nd / 3rd frame while the game keeps its speed (less GPU work; effects updated on skipped frames can flicker). 120 = frame generation x2 at 120 FPS (see lsfg). 60 = normal |
| `fps_limit` | `1` | 1 = 60 FPS, 30 = show at most 30 images a second while the game keeps its 60 Hz steps, 0 = unlimited. F4 switches between 60 and unlimited |
| `forced_vsync` | `0` | 1 = always wait for the display's refresh (can cost a lot of FPS on slow PCs) |
| `present_interval` | `0` | 0 = the game chooses how many vsyncs a frame lasts; 1 = force every vsync (a 30 FPS game drawn at 60: only for games that keep their speed, see the profile), 2 = force every second vsync |
| `null_trap` | `0` | 1 = a write through a NULL pointer stops the game with a precise log (diagnostics; 0 = normal) |
| `memory_gb` | `0` | Memory the game sees: 0 = automatic (as the console, more for games that need it), 4 = console, 6 or 8 = extended (as yuzu's extended memory layout) |
| `driver_exclusive` | `auto` | auto = on for Intel graphics (its driver crashes when compiling and drawing at the same time), 1 = always, 0 = never |
| `shader_thread` | `1` | 1 = build pipelines on threads of their own (no freezes), 0 = on the GPU thread (short freezes; try it if the game crashes inside the video driver) |
| `low_priority_shader_compiler` | `1` | 1 = build shaders and pipelines at low priority (less stutter on weak CPUs) |

## Frame generation

| Key | Default | What it does |
|---|---|---|
| `lsfg` | `0` | Frame generation: in-between frames shown while the game is below lsfg_target (1 = on, 0 = off) |
| `frame_gen_type` | `builtin` | builtin = the engine's own (motion estimated on the shown frames, needs nothing else); lsfg = LSFG with the Lossless.dll of YOUR Lossless Scaling (copied beside the game, set in lsfg_dll, or found in Steam); without a usable Lossless.dll the built-in one is used |
| `lsfg_target` | `60` | The FPS frame generation aims for |
| `lsfg_multiplier` | `2` | 2 = x2, 3 = x3, 4 = x4 |
| `lsfg_flow` | `0.5` | Flow scale: 1 = best quality, 0.5 = faster (for integrated GPUs) |
| `lsfg_resolution` | `720` | The height frame generation works at |
| `lsfg_dll` | (empty) | Path of Lossless.dll (empty = look for it) |
| `lossless_scaling` | `0` | 1 = also start the Lossless Scaling app with the game (not needed for lsfg) |
| `lossless_path` | (empty) | Path of LosslessScaling.exe (empty = look in Steam) |

## Audio

| Key | Default | What it does |
|---|---|---|
| `audio_slowdown` | `1` | 1 = slow the sound down smoothly when the PC cannot keep up, keeping every queued sample (F1) |

## Recording

| Key | Default | What it does |
|---|---|---|
| `smooth_recording` | `1` | 1 = F7 videos are always smooth: the game runs in slow motion while recording if the PC cannot keep up, and the video plays at normal speed with no stutter |
| `recording_fps` | `60` | The video's FPS (constant, 15 to 60) |

## Debug

| Key | Default | What it does |
|---|---|---|
| `log` | `1` | Log detail: 1 = normal, 2 = everything (slow) |
| `diag` | `0` | 1 = automatic diagnostic dumps (F5 always works) |
| `profile` | `0` | 1 = write profile.txt every 10 seconds (CPU profiler) |
| `metrics` | `0` | 1 = write metrics-<time>.csv (each second: CPU, waits by kind, dropped work) and frames-<time>.csv (each frame) beside the log |
