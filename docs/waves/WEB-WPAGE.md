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

## Results

(filled in at the end)
