# MKW-SETS — the lanes' per-draw cost (descriptor sets, locks) and the replay thread's prep

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~02:30, worktree
`D:\Projects\RecompLauncherEngine-mkwsets`, branch `mkwsets` (base `mkwahead` `08bddb3`: balanced segments, park chain,
SET-FAST step 1, USE-SYNC chunk tracking). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only,
GPU 0 (OOT-REPLAY owns OoT on GPU 1; what else ran on the machine during these runs is not known to me). Deadline 4 h.

## Starting point

MKW-AHEAD: with the pacer forced to 1 vsync (`NXOS_FPS_HOLD=off`) MKW runs 53-54 FPS, the frame op 11.5-15 ms; the
default pacer holds 1 vsync only after 30 frames ≤ 15.8 ms, so it sits at 30-40. Inside the op: A2 / A3 at ~20 µs a
draw on the lanes (~7.5 serial), the first part's prep ~2 ms. SET-FAST: ~700 whole sets a frame at segment start
(~9.3 µs, 95-97 % "no copy of the module": a segment's `QState` starts empty), ~2,300-2,600 patched at ~2.6 µs, half
of the set time unattributed.

## Plan (written before code)

1. **Stack profile** of the replay thread and the lanes over the race (`NXRS_HOST_PROF` with stacks, base build with
   PDB, hold 1): per-draw time split into set build (whole / patched), view lookups, banks, staging, and lock waits
   (which lock, its holder's path).
2. **Cut it**, each behind a switch (0 = old), chosen by step 1's split:
   - segment seeding: a segment starts with the sets its stage shaders had at the end of the segment before it (or
     the frame's last state), so it patches instead of building whole sets; per-submission side effects (staging,
     image preparation, GPU-write notes) redone for the classes that have one;
   - the hot lock(s): per-lane caches / read-mostly structures;
   - the first part's prep, if step 1 shows a cheap lever.
3. **Measure**: default pacer (FPS, 5 % low) and `NXOS_FPS_HOLD=off`; frame op median / p95 (frames CSV replay +
   prep, and the chain trace); same-build switch-off controls back to back. Target op ≤ ~12 ms, p95 < 15.8 ms.
4. **Verify**: `mkw`, `mkw-turn` (route checks, every sheet); `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/exec.rs`, maybe `queue.rs`, `tables.rs`, `image.rs`, `docs/env_switches.tsv`,
this doc. What would disprove it: the profile shows the lanes' time is not in sets or locks (e.g. driver calls, guest
memory reads), or seeded sets still miss (keys differ for another reason).

## 1. Profile split

How it was taken: `NXRS_HOST_PROF=265,40,<file>`, `NXRS_HOST_PROF_STACK=1`, the MKW DLL linked with
`NXRS_LINK_LIBS=-Wl,/DEBUG`, symbolized with `llvm-symbolizer` against the PDB (a scratch script; samples whose stack
holds `Condvar::wait` are idle and left out; "busy" below is the rest). Hold 1.

### Replay thread and lanes together (base build, `20261127-023005-ms-base-prof`, 97,730 busy of 288,030 samples)

| where | share of busy samples |
|---|---|
| **contended lock waits** (futex) | **28.4 %** |
| — pools' lock in `mem::room` (via `room_cached`, every read-only storage buffer and uniform buffer of a set) | 8.1 |
| — pools' lock in `note_gpu_write` (every written storage buffer of every set) | 4.9 |
| — pools' lock in `mem::resolve` (address-cache misses 2.9; indirect mesh records, uncached, 2.3) | 5.1 |
| — images' lock in `tex_view_uncached` (view misses: a segment's memo starts empty) | 3.2 |
| — images' lock in `target_image` (pass begins) | 1.5 |
| — tables' lock in `samp_at` (every texture slot's sampler) | 1.4 |
| — lazy images' `make_lock`, tables' `placeholder_at` / `tex_at`, `begin_submit_ex` | 2.4 |
| `stage_set` (all set building) | 45.8 |
| — patched sets (`patch_set`) / whole sets | 23.3 / 22.9 |
| — inside: `ubo_info` (its `room` lock) / texture views / `note_gpu_write` / `write_db_set` / banks / `samp_at` | 11.7 / 11.8 / 5.6 / 4.7 / 3.8 / 2.3 |
| address memos' SipHash + probing (`room_cached`, `resolve_cached`) | ~4 |
| pool sync (`begin_submit_ex`) / image walk (`image_begin_submit`) / present | 8.1 / 4.5 / 6.6 |

