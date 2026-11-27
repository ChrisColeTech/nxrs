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
| W2 | Page, boot, log, dev server | `src/web/index.html`, `boot.js`, `dev_server.py` | Complete | cross-origin and memory64 checks; **the page is the game (`WEB-WPAGE`):** canvas fills the window letterboxed, engine log to the console only (`NXRS_LOG` a 5,000-line ring), one notice line for loading / sound / failures, title from `build.json` (the on-page log grew ~3 divs a second); main on its own pthread (Emscripten). W2 data path: the whole data folder fetched into the in-memory file system before main (936 MiB in 4.4 s on localhost); W3 replaces it with reads on demand |
| W2 | Anime Girls reaches `nnMain` in Chrome | — | Complete | **Chrome 158**: libc/SDK init, the modules' initializers, `nnMain`, SSL, oe, hid, NVN bootstrap (no Vulkan device: W4), audio open (no device: W3), save mounted (`/data/save/user1`), game threads scheduled; then quiet, waiting on the GPU. Before that, under Node (`NXRS_WEB_LINK_FLAGS=-sNODERAWFS=1`): libc/SDK init, the three modules' initializers, `nnMain`, then SSL, oe, hid, NVN bootstrap (no Vulkan device: W4), audio open (no device: W3); 31 threads; three runs of 120 s without a crash after the stack fix. Chrome needs the page and file worker |
| W3 | I/O worker, RomFS manifest, boot set | `src/web/io-worker.js`, `src/os/src/web/io.rs`, `src/os/src/webdata.rs`, `services/fs.rs`, `boot.rs`, `translator/src/package/romfs.rs`, `src/cli/src/webpack.rs` | Complete | Beach Bounce in Chrome 158, no whole-folder preload (3 host files, 113 KiB, in the in-memory FS; RomFS + ExeFS through the I/O worker: shared-memory slots, futex waits on engine threads, range fetch + OPFS block store). Data 316.2 MiB / 940 files. Cold, no boot set: nnMain at 4.89 s with 18.3 MiB fetched (ExeFS), the game then streamed 316.1 MiB on demand within 30 s (2,025 requests). Cold with the recorded boot set (native `NXRS_BOOTSET` run: 932 files, 316.1 MiB, the whole RomFS, read in the first 60 s): prefetched in 1.98 s, nnMain at 4.87 s. Warm (OPFS): 0 bytes fetched, nnMain at 4.50 s. `WEB-W3` (`a40f08d`) |
| W3 | Saves in OPFS with acknowledged commits | `src/web/saves.js`, `io-worker.js`, `src/os/src/web/saves.rs`, `services/fs.rs` | In progress | Built: `fs::commit` packs the committed save, the I/O worker writes `.new`, flushes, moves over `.bin`, acknowledges through the slot (10 s bound, red line + failed commit otherwise); restore before main, a damaged save stops the start. Shown in Chrome on the worker and page halves (commit acknowledged in 13 ms, overwrite, reload, restored bytes equal, damaged file refused). Not yet shown from the game: Beach Bounce (Ren'Py) wrote no save in any web or native run driven blind; needs W4's frames to drive its Save menu |
| W3 | Audio ring + worklet | `src/web/audio-worklet.js`, `src/os/src/web/audio.rs`, `src/os/src/media/sink.rs` | Complete | 48 kHz context started at the first click; ring 8192 frames kept 3072 ahead; Beach Bounce's music reaches the worklet (mix RMS about -21 dBFS); 0 underruns over 90 s in a clean tab (74 events in a tab still running an old instance); underruns and the longest refill gap logged |
| W3 | Input block (keyboard, gamepad) | `src/web/input.js`, `src/os/src/input/web.rs`, `input/mod.rs` | In progress | Keyboard: page key → block (held bits + press latch) → input thread → `hid`: `NXRS_CONTROLLER_TRACE` shows the game reading A 0x1, B 0x2, D-pad right 0x4000 / down 0x8000 after key presses, a CDP tap included. Gamepad: written and mapped (unit test); Chrome 158 sees the attached DualSense (054c:0ce6) with `standard` mapping, 18 buttons, 4 axes (2026-11-27); not yet shown driving the game |
| W4 | Graphics split: draw decisions vs Vulkan calls; command ring | `src/graphics/src/web/` (`icd.rs`, `cmd.rs`, `submit.rs`, `ring.rs`, `obj.rs`, `fmt.rs`) | Complete | The cut is under the replay, not inside it: a Vulkan device implemented on WebGPU (web build only; ash loads its tables from our `vkGetInstanceProcAddr`), command buffers kept as records and resolved at submission into WebGPU-shaped ring packets (passes, pipelines from the draw's dynamic state, bind groups, draws, copies, uploads by linear address, read-backs, timeline signals). Native: `cfg(emscripten)` arms only (`docs/waves/WEB-W4.md` §1-2) |
| W4 | GPU worker (WebGPU executor) | `src/web/gpu-worker.js`, `gpu.js`, `nxgfx.lib.js` | Complete | OffscreenCanvas + WebGPU device in a worker, `Atomics.waitAsync` on the ring, uploads/read-backs on the shared memory, blits/clears by small pipelines; frame line and `?shots=N` for development |
| W4 | SPIR-V → WGSL stage and shader packs | `src/shaders/src/wgsl.rs` | Complete | Run-time shaders translated on request by the launcher (`WEB-W7b`, W7 row); web loop bound 4,096 back-edges a call, TDR-safe (`WEB-WPAGE`; native 65,536). Conversion done (W0 rules + outer product, Y flip / depth mode / base instance from a per-draw uniform): used set 114/114, the replay's kernels (test). Converted at vkCreateShaderModule (the final SPIR-V is made at run time); the library is served on first use. **Shader packs (`WEB-W6b`):** the modules a native run recorded (`NXGFX_SHADER_REC`, first-use order, `<name>_data\shaders.rec.txt`) cut into ~1 MiB packs beside the page (`shaders/pack-<n>.bin` + `packs.txt`, format `src/shaders/src/pack.rs`), fetched whole at the first need of any member: Anime Girls 175 modules in 5 packs (1.3 MiB gzip), a cold launch to the beach makes 11 shader requests instead of 346. SPIR-V packs; WGSL is still made at module creation |
| W4 | Pool sync without write watch | `src/graphics/src/vk/mem.rs`, `image.rs` | Complete | Sync by use runs in Chrome (W4c/W4d, beach 56-60 FPS in W6); `WEB-WPAGE`: pools compared by use are never answered from the MKW-SETS lane caches' copy (their CPU writes were never uploaded: the frozen splash / black canvas). Earlier: Measured (W4): snapshot compare of every mirror (38 pools, 626 MB) 86 ms a frame, the FPS limit (~10 FPS). Built (WEB-W4b, `24061d1`): pages compared only when a use reads them, once a sync (buffers at `resolve_in`, textures at `image_begin_submit`), word compare; `NXGFX_WEB_USE_SYNC=0` the old path; unit-tested. Not measured in Chrome: every run (base `055c41d` included) stalls after the 4th submission before the title menu (`docs/waves/WEB-W4b.md` §5.1) |
| W4 | Anime Girls frames in Chrome | — | Complete | `WEB-WPAGE` `after-fix` (Chrome on GPU 1): menu at 40 s, beach gameplay with HUD at 120 s, 0 WebGPU errors (the lost submission every ~110 s fixed: bind groups destroyed before their submission). Open: on 2026-11-27 two runs on the display GPU hung it (`DXGI_ERROR_DEVICE_HUNG`) and the second locked the machine; cause not named (`?subtrace=1` names the submission if it recurs; web runs now on GPU 1 only). Earlier, W4: Chrome 158: title menu, level select, beach gameplay drawn (`build/WEB/W4/m1-title-menu-chrome.png`, `m4-beach-gameplay-chrome.png`), 0 WebGPU errors in the gameplay run; 9.7-10.7 FPS (pool sync) |
| W5 | Output layout + `build.json`; launcher Build for web, serving, Play | `src/cli/src/webhost.rs`, `src/launcher/src/services/web_serve.rs`, `web_reply.rs`, `browser.rs`, `flows/web.rs`, `ui/chrome/menu_*.rs`, `docs/LAUNCHER_CONTRACT.md` | Complete | `--target web` writes the 8 host files (embedded, `HOST_FILES`) and `build.json` (format 1, roles); the data folder stays beside the build. Launcher: in-process server on 127.0.0.1 (COOP/COEP/CORP, exact 206/416, `/<slug>/data/` → the data folder, shader files included, POSTs taken), fixed port 8780 (`NXRS_WEB_PORT`) so OPFS data and saves survive; one **Play** and one **Build** (WEB-W5b, owner 2026-11-27: not two of each) in the Game menu, the context menu, the card, Enter and Ctrl+B, each opening a Native / Web chooser (two equal buttons acting at once; a target that cannot run disabled with its reason: no native build yet, no web build yet, no Chromium browser found — the default if Chromium-based, else Chrome by path). Beach Bounce in Chrome 158 from the launcher's server, opened by clicking Play in browser (W5's entry, before W5b): `crossOriginIsolated` true, boot set 316.1 MiB fetched cold in 1.65 s (1,115 range requests), nnMain 4.68 s, audio running after a click at about -21 dBFS, GPU worker 60 FPS presents (frames black: this title's shaders are not translated for the web, W4's side). `WEB-W5`, evidence `build/WEB/W5` |
| W6 | Hot/cold split, wasm-opt, compressed data, lazy shader packs | `src/cli/src/websplit.rs`, `webzip.rs`, `webpack.rs`, `cc.rs`, `src/web/split.js`, `io-worker.js`, `dev_server.py`, launcher `web_reply.rs`, `web_file.rs`, `model/http_cache.rs` | Complete (split opt-in) | Built (`WEB-W6`, `docs/waves/WEB-W6.md`): hot set from a native `NXRS_FNPROF` run (`<name>_data\hotset.txt`, 26,774 functions) -> `wasm-split` after `wasm-opt -O1` (457 -> 410 MiB, ~4 min): `game.wasm` 140 MiB (49 MiB gzip) + `game.cold.wasm` 273 MiB (53.5 MiB gzip), names stripped, `game.symbols` kept; gzip copies sent with Content-Encoding by both servers, ETag revalidation; game data as 64 KiB raw-deflate extents (`zdata/`, 905 -> 410 MiB) inflated by the I/O worker; Windows font replaced by the built-in Hind (OFL license beside the page); per-worktree `EM_CACHE`. **Chrome, cold, launcher server: 1,362 -> 551 MB, 7,449 -> 1,530 requests, title ~32 -> <= 26 s, beach 56-60 FPS** (one module). **`WEB-W6b`:** lazy shader packs (W4 row; 346 -> 11 shader requests); warm launch **112.1 MB -> 0.11 MB** (the page keeps `game.wasm` in Cache Storage and revalidates it with `If-None-Match`: Chrome's HTTP cache never stored the 107 MB entry), nnMain 3.8 s warm; cold 546.5 MB / 1,186 requests, title <= 28 s, beach 47-60 FPS with another wave's game running. **Split: not usable with pthreads** - each of ~40 engine threads instantiates the hot module's 290,138 placeholder imports (~140 MiB per instance), the renderer reaches 16.6 GB and is killed (`docs/waves/WEB-W6b.md` §1); one module stays the default, the split opt-in |
| W7 | Anime Girls to first gameplay at FPS goal in Chrome; second title; Android Chrome | `build/WEB/W7`, `docs/34-LIBRARY-VALIDATION.md`, `src/cli/src/shaderimport.rs`, `src/graphics/src/vk/live.rs`, `src/os/src/nvn/mod.rs`, launcher `model/args.rs` | In progress | **Anime Girls FPS (2026-11-27, `WEB-WPAGE`):** beach gameplay 23-47 FPS with the frames really drawn (earlier 56-60 readings included the frozen splash); below the 60 goal. **Memory (`WEB-WMEM`):** renderer private 12,497 -> 10,630 MiB (web first base 7.25 GiB, web host stacks 4 MiB); ~5.5 GB still committed and unused (the alias region's bottom 3 GiB). **Builds below normal priority (`WEB-WFREEZE`):** a normal program's 2 ms of work worst 643 -> 6.8 ms during a web build. **Second title (Beach Bounce Remastered, Ren'Py), `WEB-W7`:** the black frames were every draw skipped for a missing shader: its 16 shaders are made at run time and the browser cannot run the translator. `nxrs-build ... --target web --shaders-from <native data folder>` imports the shaders the native build's runs translated (Build for web passes it); the web live translator names the cause in a red line. Chrome 158: title menu, then the hospital scenes and Yuuki at the desk, the same as the native sheet at the same steps (`build/WEB/W7/run7-validate`); 0 red lines; audio running, 0 underruns (`run8-measure`); 55.6 FPS mean over 60-230 s (42.8-59.8 by 5 s window; replay 1-11 ms; another wave's native game running), below the 60 goal. **Run-time shaders on request, `WEB-W7b`:** a shader the web library lacks is POSTed to the page's server (`/<slug>/shader-xlate/<key>`), which runs `nxrs-build shader-xlate` (the native live translator, `translate_one`) into `<data>\shaders\live\<key>\` and answers with the SPIR-V; the web live translator waits for it on its own threads, a warm page reads `live/` first. Beach Bounce built without `--shaders-from`: 16 translated cold at 0.12-0.20 s each, byte-identical to the native live records, title and story scenes as run7, 0 red lines, 0 skipped draws; warm: 0 translated (`build/WEB/W7b`). `dev_server.py` has the same route; a static host keeps the red line. Android Chrome: not tried (no device) |
| W10 | Mario Kart 8 Deluxe in Chrome (stale library; heap region) | `src/cli/src/cc.rs`, `pipeline.rs`, `src/translator/src/loader/layout.rs` (`WEB_HOST_PROTOCOL`), `src/os/src/web/mod.rs`, `src/runtime/src/mem.rs` (`compact_regions`), `src/os/src/boot.rs` | In progress (title logo) | `WEB-W10-MK8D` (`docs/waves/WEB-W10-MK8D.md`, `build/WEB/W10/mk8d`): the owner's build died 1 s in because nxrs-build linked a `libnxos.a` from before the I/O worker (nothing rebuilt it after merges). nxrs-build now refreshes a dev tree's library (`web.ps1 -Crates os`) and refuses one without its page protocol mark. Then MK8D's 3.03 GB SetHeapSize failed in the compact map's 512 MiB heap region: the block is now split by virtual address memory (heap of the pool for titles without it). MK8D reaches its title logo at 60 FPS; it stays there: WebGPU refuses non-multiple-of-4 BC textures, `rgb10a2unorm` views of `rgba8unorm`, a perspective/linear interpolation mismatch and `A2B10G10R10_SNORM` vertices (16281 errors, command buffers invalid) |
| W10b | MK8D's WebGPU validation errors | `src/web/gpu-worker.js`, `src/shaders/src/wgsl.rs`, `src/graphics/src/web/{icd,submit,fmt}.rs` | In progress (title logo) | `WEB-W10b` (`docs/waves/WEB-W10b.md`, `build/WEB/W10b`): BC textures not whole blocks become a decoded stand-in at the logical size fed by a GPU decode pass from a block-rounded staging texture; views in a format WebGPU cannot view get a shadow texture kept bit-equal by reinterpreting passes; vertex outputs take the fragment stage's interpolation (a vertex module variant); A2B10G10R10 SNORM/UINT/SINT/SCALED attributes fed as `uint32` and unpacked in a variant. All four causes gone; MK8D still on its logo: next blocker a pass sampling its own depth32float attachment (needs `depthReadOnly`), 14,940 errors by 157 s. Beach Bounce (rebuilt) to the story scenes, 59.3 FPS, 0 errors; Anime Girls not re-run |
| W11 | Module size | `src/profiles/build.rs`, `src/cli/src/websplit.rs` (`fold`), `pipeline.rs`, launcher `services/web_serve.rs` (`played_web_dir`) | Complete (Beach Bounce) | `WEB-W11` (`docs/waves/WEB-W11.md`, `build/WEB/W11`): Beach Bounce **138.1 -> 103.8 MiB, gzip 50.6 -> 25.2 MiB**. 26.2 MiB of the module was every profile's nxnative shader tables (`shaders.tsv.zst`, build-time data, already compressed): left out of the emscripten engine; then `wasm-opt --duplicate-function-elimination` (28 s, `NXRS_WEB_FOLD=0` off). Measured, not kept: unreachable functions at translation (3 functions; 3% with only imported symbols as roots, but by-name runtime calls would be interpreted), `-O1`/`-O2`/`-Os` (5-7% gzip for 3-6 min), `--merge-similar-functions` (5% gzip, forwarding calls), game code at `-Os` (4%). Chrome compile is lazy (0.1-0.3 s any variant); page compile+instantiate 0.45 s. Chrome on GPU 1: title, hospital, Yuuki at the desk as run7, 0 red lines, 53.3 FPS mean (shared machine; run8 55.6). Launcher found no web build (folder named by the CLI's default, not the entry's output name): folder renamed and rebuilt; Play finds a build named otherwise by `build.json`'s title id. Anime Girls not rebuilt (W8's title) |
| W8b | Anime Girls FPS continued (game threads, GPU worker) | `src/translator/runtime/simd_fp.h`, `src/cli/src/cc.rs` (`-mrelaxed-simd`), `src/graphics/src/web/submit.rs` (`BG_CACHE_MAX`), `src/cli/tests/web_fp.rs` | In progress (60 not reached) | `WEB-W8b` (`docs/waves/WEB-W8b.md`, `build/WEB/W8b`): beach 43.2 FPS final (W8 40.7; 47-58 in quieter runs). Kept: FZ fast path + relaxed-madd FFMA (job threads' FP ~4 -> ~1 ms a frame, bit-identical: 11.6 M results checked), bind group cache 50,000 -> 4,096 (GPU worker GC 7.9 -> 6.6 ms a frame). Measured, no lever: memory64 bounds checks (already trap-handled), setjmp owner (0.4%). Open: main thread guest code 18-27 ms a frame (V8 code quality); GPU worker ~165 full GCs a second because V8 counts the ~9.4 GB shared linear memory as its external memory |
| W8c | Anime Girls FPS continued (GPU worker GC, main thread) | `src/web/boot.js`, `gpu.js`, `gpu-worker.js`, `src/graphics/src/vk/queue.rs` (web-only red line) | In progress (60 not reached) | `WEB-W8c` (`docs/waves/WEB-W8c.md`, `build/WEB/W8c`): **GPU worker GC 221.7 -> 0.1 full collections a second** (187 -> 1.4 ms a second): V8 counts a shared memory's length when an isolate receives it (later growth not counted; stand-alone page: >= 8 GiB at hand-over collects continuously), so the page hands the GPU worker the memory at instantiation (256 MiB) instead of at `nxgfx_web_start` (~9.4 GB). Beach 51.7 FPS final (46.5-53.8 over the wave's runs on a shared machine; W8b 43.2). Measured, not kept: wasm-opt -O2 on the whole module (mean 53.1 vs 50.5, inside the spread; 304 s a build). Not on the critical path: lanes' pools lock (replay thread waits 0 ms). Open: main thread guest code 17.6 ms a frame; one silent abort (GPU completion budget, `after3`), now named by a red line on the web |
| W8e | Anime Girls: the level-load GPU completion abort; the main thread's waits | `src/web/gpu-worker.js`, `gpu-worker.test.js` | In progress (60 not reached) | `WEB-W8e` (`docs/waves/WEB-W8e.md`, `build/WEB/W8e`): **the abort's cause**: Chrome compiles a pipeline in its GPU process after `createRenderPipeline` returns, ahead of the next submission and after the worker stored its "queued" word (the engine's 5 s GPU budget): cold run `l1` 12 submissions 1.0-1.9 s late at the level load with the GPU process's one thread at 100% and GPU 1 at 0%. **Fix**: async pipeline creation, the worker waits for them before the next submission (compile before the hand-over, as a native `vkCreate*Pipelines`); budget unchanged. Cold `c1`: 0 slow submissions, the level's compiles spread over ~7 GPU-process threads (4.25 s CPU in one second), frames after the load 57.5 vs 31.3 FPS. Waits: main thread 80.4% running, SleepThread 1.8 ms and kernel waits 1.9 ms a frame from the game's own job-wait loop (`f_1d0858ab0`), fence 0.45, core 0.37: no web-only wait to remove; exact busy (`QueryThreadCycleTime`) 76-83% in six runs, ~15-16 ms CPU a frame. Beach 95-135 s: base 34.7/54.8/50.0, new 56.5/49.8/39.1 (shared machine) |
| W8d | Anime Girls FPS: why the game code is ~3x slower as wasm | none kept (evidence and tools under `build/WEB/W8d`) | In progress (60 not reached) | `WEB-W8d` (`docs/waves/WEB-W8d.md`): the premise does not hold per shape. Microbenchmark of gen.py's shapes, native clang -O1 vs emcc -O2 wasm64 under V8: guest register/memory traffic 1.37 vs 1.80 ns (1.3x), guest calls 1.07x, scalar FP through `c->v` faster as wasm, FZ fast path +10%, memory64 vs memory32 4%. Main-thread profile with names: flat (top function 1.4%, guest 69% of the window, futex wait 19.5%), and V8's sampler counts blocked/descheduled time as busy: W8c's 17.6 ms is wall time against native's 5.7 ms CPU time. Real per-thread CPU time (`runs/cpu1`, 54.6 FPS): the busiest renderer thread 59.5% busy, ~10.9 ms CPU a frame (~1.9x native's all-work 5.7 ms); no thread above 60%. Named cause: the main thread's frame is ~40% waiting (futex waits, sleeps, GPU fence), not 3x slower code; next a per-thread wait timeline. The level-load GPU completion abort reproduced and is named by W8c's red line (submission not complete in 5 s) |
| PADBRIDGE | Pad rumble and lights through the launcher | `src/web/pad-bridge.js`, `input.js`, launcher `services/pad_bridge.rs`, `model/ws.rs`, `web_reply.rs`, `src/os/src/input/bridge.rs`, `input/mod.rs`, `src/cli/src/main.rs` (`pad-bridge`), `docs/LAUNCHER_CONTRACT.md` | Complete (owner felt rumble and saw the lights, DualSense USB, 2026-11-27) | `WEB-PADBRIDGE` (`docs/waves/WEB-PADBRIDGE.md`, `build/WEB/PADBRIDGE`): a page the launcher serves sends `pad <active> <low> <high> x4` over a WebSocket (`/<slug>/pad-bridge`) at each change and every 250 ms; the launcher writes each line to a child `nxrs-build pad-bridge`, which drives the pads with the native input layer (slots, player LEDs, lightbar, output reports, XInput, Nintendo standard rumble); 1 s watchdog, EOF quiets the pads; Bluetooth Sony and Switch 2 BLE pads left to the browser. No browser permission. Beach Bounce, Play from the branch launcher, Chrome on GPU 1, self-test: the owner's DualSense (USB) got `02 03 14 00 80 ..` (low 128) then `.. 80 00 ..` (high 128) then stop, player 1 LED + blue lightbar while active, lights off on menu open and on tab close, every write completed. No bridge (dev server, static host): the page's vibrationActuator / WebHID paths as before |

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
| **W8 Picture and FPS** | Sharp picture at the native build's FPS | Canvas backing store at the displayed size × devicePixelRatio (today a fixed 1280×720 stretched to the window), the game's own output resolution presented 1:1 where it fits; where the frame time goes (engine replay, ring, GPU worker, GPU) measured per frame and the largest costs cut; Anime Girls beach at 60 FPS in Chrome on GPU 1, measured with nothing else running |
| **W9 Saves, loading, settings** | Saves, loading and settings for web builds in the game page itself: an in-game menu over the game in Chrome, settings and saves stored in the browser (owner 2026-11-27: no separate web launcher page, not in the desktop launcher) | Saves kept and a game-written save shown surviving a reload; loading with real progress (bytes and phases) instead of a notice line; settings read by the engine at start: graphics (render scale / output resolution, FPS cap, vsync), audio (master volume, mute), controller (layout: A/B position, remap keyboard and gamepad buttons, dead zone). **Design reviewed by the owner before it is built** |
| **W10 Library in Chrome** | Every library title tried in Chrome | Each title built for the web and run to the nxrs bar (past the intro into first gameplay) on GPU 1, one at a time; results in `docs/34-LIBRARY-VALIDATION.md` beside the native column; failures named |
| **W11 Module size** | A smaller, faster-loading `game.wasm` | Measured where the 450 MiB (107 MiB gzip) goes; unreachable guest functions dropped at translation, wasm-opt passes that pay, data/table sections trimmed; size, compile time and FPS before/after, the game still reaching first gameplay |

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
