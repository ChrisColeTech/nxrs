# 35 - Web target plan (nxrs → WebAssembly + WebGPU)

**Status: proposal, 2026-11-26. Nothing in this document is implemented.** It plans a second nxrs build target that
turns a Switch package into a browser build (`.wasm` + JS + streamed data) the way `P:\Games\Web\GTA5` is served.
The Windows target stays the reference: the web target reuses its translator, HLE and profiles and must not
regress it. Owner decisions this plan needs are in section 9; work past phase W0 waits for them.

## 1. What the web target is

```text
NSP ──► translator (unchanged) ──► generated C ──► emcc ──► game.wasm (hot) + game.cold.wasm
                                                     │
            os / runtime / graphics (cfg wasm) ──────┘──► nxrs.js host + workers
RomFS ──► per-file manifest + recorded boot ranges ──► HTTP range streaming + OPFS cache
Shaders ──► used-pipeline set ──► WGSL pack (rest fetched on first use)
```

The input is the translator's C, **not** a finished Windows build: `<name>_code.dll` is x86-64 machine code and
cannot be converted. Everything runs as wasm compiled ahead of time by emcc; there is no JIT and no generic WASM or
ARM emulator in the shipped path (the existing `runtime/src/interp` remains the rare-path fallback it already is).

It is a **target of nxrs, not a separate project** and not part of wxrs. The translator, Horizon HLE, NVN semantics,
the shader translator and profiles are all nxrs code; a separate repository would be a fork that drifts with every
game fix. wxrs translates in the opposite direction and its own plan (02 §1) forbids coupling.

## 2. Measured starting point (Anime Girls Sun of a Beach, 0100C6801E0FA000, 2026-11-26 build)

| Quantity | Value | Consequence for web |
|---|---|---|
| Generated C | 416 files, 1,331 MB, **316,912 functions** (main 262,368 / sdk 40,376 / subsdk0 14,168) | ~20x the source behind GTA's 60 MB `game.wasm`; must be split (W6) |
| Trap-only functions | 685 (≈0 MB) | Padding is not the bloat; the bloat is real, mostly unused Unity/IL2CPP code |
| Branches via dispatch | 34,873 + 10,767 + 4,303 | Any function may be an indirect target, so static dead-code removal is unsafe |
| Windows code DLL | 330 MB | Order-of-magnitude hint only; wasm size is measured in W0 |
| Guest code base | `0x70_0000_0000` (≈448 GiB) | Generated `LD/ST` use guest addresses as host pointers; must be rebased (W1) |
| NSP / extracted RomFS | 565 MB / 887 MB | RomFS is compressed inside the NCA; stream compressed ranges, never ship extracted files |
| Build-time shaders | 53,109 SPIR-V (2.94 GB) + 187 MB json | Every Unity variant; WebGPU cannot use SPIR-V; ship only the used set as WGSL |
| Used pipelines | `pipeline-keys.bin`, 8 KB | The seed for the shipped shader set |
| `FN_ENTER` profiler | hook present in every function; `fnprof_hit` is a no-op (`runtime/src/rt.rs:398`) | Implement it to record the hot function set (W0) |

Host coupling today: `runtime` 8/29 files use Windows APIs (`mem.rs` VirtualAlloc + `GetWriteWatch`, `fiber.rs`
host fibers, `hostprof.rs`), `os` 20/89, `graphics` is ash/Vulkan with write-watch pool sync (`vk/mem.rs`), `front` is
a winit window, `cli/cc.rs` drives clang-cl/lld-link. `runtime/src/linux.rs` and `front/src/window_linux.rs`
establish the per-platform `cfg` pattern the web target follows.

## 2a. W0 results so far (2026-11-26)

Evidence: `build/WEB/W0-shaders/report.md`, `build/WEB/W0-gta/graphics.md`, `build/WEB/W0-gta/runtime-io.md`,
run `build/play/20261126-141932-web-w0-fnprof` (scenario `agsob`, reached beach gameplay, 59.8 FPS with the
recorder on), hot set `build/WEB/W0-run/fnprof-hot.txt`.

