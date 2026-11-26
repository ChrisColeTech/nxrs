# 35 - Portable wasm target plan (nxrs → one `.wasm` per game, played in the nxrs player window)

**Status: plan, owner-directed 2026-11-26 (option 1).** A second nxrs build target turns a Switch package into **one
portable game build**: the game's translated code plus the nxrs runtime, OS services and renderer front half, all
compiled to a single 64-bit WebAssembly module, with the game's data beside it. A small native **nxrs player** per
operating system (wasmtime + wgpu + a window, like the nxrs game window today) runs that same file on Windows, Linux
and macOS without rebuilding the game. The Windows native target stays the reference and must not regress.

A browser host for the same `.wasm` (the GTA web port's model, including mobile browsers) is an optional later phase
(W8): the module talks to its host only through the `nxhost` import set (section 4.3), so a JS implementation of that
set runs it unchanged. Nothing in W0-W7 depends on a browser.

## 1. What it is

```text
NSP ──► translator (unchanged) ──► generated C ─┐
nxrs runtime / os / graphics front (Rust, cfg wasm) ┴─► clang/rustc → wasm64 ──► <name>.wasm   (one file, every OS)
RomFS + WGSL shader pack + manifest ─────────────────────────────────────► <name>.nxdata (beside it)

nxrs-player (native, per OS): wasmtime (Cranelift, compiled-code cache) + wgpu (Vulkan / DX12 / Metal)
                              + window, audio, input, files  ──  implements `nxhost`
```

- The input is the translator's C, never a finished Windows build: `<name>_code.dll` is x86-64 machine code.
- "Without rebuilding" means the game is translated and compiled once, by whoever builds it, into a `.wasm`. The
  player compiles that wasm to the local CPU on first launch and caches the result (a cache, not a game rebuild).
- It is a target of nxrs, not a separate project and not part of wxrs: the translator, Horizon HLE, NVN semantics,
  shader translator and profiles are nxrs code. wxrs translates the other way (wasm → native) and its plan forbids
  coupling.
- Compared with the native build on the same PC, expect lower speed (wasm bounds checks on 64-bit memory, no direct
  page-protection tricks). The point is portability, not beating native.

## 2. Measured (Anime Girls Sun of a Beach, 0100C6801E0FA000; W0, 2026-11-26)

Evidence under `build/WEB/`: `W0-shaders/report.md`, `W0-gta/graphics.md`, `W0-gta/runtime-io.md`, `W0-run/`,
`W0-wasm/` (headers, scripts, logs, modules); runs `build/play/20261126-141932-web-w0-fnprof` and
`build/play/*-web-w0-compact-*`.

| Step | Result |
|---|---|
| Generated C | 416 units, 1,085 MiB of function bodies, **316,912 functions**; 685 trap-only (padding is not the bloat) |
| (a) compile to wasm | **All 416 units compile** with clang 22 (emsdk 7.0.1) for wasm32 (161 s, 24 jobs) and wasm64 (163 s), after a small web header variant (`build/WEB/W0-wasm/include`: guest address rebase `RT_HOST`, 64-bit `Cpu` slots, no MXCSR, one asm constraint). Translator output itself needed no change |
| (b) module | Linked (-O1, no wasm-opt): **wasm32 397 MiB, wasm64 414 MiB** (408 MiB code), 327,683 functions, **largest 0.40 MiB**. V8 13.6 validates both in 0.3 s; lazy compile 0.5 s / ~0.9 GB; eager baseline (Liftoff) 3.7 s / 2.3 GB (32) and 6.2 s / 3.0 GB (64); eager optimizing (TurboFan) 100 s / 3.3 GB and 166 s / 3.9 GB. wasmtime/Cranelift not yet measured (W0(i)) |
| (c) hot set | 26,824 functions (8.5%) ran from boot through two minutes of beach gameplay, 12.1% of the C. `wasm-split` on it: **hot module 108.5 MiB** (94 MiB code + 8 MiB of placeholder imports), cold 296 MiB. Recorder: `NXRS_FNPROF=<file>` |
| (d) memory | ~2.27 GB guest committed. `NXRT_LAYOUT=compact` (one window around the modules, kernel region moved beside them) **runs with an alias region of 6 GiB or more** (6,144 and 8,192 MiB: running at 45 s); 5,120 and 4,096 MiB crash, 3,072 MiB aborts in the SDK. So this title needs a ~7.3 GiB guest window: **wasm64 required** (fine: wasmtime has no 16 GiB browser cap) |
| (e) shaders | Used set (57 pipelines / 114 shaders) 111/114 → valid WGSL after five representational rewrites; random 1,000 of 53,109 variants 834 (164 need a base-instance uniform, 2 hit a naga defect). Used set 0.94 MB gzip |
| (f) fibers | Not used by this title |
| (g) FP under wasm | Open: wasm has no rounding modes and no flush-to-zero; measure with the T-lane oracle classes |
| (h) GTA web port reading | Done (design only): shared-memory command ring to a GPU worker, writes straight from shared-memory views, offline WGSL packs, async pipelines with recipe warmup; I/O worker with per-thread request slots, recorded boot set, acknowledged saves, audio ring |
| Pool sync | 43 pools / 706 MB tracked, 2.2-2.6 MB uploaded a frame; no write watch inside wasm, so the replacement must check only pools the frame's draws bind |

## 3. Why this shape

- **One file for every OS** needs a portable code format: wasm. The native player is small and per-OS; the game
  build is not.
- **wasmtime** compiles wasm ahead of time with Cranelift, supports 64-bit memory without a 16 GiB cap, shared
  memory and threads, SIMD, and caches compiled code on disk. No browser in the path.
- **wgpu** gives one GPU API over Vulkan, DX12 and Metal (WebGPU semantics), with WGSL shaders, the same model a
  browser host would use later.
- **Host boundary as a command ring** (GTA's proven design): the renderer inside the module writes resolved replay
  commands into shared memory; the player executes them with wgpu on its own thread. Few, coarse host calls keep the
  wasm/host crossing cheap, and the same ring is what a JS host would read.

## 4. Architecture

### 4.1 Inside `<name>.wasm`

- Translated game code (generated C, web header variant: `RT_HOST` rebase, `__wasm__` branches).
- nxrs `runtime` (CPU state, dispatch, kernel, sync, memory map in compact layout), `os` (Horizon HLE services),
  and the replay half of `graphics` (NVN state tracking, resource lifetimes, pass building, shader-key selection),
  built for `wasm64` with `cfg(target_family = "wasm")` arms beside the existing Windows/Linux ones. Their
  platform calls go to `nxhost` imports, never to Windows APIs.
- No JS glue, no Emscripten runtime: clang `--target=wasm64` for C, rustc for Rust, linked with wasm-ld.

### 4.2 `nxrs-player` (native, per OS)

- wasmtime embedding: memory64 + shared memory + threads + SIMD (+ exception handling for setjmp/longjmp, to
  verify); Cranelift compilation with the on-disk cache; one OS thread per guest thread, each running a new
  instance on the shared memory.
- wgpu device and surface on a winit window; the command-ring executor on a dedicated thread; WGSL pipelines
  created asynchronously from the shader pack.
- Audio (cpal ring), input (keyboard, gamepads), file I/O from `<name>.nxdata`, saves in the user's save folder.
- The nxrs front's control grammar where functional: controls page, screenshot, frame limiter, diagnostics,
  fullscreen, clean exit.

### 4.3 `nxhost`: the import set (versioned)

| Group | Contents |
|---|---|
| process | log line, fatal with message, exit, version handshake |
| threads | spawn (entry, arg, stack size) → id, join, yield; waits are wasm atomics (`memory.atomic.wait/notify`) |
| time | monotonic ns, wall clock |
| files | open in package / save area, size, range read and write into linear memory, async completion through a request ring (blocking readers wait on a futex word) |
| saves | commit with acknowledgement (no empty-start on a failed restore) |
| audio | shared ring (format, rate, channels), underrun counter |
| input | published input block (pad state, keys, touch-free) |
| gpu | command ring base/size, doorbell, fences/readback as futex words; shader pack lookup by key |

The same table, implemented in JS against browser APIs, is the W8 browser host.

### 4.4 Memory

64-bit linear memory holding the compact guest layout (`NXRT_LAYOUT=compact`, alias sized per title; Anime Girls
needs ≥ 6 GiB of alias address space) plus the runtime's own heap. Guest address → linear offset is one subtract
(`RT_HOST`). wasm has no page protection or write watch: every nxrs use of them (`fault.rs`, lazy heap commit,
`GetWriteWatch` pool sync) gets an explicit replacement listed and closed in W2/W4.

### 4.5 Graphics

- Replay layer stays in the module; it emits a resolved command stream (passes, pipeline and bind-group ids, draws,
  copies, uploads by guest address and size: the player reads them straight from linear memory).
- Shaders: Maxwell → SPIR-V (existing `shaders` crate) → WGSL at build time with the W0(e) rewrites (unused
  capabilities, split combined samplers, PointSize/clip members, isnan/isinf, base-instance uniform); packed by key.
- Textures: ASTC passes through where the GPU supports it (`TEXTURE_COMPRESSION_ASTC`), else BC7 from the existing
  ASTC→BC7 path, else RGBA8.
- WebGPU-model gaps (geometry/tessellation/mesh shaders, transform feedback, sparse images) get explicit lowerings or
  a per-title refusal the launcher shows; never a silently skipped draw.

### 4.6 Packaging and launcher

- `nxrs-build build --target wasm` writes `<source>\nxrs-wasm\<name>\`: `<name>.wasm`, `<name>.nxdata` (RomFS,
  compressed; WGSL pack; manifest), `build.json` (identity, hashes, tool versions, required `nxhost` version).
- The launcher gets the player beside the engine and a library entry kind "wasm build"; Play opens the player on it.
- Builds stay local and user-supplied, as today.

## 5. Phases

Each phase closes with a measured result in `build/WEB/<phase>/report.md`; from W2 the real-I/O rule applies (no null
GPU, fake pads or silent audio counted as progress).

| Phase | Outcome | Exit criterion |
|---|---|---|
| **W0 Feasibility** | Mostly done (section 2) | Remaining: (g) FP classes under wasm; (i) wasmtime: load the W0 wasm64 module, Cranelift compile time and memory, compiled-cache size, feature check (memory64, threads, SIMD, exception handling); (j) clang `--target=wasm64` without Emscripten links the 416 objects |
| **W1 Toolchain** | `nxrs-build --target wasm` produces a linkable module | Web header variant upstreamed into the translator's runtime headers; `cc.rs` wasm lane (pinned LLVM, chunking, object cache); runtime/os/graphics-replay build for wasm64 with `nxhost` stubs that fail loudly; Windows build unchanged |
| **W2 Player + runtime** | Guest code runs in the player to `nnMain` | `nxrs-player` window up; wasmtime with threads and shared memory; compact layout; fault/lazy-heap replacements closed; log in the player's diagnostics |
| **W3 Host I/O** | Real files, saves, audio, input | RomFS from `.nxdata`, save round trip, audible audio with underrun counter, keyboard and gamepad driving the pad state |
| **W4 Graphics** | The title's real frames on screen | Command ring → wgpu executor; WGSL pack from used pipelines; pool sync without write watch, cost measured; device loss surfaced |
| **W5 Packaging + launcher** | One folder the launcher plays | `.wasm` + `.nxdata` + `build.json`; launcher entry kind and Play; player shipped with the engine |
| **W6 Startup and size** | Cold start measured and bounded | Compiled-code cache; hot/cold split if compile time demands it; compressed data; first-launch and warm-launch times recorded |
| **W7 Validation** | nxrs bar, portable | Anime Girls past the intro into first gameplay at its FPS goal in the Windows player; then the **same `.wasm`** on Linux; then a second, structurally different title |
| **W8 Browser host (optional)** | Same `.wasm` in a browser | JS `nxhost` over WebGPU, workers, OPFS, HTTP range streaming (GTA design); desktop Chrome first, then Android Chrome; iOS only once Safari has 64-bit wasm |

## 6. Code layout

| Path | Change |
|---|---|
| `src/translator` | none; its `runtime/*.h` gain `__wasm__` branches and `RT_HOST` (W1) |
| `src/runtime`, `src/os` | `cfg(target_family = "wasm")` arms calling `nxhost`; compact layout (`mem.rs`, landed as a W0 switch) becomes the wasm default |
| `src/graphics` | replay half builds for wasm and writes the command ring; the Vulkan executor stays native-only |
| `src/shaders` | SPIR-V → WGSL stage with the W0(e) rewrites, shader pack writer |
| `src/cli` | `--target wasm` lane in `cc.rs` and the packager |
| `src/player` (new crate) | `nxrs-player`: wasmtime host, wgpu executor, window, audio, input, files, saves |
| `src/launcher` | entry kind and Play for wasm builds (after W5) |

## 7. Risks

| Risk | Phase | Mitigation / gate |
|---|---|---|
| Cranelift compile of ~400 MiB is slow or memory-heavy on first launch | W0(i) | compiled-code cache; hot/cold split; lazy tiers if wasmtime offers them |
| Rust on wasm64 (tier 3: nightly + build-std) | W1 | pin the toolchain; keep runtime crates `std`-light behind `nxhost` |
| setjmp/longjmp (`sjlj.rs`) needs wasm exception handling in wasmtime | W0(i), W2 | verify support; else a translator-level unwind scheme |
| Fibers (other titles) | W2+ | wasm stack switching in wasmtime when available; refuse per title until then |
| wasm FP differs from FPCR.FZ / directed rounding | W0(g) | exact slow paths where the oracle shows differences |
| Pool sync without write watch | W4 | draw-bound checking; measured exit metric |
| Large address windows (this title ≥ 7.3 GiB) | W2 | memory64 reservation is address space, not RAM; measure committed memory per title |
| Divergence from the native target | all | shared translator/HLE; native suite green each phase |

## 8. Non-goals

- Converting finished Windows builds (`_code.dll`) to wasm.
- Cutting game data.
- Native mobile apps (not wanted: an installed game takes device space; phones are only a browser-host question, W8).
- Public hosting of games.

## 9. Owner decisions

| # | Decision | Recommendation |
|---|---|---|
| D1 | `DECISIONS.md` §3 makes nxrs Vulkan only; the player renders through wgpu (Vulkan on Windows/Linux, Metal on macOS) | Amend: native target Vulkan only; wasm player wgpu |
| D2 | Rust runtime on wasm64 via nightly + build-std (tier 3) | Accept, pinned toolchain |
| D3 | Package: `.wasm` + `.nxdata` beside it, or one file | Two files (code updates without re-shipping data) |
| D4 | Browser host (W8) | Keep as optional later phase; design `nxhost` for it now |
| D5 | First title | Anime Girls; second chosen after W4 |
