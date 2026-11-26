# 35 - Web target plan (nxrs → wasm + WebGPU, played in Chrome)

**Status: plan, owner-directed 2026-11-26.** A second nxrs build target turns a Switch package into a **browser
build**, delivered the way the GTA V web port in `P:\Games\Web\GTA5` is: Emscripten-built wasm, a JavaScript host
(workers for the GPU, file I/O and audio), and game data streamed on demand. **Chrome is the target**; the game is
played in a tab or in Chrome's own app window (Install as app). No separate native player, no Electron. The Windows
native target stays the reference and must not regress.

## 1. What it is

```text
NSP ──► translator (unchanged) ──► generated C ──┐
nxrs runtime / os / graphics replay (Rust, cfg wasm) ┴─► emcc (memory64, pthreads, SIMD) ──► game.wasm + game.js
                                                                                        (hot + cold split)
host JS (ours): GPU worker (command ring → WebGPU), I/O worker (range fetch + OPFS), audio worklet, input, saves
RomFS ──► per-file manifest + recorded boot ranges ──► HTTP range streaming + OPFS cache
Shaders ──► used-pipeline set ──► WGSL packs (rest fetched on first use)
```

- The input is the translator's C, never a finished Windows build (`<name>_code.dll` is x86-64 machine code).
- It is a target of nxrs, not a separate project and not part of wxrs (which translates the other way).
- Other browsers: Android Chrome is a later check (memory64 + WebGPU are there; tab memory is the risk). Safari is
  out until it ships 64-bit wasm; Anime Girls needs it (section 2, (d)).

## 2. Measured (Anime Girls Sun of a Beach, 0100C6801E0FA000; W0, 2026-11-26)

Evidence under `build/WEB/`: `W0-shaders/report.md`, `W0-gta/graphics.md`, `W0-gta/runtime-io.md`, `W0-run/`,
`W0-wasm/` (web header variant, scripts, logs, modules); runs `build/play/20261126-141932-web-w0-fnprof` and
`build/play/*-web-w0-compact-*`.

| Step | Result |
|---|---|
| Generated C | 416 units, 1,085 MiB of function bodies, **316,912 functions**; 685 trap-only |
| (a) compile | **All 416 units compile** with emcc 7.0.1 (clang 22) for wasm32 (161 s, 24 jobs) and wasm64 (163 s) after a small header variant (`build/WEB/W0-wasm/include`: guest address rebase `RT_HOST`, 64-bit `Cpu` slots, no MXCSR, one asm constraint). Translator output itself unchanged |
| (b) module | Linked at -O1, no wasm-opt: **wasm32 397 MiB, wasm64 414 MiB**, 327,683 functions, largest 0.40 MiB. V8 13.6 (Node 24): validate 0.3 s; lazy compile 0.5 s / ~0.9 GB; eager baseline 3.7 s / 2.3 GB (32) and 6.2 s / 3.0 GB (64); eager optimizing 100 s / 3.3 GB and 166 s / 3.9 GB. Not yet loaded in Chrome itself |
| (c) hot set | 26,824 functions (8.5%) from boot through two minutes of beach gameplay, 12.1% of the C. `wasm-split`: **hot 108.5 MiB**, cold 296 MiB. Recorder: `NXRS_FNPROF=<file>` |
| (d) memory | ~2.27 GB guest committed. `NXRT_LAYOUT=compact` runs with an alias region of **≥ 6 GiB** (6,144 / 8,192 MiB ok; 5,120 / 4,096 MiB crash; 3,072 MiB SDK abort): a ~7.3 GiB guest window, so **wasm64 is required** for this title; it fits Chrome's 16 GiB cap |
| (e) shaders | Used set 111/114 → valid WGSL (naga) after five rewrites; random 1,000 of 53,109 variants: 834 (164 need a base-instance uniform, 2 naga defects). Used set 0.94 MB gzip. Chrome's own WGSL compiler not yet tried |
| (f) fibers | Not used by this title |
| (g) FP | Open: wasm has no rounding modes or flush-to-zero |
| (h) GTA reading | Done (design only, no code copied): see section 3 |
| Pool sync | 43 pools / 706 MB tracked, 2.2-2.6 MB uploaded a frame; no write watch in wasm, so check only pools the frame's draws bind |