| Step | Result |
|---|---|
| (a)/(b) emcc compile, Chrome limits | **Open.** emsdk/Binaryen not installed. Size estimate from the Windows DLL: 266 MiB `.text` at `-O1` + 44 MiB `.rdata`, so roughly 200-300 MB of wasm before splitting |
| (c) hot set | **26,824 of 316,912 functions (8.5%) ran**, **12.1% of the C by size** (131 of 1,085 MiB), boot through two minutes of beach gameplay. At the estimate above the hot module is ~25-35 MB, below GTA's 60 MB. Recording costs nothing visible (`NXRS_FNPROF=<file>`, `rt.rs` `fnprof_hit`) |
| (d) memory | Guest committed ≈ **2.27 GB** (modules 110 MB, stack 59 MB, physical maps 1,985 MB, heap 0); process peak 4.4 GB on Windows. Committed memory fits wasm32, but the reserved layout spans `0x1_1000_0000`..`0x14_0000_0000` plus modules at `0x70_0000_0000` (alias region alone 64 GiB). W1 needs a compact HLE layout (smaller region sizes reported to the game) and a Windows run proving the game accepts it |
| (e) shaders | Used set 57 pipelines / 114 shaders: **111/114 convert to valid WGSL** after five representational rewrites; random 1,000 of 53,109 variants: 834 convert, 164 need a base-instance uniform, 2 hit a naga defect. No subgroup/f64/clip-distance use. Used set 0.94 MB gzip. Chrome/Tint acceptance untested |
| (f) fibers | **Not used**: no `InitializeFiber` in either run (the engine logs every one) |
| (g) FP under wasm | **Open** (needs emsdk) |
| (h) GTA reading | Done. Graphics: shared-memory ring + JS WebGPU worker, writes straight from shared-memory views, offline WGSL packs fetched lazily, async pipelines + recipe warmup. Runtime: engine `main` in a worker (no PROXY_TO_PTHREAD), I/O worker with per-thread request slots + hint ring, OPFS block store + journal, recorded first-touch boot set (555 MiB), acknowledged saves, AudioWorklet ring, 1 ms timer trick on Windows, `crossOriginIsolated` + memory64 probes |
| Pool sync | 43 mirror pools / 706 MB tracked, ~2.2-2.6 MB uploaded per frame. Hashing every pool every frame is not viable in a browser; the replacement must check only pools the frame's draws bind |

## 3. Reference: how the GTA web build is delivered

From `P:\Games\Web\GTA5\mirror\playgta5.com` (inspected 2026-11-26):

- Code is one 60 MB Emscripten `game.wasm` per variant (`b/<build>/`), plus `io_worker.js`, `wgpu_worker.js`,
  `audio-worklet.js`; rendering goes through WebGPU in a worker.
- Data is 18 GB as a loose per-file tree (`data/x64`, `data/common`), described by `data/manifest.json`
  (path, size, stamp). Nothing is downloaded up front: `bootset.json` / `bootset_low.json` list the byte ranges read at
  boot (often only an archive header), everything else is fetched by HTTP range on demand.
- The data itself was cut (few early DLC packs, small audio), which is not an option for nxrs: we ship the game's
  RomFS as-is and save bytes by streaming and compression only.

## 4. Architecture

### 4.1 Toolchain

- **emsdk pinned** in `scripts/` like the existing pinned tools; one toolchain compiles generated C, `runtime/guest`
  C and the Rust crates so there is one linker, one libc and one pthread implementation.
- Rust crates build for an Emscripten target. 32 vs 64-bit memory is decision D2 (section 9): wasm32 is tier 2 in Rust
  and fast in browsers but caps the whole process at 4 GiB; memory64 lifts that to 16 GiB in Chrome but Rust support
  is tier 3 and bounds checks cost speed. W0 measures both on the first title.
- Binaryen (`wasm-opt`, `wasm-split`) pinned with emsdk.

### 4.2 Generated code (translator stays unchanged)

