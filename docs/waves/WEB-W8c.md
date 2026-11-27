# WEB-W8c - Chrome FPS, continued (the GPU worker's GC, then the main thread's code)

**Brief (2026-11-27):** Anime Girls Sun of a Beach (0100C6801E0FA000) beach gameplay in Chrome from 43.2 FPS (WEB-W8b
final; 47-58 on quieter runs) to a steady 60. docs/35 phase W8. Worktree `D:\Projects\RecompLauncherEngine-web-w8c`,
branch `web-w8c`; deadline 3 h. Library web folder `P:\Games\Nintendo\DUMPS\Anime Girls Sun of a Beach
[0100C6801E0FA000][v0]\nxrs-web\Anime Girls Sun of a Beach`, built with the launcher's arguments (`--outdir` that
folder, `-o "Anime Girls Sun of a Beach"`, `--shaders-from` the native data folder, `--target web`).

## Plan

1. **(a) GPU worker GC first.** W8b: ~165 full mark-compacts a second on the GPU worker because V8 counts the ~9.4 GB
   shared linear memory as that isolate's external memory. Measure before choosing:
   - a stand-alone page (`build/WEB/W8c/gcbench`): a worker allocating at the GPU worker's rate, handed a shared
     memory64 `WebAssembly.Memory` of several sizes, handed it small and grown later, or a plain small
     `SharedArrayBuffer` - which of these makes the worker collect continuously;
   - then the cheapest fix that removes the cause for the real worker: hand the GPU worker the memory before it is
     large, or a GPU worker that never holds the wasm memory (its own small shared buffer; uploads copied by the
     producer, their cost measured), or a smaller linear memory (WEB-WMEM: needs a full retranslation; cost stated
     before doing it). No unsupported Chrome flags in the shipped page.
   - GC trace of the GPU worker (browser-wide trace, per thread) before and after.
2. **(b) main thread guest code** if time remains: wasm-opt -O2/-O3 over the whole module measured for FPS, tier-up,
   the translation lanes' pools-lock wait.
3. Native unchanged: web-only files and `cfg(target_os = "emscripten")` arms only.

## Expected files

- `src/web/gpu.js`, `src/web/boot.js`, `src/web/gpu-worker.js` (when and what the GPU worker is handed).
- Possibly `src/graphics/src/web/ring.rs`, `src/web/nxgfx.lib.js` (a separate shared buffer), `src/cli/src/cc.rs`
  (wasm-opt level for web links).

## Safety

Chrome WebGPU on GPU 1 only (`--use-adapter-luid=0,113326`); one game Chrome on the machine (the driver refuses to
start while any other Chrome with `--use-adapter-luid` or `--remote-debugging-port` runs); a dev-server port nobody
else serves; screenshots >= 20 s apart; every build below normal priority with `EM_CACHE=target\emcache`; processes
stopped only by PID, only our own; no native game runs.
