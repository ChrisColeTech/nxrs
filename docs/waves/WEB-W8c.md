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

## Results (2026-11-27)

Evidence: `build/WEB/W8c/` (`tools/drive.mjs`: `--luid` required, refuses to start while another Chrome with
`--use-adapter-luid` / `--remote-debugging-port` runs, PNG screenshots every >= 20 s, X every 6 s from 40 s,
`--trace=a,b` a browser-wide V8 GC trace counted per thread with threads named by the scripts they ran, nvidia-smi
per GPU every 10 s; `tools/fps.mjs` FPS over 95-135 s from the pacing lines; `gcbench/` the stand-alone page).
Chrome 158 maximized on the 4K screen, WebGPU on GPU 1 (`--use-adapter-luid=0,113326`: re-read from the adapters'
`{60b193cb-...} 2` property, PCI bus 4 = nvidia-smi index 1; its memory rose to 2.4 GB during every run, GPU 0's
did not change with ours). Dev server `src/web/dev_server.py` on port 8793 (the launcher serves 8780).

### (a) The GPU worker's GC

**Stand-alone measurement (`gcbench/`, headless Chrome 158, no GPU):** one worker allocating ~15 MB/s of objects
that live ~1-2 s (so they reach the old generation), touching `memory.buffer` every ms as `gpu-worker.js` does:

| what the worker was handed | full GCs on the worker, 6-8 s |
|---|---|
| nothing / a 1 GiB `SharedArrayBuffer` | 1 |
| a shared memory64 `WebAssembly.Memory` of 1, 2, 3, 4, 6, 7 GiB | 2 |
| the same of **8 GiB** | **766 in 6 s** |
| the same of 9.4 GiB | **1,099 in 8 s** (reason "finalize incremental marking via task") |
| 1 MiB handed, then grown to 9.4 GiB (2, 40 or 2,000 grows) while the worker reads `memory.buffer` | 1 |

So the external-memory figure is the memory's length **at the moment the isolate receives it**; later growth is not
counted. Somewhere between 7 and 8 GiB it passes the worker's global GC limit and every collection is followed at
once by the next. Short-lived objects alone (scavenges only) do not show it.

**Cause in the game:** the engine handed the memory to the GPU worker in `nxgfx_web_start`, when it creates its
Vulkan device - after the guest window was claimed (~9.4 GB). **Correction** (`src/web/boot.js`, `gpu.js`,
`gpu-worker.js`; JS only, native untouched): the page creates the GPU worker in `instantiateWasm` and posts it the
memory there (`imports.env.memory`, 256 MiB: `-sINITIAL_MEMORY`); `nxgfx_web_start` later sends only the ring, the
canvas and the options (and the memory only on a page without that hook). The canvas still goes to the worker at
start, and resizes before then still size the canvas on the page.

