# OOT-MEM — Ocarina of Time's image growth (the 32 GB cap), then the pool-lock wait

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 07:05, worktree
`D:\Projects\RecompLauncherEngine-ootmem`, branch `ootmem` (base `gameplay-harness` `d3e4fc2`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1 (MKW-SPIKE had MKW on GPU 0 and built in its
worktree during most runs). Deadline 4 h (11:05).

## Plan (written first)

1. **Census diagnostic** (`image.rs` `image_census`, logged with the memory line every 600 frames): images made by
   how (new, parked reuse, takeover, alias), released and destroyed; nvnTextureInitialize / Finalize / Initialize
   over a live object; and the live images whose storage a newer live image overlaps (the game made another texture
   over that memory without finalizing this one), grouped by format and size.
2. **Baseline** `oot` with the census on (GPU 1): memory and image lines over the run, FPS / 5 % low per window
   (house [600, 830], walk [845, 915]), replay busy, pool-sync split.
3. **Fix the cause at source**: free or reuse what the game abandoned or finalized; no caps.
4. **Pool-lock wait** behind write-back landings (completion thread), if time is left.
5. **Verify**: full `oot` to its end without the cap, `oot-house --faults`, route checks, every sheet;
   `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/image.rs`, `vk/perf.rs`, `nvn/tex.rs`, possibly `vk/queue.rs` / `vk/mem.rs`,
`docs/env_switches.tsv`, this document.

## 1. What the census showed (baseline `20261127-071956-om-base-oot`)

The baseline died at **690 s** (`memory allocation of 1572864 bytes failed`, private 32.7 GB), in the house.

- The game finalizes almost every texture object it makes: by 662 s 373,526 initializations, 366,744 finalizations,
  372,328 images taken over by a newer object of the same description. The live images (6,328, 6.35 GB) are the
  ~6,800 objects it never finalizes.
- Memory pools are not the path: 4,385 mirror pools stay alive; only a few hundred are finalized in a run and they
  carry 0-3 images (`images released with their pool`). Images over no live pool: 0.
- Images step up at scene loads (forest ~330 s: 3,990 / 4.4 GB; house ~520 s: +2,300 / +1.8 GB), then **keep
  growing while Link moves**: the census's "live under a newer live image's storage" set is render targets the game
  places over overlapping parts of a transient heap, each placement a new object never finalized, of the **same
  format and size** as the ones it overlaps (`R16G16B16A16_SFLOAT 1280x720` x55 -> x161, `D32_SFLOAT 1280x720`,
  `B10G11R11 2560x1440`, `D32_SFLOAT 2560x1440`, ...): 520 images / 2.0 GB by the house, **894 / 4.9 GB by the end
  of the walk** (`om-v4`). Only ~30 % start at the same address as the image they overlap, so neither the takeover
  (same description and address) nor the alias (same address) caught them. All of them are still named by a
  texture-pool slot.
- Besides images: pooled submissions' scratch 3.5 GB (55 x 64 MB) and staging 1.15 GB in the house; the pipeline
  cache save (391 MB, plus the driver's serialization) spikes private memory at ~680 s.

## 2. Changes

1. **Census** (`image.rs` `image_census`, `perf.rs`, `nvn/tex.rs`, `mem.rs` `outside_pools`): the line above,
   every 600 frames beside the memory line.
2. **A new texture over part of a live GPU-drawn image of exactly its format and shape shares that image**
   (`alias_image`, `NXGFX_ALIAS_OVERLAP`, default on): the same-address alias (lane W2) extended to overlapping
   placements, for GPU-drawn owners of the identical Vulkan format and shape only. On the console those objects are
   the same memory; one image is what the console does (what one draws, the other reads), and the placements stop
   being new images.
3. **Pooled submissions idle for `NXGFX_SUB_IDLE` presents (300) give back staging and scratch** even inside the
   kept 64 / 16 (`queue.rs` `trim_free`): `pop` takes from the back, so most kept submissions sat idle with a 64 MB
   scratch chunk each.
4. **Images over a finalized memory pool (or one replaced at the same base) are released with it**
   (`drop_pool_images`, `NXGFX_POOL_DROP_IMAGES`, default on; `Images::drop_pool` was a no-op since the first
   commit). Console-correct, small for OoT (0-3 images a run).
5. **Pool lock: a landing skips every whole page `landing_pre` found unchanged** (`mem.rs` `apply_writebacks`,
   `upskip.rs` `LandingPre::same_pages`, `NXGFX_LAND_SKIP_SAME`, default on): before, only the re-armed ones were
   skipped and the rest were compared again under the pool lock.

Tried and taken out: **letting go of the overlapped images** (`NXGFX_RETIRE_OVERLAP`, `3ac23f9`, reverted in
`1f6ec2d`): parked, then destroyed after the park window; their pool slots got the zero stand-in and the scene went
black from the title on (`om-v3-oot`, all six route checks off route). The overlapped objects are still sampled
through their slots; sharing the image (change 2) keeps them valid.

RESULTS_PLACEHOLDER
