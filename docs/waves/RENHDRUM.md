# RENHDRUM: should nxrs use RenHDrum for HD rumble?

Evaluation and plan, 2026-11-25. Read-only survey: nothing was built, run or felt. RenHDrum 0.5.0 at
`D:\Projects\RenHDrum-main` (MIT OR Apache-2.0, Three-Dola); nxrs at branch `gameplay-harness`.

**Verdict: integrate.** Vendor RenHDrum's Rust core as a path crate (rlib only, no DLL, no build script, Opus from the
libopus nxos already links). Call it from a new rumble module in `src/os/src/input/`, with nxrs's own HID / XInput /
cpal transports. Do it in three phases. Phase 1 is Switch Pro and Joy-Con plus the Switch 2 amplitude mode, phase 2 is
DualSense HD haptics, phase 3 is Xbox and Bluetooth "Detailed". Switch 2 controllers, DualShock 4 and WinMM stay on
the current path.

---

## 1. What nxrs does today

### Data path

1. The game calls nn::hid's SDK functions, which nxrs takes over by name (`src/os/src/services/hid.rs:1119-1129`).
   `SendVibrationValue` (`vib_one`, :956) and `detail::SendVibrationValues` (`vib_many`, :960, at most 16 values)
   read each 16-byte `VibrationValue` (`vib_send`, :903). `vib_route` (:917) resolves the npad id to a player and
   side and calls `input::vibrate_with`. `InitializeVibrationDevice` (:952) forces an idle value.
   `GetActualVibrationValue` (:967) always answers idle. `IsVibrationPermitted` answers 1 (:1127),
   `IsOunceVibrationDeviceMounted` answers 0 (:1123), and `PermitVibration` is not taken over.
2. `input::vibrate_with` (`src/os/src/input/mod.rs:263`) cleans the value (`clean`, :173) and applies the ini's
   `rumble` / `rumble_gain` / `rumble_min` (`RumbleSettings`, :208-255, read on every call). It then stores **only the
   latest value** per player and side in `RUMBLE` (:194) and sets a dirty flag.
3. The input thread ticks every 4 ms (:1480). On a tick it takes the dirty values (:1344-1408) and calls
   `Dev::rumble` (:735). `Dev::rumble` coalesces nonzero updates to one per 15 ms (:739; values in between are
   dropped). It restates a nonzero value every 0.5 s with no end (:1401), so a game that stops sending without a
   zero keeps rumbling. It stops on focus loss (`act`), at `quiet` (:788) and at `shutdown` (:1496).
4. Every write runs on the input thread. `hidio::HidDev::write` (`hidio.rs:272`) waits up to 30 ms for a write to
   complete, so a busy Bluetooth link holds up input reads.

### Per controller

| Controller | Today | File:line |
|---|---|---|
| Switch Pro (USB / BT), Online pads | One HD rumble side per band pair, nxrs's own formula (`encode_side`, `nintendo.rs:47`), report 0x10 (`rumble_report`, :39), one value per packet, at most every 15 ms on both links. No Bluetooth pacing, no 5 ms envelope, no AM/FM 3-sample packets, no resend except the 0.5 s one. The light change sends a subcommand with **neutral** rumble bytes (`mod.rs:727`), which cuts a running rumble | `mod.rs:752-767` |
| Joy-Con L / R (BT) | The same encoder, own side only, other side neutral | `mod.rs:755-758`, `:1390-1397` |
| DualSense / Edge (USB / BT) | The HD rumble is folded to two classic motor bytes, `max(amp_low)` and `max(amp_high)` of both sides, with no frequency and no voice-coil audio. Report 0x02 / 0x31 with "compatible vibration v2" (`sony.rs:82`, `:95-160`). No speaker, no triggers | `mod.rs:751`, `:799-855` |
| DualShock 4 | The same two-motor fold | `sony.rs:150-158` |
| Xbox / XInput | The same fold to XInputSetState's left (low) and right (high) motors. The trigger motors cannot be reached through XInput. Linux uses SDL2 `GameControllerRumble` | `xinput.rs:92`, `:442`, `:470` |
| Switch 2 Pro / Joy-Con 2 (BLE) | One value repeated as all three samples of each actuator block (`rumble_packet`, `switch2.rs:403`), at the same 15 ms coalescing, through the BLE writer thread (`switch2_ble.rs:81`) | `mod.rs:769-782` |
| WinMM | none | `mod.rs:568` |

### Settings

Only the game ini's Controls keys `rumble`, `rumble_gain` and `rumble_min` (`src/config/src/options.rs:56-59`) exist.
They apply live through nxconfig's reload. The F1 page has no rumble row (`src/front/src/help.rs:34-39`), and the
launcher has no rumble setting.

### Gaps

- The game's envelope is lost. Values between two sends are discarded, packets carry one sample, and HD rumble
  "texture" (notes, clicks, rain) is flattened.
- Bluetooth Pro / Joy-Con have no congestion control. Packets go out every 15 ms while a 30 ms blocking write sits
  on the input thread, which is the input-lag pattern RenHDrum's research/03 documents.
- There is no silence watchdog: rumble can stay stuck through pauses and loading screens.
- DualSense voice coils are driven as rumble motors: no HD haptics, speaker or adaptive triggers.
- Switch 2 games' amplitudes go to the pads untranslated. RenHDrum's research/11 measures them on another scale, which
  the "Mode 2" laws handle.
- Lights cut a running Nintendo rumble, and the 0.5 s restate never ends.