So the per-draw ~20 µs on a lane was **about a third lock waits**, almost all on the pools' lock (`g.mem`) and the
images' lock, taken by lookups whose answers rarely change; the rest is set building (texture views at segment start,
the uniform-buffer slots, the GPU-write notes).

### Replay thread alone (after the lane caches and seeding, `20261127-031701-ms-replay-prof`, 44,750 busy samples)

| where | share |
|---|---|
| translating its own segments (lane 0) and serial streams | ~36 |
| **pool syncs** (`begin_submit_ex`; of it `upskip::upload_changed` 5.5, `watch::take` 3.6, `watch_lock` 1.9) | **18.9** |
| **image walks** (`image_begin_submit`; 10 of it the many-dirty-ranges branch looking every image up) | **13.6** |
| presents (`present_locked`): its own pool sync 6.9 + image walk 7.5 + record / submit 3 | 17.5 |
| pre-pass (`plan`) | 6.7 |
| `vkQueueSubmit2` / `vkQueuePresent` / end command buffer | 3.0 / 1.9 / 0.6 |

The replay thread is busy ~16.5-17 ms of a ~18 ms frame at hold 1 (`[vk] pipelining` line): it, not the lanes, is
the limit now, and a third of it is per-submission prep (syncs and image walks), a third of that at the present.

## 2. Built (each behind a switch, `0` = the old path; rows in `docs/env_switches.tsv`)

### Lane caches (`NXGFX_LANE_CACHES`, `mem.rs`, `tables.rs`, `exec.rs`)

The lanes' lookups that took a shared lock although their answer changes only when the pools or the samplers do:

