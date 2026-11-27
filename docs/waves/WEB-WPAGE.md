# WEB-WPAGE - the page is the game; the black canvas

**Owner (2026-11-27), playing Anime Girls Sun of a Beach (0100C6801E0FA000) in Chrome from `nxrs.exe --serve-web`:**
"why is it logging on the page instead of to the console", and the canvas stayed black while the GPU worker reported
~59 FPS presents, 0 WebGPU errors, ~96 draws / 68 passes per frame (frame 16,800, ~5 min in).

## Plan

1. **Page.** The canvas fills the window at the game's aspect (letterboxed, black); no log, no status bar. The
   engine's log goes to the console only (and the dev server's `/log`, as now). `window.NXRS_LOG` stays as a bounded
   ring (last 5,000 lines) for drivers. A small overlay only for what the player must see: boot failure, not
   cross-origin isolated, no WebGPU / memory64, a fatal error, and the start-audio hint if sound waits for a click.
   The title is the game's name from `build.json`.
2. **Black canvas.** Reproduce from the current library build on port 8791 with this worktree's `nxrs.exe`, a CDP
   driver (from `build/WEB/WMEM/tools/drive.mjs`) screenshotting the canvas at intervals, with and without key
   presses, and compare with WMEM's `fix-cold` run (which pressed X every 6 s from 40 s). Candidates: the present
   path, layout hiding/resizing the canvas, a state difference (empty OPFS saves), a black screen waiting for input,
   a regression from a merged engine wave.
3. Before/after FPS (the GPU worker's `frames:` line) and whether the DOM log made CDP screenshots time out.

## Expected files

- `src/web/index.html`, `src/web/boot.js`, `src/web/gpu.js`: the page layout, the console-only log, the overlay.
- whatever the black-canvas cause names.
- Drivers under `build/WEB/*/tools` read `window.NXRS_LOG` or console lines (checked; updated if they read the page).

## Results (2026-11-27)

Evidence: `build/WEB/WPAGE/` (runs `base-nokeys`, `after-page`, `repro`, `bisect-*`, `after-fix`; driver
`tools/drive.mjs`, which now requires `--luid=<high>,<low>`: Chrome's WebGPU on GPU 1, PCI bus 4, LUID 0,113326).

**Page.** `index.html`/`boot.js`/`gpu.js`: the canvas is the page (as large as the window at 16:9, centred on black);
the engine's log goes to the console (and the dev server's `/log`) only; `window.NXRS_LOG` is a ring of the last
5,000 lines; one notice line for loading, "Click or press a key for sound" (cleared by the first click or key) and
failures (not cross-origin isolated, no WebGPU, no memory64, the game stopped or exited, a lost GPU device: these
stay, centred, red); Emscripten's "Running..." is a log line; the title is `build.json`'s. Page elements: 14 for the
whole run (before: +3 a second, 39 -> 1,000+ divs in 5 minutes). No driver under `build/WEB/*/tools` read the page
text or `NXRS_LOG` (they read console lines), so none needed changing. Screenshot times are unchanged by it (the first
capture at 20 s takes 2.6-4.8 s in both, during boot; later ones 110-460 ms) and the FPS cannot show a difference:
the old page ran at the 60 FPS cap on the menu as the new one does.

**Black canvas: not the page, not lean presents.** The picture stood still (the Halva Studio splash, or black at a
fade) while the game drew its menu (95 draws, 69 passes, 1728x972 scene passes: the WMEM menu's numbers). A run
with `NXGFX_PRESENT_LEAN=0` (`after-page`) stood still too. Switches of the MKW-SETS merge (`464633e`) off together:
the menu appears; `NXGFX_IMG_DUE`/`NXGFX_IMG_SPANS` off: still; `NXGFX_LANE_CACHES=0` alone: the menu. Cause:
MKW-SETS' lane caches (commit in `464633e`) let `mem::resolve` answer a plain pool's range from a copy of the pools
(`PLAIN`) without the pools' lock. On the web a pool compared by use is found changed only in `resolve`'s locked path
(`web_sync`); the copy answered for every pool after the sync's first slow resolve, so the CPU's writes (uniforms:
the splash's fade) never reached the GPU. Fix (`mem.rs`): a pool compared by use is never plain. After: menu at
40 s, beach gameplay after the keys (`after-fix`), 0 WebGPU errors, no device lost.

**Invalid submission.** "No bind group set at group index 0 ... DrawIndexed" then "[Invalid CommandBuffer]" every
~110 s: the web bind-group cache (`submit.rs`) destroyed its groups at 50,000 while the submission being resolved had
already bound some; the destroy went into the ring before that submission. Now the dropped groups are destroyed after
the submission is in the ring. 0 WebGPU errors in the 150 s and 160 s runs after.

**GPU hang (DXGI_ERROR_DEVICE_HUNG, machine lock).** Both were in `NXGFX_PRESENT_LEAN=0` runs on GPU 0 (the
display card), one of them with a second game Chrome open. Not reproduced on GPU 1 (`repro`, `?subtrace=1`, 150 s;
`after-fix` 160 s). The web loop bound is now 4096 back-edges a call (native 65536): 65536 for every fragment of a
1728x972 pass is ~1.1e11 iterations, a certain TDR if a loop runs away. `?subtrace=1` logs each submission (draws,
largest, pipelines) to the dev server every 250 ms and, at a lost device, the last six submissions' modules and the
WGSL of any with a loop.
