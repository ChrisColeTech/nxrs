# DIRECT-MEM — the GPU reads guest memory directly (no per-frame pool uploads)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~11:50, worktree
`D:\Projects\RecompLauncherEngine-direct`, branch `direct` (base `gameplay-harness` `f83eccf`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World (`mkw`), GPU 0; Ocarina of Time only when the lead says.
Deadline 8 h (design/feasibility 2 h).

## Goal and baseline

MKW presents at its 30 cap because the replay thread needs ~21 ms a frame (translation ~12 ms + prep ~9 ms; prep is
pool sync: lock wait ~1.1, write-watch scan ~3.5, per-pool pass ~1.8 ms, plus waits); OoT's pool sync is ~14-18 ms a
frame. nxnative (Metal, unified memory) has no upload step. Here every CPU-visible pool is a device-local **Mirror**:
CPU writes found by the write watch and uploaded at each submission (`mem::begin_submit`, `upskip`), GPU writes copied
back at the fence (`apply_writebacks`, `lazy`). Baseline (UPLOAD-SYNC v5, `20261126-055832-v5-mkw`): 30.4 / 29.5 FPS,
replay 12.0, prep 8.7, pool sync 6.5 ms, GPU 5.1 ms a frame, ~3,950 mirror pools (4.8 GB), 1.5 MB uploaded a frame,
completion write-back 4.4 ms a frame (under the pool lock, which the translation lanes wait on).

## Candidates (evaluated)

- **(a) Import the guest pages (VK_EXT_external_memory_host).** Already present as the `Host` placement
  (`NXGFX_POOL=host`, `mem::import`), never measured for speed. Facts: the device imports at 4 KiB alignment (run log:
  `host import true (align 0x1000)`); guest memory is `VirtualAlloc` reservations with `MEM_WRITE_WATCH`, the heap
  committed at SetHeapSize (lazy heap is opt-in, `NXRT_LAZY_HEAP`), so pool pages are committed when the pool is made;
  the imported memory type is host-visible and coherent, and every submission already ends with a
  MEMORY_WRITE -> HOST_READ barrier (`queue.rs`), so GPU writes are visible at the fence and CPU writes at submit. A
  Host pool needs no upload and no write-back; the write watch still runs for it (the texture check needs CPU-written
  texture bytes). Risks: one `vkAllocateMemory` per pool against NVIDIA's `maxMemoryAllocationCount` (4,096 on
  Windows) with ~4,000 pools in MKW; GPU reads over PCIe for heavily read buffers (vertex data); pinned host pages.
- **(b) Guest memory backed by host-visible VRAM (ReBAR).** Guest addresses are fixed host addresses chosen by the
  runtime's reservations; a Vulkan mapping cannot be placed at a given address without VK_EXT_map_memory_placed
  (not relied on here), and the game's own CPU reads of uncached write-combined VRAM would be slow. Rejected.
- **(c) Hybrid.** Direct (imported) for the pools where it pays, Mirror for the rest; follows from (a)'s measurements.

## Plan