| GPU worker, GC trace 100-110 s of beach gameplay | `base` (library build, W8b final) | `fix1` (same module, the page's new scripts) |
|---|---|---|
| full GCs (MajorGC) | **221.7 a second, 187 ms a second** | **0.1 a second, 1.7 ms a second** |
| minor GCs | 0 | 0.2 a second |
| reasons | "finalize incremental marking via task" 2,204, "external memory pressure" 13 | allocation failure 2, one marking |
| every other thread | <= 8.6 minor GCs a second, no full ones | the same |

FPS did not move with it: `base` 52.3, `fix1` 51.0 over 95-135 s (both with this wave's builds running at below
normal; the run-to-run spread is ~+-15%). The GPU worker's frames line stayed at median 18.4-18.6 ms: the worker was
not the critical path at these frame rates; the game's main thread is (W8b). The GC fix removes the worker as the
next limit (W8b measured it busy 17-25 ms a frame, half of it GC).

The other two options were not needed: a separate shared buffer the worker owns (the producer copying packets and
uploads into it) would be the more general fix if V8 ever starts counting growth; a smaller linear memory could not
get below ~7.5 GiB anyway (the guest window alone is ~7.3 GiB; WEB-WMEM's unused 5.5 GB is commit, not length).

### (b) The main thread, wasm-opt, the lanes

**CPU profile** (`prof`: the library build relinked with `NXRS_WEB_LINK_FLAGS=--profiling-funcs`, every target
100-110 s, 40.6 FPS window, 407 frames; `tools/cpu2.mjs`, `tools/waits.mjs`):

| thread | busy a frame | of which | waits a frame |
|---|---|---|---|
| game main thread | 19.2 ms | **guest code 17.6 ms**, `rt_dispatch` 0.6, futex wake 0.3 | kernel waits (SleepThread / events) 3.8, GPU fence 0.7, core acquire 0.2 |
| replay thread | 14.6 ms | `buf_read` 2.1, `Pool::web_sync` 1.5, malloc/free 1.9, page hash 0.4 | translation lanes 7.6 |
| GPU worker | 7.9 ms | `writeBuffer` 4.0, GC 0.05 | - |
| job threads 8 / 9 | 4.3 / 4.2 ms | guest 3.1 / 3.0 | |

The main thread is the limit, as in W8b: its guest code alone is above 16.7 ms a frame in this scene.

**wasm-opt -O2 over the whole module** (emsdk's binaryen, 304 s at below normal, 394.7 -> 387.3 MB; swapped into the
library folder for the runs with `game.wasm` / `game.wasm.gz` backed up and restored, sizes checked), runs alternated
with the module as built:

| module | FPS 95-135 s (runs in order) | mean |
|---|---|---|
| as built (fold only; `base`, `fix1` are W8b's module) | 52.3, 51.0, 48.8 (`after`), 53.8 (`after2`), crash (`after3`), 46.5 (`after4`) | 50.5 |
| `-O2` | 58.4 (`o2`), 52.1 (`o2b`), 53.1 (`o2c`), 48.7 (`o2d`) | 53.1 |

Adjacent pairs disagree (`o2b` 52.1 vs `after2` 53.8; `o2d` 48.7 vs `after4` 46.5) and the machine's load changed
under the runs (another session's GPU 0 work 7-39%, a `pkg-to-anyps5` job, ~31% CPU): +5% on the means is inside
the spread, so -O2 (5 more minutes a build) is **not kept**. A quiet-machine A/B is the way to settle it.

**Translation lanes' pools lock:** the lanes wait 2.9-4.9 ms a frame together at resolves (`[vk] translate
(parallel)` lines) but the replay thread waited 0.00 ms a frame for them: not on the critical path, not changed.

**Tier-up:** not a page lever (W8b: `--wasm-tiering-budget` only by flag); not measured again.

### A one-off abort (`after3`) and its diagnostic

One run of the library build (fold-only module) stopped at 59 s: the page said "The game stopped:" with no reason.
The trap's stack, named from the same build relinked with function names (`wasm-opt --print-function-map`), is
`completion_loop` -> `submission_safety::resource_safety_fatal` -> `abort`: a submission did not complete within the
5 s GPU completion budget (or its wait failed). On the web that fatal is a silent abort (native's exit status 72
reaches no one), so the cause could not be told apart. **Change** (`src/graphics/src/vk/queue.rs`, a
`cfg(target_os = "emscripten")` block only): before the fatal, a red line names the submission, the seconds since
the worker handed it to the GPU, or the wait's error. Not reproduced in the 8 other runs with this wave's scripts.
Likely related (not shown): the run started at the level load (A4B4G4R4 line at 54 s), when Dawn compiles new
pipelines in the GPU process after the queued word, on a loaded machine.

### Final run (`final`, the library build relinked from this worktree without names)

Chrome on GPU 1, another session's GPU 0 work at 7-10%, no builds. Boot, menu, beach gameplay (`t140.png`: Kayla on
the pier, two spiders attacking, HUD, sharp, native colours), 0 WebGPU errors, no lost device, swap chain 3376x1899
B8G8R8A8_SRGB. **51.7 FPS** over 95-135 s (51.4 over 135-160 s; GPU worker's last lines 50.0 / 56.1 / 59.1 FPS);
GPU worker GC 0.1 full collections a second, 1.4 ms a second (baseline 221.7, 187 ms).

| per frame | W8b final | this wave |
|---|---|---|
| FPS 95-135 s | 43.2 (47-58 quieter) | 51.7 (`final`); 46.5-53.8 as built; `base` 52.3 on this machine today |
| GPU worker full GCs | ~165 a second | **0.1 a second** |
| main thread guest code | 18-27 ms | 17.6 ms (one window) |

### Unproven / next

- **60 FPS not reached.** The game's main thread runs 17.6 ms of guest code a frame as V8 code (native 5.7 ms of all
  work); its own sleeps and waits add ~4.7 ms. No page lever is left that measured above the spread. Next: a
  quiet-machine A/B of wasm-opt -O2 (or -O3) with several runs a side; the GPU worker's `writeBuffer` (4 ms a frame)
  once the main thread is faster.
- The early hand-over relies on V8 accounting a shared memory's length when an isolate receives it, not its growth
  (measured on Chrome 158). If a later Chrome counts growth, the storm comes back; then the GPU worker needs its own
  small shared buffer (the producer copying packets and uploads into it).
- The `after3` abort: one in 9 runs; the new red line names it next time.

## Safety

Chrome WebGPU on GPU 1 only (`--use-adapter-luid=0,113326`); one game Chrome on the machine (the driver refuses to
start while any other Chrome with `--use-adapter-luid` or `--remote-debugging-port` runs); a dev-server port nobody
else serves; screenshots >= 20 s apart; every build below normal priority with `EM_CACHE=target\emcache`; processes
stopped only by PID, only our own; no native game runs.
