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
3. **A snapshot's refresh finds its writes by binary search** (`tables.rs` `refresh_ranges`): the descriptor-write log
   (up to 65,536 entries) is in generation order, so the writes since a recycled buffer's generation are its tail
   (`partition_point`) instead of a filter over the whole log, under the tables' lock, at each of 40-60 snapshots a
   frame. Pure speed: the same ranges (`recycled_snapshot_gets_the_writes_since_its_generation` unchanged and passing).

## 3. Profiles after the changes

**Walk, after change 1** (`20261127-011621-or-v1-prof`, same window [855, 880] and threads as section 1). Replay
thread: `Tables::snapshot` **6.4 % -> 0.6 %** (no `desc_buffer` left), waiting for the tables' lock in `tex_at`
**5.0 % -> 0.8 %**; `tex_view` 8.0 -> 3.8 %; `stage_set` 12.0 -> 10.1 %; waiting at lane joins 11.8 -> 6.5 %; idle in
the replay queue 20.1 -> 25.7 %. Lanes: `snapshot` 3.7 -> 0.24 %. Left on the replay thread: pool sync and image prep
(`begin_submit_ex`) 19.5 % (prehash 5.5, write-watch take 3.6, upload skip 2.9, the watch lock 2.2, image walk 7.3),
translation `record` 29.4 % (`bind_sets` 18.6: `stage_set` 10.1, `patch_set` 5.7), contended locks 10.0 %
(`gate::lock` 5.1: the pool and images locks the lanes share), presents 2.8 %. The log's lane line over the walk, per
frame: **lanes' CPU 97-159 ms -> 32-33 ms; the replay thread waiting for the lanes 7.6-9.3 -> 2.7-3.3 ms**.

**House standing, final build** (`20261127-021206-or-v2-oot`, sampler over [700, 720], Link standing in his house,
the replay thread busy ~31 ms a frame). Replay thread: **idle in the replay queue 27.4 %, waiting at lane joins
12.3 %**; pool sync and image prep 20.5 % (prehash waiting for the hash threads 6.0, write-watch take 3.7, the watch
lock 2.6, upload skip 2.6, image walk 8.0); translation `record` 22.0 % (`bind_sets` 14.2 = `stage_set` 8.0 +
`patch_set` 4.4; `tex_view` 2.6, `note_gpu_write_version` 1.9, `gmem::bindings` 1.8); compute dispatches 5.5 %;
contended locks 6.6 %; snapshot 0.14 %; presents 1.1 %. Here the snapshot was never the cost (the house makes few):
of the ~31 ms "busy", ~4 ms is waiting for lanes and ~6 ms the pool sync. The house gains only modestly from this
wave (busy 29-34 ms with it against 35-45 / 37-38 in the same-build controls; section 4).

## 4. Runs (GPU 1; MKW-AHEAD's MKW on GPU 0 throughout)

FPS / 5 % low over [845, 915] are the report's; the other windows from `frames-*.csv` (FPS = frames / time, 5 % low
= 1000 / 95th-percentile frame ms). "busy": the replay thread's busy ms a frame from the `occlusion, per frame` lines
in the window. "shown": share of frames on screen 1 / 2 / 3 / 4+ vsyncs. "snap": snapshot ms summed over the
window's slow-frame lines (all threads). "made": snapshot buffers allocated new in the whole run.