- `cpu.h` / `simd.h` / `simd_fp.h` get a web variant selected by the build, not by the translator:
  - `LD*/ST*` become `*(T*)(g_mem_base + (a - GUEST_WINDOW_BASE))`. The OS layer guarantees every guest region lies
    inside one contiguous window (4.3), so this is one subtract and an add, no page table.
  - `immintrin` paths move to `wasm_simd128.h` (emscripten's SSE headers as a stopgap only where exact).
  - MXCSR (`__builtin_ia32_stmxcsr/ldmxcsr`) has no wasm equivalent: wasm floats never flush denormals and always
    round to nearest. FPCR.FZ / RMode are honoured on the existing exact path where the lifted code depends on them;
    the gap is measured in W0 with the oracle's FP classes, not assumed harmless.
  - `FN_ENTER` compiles out except in the profiling build.
- Indirect branch dispatch becomes `call_indirect` through the function table; no change to its semantics.

### 4.3 Runtime (`runtime`, `cfg(target_family = "wasm")`)

- **Memory:** one linear memory; `reserve/commit/decommit` map to a window allocator inside it (commit = bookkeeping,
  growth via `memory.grow`). The HLE's region layout (code, heap, alias, stack, TLS, transfer and GPU pools) is
  compressed into the window. Page protection and access faults do not exist in wasm: `protect`/`readable` become
  explicit checks where nxrs relies on them, and guest null dereferences do not fault. Every caller of `fault.rs`
  behaviour is listed and given an explicit replacement in W2; none is silently dropped.
- **Threads:** guest threads → Emscripten pthreads → Web Workers on `SharedArrayBuffer` (needs COOP/COEP headers). The
  game runs entirely off the browser main thread, which never blocks.
- **Fibers:** `fiber.rs` swaps host stacks; wasm cannot. Options are JSPI or the stack-switching proposal (preferred)
  or one worker per guest fiber with handoff. W0 records whether the first title uses `nn::os` fibers at all.
- **setjmp/longjmp** (`sjlj.rs`): wasm exception-handling based SjLj.

### 4.4 OS / HLE (`os`)

- **RomFS:** a manifest of the RomFS (path, offset, compressed extents) and a **boot range set** recorded from a real
  Windows run (the fs layer logs every range it reads). The browser prefetches the boot set, then fetches other ranges
  on demand from a worker and caches them in OPFS. Blocking reads wait on a futex in shared memory.
- **Saves:** OPFS, with the same commit semantics as the Windows save path.
- **Audio:** the audio renderer's output goes to an `AudioWorklet` through a shared-memory ring (as GTA's
  `audio-worklet.js`). No silent sink: if audio cannot start (autoplay policy), the shell says so.
- **Input:** keyboard / Gamepad API on the main thread → shared-memory pad state read by HID. Same default mapping and
  settings as the Windows front.
- **Time / clocks:** `performance.now()` based, cross-origin isolated for full resolution.

### 4.5 Graphics (`graphics`) - the largest piece

