# WEB-W8 - picture and FPS in Chrome

**Owner (2026-11-27):** the picture in Chrome looks blurry / low resolution, and FPS is low (Anime Girls Sun of a
Beach, 0100C6801E0FA000, beach gameplay 23-47 FPS after WEB-WPAGE; goal 60; native 59.6). Make it sharp and fast.
docs/35 phase W8.

## Plan

1. **Picture.** The canvas's backing store is a fixed 1280x720 that CSS stretches to the window (on the owner's 4K
   screen at devicePixelRatio 3, ~2.6x). Make it the displayed size in device pixels (`ResizeObserver`,
   `device-pixel-content-box`), resized with the window; the engine's swap chain follows it through the out-of-date /
   suboptimal path it already has (`acquire_next_image` compares the swap chain with the canvas); the game's frame is
   scaled once, by the engine's present (sharp upscale when the screen is larger, its filtered blit otherwise), and
   the canvas takes each presented image 1:1. Check the present's colour path against native (an sRGB picture into
   the swap chain). Compare a crop of a gameplay scene with the native build's screenshot.
2. **FPS.** A baseline run with the diagnostics on (the engine's `[vk]` replay / translate / pacing lines, the GPU
   worker's `[gpu] frames` line, a CPU profile of every thread over a fixed gameplay window, with function names from
   a `--profiling-funcs` link); find where a frame's time goes (game threads, replay, translation lanes, ring, GPU
   worker JS, waits, present pacing); cut the largest costs; repeat. No GPU timestamps on the web device today
   (`GPU 0.0 ms`).
3. Verify: graphics / shaders / os tests; the after run (gameplay screenshot read, sharpness crop, FPS over a fixed
   window, what else ran); relink the library web build from this worktree.

## Expected files

- `src/web/gpu.js`, `src/web/gpu-worker.js`: backing store and resize; 1:1 present.
- `src/graphics/src/web/icd.rs`: surface formats / swap chain format.
- whatever the frame measurement names (`src/graphics/src/web/*`, `src/graphics/src/vk/*`).

## Safety

Chrome's WebGPU on GPU 1 only (`--use-adapter-luid=0,113326`, PCI bus 4, checked from `DEVPKEY_Device_AdapterLuid`);
one game Chrome on the machine (checked by command line before each run); screenshots >= 20 s apart; builds below
normal priority.

## Results (2026-11-27)

Evidence: `build/WEB/W8/` (runs `base`, `run2`, `run3`, `final`; driver `tools/drive.mjs`: `--luid` required, PNG
page screenshots every >= 20 s, `--profile` CPU profile of every thread, `--heap` the GPU worker's allocation
sampling, `--resize`; crops `tools/crop.ps1`). Chrome 158 maximized on the owner's 4K screen (devicePixelRatio 3,
1280x720 CSS: the canvas shows at 1125x633 CSS = 3376x1899 device pixels), WebGPU on GPU 1 (`--use-adapter-luid=0,113326`,
the "nvidia blackwell" adapter). Other sessions' builds (nxrs-build + wasm-opt, ~13 cores) ran during every run: numbers
are indicative. FPS = frames over 95-135 s (beach gameplay, X pressed every 6 s from 40 s) from the engine's pacing
lines.

### Picture

- **Blur:** the canvas's backing store was a fixed 1280x720 that CSS stretched 2.6x to 3376x1899 device pixels, after
  the engine had already scaled the game's 1920x1080 picture down to 1280x720. Now `gpu.js` sizes the backing store
  from a `ResizeObserver` on `device-pixel-content-box` (fallback CSS size x devicePixelRatio) and sends resizes to the
  GPU worker, which writes them into the ring header; the engine's next acquire finds the swap chain out of date
  (`acquire_next_image` already compared it with the canvas) and makes one of the new size; the worker sizes the
  canvas to the image it presents, so the copy to the canvas is 1:1. The engine's present then upscales the picture
  once into the swap chain: `[vk] swapchain 3376x1899, 3 images, B8G8R8A8_SRGB`.
- **Sharp upscale refused on the web:** the present's contrast-adaptive upscale (`upscale.comp`) failed WGSL
  conversion (`naga spv-in: invalid id %28`), so the web fell back to the bilinear blit. Cause: glslang's SPIR-V 1.4+
  `OpEntryPoint` lists every global; the web rewrite (`shaders::wgsl::rewrite_spirv`) removes a combined image-sampler
  variable and adds its texture and sampler halves but left the removed id in the interface list (the translator's
  own shaders list inputs and outputs only, so it never showed). The entry point now names the two halves. Test
  `vk::upscale::tests::upscale_shader_converts_for_the_web` (failed before: `invalid id %28`; passes).
