# WEB-W9 - Saves, loading, settings in the game page

**Status: built (2026-11-27), verified on Beach Bounce in Chrome; results in section 8.** Phase W9 of
`docs/35-WEB-TARGET-PLAN.md`. Branch `web-w9`, base `cee67c0`, gameplay-harness (W8, W11) merged in. Owner, at the
build: the three questions of section 7 take the recommendations; the Saves page carries no developer detail (no
acknowledgement times, no storage wording; one plain line only when a save could not be stored); Restart is one
Group "Game" with a single "Restart game…" button, its explanation in the info mark.

Owner decisions this design keeps (2026-11-27): settings and save management live **in the game page**, as an in-game
menu over the game in Chrome, stored in the browser. Not a separate web launcher page, not the desktop launcher.
Scope: saves and loading; settings for graphics, audio and the controller layout.

Mockup: `build/WEB/W9/mockup.html` (`?view=loading|saves|delete|graphics|audio|controller`) over a real Chrome frame
of Anime Girls (WEB-W4d `run6-fix-on/t110.png`, cropped to the canvas: `bg.jpg`; W8's `final/t120.png` went with the
W8 worktree). Captures: `build/WEB/W9/{loading,saves,delete,graphics,audio,controller}.png`. The numbers in the
mockup are examples.

## 1. What exists today

### 1.1 The page (`src/web`)

| File | What it does for W9 |
|---|---|
| `index.html`, `boot.js` | The page is the canvas plus **one notice line** (`#notice`): "Loading" until the first picture (`nxrs-gpu` `first`), then "Click or press a key for sound" until a gesture, or a failure in red that stays. The engine log goes to the console and `window.NXRS_LOG`. Progress exists only as log lines: `[web] data at N s: fetched X MiB of Y MiB` every 30 s and the nnMain line. `?env=NAME=value,...` sets engine variables; `?fresh=1`, `?bootset=0` |
| `saves.js` | `NxrsSaves.restore` before `main`: every `nxrs-saves/<title>/<name>.bin` in OPFS (a `.new` only when no `.bin`: an interrupted commit) is unpacked (`NXSV1`, `webdata::pack_save`) into `/data/save/<name>`. A save that cannot be read **stops the start** (never empty saves over stored ones). Restore only: no list, export, import or delete |
| `io-worker.js` | `saveCommit`: op 2 writes `<name>.new`, flushes, moves it over `<name>.bin`, then marks the slot done (the acknowledgement). Serialised by `saveChain`. `stats` answers bytes fetched (boot set / on demand / cache hits) |
| `input.js` | Keyboard (by `KeyboardEvent.code` → Windows VK) and up to 4 gamepads (standard mapping, buttons 0-31, axes 0-3) into the engine's input block. F5, F11, F12 and Ctrl+Shift pass to the browser. Escape is mapped to VK 0x1B but no guest button uses it |
| `gpu.js` | Canvas letterboxed at 16:9, backing store at device pixels (W8), GPU worker owns it; `nxrs-gpu` events `first` / `fatal` |
| `audio-worklet.js` | The ring read into a 48 kHz `AudioContext` started at the first gesture. No gain stage |

### 1.2 The engine side

- `src/os/src/web/saves.rs` `persist`: called from `fs::commit` (`services/fs.rs`) on the web; packs the committed
  folder, asks the I/O worker to store it, waits up to 10 s; no acknowledgement is a red line and the commit fails.
  Save folders under `/data/save` are `user<N>` (account saves, `mount_save`), `temp`, `cache<N>`; only committed ones
  (the account saves) reach the browser.
- `src/os/src/input/web.rs`: keys through a **fixed copy** of the native front's table (`front/src/keys.rs`); gamepad 0
  **by position only** (`pad_buttons`, a fixed table) with a **fixed 10% dead zone** (`pad_sticks`). It does **not**
  go through `input/layout.rs`, so `button_layout` and `button_map` have no effect on the web today.