- `mem::room`: from a copy of the pools' extents (`EXTENTS`: base, size, window, by base) kept by a new
  `LAYOUT_GEN`, which moves where `POOL_GEN` moves except at a sync's deferral of CPU writes (a deferral changes no
  pool's extent; it moved `POOL_GEN` and flushed every lane's `room` memo, after which each miss took the pools' lock).
  The copy is made again under the pools' lock when older (every layout change holds that lock). The thread memos
  (`room`'s `LAST`, `exec::room_cached`) key on `LAYOUT_GEN` too.
- `mem::resolve`: a plain pool's range (one buffer made, no granules, no windows, no deferred writes) from a copy of
  the pools as of `POOL_GEN` (`PLAIN`), as its per-thread `LAST` entries already answered; anything else takes the lock
  as before.
- `note_gpu_write`: the pools' map's answer for a range (pool, identity, pieces) remembered per thread by `POOL_GEN`;
  the write-back records themselves still go into each submission (`note_writeback`).
- `Tables::samp_at_cached`: samplers per thread by `SAMP_GEN` (moved by `write_sampler`, the one writer).
- The indirect mesh records resolve through `resolve_cached` (they took the pools' lock at every mesh draw).
- The memos hash with a one-multiply hasher (`mem::AddrHasher`) instead of SipHash.

### Segment seeding (`NXGFX_SET_SEED`, `exec.rs`)

A segment's `QState` started with no stage-set host copies, so its first set of each module was built whole. Now:

- At a parallel run's join the copies of every segment but the last are moved (not copied) into the queue's seeds
  (`QState::seed`, `Seeds`: per stage, one valid keyed copy per module, newest first, at most 160), merged over the
  seeds the stream began with; every state forked from the queue's shares them (`Arc`).
- `patch_set`: when the stage has no copy of the module, the module's seed is cloned in (`seed_into`, if it is of the
  same `POOL_GEN`, valid, keyed, holds no twin view). A copy of another submission is patched by `patch_classes_x`
  with `CL_SUBMIT` added: every slot with a per-submission effect is made again (c0 / c1 / c2 / push / support staged
  bytes, every storage buffer with its GPU-write note, storage images, mesh draw records); only directly bound uniform
  buffers (resolved from global pool state) and sampled textures are kept. The texture slots are made again too when
  the tables' generation, the pools or the handles changed (as before) or when a texture's storage was written behind
  the graphics layer since (`image::TEX_DUE`, moved by `storage_written`); a set holding a twin view (copied from its
  image per submission, `image::is_twin_view`) is never seeded.
- Counters in the `replay up to frame` line: `N of them from a seed, M seeds stale`.

First version (the queue state's own copies deep-copied at every fork) seeded 2 sets a frame and cost the replay
thread more than it saved (two ~330 KB copies a stream); taken out before the measurements below.

### Image walk (`NXGFX_IMG_SPANS`, `NXGFX_IMG_DUE`, `image.rs`)

`image_begin_submit` runs at every submission (~10 a frame) and walked every image twice:

- With ≥ 64 dirty ranges it looked every image up (`by_serial`) to binary-search the ranges. Now a list of the images'
  spans (start, end, serial; by start) is kept while `by_addr` stands (`addr_gen`, moved where an image comes, goes or
  is taken over) and walked beside the sorted ranges with a forward pointer (`spans_hit`).
- The walk for images due (init / upload) over every image: now over candidates (`Images::due`: made or CPU-written
  since, or found due and not prepared yet; the images the CPU writes overlap now; the sparse ones).

### Lean presents (`NXGFX_PRESENT_LEAN`, `queue.rs`)

A present made its own pool sync and image walk (~14 % of the replay thread). When the presented image is a picture
the GPU drew (`image::presented_gpu_drawn`: GPU-owned, not sparse, nothing due) the present records only its blit; the
next submission syncs and walks before any GPU work reads guest memory (`record_present` prepares the presented image
itself, as before). Counted in the `pipelining` line (`presents without one`).

## 3. Measured effect

Frame op from two sources: the chain trace's frame op (`NXGFX_CHAIN_TRACE=280:0.3`, 15-18 ops a run: small n) and the
frames CSV's replay + prep per presented frame over [250, 325] s (~4,000 frames; it holds the other streams of the
frame too, so it is ~2 ms above the chain-trace op). "≤ 15.8" = share of frames whose replay + prep fits the pacer's
1-vsync trial line. Pairs were run back to back on the same build, the second with the switch(es) at `0`.

### Hold 1 (`NXOS_FPS_HOLD=off`)

| Run (`build/play/`) | switches | FPS / 5 % low | chain op median | replay + prep p50 / p95 | ≤ 15.8 | lanes CPU a frame (lock waits: pools GPU-write / resolve, images) |
|---|---|---|---|---|---|---|
| `20261127-024241-ms-lc-on-hold1` | lane caches (1st cut) | 52.7 / 39.0 | 11.39 | 14.60 / 19.60 | 69 % | 23.1 (0.06 / 3.44, 1.57) |
| `20261127-024819-ms-lc-off-hold1` | `NXGFX_LANE_CACHES=0` | 53.0 / 40.0 | 11.63 | 14.93 / 19.42 | 65 % | 28.8 (1.88 / 1.71, 1.82) |
| `20261127-032631-ms-all-on-hold1` | lane caches + seeds + spans | 53.6 / 42.6 | 12.66 | 14.67 / 18.10 | 74 % | 23.1 (0.30 / 0.23, 2.15) |
| `20261127-033210-ms-all-off-hold1` | all three `0` | 51.5 / 39.0 | 14.15 | 15.64 / 19.89 | 53 % | 30.6 (2.12 / 2.13, 1.85) |
| `20261127-034058-ms-lean-on-hold1` | + lean presents | **55.6 / 46.0** | 11.90 | **13.39 / 17.36** | **87 %** | 21.4 (0.33 / 0.20, 1.74) |
| `20261127-034635-ms-lean-off-hold1` | `NXGFX_PRESENT_LEAN=0` | 53.6 / 41.0 | 12.53 | 13.95 / 18.90 | 79 % | 25.5 |
| `20261127-040517-ms-due-on-hold1` | + due walk (final build) | 54.6 / 45.3 | 12.44 | 13.96 / 17.65 | 81 % | 24.3 |
| `20261127-041055-ms-due-off-hold1` | `NXGFX_IMG_DUE=0` | 51.9 / 42.4 | 11.54 | 14.95 / 19.43 | 63 % | 24.2 |

Seeding alone (`20261127-031112-ms-seed2-on-hold1`, 52.6 / 40.9): ~370-390 sets a frame patched from a seed (of
~650 whole before), 1-30 seeds a frame stale, lanes 23.3 → 21.6 ms CPU. The first seeding version
(`025750-ms-seed-on-hold1` 53.6 / 43.0 vs `030328-ms-seed-off-hold1` 56.7 / 47.0) seeded 2 sets a frame and is not
in the branch.

Reading the pairs: every switch moves the lanes' CPU and the replay + prep distribution the right way (lanes −5 to
−7.5 ms a frame; p95 −1.5 to −1.8 ms each for lean presents and the due walk; frames ≤ 15.8 ms 53 → 74 %, 79 → 87 %,
63 → 81 %); the FPS means move +2 to +3 within run-to-run spread (two same-build runs differ by up to ±2-3 FPS here).
The chain-trace op medians (n 15-18) do not separate the pairs; the CSV distributions do.

