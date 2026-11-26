# WEB-W3 — Host I/O for the web build: streamed data, browser saves, audio ring, input block

Owner: one delegated agent. Lead: main session. Opened 2026-11-26. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.
Worktree `D:\Projects\RecompLauncherEngine-web-w3`, branch `web-w3`, base `3d12ad0`. Title: **Beach Bounce
Remastered** (web build only, `<source folder>\nxrs-web\Beach Bounce Remastered`). GPU 0 for native runs. Dev server
port 8771. Plan rows: the four W3 rows of `docs/35-WEB-TARGET-PLAN.md`; design from
`build/WEB/W0-gta/runtime-io.md` (designs only, no code copied).

## Goal

A web build's files, saves, audio and input are real and on demand:

1. game data (RomFS, ExeFS) streamed by HTTP range through an I/O worker with an OPFS block cache and a recorded boot
   set, replacing W2's whole-folder preload;
2. saves kept in the browser (OPFS) with acknowledged commits, surviving a reload; a failed restore never starts;
3. the engine's audio output to an AudioWorklet through a shared ring, underruns counted and logged;
4. keyboard and gamepads to the game's pad state through a shared input block.

## Plan

### Shared-memory protocol (engine ↔ I/O worker)

A table of 32-bit words in linear memory, allocated by the engine at the first read and published once to the page
(`nxweb_io_start`, a JS-library function proxied to the page thread, which posts `{memory, table}` to the I/O
worker). Header 16 words (`[0]` doorbell, `[1]` slot count, `[2]` protocol version, `[3]` slot-freed counter), then
slots of 16 words: `[0]` state (0 idle, 4 claimed by an engine thread, 1 requested, 3 taken by the worker, 2 done),
`[1]` op (1 read, 2 save commit), `[2]` file id / name length, `[3..5)` offset / name address, `[5..7)` destination
/ data address, `[7]` length, `[8]` result. An engine thread claims any idle slot (CAS), fills it, sets state 1,
bumps the doorbell and futex-waits on the state word; the worker `Atomics.waitAsync`s on the doorbell, serves the
slot asynchronously (OPFS / range fetch straight into linear memory) and `Atomics.notify`s the state word. Engine
threads never touch the page's thread for game data; nothing goes through Emscripten's file system.

### Data (manifest, block store, boot set)

