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
reads the storage itself as before, when the image changed meanwhile (gone, uploaded, or any field the decode and its
copies use: storage, size, format, extent, levels, target, flags, stride, layers, ASTC, expansion; the shape check was
widened from storage and format after the GPU hang in §5) or when the CPU wrote texture storage behind the graphics
layer (`TEX_DUE` moved: WriteTexels, a movie frame). Two lanes may decode one texture at once; the first back under
the lock uploads it, the other's parts are dropped. Counted in
the `uploads from storage` line (decoded unlocked, dropped, ms a frame). The recording (staging copy, ASTC dispatch or
cached BC7 blocks, the copies) stays under the lock, in the shared upload buffer's order (`gate::pre_lock`).

### The walker's pages given out over a kept list (`NXGFX_DRAIN_LIST`; `mem.rs`)

`drain_walk` (every landing and every sync, under the pool lock) made the list of every watched pool again at each
drain to give the walker's written pages to their pools. The list is now kept in `Pools` with the `POOL_GEN` it was
made at, and made again only when that moved; `POOL_GEN` moves under the pool lock after every change of a pool's
membership, placement, watch or buffer (`pool_add` after its insert, `pool_del`, `note_address` at the end of every
buffer made), which the walker's own `scan_list` cache already relies on. Counted in the walker's line (drains, and
how many made the list again). `NXGFX_LAZY_CPU_CHECK` is read once instead of at every write-back under the lock.

### Not built

- Reading a BC7 cache hit straight into the lane's staging memory (it is read into a fresh zeroed Vec, then copied
  into staging under the images' lock: 64 MB for an 8192² level). Safe only if the staged copy is recorded in the same
  child submission; a decode left on the image for a later submission would copy from recycled staging. Not attempted
  after the hang in §5.
- The walker's `scan_list` (≈0.8 ms a frame under the pool lock): its cache key (`POOL_GEN`, `SEAL_GEN`, `RUNS_GEN`)
  moves at nearly every walk (stores wake pools, chunks open and close every frame); an incremental list is the lever.
- The sync's GetWriteWatch scan under the pool and watch locks (≈0.5 ms a frame): moving it out needs the walker's
  piece scheme (lock order pool → watch forbids releasing only one).

## 4. Measured

