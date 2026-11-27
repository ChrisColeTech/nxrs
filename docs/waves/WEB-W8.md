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

## Results

(below, as the work lands)
