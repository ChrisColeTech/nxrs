# WEB-RUMBLE-HID - the DualSense rumbles in the web games (WebHID)

**Status: in progress (2026-11-27).** Branch `web-hid`, base `ed9e5d6` (gameplay-harness after WEB-RUMBLE). Title:
Beach Bounce Remastered (library web build `nxrs-web\Beach Bounce Remastered nsw2u`). GPU 1
(`--use-adapter-luid=0,113326`). Evidence `build/WEB/RUMBLE-HID`.

## 1. Goal and cause

Owner: the DualSense (USB, 054c:0ce6) must rumble in the web games. WEB-RUMBLE carries the game's motor levels to
the page (input block `[56 + 2p]` low, `[57 + 2p]` high, 0..65535, already 0 while paused, unfocused or hidden and
after the Rumble / Rumble strength settings) and plays them through `Gamepad.vibrationActuator`. Chrome on Windows
gives the DualSense no actuator (WEB-RUMBLE 4.3: it is neither in `Dualshock4Controller` nor in
`HidHapticGamepad`'s table), so nothing plays. The motors are reachable from a page only through WebHID, with the
output report the native engine already writes (`src/os/src/input/sony.rs` `output`).

## 2. The report (from sony.rs)

| Transport | Report id | Bytes after the id | Flags | Motors |
|---|---|---|---|---|
| USB | `0x02` | 47 (the descriptor's size, read from `HIDDevice.collections`) | `[0] = 0x03` (compatible vibration + haptics select), `[1] = 0x00`, `[38] = 0x04` with "vibration v2" | `[2]` high (right, light), `[3]` low (left, heavy) |
| Bluetooth | `0x31` | 77: `[0] = seq << 4`, `[1] = 0x10`, then the USB layout at `[2..]`, CRC-32 (seed `0xa2` over the id and bytes 0..72) little-endian at `[73..77)` | as USB | as USB, shifted by 2 |

- **Levels:** byte = round(word x 255 / 65535), the same 0..255 native gets from `round(amp x 255)`.
- **Lights:** native's packet also sets `[1] = 0x14` (lightbar and player LED control) and the lights. The page leaves
  `[1] = 0`, so it never changes the pad's lights; no trigger effect is sent (native sends none either).
- **Vibration v2:** as native (`sony::vibration_v2`): feature report `0x20`'s firmware update version at bytes 44-45
  >= `0x0215`, always on the Edge (0df2), on when the report cannot be read.
- **Models:** DualSense (0ce6) and DualSense Edge (0df2). The DualShock 4 keeps `vibrationActuator` (Chrome drives
  it: `Dualshock4Controller`), so it never takes this path.

## 3. Plan

| Part | Change | Files |
|---|---|---|
| Report and devices | `NxrsHid`: the report builder (above), the device list (`navigator.hid.getDevices()` at start and on `connect`: a pad granted once reopens on later visits), the transport from the output report ids, v2 from feature `0x20`, one report in flight per device (the latest level waits), zero on `stopAll` | `src/web/hid-rumble.js` (new) |
| Levels | `input.js` `pollRumble`: a gamepad without a dual-rumble actuator whose id names a DualSense gets its player's levels through `NxrsHid` (the n-th such gamepad the n-th open device of that product); the same off rules (menu, blur, hidden), `stopAllRumble` stops these too; `pagehide` sends zero | `src/web/input.js` |
| Menu | Controller section: Row "Rumble on this controller" (info mark: what it does and that Chrome asks once), readout of the state and a ghost **Allow…** button (`navigator.hid.requestDevice({filters: [{vendorId: 0x054c, productId: 0x0ce6}, {.., 0x0df2}]})`), shown only when a connected pad is a DualSense without an actuator and WebHID exists; once allowed the readout says so and the button goes | `src/web/menu.js` |
| Shipping | `hid-rumble.js` in the host files, `index.html`, `dev_server.py --live` | `cli/src/webhost.rs`, `index.html`, `dev_server.py` |
| Test | node test of the report builder: byte positions, flags, scaling, Bluetooth CRC (checked against the standard CRC-32 vector sony.rs tests) | `src/web/hid-rumble.test.js` (run with `build\tools\emsdk` node; not shipped) |

## 4. Verification

- The node test; `cargo test -p nxrs-cli webhost` for the host list.
- Chrome (own profile, GPU 1, Beach Bounce, a free dev-server port whose `build.json` is Beach Bounce's): an injected
  DualSense-named standard gamepad without `vibrationActuator` and a `navigator.hid` shim (no real permission prompt
  is clicked or bypassed): the menu row shown, Allow... calls `requestDevice` with the Sony filters (logged), and with
  the shim's device "granted", the self-test's levels produce the logged USB reports (`0x02`, flags, high/low bytes),
  then zero; zero on menu open and blur.
- Relink Beach Bounce's library web build at the end.

## 5. Results

(filled in at the end)
