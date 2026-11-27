# WEB-RUMBLE - the game's vibration reaches the gamepad in Chrome

**Status: plan (2026-11-27).** Branch `web-rumble`, base `01ddbfe` (gameplay-harness after WEB-W9). Title: Beach
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

(filled at the end)
