# WEB-W8b - Chrome FPS, continued (the game's main thread as wasm)

**Brief (2026-11-27):** Anime Girls Sun of a Beach (0100C6801E0FA000) beach gameplay in Chrome from 40.7 FPS (WEB-W8
final) to 60 (native 59.7). The limit is the game's main thread as wasm: 29.7 ms CPU a frame vs native 5.7 ms.
docs/35 phase W8. Worktree `D:\Projects\RecompLauncherEngine-web-w8b`, branch `web-w8b`; deadline 3 h.

## Plan

Measure each lever WEB-W8 named before changing it, with a CPU profile of the game's main thread (Chrome's Profiler
domain over CDP on the worker targets, function names from a `--profiling-funcs` link):

1. V8's memory64 bounds checks on every guest access: their share, by running the same build with V8's own switches
   (`--js-flags=--no-wasm-bounds-checks` as the upper bound of what removing them could give,
   `--wasm-enforce-bounds-checks` to see whether the trap handler is in use now); then what Chrome offers.
2. The setjmp frame (`rt_sj_owner`, 4.7% of the main thread in W8): where it is entered per guest call, and whether
   the hot path can avoid `setjmp`.
3. Software `fmaf` on the game's worker threads (relaxed SIMD madd, within W0-fp's rules).
4. wasm-opt on the one-module build: FPS, not size.
5. GPU worker GC 33-40% (bind group cache bound); translation lanes waiting on the pools lock.

Keep only changes that move FPS / frame time; web-only branches (native unchanged).

## Expected files

- `src/runtime/web/boundary.c` (setjmp owner), `src/cli/src/cc.rs` (web compile / link flags),
  `src/translator/runtime/*.h` (`__wasm__` branches only), `src/web/gpu-worker.js`.

## Safety

Chrome WebGPU on GPU 1 only (`--use-adapter-luid=0,113326`: PCI bus 4, the LUID read from the adapter's
`{60b193cb-5276-4d0f-96fc-f173abad3ec6} 2` property, bus 2 = 108479 is the display GPU); one game Chrome on the
machine (checked by command line before each run); screenshots >= 20 s apart; builds below normal priority.

## Results

(filled in as the wave lands)