- A **WebGPU backend** beside the Vulkan one, behind the replay layer that already separates NVN semantics from Vulkan
  calls. Rendering runs in a worker on an `OffscreenCanvas`. How wasm reaches WebGPU is decision D3 (preferred: replay
  commands written to a shared-memory stream and executed by a JS WebGPU worker, as GTA's `wgpu_worker.js` does).
- **Shaders:** the `shaders` crate keeps translating Maxwell to SPIR-V; a new stage converts SPIR-V to WGSL (naga, or
  Tint at build time), and refuses visibly what WGSL cannot express. Shipped set = pipelines in `pipeline-keys.bin`
  from a real run; other shaders are converted at build time but fetched lazily, compressed.
- **Feature gaps WebGPU does not have:** geometry / tessellation / mesh shaders, transform feedback, sparse images,
  descriptor buffers, push constants, many storage-image formats, conditional rendering. Each gets an explicit
  lowering (compute emulation, uniform buffers, bind-group caches) or a per-title refusal listed in the launcher,
  never a skipped draw that pretends to work. Unity's forward/deferred paths are expected to need few of these;
  W0's shader census on the used set says which.
- **Pool sync:** `GetWriteWatch` has no browser equivalent. Replacement: hash or compare tracked pool pages at sync
  points, narrowed by the existing pool/alias bookkeeping. Its CPU cost is a W4 exit metric.
- **Textures:** reuse the ASTC → BC7 pack where `texture-compression-bc` exists; decode to RGBA8 otherwise.

### 4.6 Front and packaging (`front`, `cli`)

- `cli build --target web` writes `<source>\nxrs-web\<name>\` beside the Windows build layout:
  `index.html`, `nxrs.js`, workers, `game.wasm`, `game.cold.wasm`, `shaders/` packs, `romfs.manifest`,
  `bootset.json`, `build.json` (identity, hashes, tool versions).
- Browser shell keeps the Windows front's control grammar (controls page, screenshot, limiter, fullscreen, diagnostics
  copy, clean exit), only where functional.
- Served with COOP/COEP by the launcher's local web server (the one serving `P:\Games\Web`, `launcher-settings.json`
  port 8008); plain-http LAN serving breaks cross-origin isolation, so LAN play needs HTTPS.
- Nothing proprietary is ever uploaded or committed; builds are local, user-supplied-game only, as on Windows.

### 4.7 Size: prune by splitting, never by deleting

- **Code:** profile build records the hot function set (`fnprof_hit` → set of entry addresses) over boot and the
  validation route. `wasm-split` puts the hot set in `game.wasm` and the rest in `game.cold.wasm`, fetched in the
  background after boot; a call into a not-yet-loaded function waits for it (JSPI) instead of failing. Plus
  `-O2`/`-Oz`, `wasm-opt` duplicate-function elimination (IL2CPP generics), stripped names. Option D4: run cold code
  through `interp` instead of shipping it.
- **Shaders:** used set in the boot pack, the rest lazily (above), compressed (a 200-file `.spv` sample compressed
  3.4x with gzip; WGSL is text and compresses further).
- **Data:** RomFS ranges served from the compressed NCA extents with HTTP compression; boot set prefetched.

## 5. Phases

Each phase closes with a measured, reproducible result in `build/WEB/<phase>/report.md`. Real-I/O rule applies from
W2: no null GPU, fake pad or silent audio sink is shipped or counted as progress.

| Phase | Outcome | Exit criterion |
|---|---|---|
| **W0 Feasibility gate** | Numbers that decide go / no-go and D2-D4 | (a) Anime Girls' generated C compiles and links with emcc in wasm32 and memory64 with rebased `LD/ST`; (b) `.wasm` size raw / `wasm-opt` / split; Chrome accepts it, compile + instantiate time and memory recorded; (c) `fnprof_hit` implemented, hot set recorded on Windows through boot + first gameplay, hot/cold sizes; (d) guest region layout fits one window (size recorded); (e) shader census: used pipelines, SPIR-V → WGSL success rate, WebGPU feature gaps hit; (f) fiber use yes/no; (g) FP oracle classes run under wasm FP; (h) targeted GTA reading, design only, no code copied: WebGPU command protocol (`wgpu_worker.js`, `wgpu.compat.js`), streaming I/O and blocking reads (`io_worker.js`, `bootset.json`), audio ring (`audio-worklet.js`), thread pool / isolation setup (`game.js`), shader delivery (`shaders/`); start from what wxrs already recorded (docs 02 §3, `WXRS-V0I-01`) |
| **W1 Toolchain** | `--target web` builds a linkable module | pinned emsdk/Binaryen; `cc.rs` emcc backend with the Windows lane's cache and chunking; header variant; `cargo build` of all crates for the web target; Windows build bit-identical before/after |
| **W2 Runtime** | Guest code runs in the browser to nnMain | memory window, pthreads/workers, fibers (if used), SjLj, fault-replacement list closed; log streamed to the shell; reaches the title's `nnMain` in Chrome |
| **W3 HLE I/O** | Real files, saves, audio, input | RomFS streaming with recorded boot set (cold and warm load timed), OPFS save round trip, AudioWorklet audible with underrun counter, Gamepad + keyboard moving the pad state |
| **W4 WebGPU** | First real frames | WebGPU backend presents the title's frames repeatedly; WGSL set from used pipelines; pool sync without write watch, cost measured; GPU-device-lost surfaced in the shell |
| **W5 Shell + packaging** | One folder the launcher serves | output layout, `build.json` identity, COOP/COEP serving, browser shell controls, launcher "Build for web" action |
| **W6 Delivery** | Download sized like the NSP, not the build | hot/cold split live, lazy shader packs, compressed ranges; first-launch bytes and time-to-title recorded |
| **W7 Validation** | Passes the nxrs bar in Chrome | Anime Girls gets past intro videos into first gameplay at its FPS goal (`feedback-nxrs-validation-bar`); then one second, structurally different title |

W0 is a measurement phase and writes no product code beyond `fnprof_hit` recording and a throwaway link script. If
W0(b) shows the module cannot be compiled by Chrome even after splitting, the plan stops there.

## 6. Code layout

| Crate / path | Change |
|---|---|
| `translator` | none (web header variant is chosen at compile time) |
| `runtime/guest` include headers | web variant of `cpu.h`, `simd.h`, `simd_fp.h` |
| `runtime` | `src/web.rs` beside `linux.rs`; `cfg(target_family = "wasm")` arms in `mem.rs`, `fiber.rs`, `sjlj.rs`, `rt.rs` (`fnprof_hit` recording, all targets) |
| `os` | `cfg` arms for fs (streaming RomFS), saves (OPFS), audio (worklet ring), input, time |
| `graphics` | `src/webgpu/` backend beside `src/vk/`; pool sync without write watch |
| `shaders` | SPIR-V → WGSL stage, used-set packing |
| `front` | `src/web/` shell (JS/TS sources + Rust glue) |
| `cli` | `cc.rs` emcc backend, `--target web`, profile build, boot-set and hot-set recorders, packager |
| `launcher` | "Build for web" and serving, after W5 |

No new top-level project. A crate is added only where a module is a real unit (`graphics` WebGPU backend may become
its own crate if `ash` cannot be cfg'd out cleanly).

## 7. Risks

| Risk | Where it bites | Mitigation / gate |
|---|---|---|
| Module too large for Chrome to compile | W0(b) | hot/cold split; if the hot set alone fails, stop |
| Guest layout does not fit a 4 GiB (wasm32) or 16 GiB (memory64) window | W0(d) | compress HLE regions; per-title refusal for large games |
| No fiber equivalent | W2 | JSPI / stack switching; worker-per-fiber fallback |
| WGSL cannot express used shaders | W0(e), W4 | per-feature lowering; per-title refusal stated in the launcher |
| Pool sync cost without write watch | W4 | narrowed hashing; measured exit metric |
| wasm FP differs from FPCR.FZ / rounding | W0(g) | exact path where it matters; oracle classes under wasm |
| Browser memory pressure (game + hot code + caches) | W2-W6 | measure per phase; drop caches before code |
| Divergence from the Windows target | all | shared translator/HLE; Windows suite must stay green each phase |

## 8. Non-goals

- Converting finished Windows builds (`_code.dll`) to wasm.
- Cutting game data (GTA's approach); the RomFS ships as the game has it.
- Mobile browsers, Safari and Firefox before Chrome passes W7.
- Hosting games publicly; builds stay local to the owner's machine and network.

## 9. Owner decisions needed

| # | Decision | Recommendation |
|---|---|---|
| D1 | `DECISIONS.md` §3 makes nxrs Vulkan only. The web target needs a WebGPU renderer. | Amend to "Vulkan on desktop, WebGPU on web"; the desktop rule is unchanged |
| D2 | wasm32 or memory64. wasm32 bounds checks are free (the engine reserves 4 GiB + guard pages); memory64 needs an explicit check on each load/store, pointers double in size, and Rust's wasm64 is tier 3 (nightly + build-std). GTA ships memory64 (i64 addresses, 16 GiB max), so it is viable | Decide from W0's timing of both; wasm32 for titles that fit |
| D3 | How wasm reaches WebGPU: (a) `webgpu.h` via emdawnwebgpu, one JS call per API call; (b) the `wgpu` crate, which on the web needs `wasm32-unknown-unknown` + wasm-bindgen and does not mix with emcc-built C; (c) a command stream in shared memory decoded by a JS WebGPU worker, as GTA does (`wgpu_worker.js`, 48 opcodes per wxrs docs 02) | (c), matching the existing async replay split; (a) as fallback. Confirm with the GTA reading in W0(h) |
| D4 | Cold code: lazy `game.cold.wasm` or `interp` | Lazy module; `interp` only if W0 shows the cold module is the blocker |
| D5 | First title | Anime Girls (Unity, small, already built); second title chosen after W4 |
