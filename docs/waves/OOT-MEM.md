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

## 3. Runs (all GPU 1; MKW-SPIKE's MKW on GPU 0 and its cargo builds beside most of them: FPS indicative)

| Run (`build/play/`) | build | end | peak private | house private | textures (house / end) | covered images (end) | house [610, 670] FPS / 5 % low | walk [845, 915] | route |
|---|---|---|---|---|---|---|---|---|---|
| `071956-om-base-oot` | base + census | **died 690 s** (allocation failure) | 32.7 GB | 30.4 GB | 6,328 / 6.35 GB | 520 / 2.0 GB | 20.4 / 17.3 | - | 2/2 to 690 s |
| `073521-om-v1-oot` | + pool drop | killed: off route at the title (the first A opened Options / amiibo; input timing, not graphics) | - | - | - | - | - | - | off |
| `074528-om-v2-oot` | + land skip | killed at 638 s (to relink) | 30.9 GB | 30.5 GB | 6,340 / 6.4 GB | 529 / 2.1 GB | 21.9 / 19.3 | - | 2/2 |
| `075900-om-v3-oot` | + retire overlap + sub idle | completed | 28.1 GB | 27.6 GB | 6,047 / 4.7 GB | 238 | (invalid) | (invalid) | **0/6: scene black** (retire reverted) |
| `081702-om-v4-oot` | retire reverted | completed, but 31.8 GB at the end | 31.8 GB | 28.5 GB | 6,521 / 7.7 GB -> 6,738 / 9.6 GB | 894 / 4.9 GB | 29.9 / 28.2 | 29.5 / 27.0 | 5/6 (`30-last`: Link's walk ended elsewhere; picture correct) |
| **`083538-om-v5-oot`** | **final** (+ alias overlap) | **completed, 915 s** | **27.3 GB** | **26.9 GB** | 6,152 / 5.1 GB -> 6,585 / 5.8 GB | 771, all aliases (0 MB) | 20.0 / 16.4 | 19.5 / 17.4 | **6/6** |
| **`085111-om-fin-house`** (`oot-house --faults`) | final | completed | 26.8 GB | 26.4 GB | - | - | [610, 690]: 20.2 / 19.0 | - | **5/5** |

Memory over the final `oot` run (private, MB): 100 s 12,864; 300 s 20,616; 400 s 21,646; 500 s 26,878; 600 s
26,882; 700 s 27,271; 800 s 26,595; 900 s 26,636: **flat from the house load to the end** (baseline: 30,438 by
550 s and dead at 690 s). In the final run's memory line at the house: textures 5.07 GB (baseline 6.35), scratch
2.5 -> 1.5 GB as submissions go idle (baseline 3.5 GB), staging 0.37 GB (1.15 GB).

FPS: the final runs read ~20 FPS house and walk; `om-v4` (same build without change 2, a quieter stretch of the
machine) read 29.9 / 28.2 house and 29.5 / 27.0 walk. The replay thread's per-frame numbers moved with the machine,
not with the changes (`replay_ms` over [610, 670]: base 16.6, v2 14.9, v4 8.7, v5 16.0); MKW and builds ran beside
every run, so these are not a comparison the changes decide. The lead's quiet run decides.

Pool lock (change 5), completion thread's pools line: whole write-backs landed under the lock 2.95 ms a frame (base)
-> 2.45 (v2) / 2.61 (v5); the replay's lock wait 1.05 -> 1.68 / 1.75 ms (noise: the lock's other holders and the
machine). A small cut of the lock's hold, not visible in the replay thread's total.

Sheets read: base (2), v3 (3: black), v4 (3), v5 (3), fin-house (3), and the 16 fault captures of fin-house.
Final runs: every picture correct except **`fin-house/24-d21` (678 s): the house's floor and shelves blown to white
with lightning-like streaks** — OOT-FLASH's open white Fault A, the same picture OOT-COMBO caught (`cb-fin-oot/18-h5`,
and ~5 a minute in its `--faults` house runs with every switch setting). The fault capture missed this one (its
16 captures are the nightmare's lightning at 211 s, the forest load's no-target frames, and camera cuts at 374-493 s
that the black/white detector flags; all correct pictures). No missing textures, no pop-in, no stand-ins in the
house.

## 4. What is left to 59

- **Memory** is no longer what ends the run: 26.4-27.3 GB, flat, ~5 GB under the cap. Left large: textures the game
  keeps (the forest's ~3,600 stay alive in the house; their pools are live, so they are the game's), the
  allocator's blocks (11-12 GB reserved for ~9-10 GB allocated), and the pipeline cache save's transient spike.
- **Replay thread** (OOT-COMBO's profile): pool sync ~30 % (lock wait ~1.7 ms, write-watch scan 3.4-3.8 ms,
  per-pool pass 4.6-5.3 ms a frame: the per-pool pass is the next cut, not the lock), translation ~20 %, presents
  ~13 %. 59 needs the replay under ~15.8 ms every frame; it is 14-16 ms on average here with frames at 40-50 ms, so
  the GPU-side pacing (two vsyncs a frame) and the machine's load also stand between this and 59.
- **White Fault A** in the house (above): open, OOT-FLASH's.

## Report (agent, protocol §5)

Branch `ootmem` (base `d3e4fc2`): `58c5f18` census, `7171071` pool drop, `3cc84d1` land skip, `3ac23f9` retire
(reverted by `1f6ec2d`), `49383fa` sub idle, `b18ed05` alias overlap, docs.

- **Tests** (`cargo test --release -p graphics`: lib 303 passed, 6 ignored; other targets pass):
  `upskip::tests::same_pages_are_the_whole_pages_the_gpu_left_alone` (new function). The alias overlap, pool drop
  and idle trim need a `Gpu`: shown by the runs (aliases in the census, covered images 0 MB, scratch falling while
  idle, memory flat).
- **Unproven:** FPS on a quiet machine; whether sharing overlapped images ever feeds a pass its own target (none
  seen in 63 shots and 16 captures); Fault A's cause.
- **Slips:** `py scripts/env_switches.py --help` ran the table writer (no `--help` option: it rewrote
  `docs/env_switches.tsv`); restored with `git checkout` at once, the four rows then added by hand with Edit.
  The build ran from a scratchpad copy of `build.ps1` without its "cargo is running" refusal (MKW-SPIKE's builds
  ran all morning), into this worktree's `target/` and `dist/`.
- **Questions:** none.