1. Measure the existing `NXGFX_POOL=host` on MKW (feasibility: import failures, FPS, GPU time, pictures).
2. Make (a) scale: imports per *arena* (a run of committed guest memory) shared by every pool inside it (each pool a
   `VkBuffer` bound into the arena's memory at its offset), so the allocation count stays small; arenas reference
   counted, freed after use. New module `vk/direct.rs`; `mem.rs` only asks it at the placement decision and buffer
   creation (separable from `upsync2`).
3. Switch `NXGFX_DIRECT_MEM` (docs/env_switches.tsv): off = today's mirrors; on = CPU-visible pools imported.
4. Cut what Host pools still pay in the sync (per-pool pass work, completion write-backs, lock holds); keep the write
   watch for the texture check.
5. Verify: `cargo test --release -p graphics`; MKW off/on, FPS / 5% low vs 60, replay / prep ms, route checks, sheets,
   anomalies.

## Findings (feasibility, 2026-11-26 11:55-12:40; GPU 0; other waves' OoT runs on GPU 1 overlapped every run)

Window [250, 325] s of `mkw`; per-frame means from the frames CSV, pool parts from the `[vk] pools:` lines.

| Run (`build/play/`) | switch | FPS / 5% low | replay | prep | pool sync | lock / scan / pass | upload MB | GPU ms | route |
|---|---|---|---|---|---|---|---|---|---|
| `20261126-120317-dm-base-mkw` | off | 25.1 / 21.0 | 16.7 | 17.1 | 13.1 | 2.3 / 7.2 / 3.9 | 1.20 | 5.3 | on |
| `20261126-115723-dm-host0-mkw` | `NXGFX_POOL=host` (all) | 17.7 / 7.9 | 34.6 | 10.0 | 7.4 | 1.1 / 5.1 / 1.6 | 0.31 | 12.4 | on |
| `20261126-121340-dm-unc1-mkw` | uncached, scanned | 30.2 / 28.0 | 12.1 | 9.9 | 7.4 | 1.3 / 4.4 / 1.6 | 0.38 | 9.0 | on |
| `20261126-122556-dm-unc2-mkw` | uncached, untextured unscanned | 24.2 / 5.0 | 18.2 | 8.7 | 7.9 | 1.6 / 4.8 / 2.3 | 0.36 | 8.9 | on |

- **Importing works at scale.** The device imports at 4 KiB; 3,832 of ~3,900 CPU-visible pools imported (46 refused with
  `VK_ERROR_OUT_OF_DEVICE_MEMORY`, not overlapping any other import; they fall back to mirrors). The allocation-count
  limit did not bite.
- **GPU writes into an imported pool are not write-watch CPU writes** (new GPU test
  `gpu_writes_to_a_host_pool_are_not_cpu_writes`): the texture check is not fooled by them.
- **Reading pools over PCIe costs GPU time, and MKW reacts to GPU time.** All pools imported: GPU 5.3 -> 12.4 ms a frame;
  the game's dynamic resolution then made ~320 render targets a second (image 32,000 -> 44,000 in 37 s), each
  uploaded from storage: texture uploads 20-25 ms a frame, replay 34.6 ms. Only the CPU-uncached pools (~2,650, 193 MB):
  GPU 8-10 ms, the same churn later and milder (unc1 2,000 images in the window, unc2 4,000 from 295 s).
- **What direct pools remove.** Uploads 1.2 -> 0.3 MB a frame; with the untextured direct pools out of the scans and the
  per-pool pass (unc2), prep was 6.1-6.5 ms a frame in the first 45 s of the window (base 16-18, unc1 8.5-11; per-15 s
  medians), before the churn.
- (b) rejected (above); the size of the GPU cost points at (c): direct for the small, per-frame-rewritten pools only.
  Of the CPU-uncached pools, ~2,400 are 64 KB or less (40 MB, per-object constants) and a few are over 4 MB (140 MB,
  streamed geometry). Next: a size cap (`NXGFX_DIRECT_MAX_KB`, default 256).

| Run | switch | FPS / 5% low | replay | prep | pool sync | lock / scan / pass | GPU ms | route |
|---|---|---|---|---|---|---|---|---|
| `20261126-123433-dm-c256-mkw` | uncached <= 256 KB (48 MB) | 17.6 / 1.0 | 22.9 | 16.8 | 15.5 | 2.4 / 9.6 / 3.7 | 8.7 | on |
| `20261126-124012-dm-off2-mkw` | off (paired) | 30.1 / 28.0 | 12.8 | 11.7 | 8.8 | 1.7 / 4.5 / 2.5 | 5.1 | on |
| `20261126-124737-dm-gpu-mkw` | `gpu`: GPU_UNCACHED pools (7, 72 MB) | 29.1 / 25.0 | 13.8 | 13.1 | 10.1 | 1.8 / 5.3 / 2.9 | 4.8 | on |

- **The GPU cost is latency, not bandwidth.** 48 MB of small pools cost as much GPU time (+3.3 ms) as 193 MB of all the
  uncached ones, while 72 MB of GPU_UNCACHED pools (large, lightly read) cost nothing. The small pools are per-draw
  constants: every draw's constant fetches go over PCIe. (c256's CPU numbers are contaminated: its prep was 16-18 ms
  from the start, as in the loaded base run; GPU time is not.)
