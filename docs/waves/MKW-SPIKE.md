# MKW-SPIKE — the frames that miss the vsync: what is in them, and cutting their causes

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~07:00, worktree
`D:\Projects\RecompLauncherEngine-mkwspike`, branch `mkwspike` (base `gameplay-harness` `d3e4fc2`: every MKW wave of the
day merged). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0 (OOT-MEM owns OoT on GPU
1). Deadline 4 h (~11:00).

## Starting point

The lead's quiet-machine run on the merged build, `build/play/20261127-065536-mkw` (main checkout; console 60 Hz,
nothing else running): **42.7 FPS mean, 5 % low 29.4**, route 3/3 on. The frame is the replay of the game's per-frame
submit plus the pacer's whole-vsync hold (MKW-SYNC); 60 needs the replay op (replay + prep in the frames CSV) under
~15.8 ms on nearly every frame.

## Plan (written before code)

1. **Spike anatomy** from the baseline's frames CSV over [250, 325] s, then a profiled baseline run of this build
   (`NXRS_HOST_PROF` with stacks on `vk replay`, `nvn-completion`, `vk pool walker` over the race's slow stretches):
   which frames miss 15.8 ms, and what is in them (translation, texture uploads, the pool sync's lock wait or scan,
   pipeline compiles, the present).
2. **Pool-lock holders**: the completion thread's and the walker's samples under the pool lock while the replay
   thread's syncs wait.
3. **Cut the spikes' causes**, each behind a switch (`0` = the old path, rows in `docs/env_switches.tsv`).
4. **Verify**: `mkw`, `mkw-turn` on GPU 0, route checks, every sheet; `cargo test --release -p graphics` (and every
   other crate touched). The final run is compared with the baseline and its diagnostics (no switch-off controls).

Expected files: `src/graphics/src/vk/image.rs` (texture uploads), `mem.rs` / `queue.rs` (by the profile),
`docs/env_switches.tsv`, this doc. What would disprove a cause: the frames it names are not the frames over 15.8 ms,
or cutting it leaves the op's p95 and the 5 % low where they were.

## 1. Spike anatomy

Two baselines, the same code (`d3e4fc2`): the lead's quiet run `20261127-065536-mkw` (main checkout, no diagnostics) and
this wave's profiled run `build/play/20261127-071357-sp-base-prof` (host profiler on three threads over 262-306 s;
another wave's builds may have run meanwhile). Window [250, 325] s of the frames CSV; op = `replay_ms` + `prep_ms`.

| | lead's quiet run | `sp-base-prof` |
|---|---|---|
| FPS / 5 % low (report) | 42.7 / 29.4 | 52.8 / 29.9 |
| op p50 / p90 / p95 / p99 | 13.33 / 17.06 / 18.32 / 23.34 | 12.92 / 15.45 / 16.76 / 21.81 |
| frames with op ≤ 15.8 ms | 80.1 % | 91.6 % |
| frames shown at 1 / 2 vsyncs | 2,038 / 1,139 | 3,462 / 480 |
| frame step in the race (held 1 / 2 of 1,800; failed trials) | 1,106 / 694; 17 | 1,591 / 209 and 1,587 / 213; 12-14 |

**What the slow frames hold** (op > 15.8 ms against op ≤ 14 ms, means; `sp-base-prof`, the lead's run alike):

| | slow (332) | fast (2,937) | delta |
|---|---|---|---|
| translation (`replay_ms`) | 15.27 | 9.58 | +5.69 |
| of it texture uploads (`tex_upload_ms`) | 2.00 | 0.03 | +1.98 |
| lanes' set time (`descriptor_ms`, CPU over all lanes) | 26.23 | 12.56 | **+13.67** |
| pool sync | 3.53 | 2.67 | +0.86 |
| draws | 2,132 | 1,675 | +458 |
| pipelines made | 0.05 | 0.00 | (9 slow frames of 332) |
| GPU | 4.18 | 3.94 | +0.24 |

Three kinds of slow frame:

1. **Upload frames** (6-9 % of race frames carry a texture upload > 0.5 ms; 20 % of the frames shown late): the upload
   itself is small (1.6 ms mean in 212 of the 236) but the **lanes' set time doubles** (13.3 → 28.4 ms CPU a frame; the
   lead's run 14.3 → 32.9) and translation without the upload grows too (10.1 → 11.5 ms). A lane's view lookup
   (`exec::tex_view_uncached`) takes the images' lock and, at a texture's first use, uploads it from its storage under
   that lock (`prepare_image`: storage read, CPU deswizzle, the ASTC → BC7 disk cache's key hash and pack read, the
   staging copy): every other lane's view lookup waits. In the profile 7 % of the replay thread's busy samples are in
   `tex_view_uncached`, ~750 of them waiting for the images' lock.
2. **Big uploads**: single frames of 20-250 ms (`tex_upload_ms` 21-241 ms) when the course streams in 4096² / 8192²
   ASTC 8x8 and BC7 textures (21.8 MB each; `uploads from storage` lines). Of their samples on the replay thread: the
   ASTC cache's synchronous pack read 26 %, the storage copy (`guest_bytes`) 15 %, the staging copy 8 %, the cache key's
   hash of the blocks 8 %, the CPU deswizzle 6 %. Rare (a few a minute), but each such frame and the late ones after it
   start a step down to 2 vsyncs.
3. **Heavy-draw stretches** (2,200-2,500 draws, e.g. 290-300 s): the op runs 14-16 ms frame after frame, and the frame
   shown is op + ~3.2 ms (GPU tail, the completion's publication, the game's next submit), so 19-20 ms: late (> 1.1
   vsync) often enough that the share rule leaves 1 vsync. No single cause: more draws on the lanes.

**What a step down costs**: every 1 → 2 step in the race was followed by 1, 2 or 4 s at 30 FPS (the trial hold-off
doubles after a failed trial: `fstep::HOLD0`); in `sp-base-prof` five steps in 44 s took ~14 s at 2 vsyncs. The steps
came after a run of 19-23 ms frames, two of the five with an upload frame among the eight frames before.

## 2. Pool-lock holders

Host profile of `nvn-completion` and `vk pool walker` beside the replay thread (`sp-base-prof`, 44 s, round-robin
samples, ~1,400 a second each; the sampler writes one tick of the three threads in turn):

| holder | under the pool lock (≈ ms a frame) | what |
|---|---|---|
| completion thread (landings, `apply_writebacks`) | **≈ 2.2** | `land` 4.1 % of its samples, `watch_lock` 3.5 % (draining the walker's pages: **the list of every watched pool made again at each drain**), `take_lazy` 2.1 %, `watch::take` 1 %, an env lookup per write-back 0.4 % |
| completion thread waiting for it | ≈ 0.5 | 2.7 % of its samples |
| pool walker (`scan_list` made again: its cache key moves at nearly every walk) | ≈ 0.8 | plus sealing ≈ 0.3; waits for the lock 5.4 % of its samples |
| replay thread's syncs | (holder too) | GetWriteWatch scan under the pool and watch locks ≈ 0.5, per-pool pass |

The replay thread's syncs wait 0.6-0.9 ms a frame (`pools` lines); the chain trace (`NXGFX_CHAIN_TRACE`, `sp-v1b-prof`,
0.3 s at hold 2) shows the other direction too: each landing waited a mean 315 µs (max 1.3 ms) for the pool lock while
a sync's prep (1.1-2.5 ms) held it, and a submission's events and fence were published 0.74 ms after its GPU completion
on average (max 3.8 ms): that wait is in the frame's tail.

What the pacer does with it: it leaves 1 vsync only when the last 30 frames average more than 1.1 vsync
(`short_of_k`) and fewer than 75 % of its window were on time; in the heavy stretches they do (frames of 19-20 ms), so
the pacer is not the fault, and a 1-vsync trial there fails within 0.3-0.5 s and doubles the hold-off (up to 8 s at
30 FPS: lead's run 273.6 → 281.6 s, 299.9 → 307.9 s). Holding 60 through them needs the shown frame (op + ~3.2 ms)
under ~17.5 ms on average, i.e. ~1.5-2 ms off every heavy frame.

## 3. Built (each behind a switch, default on, `0` = the old path; rows in `docs/env_switches.tsv`)

### A lane's upload decoded without the images' lock (`NXGFX_UPLOAD_UNLOCKED`; `image.rs`, `exec.rs`, `tex/astc.rs`)

Before `tex_view_uncached` calls `prepare_image`, `Gpu::decode_unlocked` looks under the lock whether that image is due
an upload from storage (64 KB and up, not of a sparse pool, not GPU-deswizzled), copies what the decode reads
(`DecodeIn`: the texture, layers, levels, ASTC or not, 3-byte expansion), releases the images' lock, and makes the CPU
part of the upload: the page hashes (taken before the read, as `image_begin_submit` does for its decode-ahead), the
storage read and its CPU deswizzle (`decode_parts_of`), and for an ASTC level the disk cache's key and pack read
(`astc::cache_probe`: the same rule as `record`'s own lookup). Under the lock again it hands the parts to the upload as
`decoded` (the path `decode_many` already uses), with the early hashes. The parts are dropped, and `prepare_image`
reads the storage itself as before, when the image changed meanwhile (gone, uploaded, another storage or format) or
when the CPU wrote texture storage behind the graphics layer (`TEX_DUE` moved: WriteTexels, a movie frame). Counted in
the `uploads from storage` line (decoded unlocked, dropped, ms a frame). The recording (staging copy, ASTC dispatch or
cached BC7 blocks, the copies) stays under the lock, in the shared upload buffer's order (`gate::pre_lock`).

### The walker's pages given out over a kept list (`NXGFX_DRAIN_LIST`; `mem.rs`)

`drain_walk` (every landing and every sync, under the pool lock) made the list of every watched pool again at each
drain to give the walker's written pages to their pools. The list is now kept in `Pools` with the `POOL_GEN` it was
made at, and made again only when that moved; `POOL_GEN` moves under the pool lock after every change of a pool's
membership, placement, watch or buffer (`pool_add` after its insert, `pool_del`, `note_address` at the end of every
buffer made), which the walker's own `scan_list` cache already relies on. Counted in the walker's line (drains, and
how many made the list again). `NXGFX_LAZY_CPU_CHECK` is read once instead of at every write-back under the lock.
