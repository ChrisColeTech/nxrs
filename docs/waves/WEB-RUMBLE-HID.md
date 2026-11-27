# WEB-RUMBLE-HID - the DualSense rumbles in the web games (WebHID)

**Status: built and verified up to Chrome's permission prompt (2026-11-27); results in section 5; a felt motor
is the owner's manual test (5.5).** Branch `web-hid`, base `ed9e5d6` (gameplay-harness after WEB-RUMBLE). Title:
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

### 5.1 Built

As planned in section 3 (`432857a`). Details: the report is the native one with the light flags left at 0 (section
2); one report in flight per device, a newer level replacing a waiting one, a report only when the 0..255 level
changes (the DualSense holds its motors until told otherwise, so a held level is not re-sent); a gamepad whose id
names a DualSense and has no dual-rumble actuator takes WebHID, any other pad stays on `vibrationActuator` (the
DualShock 4 included). Stops: menu open, blur and hidden go through `stopAllRumble` (now also `NxrsHid.stopAll`),
`pagehide` sends zero, an open device no gamepad takes is set to zero. The menu row appears only when WebHID exists
and a connected pad is such a DualSense; "Allow…" from a gamepad's A has no user activation in Chrome, so it shows
"Not allowed: …" (Chrome's reason); a mouse click or Enter works.

### 5.2 Tests

- `src/web/hid-rumble.test.js` (node 24.19 from `build\tools\emsdk\node`): 5/5 pass: CRC-32 standard vector (as
  sony.rs `crc32_matches_the_standard`); word -> byte rounding (32768 -> 128 = native's `(0.5 * 255 + 0.5) as u32`);
  USB 0x02 47 bytes `03 00 <high> <low>`, nothing else set, v2 at data[38] (packet [39]), the descriptor's larger
  size kept; Bluetooth 0x31 77 bytes `seq<<4 10 03 00 <high> <low>`, CRC at packet [74..78) over `a2 31 ...`, v2
  shifted; gamepad ids (0ce6, 0df2 yes; DualShock 4 and Xbox no) and the filters.
- `cargo test --release -p nxrs-cli webhost`: 2/2 (the host list and build.json now carry `hid-rumble.js`; the count
  assertions failed before they were updated for the eleventh host file).

### 5.3 Chrome (Beach Bounce, own profiles, GPU 1, dev server on 8795 serving Beach Bounce's build.json)

Driver `build/WEB/RUMBLE-HID/tools/drive.mjs`: a DualSense-named standard gamepad with `vibrationActuator` null; in
mode `shim` a `navigator.hid` test double whose `requestDevice` logs its arguments and requires user activation, and
whose `sendReport` logs the bytes. Page `?env=NXRS_CONTROLLER_SELFTEST=1,NXRS_CONTROLLER_TRACE=1`.

- `run2-shim`: Esc, Controller: the row "Rumble on this controller · Not allowed yet · Allow…"
  (`1-controller-row.png`). A real CDP mouse click on Allow…: `requestDevice {"filters":[{"vendorId":1356 (054c),
  "productId":3302 (0ce6)},{.., 3570 (0df2)}]} userActivation true`; the device opened (`USB, report 0x02, 47 bytes,
  vibration v2 on`), the readout "On: DualSense Wireless Controller" (`2-allowed.png`). Reload: `getDevices -> 1`,
  reopened without a click; the self-test: `sendReport 0x02 47 bytes: 03 00 00 80 ... 04 ...` (heavy 128), 0.5 s
  later `03 00 80 00` (light 128), 0.5 s later `03 00 00 00` (stop). Reload with the pad connected after the game
  was up, Esc during the heavy motor: `report 0x02 low 0 high 0 (menu open)` 160 ms later, and no report while the
  menu stayed open although the self-test's light phase ran in the engine.
- `run3-shim-hidden`: during the heavy motor, `visibilitychange` to hidden: `low 0 high 0 (page hidden)`; then
  `pagehide`: `low 0 high 0 (page closing)` (both events dispatched in the page; a tab in front did not hide the
  page under CDP focus emulation).
- `run4-real-prompt`: Chrome's own `navigator.hid`; the same click on Allow…: `rumble: requestDevice
  [{"vendorId":"0x054c","productId":"0x0ce6"},{"vendorId":"0x054c","productId":"0x0df2"}]`, no rejection while the
  chooser was up (a missing user activation rejects at once); nothing was chosen, Chrome closed by PID. CDP
  `DeviceAccess` reports no HID prompt, so the chooser itself is not in the evidence. No DualSense was connected.
- `run5-relinked`: not valid: the first relink used this worktree's `nxrs-build` without the web engine library
  (`scripts/build/web.ps1 -Crates os` not yet run here), so the game stopped (`missing function:
  nxrt_patch_table`). Built the library, relinked again (game.wasm 91.19 MB as before).
- `run6-relinked`: the library's relinked build served as built (no `--live`): the game runs; the row, Allow… with
  the Sony filters, "On: DualSense Wireless Controller", reopened on reload, the self-test's reports `03 00 00 80`,
  `03 00 80 00`, `03 00 00 00`, and the stop at menu open (`(menu open)` 160 ms after the heavy motor began).

### 5.4 Unproven

A motor felt on the owner's DualSense (the real chooser cannot be clicked here); Bluetooth (report 0x31 built and
tested in node only; sending 0x31 also switches a DualSense to its full input reports, as natively, which Chrome's
gamepad reading over Bluetooth was not checked against); the real `receiveFeatureReport(0x20)` layout (either
layout is read; failure means v2 on, as native); several DualSenses at once.

### 5.5 Manual test for the owner

1. Plug the DualSense in by USB. Launcher: Beach Bounce Remastered, Play -> Web. Press a button on the pad once the
   loading screen shows (Chrome shows a pad only after a press).
2. Esc (or the PS button), Controller. The row "Rumble on this controller" shows "Not allowed yet" and **Allow…**.
   Click Allow… with the mouse, pick "DualSense Wireless Controller" in Chrome's list, Connect. The readout becomes
   "On: DualSense Wireless Controller". Close the menu (Esc).
3. Add `?env=NXRS_CONTROLLER_SELFTEST=1` to the page's address (after `index.html` or `/`) and reload; press a pad
   button once the loading screen shows. Feel: the heavy (left) motor 0.5 s, then the light (right) motor 0.5 s, then
   nothing. No new Allow… is needed (the console shows `rumble: 1 controller(s) allowed before, reopening` and
   `rumble DualSense Wireless Controller: report 0x02 low 128 high 0`).
4. The lights of the pad do not change.
