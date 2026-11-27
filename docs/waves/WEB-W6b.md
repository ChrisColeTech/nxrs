# WEB-W6b - The split running in Chrome, shader packs, warm launches without the module

Owner: one delegated agent. Lead: main session. Opened 2026-11-26. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.
Worktree `D:\Projects\RecompLauncherEngine-web-w6b`, branch `web-w6b`, base `26ad1c9`. Title: **Anime Girls Sun of a
Beach** (0100C6801E0FA000), web build `<source folder>\nxrs-web\Anime Girls Sun of a Beach`. GPU 1, port 8774.
Follows `docs/waves/WEB-W6.md`. Not touched: `src/graphics/src/vk/live.rs` and the launcher server's shader request
handling (WEB-W7b's); `web_reply.rs` / `web_file.rs` only if a change there cannot be avoided (named in the report).
Evidence: `D:\Projects\RecompLauncherEngine\build\WEB\W6b\`.

## Deliverable

1. The split build (`NXRS_WEB_SPLIT=1`) reaches beach gameplay at ~60 FPS in Chrome; if it does, it becomes the
   default.
2. Lazy shader packs: a cold launch makes a few pack requests instead of ~346 shader-file requests.
3. A warm launch does not download the 112 MB module again.

Baseline (W6, launcher server, Chrome 158): cold 550.9 MB / 1,530 requests / title <= 26 s / 56-60 FPS; warm
112.1 MB / 377 requests.

## Plan

### 1. The split stall (1.5 h to a fix or a named wait)

- Build the split (`NXRS_WEB_SPLIT=1`), run it in Chrome over CDP, and at the stall pause every worker
  (`Debugger.pause` per target) and name the wasm frames with `game.symbols` (as W4b did with `game.js.symbols`).
- Hypotheses to separate: a cold call on a thread whose synchronous loader cannot run there; a deadlock between the
  loader and a lock its caller holds; an out-of-memory kill (the reload at ~87 s) from fetching and compiling the
  273 MiB cold module once per thread.
- `pipeline.rs`'s split path returns `Ok(())` inside the compile/link closure only, so the later web steps (host
  files, `build.json`, gzip copies, data manifest) still run after it: to be confirmed from the build log.
- Fix in source. Planned shape, if the stacks agree: the page compiles `game.cold.wasm` once (asynchronously, from
  the code cache of item 3) and hands the compiled module to every engine worker before the worker runs a thread;
  a worker that has it instantiates it at its first cold call (no fetch, no compile); the synchronous fetch stays
  only as the fallback.

### 2. Shader packs

- **Recorder:** `NXGFX_SHADER_REC=<file>` (graphics `vk/shader.rs`): each shader module the replay loads, once, in
  first-use order (hex key a line). Recorded natively through beach gameplay and copied into the web data folder as
  `shaders.rec.txt` (beside `bootset.rec.json` and `hotset.txt`, the same kind of recording).
- **Format** (`src/shaders/src/pack.rs`, used by both the writer and the engine): `shaders/pack-<n>.bin` beside
  `game.js` (magic, count, then per module: 8-byte key, spv length, json length, bytes) and `shaders/packs.txt`
  (`<hex> <n>` lines). Packs are cut in first-use order at about 1 MiB, so the boot's first draws need only the
  first pack. The members come from the data folder's `shaders` library (W7's `--shaders-from` imports land there
  too). Modules outside the recording stay single files, fetched as before.
- **Serving:** the packs are ordinary build files with gzip copies (`.gz`), so both servers already send them with
  `Content-Encoding` and ETags (no server change).
- **Engine:** on the web, a module not in memory is looked up in `packs.txt` (fetched once); its pack is fetched
  whole and every member goes into the in-memory library; otherwise the single-file fetch as today.
- Justified by numbers: modules and bytes per pack, requests and bytes on a cold launch to the beach, against W6's
  346 shader-file requests.

### 3. Warm launch

- Confirm the cause: Chrome's HTTP disk cache does not store an entry larger than an eighth of its size (simple
  cache backend), so the 107 MB `game.wasm.gz` is never stored and no revalidation can happen; the server's ETag /
  `no-cache` answers are right but unused.
- Fix: the page keeps the modules in Cache Storage itself (`Module.instantiateWasm` in `boot.js`): it asks the server
  with `If-None-Match: <stored ETag>`; a `304` instantiates the stored response (streaming compile), a `200`
  replaces it. A changed build has a new ETag (size + modification time), so it is never served stale. Same for
  `game.cold.wasm` in a split build.

## Results (2026-11-26)

### 1. The split: named, not fixed (stays opt-in)

- `pipeline.rs`'s early `return Ok(())` is inside the compile/link closure only: the split build's log shows the data
  manifest, gzip copies, host files and `build.json` written after the split (`build/WEB/W6b/build-split1.log`).
  Not a bug.
- **Where it stops** (`build/WEB/W6b/split1-cold`, launcher server, fresh profile): every worker paused over CDP and
  named with `game.symbols` (`stacks.txt`). No thread is in the cold loader and none is deadlocked: the game threads
  wait in `nxrt::kernel::block` on guest SVCs as usual, and the new engine workers are in
  `createWasm -> receiveInstance`, still instantiating `game.wasm`. Then Chrome reports `Inspector.detached: Render
  process gone` (21-34 s across three runs; W6's 87 s "reload" was the same).
- **Why:** the renderer's private memory, sampled every second (`split1-mem.tsv`), climbs from 13.0 GB to
  **16.6 GB** while the game's ~40 workers instantiate, then the process is gone. The split module imports
  **290,138 placeholder functions** (`placeholder.cold.deferred`, one per cold guest function, `exports.mjs`), and
  every pthread instantiates its own copy against Emscripten's import proxy: measured under V8 (`inst.mjs`, same
  module), **~140 MiB per instance** with Emscripten's one-closure-per-placeholder proxy and still ~120 MiB with one
  shared function for all of them (V8's per-import data). ~40 threads x ~140 MiB is the ~3.5 GB that pushes the tab
  over. The unsplit build peaks at 12.7 GB in the same place (`w6b-cold-mem.tsv`) and runs.
- **Consequence:** with pthreads (one instance and one table per thread) a lazily loaded cold module costs every
  thread the full placeholder table. The working alternative, `wasm-split --no-placeholders` with the page compiling
  `game.cold.wasm` and every worker instantiating it before its thread runs, needs the cold module before any thread
  starts: the same bytes and startup as the one module (48.9 + 53.5 MiB gzip against 106.6 MiB), so it buys nothing.
  The split stays opt-in (`NXRS_WEB_SPLIT=1`); one module remains the default.

### 2. Shader packs

- Recording: native run `build/play/20261126-225548-agsob` (GPU 1, `NXGFX_PREWARM_MAX=0` so the order is the game's
  own first use, `NXGFX_SHADER_REC`): 175 modules, normal exit, 59.0 FPS mean, 0 red lines, sheet read (title, swim,
  beach). Stored as `<name>_data\shaders.rec.txt`.
- Build: 175 modules in 5 packs of ~1 MiB, 4.9 MiB raw, **1.3 MiB gzip**. Packing by first-use order: the title
  needs pack 0 (fetched at 10.9 s), the beach packs 1-2 (19.6 s), later play 3-4 (47-49 s). Packing the whole
  53,109-module library instead would copy 3.4 GB beside the page for modules no run touched.
- Chrome, cold (`final-cold`): **11 shader requests** (index + 5 packs + 2 modules outside the recording as single
  files, one of them a 404 json) against W6's **346**; ~1.3 MB. Warm: the packs come from the HTTP cache.
- Order in `Library::load` on the web: in memory -> W7b's `live::web_known` -> pack -> single files -> W7b's run-time
  translation (`live::web_fetch_live`): a pack miss still reaches the server's translation route.

### 3. Warm launch

- Cause: Chrome's HTTP disk cache keeps no entry over an eighth of its size, so the 107 MB `game.wasm.gz` was never
  stored and no `If-None-Match` was ever sent (W6's warm run: no revalidation, 112 MB again).
- Fix (`boot.js`, `Module.instantiateWasm`): the page keeps the module in Cache Storage (`nxrs-code-1`) and asks the
  server with the kept ETag; `304` uses the kept copy, `200` (a rebuilt module: new size/time ETag) replaces it.
- Chrome, warm (`final-warm`, same profile as `final-cold`): `game.wasm` **304, 283 bytes**, compiled and
  instantiated in 1.0 s; whole launch **0.11 MB / 160 requests** (87 of them the page's log POSTs, 49 the workers'
  `game.js` from memory) against W6's 112.1 MB / 377; nnMain 3.8 s.

### Numbers (Chrome 158, launcher server `nxrs --serve-web`, port 8774)

| Run | Transferred | Requests | nnMain | Title menu | Beach FPS |
|---|---|---|---|---|---|
| W6 cold (`w6d-cold`) | 550.9 MB | 1,530 | 7.6 s | <= 26 s | 56.2-60.0 |
| W6b cold (`final-cold`) | **546.5 MB** (module 111.8, data extents 406.7, small data 26.9, shader packs ~1.3) | **1,186** | 8.0 s | <= 28 s | 47-60 (57.1 pacing at 90 s); another wave's native game was running |
| W6 warm (`w6d-warm`) | 112.1 MB | 377 | 3.8 s | - | - |
| W6b warm (`final-warm`) | **0.11 MB** | **160** | 3.8 s | first present 12.4 s | - |
| Split cold (`split1-cold`) | - | - | 10.4 s | never: renderer gone at 21-34 s | - |

The beach screenshot (`final-cold/t100.jpg`) shows the HUD, both monsters, the pier and clean water.

## Files expected

`docs/waves/WEB-W6b.md`, `docs/35-WEB-TARGET-PLAN.md` (W6, shader-pack rows); `src/web/boot.js`, `src/web/split.js`;
`src/cli/src/pipeline.rs` (packs step, split default), `src/cli/src/webshaders.rs` (new: pack writer), `webhost.rs`
or `webpack.rs` if the manifest must exclude the recording; `src/shaders/src/pack.rs` (new), `src/shaders/src/lib.rs`;
`src/graphics/src/vk/shader.rs` (recorder, pack lookup); `src/web/dev_server.py` if it lacks `If-None-Match`.