## 3. Design taken from the GTA web port

From `build/WEB/W0-gta/*.md`:

- **Threads:** the engine's `main` runs in a worker (not the page), all helper workers created before it blocks;
  the page only does UI, input and the audio context. Refuse to start without `crossOriginIsolated`; probe memory64.
  On Windows, a 1 ms page timer keeps short `Atomics.wait` timeouts precise.
- **GPU:** the renderer writes a command ring in shared memory; one GPU worker owns the WebGPU device and an
  `OffscreenCanvas`, executes the ring, writes uploads straight from shared-memory views, answers fences and
  readbacks through futex words, yields every ~2 ms. nxrs streams its **already-resolved replay output** (passes,
  pipeline and bind-group ids, draws, uploads by guest address) so the JS executor stays thin.
- **Shaders:** WGSL generated offline, keyed by hash, in lazily fetched packs; pipelines created async; warmup from
  recorded recipes (the web form of `pipeline-keys.bin`).
- **Files:** one I/O worker, per-thread request slots plus a hint ring for prefetch; OPFS block store with a journal;
  HTTP range requests with response checks; a boot set recorded from a real run, prefetched while the module
  downloads and compiles.
- **Saves:** acknowledged commits; a failed restore never starts with empty saves.
- **Audio:** float stereo ring read by an AudioWorklet; underruns counted; context unlocked on first input.
- **Input:** a published input block (keys, pad) written with Atomics; gamepads through it too.
- **Serving:** COOP/COEP, exact 206 range responses, `application/wasm`, compression; served by the launcher's local
  web server (as `P:\Games\Web\GTA5` is, `launcher-settings.json`).

## 4. Architecture

### 4.1 Toolchain

- Pinned emsdk (W0 used 7.0.1) with Binaryen; flags `-sMEMORY64=1 -pthread -msimd128`, wasm exception handling for
  setjmp/longjmp (`sjlj.rs`), not the JS `invoke_*` mode GTA uses.
- Rust crates (`runtime`, `os`, graphics replay) must link into the same module: Rust has no wasm64 Emscripten
  target, so they build for `wasm64-unknown-unknown` (tier 3, nightly + build-std) as C-ABI static libraries linked
  by emcc, using Emscripten's libc and pthreads through FFI. Decision D2; proven or refuted in W1.

### 4.2 Generated code

The W0 header variant moves into the translator's runtime headers as `__wasm__` branches: `RT_HOST(a)` (guest
address → linear offset, one subtract), pointer-free 64-bit `Cpu` layout, no MXCSR, wasm SIMD for the vector types.

### 4.3 Runtime and OS (`cfg(target_family = "wasm")`)

- Compact guest layout in one linear memory (alias sized per title); `memory.grow` for commit.
- No page protection, faults or write watch: each nxrs use (`fault.rs`, lazy heap, pool sync) gets an explicit
  replacement, listed and closed in W2/W4.
- Guest threads → Emscripten pthreads (workers). Fibers: refuse per title until wasm stack switching (not needed for
  Anime Girls).
- Files from the I/O worker; saves through the acknowledged path; audio ring; input block.

### 4.4 Graphics

Replay layer in wasm → command ring → our GPU worker (WebGPU). Shaders: Maxwell → SPIR-V → WGSL at build time with the
W0(e) rewrites (drop unused capabilities, split combined samplers, PointSize/clip members to a dummy, exact
isnan/isinf, base instance as a per-draw uniform). Textures: BC7 where `texture-compression-bc` exists, ASTC where
`texture-compression-astc` exists, else RGBA8. WebGPU gaps (geometry/tessellation/mesh shaders, transform feedback,
sparse images) get explicit lowerings or a per-title refusal the launcher shows; never silently skipped draws.

### 4.5 Output and serving