## 2. What RenHDrum provides, and what it does not

RenHDrum's core (`src/`, about 25k lines of Rust with tests, no dependencies, libopus 1.5.2 vendored for one
encoder) is pure. It takes `(now_us, side, amp_low, freq_low, amp_high, freq_high)` samples and gives packets,
audio frames or motor values. It **opens no device**: all I/O is the host's.

| Area | Provides | nxrs lacks it |
|---|---|---|
| Pro / Joy-Con packets | `NintendoHdEngine` (`backend/nintendo_hd.rs`): timestamped half-samples, `tick(now_us) -> 10-byte 0x10 packet`; the 5 ms AM/FM envelope planner (`planner/amfm_planner.rs`, three 5 ms samples per packet, frequency in packet); crisp-note accents; click encoding; a dense-rumble auto-slow; the dekuNukem amplitude table encoder (`encoding/standard.rs` `hd_encode_side`, with motor-overdrive protection) | all of it |
| Bluetooth pacing | Compatible (level 4: newest value, 32 ms minimum spacing, 40 ms resend while rumbling) and Detailed (5 ms framing; needs an idle-gated, write-after-input transport, which the host provides) | yes |
| DualSense HD haptics | A voice-coil DSP (`dualsense/haptics.rs`, 4-channel 48 kHz f32 for USB audio); HD Haptics (motors on channels 2-3, speaker taps on channel 1, L2/R2 trigger kicks while held: `tri_trigger.rs`, 47-byte effects blocks); Bluetooth report 0x32 and the combined report 0x36 (398 bytes every 10.667 ms, speaker as Opus 160 kb/s inside) | all of it |
| Xbox translation | `XboxRumbleMapper` (`xbox/mod.rs`): HD rumble to four ERM motors (grips by band, triggers by onsets, melody/beat classification), 10 ms USB / 20 ms BT cadence, and a Direct mode | the translation (nxrs folds to `max`) |
| Switch 2 games | `set_nx2_mode(2)`: the DualSense law `min(1, 3.5 a^0.75)`, the Pro curve with a 25 ms 1.2x low-band onset boost, and the Pro curve before the Xbox mapper | yes |
| Settings model | `renhdrum_options` / `RenHdcStore`: keys, ranges, the recommended preset | n/a |
| Steam Controller 2026 | grip reports, experimental | nxrs does not read this pad at all |

What it does **not** cover:

- Switch 2 Pro Controller and Joy-Con 2. The README lists them as "still in research".
- DualShock 4, and generic or third-party pads other than Xbox-style ones.
- Any transport. In particular, the half of Detailed Bluetooth that the patched SDL3 does (idle-gated writes,
  write after input, peak merge) is the host's job. The C bridge's host rules are also the host's: the 250 ms
  silence watchdog, the 40 ms USB resend, one trigger writer, and release on exit. In nxrs they would be
  reimplemented in Rust.
- Xbox trigger motors through XInput (only WGI or raw input reach them).
- A Linux DualSense audio path (the docs and tests are Windows / SDL).
- Joy-Con is not in its verified controller table. The engine has `is_pro_controller = false` and the SDL3 guide
  routes Joy-Con to it, but nobody has felt it in nxrs.

## 3. Integration options

| | A. Vendored Rust crate (recommended) | B. `renhdrum.dll` via the C ABI | C. Port the algorithms into `input/` |
|---|---|---|---|
| Code | upstream code byte for byte, in-process, statically linked | upstream binary | 25k lines retyped: freestyling the most-tested part |
| Packaging | nothing new to ship | a DLL beside **every built game's exe** (`buildDir`), loaded by full path, version-branch check | nothing |
| Calls | Rust API, or its `extern "C"` functions as plain Rust calls (they carry the glue: nx2 boosts, NaN cleaning, `catch_unwind`) | `LoadLibraryExW` + `GetProcAddress` in Rust: more unsafe code and more failure modes | native |
| Upgrades | re-copy `src/`, pinned version | swap the DLL + header | manual diff |
| Fit | edition 2021 like the engine; pure Rust (MSVC, gnullvm, Linux) | Windows-only as shipped | n/a |

**A** wins. The remaining questions:

- **Workspace.** Put the copy at `src/os/third_party/renhdrum/` next to `opus/`, `vpx/` and `dav1d/`, as a path
  dependency of `os`. A path dependency inside the root becomes a workspace member, so add it to the root
  `exclude` and its own unit tests (some write files) stay out of the seconds-fast `cargo test`. Its
  `[profile.release]` (fat LTO, strip) is ignored as a dependency.
- **Manifest edits** (the only ones):
  - `crate-type = ["rlib"]`. Cargo builds a dependency's declared types too, and otherwise a cdylib and a staticlib
    would be built on every engine build.
  - **Drop `build.rs` and `vendor/opus`.** nxos already links libopus 1.5.2 (`src/os/build.rs:22`,
    `third_party/opus/libopus.a`), and that archive contains the encoder (`opus_encoder_init` and
    `opus_encode_float` are present). Compiling a second libopus would give duplicate symbols at the game's link.
    RenHDrum declares its Opus functions as plain `extern "C"` (`dualsense/opus_codec.rs:33`), so they resolve
    against nxos's copy. On Linux they resolve against the system libopus nxos already links.
  - Do not copy `tests/`, `integrations/`, `examples/` or `docs/`. Record the upstream version and the SHA-256 of
    the copied tree in `third_party/renhdrum/VERSION`.