| Run (`build/play/`) | build / switches | walk [845, 915] FPS / 5 % low | walk busy | walk shown | house [600, 830] FPS / 5 % low | house busy | house shown | snap walk / house | made | route |
|---|---|---|---|---|---|---|---|---|---|---|
| OOT-OCC `000105-oc-v5-oot` | base | 16.3 / 6.2 (to 884 s) | 55 | 0/32/51/17 | 28.4 / 20.3 | 31-35 | 0/93/6/1 | 11.4 s / 4.0 s | - | 5/5 |
| `004205-ob-prof-walk` | base, sampled [855, 880] | 14.2 / 6.0 | 54-63 | 0/30/51/20 | 19.2 / 9.9 | 34-47 | 0/38/55/7 | 24.7 s / 38.1 s | - | 6/6 |
| `010048-or-v1-oot` | v1 (1, 2) | 27.9 / 20.0 * | 26 | 0/81/18/0 | 22.1 / 19.2 | 29-34 | 0/30/69/0 | 0.08 s / 1.5 s | 64 | 6/6 |
| `011621-or-v1-prof` | v1, sampled [855, 880] | **24.3 / 19.1** | 33 | 0/56/44/1 | 26.2 / 19.6 | 31-34 | 0/74/26/0 | 0.5 s / 0.6 s | 64 | 6/6 |
| `013154-or-v1-off-oot` | v1, `NXGFX_SNAP_KEEP=32` | **15.0 / 9.0** | 50-54 | 0/26/53/20 | 20.3 / 13.4 | 35-45 | 0/31/63/7 | 22.0 s / 26.4 s | 7,936 | 6/6 |
| `021206-or-v2-oot` | final (1-3), sampled [700, 720] | 28.2 / 20.0 * | 27-30 | 0/91/8/0 | 25.8 / 19.5 | 30-34 | 0/70/30/0 | 0.02 s / 0.23 s | 64 | 6/6 |
| `022729-or-v2-off-oot` | final, `NXGFX_SNAP_KEEP=32` | **18.5 / 8.0** | 32-46 | 0/43/44/13 | 21.7 / 12.5 ** | 25-41 | 0/56/37/7 | 15.8 s / 29.9 s | 11,264 | 6/6 |

\* From the walk's second or third shot on, Link stood by the bed with the camera pressed under the bed's canopy (the
thatch fringe along the top; OOT-DESC's `v5 23-walk3` case) and stayed out of view: a cheaper, near-static picture
(v1 from `22-walk2`, v2 from `25-walk5`; the controls with the old snapshot rule reached the same spot with Link out
of view too, `or-v1-off` at `25-walk5`/`26-walk6`, `or-v2-off` at `23-walk3`, and walked out again). Not comparable with a walking run: the fair walk A/B is the two runs sampled over [855, 880] with
Link walking (`ob-prof-walk` base 14.2 / 6.0, `or-v1-prof` 24.3 / 19.1) and the controls (Link walking: 15.0 / 9.0,
18.5 / 8.0).

\*\* v2-off: from `17-h4` Link stood by the bed's corner, not at the door (the dialogue's A presses fell differently;
route checks still on): its house stretch is a different picture, its busy fell to 25-30 there.

`oot-house --faults` (window [610, 690], Link standing, A every 1.5 s):

| Run | switches | FPS / 5 % low | busy | shown | snap | made | route |
|---|---|---|---|---|---|---|---|
| OOT-OCC `001552-oc-v5-house` | base | 26.1 / 17.0 | 27-32 | - | - | - | 5/5 |
| `014718-or-v1-house` | v1 | 21.6 / 18.5 | 33-38 | 0/32/66/2 | 0.37 s | 128 | 5/5 |
| `015911-or-v1-house-off` | v1, `SNAP_KEEP=32` | 19.9 / 18.0 | 37-38 | 0/17/79/4 | 2.3 s | 4,096+ | 5/5 |
| `024255-or-v2-house` | final (1-3) | 21.4 / 18.0 | 33-35 | 0/29/69/2 | 0.12 s | 64 | 5/5 |