`nxrs-build build --target web` writes `<source>\nxrs-web\<name>\`: `index.html`, our host JS, `game.wasm` (hot),
`game.cold.wasm`, `shaders/` packs, `romfs.manifest`, `bootset.json`, `build.json`. The launcher serves it locally
with the right headers and opens it in Chrome (tab or installed-app window).

### 4.6 Size and startup

- Code: hot/cold split from the `NXRS_FNPROF` profile (108.5 MiB boot module measured), cold module fetched in the
  background; `-O2`/`wasm-opt`, stripped names; Chrome caches compiled code.
- Data: RomFS streamed by range from compressed extents, boot set prefetched; shaders lazily, compressed.

## 5. Phases

Each phase closes with a measured result in `build/WEB/<phase>/report.md`; from W2 the real-I/O rule applies.

| Phase | Outcome | Exit criterion |
|---|---|---|
| **W0 Feasibility** | Mostly done (section 2) | Remaining: load the wasm64 module in **Chrome** (compile time, tab memory); Chrome's WGSL compiler on the used set; (g) FP classes under wasm |
| **W1 Toolchain** | `--target web` builds and links | Header variant upstreamed; `cc.rs` emcc lane (chunking, object cache); Rust crates linked into the module (D2); Windows build unchanged |
| **W2 Runtime** | Guest code reaches `nnMain` in Chrome | Workers, compact layout, fault/lazy-heap replacements closed; log visible in the page |
| **W3 Host I/O** | Real files, saves, audio, input | RomFS streaming with recorded boot set (cold/warm timed), OPFS save round trip, audible audio with underrun counter, keyboard + gamepad |
| **W4 Graphics** | The title's real frames in Chrome | Command ring → GPU worker; WGSL packs; pool sync without write watch, cost measured; device loss surfaced |
| **W5 Packaging** | One folder the launcher serves | Output layout, `build.json`, launcher "Build for web" and Play (opens Chrome) |
| **W6 Delivery** | First launch sized like the NSP, not the build | Hot/cold split live, lazy shader packs, compressed ranges; first-launch bytes and time-to-title recorded |
| **W7 Validation** | nxrs bar in Chrome | Anime Girls past the intro into first gameplay at its FPS goal; then a second, structurally different title; then an Android Chrome check |

## 6. Code layout

| Path | Change |
|---|---|
| `src/translator` runtime headers | `__wasm__` branches, `RT_HOST` |
| `src/runtime`, `src/os` | `cfg(target_family = "wasm")` arms; compact layout (landed as `NXRT_LAYOUT=compact`) default on web |
| `src/graphics` | replay half builds for wasm and writes the command ring; Vulkan executor stays native-only |
| `src/shaders` | SPIR-V → WGSL stage with the W0(e) rewrites, pack writer |
| `src/cli` | emcc lane in `cc.rs`, `--target web`, packager, boot-set recorder |
| `src/web` (new) | the host JS: page, engine worker bootstrap, GPU worker, I/O worker, audio worklet, input, saves |
| `src/launcher` | Build for web, local serving, Play in Chrome (after W5) |

## 7. Risks

| Risk | Phase | Mitigation / gate |
|---|---|---|
| Chrome compile time / tab memory for a ~400 MiB module | W0 | hot/cold split; Chrome code cache; stop if the hot module alone fails |
| Rust has no wasm64 Emscripten target | W1 | `wasm64-unknown-unknown` static libs linked by emcc (D2); fallback: runtime pieces in C |
| Per-title address window (this title ~7.3 GiB of 16 GiB) | W2 | compact layout; per-title refusal above the cap |
| WGSL accepted by naga but not Chrome | W0, W4 | test the used set in Chrome early |
| wasm FP vs FPCR.FZ / directed rounding | W0(g) | exact slow paths where the oracle differs |
| Pool sync without write watch | W4 | draw-bound checking; measured |
| Fibers (other titles) | later | refuse per title until wasm stack switching |
| Divergence from the native target | all | shared translator/HLE; native suite green each phase |

## 8. Non-goals

Converting finished Windows builds; cutting game data; a native player or Electron window; native mobile apps;
public hosting (builds stay local, user-supplied).

## 9. Owner decisions

| # | Decision | Recommendation |
|---|---|---|
| D1 | `DECISIONS.md` §3 makes nxrs Vulkan only; the web target renders with WebGPU | Amend: native Vulkan only, web WebGPU |
| D2 | Rust crates on wasm64 via nightly + build-std, linked by emcc | Accept, pinned toolchain |
| D3 | GPU boundary: command ring + JS GPU worker (GTA's design) | Accept |
| D4 | First title | Anime Girls; second chosen after W4 |
