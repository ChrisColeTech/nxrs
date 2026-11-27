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

## Results (2026-11-27)

Evidence: `build/WEB/W8b/` (W8's `tools/` went with its worktree; rebuilt as `tools/drive.mjs`: `--luid` required, refuses
to start while another game Chrome runs, PNG screenshots every >= 20 s, X every 6 s from 40 s, `--cpu=a,b` CPU
profile of every target, `--heap` / `--trace` / `--census` for the GPU worker; `fps.mjs`, `main.mjs`, `threads.mjs`,
`waits.mjs`, `gpuw.mjs` summarize). Chrome 158 maximized on the 4K screen, WebGPU on GPU 1 (`--use-adapter-luid=0,113326`).
FPS = frames over 95-135 s from the engine's pacing lines. Run-to-run spread on one build is about +-15% (the beach
scene differs: how many spiders, where), so a change is judged by the stage it targeted, per frame.

### What the levers measured

| lever | measurement | result |
|---|---|---|
| memory64 bounds checks | same build, `--js-flags`: default / `--no-wasm-bounds-checks` / `--wasm-enforce-bounds-checks` (runs `names2`, `v8-nobc`, `v8-enforce`) | main thread guest code 21.1 / 23.7 / 24.8 ms a frame: no checks at all is **not** faster; forced explicit checks are slower, so V8 already uses the trap handler for this memory64 module (`--wasm-memory64-trap-handling` is on by default). Nothing to win; a 4 GiB wasm32 view is not worth it |
| V8 tier-up | `--wasm-tiering-budget=20000` (`v8-tier`); `--no-liftoff` stalls the game (lazy TurboFan of huge functions: 3 presents) | guest 18.4 ms a frame (-13%, within the spread); a page cannot set it. Not a product lever |
| setjmp owner frame | CPU profile with names (`names2`) | `rt_sj_owner` 0.43% of the main thread (W8's 4.7% was the unnamed profile's `wasm-function` that is the atomic wait). Not a lever |
| main thread make-up | `names2` thread 15 | 72.7% guest code over 2,592 functions (260 make half; all-label `fb_` 5.3%), `rt_dispatch` 3.4%, engine/libc 3%, waits 19%: the game's SleepThread 2.2 ms, kernel block 1.4, GPU fence 1.2, core acquire 0.7 ms a frame. No hot spot: the gap to native is V8's code for the guest code as a whole |
| software fmaf on the job threads | threads 8/9 (`names2`) | **kept, change 1** |
| GPU worker GC | CPU profile, allocation sampling, GC trace, object census | **root cause found; change 2 kept, the cause itself open** |
| wasm-opt -O2 on the one module | not run | the main thread has no single hot spot wasm-opt could fix, and at +-15% run spread a few % is not measurable in this wave's runs |
| translation lanes' pools lock | not reached | |

### Change 1: FP under FPCR.FZ and fused multiply-add (`src/translator/runtime/simd_fp.h`, `src/cli/src/cc.rs`)

Diagnosis: the game runs with FPCR.FZ; wasm has no FTZ/DAZ, so every scalar and vector FP op took the software slow
path (inputs flushed, volatile temporaries, vectors lane by lane), and every fused multiply-add was a call of the
software `fmaf` (no FMA instruction in wasm). On the job threads 8/9 that was `fmaf` 0.8 ms and the Unity job
function `f_1d11c5da0` 3.2-3.5 ms a frame.

Correction (wasm only; native preprocessing unchanged: `clang -E` of `simd.h` before/after identical for
x86_64-pc-windows-msvc, x86_64-linux-gnu, aarch64-linux-gnu):
- FZ fast path in `op` / `vop`: when no operand is denormal the flush changes nothing, and a result that is neither
  NaN nor tiny (|r| above the smallest normal, or zero) is exactly what the slow path returns; only the other cases
  take it. RSQRTS, min/max and directed rounding keep the slow path.
- `FFMA`: the relaxed SIMD madd (`-mrelaxed-simd` in `WEB_FLAGS`), which V8 makes one fused instruction on hosts with
  FMA (x86-64 FMA3, every AArch64): bit-identical to fma there. Scalars use lane 0 of `(VT){x}` (not `VK(x)`, which
  adds x to zero and turns -0 into +0: the test caught it).
- Test `src/cli/tests/web_fp.rs` (opt-in, `--ignored`: emcc build + emsdk Node, 28 s) runs `tests/web_fpcheck.c`:
  9.6 M fast-path results against the slow paths (FZ, with and without DN; special values, denormals, results around
  the smallest normal) and 2 M FFMA results against libm's fma: 0 differ. Before the `(VT){x}` fix: 4 and 195 differ.
- `cc.rs` test: web flags carry `-mrelaxed-simd`, native ones do not.

