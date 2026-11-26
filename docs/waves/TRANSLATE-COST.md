# TRANSLATE-COST — the per-frame CPU cost of translating NVN command buffers (MKW to 60, OoT's frame)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26, HEAD `5056038` (branch `gameplay-harness`).
Protocol: `docs/31-DELEGATION-PROTOCOL.md`; shared rules: `docs/waves/PARALLEL-RULES.md`. Titles: Mario Kart World
(`mkw`, `mkw-turn`), Ocarina of Time (`oot`); regressions `dkb`, `smash`. GPU 1. Deadline 6 hours.

## Brief (lead, condensed)

At 60 FPS the replay thread must finish a frame in 16.7 ms; MKW is at ~20.4-21.7 ms (translation ~11-12, prep ~8.7)
and presents at the 30 cap; OoT's translation is ~20-23 ms of a ~52 ms frame. Cut the CPU cost of translation on the
replay thread and its 6 lanes. Profile first, then fix the biggest costs. Guards: `NXGFX_TRANSLATE_CHECK=1` 0
entry-state differences; MKW-RECORD's gate and ordering rules hold (the dark turn was a lane ordering fault). No
skipped draws, no weakened robustness, native NDVE off. Territory: `exec.rs`, `gate.rs`, `tables.rs`, the set cache,
pipeline lookup, image / view lookups during translation. Not `upskip.rs`, `mem.rs`, `queue.rs`, `lazy.rs`, `gmem.rs`
(another agent's audit fixes); a change needed there is written here and reported to the lead.

## Baseline (from the latest runs, before this wave)

`build/play/20261126-055832-v5-mkw` (UPLOAD-SYNC v5, the build in `dist` at HEAD), race window [250, 325] s:
30.4 / 29.5 FPS; `[vk]` lines: replay 11.0-11.9 ms a frame, prep 7.6-7.9 (pools 5.3-5.5); translate line: 3.0
streams in segments a frame (2 in parallel, 1 sequential: a rare record), 10.8 segments, pre-pass 0.87 ms, segments
9.14 ms wall; **lanes 29.2 ms of CPU a frame together** (lock waits: pools 1.13 at resolves, images 1.08); the
replay thread waited 0.76 ms for the lanes. Serial translation of the same work was ~15-16 ms (MKW-PERF5): the lanes
spend about twice the serial CPU.

## Plan

1. **Profile** (no behaviour change). The engine's host sampler (`NXRS_HOST_PROF`, `runtime/hostprof.rs`) sampled one
   thread and only its leaf address. Extended (diagnosis only): several threads by name prefix
   (`NXRS_HOST_PROF_THREAD=vk replay,vk translate`) and, with `NXRS_HOST_PROF_STACK=1`, each sample's unwound stack.
   The game DLL is relinked with a PDB (`NXRS_LINK_LIBS=-Wl,/DEBUG -Wl,/OPT:REF -Wl,/OPT:ICF`: same code, symbols
   beside it) and the samples are symbolized with `llvm-symbolizer` (a scratch script, not committed). Plus the
   existing counters (`NXGFX_PROFILE=1` set parts, `NXGFX_OP_TIMES=1` per record kind) where they help.
2. **Fix the largest costs** found, in my territory, each behind its existing or a new off switch where it changes
   behaviour, each built (dist lock), MKW relinked, run and its sheet read; `cargo test --release -p graphics`.
3. **Measure** paired before/after under `machine.lock` on GPU 1: `mkw`, `mkw-turn`, `oot`, and `dkb` + `smash`.
   `NXGFX_TRANSLATE_CHECK=1` once on MKW (0 differences).

## Profile

**Runs** (HEAD build, GPU 1, `machine.lock`): `build/play/20261126-062719-tc-prof-lane1` (leaf samples of lane 1,
[255, 315] s) and `20261126-063435-tc-prof-stack` (the replay thread and the 5 lanes with stacks, [255, 305] s,
472,050 samples; the sampler stops each thread ~2,000 times a second, so the game ran slower: shares, not ms).

**The lanes inside segments** (69,178 samples whose stack holds `translate_segment`, replay thread's lane 0
included), inclusive:

| Where | Share | What |
|---|---|---|
| contended mutexes (`lock_contended` → `WaitOnAddress`) | 23.6 % | below, by lock |
| `mem::room` (uniform buffer ranges, read-only storage to the pool end) | 15.0 % | its 8-window thread cache misses take `g.mem`, which completion holds while it lands write-backs: 11.7 % is the wait |
| `tex_view_uncached` (views missed by a segment's own memo) | 15.0 % | the images' lock (`gate::lock` 9.5 % of all segment time is the images' and the pools' locks) |
| `mem::resolve` | 8.5 % | its 16-entry cache misses take `g.mem` |
| `vertex_input` | 5.4 % | resolves of vertex buffers |
| `lost::checkpoint` (mesh draws, indirect draws, dispatches) | 3.0 % | text formatted under the ring's global lock |
| `nvn::pool::dpool_base` (`pool_bases`, 3+ calls a stage set) | 2.8 % | a global mutex per call |
| `vpool::is_sparse` (every resolve / room) | 2.5 % | a global `RwLock` read per call |
| staging chunks made (`gpu_allocator` `MemoryBlock::new`) | 1.7-2.1 % | a child's second 8 MB staging chunk, freed again when the child is reused (`queue.rs` `make_sub`) |
| Vec growth / heap (`finish_grow`, `process_heap_alloc`) | ~5 % | per-draw / per-set vectors |

**The replay thread** (78,679 samples under `aq_main`): idle 11 %; pool sync (`begin_submit`) 20 %; translation
(`run_segmented` 38 %, of it lane 0's segments 27 %, `segmentable` + `has_rare` stream walks 2.6 %, the pre-pass
~1.5 %); one stream a frame with a rare record (DOWNSAMPLE / RESOLVE_DEPTH / DLSS) translated in segments one after
another on the replay thread (~5 % of the thread); `Gpu::scrub_slots` → `Tables::ids_of` 3.3 % (a walk of every
registered texture ID under the tables' lock for each image that goes); `vkQueuePresentKHR` 5.8 %.

## Changes (as made; each with its off switch where it changes behaviour)

1. `tables.rs`: `Inner::tex_ids`, a reverse index serial -> IDs kept with `tex` (`tex_put` / `tex_del`); `ids_of`
   reads it instead of walking every registration under the tables' lock (pure speed).
2. `nvn/pool.rs`: `dpool_base` reads a lock-free published copy of the (append-only) pool-base table (pure speed).
3. `nvn/vpool.rs`: `with_pool` answers addresses outside every virtual pool's span without the pools' `RwLock`
   (`SPAN`, published under the write lock; `generation` also moves when a range is reserved or dropped).
4. `exec.rs` `room_cached` / `resolve_cached`: per-thread memos of `mem::room(a)` and `mem::resolve(a, n)` keyed by
   `POOL_GEN` (moved by every pool change and every CPU-write deferral) and the virtual pools' generation; failures not
   kept; a resolve that moved the generation not kept. `NXGFX_ADDR_CACHE=0` off.
5. `gate.rs` `view_get` / `view_put` + `exec::tex_view`: a parallel run's segments share their view lookups with the
   *later* segments of the run (the serial order's memo), never from a segment that gave up, never across a change of
   shared state (run epoch). `NXGFX_VIEW_SHARE=0` off.
6. `exec.rs` `segmentable` also reports the rare records (one walk of the stream instead of two).
7. `exec.rs` `run_mixed`: a stream with a rare record is no longer translated wholly on the replay thread; only the
   segments holding a rare record are, the runs between them in parallel, every first-use initialization in the
   parent's upload buffer in the order made (after a redo at a join: the rest sequential with own upload buffers, as
   before). `NXGFX_TRANSLATE_RARE_SPLIT=0` off.
8. `lost.rs` / `exec.rs` `checkpoint`: the checkpoint text is made before the ring's lock (and the old text freed after
   it), in one string.
9. Diagnosis only: `runtime/hostprof.rs` several threads and unwound stacks (`NXRS_HOST_PROF_STACK=1`).

## For the lead: changes needed outside this wave's files (not made)

- **`mem.rs`, the pool lock (`g.mem`) on the translation path.** `room` (8 windows) and `resolve` (16 windows) keep
  small per-thread caches; MKW binds from hundreds of pools a frame, so the lanes miss and take `g.mem`, which the
  completion thread holds while it lands write-backs (and syncs hold for whole passes): in the profile 11.7 % (room)
  plus ~4 % (resolve) of the lanes' segment time was waiting there. `exec::room_cached` / `resolve_cached` now absorb
  the hot callers, but the lasting fix is in `mem.rs`: larger caches (or a lock-free pool map by base), and landing
  write-backs without holding `g.mem` for the whole landing.
- **`queue.rs` `make_sub`, staging of children.** A reused child keeps one 8 MB staging chunk and frees the rest; a
  segment that needs a second chunk allocates a fresh block (`gpu_allocator::MemoryBlock::new`, a `vkAllocateMemory`)
  every frame (1.7-2.1 % of the lanes' time, plus the replay thread's `stage_bytes_fresh`). Keeping two chunks for
  lane children (or one shared pool of chunks per lane) removes it.
- **`queue.rs`, overlap of the next op's pool sync with lane translation** (the replay thread's prep is ~9-12 ms of its
  ~21-24 ms in MKW, mostly `begin_submit`): the largest remaining item on the replay thread, outside this wave.

## Report (agent, 2026-11-26)

### Built

Diagnosis (above, "Profile") kept apart from the corrections ("Changes" 1-9). Why each cuts cost:
- (1) `ids_of` walked every registration under the tables' lock for each image that went (~1 ms a frame on the replay
  thread, and every lane's `tex_at` waited behind it); now a reverse index.
- (2, 3) two global locks taken several times per stage set (`dpool_base` mutex, `vpool` `RwLock`) are gone from the
  common path.
- (4) per-thread memos in front of `mem::room` / `mem::resolve`: ~89 % of the lanes' asks are answered without the pool
  lock (translate line: room 5,800-6,400 found / 720-910 asked mem a frame; resolve 9,100-9,900 / 1,100-1,400).
- (5) shared view lookups: only ~3 % of segment memo misses are found there (tables' generation moves too often in MKW);
  kept (harmless) with an off switch, could be dropped.
- (6, 8, arrays) fewer stream walks, no checkpoint text under the ring lock, no per-draw blend vectors.
- (7) rare-record streams in parallel runs: MKW's sequential stream turned out to be a 1-segment stream, not a rare one,
  so (7) does nothing in MKW (no `run_mixed` there); kept with its switch.
- Instrumentation: translate lines now give address-cache hits, shared-view hits, segments by lane, lane join latency.

### Tests

`cargo test --release -p graphics`: 249 lib tests pass + integration tests (run after the last change).
New: `gate::tests::views_are_shared_with_later_segments_only` (a view is seen only by its own and later segments, never
from a segment that gave up, never after a change at the join; nothing outside a run). `cargo test --release -p runtime
hostprof`: 4 pass, new `samples_carry_the_unwound_stack`. Neither can fail on the old code as written (new functions).

### Verified (all `py scripts/play.py mkw`, window [250, 325] s; replay thread = `replay_ms` + `prep_ms`)

| Run | GPU | Build / switches | FPS / 5% low | replay | prep | lanes CPU | segments wall |
|---|---|---|---|---|---|---|---|
| `055832-v5-mkw` (UPLOAD-SYNC) | 1 | HEAD before | 30.4 / 29.5 | 12.0 | 8.7 | 29.2 | 9.1 |
| `062719-tc-prof-lane1` | 1 | HEAD + 1-thread sampler | 29.9 / 28.0 | 12.5 | 9.4 | 25.8 | 8.8 |
| `065221-tc-r1-mkw` | 1 | changes 1-6, 8 | 30.1 / 28.7 | 11.8 | 12.0 | 26.9 | 8.6 |
| `070945-tc-r2-mkw` | 1 | + 7 | 30.1 / 28.0 | 11.5 | 11.8 | 22.9 | 8.2 |
| `073938-tc-r3-mkw` | 1 | + resolve memo | 23.9 / 14.0 (**contaminated**: AUDIT-FIX's MKW copy ran 27 s beside it) | 20.2 | 13.5 | 23.8 | 22.1 |
| `074518-tc-r3-mkw-cut128` | 1 | MIDCUT/SEG 128 | 29.3 / 23.0 | 11.7 | 12.6 | 30.0 | 7.7 |
| `075056-tc-r3-mkw-off` | 1 | ADDR_CACHE/VIEW_SHARE/RARE_SPLIT off | 30.3 / 29.0 | 11.8 | 10.0 | 19.0 | 8.3 |
| `075657-tc-r4-mkw-check` | 0 | final, `TRANSLATE_CHECK=1` + 7-thread sampler | 30.1 / 27.0 | 12.4 | 13.4 | 25.0 | 8.6 |
| `080731-tc-r4-mkw` | 0 | final | 29.9 / 27.0 | 12.0 | 13.3 | 23.7 | 8.3 |
| `081436-tc-r5-mkw-lanes` | 0 | final + lane counters | 29.3 / 26.0 | 13.2 | 15.3 | 24.6 | 8.8 |

- **No FPS gain; 60 is not reached.** MKW stays at its 30 cap. The lanes' CPU fell (29 → 19-25 ms a frame), but the
  segments' wall time (8.2-8.8 ms) and `replay_ms` (~12 ms) did not move outside the noise. Prep rose from 8.7-9.4 to
  10-15 ms in every run after 06:45: the tree then carried AUDIT-FIX's uncommitted `mem.rs` / `upskip.rs` work and
  other waves' games ran beside mine (machine shared; all numbers indicative).
- **Why the wall did not move** (lane counters, `081436`): lanes join a run 0.011-0.016 ms after it starts; the replay
  thread takes ~20 % of segments; it waits ~1.2-1.7 ms for the others. So the wall is per-segment cost, not wake-up or
  imbalance: a 256-draw segment costs ~2.5-4 ms on a lane against ~1.9 ms serially, because ~20 % of the lanes' time is
  still spent waiting on shared locks (second profile `tc-prof-stack2.txt`: `lock_contended` 20.5 % of segment samples;
  `mem::room` misses 8 %, images lock in `tex_view_uncached` 5.9 %, `resolve` misses ~2 %). Smaller segments (128) cut
  the wall only 8.3 → 7.7 ms while the lanes' CPU rose to 30 ms: contention grows with them.
- The pool lock's holder: the completion thread spends ~10-15 % of its time in `apply_writebacks` / `land` /
  `take_lazy` under `g.mem` (~4 ms a frame), which is what the lanes' room/resolve misses wait for.
- **Correctness:** 0 entry-state differences (18 translate lines, `075657-tc-r4-mkw-check`); stand-ins in the race 0
  (`r4-check`, `r4-mkw-turn`, as the pre-wave `031507-after-mkw-turn`); gate asks 21-59 per 600 frames as before;
  red lines: only the two known (256 timestamp queries at load, `pipeline-crashed.bin`); every run exited normally; no
  device loss. MKW's `pipeline-crashed.bin` unchanged (3 entries, dated 11-24) after the 27 s two-copy overlap.
- **Sheets read:** `065221-tc-r1-mkw`, `073938-tc-r3-mkw`, `075657-tc-r4-mkw-check` (banked turn lit with a Bullet Bill),
  `080235-tc-r4-mkw-turn` (both sheets: t37-t46 across the turn, all lit), `080731-tc-r4-mkw`, `081436-tc-r5-mkw-lanes`:
  boot, title, menus, grid, race, lap 2 correct. The harness's new route check marks the race shots OFF-ROUTE (colour
  distance to the v5 reference at a different spot of the track); the pictures are the track, correctly drawn.
  `r4-mkw-turn` `14-t46` shows a round dark disc on the track beside a large coin: most likely that coin's blob shadow;
  not proven.
- Not run (lead's direction): OoT, DKB, Smash. DKB and Smash were relinked with this wave's engine at 07:40 (before the
  direction); the lead takes those regressions.

### Needs audit

- `gate.rs` `view_get` / `view_put` (segment ordering of shared lookups; epoch rules) — ordering is the dark-turn
  class of fault. Off: `NXGFX_VIEW_SHARE=0`. Its measured benefit is small; dropping it is a fair option.
- `exec.rs` `resolve_cached`: assumes a second `resolve` of the same (a, n) with an unchanged `POOL_GEN` / virtual
  generation does nothing but find the range (no new deferred upload, granules still bound) — the same assumption
  `resolve`'s own cache makes, checked against `mem.rs` (deferrals move `POOL_GEN`), but `mem.rs` is being changed by
  AUDIT-FIX. Off: `NXGFX_ADDR_CACHE=0`.
- `exec.rs` `run_mixed` (upload-buffer order when a stream has a rare record; not exercised by MKW). Off:
  `NXGFX_TRANSLATE_RARE_SPLIT=0`.
- `vpool.rs` `SPAN` fast path and the `generation` bump on reserve/drop.

### Unproven

- No measurable translation-wall or FPS gain on a shared machine; the lead's quiet A/B (`NXGFX_ADDR_CACHE=0
  NXGFX_VIEW_SHARE=0 NXGFX_TRANSLATE_RARE_SPLIT=0` against defaults, same build) is the real measure. The
  non-switchable changes (1-3, 6, 8) are pure-speed.
- Next, by the profile: the pool lock on the translation path and the completion thread's landing under it (`mem.rs`,
  see "For the lead"); then the images' lock in `tex_view_uncached` (view memo invalidated by every tables generation
  bump: a per-ID key would let lookups survive unrelated registrations); per-draw driver calls (~6 % of lane time;
  dynamic state could be skipped when unchanged in a command buffer — every graphics pipeline shares one dynamic list
  except mesh pipelines). OoT's translation (pool snapshots 16 ms a frame of lane time in `v5b-oot`) was not looked at
  (OoT is AUDIT-FIX's).

### Slips

- `cargo test` compiled during the first ~90 s of `070945-tc-r2-mkw` (boot, before its window) under my machine lock.
- Stopping a background waiter killed my own run script mid-report (`070945-tc-r2-mkw`, report regenerated with
  `play.py report`; the cut128 run of that batch never ran); the machine lock was left until I removed it.
- `073938-tc-r3-mkw` overlapped AUDIT-FIX's MKW copy for 27 s (their start, under the then-new rules); numbers unused.
- Runs before 07:55 were on GPU 1 (the brief's GPU); later ones on GPU 0 per the lead.
- The MKW DLL in the build folder is linked with a PDB beside it (`NXRS_LINK_LIBS=-Wl,/DEBUG -Wl,/OPT:REF
  -Wl,/OPT:ICF`, for symbols); a normal relink drops it.

### Questions

None.