- **No settings file on the web.** `nxconfig::open` is called only by the native front (`front/src/lib.rs`, the
  `<name>.ini` beside the exe). `nxweb_main` (`os/src/boot.rs`) never opens one, so every key is its default; only
  `NXOS_*` / `NXGFX_*` environment overrides reach a web run, through `?env=`.
- `oe::set_focus` (window focus as the game sees it, `mute_on_focus_loss`, `pause_on_focus_loss`) is called by the
  native front and the watch control only; the web calls `input::set_focus` alone, so those two keys are inert there.

### 1.3 How native nxrs exposes the same settings

- **The game's INI** (`config/src/options.rs`, the table `docs/settings.md` is generated from) is the engine's one
  settings store. The keys W9 needs, all existing:

  | Key | Values | Read by | Live natively |
  |---|---|---|---|
  | `docked` | 1 TV, 0 handheld | `os/options.rs` listener → `oe::set_opmode` (operation-mode notification) | yes |
  | `resolution` | 0.25-4 (1 native) | `graphics/vk/scale.rs` `wanted()` | yes |
  | `dynamic_resolution`, `dynamic_resolution_min` | 0/1, 0.25-1 | `scale.rs` | yes |
  | `shadows` | 0/1 | `vk/exec.rs` `shadows_on()` (re-read on `generation`) | yes |
  | `upscale` | `standard` or sharp (not in the table; `vk/upscale.rs` `sharp()`) | present | at present setup |
  | `fps_limit` | 1 = 60, 30, 0 unlimited, 60-240 | `vk/present.rs` `pace_period` (all targets) | yes (F4) |
  | `forced_vsync` | 0/1 | `present.rs`: FIFO vs MAILBOX present mode | swap chain rebuild |
  | `mute_on_focus_loss` | 0/1 | `oe::set_focus` → `media::sink::set_muted` | yes |
  | `audio_slowdown` | 0/1 | the sink | yes (F1 page) |
  | `button_layout` | `switch` / `xbox` | `input/mod.rs` `layout_update` (on `generation`) | yes |
  | `button_map` | `physical:guest` pairs, e.g. `south:a east:b` | same | yes |

- **The launcher** writes a subset into each built game's INI: Properties > Runtime "Screen mode" (`docked`) and
  "Controller layout" (`button_layout`, Use default = Settings > Controller), Settings' play options
  (`pause_on_focus_loss`, `mute_on_focus_loss`, `docked`, `ask_user`), and "Settings file" opens the INI for the rest
  (`flows/properties_apply.rs`, `services/ini_file.rs`).
- **The in-game F1 page** (`front/src/help.rs`): four options (right stick from mouse, mouse direction, mouse
  sensitivity, smooth audio slowdown) changed with arrows / Enter / click, saved with `nxconfig::set`; Esc, F1 or
  Backspace leaves; while open `input::set_suspended(true)` (the game reads no pads) **but the game keeps running**.
