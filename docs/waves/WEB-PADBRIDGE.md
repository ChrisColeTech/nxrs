# WEB-PADBRIDGE - rumble and lights of web games through the launcher

**Status: built and verified with the owner's DualSense on USB (2026-11-27); results in section 4; a felt motor and
the seen lights are the owner's manual test (4.5).** Branch `web-padbridge`, base `8b7981e` (gameplay-harness). Titles: Beach
Bounce Remastered / Anime Girls (web builds; run only, not relinked unless needed). GPU 1
(`--use-adapter-luid=0,113326`). Evidence `build/WEB/PADBRIDGE`.

## 1. Goal and cause

Owner: on web builds controller rumble and lights do not work. Chrome on Windows gives the DualSense (USB 054c:0ce6)
no `vibrationActuator` (WEB-RUMBLE 4.3); the WebHID path (WEB-RUMBLE-HID) needs a manual "Allow…" no profile has
granted; lights were never sent on the web (the HID wave leaves `[1] = 0`). The native engine drives rumble and
lights for DualSense, DualShock 4, Switch Pro / Joy-Con and Xbox pads (`src/os/src/input/mod.rs`).

Lead's decision: a web game played through the launcher sends its pad output to the launcher's server, and the
launcher forwards it to a child `nxrs-build pad-bridge` that drives the real pads with the engine's own native pad
code. No browser permission. Input stays on the Gamepad API.

## 2. Design

| Part | Change | Files |
|---|---|---|
| Page | `NxrsPadBridge`: a WebSocket to `/<slug>/pad-bridge` on the page's own origin. Each poll (and every 250 ms as a heartbeat) it sends one text line `pad <active> <low1> <high1> .. <low4> <high4>` when it changed or the heartbeat is due: `active` = focused, visible and menu closed (what native takes from window focus and the F1 page), the levels the input block's words `[56..64)` (already 0 when not active). Zero at once on menu open, blur, hidden, `pagehide`. While the socket is open the page's own vibrationActuator / WebHID paths are off and the menu's WebHID row is hidden; with no bridge (dev server, a static host, an old launcher) they work as before | `src/web/pad-bridge.js` (new), `input.js`, `index.html`, `webhost.rs` `HOST_FILES`, `dev_server.py --live` |
| Launcher | `web_reply::serve`: a `GET /<slug>/pad-bridge` with `Upgrade: websocket` is answered `101` (RFC 6455 accept key: SHA-1 + base64) when the mount has an nxrs-build, else `503`; the connection then belongs to `pad_bridge`: it starts `nxrs-build pad-bridge` (stdin piped, stdout+stderr to `<webDir>\pad-bridge.log`), checks each text frame (`model::pad_line`) and writes it to the child's stdin; ping -> pong; close, a broken socket or 5 s without a frame end it: stdin closed (the child stops the motors, turns the lights off and exits), the child waited for 2 s, else killed by its handle | `services/pad_bridge.rs` (new), `model/ws.rs` (new: SHA-1, accept key, frame header), `model/pad_line.rs` (new), `services/web_reply.rs`, `constants` |
| Engine | `nxrs-build pad-bridge` (Windows): the engine's input layer in bridge mode: the input thread as in a native run (scan, slots, `set_lights`, `Dev::rumble`, `sony_write` ...), with `set_focus(active)` and every player's levels put into `RUMBLE` as the native standard path would hold them (no settings again: the page's levels already carry them). Bridge mode leaves out what would take a pad from Chrome: Switch 2 BLE connections and Sony pads over Bluetooth (sending the 0x31/0x11 output switches them to report modes Chrome's RawInput does not read); HD rumble off (the page carries the two standard levels only). Watchdog: no line for 1 s -> motors zero, lights off (inactive) until the next line; stdin EOF -> `shutdown()` (quiet) and exit 0. `NXRS_CONTROLLER_TRACE=1` is set for the child, so the log has every output report's bytes and write result | `src/os/src/input/bridge.rs` (new), `input/mod.rs`, `src/cli/src/main.rs`, `src/cli/Cargo.toml` (os, Windows only) |
| Contract | the `pad-bridge` verb (contract 2, added by this wave; an nxrs-build without it exits 2 and the page falls back) | `docs/LAUNCHER_CONTRACT.md` |