### Default pacer (the profile's `fps_hold`)

| Run (`build/play/`) | switches | FPS / 5 % low | replay + prep p50 / p95 | ≤ 15.8 | frames held 1 / 2 vsyncs (last three summaries, of 1,800) | route |
|---|---|---|---|---|---|---|
| `20261127-035232-ms-final-on` | all but the due walk | 43.0 / 29.9 | 12.80 / 21.06 | 72 % | 1,105 · 1,557 (+520 earlier) | 3/3 on |
| `20261127-035811-ms-final-off` (right after) | those four `0` | 31.7 / 29.0 | 16.87 / 21.83 | 32 % | 499 · 1,239 · 405 | 3/3 on |
| `20261127-042213-ms-fin3-on` | final build, all on | **45.2 / 29.0** | 13.93 / 17.98 | 81 % | 1,588 · 1,047 · 1,170 | 3/3 on |
| `20261127-043403-ms-fin4-off` (not comparable) | all five `0` | 24.8 / 5.2 | 22.69 / 67.53 | 0 % | 1,695 · 1,541 · 135 | 3/3 on (HUD) |
| MKW-AHEAD's final (for reference) | | 32.4-40.0 / 29-30 | | | 294-880 | 3/3 on |

`mkw-turn` on the final build, `20261127-044608-ms-turn2` (window [250, 290]): 30.7 / 25.0, replay + prep p50 19.5 /
p95 29.2, 4/4 on route; OOT-REPLAY's OoT ran from 04:44 (GPU 1, ~1.6 cores) and this run's opening ran late as in
`fin4-off` (title shot = city flyover), so the window holds the grid and the first turn's streaming; MKW-AHEAD's
`mkw-turn` was 41.4 / 30.0. Not comparable; the lead's quiet run decides.

Coordinator, 04:50 (owner rule, docs/31 §4 `ec16da5`): no more switch-off control runs; the final run is compared with
the wave's baseline (`023005-ms-base-prof`, hold 1, base build: 50.3 / 41.0 with the sampler running; MKW-AHEAD's
default-pacer finals 32.4-40.0 / 29-30). The on/off pairs above were taken before that rule.

`043403-ms-fin4-off` is not a valid control: its opening ran late (the 139 s shot is the city flyover, 158 s the
course flyover), so the input replay drove the kart off the track into the desert sand (`06-race_later`), with
texture streaming (uploads ~30 ms a frame in slow frames) and gate give-ups inside the window; the HUD route check
cannot tell. The valid default-pacer control is `035811-ms-final-off`.

**The default pacer still does not hold 60.** It now holds 1 vsync for 60-88 % of the race's frames (MKW-AHEAD:
16-49 %), so the mean rises to 43-45 FPS, but the 5 % low stays on the 30 line: the 1-vsync trials keep failing
(14-22 failed trials a run) because the frames' cost at hold 1 is ~17-17.5 ms median (frame p50 at hold 1, above),
past the 15.8 ms trial line and near the 18.3 ms late line, so every stretch of slower frames steps back to 2 vsyncs.

### Against the brief's target

