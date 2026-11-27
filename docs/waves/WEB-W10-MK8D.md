# WEB-W10-MK8D: Mario Kart 8 Deluxe's web build dies 1 s after start

Branch `web-w10-mk8d` (from `gameplay-harness` 7a4b2bf). Title: Mario Kart 8 Deluxe (0100152000022000, NSP with
update 4.0.1 and DLC), web folder `P:\Games\Nintendo\DUMPS\Mario Kart 8 Deluxe\nxrs-web\Mario Kart 8 Deluxe 1G1U1D`.
Chrome on GPU 1 only (`--use-adapter-luid=0,113326`).

## Symptom (lead, build/WEB/W10/mk8d-1)

```
[nxrs] [web] 453 host files (23536 KiB) in /data; game data streamed by the I/O worker (opfs cache); 0.98 s
[nxrs] [os    0.003] unknown title (0100000000001000): pool 0xcd500000 ...
[nxrs] [nxrt] FATAL: /data/exefs/main: No such file or directory (os error 44)
```

No `web: data manifest ...` line and no `[RED] web: no data manifest` line: `web::io::manifest` never ran.

## Plan

1. Evidence for why `webdata::read` never reached the I/O worker.
2. Fix the cause in source, with a test.
3. Rebuild what is needed into the same web folder (launcher CLI / nxrs-build, below normal priority), run it in
   Chrome on GPU 1 with `build/WEB/W8e/tools/drive.mjs`, and take MK8D as far as it goes.

Expected files: `src/cli/src/cc.rs`, `src/cli/src/pipeline.rs`, `src/os/src/web/mod.rs`,
`src/translator/src/loader/layout.rs`, this document, `docs/35-WEB-TARGET-PLAN.md` (W10 row).

## Results

### 1. Cause of the 1 s death: a stale web engine library (diagnosis)

The owner's build linked `D:\Projects\RecompLauncherEngine\target\wasm64-unknown-emscripten\release\libnxos.a`
(the build log: `with ...\libnxos.a (nxweb_main)`), found by `cc::find_web_lib`'s development-tree fallback. That
file was dated 2026-11-26 17:41; WEB-W3's I/O worker (`a40f08d`, `web/io.rs`, `webdata.rs`) was committed at 18:23
that day. Its bytes hold `unknown title` but not `data manifest`, `I/O worker table` or `streamed by the I/O worker`:
the library has no `web::io` at all, so `webdata::read` never existed in it and boot read `/data/exefs/main` with
std::fs, while the page (host files embedded in the current nxrs-build) no longer puts game data in `/data`. Nothing
rebuilt the library when the later web waves were merged (`build.ps1` neither builds nor ships it), and nothing
checked that the library matches the page. Anime Girls / Beach Bounce worked because their modules were linked in
wave worktrees with fresh libraries.

### 2. Correction

- `translator::loader::layout::WEB_HOST_PROTOCOL` (`nxrs-web-host-protocol=2;`): the page interface number; the web
  library carries it as bytes (`os::web::NXWEB_HOST_PROTOCOL`, `#[used]`).
- `nxrs-build --target web` (`pipeline.rs`, `cc.rs`): a library from a development tree is first brought up to date
  by that tree's `scripts/build/web.ps1 -Crates os` (cargo decides staleness; 15-19 s when current); then every
  library (dev, shipped `web\libnxos.a`, `NXRS_WEB_LIB`) must carry this nxrs-build's protocol mark or the build
  stops with a message naming the fix. A library from before the mark is refused ("older than the I/O worker").

### 3. Next blocker found and fixed: the compact map's 512 MiB heap region

With a current library MK8D reached `nnMain` and died at once (`run-heap`):
`SetHeapSize 0xc1c00000: larger than the heap region 0x20000000 (ResultOutOfMemory)`, then `memory access out of
bounds`. MK8D has no virtual address memory (NPDM system resource 0): it sets a heap of about its pool. The web's
compact map had a fixed 512 MiB heap and 6 GiB alias (sized for Anime Girls, which maps its memory in the alias
region). Correction (`runtime/src/mem.rs` `compact_regions`, `os/src/boot.rs`): the os layer tells the runtime the
pool and whether the title has virtual address memory before `Space::new`; the same block below the modules is then
split as a heap region of the pool (256 MiB-aligned, here 3.25 GiB) plus 3.25 GiB alias for a title without it, and
unchanged (512 MiB + 6 GiB) for one with it. The block's size and bottom are unchanged, so the linear memory is too.

### 4. MK8D in Chrome (GPU 1), `build/WEB/W10/mk8d/run-title`

Boot, RomFS from the I/O worker, saves, audio renderer, Mii database, NVN; the **Mario Kart 8 Deluxe title logo at
60.0 FPS** (t60/t100/t140). It does not go past the logo: t60 and t100 are byte-identical, and A presses (logged
`buttons 0x0001`) change nothing. **Next blocker: WebGPU validation errors (16281 by 157 s)** that invalidate whole
command buffers, so the game's later frames never reach the screen:

- block-compressed textures whose size is not a multiple of 4 (`bc4-r-unorm 80x45`, `bc5 74x74`, `bc7 170x105`, ...):
  WebGPU refuses them (Vulkan allows them); every view, bind group and command buffer using one is invalid after;
- `rgb10a2unorm` views of an `rgba8unorm` texture (not in `viewFormats`, and not view-compatible in WebGPU);
- a render pipeline whose vertex output 0 is `perspective` and fragment input 0 `linear`;
- `[RED] web: vertex format A2B10G10R10_SNORM_PACK32 (not a WebGPU vertex format): the draw is refused`.

These are the graphics web layer's (`src/graphics/src/web`, `src/web/gpu-worker.js`, the WGSL stage) and are not
fixed here.

### Files

`src/translator/src/loader/layout.rs`, `src/os/src/web/mod.rs`, `src/cli/src/cc.rs`, `src/cli/src/pipeline.rs`,
`src/runtime/src/mem.rs`, `src/os/src/boot.rs`, this document, `docs/35-WEB-TARGET-PLAN.md`.

### Notes

- MK8D's web folder was rebuilt twice with this branch's nxrs-build (same folder, CLI arguments as the launcher's);
  the owner's previous `game.js`/`game.wasm`/`build.json`/build log were backed up first (evidence keeps the log).
- `data.json`'s version changes on every build (`ac3adf..`, `d71cc4..`, `edbac6..`), so the browser's OPFS data cache
  starts empty after each rebuild even when the data did not change. Not fixed here.
- The main checkout's `target\wasm64-unknown-emscripten\release\libnxos.a` is still the stale one; the first web build
  with the merged nxrs-build from the main tree refreshes it (`web.ps1`, a full os build the first time).
