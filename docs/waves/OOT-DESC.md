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

## Changes

1. **Write-back buffers kept for reuse** (`vk/queue.rs` `wb_bufs`): the readback and baseline buffers of a submission's
   write-backs come from a shelf of kept host buffers (power-of-two sizes from 64 KiB, at most 64 MB kept) and go back
   to it at completion, after `apply_writebacks` has landed them, instead of being made and freed per submission.
   Every byte a submission reads back is written by its own copies, so a reused buffer's old bytes are never read.
   `NXGFX_WB_RECYCLE=0`: made and freed each time. The memory line reports made / reused / kept. Separate from
   TRANSLATE-REUSE (`queue.rs` and `perf.rs` only).