- Frame op ≤ ~12 ms: **met at the median** at hold 1 (chain op 11.9-12.4 in the final-build runs; replay + prep
  13.4-14.0 including the frame's other streams). **p95 < 15.8 ms: not met** (replay + prep p95 17.4-18.0).
- Default pacer holding 1 vsync: **not met** (above): 43-45 FPS mean, 5 % low 29-30.

## 4. What is left (by the replay-thread profile)

- **The replay thread is the limit**: busy ~16-18 ms of each ~17-18 ms frame at hold 1 (`[vk] pipelining`), idle
  0.7-1.5 ms. The frame op is ~12 ms of it; the rest is the frame's other streams, the present (now without its sync)
  and the pre-pass (`plan`, ~6.7 % of the thread).
- **Pool syncs** (~12 % of the thread after lean presents: the first part's 1.4-1.5 ms, of it `upskip::upload_changed`
  copy-and-hash and `watch::take`): USE-SYNC's use-driven sync is the lever.
- **Lanes**: 21-25 ms of CPU a frame for ~8.5-9.5 ms of translation wall; the replay thread still waits ~2 ms a frame
  for them. Left in them: the images' lock at view misses (~2 ms of waits a frame: views are per submission), set
  building (`ubo_info`, `write_db_set`, banks).
- The pacer's line: a 1-vsync hold needs frame cost ≤ 15.8 ms for 30 frames in a row; the frame cost at hold 1 is
  now ~17 ms median. Roughly 2 ms more off the replay thread's total, at every frame, is what 60 at the default
  pacer still needs.

### Not built

- The first part's pool sync (~1.4 ms; `upskip::upload_changed` and `watch::take` inside): USE-SYNC's use-driven
  sync, not reached.
- View lookups at a segment's start (images' lock ~2 ms a frame of lane waits): views are kept per submission because
  `prepare_image` is per use; seeding keeps a seeded set's sampled textures, which removes many of them.

## 5. Pictures

Every sheet read (`sheet-1.png` of each run in §3; `mkw-turn` sheets 1-2). Boot, title / course flyover, menus, grid,
race and lap 2 drawn correctly in every run of every switch state, both pacers: lit and textured, coins, items, HUD
and minimap present; no pop-in, stale or wrong textures, flicker or missing geometry. Route checks on in every run
(3/3; `mkw-turn` 4/4: t37-t46 on the banked desert slope, lit and textured, coins, items, HUD). Anomalies: the known set only (zero stand-ins at boot / loads, none in the race window;
the crashed-pipeline and timestamp-query red lines with their skipped draws; gate give-ups at the loads around
75-90 s in both switch states, e.g. 988 + 919 on / 1,304 + 418 off, 4-5 in the race window; held-back CPU writes at
the off runs' rate). The `02-title` shot (139 s) falls on the opening-to-title cut as before: the race grid at hold 1
(the opening already past), black in `035232-ms-final-on`, a dim fade-in in `035811-ms-final-off`, the course
flyover in `042213-ms-fin3-on` (MKW-PIPE / MKW-AHEAD saw the same three in both switch states).

## Report (agent, protocol §5)

Branch `mkwsets` (base `mkwahead` `08bddb3`): `2da5cb0` (plan, lane caches), `56b6d8a` (seeding first version, plain
resolve), `aabac57` (seeds gathered at joins, image spans, switch rows), `49237c9` (lean presents), `cbcdddd` (due
walk, doc), this commit (doc).

### 1. Built
- **Diagnosis** (§1): the lanes' ~20 µs a draw was about a third contended locks (pools' lock in `room`,
  `note_gpu_write`, `resolve`; images' lock at view misses; tables' lock per sampler) and two thirds set building, half
  of it whole sets at segment start. The replay thread, not the lanes, limits the frame now: busy ~16-18 ms a frame,
  a third of it per-submission prep (pool syncs, image walks), a third of that at the present.
- **Corrections** (§2), each switch default on, `0` the old path: `NXGFX_LANE_CACHES` (read-mostly copies and memos
  for room / resolve / GPU-write notes / samplers), `NXGFX_SET_SEED` (segments patch from seeds gathered at parallel
  joins; per-submission slots made again), `NXGFX_IMG_SPANS` and `NXGFX_IMG_DUE` (the per-submission image walks over
  a kept span list and due candidates), `NXGFX_PRESENT_LEAN` (no pool sync / image walk at a GPU-drawn present).