Sheets read: every sheet of every run above (ob-prof-walk, v1, v1-prof, v1-off, v1-house, v1-house-off, v2, v2-off,
v2-house). Pictures correct: the house's occlusion-tested objects (Navi's glow, the door's light, the lamps, pots and
herbs, the window's light shafts) present and steady in all 30 shots of each house run and in every `oot` house and
walk shot; no flicker, popping or pop-in of them. **One wrong frame:** `or-v2-house/24-d21` (676 s): the hanging
herbs, vines and the ceiling's thatch drawn displaced (floating in the room, leaves over Link) for that shot, the
shots before and after correct. That is OOT-FLASH's open "Fault A" (one frame, the same props displaced, its
`064040-hd/16-d13`; per-object data read stale, root cause not proven there), seen before this wave on the base; this
wave's changes touch only the texture pool snapshots (which textures a slot names, not where a mesh goes). Not
counted as caused here; not proven absent either (one shot in ~150 house shots). `06-deku_tree` of v1 shows the forest with a Kokiri girl on a woven pink mat
(correct scene, a different moment of the cutscene than other runs). Fault captures (house runs): the known 16 (the
nightmare's lightning 1-2, the forest load's no-target frames 3-10, cutscene cuts 11-16, e.g. `fault-14-p13854.png`
the cut to the fence), none in the house stretch. Red lines: the known two, plus in v2 at 904.6 s **"pipeline cache
save skipped: its 391544688 bytes could not be allocated ... the file keeps the last save"**: change 2 working: the
same allocation that killed OOT-OCC v5 failed again under the machine's memory pressure, and the game ran on to its
time limit.

## 5. The 391 MB allocation and OoT's 32 GB

- **Why one 391 MB allocation:** Vulkan's `vkGetPipelineCacheData` hands the whole cache as one block; ash allocates it
  in one `Vec`, and a failed Rust allocation aborts. The cache is 391.5 MB because each session loads the file, merges
  it into the live cache and saves all of it again: it only grows (OoT's pipelines from every session since the
  driver's last change). Fixed (change 2) so a failure skips the save. Not done: trimming the file (a fresh cache per
  driver change would need re-warming; out of scope).
- **Why ~28-32 GB private:** the `[vk] memory` lines (v1): 11.8 GB already at boot (the guest's memory, the pools'
  mirrors 3.7-4.5 GB, the loaded pipeline cache and its merged copy); at the forest load (~320 s) **staging jumps from
  72 MB to 3.5 GB in 415 chunks** (8 MB each: every pooled submission keeps one staging chunk, and the load's burst
  made ~415 submissions that stay pooled: `queue.rs` `free_subs` is never trimmed); textures 3.4 -> 6.3 GB in the
  house, scratch 0.8 -> 2.6 GB (64 MB chunks, also kept per pooled submission), this layer's system memory 4.1 GB,
  the write-back shelf 0.25-0.5 GB. Not changed: trimming the pooled submissions (keep N, free the rest with their
  staging and scratch) would return ~5 GB, but freeing a pooled `Sub` safely is more than this wave's time allowed;
  left as the next step.

## 6. What is left between this and 59

- **59 needs the replay thread under ~15.8 ms; it is ~27-30 ms on the walk now (was 50-63) and ~30-34 in the house
  standing (unchanged in substance).** Measured shares of what remains (house profile, section 3): waiting for the
  lanes at joins ~12 % (~4 ms), the pool sync ~20 % (~6.5 ms: prehash waits, write-watch takes, the upload skip,
  the image walk), translation ~22 % (~7 ms, sets), dispatches ~5 %; and 27 % of the thread is idle in its queue, so
  in the house the frame step's 2-vsync hold is no longer set by the replay alone (the guest's frame and the pacing).
- The pool sync is USE-SYNC's ground (use-driven sync from fault stamps, REF-ARCH 1); lane joins and translation
  belong to MKW-AHEAD (translate-ahead) and SET-FAST. TRANSLATE-REUSE's change 7 (`NXGFX_VIEW_ID_KEY`, views keyed per
  texture ID, on branch `reuse`, not merged) matters for OoT: its `tex_view` memo is keyed by the broad tables
  generation, which OoT moves 40-60 times a frame, so the memo rarely hits.
- **The box tests are not a cost of their own:** a reset and a report are an `end_query` / `begin_query` pair and a
  `Post` each; the bounding-box draw is an ordinary draw (set patch, dynamic state). Their cost is OoT's per-draw cost
  times ~1,600, i.e. the set work above; making them cheap is the general per-draw work, not an occlusion fast path.
  No change made there.

## Report (agent, protocol §5)

Branch `ootbox` (base `ootocc` `c32f047`): `f9b3a6e` (changes 1, 2, plan), `2db18a4` (change 3), docs after.