Slots and lights are the native rule (`Slots::take_with`, `sony_leds`, `nintendo::lights`): the first pad is player
1. The page's gamepad i takes player i's levels; with one pad both are player 1.

## 3. Verification

- Engine: `cargo test --release -p os input::bridge` (line parsing, watchdog schedule), `-p nxrs-cli webhost`.
- Launcher: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`: SHA-1 vectors, RFC 6455's
  accept example, frame decoding (masked, 16-bit length, close), the line check, an end-to-end server test with
  the fake engine answering `pad-bridge` (the lines reach its stdin).
- Node: `pad-bridge.test.js` (line encoding, change + heartbeat rule, URL).
- Run: launcher built from this branch, `NXRS_DATA_DIR` on a temp copy of the owner's library, Play > Web on Beach
  Bounce, the page in a separate Chrome (`--user-data-dir`, GPU 1) with `?env=NXRS_CONTROLLER_SELFTEST=1`;
  `pad-bridge.log` shows the DualSense's output reports (bytes, write results) and its lights.

## 4. Results

### 4.1 Built

As planned in section 2 (`d55afef`). Details: the launcher's frame reader takes only whole, masked frames of at most
1 KiB (a fragmented, unmasked or larger frame closes the socket); it forwards only lines that match the engine's
grammar. The bridge logs, in bridge mode, every DualSense / DualShock 4 output report it wrote (bytes) for its first
400 writes (a failed write: `output FAILED ... error=<Win32>`; "output sent" is printed only for a write that
completed whole) and every XInput write with its status. The fake engine answers `pad-bridge` (echo) for the
launcher's tests (`src/bin/fake_engine/running.rs`, with `sleep` moved there to keep the file at 200 lines).

### 4.2 Tests

- Engine: `cargo test --release -p os -p nxrs-cli`: os 181 pass (new `input::bridge::tests::pad_lines`: the line
  grammar, active 0/1, eight 16-bit levels, nothing more), nxrs-cli 45 pass (the `webhost` host-list and
  `build.json` assertions updated for the twelfth host file `pad-bridge.js`; they failed before the update).
- Launcher: `cargo test` all pass; `cargo clippy --all-targets -- -D warnings` clean; `cargo fmt`. New:
  `model::ws_tests` (SHA-1 against FIPS 180 vectors incl. one million `a`, RFC 6455's accept example, frame heads,
  5.7's masked "Hello", server frames, the route, the line rule) and `tests/web_pad_bridge.rs` (upgrade answered 101
  with the RFC accept key; a pad line reaches the fake bridge's stdin, a non-pad text frame does not; ping -> pong;
  a close frame and a dropped socket both end the bridge; 400 without a key, 404 for an unknown build, 503 with no
  nxrs-build).
- Node (`build\tools\emsdk\node`): `src/web/pad-bridge.test.js` 5/5 (route beside the page, line encoding and
  clamping, inactive = zeros, change/heartbeat rule, no socket = no bridge); `hid-rumble.test.js` still 5/5.

### 4.3 Verified (evidence `build/WEB/PADBRIDGE`)

- **Engine alone** (before the page): `nxrs-build pad-bridge` fed four lines then silence then EOF: the owner's
  DualSense (054c:0ce6, USB, firmware 0630, vibration v2) opened read+write; lightbar release, then on `active=1`
  report `02 03 14 ... [39]=04 ... [44..48)=04 00 28 80` (player 1 centre LED, blue lightbar), low 128 at byte 4,
  high 128 at byte 3, stop; the watchdog after 1 s: motors 0, LEDs and lightbar off; EOF: quiet, exit 0.
- **Play > Web from the branch launcher** (`target\release\nxrs.exe`, `NXRS_DATA_DIR` = a scratch copy of the
  owner's `library.json`, `NXRS_BUILD` = this branch's nxrs-build, `NXRS_WEB_PORT=8797`, `NXRS_BROWSER` = a script
  that records the address). The Native / Web chooser could not be clicked from here (its child viewport has no UI
  Automation tree and a synthetic click did not land), so the scratch entry's native build folder was pointed at a
  folder that does not exist: Play then runs the only version that can (`ask_target` -> `play_in_browser`, the same
  path Web takes). UI Automation invoked "Play Beach Bounce Remastered"; the launcher served
  `http://127.0.0.1:8797/beach-bounce-remastered-nsw2u/`. The library's Beach Bounce web build got this branch's
  `index.html`, `input.js`, `pad-bridge.js` for the run (originals backed up in `backup/`, restored afterwards, the
  new file and the log removed: the lead's relink brings them).
- **Chrome** (own profile, `--use-adapter-luid=0,113326`, no other game Chrome running; driver `tools/drive.mjs`, an
  injected standard gamepad named as the DualSense because Chrome shows a real pad only after a press), page
  `?env=NXRS_CONTROLLER_SELFTEST=1` (`run1`): title menu drawn (`t040.png`), the menu opens (`t065-menu.png`).
  `run1/page.log` and `run1/pad-bridge.log`, same clock within ~0.9 s:

  ```
  page 0.35  pads (launcher): rumble and lights through the launcher's pad bridge
  page 4.80  input: rumble self-test: player 1 low motor 0.5 s, high motor 0.5 s, stop
  page 4.82  pads (launcher): pad 1 32768 0 0 0 0 0 0 0
  bridge 4.447 DualSense 054c:0ce6 USB: output sent id=02 bytes=48 low=0 high=0 leds=0x04   (input active: lights)
         report 02 03 14 00 00 .. 04 00 00 00 00 04 00 28 80
  bridge 4.463 output sent low=128 high=0 leds=0x04    report 02 03 14 00 80 .. 04 00 28 80
  bridge 4.963 output sent low=0 high=128 leds=0x04    report 02 03 14 80 00 .. 04 00 28 80
  bridge 5.463 output sent low=0 high=0 leds=0x04      report 02 03 14 00 00 .. 04 00 28 80
  page 42.04 pads (launcher): stop (menu open)
  bridge 41.687 output sent low=0 high=0 leds=0x00     report ... 04 00 00 00 00 00 00 00 00   (lights off)
  page 67.52 menu closed -> bridge 67.159 leds=0x04 (lights back)
  page 89.35 pads (launcher): stop (page closing)
  bridge 88.986 the page's connection ended: motors stopped, lights off; two zero reports, exit
  launcher: pad-bridge beach-bounce-remastered-nsw2u: page gone, bridge ended (347 lines forwarded)
  ```

  Every write completed (no `FAILED` line). Input stayed the browser's (Chrome still listed its pad; the bridge
  opens the DualSense shared, as native does alongside Steam).

### 4.4 Unproven

- A motor felt or the lightbar seen: the agent cannot feel or see the pad; the log shows the reports were written
  whole to the DualSense's HID interface.
- The chooser's **Web** button itself (same `play_in_browser` reached through Play; the button is the chooser's).
- XInput, DualShock 4 and Nintendo pads through the bridge (same native code as a native run; none connected).
- Several pads: the bridge's slots are the native rule, the page's levels are by gamepad index; with more than one
  pad the two orders can differ.
- Bluetooth Sony pads get no output from the bridge (by design: the 0x31/0x11 output would break Chrome's reading of
  their input); such a pad keeps the page's WebHID path only when the bridge is absent.
- A real (not injected) gamepad pressed in Chrome while the bridge runs (expected unaffected: shared open).

### 4.5 Manual test for the owner

Launcher from this branch: Beach Bounce, Play > Web; add `?env=NXRS_CONTROLLER_SELFTEST=1` to the address and reload,
press a pad button. With the page in front: the DualSense's player LED (centre) and blue lightbar come on, the left
motor runs 0.5 s, then the right one 0.5 s. Esc: lights off; Esc again: lights back. Close the tab: lights off. No
"Allow…" is needed (the menu's WebHID row is hidden while the bridge drives the pads).
