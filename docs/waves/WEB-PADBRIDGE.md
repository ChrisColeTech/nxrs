# WEB-PADBRIDGE - rumble and lights of web games through the launcher

**Status: in progress (2026-11-27).** Branch `web-padbridge`, base `8b7981e` (gameplay-harness). Titles: Beach
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

(filled at the end)
