# Game state

The games' own state: saves, state a scripted run sets up, and game behaviour that looks like an engine bug but is
not. Saves are served by `src/os/src/services/fs.rs`; scripted input is `src/os/src/input/script.rs`.

## Saves

Saves live under `<data>\save` (or `NXOS_SAVE`), one folder per mount:

| Mount | Folder |
|---|---|
| account save of user n | `save\user<n>` |
| device save | `save\device` |
| cache storage n | `save\cache<n>` (+ `cache<n>.size`, "<data> <journal>") |
| temporary storage | `save\temp` |
| system data | `save\system\<id>` |

**Transactions.** A save mount works on `<save>.work`, a copy of the committed `<save>` made at the first mount.
`Commit` copies the work tree to `<save>.new`, moves the committed one to `<save>.old`, moves `.new` into its place and
deletes `.old`. A run killed at any point leaves the committed save whole; a `.new` left with no committed save is
taken as committed at the next mount. A Commit of a mount list spanning two saves is refused before either is
published. Temporary and cache storage are written directly.

**Cache storage sizes** come from the control data (`<data>\control.nacp`: CacheStorageDataAndJournalSizeMax at
0x3180, CacheStorageIndexMax at 0x3188), else 256 MB of each.

**Profiles.** The player is `pc_user` (1..8); other users exist when a game made them (`user<n>.registered`
markers). Switching `pc_user` switches the save folder.

**Turbo saves.** A Turbo build keeps the account save in `save\user` (older ones in `save\system`). The first time
user 1's save opens and none exists, the Turbo one is copied in and left where it was (`adopt_turbo_save`). Log:
`fs: <path> is a Turbo build's save: copied to <path> (the original is kept)`. The launcher also offers a copy between
Turbo and nxrs builds (PORT_PLAN M9).

## Scripted state

Inputs and other state for unattended runs (the harness is `scripts/play_scenario.py`):

| Switch | Effect |
|---|---|
| `NXRS_KEYS=<steps>` or `@<file>` | `seconds:key[+key][~hold][*count/interval],...`: A B X Y LS RS L R ZL ZR Plus Minus Left Up Right Down, LLeft ... RDown (a stick pushed), `L<x>/<y>` / `R<x>/<y>` (part way, -1..1); held 0.15 s by default |
| `NXRS_INPUT_RECORD=<file>` / `NXRS_INPUT_REPLAY=<file>` | record player 1's physical input as `<ms> <buttons> <lx> <ly> <rx> <ry>`; replay it from when the file appears (rewritten, it plays again) |
| control pipe `press <keys>[~hold]`, `P<n> <keys>` | presses for player 1 or n; `touch`, `drag`, `pinch`, `rotate` for the touch screen |
| `NXOS_FIXED_TIME=<POSIX seconds>` | pins the user clock, for comparable runs |
| `NXRS_MKW_SEED=<n>` with `NXRS_PATCH_MKW_SEED=1` | fixed random seeds in Mario Kart World |
| `keyboard_text`, `keyboard_box=0`, `NXOS_SWKBD=auto` | the software keyboard answered without a window |
| `NXOS_CTRL_PROMPT=0` / `controller_prompt=0` | the controller-support applet answers at once with the players there are |
| `NXRS_NO_ACTIVATE=1`, `NXRS_KEEP_FOCUS=1` | the window does not take focus; the game is always told it is in focus |
| `NXRS_ESC_QUIT=0`, `NXRS_CLOSE_QUIT=0` | Esc and a single close are logged and ignored |
| `NXOS_EXIT_AFTER=<s>`, `NXOS_EXIT_AFTER_SUBMIT=<s>`, `NXRS_SECONDS=<s>` | a timed end |

### Name entry in a scripted run

- **Symptom:** a scripted run stops at a name entry screen.
- **Cause:** the software keyboard's text box waits for typing.
- **Fix:** `keyboard_box=0` (or a headless run) answers `keyboard_text` at once.

## Game behaviour that is not an engine bug

### Jamboree's white layer

- **Symptom:** Super Mario Party Jamboree shows a white layer over the picture.
- **Cause:** the game draws an opaque white fade on purpose: the quad's texture is all 1.0 in guest memory and on the
  GPU, blend and colour mask are ordinary. Its fade state never advances: game state, not a renderer bug
  (INTERFACES "Jamboree's white layer: not a blend state").
- **Status:** open; what keeps the fade from advancing is not found yet.

### Donkey Kong Bananza's 20 FPS

- **Symptom:** Donkey Kong Bananza runs at about 20 FPS on a fast PC.
- **Cause:** the game picks present interval 3 itself whenever a frame passes about 33 ms; a lower replay CPU time does
  not change the rate until frames stay under that.
- **Fix:** none needed in the present path; reduce frame cost (INTERFACES, lane G4 notes).

### Story points that need a hand play

- **Symptom:** a long scripted run of Pokémon Legends: Arceus stops progressing at a story point.
- **Cause:** the story needs input a fixed script does not give; the run itself is healthy at 30.0 FPS (PORT_PLAN M6).
- **Fix:** none: the 5B bar is first controllable gameplay, not story progress.

### Two players asked for, one connected

- **Symptom:** a game waits at a "connect controllers" screen.
- **Cause:** it asked the controller-support applet for more players than are connected; with `controller_prompt=1`
  the engine waits for them to join, as a console would.
- **Fix:** press A / Return to continue with the players there are, connect another pad, or set
  `controller_prompt=0`.

### Buttons by position

- **Symptom:** A and B seem swapped on an Xbox or PlayStation pad.
- **Cause:** `button_layout=switch` maps by position (right = A), as a Switch does; Nintendo pads always use their
  printed labels.
- **Fix:** `button_layout=xbox` maps by label; `button_map` changes single buttons.

nxnative's per-game entries (Balatro's Lua errors, Diablo II's offline login window, Rhythm Heaven's calibration)
are about games not in this engine's verified library; their causes are game policy and would apply here unchanged.