- **Device ownership.** There is no conflict. The core opens nothing, and nxrs keeps every handle. Inside nxrs one
  rule holds: **one writer per pad**. The new rumble thread writes claimed Nintendo pads through its own second
  handle on the same HID path (hidio opens with full sharing). It shares the 4-bit packet counter with the input
  thread's subcommands through an atomic in the device and rewrites byte 1 of each core packet with it. The light
  subcommand carries the last rumble bytes instead of `NEUTRAL_RUMBLE`.
- **Threading and timing.** The core wants a tick about every 1 ms. The 4 ms input thread, with its 30 ms blocking
  writes, cannot provide that. A dedicated `rumble` thread runs only while a pad is claimed and wakes every 1 ms
  (Rust's `thread::sleep` uses a high-resolution waitable timer on Windows 10). The game thread pushes
  timestamped samples straight into the pad's engine under a short mutex. The engines allocate nothing per call.
  The audio callback for DualSense USB pulls from the DSP under its own mutex. The lock order is registry, then
  engine; no lock is held across a HID write or a join.
- **Global state.** `nx2` mode is a process global. That fits nxrs (one game per process): set it once at boot from
  the program id (`runtime::process::Program::switch2`, `src/runtime/src/process.rs:27`), as the recomp bridge's
  `rh_set_switch_gen` does. `renhdrum_dualsense_dsp_create` also reads a Brontide-App shared-memory block
  (`Local\RenHDrum_SharedState_v1`) when one exists. With no such app it does nothing; leave it.
- **Panics.** nxrs keeps `panic = unwind`, so the C-ABI functions' `ffi_guard` (`catch_unwind`) works when called
  from Rust. Prefer those functions over the raw Rust types wherever they hold glue: engine create, push and tick,
  the DualSense DSP and the Xbox mapper.
- **Licensing.** MIT OR Apache-2.0 is compatible with nxrs's MIT. Take it under MIT:
  - Add `LICENSES/renhdrum.txt` (the MIT text with "Copyright (c) 2026 Three-Dola").
  - Add a README "Credits and licenses" row: *RenHDrum 0.5.0 | © 2026 Three-Dola, MIT OR Apache-2.0 (used under MIT)
    | the HD rumble core in `src/os/third_party/renhdrum/` (Pro / Joy-Con packets, 5 ms envelope, Bluetooth pacing,
    DualSense HD haptics, Xbox translation, Switch 2 amplitude laws); its recomp bridge's host rules (silence
    watchdog, resends, release on exit) reimplemented in `input/rumble.rs`*.
  - RenHDrum's NOTICE says parts came from Ryujinx / Ryuhdr (MIT). `LICENSES/ryujinx.txt` already exists; name
    RenHDrum in that row's text.
  - Extend the libopus row: "and the encoder for the DualSense Bluetooth speaker".
  - The copied files have no per-file headers, and none are added. Don't copy the GPL Brontide or longshot patches
    or the zlib SDL patches, and don't copy their code. The bridge (MIT OR Apache-2.0) is read for its rules only.
- **Binary size.** Not measured, because no build was allowed. Expect about 0.5-1 MB more in `nxrt.lib` / the game
  exe: the core plus the Opus encoder objects, which are now pulled from `libopus.a`. Engine binaries are already
  tens of MB.

## 4. Verdict

Integrate, with option A.

RenHDrum fixes nxrs's real rumble gaps: the lost envelope, Bluetooth pacing, stuck rumble, DualSense voice coils, and
Switch 2 amplitudes. It does this with code proven in five shipped hosts, including two recomps of titles in this
library (dkb-pc-recomp, mkw-recomp). Its core does no I/O, so it slots under nxrs's transports without ownership
fights. Its license and toolchain fit, and the one link hazard (a second libopus) is avoided by deleting its build
script.

It is not worth it for Switch 2 controllers, DS4, Steam Controller or Xbox triggers, because RenHDrum does not cover
them or nxrs cannot reach them. Those stay as they are, and no setting is shown for them.

## 5. Plan

Phases are separate deliverables, each ending with the owner's feel check (section 5.6). Agents follow
`docs/31-DELEGATION-PROTOCOL.md`: one build or game at a time, and commit only when asked.

### 5.1 Phase 0: vendor and wire (no behaviour change)

1. Copy `RenHDrum-main/src/**` plus `Cargo.toml`, `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE` to
   `src/os/third_party/renhdrum/`. Edit the manifest (crate-type rlib, no `build.rs`, no `[build-dependencies]`,
   no `[profile]`) and add `VERSION`.
2. Root `Cargo.toml`: add `src/os/third_party/renhdrum` to `exclude`.
3. `src/os/Cargo.toml`: `renhdrum = { path = "third_party/renhdrum" }`.
4. `LICENSES/renhdrum.txt` and the README rows from section 3.
5. Check: `cargo build --release -p runtime` links (no duplicate Opus symbols), and `cargo test -p os` still takes
   seconds.

### 5.2 Phase 1: Switch Pro, Joy-Con and Switch 2 games

**New `src/os/src/input/rumble.rs`** (the host glue, about 700 lines):

- `HdPad`: one per claimed device (`Arc`). It holds:
  - `Mutex<*mut RenHdcEngine>` (via `renhdrum_engine_create(bt, is_pro)`, then `renhdrum_settings_resolve_pure` +
    `renhdrum_engine_apply_settings` from the ini, never the bare defaults; recomp-games.md lesson 15);
  - its own output `HidDev` (a second handle on the device path);
  - the shared counter atomic;
  - `ready` (set by the input thread when the handshake has enabled vibration);
  - the last packet and its time, and the last push time per side.