- **What the direct pools save on the CPU** (unc2 against the paired off run, 250-295 s, `[vk] pools:` lines): lock
  wait 1.65 -> 1.35 ms, scan ~4.4 -> ~3.4, per-pool pass 2.4 -> 1.65, the completion thread's pool-lock time 3.0 -> 1.4;
  prep 10.1-11.3 -> 6.1-6.5 ms (per-15 s medians). Replay + prep ~17.7 ms there: near the 16.7 a 60 FPS frame needs.
- **(b) is not possible here:** the driver has no `VK_EXT_map_memory_placed` (vulkaninfo), so device memory cannot be
  mapped at a guest address; and the runtime's reservations are plain `VirtualAlloc` regions (no placeholders).
- **Write-backs** (trace, `NXGFX_DIRECT_TRACE=1`): ~230 mirrors a frame, mostly 16 KB CPU-cached pools (0x24), ~10
  GPU writes each; making them direct would put their later GPU reads over PCIe as well.

## Decision (2026-11-26 ~13:05, inside the 2-hour design limit)

- Built: (a)+(c) as an opt-in placement, `NXGFX_DIRECT_MEM` (`1`: CPU-uncached pools up to `NXGFX_DIRECT_MAX_KB`, `gpu`:
  GPU_UNCACHED pools, `all`), with untextured direct pools out of the write watch. It removes ~40% of MKW's prep, as
  asked, but **in MKW it is a net FPS loss**: the pools whose sync costs the CPU are exactly the ones whose GPU reads are
  latency-bound over PCIe, GPU time rises 5 -> 8-9 ms, and MKW's dynamic resolution then makes render targets without
  end (each uploaded from storage), which costs far more than the sync saved. The pools that are cheap to read over
  PCIe (GPU_UNCACHED) save nothing. **Default stays off.**
- Last cheap lever tried: the host-cached import type (the pointer allows types 2 and 3; the import takes 2), in case
  the GPU caches those reads: `20261126-130024-dm-c256c-mkw` GPU 8.24 ms, the same as type 2 (8.2-8.7). Dropped.
- Where it should pay: **Ocarina of Time** (CPU-bound at ~16-19 FPS, GPU 9.5 ms of a 53 ms frame, per-pool pass 6.1 ms,
  write-backs ~1 MB / 9.9 ms a frame): GPU time to spare and no known dynamic resolution. Needs the lead's go for OoT.

## OoT (lead's go, 2026-11-26 13:20; `gameplay-harness` 288b07a merged into `direct` as 329d763, OoT relinked 13:33)

`oot`, GPU 0, window [845, 915] s (Link in his house), target 59. Breakdown: per-frame means from the frames CSV, pool
parts from the `[vk] pools:` lines in the window.

| Run (`build/play/`) | switch | FPS / 5% low | replay | prep | pool sync | lock / scan / pass | write-back MB (ms) | GPU ms | direct pools | route |
|---|---|---|---|---|---|---|---|---|---|---|
| `20261126-133352-dm-oot-off` | off | **18.7 / 17.0** | 17.5 | 25.2 | 20.7 | 2.4 / 11.1 / 6.6 | 1.01 (10.8) | 9.3 | - | 6/6 on |
| `20261126-134901-dm-oot-on1` | `1` | **18.8 / 17.1** | 17.4 | 25.3 | 21.0 | 2.7 / 11.0 / 6.8 | 1.05 (10.5) | 11.2 | 761 (21 MB) | 6/6 on |
| `20261126-140410-dm-oot-gpu` | `gpu` | **18.6 / 15.0** | 18.6 | 24.3 | 20.2 | 2.4 / 10.7 / 6.4 | 1.05 (10.8) | 9.5 | 7 (55 MB) | 6/6 on |

