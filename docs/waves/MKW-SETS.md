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

### Not built

- The first part's pool sync (~1.4 ms; `upskip::upload_changed` and `watch::take` inside): USE-SYNC's use-driven
  sync, not reached.
- View lookups at a segment's start (images' lock ~2 ms a frame of lane waits): views are kept per submission because
  `prepare_image` is per use; seeding keeps a seeded set's sampled textures, which removes many of them.