Window [250, 325] s, default pacer, GPU 0, 60 Hz console. Profiled runs: host profiler on three threads 262-306 s.
**What else ran** (folder times of every other wave's runs): `sp-base-prof` ran alone; every later run of this wave
overlapped an OOT-MEM OoT run on GPU 1 (v1 / v1b `om-v1-oot` 07:35-07:42, v2 `om-v2` / `om-v3`, final `om-v3` 07:59-
08:10, final2 `om-v4` from 08:17); web waves' builds (`wasm-opt` at ~5.7 cores, cargo) ran at times too. Under that
load MKW's lanes, replay thread and even its GPU time slow down (GPU p50 3.9 → 4.2-4.6 ms) and the pacer sits at
2 vsyncs, so **the absolute FPS of the later runs is not comparable with the baselines**; the within-run figures of the
stages each change targets are.

| run (`build/play/`) | build | FPS / 5 % low | op p50 / p95 | op ≤ 15.8 | lanes' set time, upload frames ÷ none | translation w/o upload, ÷ none | sync pool-lock wait ms/frame | completion pool-lock wait | walker drain wait ms/walk | GPU p50 | route |
|---|---|---|---|---|---|---|---|---|---|---|---|
| main `20261127-065536-mkw` (lead, quiet) | `d3e4fc2` | 42.7 / 29.4 | 13.33 / 18.32 | 80 % | 2.31 | 1.13 | 0.71 | 1.80 | 0.166 | 4.23 | 3/3 |
| `20261127-071357-sp-base-prof` (alone) | `d3e4fc2` | 52.8 / 29.9 | 12.92 / 16.76 | 92 % | 2.14 | 1.14 | 0.62 | 1.42 | 0.162 | 3.93 | 3/3 |
| `20261127-073921-sp-v1b-prof` (OoT) | + unlocked upload | 36.6 / 25.5 | 14.25 / 19.43 | 75 % | **1.57** | **0.87** | 0.86 | 2.11 | 0.184 | 4.28 | 3/3 |
| `20261127-075355-sp-v2-prof` (OoT) | + drain list | 30.6 / 0.0 (hang, §5) | 15.18 / 21.11 | 61 % | 1.71 | 0.98 | 0.57 | 1.43 | **0.074** | 4.24 | 2/2 (died before the 3rd) |
| `20261127-080555-sp-final` (OoT) | final (`9842234`) | 31.2 / 28.7 | 15.67 / 21.69 | 53 % | 1.65 | 0.92 | **0.55** | 1.46 | **0.076** | 4.51 | 3/3 |
| `20261127-081923-sp-final2` (OoT) | final | 30.7 / 29.1 | 17.14 / 23.03 | 29 % | 1.63 | 1.02 | 0.68 | 1.69 | **0.084** | 4.63 | 3/3 |
| **`20261127-082830-sp-final3`** (no OoT run) | final | **45.8 / 29.5** | **12.71 / 18.52** | **84 %** | 1.75 | 1.09 | **0.49** | **1.20** | **0.066** | 4.21 | 3/3 |

(`sp-v1-prof`, the first run of the v1 build, went off route: its title-screen presses landed in another menu path,
another character and mode, then the attract race; the game's HID came up 2.3 s later than in the baseline while a
web wave's build ran. FPS invalid, not used.)

- **Unlocked upload**: in frames carrying a texture upload the lanes' set time fell from 2.1-2.3× to 1.6-1.7× that of
  frames without one, and the translation besides the upload from 1.13-1.14× to 0.87-1.02×, in every run of the new
  build. The replay thread's (lane 0's) `tex_view_uncached` samples fell from 4.5 % to 3.1 % of all its samples, its
  images-lock wait samples in there from ~650 to ~330 (`sp-v1b-prof` against `sp-base-prof`, loaded against unloaded).
  `uploads from storage` lines: about half the uploads go the unlocked way, 0-45 dropped per ~10 s (most at boot).
- **Drain list**: the list was made again at 0-13 of ~370 drains between walker lines; the syncs' and landings' wait
  for the watch lock with the drain fell from 0.16-0.18 to 0.074-0.084 ms a walk, and the replay syncs' pool-lock wait
  to 0.55-0.68 ms a frame under load (0.86 in the loaded v1b without it; 0.62-0.71 unloaded at the baseline).
- **`mkw-turn`** (`20261127-081308-sp-final-turn`, final build, window [250, 290] s): **55.0 / 31.2**, route checks
  **4/4 on** (grid, t37, t41, t46); `om-v3` ended at 08:14 and `om-v4` started at 08:17, inside its window. Earlier
  waves' `mkw-turn`: 29.1 / 20.0 (MKW-SYNC, loaded), 30.7 / 25.0 (MKW-SETS), 41.4 / 30.0 (MKW-AHEAD); not comparable.
- **Final against baseline** (`sp-final3`, timed so its race window fell between OOT-MEM's runs: no OoT run in it;
  other waves' cargo started at 08:33:50, the window's last seconds): 45.8 / 29.5 FPS against the lead's quiet 42.7 /
  29.4; op p50 / p95 12.71 / 18.52 against 13.33 / 18.32; frames with op ≤ 15.8 ms 84 % against 80 %; the syncs' and
  the completion's pool-lock waits the lowest of any run (0.49, 1.20 ms a frame). Against this wave's unloaded profiled
  baseline (52.8 / 29.9, op 12.92 / 16.76) it is lower. The two baselines are one build 10 FPS apart, so **the FPS and
  p95 differences are inside run-to-run spread**: no FPS effect is shown either way; the targeted stages moved in every
  run of the new build.
- **60 at the default pacer: not reached.** `sp-final3`'s frame steps held 1 vsync for 1,301 and 1,349 of 1,800 frames
  with 16-19 failed trials (lead's run 1,106; `sp-base-prof` 1,587-1,591); the 5 % low stays on the 30 line: the
  heavy-draw stretches' frames are 19-20 ms.

## 5. A GPU hang in `sp-v2-prof`

At 314.9 s the game presented nothing for 3.2 s (frame 12463, an ordinary replay of 13.9 ms before it), presented
once more, and at 324.5 s ended with status 72 (the completion thread's deadline on a submission's GPU completion).
Windows logged `nvlddmkm` event 153 at 07:59:20 and "Display driver nvlddmkm stopped responding and has successfully
recovered" (4101) at 07:59:25, LiveKernelEvent 141. OoT on GPU 1 (`om-v3-oot`, started 07:59:00) ran on without a
device loss, so the hang was MKW's GPU 0 work. Status 72 leaves no checkpoint dump (that is the device-lost path, 71),
so the hung submission is not known. No other MKW run with a result since 11-25 (53 runs in the worktrees' and main
`build/play`) ended in a device exit.

What was checked:
- Drain list: every production change of `by_base` (`pool_add` after its insert, `pool_del`) and of a pool's watch or
  placement (`ensure_buffer`, ending in `note_address`) moves `POOL_GEN` under the pool lock, so the kept list is the
  list a fresh walk would make; the test `drained_pages_reach_their_pools_with_the_list_kept` shows a pool added since
  gets its pages.
- Unlocked upload: the decode is `decode_parts` on the same bytes; the hashes are taken before the read as
  `image_begin_submit`'s decode-ahead does; `lazy::touch` / `bring_back` are made for any thread. One weakness was
  closed: the relock check compared storage, size and format only, not the shape the parts were cut to; a mismatch
  would record copies past the image's extent, a command that can hang a GPU. Now every field the decode and its copies
  use must stand (`9842234`).
- After that, four runs of the final build (`sp-final`, `sp-final-turn`, `sp-final2`, `sp-final3`) ran to the end
  without a hang. One hang in 7 runs of this branch is not proven caused or uncaused by it; the cause stays
  open (§7).

## 6. Pictures

Every sheet read: `sp-base-prof`, `sp-v1-prof` (off route: the attract race and its results, correctly drawn),
`sp-v1b-prof`, `sp-v2-prof` (to 292 s), `sp-final`, `sp-final2`, `sp-final3`, `sp-final-turn` (`sheet-1.png`,
`sheet-2.png`). Boot (highway flyover), title (the Mario Kart arch; black or a dim sky where the shot falls on the
opening-to-title cut, as in earlier waves), menus and grid (Mario at the Mario Bros. Circuit start), the desert slope
and lap 2 by the water: lit and textured, HUD, coins, items, minimap and opponents in every run; `mkw-turn` t37-t46 up
the banked slope, all correct. No pop-in, stale or wrong textures, flicker or missing geometry. The roadside shrubs
render as dark silhouettes in every run, the baseline's too (MKW-SYNC noted it; not from this wave). Anomalies: the
baseline's known set only (zero stand-ins at boot, none in the race window; the crashed-pipeline and timestamp-query
red lines with the crashed pipeline's skipped draws; gate give-ups at loads, 4 in the final window; held-back-frame
70-90 in the window against the lead's 90).

## 7. What is left

- **The GPU hang (§5)**: unexplained. Checkpoints do not help as things stand: status 72 is a deliberate no-logging
  self-termination 5 s into a completion wait (`submission_safety::resource_safety_fatal`), so a recurrence leaves no
  checkpoint dump unless another thread meets `VK_ERROR_DEVICE_LOST` first (in `sp-v2-prof` none did). Localizing it
  needs either the last checkpoints written before status 72 (a change to that exit's no-logging contract, the owner's
  and the harness's) or repeat runs with one switch off at a time once it recurs. Until then, watch the merged build's
  MKW runs for exit 72 / 71.
- **Heavy-draw stretches** (2,200-2,500 draws): the frame there is op 14-16 ms + ~3.2 ms; 60 needs ~1.5-2 ms off
  every such frame. Left on the path: the pool sync (3.0-3.5 ms: lock wait 0.55-0.7, GetWriteWatch scan ≈0.5 of it
  under the locks, the per-pool pass ~1.1), the lanes' view lookups (`by_serial` / view-map probing, ~1 % of the replay
  thread), the completion's publication after GPU completion (0.74 ms mean in the chain trace, of it the pool-lock wait
  0.3).
- **Big uploads**: the BC7 cache hit read into staging (§3 Not built), the walker's `scan_list` and the sync's scan
  under the locks (same).
- **Quiet-machine numbers**: the lead's run after merge (only `sp-final3` here had no OoT run in its window).

## Report (agent, protocol §5)

Branch `mkwspike` (base `gameplay-harness` `d3e4fc2`): `1678163` (plan; unlocked upload), `071859f` (drain list, switch
rows, tests), `9842234` (relock shape check; doc), this commit (doc).

### 1. Built
- **Diagnosis** (§1-§2): the frames over 15.8 ms are (1) upload frames, where a lane uploads a texture under the
  images' lock and every other lane's view lookup waits (lanes' set time ×2.1-2.3), (2) rare big-upload frames of
  20-250 ms (ASTC cache pack read, storage copy, staging copy), (3) heavy-draw stretches whose frames are 19-20 ms
  (op 14-16 + ~3.2 ms tail), which is what makes the pacer leave 1 vsync, and its failed trials' doubling hold-off (up
  to 8 s at 30 FPS) is the 5 % low. Pool-lock holders: the completion thread ≈2.2 ms a frame (of it the drain's list
  of every watched pool made again ≈0.8 ms), the walker's `scan_list` ≈0.8, the syncs' own scan ≈0.5; landings wait
  ~0.3 ms each behind the syncs, which delays the frame's fence.
- **Corrections** (§3), default on, `0` = old path: `NXGFX_UPLOAD_UNLOCKED` (the CPU part of a lane's upload made
  without the images' lock), `NXGFX_DRAIN_LIST` (the watched list kept by `POOL_GEN`); `NXGFX_LAZY_CPU_CHECK` read once.
- **Effect** (§4): upload frames' lanes set time 2.1-2.3× → 1.6-1.75×, their translation besides the upload 1.13 →
  0.87-1.09×; drain wait 0.16 → 0.066-0.084 ms a walk; the syncs' pool-lock wait 0.62-0.71 → 0.49 ms a frame in the
  unloaded final. FPS: final 45.8 / 29.5 against the lead's quiet 42.7 / 29.4 and the profiled baseline's 52.8 / 29.9:
  inside spread, not shown.

### 2. Tests (`cargo test --release -p graphics`: lib 305 passed, 6 ignored; other targets pass)
- `image::decode_in_tests::a_decode_from_the_copied_fields_gives_every_level`: a block-linear texture's levels decoded
  from the copied fields (`DecodeIn`) are the rows swizzled in, at their sizes, with no cache lookup attached.
- `image::decode_in_tests::the_cache_lookup_ahead_applies_where_record_would_look`: no lookup without the pack and the
  full encoder, for RGBA8 output, or for blocks outside 4..=12.
- `mem::seal_tests::drained_pages_reach_their_pools_with_the_list_kept`: drained pages reach every watched pool holding
  them (nested too) with the list kept; a pool added since (`POOL_GEN` moved) gets its pages; an unwatched one none.
- New functions: no failing-before state. The unlocked path's concurrency (two lanes, drops) is covered by the runs only.

### 3. Verified
Runs and numbers §4, pictures §6 (every sheet read). Route checks on in every run of the new build but `sp-v1-prof`
(menu path, §4; FPS invalid) and `sp-v2-prof` (died at 324.5 s after its two route shots); `mkw-turn` 4/4 on, 55.0 /
31.2. Target 60 at the default pacer: not reached.

### 4. Unproven
- **The GPU hang** (§5, `sp-v2-prof`, exit 72 and a TDR on GPU 0): cause unknown; both changes were checked for a
  mechanism, one weakness closed; 4 runs clean since. Next step in §7.
- FPS effect: no run here is comparable enough (the lead's quiet run after merge decides).
- Other titles (OoT, DKB, PLZA) not run (not owned); both switches apply to every title.

### 5. Slips
- Relinks ran the normal CLI (`dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build`) with
  `NXRS_LINK_LIBS=-Wl,/DEBUG` set (a PDB beside the DLL, for the host profile), as earlier MKW waves did; the DLL and PDB
  of each build were copied into the scratchpad for symbolizing only. The MKW DLL in the library is this branch's
  `9842234` build.
- Two symbolizations of the baseline profile were made against the relinked DLL by mistake and thrown away (not used).
- None of §4's editing rules broken (editor tools only; no `cargo fmt`).

### 6. Questions
None.