- `REGISTRY: Mutex<[Vec<Arc<HdPad>>; PLAYERS]>`, kept by the input thread as slots and Joy-Con sides change. A
  Joy-Con registers for its side only.
- `push(p, side, v, now_us)`: called from `input::vibrate_with` with the **raw** cleaned value. It runs only when
  `rumble` is on, and RenHDrum's gain is the one place intensity is applied (checklist item 2). It calls
  `renhdrum_engine_push_half_sample`, and returns whether every pad of player p is claimed, so the legacy path still
  serves an unclaimed second pad of that player.
- The `rumble` thread (1 ms):
  - `tick`, then write the packet if it is due (byte 1 set from the shared counter).
  - USB: restate the last non-neutral packet after 40 ms without one. Bluetooth: after 150 ms. The core leaves this
    to the transport (`nintendo_hd.rs:372`).
  - **250 ms silence watchdog** per side: push neutral `(0, 160, 0, 320)`.
  - **Focus loss / prompt**: push neutral and mute.
  - **Exit**: `input::shutdown` stops the thread first (bounded by 200 ms like today), then writes the neutral packet
    `10 c 00 01 40 40 00 01 40 40` to each pad. A wired Pro is left in its normal mode, as today.
  - Writes are never queued. A write still pending makes the next one skip, so the newest value wins.
- `set_enabled(on)`: `hd_rumble` turned off at run time drops all claims, so `Dev::rumble` takes over again without a
  restart. Turned on, it re-claims.

**`input/mod.rs`:**

- `vibrate_with` (:263) calls `rumble::push` before `RumbleSettings::apply`. If every pad of that player is claimed,
  it skips the legacy store.
- The scan (about :1090-1120) creates the `HdPad` for `nintendo::kind` 1 / 2 / 3 when `hd_rumble` is on and the handle
  is writable.
- The rumble loop (:1344-1408) skips claimed pads.
- `set_lights` (:727) uses the pad's last rumble bytes.

**`services/hid.rs`:**

- Boot: `renhdrum_set_nx2_mode(if program.switch2() { 2 } else { 0 })`, once, from the hid service's registration or
  the boot path.
- Add `PermitVibration(bool)`. It is not taken over today, so first get its exact mangled name from a game's
  UNIMPL / import list. When false it pushes neutral to every pad and mutes; when true it unmutes.
  `IsVibrationPermitted` reflects the permission.
- `IsOunceVibrationDeviceMounted` (:1123): RenHDrum's hosts answer 1. Change it **only after** a
  `NXRS_CONTROLLER_TRACE=1` run of MKW and DKB, before and after, shows the values the game sends don't move to an
  untaken-over send function. Otherwise keep 0.

**Retire:**

- `nintendo::encode_side` (`nintendo.rs:47-83`). The legacy path (HD rumble off) uses
  `renhdrum::hd_encode_side`, the proven table with overdrive protection. Its unit test moves to the new encoder.
- The 15 ms coalescing and the endless 0.5 s restate, for claimed pads.
- Keep both for unclaimed pads. Consider giving the legacy path the same 250 ms watchdog (a small separate fix).

**Settings** (`src/config/src/options.rs`, Controls section, all live; each key is shown only if it works):

| Key | Values | Default | Help |
|---|---|---|---|
| `hd_rumble` | 1 / 0 | 1 | HD Rumble: the game's vibration in full detail on a Switch Pro Controller or Joy-Con (and DualSense / Xbox when phases 2 and 3 land). 0 = the standard rumble |
| `hd_rumble_bt` | `compatible` (phase 1), `detailed` (phase 3) | `compatible` | Bluetooth Pro Controller / Joy-Con. Compatible works on every adapter. The row appears only with phase 3, so phase 1 ships without it |

`rumble` stays the master switch. `rumble_gain` maps to the core's `gain` (x100, clamped to its 25-1000 % range)
when a pad is claimed. `rumble_min`'s help says "standard rumble only". RenHDrum's Advanced rows are not exposed: the
recommended preset is used, and no row is shown that nobody has tested. An F1 row "HD Rumble On / Off" (`help.rs`,
NOPT 4 → 5) is optional and is the owner's call.

**`src/os/src/bin/nxinput.rs`:** add `nxinput rumble --hd`. It plays a short scripted HD rumble envelope (a 5 ms note
ladder, then a click train, then silence) through the same `rumble.rs` path. That allows a feel check without a game.

### 5.3 Phase 2: DualSense HD haptics

**New `src/os/src/input/dualsense_hd.rs`** (about 500 lines):

- **USB.** Use the DSP from `renhdrum_dualsense_dsp_create(strength, crisp = true)` and a cpal output stream (cpal 0.18
  is already a dependency) on the controller's audio endpoint.
  - Find the endpoint by name, "DualSense" / "Wireless Controller", with 4 channels at 48 kHz f32. Retry every 2 s,
    because the endpoint appears 1-2 s after the HID interface. Each pad gets its own stream.
  - The callback calls `renhdrum_dualsense_dsp_fill_f32` and outputs zeros on error.
  - HD Haptics: the speaker block and the trigger blocks (`renhdrum_hd_haptics_*_block`, `tri_trigger_update` fed
    with ZL / ZR depth and `note_action` from the pad's own buttons) go out in report 0x02. They are written by the
    rumble thread only, with the player LED bytes merged in.
  - Once the stream is open, the classic motor bytes stop (motor flags off in `sony::output`).
- **Bluetooth.** The rumble thread builds `renhdrum_dualsense_dsp_build_bt36_report` every 10.667 ms (period
  `RENHDRUM_DS5_BT_PERIOD_NS`) and writes it when it returns 1. It calls `set_speaker` explicitly. Light changes still
  go in 0x31 with the motor flags cleared. Check the write length against the device's `output_len` (hidio pads to
  it).
