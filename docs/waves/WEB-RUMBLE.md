# WEB-RUMBLE - the game's vibration reaches the gamepad in Chrome

**Status: built and verified with an injected gamepad (2026-11-27); results in section 4. The owner's DualSense gets
no rumble from Chrome's Gamepad API on Windows (section 4.3).** Branch `web-rumble`, base `01ddbfe` (gameplay-harness after WEB-W9). Title: Beach
Bounce Remastered (library web build `nxrs-web\Beach Bounce Remastered nsw2u`). GPU 1 (`--use-adapter-luid=0,113326`).
Evidence `build/WEB/RUMBLE`.

## 1. Goal and cause

Owner: controller rumble does not work in the web games. The game's vibration reaches `input::vibrate_with`
(`src/os/src/input/mod.rs`), which keeps the standard path's value in `RUMBLE` (per player and side, after
`RumbleSettings`: `rumble`, `rumble_gain`, `rumble_min`). On native the pad backends read it (`Inputs::publish`).
On the web there are no pad backends: `input/web.rs` and `src/web/input.js` only carry input into the engine, and
nothing carries motor levels out to the page. HD rumble (`rumble::push`) claims only native Nintendo HID pads, so on
the web every vibration already takes the standard path: two levels per side, reduced to one low and one high motor
as native does for a rotating-motor pad (`Dev::rumble`: the larger of the two sides).

## 2. Plan

| Part | Change | Files |
|---|---|---|
| Block | Words `[56..64)` (free: latches are `[48..56)`, the mailbox starts at `[64]`): player p (0-3) low motor at `[56 + 2p]`, high at `[57 + 2p]`, 0..65535 | `input/web.rs`, `input.js` (header comments) |
| Engine | each input tick (web only) writes every player's two levels: the larger side of `RUMBLE`'s `amp_low` / `amp_high`, 0 while input is not active (menu open = suspended, page not focused, a host prompt) | `input/mod.rs` (`motor_levels`, emscripten/test only), `input/web.rs` |
| Self-test | `NXRS_CONTROLLER_SELFTEST=1` on the web: once gamepad 0 is connected, player 1's low motor 0.5 s, then high 0.5 s, then stop (as native per rumble pad) | `input/web.rs` |
| Page | each poll: gamepad i takes player i's levels through `vibrationActuator.playEffect('dual-rumble', {strongMagnitude: low, weakMagnitude: high, duration, startDelay: 0})`, re-issued before it ends while the level holds, `reset()` at zero; none while the menu is open, on blur / hidden | `input.js` |
| Menu | Controller section: "Rumble" on/off (`rumble`) and "Rumble strength" (`rumble_gain`, 0-300 %), stored with the shared controller settings and sent live through the mailbox | `menu.js` |

Native rumble is unchanged: the new engine code is `cfg(target_os = "emscripten")` (and `test` for the pure mapping).

## 3. Verification

- Unit tests: the block encoding (levels to words), the level mapping (larger side, inactive = 0, clamping), the
  self-test schedule.
- Chrome (own profile, GPU 1): the DualSense's `vibrationActuator` and its `effects` read on the real pad; Beach
  Bounce with a gamepad (the real one, else injected through a page shim), `playEffect` calls logged with their
  magnitudes when the engine vibrates (the game if it does, else the self-test).
- Relink Beach Bounce's library web build at the end.

## 4. Results

### 4.1 Built

As planned in section 2 (`d3b4c03`). The engine writes the levels every 4 ms tick; the page plays 200 ms effects,
re-issued 80 ms before they end while the level holds (about every 130 ms at 60 Hz polling), `reset()` at zero, on
menu open, on blur and when hidden. The menu's Rumble switch also silences a level the game is holding (the engine
reads `rumble` each settings generation); strength (`rumble_gain`, 0-300 %, stored as `1.00` = 100 %) applies to the
game's next vibration, as on native. Tests: `input::web` `two_sides_make_two_motors`,
`motor_levels_encode_into_the_block`, `selftest_runs_low_then_high` (8/8 of the module pass).

### 4.2 Chrome (Beach Bounce, library web build relinked from this branch, GPU 1)

`build/WEB/RUMBLE/run2` (driver `tools/drive.mjs --shim --focus --menu=70`, page
`?env=NXRS_CONTROLLER_SELFTEST=1,NXRS_CONTROLLER_TRACE=1`): a standard gamepad named as the owner's DualSense injected
before the page's scripts, its `vibrationActuator` logging every call. Beach Bounce (Ren'Py) sends no vibration of its
own (no `hid: vibration` line in its native log or in this run), so the engine's self-test drove it:

```
4.36 input: rumble self-test: player 1 low motor 0.5 s, high motor 0.5 s, stop
4.36 input: rumble to the page: player 1 low 32768 high 0 (of 65535)
4.37 [shim] playEffect dual-rumble strong 0.500 weak 0.000 duration 200 startDelay 0   (re-issued 4.51, 4.64, 4.77)
4.86 input: rumble to the page: player 1 low 0 high 32768 (of 65535)
4.87 [shim] playEffect dual-rumble strong 0.000 weak 0.500 duration 200 startDelay 0   (re-issued 5.00, 5.14, 5.28)
5.36 input: rumble self-test finished / player 1 low 0 high 0
5.37 [shim] reset
```

Menu: opened at 70 s (game paused, input suppressed), Controller section with the new Rumble and Rumble strength rows
(`run2/t080.png`); Rumble off from the menu reached the engine (`settings from the page: rumble=0`); closed at 100 s.

`run1-wrong-server`: not a valid run (see the report's slips): port 8791 was already served by W8b's dev server, so
that page was W8b's Anime Girls build with W8b's JS; nothing from it is used.

### 4.3 The DualSense in Chrome on Windows

Every 10 s the run logged the real `navigator.getGamepads()`: empty throughout (Chrome exposes a pad only after a
button press on it, which no one made). From Chromium's source (`device/gamepad`, main): on Windows a pad named
"STANDARD GAMEPAD Vendor: 054c Product: 0ce6" is a RawInput device (`raw_input_gamepad_device_win.cc`), which has
vibration only through `Dualshock4Controller` (`IsDualshock4`: Sony 05c4, 09cc, SCUF 7725) or `HidHapticGamepad`
(a table of a GameCube adapter and the Stadia controller). The DualSense (0ce6) is in neither, so `SupportsVibration()`
is false and its `vibrationActuator` is null: the page logs `vibrationActuator absent` and plays nothing. Xbox pads
(XInput) and the DualShock 4 do get `dual-rumble`. The DualSense's motors are reachable from a page only through
WebHID (the output report `sony.rs` already writes natively), which needs a one-time permission from the player.

### 4.4 Unproven

A motor felt (owner's manual test below); the menu-open / blur stop while a level is held (the self-test ends before
the menu can open; covered by `motor_words(.., false)` and the page's `stopAllRumble`); a title that vibrates by
itself on the web.

### 4.5 Manual test for the owner

Pad: an Xbox controller or a DualShock 4 (the DualSense will not rumble in Chrome, 4.3). Play Beach Bounce in the
browser from the launcher, then add `?env=NXRS_CONTROLLER_SELFTEST=1` to the page's address and reload; press any button
on the pad once the loading screen shows. Feel: the heavy (left) motor for half a second, then the light (right) one
for half a second, then nothing. The console shows `rumble gamepad 0: playEffect ...` lines; with the DualSense it
shows `vibrationActuator absent`. Esc > Controller shows Rumble and Rumble strength.