- **No change in OoT.** Its sync cost is the write-watch scan of its ~4,400 mirrored pools (4.4 GB, ~11 ms) and the
  per-pool pass over them (~6.5 ms); the pools the switch takes are a small part (21-55 MB) and the GPU's ~1 MB a frame of
  write-backs still lands in mirrors. `=1` costs +1.9 ms of GPU (per-draw reads over PCIe again), harmless at OoT's 53 ms
  frame but no gain. Sheets (all 3 of each run) correct: title, story, nightmare, house, forest load (black, as always),
  Deku Tree, Navi, Link woken, the house, every walk shot. Red lines: the known three (no-target draw, occlusion queries
  used up, crashed pipeline), plus one `host import failed ... mirrored instead` in `on1`.
- **Where OoT's GPU writes land** (`20261126-141919-dm-oot-trace`, `=1` + `NXGFX_DIRECT_TRACE=1`, house phase 543-607 s;
  the run's harness process exited at 590 s for an unknown reason, the orphaned game was stopped by PID, so it has no
  FPS window): 58 mirrors a frame, led by one **88 MB CPU-uncached pool** (`0x15af30000`, flags 0x22, ~66 write-backs
  and ~1.8 MB a frame), over `=1`'s 256 KB cap; then CPU-cached pools of 2.2 MB (two, ~2.5 x and ~5.6 MB of noted
  ranges a frame each) and 15 MB (~1.5 x, ~23 MB noted). Next run: `=1` with `NXGFX_DIRECT_MAX_KB=131072`, which takes
  the 88 MB pool direct (its write-backs gone).
- **`20261126-142943-dm-oot-big` (`=1`, cap 128 MB): GPU device lost at 480.5 s (exit 71), and the run had desynced**
  (sheet: still in the opening story at 340-440 s, title logo at 440 s, black at 472 s), so its 30 FPS stretch at 360-470 s
  is cutscenes, not a gain. The loss: at the forest load (479.2-479.3 s) ten imports were refused at once
  (`VK_ERROR_OUT_OF_DEVICE_MEMORY`), among them the 88 MB pool, which fell back to a granule-backed (sparse) mirror; the
  device was lost 1.2 s later, first seen at `vkQueueBindSparse`, the driver's report naming an invalid write at 0x0
  while the GPU ran texture deswizzles. Not re-run unchanged.
- `20261126-143950-dm-oot-big-diag` (same + `NXGFX_CHECKPOINTS=all`, `NXGFX_POOL_LOG=1`, stopped at 560 s): no loss;
  the same refusals came at its forest load (348.5 s, then 14 more to 444 s), the 88 MB pool mirrored from 348.7 s; prep
  19-22 ms at 360-470 s, as the default. The refusals cluster where the game makes many pools at once: consistent with a
  cap on pinned (imported) host memory, not proven. New with the checkpoints: `stale staging buffer ... bound by a
  descriptor in submission N (not its own)` from 448 s (not checked against a default run with the same switch).
- `20261126-144916-dm-oot-big2` (same configuration as `big` + `NXGFX_CHECKPOINTS=all`, full length): **no device loss**,
  6/6 on route, sheets correct; 14.9 / 11.3 FPS, not comparable (the per-draw checkpoints double the replay, 35.8 ms);
  16 import refusals again, the 88 MB pool among them (mirrored), write-backs still 0.82 MB a frame. So that pool cannot
  be made direct here: its import is refused whenever it is first used at the forest load.

**OoT conclusion.** No setting of the switch moves OoT's FPS (18.6-18.8 against 18.7 default, target 59): the pools it
can import are a small part of OoT's sync cost; the one pool that carries its GPU writes is refused by the driver. One
device loss (`dm-oot-big`, a desynced run) came right after a burst of refused imports and a large pool's fallback to a
sparse mirror; two later runs of that configuration (one full) did not repeat it. The switch stays off by default; the
large cap is unsafe until the refusals and that fallback are understood. A refused import should probably not fall back
mid-game to a sparse mirror; next step if this is pursued: log the import refusal's size and the process's pinned total
(`NXGFX_POOL_LOG=1`), and test whether the refusals follow a pinned-memory cap.