| per frame (profile 110-120 s) | before (`names2`, 34.5 FPS window) | after (`fz1`, 25.8 FPS window) |
|---|---|---|
| job threads 8 / 9: `f_1d11c5da0` | 3.20 / 3.45 ms | 1.04 / 1.01 ms |
| job threads 8 / 9: `fmaf` | 0.80 / 0.79 ms | 0 |
| job threads 8 / 9: slow paths (`slow_s`, `vslow_s`) | 0.16 / 0.19 ms | 0.01 / 0.02 ms |
| job threads busy (their share of the time) | 23.2% / 26.7% | 15.5% / 16.1% |
| `game.wasm` | 346 MiB | 382 MiB (the fast path inlined at each FP op) |

The job threads are not the critical path (the main thread waits on them ~1.4 ms a frame at most), so FPS does not
show it reliably: unprofiled `fz2` 47.0 FPS (95-135 s).

### Change 2: GPU worker GC, the bind group cache bound (`src/graphics/src/web/submit.rs`)

Diagnosis: the GPU worker is busy 17-25 ms a frame, half of it GC: over 60 FPS's 16.7 ms on its own, so it is the
next limit once the game threads are faster. Its JS heap is tiny (2.5 MB used; 13.8 MiB/s allocated, sampled), but
a trace shows **~165 full mark-compacts a second** on that one thread (486 in 3 s), each "finalize incremental
marking via task", started for "approaching global allocation limit": `global_consumed_bytes` 9.36 GB against an old
generation of 1.6 MB. V8 counts the game's shared linear memory (modules at 8 GiB plus the heap: ~9.4 GB) as this
isolate's external memory, so after every collection the global limit is only ~8 MB above consumption and the next
incremental marking starts at once. Each cycle marks every live WebGPU wrapper: the census found 15,874 objects,
14,791 of them bind groups (cache bound 50,000; many name views of destroyed textures).

Correction: `BG_CACHE_MAX` 4,096 (the cache retires all and remakes on use, as before at 50,000).

| GPU worker (run `bg1` vs `trace2`/`trace3`, `names2`/`fz1`) | before | after |
|---|---|---|
| live bind groups / objects | 14,791 / 15,874 | 2,599 / 3,623 |
| MajorGC | 162-136 a second, 1.30-1.46 ms each | 167 a second, 1.10 ms each |
| GC per frame (CPU profile) | 7.9 ms (`names2`), 13.8 ms (`fz1`) | 6.6 ms |
| busy share of the time | 58.4% / 63.4% | 58.3% |

The cycle count is set by the external-memory accounting, which page code cannot change: see Unproven.

### Final run (`final`, the library build relinked from this worktree without profiling names)

Chrome on GPU 1 (adapter "nvidia blackwell"), another session's native Mario Kart World running, no builds. Boot,
menu, beach gameplay (`t120.png`: Kayla on the pier, HUD with W8's colours, sharp; `t160.png`: spiders attacking, the
game's red damage vignette), 0 WebGPU errors, no lost device, swap chain 3376x1899 then 3114x1751 B8G8R8A8_SRGB.
**43.2 FPS** over 95-135 s (W8 final 40.7; goal 60 not reached); last GPU worker lines 47-51 FPS (median 19-20 ms).
Other unprofiled runs of the change-1 build on a quieter machine: 47.0 (`fz2`), 57.9 (`heap1`, GPU worker heap
sampling only; another wave's Chrome started in its last minute), 58.3 over 95-125 s (`trace1`, 5 s trace).

| per frame | native | W8 final | W8b |
|---|---|---|---|
| FPS 95-135 s | 59.7 | 40.7 | 43.2 (`final`; 47.0-58 in quieter runs) |
| main thread guest code | 5.7 ms CPU (all work) | 29.7 ms work (profiled) | 18-27 ms (scene-dependent; unchanged by this wave) |
| job threads' FP (`fmaf` + job function) | - | ~4 ms | ~1 ms |
| GPU worker busy / GC | - | 59-62% / 33% | 58% / 25% of the time |

### Unproven / next

- **60 FPS:** two limits remain. (1) The main game thread: 18-27 ms of guest code a frame, flat over ~2,600 functions,
  bounds checks already trap-handled. What is left is V8's code quality and its tier-up, neither of which a page
  controls. Worth measuring next: wasm-opt -O2/-O3 over the module, with several runs a side because of the scene
  spread. (2) The GPU worker's continuous GC. The structural fix is to keep V8's external-memory figure for that
  isolate below its global maximum: a smaller linear memory (modules below today's 8 GiB `WEB_FIRST_BASE`;
  docs/35 W2's layout decision, needs a full retranslation), or a GPU worker that does not hold the wasm memory (a
  separate shared buffer for the ring and the upload data). Both are changes beyond this wave. A V8/Chromium report
  is worth filing: shared wasm memory counted per isolate makes any allocating worker GC continuously.
- FPS numbers are indicative: the machine was shared (other waves' Chrome on GPU 1, native MKW, builds) and the scene
  varies between runs.
