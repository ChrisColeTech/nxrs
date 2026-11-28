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

## Baseline (`build/play/20261128-074701-host-base`, `9b5f066`, this worktree's dist)

`oot-still`, GPU 0, `NXRS_WAIT_ACCT=1`, `NXGFX_POOL_LOG=1`. Completed, normal exit, on route (0.005-0.046 at x5 / h1 /
h4 / h7 / walk1 / last), every sheet read (title, story, nightmare, the Deku Tree talk, forest load, Navi, Link in his
house at every check shot, the walk; OOT-FIX's colours). Anomaly kinds as OOT-REPLAY2's baseline. Machine: this agent
ran unit-test compiles at 07:49-07:53 (boot, before both windows); nothing else of its own.

| window | FPS / 5 % low / min | frame ms | GPU ms | replay + prep (p50) | pool sync | replay busy | draws |
|---|---|---|---|---|---|---|---|
| fixed [660, 820] | **46.7 / 41.6 / 30.0** | 21.39 | 9.19 | 18.56 (18.68) | 8.46 | 20.55 | 2,094 |
| walk [845, 915] | 52.9 / 31.0 / 25.5 | 18.67 | 8.31 | 14.57 (14.43) | 6.66 | 16.72 | 1,072 |

Peak memory 29,052 MB. Pools lines over the fixed window (means): landings first 1.84 ms, sync lock wait 1.02,
write-watch scan 2.32, per-pool pass 3.26; completion: pool lock 3.97 ms, landed 3.44; write-back 2.28 MB a frame in
8.16 ms; upload skip: 57.49 MB a frame of CPU-written pages with the GPU copy's bytes, copy and hash 2.45 ms; write-backs
compared on the GPU over 67.5 MB of ranges a frame; uploads 2.61 MB a frame; the game's fence wait 6.12 ms a frame.

**Which pools.** The pool log names the largest write-back ranges as whole pools: `0x4ded06000` (9.5 MB, x1.0 a
frame), `0x51da46000` and `0x51e97b000` (15.2 MB each, x0.5), `0x51c0ee000`, `0x51cd9a000`, `0x51d3f0000` (6.3 MB each,
x0.5), all flags 0x24 (CPU cached, GPU cached), made at 6.3-6.9 s. 3,295 MB of OoT's pools carry the same flags, so
the flags cannot choose them; what they do (written by the CPU and by the GPU on most frames) can.

## Experiment: every pool imported (`NXGFX_POOL=host`), `build/play/20261128-080501-host-all`

Build `dcd548e` (the hot rule has nothing to do here: every CPU-visible pool is Host from its creation), same
diagnostics. **Disproves importing everything:**

- 131 imports refused (`VK_ERROR_OUT_OF_DEVICE_MEMORY`, red lines at 9-32 s: pools of 1-48 MB, flags 0x12 / 0x22 /
  0xa2 / 0x24, among them the opening's 48 MB write-back pool `0x195eea000`), mirrored instead; 3,635-3,699 pools
  (2.8 GB) imported.
- GPU time: the opening 13.5-15.8 ms a frame against 10.0-10.2 in the baseline at the same scenes (150-260 s); in
  the house (from ~280 s, ~2,000 draws) **230 ms a frame** against ~8: the game's per-draw reads (vertex data,
  constants) over PCIe. 4 FPS from 280 s; the run fell behind its steps (title logo at the `house` shot) and ended at
  314 s (window closed, 97 of 363 steps), so it has no window numbers.
- The write-watch scan rose 1.23 -> 6.26 ms a frame (an imported pool is never sealed: `seal_eligible` takes mirrors
  only), the per-pool pass fell 1.38 -> 0.79, uploads 3 -> 0.07 MB a frame.
- Pictures (sheet 1: title, story text, nightmare, title logo): right for the scenes they show.

So the GPU must keep reading most pools from VRAM; only pools whose mirror costs more than their PCIe reads may be
imported, which is the selective rule's job.

## The selective rule: hot mirrors imported at run time (`dcd548e`, `NXGFX_HOST_HOT`) - built, measured, taken out

Built (`9abdc38`, `dcd548e`): per pool, the frames of the last 64 in which the CPU wrote >= 256 KB of it and in which a
write-back of it landed (`vk/hot.rs`, unit-tested); a Mirror of 1 MB - 256 MB with >= 24 CPU frames and >= 16
write-back frames was switched at the next sync with none of its write-backs in flight (`Pool::to_host`: device-held
pages brought back, seal entries removed, the guest pages imported, the old buffer and granules freed after their last
use, `POOL_GEN` / `LAYOUT_GEN` moved, buffer textures requeued, its upload-skip entry dropped). An imported pool's
scanned pages were no longer hashed ahead and its ranges skipped the in-flight guard (both wasted for Host pools).

`build/play/20261128-081047-host-hot1` (same diagnostics as the baseline): the rule fired once the game began
rewriting its per-frame pools (~199 s): **20 pools, 88 MB imported** (the 6.3-15.2 MB storage pools named above and
fourteen 1.1-8.6 MB pools of the same pattern, flags 0x24), **one refused**: `0x15af30000` (88 MB, flags 0x22, the pool
DIRECT-MEM also saw refused), which stayed mirrored. No switch waited on an in-flight write-back; no device loss.

**Disproved by the GPU time.** From the switch on the GPU frame doubled: 17.3-18.6 ms a frame at 200-210 s and
15.3-15.4 at ~266 s, against 8.3-8.9 in the baseline at the same scenes; the game fell to ~28-31 FPS (the replay
thread went idle ~20 ms a frame waiting for the GPU). These pools are storage buffers the game's shaders both write
and read heavily within the frame; over PCIe that costs more than their whole mirror cost on the CPU (the pool sync's
~8 ms a frame, of which these pools are ~5). Pictures at every shot read (title logo, Link's hand, the Deku Tree,
the house at the window capture) were right; the owner saw a fine grid over the screen in bright cutscene moments and
in game during this run, which no window capture or saved shot shows (open item, below).

The game window was closed at 645 s (by the owner, watching it at ~30 FPS), before the fixed window; route checks x5
and h1 on route (0.009, 0.021), sheets 1-2 read (all 14 shots right: title, story, title logo, Deku Tree, forest,
Navi, Link in his house). Compared over the standing stretch both runs reached, [600, 640] s (x5 to h1, Link standing
in his house, ~2,000-2,090 draws):

| [600, 640] s | baseline `host-base` | `host-hot1` | target |
|---|---|---|---|
| FPS / 5 % low / min | 47.6 / 43.5 / 40.7 | **29.7 / 27.2 / 26.0** | 59 |
| frame ms | 21.02 | 33.72 | <= 16.9 |
| **GPU ms a frame** | **9.16** | **24.62** | |
| replay + prep (p50) | 18.56 (18.61) | 16.02 (15.65) | |
| replay busy | 20.45 | 18.06 | <= 15.8 |
| pool sync (frames CSV) | 8.69 | 5.71 | |
| landings first / sync lock wait / per-pool pass | 1.97 / 1.13 / 3.21 | 0.45 / 0.24 / 2.29 | |
| write-watch scan | 2.36 | 2.55 | |
| completion: pool lock / landed | 4.19 / 3.43 | 1.94 / 0.69 | |
| same-byte CPU pages hashed (MB a frame) / copy and hash ms | 57.48 / 2.42 | 13.11 / 1.26 | |
| write-back ranges compared on the GPU (MB a frame) | 67.50 | 23.07 | |
| the game's fence wait | 6.23 | 4.71 | |
| peak memory | 29,052 MB | 28,537 MB | < 32,768 |

So the hypothesis held on the CPU side (the targeted stages fell: pool sync -3.0 ms, replay busy -2.4, the completion
thread's landings -2.7, 44 MB a frame less hashing, 44 MB less write-back comparing) and failed on the GPU side: +15.5
ms of GPU a frame for 88 MB of imported pools, so the frame became GPU-bound at ~34 ms. The disproof the brief named.

The code was taken out (`docs/31` §4: a change that makes the frame worse goes); the worktree's source is the base's
again (`git diff 99280a2 -- src` empty). No `oot-house`, `oot` or `mk8d` run was made: the final code is the base
code, and the owner asked to wrap up.

## Open items

- **A fine grid over the screen (owner, during this wave's runs):** seen in bright cutscene moments and in game. Not
  in any saved shot or window capture (PrintWindow at 08:16 and 08:18, the door's bright sky zoomed 4x, identical to
  the baseline's shot). The bright cutscenes where it was "most visible" ran before any pool was imported (199 s), so
  it is not this change; candidates: the sharp (contrast-adaptive) upscale to the window, or tearing at ~30 FPS.
  Needs a photo of the monitor and an owner of its own.
- **Runs start from boot.** Every `oot` / `oot-still` / `oot-house` run plays ~11 minutes of opening before its
  window (owner: "we need to start from a save so we don't waste time"). Recommendation: a save with Link standing in
  his house and a scenario that loads it, so the fixed window starts within a minute.
- What would still cut the replay thread's mirror cost without PCIe reads: the GPU copies a hot pool's guest bytes
  into its VRAM buffer itself each submission (DMA from an imported view, no CPU hash), with write-backs as now;
  not built (it keeps the landings, which were ~3 ms of the saving).

## Report (agent, protocol §5)

Branch `oothost` (base `9b5f066`): `99280a2` (plan), `9abdc38` (hot rule and Host skips), `dcd548e` (24 of 64),
then the take-out and this document.

### 1. Built

- Diagnosis: OoT's ~57 MB a frame of same-byte CPU rewrites and ~67 MB of write-back ranges are whole pools of
  1-15 MB, flags 0x24 (as 3.3 GB of other pools), rewritten by the CPU and written by the GPU every frame or every other.
- Experiment `NXGFX_POOL=host`: 131 imports refused, GPU 230 ms a frame in the house, scan +5 ms (Host pools are
  never sealed). Disproves importing everything.
- Selective rule (`vk/hot.rs`, `Pool::to_host`, Host skips in the sync): worked as designed (20 pools, 88 MB, one
  refusal, no device loss, pictures right), CPU stages down as predicted, GPU +15.5 ms: FPS 47.6 -> 29.7. Taken out.

### 2. Tests

- While the code was in: `cargo test --release -p graphics --lib` hot rule tests (4: hot when both write on most
  frames; floors, sizes and CPU-only pools stay mirrored; tried once; older frames don't count) and
  `only_mirrors_pages_are_hashed_ahead`: all passed. They went out with the code.
- After the take-out (source = base): `cargo test --release -p graphics`: lib 325 passed, 6 ignored; other targets
  pass.

### 3. Verified

Runs (this worktree's `build/play/`): `20261128-074701-host-base` (baseline, completed, on route, sheets 1-3 read),
`20261128-080501-host-all` (ended at 314 s, sheet 1 read), `20261128-081047-host-hot1` (window closed at 645 s, on
route at x5 / h1, sheets 1-2 read). Numbers in the tables above. Target 59: not reached; the change made it worse.

### 4. Unproven

- Which of the 20 pools carry the GPU cost (no per-pool GPU read measure); a subset was not tried (owner: wrap up).
- The grid's cause (open item).

### 5. Slips

- Unit-test compiles ran at 07:49-07:53 during the baseline's boot (before its windows).
- `git rm` removed `vk/hot.rs` in the take-out (a deletion, not a scripted edit); every other take-out edit was made
  with the editor.

### 6. Questions

None.
