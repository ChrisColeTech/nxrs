# OOT-DESC — what limits Ocarina of Time's frame after SCAN-COST, and its largest cost cut

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~17:40, worktree
`D:\Projects\RecompLauncherEngine-ootdesc`, branch `ootdesc` (base `gameplay-harness` `0e04404`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1. Deadline 6 h (critical-path step 2 h).

## Goal and baseline

OoT presents ~19.4 FPS (target 59). SCAN-COST `163048-sc-v4-oot` (window 845-915 s, Link in his house), ms a frame:
frame ~51; replay (translation) 18.9; prep 16.1 (pool sync 11.9 = lock 2.5 / scan 2.6 / per-pool pass 5.9);
completion write-back 9.7; GPU ~9.5; `descriptor_ms` ~19. Replay + prep is 35 ms of the 51 ms frame, so the frame is
held by something those counters do not show.

What the counters are (read before measuring):

- `descriptor_ms` is `perf::C::SetNs`, added by **every** thread that makes descriptor sets: the replay thread and
  the translation lanes together (`exec.rs`), so it is CPU summed over threads, not time on one thread's path. In the
  lead's `131123-lead-oot-merged` log the lanes alone spent 34.7 ms a frame of CPU together while the replay thread
  waited 2.5 ms a frame for them.
- `replay_ms` / `prep_ms` are timed inside `run_stream_due` on the replay thread; what the replay thread does
  outside them (`exec::end`, `submit_sub`, presents, parked-work retries, waits) is in neither.
- The game is held to the replay thread by `aq_wait_room` (63 ops or 2 presents queued); that log shows the replay
  queue 54 ops behind at 881 s, and the guest's busiest threads at ~10 % of a core: the guest is mostly waiting.

## Plan

1. **Critical path (diagnosis, no behaviour change, 2 h).** One `oot` run on this branch's build with the host
   sampler over the house: `NXRS_HOST_PROF=<start>,<s>`, `NXRS_HOST_PROF_THREAD=vk replay,vk translate,nvn-completion,
   guest`, `NXRS_HOST_PROF_STACK=1`, the game DLL relinked with a PDB (`NXRS_LINK_LIBS`) so stacks symbolize. From it:
   where the replay thread's wall time goes (prep, translation, waits for lanes, submit, present, idle), whether the
   game's render thread blocks in `aq_wait_room` (replay-bound) or on fences (`nvnSyncWait`: GPU / completion-bound),
   and what the completion thread holds. Written below before any code change.
2. **Fix the largest cost** on that path, in source, behind a switch where it changes behaviour.
3. **Verify:** `cargo test --release -p graphics`; `oot` before/after (FPS / 5% low vs 59, ms breakdown, route checks);
   one `oot-house --faults`; every sheet and report read.

## Critical path (written 18:10, before any code change)

**Run** `build/play/20261126-174908-od-prof1-oot` (base build `0e04404` + nothing, relinked 17:48 with a PDB beside the
DLL; GPU 1; MKW of TRANSLATE-REUSE on GPU 0 ran beside it): host sampler over [860, 885] s (Link in his house),
nine threads with unwound stacks, 29,568 samples each (the sampler takes every thread once a round, in a fixed order,
so each sample's thread is its index modulo 9). Shares below are of each thread's samples, i.e. of wall time.

1. **The frame is quantized by the frame step.** OoT runs under the frame-step pacer (`os/fstep.rs`), max hold 3
   vsyncs: in the house it holds **3 vsyncs (20 FPS) for 1,734 of 1,800 frames**, with ~70 % of them late (cost over 3.1
   vsyncs) and every trial at 2 vsyncs failed (`frame step: ... held 1 / 2 / 3 vsyncs 66 / 0 / 1734 frames, ... 42
   failed trials`, lead's `131123` log at 878 s). The cost it holds against is the game's frame from acquire to the
   next acquire, less the pacing wait. So 19.4 FPS = the 20 FPS step less late frames; **30 FPS needs that cost under
   ~31.7 ms (0.95 x 2 vsyncs), 60 under ~15.8 ms.**
2. **What the game's frame waits for: the replay thread.** OoT reports samples-passed (occlusion) counters, so every
   present waits in `aq_wait_room` until the replay thread has replayed everything queued (`occl_sync`, MK8D's fix).
   The render thread `guest 0x1004` is **82.7 % in `aq_wait_room` from `nvnQueuePresentTexture`**, 1.6 % submitting;
   the main thread `guest 0x1001` is 69 % blocked on guest sync objects, 7.5 % in the acquire (pacing), 5.9 % in
   `nvnSyncWait`. The guest's own CPU is small. **The replay thread is the critical path**: it is idle only 8.9 %.
3. **The replay thread's wall time** (inclusive): pool sync and texture prep (`begin_submit_ex`) 24.1 % (of it the
   upload skip's page hashing 10.0 %, write-watch take 4.1 %, the pool lock wait 3.4 %); translation
   (`run_segmented` / `record`) ~29 % with 5.3 % waiting for the lanes at a run's join; **`submit_sub` 16.8 %, of it
   `Gpu::buffer` 15.0 % = `gpu_allocator` making a new memory block (`MemoryBlock::new` -> `vkAllocateMemory`,
   13.8 %)** for the two host buffers every submission with write-backs makes (`write-back`, `write-back base`); presents
   4.2 %. Waiting on contended locks 18.3 % in all, mostly the pool lock (`g.mem`) taken by `note_gpu_write`
   (3.7 %), `resolve_in` (4.4 %), `begin_submit_ex` (3.4 %), and `gmem::bindings` 1.8 %: the completion thread holds
   `g.mem` while it lands write-backs.
4. **The completion thread** is busy ~32 %: `apply_writebacks` 18.7 % (under `g.mem`), `free_buffer` of those same
   two buffers 5.8 % (`vkFreeMemory`), waiting for the GPU 26 %, idle 42 %. **The lanes** are idle 92.6 %: descriptor work
   (`stage_set` 3.7 % of lane time, `descriptor_ms` ~19 ms a frame summed over threads) runs in parallel and is not on
   the path; on the replay thread `bind_sets` is 19.3 % (its own share of segments plus the serial streams).
5. The GPU (9.5 ms a frame) is not on the path.

**Largest single cost on the path:** a fresh `vkAllocateMemory` (and later `vkFreeMemory`) of host memory for the
write-back buffers, ~15 % of the replay thread's wall time (~7 ms of a ~51 ms frame) plus ~6 % of the completion
thread's (which holds the pool lock the replay thread then waits for). Next: the pool lock held by landings
(~10 %), the upload skip's page hashing on the replay thread (~10 %).

## Runs (GPU 1; `oot`, window [845, 915] s unless said; other waves' MKW and web runs shared the machine)

- `20261126-174908-od-prof1-oot` (base `0e04404`, PDB relink): the profile above; FPS 18.6 / 13.0 in the window (the
  sampler ran over [860, 885]); [845, 858] unsampled: 18.9 FPS, frame 53.0 ms, replay 21.1, prep 18.5, GPU 9.2;
  completion write-back 0.97 MB in 11.9 ms; frame step held 3 vsyncs for 1,715 of 1,800 frames. 6/6 on route.
- `20261126-180856-od-wb1-oot` (change 1, 64 MB cap): stopped by PID at ~240 s after its log showed 26 % reuse.
- `20261126-181606-od-wb2-oot` (change 1, final cap): 4 buffers made, all later write-backs reused; **its harness
  process ended at 317 s** (last step 96, no report; started with `Start-Process` from a shell that went on to build),
  the game ran on without inputs and was stopped by PID. No window.
- `20261126-183520-od-v3-oot` (changes 1-3, the landing's compares on the shared hash threads): **OFF-ROUTE at `14-h1`**
  (distance 0.31, limit 0.3; the other five on route), window FPS invalid; [845, 895]: 14.6 FPS with 28 frames of
  200-950 ms (replay normal, the game's frame idle: the machine-side stall SCAN-COST saw, while `usesync` ran MKW
  runs 18:35-18:48), the replay thread's copy and hash 4.8 -> 6.2 ms a frame: the landing's compares took the
  `vk page hash` threads from the replay thread's sync. Corrected (change 2 off those threads). Its profile was not
  written (the run ended at 913 s, inside the sampled stretch).
- `20261126-185414-od-v4-oot` (changes 1-3, change 2 on its own threads; profile [700, 720] s, the house before the
  window): 6/6 on route, **17.8 / 13.0 FPS**, frame 55.4 ms, replay 22.2, prep 15.2 (pool sync 11.9 = lock 1.9 /
  scan 3.0 / pass 5.8), completion write-back 1.1 MB in **6.2 ms** (was 11.9), GPU 9.9; still held 3 vsyncs for all
  1,800 frames. Profile, replay thread: `submit_sub` 15.3 % of which **12.3 % still making buffers**: the shelf's
  256 MB cap held while 64 MB write-backs were in flight (~one made a frame in the house); contended locks 18.3 ->
  6.8 %; idle 20 %, waiting for the lanes at joins ~6 %. Completion: landing compares before the lock 12.7 %, under
  it ~13 %. Sheets (3) read: all correct.
- `20261126-191140-od-v5-oot` (+ 1 GB shelf, merged dirty ranges; profile [700, 720] s): 6/6 on route, **17.5 / 11.0
  FPS**, frame 56.8 ms, replay 27.7, prep 14.7 (pool sync 11.1 = 1.5 / 2.6 / 5.8), write-back 6.1 ms, GPU 9.7;
  `descriptor_ms` 43 (lanes; other waves compiled throughout). Profile, replay thread: `submit_sub` **2.4 %** (no
  buffer made in the house), contended locks 9.6 %, **idle 16.5 % in the replay queue and 14.1 % waiting at
  `par::run` joins for the lanes**, `begin_submit_ex` 25 % (upload skip 10.4 %: its serial page hashing 5.5 %, image
  prep 8.2 %: two walks over all ~6,700 images per submission), translation `record` 25 %. Sheets read: correct;
  `23-walk3` shows the camera pressed into the bed's canopy beside the door (stretched thatch along the top; the
  walk's timing differs run to run, no other shot shows it): judged camera clipping, to be watched in the house runs.
- `20261126-192933-od-v6-oot` (+ prehash of the scan's pages; profile [700, 720] s of the replay thread and lanes):
  6/6 on route, but **12.2 / 7.0 FPS** with the translation inflated from 840 s on (replay 50-61 ms, `descriptor_ms`
  117-166 summed over the lanes, in 15 s bins; 700-840 s: replay 23-31, descriptor 25-40) while other waves compiled;
  [700, 720]: 18.6 FPS, frame 53.8, replay 23.3, prep 14.0. Profile: the replay thread **idle 30.7 % in the replay
  queue** and 10.2 % at lane joins; `page_hash` still 4.9 %: prehash hashed almost nothing (`taken` was empty at most
  syncs, see change 5); the lanes 88.6 % idle.
- `20261126-194802-od-v7-oot` (+ pending pages prehashed, one image walk): **invalid, stopped by PID at ~750 s**: at
  555.8 s the swapchain was recreated at 1164x764 (the window was resized from outside), and from there every present
  took ~500 ms (2.0 FPS; the replay thread 91 % inside `vkQueuePresentKHR`). Not this wave's change (the window's size
  is not touched by it); its profile is of the throttled presents only.
- `20261126-200158-od-v8-oot` (changes 1-7, `f1f3ead`): **6/6 on route, 21.0 / 10.0 FPS** in [845, 915] (frame 48.2
  ms, replay 23.6, prep 12.7, pool sync 9.7 = 1.6 / 2.5 / 4.3, write-back 1.0 MB in 5.3 ms, GPU 9.1); [600, 830] (the
  house before the walk): **22.2 FPS** against 18.0 in the lead's base run `131123` over the same stretch. The frame
  step now holds **2 vsyncs for 540-1,111 of each 1,800 frames** in the house (base: 0 of 1,800); the frames' cost
  in the house: median 1.76-1.85 vsyncs, 90 % 2.2-4.1, **56-67 % fit 2 vsyncs**. Sheets (3) read: all correct. Red
  lines: the known three, plus `audio: no output device` at 5.9 s (the machine's audio device, not graphics).
- `20261126-201814-od-v8-seg128-oot` (v8 + `NXGFX_TRANSLATE_SEG=128 NXGFX_TRANSLATE_MIDCUT=128`, an experiment on the
  replay thread's lane-join waits): **stopped by PID at ~690 s, worse**: 17.1 segments a frame (9.7 at 256) but the
  lanes spent **193-198 ms a frame of CPU** (49 at 256) and a run's wall rose 18.0 -> 38 ms a frame; house cost median
  2.17 vsyncs (1.57-1.85 at 256). The cost is per segment, not per draw (not looked into further: most likely each
  segment's first sets and staging made again, TRANSLATE-REUSE's ground). Not adopted; the defaults stay.
- `20261126-202915-od-v8-off-oot` (control, the same build with `NXGFX_WB_RECYCLE=0 NXGFX_LAND_PRE=0
  NXGFX_NOTE_MEMO=0 NXGFX_PREHASH=0`; changes 4 and 6 have no switch and stay on): 6/6 on route, **18.9 / 15.0 FPS**
  in [845, 915] (frame 53.0, replay 20.6, prep 14.5, write-back 9.6 ms, re-arming under the lock 3.7 ms), [600, 830]
  19.9 FPS; **held 3 vsyncs for 1,790-1,800 of each 1,800 frames from 480 s on, house cost median 2.25-2.56 vsyncs,
  0-1 % fitting 2** (v8 with them on: median 1.76-1.85, 56-67 %). Sheets (3) read: correct.

## Changes

1. **Write-back buffers kept for reuse** (`vk/queue.rs` `wb_bufs`): the readback and baseline buffers of a submission's
   write-backs come from a shelf of kept host buffers (power-of-two sizes from 64 KiB, at most 64 MB kept) and go back
   to it at completion, after `apply_writebacks` has landed them, instead of being made and freed per submission.
   Every byte a submission reads back is written by its own copies, so a reused buffer's old bytes are never read.
   `NXGFX_WB_RECYCLE=0`: made and freed each time. The memory line reports made / reused / kept. Separate from
   TRANSLATE-REUSE (`queue.rs` and `perf.rs` only). First cut (64 MB cap) reused only 26 % (`od-wb1-oot`, stopped):
   two 32 MB write-backs (size-0 storage buffers reaching their pool's end) filled the cap; now 256 MB and at most 64
   buffers of a size: 4 buffers made in a whole run, every later write-back reuses them.
2. **A landing's compares and hashes before the pool lock** (`upskip::landing_pre` / `landed_with`, `mem::apply_writebacks`):
   the completion thread compared each write-back page with its baseline and hashed the changed ones (~59 MB a frame,
   "re-arming 3.6-3.7 ms" a frame) while holding `g.mem` and the watch lock, which the replay thread's syncs,
   resolves and GPU-write notes wait for. That work reads only the write-back's own readback and baseline buffers
   (nothing changes them after completion), so it now runs before the lock; under the lock the same decisions are
   applied from it (a page the GPU left alone without a kept hash is hashed there, as before). `NXGFX_LAND_PRE=0`: as
   before.
3. **Repeated GPU-write notes skip the pool lock** (`mem::note_gpu_write_version`, `note_memo`): a recording noting the
   same range again (same pool generation, identity) already holds that write-back; the note returns before `g.mem`.
   Keyed by the recording's serial (unique per `make_sub`, children included), so no other recording matches.
   `NXGFX_NOTE_MEMO=0`: as before.
4. **Image prep merges the dirty ranges** (`image.rs` `image_begin_submit`, `merge_ranges`): the pool sync hands each
   pool's CPU-written ranges in pool (hash-map) order, so the "sorted" test failed at every sync and the slow path ran:
   a `by_addr` range scan per dirty range over the largest texture's span, and for every texture hit a filter over
   all dirty ranges (6 % of the replay thread's samples, self time, in `od-v4-oot`'s profile). They are sorted and
   merged first (the same bytes, each once), so the sorted paths run.
5. **A sync's written pages hashed at once** (`upskip::prehash`, `mem::begin_submit_ex`): after the scan, every page it
   found written is hashed on the `vk page hash` threads in one job; `upload_changed` decides on those hashes instead of
   hashing each small range's candidates on the replay thread (ranges under 4 MB were hashed there alone: `page_hash`
   8 % of the replay thread). The hashes only decide skips, are taken after the scan reset the pages' write watch (a
   later store is found by the next scan, as with `par_hashes`), and are dropped at the end of the sync.
   `NXGFX_PREHASH=0`: as before. First cut hashed only the scan's own pages (`od-v6-oot`: prehash idle, the serial
   hashing unchanged): most of OoT's written pages reach a sync through the pool walker's walks, drained into the
   pools' `pending`, with no scan of the sync's own; the pending pages of the pools the sync visits are hashed too.
6. **One walk over the images per submission** (`image_begin_submit`): the sparse-texture check and the collection of
   images due were two walks over every image (~6,700) at each of ~11 submissions a frame; one walk does both.
7. **Diagnosis:** the frame step's summary line (`os/fstep.rs`, every 1,800 presents) gives the frames' cost: median,
   90th percentile, and the share fitting 1 / 2 / 3 vsyncs.

Files: `src/graphics/src/vk/queue.rs` (1), `upskip.rs` (2, 5), `mem.rs` (2, 3, 5), `image.rs` (4, 6), `perf.rs` (1's
memory-line part), `src/os/src/fstep.rs` (7), `docs/env_switches.tsv`. None in `exec.rs`, `gate.rs` or `tables.rs`
(TRANSLATE-REUSE's files): separable from that wave.

## Report (agent, protocol §5)

Branch `ootdesc` (base `0e04404`): `b66f5c1` (diagnosis + 1), `712fd85` (1: cap), `74f0437` (2), `91c0ace` (3),
`43274c3` (2: own threads), `b35c65e` (1: 1 GB, 4), `7c65fbc` (5), `bca4cbf` (6), `56e93a4` (5: pending pages),
`f1f3ead` (7), docs after.

### 1. Built

**Diagnosis** (section "Critical path", written before any change): OoT's frame is quantized by the frame step (hold
3 vsyncs = 20 FPS unless the frame's cost fits ~1.9 vsyncs); the game's render thread spends 83 % of its time in
`aq_wait_room` at each present (occlusion results in use: every present waits until the replay thread has replayed
everything), so the frame is the replay thread's work; that thread was idle 9 %, and its largest single cost was a
fresh `vkAllocateMemory` for each submission's write-back buffers (15 %), then waits for the pool lock the completion
thread held while landing write-backs (~10 %), the upload skip's serial hashing (~8 %) and walks over every image
(~6 %). `descriptor_ms` (~19 ms) is CPU summed over the replay thread and five lanes; the lanes were 92 % idle, so it
was not the path.

**Corrections** (section "Changes" 1-6, each a cut of the replay thread's work or of the time it waits): write-back
buffers kept for reuse (no `vkAllocateMemory` / `vkFreeMemory` per submission); a landing's page compares and hashes
made before the pool lock (the lock held ~3.7 ms a frame less); repeated GPU-write notes without the pool lock; the
dirty ranges sorted and merged before the image check; a sync's written pages (scan and pending) hashed at once on
the hash threads; one walk over the images per submission. 1-3 and 5 have off switches (`NXGFX_WB_RECYCLE`,
`NXGFX_LAND_PRE`, `NXGFX_NOTE_MEMO`, `NXGFX_PREHASH`); 4 and 6 are pure speed (the same images found, the same bytes).
7 is diagnosis.

### 2. Tests (`cargo test --release -p graphics`: lib 274 passed, 6 ignored, the other targets pass; `-p os`: 167
passed, 1 ignored, the other targets pass)

- `queue::wb_bufs::tests::the_smallest_fitting_buffer_is_reused_and_the_cap_holds`: the smallest kept buffer holding
  the size is handed out; nothing past the byte cap or the per-size count is kept; size classes.
- `upskip::tests::a_landing_compared_beforehand_decides_as_one_that_compares_itself`: on a pool with unchanged pages
  (with and without a kept hash), changed pages, partial pages at both ends and a page restamped by a later note, a
  landing with `landing_pre` leaves exactly the page states and unchanged-page list the old path does; small and
  large (threaded) write-backs.
- `upskip::tests::prehashed_pages_decide_the_skip_of_their_sync_only`: prehashes decide the skip of their sync; a page
  changed after its prehash is decided on the prehash (as a `par_hashes` hash); gone after `prehash_clear`.
- `mem::note_memo::tests::only_the_same_note_of_the_same_recording_is_found`: a note is found again only with the
  same recording, pool generation, range and identity; 16 kept.
- `image::merge_tests::ranges_are_sorted_and_merged`: out-of-order, overlapping, nested, touching and empty ranges.
- `fstep::tests::cost_text_gives_median_tail_and_fits`.
- All are new functions (no old code to fail on); the landing test compares the new path against the old one.

### 3. Verified (GPU 1; `oot`, window [845, 915] s, target 59; other waves compiled and ran MKW / web runs throughout)

| Run (`build/play/`) | build | FPS / 5% low | frame | replay | prep | write-back ms | house: held 2 of 1,800 | house cost median (fit 2) | route |
|---|---|---|---|---|---|---|---|---|---|
| `174908-od-prof1-oot` | base (sampled 860-885) | 18.6 / 13.0 | 53.0* | 21.1* | 18.5* | 11.9 | 0 | - | 6/6 |
| `202915-od-v8-off-oot` | final, switches off | **18.9 / 15.0** | 53.0 | 20.6 | 14.5 | 9.6 | 0 | 2.25-2.56 (0-1 %) | 6/6 |
| `200158-od-v8-oot` | **final** | **21.0 / 10.0** | 48.2 | 23.6 | 12.7 | 5.3 | 540-1,111 | **1.76-1.85 (56-67 %)** | 6/6 |

(* [845, 858] unsampled.) Over [600, 830] (Link in his house before the walk): final 22.2 FPS, control 19.9, the
lead's base `131123` 18.0. SCAN-COST's `sc-v4` 19.4 / 17.3. **Target 59 not reached; 30 not held**: the cost now
straddles the 2-vsync line (median ~1.8 vsyncs), so the frame step alternates 2 and 3 vsyncs; the 5 % low is the
3-vsync frames and stalls. Sheets read (all three) of prof1, v4, v5, v8, v8-off, v8-seg128 (to its stop): correct;
`v5 23-walk3` judged camera clipping (see the run note). Anomalies: the known set (one stand-in at the forest load,
no-target draws, the crashed pipeline, occlusion queries used up); new red line only `audio: no output device`
(machine). Route checks on in every completed run except `v3` (`14-h1` 0.31).

`oot-house --faults`, `20261126-204440-od-v8-house` (final build): completed, **5/5 on route**, house window [610, 690]
**19.3 / 18.0 FPS** (SCAN-COST's house3: 19.3 / 18.0): in this scenario's stretch (Link standing, A every 1.5 s) the
cost rose to a median of 1.96-2.03 vsyncs (31-45 % fitting 2), so the frame step held 3 vsyncs for 1,298-1,660 of
1,800 frames; the gain of the `oot` house stretch does not show here. All 33 house shots read (x5, h1, d01-d30, last):
correct. 16 fault captures, all read: nightmare lightning (1-2), the known no-target black frames at the forest load
(3-10), scene cuts in the forest and the wake-up cutscene (11-16): the same set as SCAN-COST's house1/house3, none in
the house stretch. No new red line.

### 4. Unproven

- Perf on a shared machine: the lead's quiet runs decide; the control (same build, switches off) is the fairest A/B
  here. The windows also caught two machine-side events (frames of 0.3-1 s with the guest idle in `v3`; a resized
  window throttling presents to 2 FPS in `v7`).
- 30 FPS needs the cost under ~1.9 vsyncs in most frames. What remains on the replay thread (v6 profile): translation
  `record` ~25 % (lanes idle ~89 %, the replay thread ~10 % waiting at joins: a run's wall is its slowest 256-draw
  segment; smaller segments cost far more per segment, `seg128`), the pool sync ~20 % (write-watch takes ~4 %, the
  upload skip's step hashing), presents ~5 %, image prep ~4-6 %. Next: translation reuse (TRANSLATE-REUSE), and the
  per-segment set work.
- The occlusion sync (`occl_sync`) serializes the game with the replay thread at every present; letting OoT's presents
  run one frame ahead would overlap the guest with the replay, but its occlusion results would come a frame later
  (MK8D's black stretches were the reason it exists): not changed, the owner's or lead's call.
- Write-back ranges of 32-64 MB a frame (readback and baseline each) are copied back and compared although ~1 MB a
  frame lands; which bindings note them was not looked into.
- Change 1 keeps up to 1 GB of host-visible write-back buffers (HOST_LIVE counts them; 4-6 x 64 MB in flight in OoT).

### 5. Slips

- `od-wb2-oot`'s harness was started with `Start-Process` from a PowerShell call and ended at 317 s without a report;
  its game ran on without inputs and was stopped by PID. Later runs used the normal background shell.
- Stopped early by PID (harness, then game): `od-wb1-oot` (a fix was already known), `od-v7-oot` (throttled presents),
  `od-v8-seg128-oot` (the experiment was clearly worse).
- Builds and lib-test compiles ran during the boot of `od-wb2`, `od-v3`, `od-v6` and `od-v8-off` (never inside a
  window [845, 915]).
- Relinks ran this worktree's `nxrs-build` directly with the launcher's command line (as SCAN-COST did), with
  `NXRS_LINK_LIBS=-Wl,/DEBUG -Wl,/OPT:REF -Wl,/OPT:ICF` (a PDB beside the DLL, for the profiles). The OoT DLL in the
  library is this branch's final build with that PDB.

### 6. Questions

None.