## Report (agent, protocol §5; 2026-11-26 11:50 - 13:30)

**Outcome.** The GPU reading guest memory directly works (imports at 4 KiB, coherent, GPU writes not taken for CPU
writes) and removes ~40% of MKW's prep, but it does **not** raise MKW's FPS: every selection that saves CPU puts per-draw
constants behind PCIe latency (+3.3-3.9 ms GPU a frame) and MKW's dynamic resolution then churns render targets. Kept
as an opt-in switch, **default off**; final build with the switch off is unchanged from the base (30.2 / 28.0).

### 1. Built

- Diagnosis (Findings above): import feasibility, the GPU cost and its latency nature, the dynamic-resolution churn it
  causes, the CPU saving, (b) impossible on this driver, where the GPU writes go.
- `vk/direct.rs` (new): the placement rule behind `NXGFX_DIRECT_MEM` (`1` CPU-uncached pools up to
  `NXGFX_DIRECT_MAX_KB`, `gpu` GPU-uncached pools, `all`); a registry of direct pools; **untextured direct pools are left
  out of every write-watch scan, walk and per-pool pass** (their CPU writes need nothing: the GPU reads them in place),
  and the first texture made over one (`image::ensure_image`, before the image reads its storage) resets that pool's own
  pages and drops its stale pages from the walker's stash under the watch lock, then the pool is asked as before (no
  CPU write older than the image's content is taken for a newer one). `NXGFX_DIRECT_TRACE=1`: write-backs by pool.
- `vk/mem.rs`: the placement decision asks `direct` (switch at the pool-backing decision); Host pools register at
  `pool_add` and leave at `release` / import fallback; one predicate (`asked`) for the scan, walker and per-pool pass
  lists; `with_watch_lock`; the import's memory type logged once; summary line part.
- `vk/image.rs`: one call (`direct::note_texture`) at the top of `ensure_image`.
- Also applies to `NXGFX_POOL=host` pools (the registry serves every Host pool), so that switch now leaves untextured
  imported pools out of the watch too.
- Separable from `upsync2`: `mem.rs` changes are a few lines at the placement decision, pool add/release, the import,
  and the scan lists; no `upskip.rs` / `lazy.rs` / `queue.rs` change.

### 2. Tests

- `cargo test --release -p graphics`: lib 253 passed, 6 ignored; integration tests pass.
- `direct::tests::only_cpu_uncached_pools_are_direct_by_default_on`: the placement rule per mode, flags and size cap.
- `direct::tests::a_direct_pool_is_asked_once_a_texture_is_made_over_it`: registry overlap lookup; a replaced pool's
  release does not drop the newer pool at the same base.
- `direct::tests::the_first_texture_drops_only_the_pools_stale_pages` (real write watch): the pool's stale bits reset and
  its stashed pages dropped, a page shared with a neighbour keeps its bit and stash entry, later writes are seen, a
  second texture changes nothing.
- GPU (`--features gpu-tests`): `gpu_writes_to_a_host_pool_are_not_cpu_writes` (new: a GPU copy into an imported,
  write-watched pool lands in guest memory and is not reported by `GetWriteWatch`); `compute_writes_buffer_visible_after_fence`
  (existing, mirror and host) passes.
- New functions; none could fail on the old code as written.

### 3. Verified (GPU 0; MKW `mkw`, window [250, 325] s; other waves' OoT runs on GPU 1 overlapped most runs)