- **What native has no option for:** a master **volume** (only `NXOS_AUDIO_MUTE` for a whole run), **keyboard
  remapping** (the key table is fixed), a stick **dead zone** (fixed: XInput ±7000, web 10%), and a **pause** that
  stops the game (the guest clock's rate has a floor of 0.05, `os/clock.rs`). Section 6 asks about the first three;
  the pause is designed in 2.2.

## 2. The menu

### 2.1 Opening and closing

| Input | Opens | In the menu | Closes |
|---|---|---|---|
| Keyboard | **Esc** | Tab / Shift+Tab and arrows move; Enter or Space acts; Left / Right change a choice or slider; Ctrl+Tab next section | **Esc** |
| Gamepad | **Home** (standard button 16: PS, Xbox guide, Switch home), or **Minus + Plus held 1 s** (for pads whose Home the OS takes, e.g. the Xbox guide opens Game Bar) | D-pad / left stick move; **A** acts (A by the chosen face-button layout); Left / Right change; **L / R** previous / next section | **B**, or Home again |
| Mouse | not needed while playing | click; the pointer shows while the menu is open | the header **X** ("Resume game (Esc)") |

- Esc is free: no guest button is mapped to it (`web.rs` `keymap`), and the native front's Esc (quit) has no web
  meaning (closing the tab quits). In page fullscreen Chrome takes Esc to leave fullscreen; the page calls
  `navigator.keyboard.lock(['Escape'])` when it enters fullscreen so Esc reaches the menu (holding Esc still leaves
  fullscreen, Chrome's rule). F11 fullscreen and the installed-app window deliver Esc normally.
- The menu takes all input while open: the page stops writing keys and pads into the engine's block and releases the
  held ones (as on blur), so nothing pressed in the menu reaches the game, and a menu button held while it closes
  does not either (released first, then input resumes on the next change).
- Dismissal follows the style guide §5: **the header X and Escape (B) only**; no Resume, Close, Cancel or Done
  button. The header says **Paused** beside the game's name.

### 2.2 It pauses the game

Opening the menu:

1. **Input suspended** (above).
2. **Guest clock held.** New: `os/clock.rs` gets a hold (time stops; today's `set_rate` floor is 0.05). The system
   counter, the steady clock and vi's vsync events follow the guest clock, so the game's frame loop blocks in its vsync
   wait and its timers stop; a game that measures time sees none pass. Released on close, with the held time removed
   (the game sees no jump).
3. **Sound suspended**: `AudioContext.suspend()`; the engine's ring fills and the producer waits, the same as a full
   ring today. Resumed on close (`resume()`), no burst: the ring holds at most 3072 frames (64 ms).
4. **Picture kept**: no present happens, so the GPU worker keeps the last image; the menu draws over it in the page
   (HTML over the canvas, nothing in WebGPU).

The page tells the engine through one new word in the input block (`[11]` menu open, bumped with the sequence); the
input thread's tick (4 ms) applies 1-3 on the engine side. If a title still advances while held (a busy loop that
never waits on vsync or a timer), that is a title bug to name, not hidden.

### 2.3 Layout

A rail work window (style guide §5) centred over the dimmed, blurred game: header (title, Paused chip, bordered X),
a rail of four sections with the selection tint, 2 px accent bar and accent ink, and the section's form in the page
grammar: Groups (32 px accent tile, bold heading, info mark), Rows (label column, control after it), explanations in
info marks only, an accent-cored hairline between Groups, ghost buttons, one accent (lime, the launcher's default).
The menu is **Live** (every change applies and is stored at once), so it has no footer and no filled button; only a
question box has one filled verb, last.

### 2.4 Sections and controls

Every engine key below already exists (1.3) unless marked **new**. "Live" means it applies while the menu is open and
the change is visible on resume; "next start" means it is stored now and used when the page loads again.

**Saves** (section 3 has the mechanism)

| Control | Wording | Does | When |
|---|---|---|---|
| List | columns **Save**, **Last written**, **Size**; a row per stored save: "Player 1" (`user1`; the name is `pc_user_name` for user 1, "Player N" otherwise), under it "user1 · written by the game" or "· imported <date>" | lists OPFS `nxrs-saves/<title>/*.bin` | refreshed at every acknowledged commit |
| Row button | **Export…** | saves the file `<title> - Player 1 - <yyyy-mm-dd>.nxsave` | at once |
| Row button | **Delete…** | question (2.5) | restarts |
| Status line | "2 saves, 244 KB · last stored today 08:12 (acknowledged in 13 ms) · storage is persistent: the browser will not clear it on its own" (or "· storage is not persistent: Chrome may clear it when the disk is low"; or "No save yet: the game has not written one") | | |
| Button | **Import save…** | file picker (`.nxsave`), then a question (2.5) | restarts |
| Button | **Export all…** | one `.nxsave` per save, in a folder the player picks (`showDirectoryPicker`) | at once |
| Group **Game** | Row "Restart", button **Restart game…** | question "Restart the game?" / **Restart**: reloads the page (saves stay) | |

**Graphics**, Group **Picture**

| Row | Control and wording | Key | When |
|---|---|---|---|
| Screen mode | select: **TV (1920×1080)**, **Handheld (1280×720)** | `docked` 1 / 0 | live (the game gets the operation-mode notification, as natively) |
| Internal resolution | select: **Half (0.5×)**, **Three quarters (0.75×)**, **Native (1×)**, **One and a half (1.5×)**, **Double (2×)** | `resolution` | live |
| Dynamic resolution | toggle | `dynamic_resolution` (min stays 0.6) | live |
| Shadows | toggle | `shadows` | live |
| Upscaling | segmented: **Sharp**, **Smooth** | `upscale` (sharp / `standard`) | live: the change marks the presenter stale so it is rebuilt at the next present |

Group **Frame rate**

| Row | Control | Key | When |
|---|---|---|---|
| FPS limit | segmented: **30**, **60**, **Unlimited** | `fps_limit` 30 / 1 / 0 | live |

Not shown: **vsync** (`forced_vsync`). Chrome always shows a canvas in step with the display; there is no present
mode to choose on WebGPU, so a vsync switch would change nothing (no fake features). The Frame rate group's info
mark says so. W8 already ties the output resolution to the window, so there is no separate output-resolution row.

**Audio**, Group **Sound**

| Row | Control | Does | When |
|---|---|---|---|
| Volume | slider 0-100 %, step 5 | a `GainNode` between the worklet and the speakers (**page-side**, no engine key: Q1) | live |
| Mute | toggle | gain 0 (page-side) | live |
| Mute in the background | toggle | `mute_on_focus_loss`; the web input tick starts calling `oe::set_focus` (1 visible and focused, 2 behind, 3 hidden) | live |
| Smooth slowdown | toggle | `audio_slowdown` (the native F1 page's option) | live |
| Status line | "Sound running · 48 kHz · 0 underruns", or "Sound starts at the first click or key" | the worklet's counters | |

**Controller**, Group **Controller**

| Row | Control | Key | When |
|---|---|---|---|
| Gamepad | readout: the pad's `Gamepad.id`, or "No gamepad: press a button on one to connect it" | | |
| Face buttons | segmented: **By position (Switch)**, **By label (Xbox)** | `button_layout`; the web pad goes through `input/layout.rs` like every native pad (today it bypasses it) | live |
| Stick dead zone | slider 0-30 %, default 10 % | **new** key (Q1) | live |

Group **Buttons**: a table per Switch button (A, B, X, Y, L, R, ZL, ZR, Plus, Minus, D-pad, left stick, right stick,
stick presses) with a **Keyboard** column and a **Gamepad** column. Choosing a cell shows **Press a key…** (or
**Press a button…**); the next key or button is bound, Esc or B leaves it unchanged; a key already bound elsewhere
moves (the other cell shows it removed). Gamepad cells write `button_map` (`physical:guest`, live). Keyboard cells write
a **new** `key_map` key in the same grammar (`space:a z:b ...`; Q1), read by `input/web.rs` instead of its fixed table
(live). Directions (D-pad, sticks) bind as four keys each. Button below: **Restore default buttons** (clears both maps).
Gamepad names follow the connected pad (Cross / Circle on a DualSense, A / B on an Xbox pad, by `Gamepad.id`).

### 2.5 Questions over the menu

The style guide's question box: title names the action, heading is the question, text, footer the dialog's own ghost
verb at the leading side and one filled verb, last; dismissal by X / Esc / B.

| Title | Heading | Text | Footer |
|---|---|---|---|
| Delete save | Delete Player 1's save? | It is removed from this browser and the game restarts without it, so it cannot write it back. Export it first to keep a copy. | ghost **Export…**, filled **Delete and restart** |
| Import save | Replace Player 1's save? (or Add Player 2's save?) | The game restarts to load it. The save there now is replaced; export it first to keep a copy. | ghost **Export…**, filled **Import and restart** |
| Restart game | Restart the game? | It starts again from the title screen; saves stay as stored. | filled **Restart** |

Import and delete restart because the running game holds its save in memory and would write the old one back at
its next commit; restarting is the only honest way to make the change stick.

### 2.6 How settings are stored and applied

- **Format:** the engine's own INI text (`nxconfig::render` / `parse`): the same keys, sections, normalisation and
  `settings_version` migration as a native game's `<name>.ini`. Page-only values (volume, mute) go in a `[Web]`
  section, which the engine keeps under `[Other]` and ignores.
- **Where:** `localStorage["nxrs-settings/<titleId>"]`, per game (the origin is the launcher's fixed
  `127.0.0.1:8780`, shared by every web game, so the key carries the title id from `build.json`). localStorage is
  synchronous, so the text is ready before `game.js` loads. If storage is blocked (a private window) the menu works
  for the session and its status says "Settings are not kept in this browser".
- **At start:** the page writes the text to `/data/settings.ini` in the in-memory file system (beside the host files);
  `nxweb_main` calls `nxconfig::open` on it (**new**, one call), so the web run reads exactly what native reads.
- **Live:** a **new** settings mailbox block, published like the input block: a sequence word and up to 4 KiB of
  `key=value` lines. The page writes the changed pair and bumps the sequence; the input thread's tick applies each
  pair with `nxconfig::set`, which fires the same listeners as F4 or the F1 page natively. The page then saves the
  whole text to localStorage. Nothing is written back by the engine (the web has no hotkeys that change settings).
- **Defaults:** a key not in the text is the table's default (`nxconfig::string`), exactly as a native INI without
  that key; the menu shows that value.

## 3. Saves

- **Detection of a game-written save:** the existing acknowledged commit. After the I/O worker moves `.new` over
  `.bin` it posts `{saved: name, bytes, ms}` to the page (**new** message); the page refreshes the list and the
  status line ("last stored today 08:12, acknowledged in 13 ms"). A failed commit is already a red line and the game's
  commit fails; the menu's status line then shows "The last save was not stored: <reason>" in the error red.
- **Kept:** on the first acknowledged commit the page calls `navigator.storage.persist()` so Chrome does not evict
  saves under storage pressure; the status line shows the answer. Restore at start stays as today (a damaged save
  stops the start with its reason).
- **List data:** name from the file (`user1.bin`), size from `File.size`, date from `File.lastModified`; "imported" is
  a small `nxrs-saves/<title>/<name>.json` beside an imported blob.
- **Export file `.nxsave`:** a 16-byte header `NXSE1` + version, then a JSON line `{titleId, title, save, written,
  engine}`, then the stored `NXSV1` pack unchanged. **Import** checks the magic, unpacks the pack fully (the same
  `unpack` as restore) before writing anything, and refuses another title's save by name ("This save is for Beach
  Bounce Remastered (0100…)"). It is written with the same `.new` → `.bin` move the worker uses.
- **Bug to fix on the way:** `saveCommit` flattens a nested name (`a/b` → `a_b`) while `restore` writes it to
  `/data/save/a_b`; harmless for `user<N>` today, fixed by storing the name escaped and restoring it back.
- **Exit criterion (from docs/35):** a game-written save shown surviving a reload: Beach Bounce's own Save menu
  (drivable since W7's frames), the menu lists it, the page reloads, the game's Load menu shows it. Then export,
  delete (the game shows no save), import (it is back).

## 4. Loading screen

Replaces the "Loading" notice line (the sound hint and the red failure stay as they are). Mockup `loading.png`.

- The game's **name** (`build.json`) and **icon** (the control data's icon, already among the host files), "first
  start on this browser" or "from this browser's cache".
- **One bar over bytes**, the real ones: game code bytes + boot-set bytes still to fetch. The caption names the step
  ("Downloading game data", "212.4 of 347.0 MB"). From cache the bar is skipped (nothing to fetch).
- **Steps** (each done, running or waiting; the right column shows its real measure):

  | Step | Measure | Source |
  |---|---|---|
  | Browser checked | the three checks | `checks()` |
  | Game code | bytes received of the module's size, or "kept copy (checked with the server)" | the `game.wasm` response body counted through a stream; total from `build.json`'s file sizes (the uncompressed size, which is what the stream yields under gzip) |
  | Game code compiled | seconds | `instantiateStreaming` |
  | Game data | boot-set bytes fetched of its total | the I/O worker's `stats`, sent every 250 ms while loading (today 30 s); total = the boot set's ranges |
  | Saves | "2 restored" / "none yet" | `NxrsSaves.restore` |
  | Starting the game | elapsed seconds until nnMain | the `>>> nnMain` log line (already watched) |
  | Shaders | packs fetched of packs needed so far | the shader pack fetches (`packs.txt` gives the count) |

- No invented percentage for compiling or starting: those steps show time, not a fill. The screen leaves at the first
  picture (`nxrs-gpu` `first`); a failure replaces the steps with the red reason (selectable), as today's notice does.

## 5. Out of scope

Rumble and HD rumble on the web; motion and touch; a second player's mapping (player 1 only, as today); game language;
the F1 mouse options (no pointer lock on the web yet); frame generation, recording, screenshots, cheats; cloud or
cross-device sync; the game's own save slots (the menu manages whole save folders, as the console does); any page in
the desktop launcher or a separate web launcher page (owner decision); Android touch menus.

## 6. Work and files when built

| Part | Files |
|---|---|
| Menu, loading screen, settings store, save list/export/import/delete | `src/web/menu.js` (new), `menu.css` (new, in `index.html`), `boot.js`, `saves.js`, `input.js`, `io-worker.js` (`saved` message, faster stats), `audio-worklet.js` host (`GainNode` in `boot.js`) |
| Settings at start and live | `os/src/boot.rs` (`nxconfig::open` in the web path), `os/src/web/mod.rs` (mailbox block), `input/web.rs` (tick applies it) |
| Pause | `os/src/clock.rs` (hold), `input/web.rs` (menu word → hold, `set_suspended`) |
| Controls through the engine's layout | `input/web.rs` (pad through `layout.rs`, `key_map`, dead zone key), `config/src/options.rs` (new keys, `docs/settings.md` regenerated) |
| Focus | `input/web.rs` calls `oe::set_focus` |
| Upscale live | `graphics/src/vk/upscale.rs` / `present.rs` (stale on change) |

Tests: `config` (new keys normalised), `os` (`key_map` and the pad through `layout.rs`, dead zone, clock hold keeps
time), page halves checked in Chrome as W3's were. Verification: Beach Bounce (save round trip, each setting seen
changing the game, pause holds the story text), Anime Girls (pause holds the beach; resolution, screen mode, FPS
limit), both on GPU 1.

## 7. Questions for the owner

**Q1. Volume, keyboard keys and dead zone have no engine option today.** Sources: `config/src/options.rs` (no such
keys), `front/src/keys.rs` and `input/web.rs` (fixed key tables), `input/xinput.rs` / `web.rs` (fixed dead zones).
Choices: (a) new INI keys (`key_map`, `stick_deadzone`; volume stays page-side because the web's sound output is the
page's), read by the web now and available to native later; (b) page-only for all three (input.js rewrites keys
before the block; no engine change); (c) new keys read by native and web in W9. **Recommendation: (a)**: the same
INI carries them, native can adopt them without a format change, and W9 does not touch the native front.

**Q2. Per game or shared?** Native has launcher-wide defaults with per-game overrides (`button_layout`). The web page
has no launcher. Choices: (a) everything per game; (b) Controller and Volume shared by every web game on this
browser, Graphics per game. **Recommendation: (b)**: a remap or a volume set once should not be redone for every
game; graphics depend on the game.

**Q3. Gamepad shortcut for the menu.** Home works on DualSense and Switch pads in Chrome; Windows takes the Xbox
guide button for Game Bar. Choices: Home plus Minus + Plus held 1 s (as designed), or Home only. **Recommendation:
both**, so every pad can open it.

## 8. Built and verified (2026-11-27)

**Built** (commits on `web-w9`):

| Part | Where |
|---|---|
| Menu (Saves, Graphics, Audio, Controller), questions, gamepad and keyboard navigation | `src/web/menu.js`, `menu.css` (new; host files in `cli/src/webhost.rs`, `dev_server.py`) |
| Opening: Esc, gamepad Home (std 16), Minus + Plus held 1 s; input released while open, held buttons stay released after | `src/web/input.js` |
| Pause: menu word `[11]` -> `clock::set_hold`, the next present waits (`graphics nvn/window.rs set_held`), vsync thread waits, `input::set_suspended`; AudioContext suspended and the ring left untouched (`media/sink.rs`); no stall warning while held (`watch.rs`) | `os/src/input/web.rs`, `os/src/clock.rs`, `services/misc.rs` |
| Settings: engine ini text in localStorage (`nxrs-settings/<titleId>` graphics, `nxrs-settings/shared` audio + controller + `[Web]` volume/mute), written to `/data/settings.ini`, opened by `nxweb_main` (`nxconfig::open`); live through the settings mailbox at input block `[64..]` (sequence lock, `nxconfig::set`) | `menu.js` `NxrsSettings`, `boot.js`, `os/src/boot.rs`, `os/src/input/web.rs` |
| New keys `key_map`, `stick_deadzone` (`docs/settings.md` regenerated); web pad through `layout.rs` (`button_layout`, `button_map`) | `config/src/options.rs`, `os/src/input/web.rs` |
| Focus 1 / 2 / 3 to `oe::set_focus` (mute and pause on focus loss) | `input.js`, `input/web.rs` |
| Saves: list, export (`.nxsave`: `NXSE1`, version, JSON, pack), import (checked whole, title id checked), delete; import / delete restart; I/O worker posts `saved` / `saveFailed`; `navigator.storage.persist()` at the first save; save names escaped (`encodeURIComponent`) and restored at the same path (the `a/b` -> `a_b` bug) | `saves.js`, `io-worker.js`, `boot.js` |
| Loading screen: name and icon from the control data, one bar over game-code + boot-set bytes, steps with real measures | `index.html`, `boot.js`, `io-worker.js` (`bootDoneBytes`) |
| Latched key presses moved from `[40..48)` to `[48..56)` (they overlapped gamepad 3's words) | `input.js`, `input/web.rs` |

Not built: the design's "Shaders" loading step (no page-side measure of shader packs exists; Beach Bounce has no
packs) and the Audio status line's underrun count (a plain "Sound is on" instead).

**Verified** (`build/WEB/W9/run1`, driver `tools/drive.mjs`, dev_server `--live` on 8779, own Chrome profile;
chrome://gpu: `LUID={0,113326} *ACTIVE*`): Beach Bounce's own Save menu wrote slot 12 (`io: save user1 written`,
`fs: commit save`); the menu listed "Player 1 · user1 · written by the game" (`09-menu-saves.png`); a reload restored it
(`saves: user1 restored`); the save exported to `run1/Beach Bounce Remastered - Player 1.nxsave`, deleted through the
menu's question (`14-delete-question.png`; restart: `none stored`), imported (another title's id refused with its
reason), restart: restored, and the game's Load screen shows slot 12 "Nov 27, 09:08" (`18-game-load-p2.png`). Pause:
presents stop at the menu (frames 14343 held), resume on close. Graphics: Handheld + Half applied live
(`oe: operation mode handheld`, `[vk] resolution scale x1.00 -> x0.50`), picture visibly softer
(`11-handheld-half.png`), and read again at start after a reload. Controller: A rebound to F through the table's
capture: X then gave no button, F gave A (0x0001); an injected standard pad (CDP `navigator.getGamepads` override):
bottom = B under switch, Home opened the menu, the pad's D-pad switched Face buttons to Xbox, its B closed the menu,
bottom = A (`button layout xbox`).

A check on the relinked build (`run2`) showed one `[RED] audio: the page's AudioWorklet stopped (context state 2)`
at resume: the hold is released a moment before the page resumes its AudioContext. The web output now keeps running
for 1 s after a hold (`media/sink.rs`); that build is the library's, but it was not run again in Chrome (W8b's Chrome
was running).

**Unproven:** the resume fix above in Chrome; the Export button's download and the Import file picker themselves (the same functions were driven
directly); a real gamepad (injected only); Minus + Plus hold; volume heard; Anime Girls (W8b's title).
