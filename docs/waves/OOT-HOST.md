# OOT-HOST — Ocarina of Time: the GPU reads the per-frame pools straight from guest memory (target 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-28, worktree `D:\Projects\RecompLauncherEngine-oothost`,
branch `oothost` (base `gameplay-harness` `9b5f066`: OOT-REPLAY2's scenario merged, its code D not merged; OOT-LOCK's
code not merged). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only (MK8D for one regression run
at the end if the change is global), GPU 0. Deadline 5 h.

## Plan (written first)

Starting point: OOT-REPLAY2's baseline `D:\Projects\RecompLauncherEngine\build\play\20261128-044556-r2-base`
(`oot-still`, fixed window [660, 820] s, ~2,050 draws): 48.1 FPS / 5 % low 44.0, frame 20.80 ms, replay busy ~20 ms a
frame against 15.8 for one vsync (pool sync 8.0, translation 6.0, lane joins 2.3, pre-pass 1.7). Its pools line at
702 s: 57.75 MB a frame of CPU-written pages found to hold the GPU copy's bytes (hashed, not uploaded), write-backs over
67.5 MB of ranges a frame (141 ranges; 4.2 MB of them changed by the GPU), the largest `0x25ed06000+9.5 MB` x1.0,
`0x29da46000+15.2 MB` x0.5, `0x29e97b000+15.2 MB` x0.5 and several `+6.33 MB` x0.5; the completion thread lands
3.4 ms a frame under the pool lock and waits 4.1 ms for it. These are the pools the CPU rewrites every frame and the
GPU writes every frame: as a Mirror each costs a watch take, a page hash of every rewritten page, a GPU compare of
every write-back range and a landing.

Prior art: DIRECT-MEM (2026-11-26, branch `direct`, not merged) measured `NXGFX_POOL=host` and selections by NVN flags
and size on MKW and OoT (then a different frame: 18.7 FPS, sync dominated by scanning 4,400 pools). Findings kept
here: imports work at 4 KiB on this device; GPU writes into an imported page are not write-watch CPU writes; GPU reads
of imported memory are latency-bound (small per-draw constant pools cost GPU time, large lightly read ones do not);
one 88 MB OoT pool's import was refused (`VK_ERROR_OUT_OF_DEVICE_MEMORY`) at the forest load, where many pools are
made at once. A selection by flags missed the pools that cost the sync; this wave selects by what the pool does.

0. **Baseline** with the diagnostics on: this worktree's dist (`9b5f066`), OoT relinked from it, `oot-still` on GPU 0
   with `NXRS_WAIT_ACCT=1` and `NXGFX_POOL_LOG=1` (one line per pool at creation: base, size, flags, placement), so
   the hot ranges above are named by pool.
1. **Cheap first experiment:** `NXGFX_POOL=host` (every CPU-visible pool imported) on the same build: what breaks
   (import refusals, pictures, device loss), what it gains (pool sync, replay busy), what it costs (GPU frame time).
2. **(a) What a Host pool still pays.** From the source: a Host pool is still scanned by the write watch (needed: the
   texture check must see CPU-written texture bytes), its scanned pages are still hashed by `upskip::prehash`
   (wasted: nothing is uploaded) and its ranges still go through the in-flight write-back guard (wasted: no
   write-back). Make the sync skip the hash and guard work for Host pools; keep the watch for the texture check.
3. **(b) Selection: hot pools switch from Mirror to Host at run time** (`NXGFX_HOST_HOT`, default on once the pictures
   are right). Per watched Mirror pool, the sync counts the frames in which the CPU wrote it and in which a write-back
   landed in it; a pool written by the CPU on most frames of a window (and above a size floor, so per-draw constant
   pools stay mirrored: DIRECT-MEM's GPU-latency finding) and no larger than one buffer is switched at a scanning sync,
   under the pool lock, when no write-back of it is in flight: device-held lazy pages brought back first, its upload
   skip entry dropped, the guest pages imported, the old buffer freed after its last use, `POOL_GEN`/`LAYOUT_GEN`
   moved (resolve caches, descriptor sets and the raw global memory table take the new buffer and device address),
   buffer textures over it requeued. An import refused falls back to staying a Mirror (never a fresh sparse mirror
   mid-game), and the pool is not tried again. Pools holding textures stay eligible (the watch still asks Host pools).
   The rule and its thresholds are a pure function with unit tests; a log line names each switched pool.
4. **(c) Correctness.** Import alignment (`minImportedHostPointerAlignment`, 4 KiB here) and one buffer per import (a
   pool over `maxBufferSize` stays mirrored); a pool whose pages are not one committed allocation is refused by the
   import and stays mirrored; the gmem table lists the imported buffer's device address; ordering: the GPU's writes are
   in guest memory at the fence (end-of-submission HOST_READ barrier), the game's CPU readers wait on its own fences
   as before; CPU writes are visible at submit (coherent import).

Each change behind a switch (rows in `docs/env_switches.tsv`); a change that moves nothing visible (FPS, frame time in
the fixed window, pool-sync ms, replay busy) is taken out. Excluded: Fault A (white house floor), skipped draws or
hidden work, faked completions, weaker checks; OOT-FIX's colours stay; live native NDVE off.

Expected files: `src/graphics/src/vk/mem.rs` (placement switch, sync skips, counters, log line), maybe a new small
module `src/graphics/src/vk/hot.rs` (the selection rule and its tests), `src/graphics/src/vk/upskip.rs` (dropping a
switched pool's entry, if `drop_pool` does not cover it), `src/graphics/src/vk/lazy.rs` (bringing a pool's pages
back, if `touch` does not), `docs/env_switches.tsv`, this document.

Verification: `oot-still` and `oot` runs after the work against the baseline and its diagnostics (no switch-off
controls), every sheet read, report.md's anomaly table and route checks; one final `oot-house --faults` run (Fault A
count as an observation); one `mk8d` run (target 60; was 59.9 / 59.0) since the rule is global. `cargo test --release
-p graphics` with tests for the selection rule and the Host skips. Numbers: FPS mean / 5 % low (`oot-still`, `oot`),
frame ms in the fixed window, replay busy and pool-sync ms, GPU frame time, peak memory, against the baseline and 59.

What would disprove the hypothesis: the imported pools make the GPU frame slower than the sync saves (GPU time over
the frame, or the replay thread's busy ms unchanged because the sync's cost was elsewhere), or the pictures break.