| Run (`build/play/`) | switch | FPS / 5% low | replay | prep | pool sync | lock / scan / pass | GPU ms | route |
|---|---|---|---|---|---|---|---|---|
| `20261126-131020-dm-final-off-mkw` | off (final build) | **30.2 / 28.0** | 11.7 | 10.7 | 8.2 | 1.5 / 4.4 / 2.2 | 5.0 | on |
| `20261126-131604-dm-final-on-mkw` | `1` (final build) | **29.9 / 28.0** | 12.0 | 10.5 | 9.5 | 1.4 / 6.3 / 1.7 | 8.4 | on |

Earlier runs: tables in Findings. Target 60: not reached either way (the 30 cap holds). Sheets read: all 10 runs (boot,
title, menus, grid, race, lap 2 correct in each; black bush silhouettes at the grid appear in the base run too). Route
checks: on route in all 10. Anomalies: as the base (stand-ins at load, the crashed-pipeline skips, gate give-ups) except:
the direct runs' `host import failed ... mirrored instead` red lines (6-46 pools; they fall back to mirrors), the render
target churn when GPU time rises (host0, unc2, c256), and one `graceful exit not done in 4.0 s` in `dm-final-on` (also in
three MKW runs of 2026-11-25, none of my other direct runs: not attributed). No device loss.

### 4. Needs audit

- The first-texture transition (`direct::into_watch`): takes the watch lock while the images lock is held (order images
  -> watch -> pre lock; checked that no `ensure_image` runs under `pre_lock` and that no watch-lock holder waits for the
  images lock); the reset of the pool's interior pages and the stash purge; the claim that every scan or walk is wholly
  before or after it (`begin_submit` reads `untextured()` under the watch lock; the walker builds its list without it
  and its pages go through the stash, which is drained with the current list).
- Untextured direct pools out of scans: `p.pending` cleared for them each sync (flushes); their bits are reset only as
  gaps of neighbouring runs; `NXGFX_POOL=host` pools behave the same now.
- Coherence of direct pools generally (pre-existing `Host` path): CPU writes visible at submit (host-coherent import),
  GPU writes at the fence (end-of-submission HOST_READ barrier); a direct pool overlapping a mirrored pool (aliases)
  would see the mirror's GPU writes only at its write-back.

### 5. Unproven

- OoT: not run (in use by other waves). Its shape (GPU 9.5 of 53 ms, per-pool pass 6.1, write-backs 9.9 ms a frame)
  suggests direct pools could pay there; next: `py scripts/play.py oot --env NXGFX_DIRECT_MEM=1` (and `=gpu`,
  `NXGFX_DIRECT_MAX_KB=...`) against the default, with `NXGFX_DIRECT_TRACE=1` once to see which pools OoT's 1 MB a frame
  of GPU writes land in.
- A way to keep the CPU saving without per-draw PCIe reads (the GPU copying the direct pools it uses into VRAM once per
  submission) was considered and not built: it needs a per-submission hook in `resolve` and its per-thread caches, and
  first-use work at the parallel translation's gate for hundreds of pools a frame (every segment off the frontier
  would give up), plus a guard for GPU writes still in flight.
- MKW's render-target churn under dynamic resolution (each new render target uploaded from storage, 10-25 ms a frame)
  is a cost of its own, outside this wave (`image.rs`), seen in the base engine whenever GPU time rises.
- Numbers indicative (shared machine): the base run under two OoT runs gave 25.1 FPS / prep 17 ms, the later pairs 30.

### 6. Slips

- `py scripts/env_switches.py --help` rewrote `docs/env_switches.tsv` (the tool has no help flag and writes the table);
  the file had no other change of mine and was restored from HEAD at once (`git checkout -- docs/env_switches.tsv`), then
  edited by hand. Nothing of it was committed.
- `cargo test` for the direct unit test ran during the first ~60 s of `dm-unc1-mkw` (boot, before its window).

### 7. Questions

- May this wave run OoT (`oot`, GPU 0) to measure `NXGFX_DIRECT_MEM` there? Sources checked: the brief (OoT reserved
  for other waves), UPLOAD-SYNC's OoT numbers. Recommendation: yes, one default and two switch runs, when OoT is free.
