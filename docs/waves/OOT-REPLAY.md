# OOT-REPLAY — Ocarina of Time: cut the replay thread's own work (toward 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 00:41, worktree
`D:\Projects\RecompLauncherEngine-ootbox`, branch `ootbox` (base `ootocc` `c32f047`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1 (MKW-AHEAD has MKW on GPU 0). Deadline 4 h.

## Goal and starting point

OOT-OCC removed the presents' wait for the replay (`NXGFX_OCCL_LAG`). The frame is now the replay thread's own work:
busy 31-35 ms a frame in Link's house standing (28.4 FPS over [600, 830]), 45-67 ms on the walk [845, 915] (~16 FPS).
59 needs under ~15.8 ms. OoT makes 1,300-2,140 occlusion box tests a frame (reset, one box draw, report each).

## Plan (written first)

1. **Profile** (no behaviour change): host sampler (`NXRS_HOST_PROF`, stacks, PDB) of the replay thread and the
   lanes over the walk [855, 880] s of an `oot` run on the base build; the house from a second sample. Split the busy
   time by function; name the top costs with numbers (below, before any code).
2. **Cut the biggest**, behind switches (0 = old): the box tests if they show (state churn per test, per-draw set
   and dynamic-state work of the box draws), and whatever else the profile puts on top. MKW-AHEAD's translate-ahead
   (lanes) is not duplicated here.
3. **The 391 MB allocation failure** (`pcache::save_now`): why one allocation, and why OoT holds ~32.5 GB; fix if
   small.
4. **Verify**: `oot` and `oot-house --faults`, route checks, every sheet; same-build switch-off controls back to back;
   FPS / 5 % low per window, replay busy ms, shown-vsync split; `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/exec.rs`, `tables.rs`, `pcache.rs`, `docs/env_switches.tsv`, this document.

## 1. Profile (written 01:00, before any change was run)

Run `build/play/20261127-004205-ob-prof-walk` (base build `c32f047` = OOT-OCC v5's, PDB beside the DLL; GPU 1;
MKW-AHEAD's MKW on GPU 0 beside it): host sampler over [855, 880] s (the walk), the replay thread and the five lanes
with unwound stacks, 26,898 samples each (the sampler takes the six threads in a fixed order, so a sample's thread is
its index modulo 6). Symbolized with `llvm-symbolizer` against the PDB. Shares are of each thread's wall time.

**Replay thread** (inclusive): idle in the replay queue 20.1 %; **waiting at `par::run` joins for the lanes 11.8 %**;
translating (`record`, its own segments and serial streams) 36.1 %, of it `bind_sets` 28.0 % = `stage_set` 12.0 %
(texture views `tex_view` 8.0 %, of which **5.0 % waiting for the tables' lock in `Tables::tex_at`**, samplers'
`samp_at` 1.8 % the same lock), **`Tables::snapshot` 6.4 % (of it 4.5 % `desc_buffer`: a new 10 MB host-visible
descriptor buffer allocated and the whole pool shadow copied in, under the tables' lock)**, `patch_set` 6.0 %; pool
sync and image prep (`begin_submit_ex`) 15.7 % (prehash 4.4, write-watch take 3.0, upload skip 2.2, image walk 5.5);
presents 3.3 % (1.6 % of it the harness's screenshots); contended locks 15.8 % in all.

**Lanes** (each): idle 82.7 %; translating 17.2 %, of it `bind_sets` 14.7 % = `stage_set` 9.1 % (`tex_at` waiting 3.1 %,
`placeholder_at` 1.4 %), **`snapshot` 3.7 % (`desc_buffer` 2.3 %)**, `patch_set` 3.1 %.

**Top costs, named:** (1) texture pool snapshots: 6.4 % of the replay thread plus 3.7 % of each lane, nearly all of
it new 10 MB descriptor buffers allocated and filled under the tables' lock, which then stalls every texture and
sampler lookup (`tex_at` / `samp_at` / `placeholder_at`: 7.5 % of the replay thread, ~5 % of each lane). The log
says the same: in OOT-OCC v5's walk, slow frames summed **11.4 s of snapshot time in [845, 884]** (frames of 200-400
ms with 45-59 snapshots made), 4.0 s over the house [600, 830]; in this profiled run 38 s over [600, 830].
`tables.rs` keeps at most 32 snapshot buffers for reuse; OoT makes 40-60 snapshots a frame (each texture registration
moves the pool generation), held 1-2 frames until their submissions complete, so the 33rd onward was a fresh
`vkAllocateMemory` + 10 MB copy. (2) The pool sync, 15.7 % (USE-SYNC's ground, not this wave's). (3) Lane joins
11.8 % (MKW-AHEAD's ground). **The box tests themselves do not show as a cost of their own**: a reset and a report
are an `end_query` / `begin_query` and a `Post` each; the box draw is an ordinary draw whose cost is the set work
above (OoT's ~1,600 tests are most of its ~2,000 draws a frame, so they carry most of `bind_sets`).

## 2. Changes

1. **Snapshot buffers kept for reuse: `NXGFX_SNAP_KEEP` (default 256; 32 = as before)** (`tables.rs` `recycle_keep`).
   A snapshot buffer the GPU is done with goes back to the shelf up to that count; a new snapshot is a recycled buffer
   brought up to date with the descriptor writes since its generation (`refresh`, unchanged), not a new allocation.
   The shelf holds only what was once in flight (64 buffers made in a whole `oot` run with it, see below). A log line
   counts the buffers made new (at powers of two).
2. **Pipeline-cache save without an aborting allocation: `NXGFX_PCACHE_TRY_ALLOC` (default 1; 0 = ash's, as before)**
   (`pcache.rs` `cache_data`). ash's `get_pipeline_cache_data` allocates the cache's size with `Vec::with_capacity`;
   when that failed under memory pressure the process aborted (OOT-OCC v5 at 884 s: "memory allocation of 391528417
   bytes failed" = OoT's `pipeline-cache.bin`). The size is asked, the buffer reserved with `try_reserve_exact`, and
   a failure skips that save with a red line (the file keeps the previous save; tried again after the next quiet
   stretch).
