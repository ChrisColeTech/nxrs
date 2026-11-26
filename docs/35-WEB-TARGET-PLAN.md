# 35 - Web target plan (nxrs → wasm + WebGPU, played in Chrome)

**Status: plan, owner-directed 2026-11-26.** A second nxrs build target turns a Switch package into a **browser
build**, delivered the way the GTA V web port in `P:\Games\Web\GTA5` is: Emscripten-built wasm, a JavaScript host
(workers for the GPU, file I/O and audio), and game data streamed on demand. **Chrome is the target**; the game is
played in a tab or in Chrome's own app window (Install as app). No separate native player, no Electron. The Windows
native target stays the reference and must not regress.

## Work progression

States: **Complete** (done and verified, evidence named), **In progress**, **Not started**. One row per deliverable;
update the row when its state changes, with the evidence or commit.

| Phase | Work item | Files | State | Evidence / notes |
|---|---|---|---|---|
| W0 | Generated C compiles to wasm32 and wasm64 | `build/WEB/W0-wasm/include`, `compile.sh` | Complete | 416/416 units, both widths |
| W0 | Module links; V8 and Chrome load it | `build/WEB/W0-wasm`, `build/WEB/W0-chrome` | Complete | 414 MiB wasm64; Chrome 158 compile 1.2 s, instantiate 87 ms |
| W0 | Hot set recorder + split | `src/runtime/src/rt.rs` (`NXRS_FNPROF`) | Complete | 8.5% of functions; hot module 108.5 MiB (`cd39cb3`) |
| W0 | Compact guest layout switch | `src/runtime/src/mem.rs` (`NXRT_LAYOUT=compact`) | Complete | runs with ≥ 6 GiB alias (`58c6d7b`) |
| W0 | Shaders to WGSL, checked in Chrome | `build/WEB/W0-shaders` | Complete | used 111/111 in Chrome; sample 834/834 with two rules |
| W0 | FP under wasm | `build/WEB/W0-fp` | Complete | default FPCR + DN bit-identical; FZ / rounding modes differ |
| W1 | `__wasm__` branches in the translator's runtime headers | `src/translator/runtime/cpu.h`, `simd.h`, `simd_fp.h` | Complete | `RT_HOST`, 64-bit `Cpu` slots, wasm FP branches; native machine code identical on 3 chunks (~400k instructions, `build/WEB/W1/hdrcheck`); wasm64 object identical to W0 |
| W1 | emcc compile lane and `--target web` | `src/cli/src/cc.rs`, `main.rs`, `pipeline.rs` | Complete | `nxrs-build build <nsp> --target web` → `<input folder>\nxrs-web\<name>\game.js` + `game.wasm` (414 MiB) for Anime Girls: 417 units through the normal chunking and object cache, emsdk found as `build\tools\emsdk` or `NXRS_EMSDK`, cached rebuild 17 s (`build/WEB/W1/build-web.log`). Runtime data defined by the link until the runtime joins (W2) |
| W1 | Platform-only dependencies gated off for wasm | `src/graphics/Cargo.toml`, `graphics/src/lib.rs`, `os/src/nvn/mod.rs` | Complete | winit is not built for `target_os = "emscripten"` (no host window on web); every other third-party crate builds for the web target as is; zstd-sys's C through emcc (`web.ps1`) |
| W1 | Rust std on wasm64 + Emscripten | `scripts/build/wasm64-unknown-emscripten.json`, `scripts/build/web.ps1` | Complete | Rust ships no 64-bit Emscripten target: custom target spec, nightly `1.102.0-nightly (6c709eb42 2026-11-25)` pinned by `web.ps1`, `-Zbuild-std`. Probe (`build/WEB/W1/rustprobe`): 4 std threads joined under emcc pthreads, 64-bit pointers, correct result |
| W1 | nxrs crates link into the module | `src/runtime`, `src/os`, `src/graphics`, `src/cli/src/cc.rs` | Complete | `web.ps1 -Crates os` builds `libnxos.a` (runtime + os + graphics) for wasm64; `nxrs-build --target web` links it with the game (`NXRS_WEB_LIB`), `main` on a pthread, wasm exceptions |
| W2 | libc crate layouts for 64-bit Emscripten | `scripts/build/libc-wasm64.patch`, `scripts/build/web.ps1` | Complete | libc 0.2.191's Emscripten types are 32-bit: `nlink_t`, `blksize_t`, `statvfs`, `msghdr` differed from C (std::fs::metadata read every file as 0 bytes). Patched copy for the nightly std (`[patch]` in the toolchain's library workspace); 38 struct sizes/offsets checked against C (`build/WEB/W2/layout`) |
| W2 | Boundary between generated code and Rust | `src/runtime/web/boundary.c` | Complete | asm.rs's dispatch, barriers and jumps in C: setjmp/longjmp over wasm exceptions, dispatch as a guaranteed tail call (`-mtail-call`) |
| W1 | Windows build unchanged | — | Complete | engine tests green (graphics 261, os 166, runtime 50, cli 30); Anime Girls rebuilt natively with the new headers (417 units, 163 s) and played: beach gameplay, 59.9 FPS, normal exit (`build/play/20261126-162247-web-w1-native-check`) |
| W2 | Web module base: translate modules at a low base so guest address = linear address | `src/translator/src/loader/layout.rs` (`FIRST_BASE` → option), `src/cli/src/pipeline.rs` | Complete | `WEB_FIRST_BASE` 8 GiB: Anime Girls' main at 0x2_0000_0000; `rt_guest_off` 0 | Design (2026-11-26): with modules at ~8 GiB instead of `0x70_0000_0000` and the compact layout around them, the whole guest window lies inside the 16 GiB linear memory, so the ~225 raw guest-pointer reads in `runtime`/`os` need no translation and `rt_guest_off` is 0 |
| W2 | Linear-memory map (compact default, the guest window claimed from the heap's break) | `src/runtime/src/mem.rs`, `web.rs` | Complete | `web.rs`: reservations claimed with sbrk, commit/protection in a page table, decommit zeroes; compact map is the web default (alias 6144 MiB) |
| W2 | Fault-based tricks replaced (lazy heap, faults) | `src/runtime/src/fault.rs`, `process.rs`, `os/src/mem.rs`, `graphics/src/vk/lazy.rs`, `os/src/wwatch.rs` | Complete | no fault handlers on the web; lazy heap and lazy write-back off there; wwatch says it is unavailable |
| W2 | Guest threads as pthreads/workers | `src/runtime/src/kernel.rs` | Complete | std threads under emcc pthreads; 31 guest threads created under Node; host stack 32 MiB on the web (31 × 256 MiB did not fit beside the guest window) |
| W2 | setjmp/longjmp via wasm exceptions; fibers refused | `src/runtime/web/boundary.c`, `fiber.rs` | In progress | built and linked; no guest jump has happened yet in a run, so the path is unexercised. Fibers stop with a message |
| W2 | Other host services on the web | `src/os/src/input/*`, `media/webcodec.rs`, `ui.rs`, `services/swkbd.rs`, `services/time.rs`, `services/fsp.rs`, `graphics/src/vk/*` | Complete (stand-ins named) | no XInput/WinMM/HID/BLE pads (W3 input block), H.264/AAC refuse to open (WebCodecs later), prompts and keyboard answer the default, no Vulkan surface (W4) |
| W2 | FPCR values logged; FZ / rounding software paths if needed | `src/runtime/src/svc.rs`, `src/translator/runtime/simd_fp.h` | Complete | Anime Girls runs with FPCR 0 and FZ (no directed rounding). On wasm, FZ ops take the software flush path: FZ differences against native fell from ~330 words in 9 classes to 2 (FMADD / FMLA rounding up to the smallest normal, the documented edge); FPCR 0 still bit-identical (`build/WEB/W0-fp`) |
| W2 | Page, boot, on-page log, dev server | `src/web/index.html`, `boot.js`, `dev_server.py` | Complete | cross-origin and memory64 checks; engine log on the page and in the console; main on its own pthread (Emscripten). W2 data path: the whole data folder fetched into the in-memory file system before main (936 MiB in 4.4 s on localhost); W3 replaces it with reads on demand |
| W2 | Anime Girls reaches `nnMain` in Chrome | — | Complete | **Chrome 158**: libc/SDK init, the modules' initializers, `nnMain`, SSL, oe, hid, NVN bootstrap (no Vulkan device: W4), audio open (no device: W3), save mounted (`/data/save/user1`), game threads scheduled; then quiet, waiting on the GPU. Before that, under Node (`NXRS_WEB_LINK_FLAGS=-sNODERAWFS=1`): libc/SDK init, the three modules' initializers, `nnMain`, then SSL, oe, hid, NVN bootstrap (no Vulkan device: W4), audio open (no device: W3); 31 threads; three runs of 120 s without a crash after the stack fix. Chrome needs the page and file worker |
| W3 | I/O worker, RomFS manifest, boot set | `src/web/io-worker.js`, `src/os/src/services/fs.rs`, `fsp.rs`, `src/cli/src/webpack.rs` | Not started | |
| W3 | Saves in OPFS with acknowledged commits | `src/web/saves.js`, `src/os/src/services/account.rs` | Not started | |
| W3 | Audio ring + worklet | `src/web/audio-worklet.js`, `src/os/src/services/audio.rs`, `audren.rs` | Not started | |
| W3 | Input block (keyboard, gamepad) | `src/web/input.js`, `src/os/src/services/hid.rs` | Not started | |
| W4 | Graphics split: draw decisions vs Vulkan calls; command ring | `src/graphics/src/nvn/`, `vk/`, `ring.rs` | Not started | |
| W4 | GPU worker (WebGPU executor) | `src/web/gpu-worker.js` | Not started | |
| W4 | SPIR-V → WGSL stage and shader packs | `src/shaders/src/wgsl.rs` | Not started | |
| W4 | Pool sync without write watch | `src/graphics/src/vk/mem.rs` | Not started | |
| W4 | Anime Girls frames in Chrome | — | Not started | |
| W5 | Output layout + `build.json`; launcher Build for web, serving, Play | `src/cli/src/webpack.rs`, `src/launcher/src/services/web_serve.rs`, `flows/`, `ui/`, `docs/LAUNCHER_CONTRACT.md` | Not started | |
| W6 | Hot/cold split, wasm-opt, compressed data, lazy shader packs | `src/cli/src/webpack.rs`, `src/web/boot.js`, `io-worker.js` | Not started | |
| W7 | Anime Girls to first gameplay at FPS goal in Chrome; second title; Android Chrome | `build/WEB/W7`, `docs/34-LIBRARY-VALIDATION.md` | Not started | |

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
| (b) module | Linked at -O1, no wasm-opt: **wasm32 397 MiB, wasm64 414 MiB**, 327,683 functions, largest 0.40 MiB. V8 13.6 (Node 24): validate 0.3 s; lazy compile 0.5 s / ~0.9 GB; eager baseline 3.7 s / 2.3 GB (32) and 6.2 s / 3.0 GB (64); eager optimizing 100 s / 3.3 GB and 166 s / 3.9 GB. **In Chrome 158** (cross-origin isolated, localhost): wasm64 streamed + compiled in 1.2 s, instantiated in 87 ms against a shared 64-bit memory with a 16 GiB maximum; the 108.5 MiB hot module compiles in 0.25 s and instantiates in 0.2 s with its 290,107 placeholder imports (`build/WEB/W0-chrome`) |
| (c) hot set | 26,824 functions (8.5%) from boot through two minutes of beach gameplay, 12.1% of the C. `wasm-split`: **hot 108.5 MiB**, cold 296 MiB. Recorder: `NXRS_FNPROF=<file>` |
| (d) memory | ~2.27 GB guest committed. `NXRT_LAYOUT=compact` runs with an alias region of **≥ 6 GiB** (6,144 / 8,192 MiB ok; 5,120 / 4,096 MiB crash; 3,072 MiB SDK abort): a ~7.3 GiB guest window, so **wasm64 is required** for this title; it fits Chrome's 16 GiB cap |
| (e) shaders | Used set 111/114 → valid WGSL (naga) after five rewrites; random 1,000 of 53,109 variants: 834 (164 need a base-instance uniform, 2 naga defects). Used set 0.94 MB gzip. **Chrome 158 (RTX 5090) compiles all 111 used shaders** with no errors or warnings in 1.0 s. Of the 834 sample, 739 compile as is; 94 fail WGSL derivative uniformity (`textureSampleBias` in divergent flow, legal in Vulkan) and pass with `diagnostic(off, derivative_uniformity)`; 1 declares a read_write storage buffer in a vertex shader that it never writes and passes as `read`. Both become converter rules: **833+1 of 834** |
| (f) fibers | Not used by this title |
| (g) FP | **Default FPCR and DN: all 27 FP/SIMD oracle classes bit-identical wasm vs native.** FZ and directed rounding differ (W0 header variant ignores them) and need software flush/rounding paths; the game writes FPCR at runtime from SDK code (values to log in W2). `build/WEB/W0-fp/report.md` |
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
isnan/isinf, base instance as a per-draw uniform, `diagnostic(off, derivative_uniformity)`, unwritten vertex-stage
storage buffers declared `read`). Textures: BC7 where `texture-compression-bc` exists, ASTC where
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
| **W0 Feasibility** | **Done** (section 2) | Chrome loads the module, compiles the shaders, FP measured. Carried into W2: log the game's FPCR values; FZ / directed-rounding software paths |
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

**D1-D4 approved by the owner, 2026-11-26.**

| # | Decision | Recommendation |
|---|---|---|
| D1 | `DECISIONS.md` §3 makes nxrs Vulkan only; the web target renders with WebGPU | Amend: native Vulkan only, web WebGPU |
| D2 | Rust crates on wasm64 via nightly + build-std, linked by emcc | Accept, pinned toolchain |
| D3 | GPU boundary: command ring + JS GPU worker (GTA's design) | Accept |
| D4 | First title | Anime Girls; second chosen after W4 |