- **Too dark:** the presented texture is `R8G8B8A8_SRGB`; native presents it into a `B8G8R8A8_SRGB` swap chain
  (decoded when sampled, encoded when written); the web surface offered only `B8G8R8A8_UNORM`, so the picture arrived
  decoded and was shown as is (the HUD's orange kills badge was dark red). The web surface now offers the sRGB format
  too and makes the swap chain in the format asked; the present's copy to the bgra8unorm canvas differs only in
  sRGB-ness (WebGPU copies the bytes).
- Crops (`crop-kills.png`, `crop-ammo.png`: the same HUD region from `base/t120.png`, `run3/t120.png` and the native
  reference frame `build/play/20261126-193134-agsob/03-play_later.png`, bicubic to the canvas's size): before soft and
  dark red; after sharp edges and native's colours. The scenes differ (the browser's save is not the native run's), so
  the comparison is on the HUD.

### FPS: where a frame's time goes

Baseline (`base`, the library build before this wave): **33.5 FPS** (95-135 s), median frame 24-30 ms. Native
(`build/play/20261126-193134-agsob`, metrics CSV, 100-160 s): 59.7 FPS.

| per frame | native | web base | web run3 | source |
|---|---|---|---|---|
| main game thread CPU (guest code + svc) | 5.7 ms (t1001) | ~26-28 ms (80% busy, 68% guest) | ~20 ms (81% busy at 39 FPS) | native metrics CSV; CPU profile thread 15 (`boot_main`) |
| the game waited for the GPU | 0.00 | 0.0-0.3 ms | 0.2-2.7 ms | `[vk] GPU time` line (now printed without timestamps) |
| replay thread busy / replay CPU | 1.6 ms replay | 17-19 ms busy, 8-11 ms replay (sets 4-16, vertex input 2-9) | 10-13 ms busy, 6-8 ms replay (sets 2-4, vertex input 2-6) | `[vk] pipelining`, `GPU time` lines |
| translation lanes (6) together | 0 (not parallel) | 14-19 ms, 5-8 ms of it waiting for the pools lock at resolves | similar | `[vk] translate (parallel)` |
| GPU worker JS | - | 77% busy: GC 40%, writeBuffer 14% | 59-62% busy: GC 33%, writeBuffer 14% | CPU profile of `gpu-worker.js` |
| GPU time | ~1 ms | not measured (no timestamp queries on the web device) | - | |

The frame is bound by the **main game thread** (threads 8/9, the game's workers, wait 69% for it; the replay thread
is idle 8-15 ms a frame; the game waits on the GPU < 3 ms). Its guest code runs flat over ~3,000 functions (no spin);
engine work around it is small (setjmp owner 4.7%, svc 3.1%, dispatch 2.7%); 9.6% is the game's own SleepThread
(native sleeps too: 4 sleeps a frame).

Cuts, in order:

1. **Web game code at -O2** (`cc.rs` `WEB_GEN_OPT`; native keeps -O1, chosen for x86 instruction counts). With (2):
   33.5 -> 39.1 FPS; the main thread's work per frame ~26-28 -> ~20 ms. Test
   `cc::tests::web_game_code_is_optimized_at_web_gen_opt_and_native_keeps_o1`.
2. **Bind groups keyed by plain values** (`web/submit.rs` `LayEnt`/`GrpEnt`): every draw's groups were built as
   `serde_json` values and printed to strings just to look them up; now hashed, JSON only for a new layout or group.
   Replay "sets" 5-10 -> 2-4 ms a frame, replay thread busy 17-19 -> 10-13 ms.
3. **Guest address rebase** (`translator/runtime/cpu.h`): on wasm every guest load and store computed
   `a - rt_guest_off`, reading that global from linear memory at each access (reloaded after every store under
   `-fno-strict-aliasing`), though it is always 0 (`runtime/src/cpu.rs`). Now the plain cast, as native. Result: see
   `final` below.
4. GPU worker: one `TextDecoder`, one `MessageChannel` for its yields, no offsets array for a bind without dynamic
   offsets (allocation; its GC share did not move: see Unproven).
