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

## Files expected

`docs/waves/WEB-W6b.md`, `docs/35-WEB-TARGET-PLAN.md` (W6, shader-pack rows); `src/web/boot.js`, `src/web/split.js`;
`src/cli/src/pipeline.rs` (packs step, split default), `src/cli/src/webshaders.rs` (new: pack writer), `webhost.rs`
or `webpack.rs` if the manifest must exclude the recording; `src/shaders/src/pack.rs` (new), `src/shaders/src/lib.rs`;
`src/graphics/src/vk/shader.rs` (recorder, pack lookup); `src/web/dev_server.py` if it lacks `If-None-Match`.