### 1. Built

Diagnosis (section 1, written before any change ran): the walk's replay thread lost its time to texture pool
snapshots: past 32 kept buffers every snapshot allocated a new 10 MB host-visible descriptor buffer and copied the
whole shadow, under the tables' lock that every texture and sampler lookup takes (6.4 % of the replay thread + 3.7 %
of each lane directly, 7.5 % more waiting for that lock; up to 400 ms of snapshots in one frame). The box tests are
not a separate cost. Corrections: snapshot buffers kept for reuse up to 256 (`NXGFX_SNAP_KEEP`, 32 = old); the
pipeline-cache save's buffer allocated fallibly (`NXGFX_PCACHE_TRY_ALLOC`, 0 = old); the refresh's write lookup by
binary search (pure speed). Files: `src/graphics/src/vk/tables.rs`, `pcache.rs`, `docs/env_switches.tsv`.

### 2. Tests (`cargo test --release -p graphics`: lib 277 passed, 6 ignored; the other targets pass)

- `pcache::tests::an_unallocatable_save_buffer_is_an_error` (new): an impossible size is an `Err`, not an abort; a
  normal size reserves. New function (no old code to fail on); ash's path aborts by construction.
- `tables::tests::recycled_snapshot_gets_the_writes_since_its_generation` (existing): passes with the binary search
  (the same ranges, the log's overflow case included).
- Change 1 is a count (no new logic); shown by the runs (64 buffers made a run against 7,936).

### 3. Verified

Section 4. Walk (Link walking, sampled runs): **24.3 / 19.1 FPS against 14.2 / 6.0** on the base build; the same-build
controls with `SNAP_KEEP=32`, Link walking: 15.0 / 9.0 and 18.5 / 8.0. Replay busy on the walk 26-33 ms against
50-63 (controls 32-54); snapshot buffers made new 64 a run against 7,936-11,264; lanes' CPU on the walk 32-33 ms a
frame against 97-159. The unsampled final runs' walk (27.9-28.2 / 20.0) had Link out of view by the bed (not
comparable). House standing: busy 29-35 ms against 35-45 in the controls; `oot-house` 21.4-21.6 / 18.0-18.5 against
19.9 / 18.0 (control): FPS there is set by the frame step's 2-or-3-vsync choice, and the replay thread is 27 % idle in
the house. Route checks on in every run (6/6, 5/5). Every sheet and the fault captures read: correct except one
frame of OOT-FLASH's known Fault A in `or-v2-house/24-d21` (section 4). The pipeline-cache allocation failure
recurred in v2 at 904.6 s and was skipped instead of aborting the game (change 2 shown in the game).

### 4. Unproven

- Perf on a shared machine (MKW-AHEAD's MKW on GPU 0 throughout): the lead's quiet runs decide.
- 59: the replay thread is ~27-34 ms; section 6 names what is left and whose ground it is.
- Link out of view by the bed from walk shot 2-5 on in v1 / v2, and at one or two shots in both same-build controls
  (old snapshot rule): judged the known camera-under-the-canopy spot, not this wave's change; that Link is drawn
  there is not shown (the camera never frames him in that spot). Because v1 / v2 stayed there, their unsampled walk
  FPS is not a fair number; the sampled pair and the controls are.
- The 5 GB of pooled staging / scratch (section 5): not trimmed.
- One displaced-props frame (OOT-FLASH Fault A) in `or-v2-house/24-d21`: judged the known open fault, not shown to
  be unrelated by an A/B (it is one frame in ~150 house shots; the controls showed none in 60).
- `NXGFX_SNAP_KEEP` keeps up to 256 x 10 MB of host-visible (BAR) memory; OoT used 64-128.

### 5. Slips

- Relinks ran this worktree's `dist\nxrs\nxrs.exe --select-title ... --build` with `NXRS_LINK_LIBS` set for a PDB
  (as OOT-DESC). Builds and test compiles ran during the boot of some runs (never inside a window), at below-normal
  priority.

### 6. Questions

None.