- `src/cli/src/webpack.rs` (called from `--target web`): writes `data.json` (`{version, title, files: [[path,
  size], ...]}`, every file of `<name>_data` but `save/`) and `bootset.json` (`[[path, [[first, last], ...]], ...]`,
  GTA's shape) beside `game.js`, the boot set from a recording in the data folder (`<name>_data/bootset.rec.json`),
  checked against the manifest (unknown files and ranges past a file's end dropped, ranges merged within 4 KiB).
- Recorder: `NXRS_BOOTSET=<file>` makes the engine (native or web) record the byte ranges it reads from the data
  folder (RomFS files as `romfs/<path>`, ExeFS, control data) in first-touch order and write them 60 s after the
  first read (`NXRS_BOOTSET_SECONDS`).
- `src/web/io-worker.js`: 4 KiB blocks; OPFS `store.bin` (append-only) + `journal.bin` (data flushed before the
  journal entries that point at it; another data version wipes it); range requests checked (206, exact
  Content-Range, length), retried; demand reads at high fetch priority, the boot set prefetched at low priority while
  the module compiles; a memory cache when OPFS is unavailable. Stats (bytes fetched for the boot set / demand,
  served from the cache) for the log.
- Engine: `translator::package::RomFs::from_stored` (a file list read through a callback: `FileLoc::Stored`), and
  `os::webdata` (`read(path)`: on the web through the I/O worker when the manifest has the file, else the host file
  system; natively the host file system plus the recorder). RomFS, ExeFS modules, `main.npdm`, `control.nacp`, the
  NVN SDK probe read through it.
- The page keeps only the small host files (data-folder files outside `romfs/` and `exefs/`, ≤ 4 MiB: control data,
  icon, profile/patch tables) in the in-memory file system, which the engine's other `std::fs` readers use.

### Saves

`fs::commit` (transactional saves) on the web, after the committed folder is in place: the folder is packed
(`NXSV` blob: directories and files) and sent through a slot (op 2); the I/O worker writes
`<OPFS>/nxrs-saves/<title>/<save>.new`, flushes, and moves it over `<save>.bin`; the slot's done state is the
acknowledgement (10 s bound, red line and a failed commit otherwise). Restore (page, before `main`): every
`<save>.bin` (or a complete `.new` left by an interrupted move) unpacked into `/data/save/<save>`; an error holds
`main` with the reason on the page (never an empty start).

### Audio

`media/sink.rs` on the web: the output device is a float stereo ring (8192 frames, power of two, free-running
counters; header: write, read, capacity, heartbeat, underrun frames, underrun events, context state, rate) published
by `nxweb_audio_publish`; the engine's device thread keeps the ring filled to a target from the same mixer the WASAPI
callback pulls. `src/web/audio-worklet.js` reads it in a 48 kHz AudioContext started on the first click / key;
underruns filled with silence and counted; the engine logs underruns and fill every 5 s while they change. Until the
context runs the mix is consumed in real time and the log says the page is waiting for a click (never presented as
sound).

### Input

`src/web/input.js` writes a published block (`nxweb_input_publish`): sequence, focus, 256 key bits by Windows VK code
(`KeyboardEvent.code` mapped), up to 4 gamepads (standard mapping buttons, 4 axes); keys released on blur. The engine's
input thread reads it every tick (`os/src/input/web.rs`): keys through the native front's default map
(`front/src/keys.rs`), gamepad 0 by position (bottom = B, right = A, left = Y, top = X), into player 1's keyboard
path; focus from the page. `NXRS_CONTROLLER_TRACE=1` (page `?env=`) logs what the game's hid reads.

### Page

`src/web/boot.js` restructured into one function per subsystem (checks, log, data, saves, audio, input, engine; a
place for W4's GPU start), `?env=K=V,...` for engine variables, `?record=1` not needed (the recorder is the engine's).
`src/web/dev_server.py` serves the new files, `data.json`, `bootset.json`, the worker and worklet scripts.

## Files expected

`src/translator/src/package/romfs.rs`; `src/os/src/lib.rs`, `src/os/src/webdata.rs` (new), `src/os/src/web/{mod,io,
saves,audio,input}.rs` (new, emscripten), `src/os/src/boot.rs`, `src/os/src/services/fs.rs`, `src/os/src/nvn/mod.rs`
(one read), `src/os/src/media/sink.rs`, `src/os/src/input/mod.rs`; `src/cli/src/webpack.rs` (new),
`src/cli/src/pipeline.rs`, `src/cli/src/cc.rs` (JS library), `src/cli/src/main.rs` (module); `src/web/boot.js`,
`index.html`, `io-worker.js`, `saves.js`, `audio-worklet.js`, `input.js`, `nxrs-lib.js` (new), `dev_server.py`;
`docs/35-WEB-TARGET-PLAN.md` (the W3 rows). Also `.gitattributes` and `scripts/build/web.ps1` (below).

## Notes

- 2026-11-26, at the first `web.ps1` run in this worktree: `core.autocrlf` checked `libc-wasm64.patch` out with CRLF,
  so its hash differed from the stamp; the script removed the shared `build\tools\libc-wasm64` (through the junction)
  and the patch then failed to apply. Restored at once with the main tree's patch and stamp (`nlink_t = u64` checked).
  Fixed here: `.gitattributes` (`*.patch eol=lf`) and `web.ps1` naming the copy by the junction's real folder (every
  worktree writes the same line into the shared nightly toolchain).

## Report

(at the end, protocol §5)