- Effect (§3): at hold 1, final build vs all off: lanes −7 ms CPU a frame, replay + prep p50 15.6 → 13.4-14.0, p95
  19.9 → 17.4-18.0, frames ≤ 15.8 ms 53 → 81-87 %, chain op median ~12; FPS 51.5 → 54.6-55.6. Default pacer: 43-45
  FPS mean (held 1 vsync 60-88 % of frames) against 31.7 with everything off, 5 % low unchanged at ~29-30.
- Rejected with numbers: seeding by deep-copying the queue's copies at every fork (2 seeded sets a frame, slower).

### 2. Tests (`cargo test --release -p graphics`: lib 279 passed, 6 ignored; other targets pass)
- `mem::extents_tests::room_from_the_extents_copy_is_the_window_end`: the extents copy answers `room` as the pools'
  map does (highest base at or below, window end, nothing past a pool, the next base remembered). New function; its
  first version expected a pool laid over to cut the window, which the map's `room` does not do: the test was wrong.
- `mem::extents_tests::plain_pools_resolve_from_the_copy`: plain pools resolve from the copy; past a pool, in none or
  in a pool not plain go to the map.
- `exec::set_patch_tests::a_seed_of_another_submission_remakes_its_per_submission_slots`: a copy of another submission
  patches only with `cross`, always with `CL_SUBMIT`; only directly bound uniform buffers and sampled textures are kept;
  within one submission the classes are as before. `a_patch_remakes_every_slot_whose_inputs_changed` and
  `pushed_buffer_addresses_and_handles_resolve` updated for `CL_SUBMIT` in `res_class`.
- `image::spans_tests::spans_hit_the_images_the_ranges_overlap`: the forward walk finds what the per-image binary
  search finds.
- New behaviour, no failing-before state for the new functions. Flaky, not mine: `mem::seal_tests::*` failed once each
  in two of ~8 full runs (`overlapping_and_uncommitted_ranges_are_not_sealed`, `a_woken_pool_is_asked_again_and_warm`)
  and passed alone and on every rerun; they touch OS page protection.
- `scripts/env_switches.py --check`: the five new rows are clean; its 30 other problems are the base's.

### 3. Verified
Runs and numbers in §3, pictures §5. Route checks on in every run. Target: the frame op ≤ ~12 ms at the median is met
at hold 1; its p95 under 15.8 ms and 60 at the default pacer are not (43-45 FPS mean, 5 % low 29-30).

### 4. Unproven
- The lead's quiet runs: my pairs' FPS differences (+2-3 at hold 1) are inside this machine's spread; the CSV
  distributions and lane CPU are the clearer signal. Each switch was measured against the build before it, not one
  by one on the final build. The final `mkw-turn` (30.7 / 25.0) ran beside OoT with a late opening: unproven either way.
- Lean presents and a presented texture the CPU writes (a movie shown directly): not exercised by MKW's route (its
  presented images are GPU-drawn; `presented_gpu_drawn` falls back to the full sync otherwise).
- Other titles (OoT, DKB, PLZA) not run (not owned); the switches apply to every title.
- Validation layers not run.

### 5. Slips
- Relinks ran this worktree's `target\release\nxrs-build.exe` directly with the launcher's argument list and
  `NXRS_LINK_LIBS=-Wl,/DEBUG` (a PDB for the host profile), as the earlier MKW waves did. The MKW DLL in the library is
  this branch's `cbcdddd` build.
- Three runs were cut when their background shell was stopped (not by me, as far as I can tell; ~12 minutes apart,
  04:21, 04:33, 04:45, together with my waiters and a monitor): `20261127-041634-ms-fin2-on`,
  `20261127-042811-ms-fin3-off`, `20261127-043944-ms-fin-turn`; I closed their games by PID (43036, 6176, 24152).
  Their folders are incomplete and not used. The last runs were started with `Start-Process` (detached) instead.
- I overwrote a `sym.py` another session had left in the shared scratchpad root before moving my scripts to
  `scratchpad\mkwsets`.

### 6. Questions
None.