- **Release** on unplug, focus loss and exit: speaker off block, trigger off blocks, stream closed under its lock.
- Switch 2 games: the nx2 law is applied by the core.
- **Settings** (shown once phase 2 lands): `dualsense_haptics` 1 / 0 (voice coils through the audio or 0x36 path;
  0 = today's motor fold), `dualsense_hd_haptics` 1 / 0 (speaker + L2 / R2 triggers), `dualsense_strength` 10-100.
  Changing the strength recreates the DSP only, never the stream. Linux: these rows are not shown unless the cpal
  4-channel path has been verified there.
- DualShock 4 stays on the motor fold.

### 5.4 Phase 3: Xbox and Bluetooth Detailed

- **Xbox.** `renhdrum_xbox_rumble_create(bt = true)`, because XInput cannot tell the link and RenHDrum says to treat
  unknown links as Bluetooth. Ticks run on the rumble thread. Of the four motors only the grips go to
  `XInputSetState`; triggers cannot be reached through XInput, and no trigger row is shown. A failed set calls
  `renhdrum_xbox_rumble_resend`. The setting is `xbox_hd_rumble` 1 / 0. Its default follows the owner's feel check
  against the current fold, because the mapper was tuned for four motors.
- **Bluetooth Detailed for Pro / Joy-Con.** The input thread signals a per-pad event when a report arrives. The
  rumble thread writes one packet right after it (write after input) and never queues; this is the transport half of
  SDL patch 0002, reimplemented from its description. After that, set `bt_fast` and `bt_after_input` in the resolved
  settings and show the `hd_rumble_bt` row. Keep `compatible` the default unless the owner's adapter shows no input
  lag in MKW (section 5.6).

### 5.5 Tests (CPU only, under a second, in `cargo test -p os`)

All of them use an injectable clock in `rumble.rs`, no device and no thread.

1. **Engine path.** A note sequence pushed on both sides of a USB Pro `HdPad` produces non-neutral 0x10 packets with
   byte 1 taken from the shared counter. 300 ms of silence produces the neutral packet (watchdog).
2. **Resend.** A held value restates after 40 ms on USB and 150 ms on Bluetooth, and does not restate when neutral.
3. **Claims.** A player with one claimed and one unclaimed pad still updates `RUMBLE`. With all pads claimed, it
   leaves `RUMBLE` untouched. `rumble = 0` pushes nothing.
4. **Settings.** The ini keys map to `RumbleConfigController`, then `resolve_pure`: `rumble_gain` 2.0 gives a gain of
   200, and `hd_rumble = 0` claims nothing.
5. **nx2 mode.** Set from a 04… program id and from a 01… id (serialised with a test mutex, because the mode is
   global).
6. **Legacy encoder.** The `hd_encode_side` neutral and known-value bytes, replacing `nintendo.rs`'s `rumble_encoding`
   test.
7. **Phase 2.** The 0x36 report is 398 bytes and its CRC checks out (RenHDrum's `ds5_bt_crc32`). The LED merge leaves
   the trigger bytes alone.
8. **Phase 3.** The four Xbox motors become XInput (left grip, right grip) and the triggers are dropped.

RenHDrum's own test suite is not run by default. It needs its bundled Opus, and can be run from an upstream checkout
when the copy is updated.

### 5.6 How a human verifies it (a physical controller is required)

Agents can show packets flow but cannot feel them. They run `scripts/play.py <scenario> --env
NXRS_CONTROLLER_TRACE=1` and check that the log has `rumble` packets during play and the neutral packet at exit.
Feel is the owner's.

Controllers: a Switch Pro Controller (USB cable, then Bluetooth), a Joy-Con pair (Bluetooth), a DualSense (USB, then
Bluetooth; phase 2), an Xbox pad (phase 3), and a Switch 2 Pro Controller (to confirm its unchanged path still
rumbles). Compare each scene with `hd_rumble=1` and `hd_rumble=0` (live ini edit).

| Title (gen) | Scene | What to feel / check |
|---|---|---|
| none | `nxinput rumble --hd` | note ladder distinct pitches, crisp clicks, clean stop |
| Mario Kart 8 Deluxe (S1) | a 50cc race | engine idle texture, drift sparks, boost pads, item hit; no endless buzz on the pause menu |
| Super Mario Bros. Wonder (S1) | World 1-1 | jump and land taps, coin ticks, pipe; detail over USB vs Bluetooth Compatible |
| Super Smash Bros. Ultimate (S1) | training, smash attacks | sharp hit transients; DualSense trigger kick while holding a trigger (phase 2) |
| Mario Kart World (S2) | Grand Prix, Mario Bros. Circuit | continuous road rumble at a sensible strength (nx2 mode), and **no button lag over Bluetooth** (the RenHDrum lesson 13 case) |
| Donkey Kong Bananza (S2) | first area, punching terrain | heavy hits; rumble stops within ~0.25 s on pause / loading |
| Super Mario Party Jamboree or MK8D two players | player 1 Pro, player 2 Xbox / DualSense | each pad gets only its own player's rumble, no doubling |

Every exit path must leave no rumble running, the speaker and triggers released, and a wired Pro reading normally
in `joy.cpl` afterwards. The paths are window close, Esc quit, Home-hold quit, focus loss (Alt+Tab, rumble stops) and
a DualSense USB unplug and replug mid-game.

### 5.7 Risks

- **Bluetooth Detailed depends on our transport.** A wrong write-after-input causes the input lag it is meant to
  prevent. That is why it lives in phase 3, behind a row and with Compatible as the default.
- **DualSense audio endpoint.** Two DualSenses can only be told apart by name order. A WASAPI mix format other than
  4-channel 48 kHz means no USB haptics: fall back to the motor fold and log it.
- **`IsOunceVibrationDeviceMounted` = 1** may change what Switch 2 games send. It is trace-gated (section 5.2).
- **The Xbox mapper on grips only** may feel worse than the fold. The owner's check decides the default.
- **Joy-Con** has no verification upstream. If it feels wrong, Joy-Con stays unclaimed: the check is simple.
- **Upstream churn.** 0.4.0 came out on 11-14 and 0.5.0 on 11-20. Pin the version, and re-copy deliberately (re-run
  tests 1-8 and the feel table).
- **Hidden external override.** The Brontide-App shared-memory block can change DualSense settings if that app runs.
  This is acceptable, and noted in the README row.

### 5.8 Size

| Phase | New / changed | Estimate |
|---|---|---|
| 0 | vendor copy, 3 manifests, LICENSES, README | small; 1 build |
| 1 | `rumble.rs` ~700, `mod.rs` ~80, `hid.rs` ~40, `options.rs` ~5, `nxinput.rs` ~60, tests ~250 | 1.5-2 agent days + owner check |
| 2 | `dualsense_hd.rs` ~500, `sony.rs` / `mod.rs` ~60, options ~10, tests ~80 | 1.5-2 days + owner check |
| 3 | Xbox ~120, BT after-input ~150, options ~5, tests ~40 | 1 day + owner check |

The total is about 2k lines of nxrs code, with RenHDrum's 25k used as a crate.

## Brief: phases 0 and 1 (lead, 2026-11-25)

**Goal / done.** Implement §5.1 (vendor and wire) and §5.2 (Switch Pro / Joy-Con / Switch 2 games) as written above.
Done = `cargo build --release -p runtime` links with one libopus; `cargo test -p os` passes and stays seconds-fast,
with the §5.5 tests for phase 1; the engine builds (`build.ps1 -SkipGnu -KeepCache`); `nxinput rumble --hd` exists;
if a Pro Controller or Joy-Con is connected (`nxinput` lists devices), a `NXRS_CONTROLLER_TRACE=1` run of
`nxinput rumble --hd` and of one game (`py scripts/play.py dkb --env NXGFX_GPU=1`, DKB is a rumble-heavy pass title)
shows HD packets written and neutral on exit; with no Nintendo pad connected, say so and show the legacy path is
unchanged (same game run, no red lines, normal exit). The final check is the owner's feel test (§5.6): list exactly
what the owner should try.

**Scope.** Phases 0 and 1 only — no DualSense (phase 2), no Xbox / Bluetooth Detailed (phase 3). Skip the
`IsOunceVibrationDeviceMounted` change unless the before/after trace in §5.2 is done. The F1 row is not added (owner's
call, ask in Questions).

**Machine / tree.** Builds and games may overlap at full parallelism; games on GPU 1 (`--env NXGFX_GPU=1`). Another
agent (FRESH-SAVE) runs Arceus/Jamboree routes on GPU 1 and edits `scripts/scenarios.toml` — don't touch it. The tree
carries the lead's and earlier waves' accepted uncommitted edits: leave them; never revert or reformat other hunks.

**Rules:** `AGENTS.md` (editor tools only; copying the vendored crate's files with `cp` is fine — it is a copy, not a
rewrite; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol §3. Foreground commands under 10 minutes; never
end a turn waiting on a background notification. Don't edit `docs/32-REMAINING-WORK-PLAN.md`. **No commits.**
**Deadline:** 2 hours wall clock; park after ~90 min; no early hand-off.

## Report (agent, protocol §5)

Phases 0 and 1, 2026-11-25. No Nintendo controller is connected to this machine (`nxinput list`: no HID pads, no
XInput user, no WinMM joystick; no `VID_057E` device present), so HD packets have been tested on the CPU and not
written to a real pad. The owner's feel test (below) is what's left.

**Built**

- *Phase 0 (vendor and wire).* `src/os/third_party/renhdrum/` holds RenHDrum 0.5.0: `src/**` (44 files, unchanged),
  `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE`, plus an edited `Cargo.toml` (rlib only, `build = false`, no
  `[build-dependencies]`, `[profile]` or `readme`) and a `VERSION` file with the source and the SHA-256 of the copied
  tree (`44e7e79c…35683`). `build.rs`, `vendor/opus`, `tests/`, `integrations/`, `examples/` and `docs/` were not
  copied. In the root `Cargo.toml` the crate is in `exclude`, and `src/os/Cargo.toml` declares it as a path
  dependency. The licence goes in `LICENSES/renhdrum.txt` (MIT text, Three-Dola; it also notes the Ryujinx/Ryuhdr
  credit and the libopus encoder use). The README Credits paragraph names RenHDrum. The README no longer has a
  per-library "Credits and licenses" table, so §3's "row" edits went into that paragraph and the LICENSES file.
- *Phase 1.* New `src/os/src/input/rumble.rs`:
  - `HdPad` holds the core engine (`renhdrum_engine_create`, then `renhdrum_engine_apply_settings` with settings from
    a `RenHdcStore` → `renhdrum_store_to_config` → `renhdrum_settings_resolve_pure`), a second output handle on the
    pad's HID path, the 4-bit counter shared with the input thread's subcommands, and `ready`.
  - The settings are the recommended preset with Bluetooth Compatible (`switch_bt_mode` = Compatible,
    `bt_after_input` off) and `gain` = `rumble_gain` × 100.
  - Amplitudes are shaped by `renhdrum_store_shape_amplitude`, the only place intensity is applied.
  - A registry per player.
  - `push` / `push_with` return whether every rumble pad of the player is claimed.
  - A `rumble` thread (1 ms) runs these host rules: the 250 ms watchdog per side, a restate of a running packet
    (40 ms USB, 150 ms Bluetooth), mute plus neutral on focus loss, prompt, F1, `rumble=0` or
    `PermitVibration(false)`, and writes that are never queued (synchronous, so the newest value wins).
  - `release()` handles `hd_rumble` turned off live, and `shutdown()` stops the thread (bounded to 200 ms), then
    writes the neutral packet to each pad.
  - `set_program()` sets nx2 mode 2 for 04… programs.
  - `encode_side()` is RenHDrum's `hd_encode_side`.
- `input/mod.rs`:
  - `vibrate_with` pushes the raw cleaned value first and skips the standard store when every pad of that player is
    claimed.
  - `Inputs::hd_update` (each tick) claims writable Nintendo pads (kind 1/2/3; a Joy-Con only its own side) while
    `hd_rumble` is on, releases them when it goes off, sets `ready` once the handshake is done (step 9), and syncs
    the registry with each player's count of unclaimed rumble pads.
  - The rumble loop skips claimed pads.
  - Handshake, light and standard-rumble subcommands take the counter from the claimed pad.
  - The light subcommand carries the last rumble bytes, not `NEUTRAL_RUMBLE`, for both paths, so a light change no
    longer cuts a running rumble.
  - `shutdown()` calls `rumble::shutdown()` first.
  - The standard path's Nintendo encoder is now `rumble::encode_side`. `nintendo::encode_side` and its test are gone.
- `services/hid.rs` takes over `nn::hid::system::PermitVibration(bool)` and `nn::hid::detail::PermitVibration(bool)`.
  I found the exact names by decompressing DKB's `sdk` NSO: there is no plain `nn::hid::PermitVibration` there.
  `IsVibrationPermitted` now reflects the permission.
- `boot.rs` calls `input::rumble::set_program(prof.program_id)` once, after the profile is loaded.
- Settings: `hd_rumble` (1 / 0, default 1, applies live) is in the Controls section of `src/config/src/options.rs`
  and in `docs/settings.md`, and `rumble_min`'s help now says "Standard rumble only". `hd_rumble_bt` is not added (it
  waits for phase 3).
- `nxinput rumble --hd`, in `src/os/src/bin/nxinput.rs`:
  - It waits until the claimed pads are ready, then sends this script through `input::vibrate` every 5 ms: an
    8-note ladder from 160 to 1000 Hz with a decaying 5 ms-step envelope, ten 10 ms clicks, then silence.
  - After the script it waits 400 ms and calls `input::shutdown`.
  - The plain `nxinput rumble` now re-sends its value every tick, as a game does, so the HD watchdog does not cut it.

**Tests** (`cargo test -p os`: 165 passed, 1 ignored, 0.38 s; all new, so none can have failed before):

- `rumble::tests::engine_path_packets_counter_watchdog` covers plan test 1. A note sequence on both sides of a USB Pro
  pad gives rumbling 0x10 packets whose byte 1 follows the shared counter (a subcommand's `next_counter` takes the
  next one). After the last push it ends neutral, between 240 and 290 ms later (watchdog plus the core's envelope
  window).
- `resend_usb_bt_not_neutral` covers test 2. A held value never goes more than 40 ms (USB) or 150 ms (Bluetooth)
  without a packet. Once neutral, nothing is restated.
- `claims_and_master_switch` covers test 3. With one claimed and one unclaimed pad, `vibrate` still marks `RUMBLE`
  dirty. With all pads claimed it leaves `RUMBLE` alone. `rumble=0` pushes nothing: no packet in 200 ms.
- `settings_from_ini_keys` covers test 4. `rumble_gain` 2.0 gives gain 200 and the shape doubles. Gain 0 is silent,
  and 50.0 clamps to 1000. The config is HD on, Bluetooth level 4, `bt_fast` and `bt_after_input` off. The resolved
  settings have `envelope_5ms` and `usb_no_queue` on. With `hd_rumble=0`, nothing is claimed.
- `nx2_mode_from_program` covers test 5: 04… gives 2 and 01… gives 0, serialised under a test lock.
- `legacy_encoder_table` covers test 6. Neutral; full high band at 320 Hz gives `00 c9 40 40`; full low band at
  160 Hz gives `00 01 40 72`; both at full scale trigger the motor protection.
- `cargo test -p config` passes. The existing `input::tests` pass, including `vibration_init_sends_idle`.

**Verified**

- *Link, one libopus.* `cargo build --release -p runtime` passes. `scripts/build/build.ps1 -SkipGnu -KeepCache`
  passes (rc 0, 59 s). DKB was relinked through `nxrs-build` into its library `buildDir` with no duplicate or
  unresolved symbols. `Donkey Kong Bananza_code.dll` grew from 210,053,632 to 210,543,104 bytes (+0.47 MB, about the
  core plus the Opus encoder objects pulled from `libopus.a`). The DLL contains the rumble module's strings and
  `opus_encode_float`.
- *No Nintendo pad, so the legacy path.* `py scripts/play.py dkb --env NXGFX_GPU=1 --env NXRS_CONTROLLER_TRACE=1`
  completed 15/15 steps with a normal exit and 0 red lines. FPS in [100, 150] s: mean 58.5, 5 % low 53.0. Run folder:
  `build/play/20261125-045606-dkb`; `sheet-1.png` shows boot, Continue, walking, punching and the dig tutorial.
- *What DKB sends.* The game sent 19,086 traced `hid: vibration` values, one per side **every ~5 ms** (from 23.3 s on).
  The game feeds rumble continuously, so the 250 ms watchdog does not cut held rumble in this title.
- *nxinput.* `nxinput rumble --hd` with no pads prints `no controllers` and exits 0.
- *Not shown.* Real HD packets and the exit neutral packet on a pad: none is connected.
- *Unrelated to this change.* The run's worst pacing was a single 4.9 s frame at 86.8 s, during boot and Continue
  loading. No pad was claimed, so the rumble code never ran.

**Unproven**

- Packets on real hardware, over USB and Bluetooth: the trace run of `nxinput rumble --hd` and of DKB with a Pro
  Controller or Joy-Con. The log should show `<pad>: HD rumble (RenHDrum 0.5.0, …)`, `HD rumble [10, …]` lines while
  playing (with `NXRS_CONTROLLER_TRACE=1`), and `HD rumble neutral at exit`.
- Feel (§5.6) and Joy-Con, which RenHDrum has not verified upstream.
- The writes run on their own thread but still use hidio's 30 ms synchronous write. A congested Bluetooth link delays
  only the rumble thread now, not input reads. That has not been measured on an adapter.
- `IsOunceVibrationDeviceMounted` is still 0. The before/after MKW + DKB trace needs a pad, so it was not done.
- The SDK also has `nn::hid::detail::SendVibrationValue`, `EnqueueVibrationValue` / `FlushEnqueuedVibrationValues` and
  `SendVibrationGcErmCommand`, which nxrs does not take over. DKB's values all arrive through the taken-over
  functions. If a title is silent, check those next.
- The 250 ms watchdog follows RenHDrum's hosts. A game that sends a value once and holds it would be cut after
  250 ms on claimed pads (DKB re-sends every 5 ms).

**Slips:** None. No commits, `scripts/scenarios.toml` and `docs/32` were not touched, and processes were killed by PID
only. `rustfmt` ran only on the new `rumble.rs`: other edited files already carried others' uncommitted hunks, so they
were not reformatted. `cargo clippy -p os` fails on three existing `0.7071` lints in `services/audio.rs`, which I did
not touch. The new code adds no clippy errors.

**Questions**

1. The F1 row "HD Rumble On / Off" (`help.rs`, NOPT 4 → 5): add it? I'd wait for the feel test. If the owner flips it
   often, add the row. Until then the ini key applies live.

**Owner's feel-test checklist** (Pro Controller over USB, then Bluetooth; then a Joy-Con pair over Bluetooth; set
`NXRS_CONTROLLER_TRACE=1` so the log shows the packets):

1. `target\release\nxinput.exe rumble --hd`. It should print "N pad(s) claimed for HD rumble, N ready", then you
   should feel eight distinct rising pitches (each fading within its 120 ms), ten crisp separate clicks, and a clean
   stop with no tail. Then run `nxinput rumble` (the standard test) and compare.
2. DKB (`py scripts/play.py dkb --env NXRS_CONTROLLER_TRACE=1`, or play it): punch terrain, which should give heavy
   hits and fine texture. Pause, or wait for a load: rumble should stop within about 0.25 s. Alt+Tab mid-rumble
   should stop it at once, and it should come back on refocus.
3. MK8D, a 50cc race: engine idle texture, drift sparks, boost pads, an item hit. On the pause menu, no endless buzz.
4. Mario Kart World over **Bluetooth**: road rumble at a sensible strength (nx2 mode), and **no button lag** compared
   with `hd_rumble=0`.
5. Wonder, World 1-1: jump and land taps, coin ticks. USB vs Bluetooth Compatible: Bluetooth is coarser, USB has the
   5 ms detail.
6. Live toggle mid-game: set `hd_rumble=0` in the game's ini. The standard rumble should take over with no restart
   (log: "HD rumble off; standard rumble"). Set it back to 1 to re-claim. With `rumble_gain=2`, HD should feel about
   twice as strong; with `rumble=0`, there should be nothing.
7. Exits: window close, Esc quit, Home-hold quit. No rumble left running, and a wired Pro reads normally in
   `joy.cpl` afterwards.
8. Lights: when a player LED changes mid-rumble (a pad joining), the rumble must not cut out.
9. Two players: player 1 on a Pro, player 2 on Xbox or DualSense. Each pad gets only its own player's rumble. Also
   check that a Switch 2 Pro Controller (unchanged path) still rumbles.
