# OOT-LOAD — Ocarina of Time: the device loss when the house autosave loads; `oot-saved` as the quick scenario

Owner: one delegated agent. Lead: main session. Opened 2026-11-28, worktree `D:\Projects\RecompLauncherEngine-ootload`,
branch `ootload` (base `gameplay-harness` `eb21946`). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: "The Legend of
Zelda Ocarina of Time" only, GPU 0 (`--env NXGFX_GPU=0` on every run). Deadline 4 h.

## Evidence

`D:\Projects\RecompLauncherEngine\build\play\20261128-083853-oot-saved-probe`: Continue at ~29 s, three A presses
(Continue -> Load list -> Load) start the load of the "Kokiri Forest" autosave; the screen goes black at ~42 s; one
replay frame (2003) takes 3.75 s (4,867 draws, ~1,000 images made and uploaded, 8 descriptor-buffer chunks of 4 MB
made between 47.9 and 50.6 s); the device is lost at 53.5 s (exit 71). The GPU had started a compute dispatch of 80x60x1
groups (compute shader `f8965bffbdf51d78`) and finished everything before it. The driver's fault report: an invalid
read at page `0x539651000`, 0x1000 above the end of the newest 4 MB chunk (`0x539250000 + 0x400000`); no recorded
buffer was ever made or freed over that page, and no kept stage set has a binding over it.

## Plan (written first)

1. **Diagnose with diagnostics added, never the same path unchanged** (protocol §4, device loss):
   - the loss report also names the buffers made *above* the faulting page (today it lists only those below), so a
     read that runs below the start of a buffer (a negative index) is seen as well as one past an end;
   - a ring of descriptor-buffer bindings (the stage-set chunk and the pool set's buffer each `vkCmdBindDescriptorBuffersEXT`
     binds, with their sizes) and the dispatch's own compute set (today only mesh draws are recorded), so the report
     says which buffers the faulting dispatch read its descriptors from and where its set lies in them;
   - a CPU-side check where each set's offset is handed to the GPU: the offset plus the set's size lies inside the
     bound chunk, and the bound chunk is the one the set was written in; a failure names the stage, the shader and the
     offsets (red line), so an out-of-range descriptor offset is caught before the GPU reads it.
   Then one `oot-saved` run on GPU 0 with these on.
2. **Fix the cause in source** (indexing or allocation; no raised limit, no skipped draws), with a unit test that fails
   without the fix.
3. **`oot-saved`**: find the presses from the title to Link under control in his house; shots dense enough to see the
   load; a standing window (Link still, as `oot-still`'s h2-h7) and a walk with `replay oot_house_walk.txt`; route
   checks; references with `py scripts/play.py checkpoint <run>` from a run whose sheets were read. Target ~3-4 min.
4. **Verify:** two `oot-saved` runs load and play with no device loss; one full `oot` run (window [845, 915]) against
   `20261128-044556-r2-base` (~44.9/30 walk) for regressions; every sheet read; `cargo test --release -p graphics`;
   `py -m unittest scripts.test_play`.

Exclusions: Fault A (the white house floor); live native NDVE stays off.

## Files expected

- `src/graphics/src/vk/lost_ring.rs` (bind ring, report above the fault), `src/graphics/src/vk/exec.rs` (bind
  records, the offset check, the fix), possibly `src/graphics/src/vk/queue.rs` / `tables.rs` (descriptor-buffer
  allocation) and `lost.rs`.
- `scripts/scenarios.toml` (`oot-saved`), `scripts/checkpoints.json` (its references), possibly `scripts/play.py` /
  `scripts/test_play.py`.
- This document (results and report).
