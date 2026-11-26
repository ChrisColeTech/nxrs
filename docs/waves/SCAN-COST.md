# SCAN-COST — the per-frame scan for CPU-written guest memory made nearly free

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~15:20, worktree
`D:\Projects\RecompLauncherEngine-scan`, branch `scan` (base `gameplay-harness` `1619d31`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1. Deadline 6 h.

## Goal and baseline

OoT presents ~18.5 FPS (target 59). DIRECT-MEM `133352-dm-oot-off` (window 845-915 s, Link in his house), ms a
frame: replay 17.5, prep 25.2, of which pool sync 20.7 = lock 2.4 + write-watch scan 11.1 + per-pool pass 6.6;
completion write-back 10.8; GPU 9.3. ~4,400 mirrored pools (4.4 GB) are watched.

What the code and the lead's `131123-lead-oot-merged` log say about where the scan goes (before any change):

- 11 syncs a frame (10 replays, 1 present); 8.3 of them are covered by an earlier scan or a walk, so ~2.7 a frame scan.
- A clean `GetWriteWatch` walk costs ~1.5 ms a GB here (measured on this machine: 1 GB committed and touched, 1.5-1.7
  ms; untouched 0.75 ms). A frame's first sync (the present's, unless a walk covers it) asks every pool, cold ones too:
  4.4 GB, ~6-7 ms. The later scanning syncs ask every pool not cold (with the sticky rule of OOT-FLASH, a pool once found
  written after a quiet stretch is never cold again).
- The pool walker walks all 4.4 GB again and again (warm walks are off, so every walk is full): ~10 ms a walk, ~70
  walks a second, and covers only ~10-20 of ~150 presents in 8 s (OoT's presents run behind parked work). Its pieces
  hold the watch lock the syncs and landings wait for (part of the lock wait).
- The per-pool pass iterates all ~4,500 pools at each of the 11 syncs; ~4 ms of its ~6.3 ms is the upload skip's
  copy-and-hash of ~56 MB a frame of CPU-written pages (real work), ~2 ms is the pass itself.

Most of the 4.4 GB is never written after its load. The scan exists only because the OS gives no cheaper "was this
written?" answer than walking every page.

## Plan: seal quiet pools with write protection (no scan, no latency)

A pool the CPU has not written for a while is **sealed**: its pages are made read-only and it leaves every write-watch
scan, walk and the per-pool pass. The first CPU store into it faults; a vectored handler opens the 64 KiB chunk
around it (read-write again), marks the pool **woken**, and the store goes on (the write watch records it as
before). The next sync takes the woken pools back into its scan. A write into a sealed pool is therefore known the
moment it happens: never missed and never later than now (also closing the cold rule's one-frame residual that
OOT-FLASH documented for the first write after a quiet stretch).

Rules (all in a new module `vk/seal.rs`, with small hooks in `mem.rs`, `lazy.rs` and `os/src/mem.rs`):

1. **Eligible:** a watched Mirror pool (Windows), wholly in one allocation, its pages all committed (the protection
   call fails otherwise), overlapping no other pool's pages, no page held by lazy write-back, no CPU write and no
   write-back landing for `NXGFX_SEAL_FRAMES` frames (default 60).
2. **Sealing** (budgeted per frame, `NXGFX_SEAL_MB`, so 4.4 GB is sealed over a few seconds rather than in one frame):
   the registry entry is armed, the pool (or only its open chunks when it was sealed before) is protected read-only,
   then — under the pool lock and the watch lock — the write watch over it is taken (reset) into its pending writes,
   and only then the pool counts as sealed. A store before the protection is a watch bit that last take finds; a store
   after it faults.
3. **Waking:** the handler (any thread, possibly holding the pool lock: it takes only the registry's own lock, which no
   holder keeps while waiting for anything else) opens the chunk, disarms the entry and queues the pool. The queue is
   drained into the pools (`Pool::sealed = false`, `dirty_frame = now`, so the cold rule cannot delay it either) by
   every holder of the watch lock, and by a scanning sync **again after it read the replay queue's tail** (a scan
   covers the ops queued up to that tail; a write made before one of them woke its pool before it was queued).
4. **Host writers:** recommitting memory resets its protection silently (`VirtualAlloc(MEM_COMMIT)` on a committed
   read-only page makes it read-write: checked on this machine). `os::mem::commit_rw` (used by `prepare` before every
   host write through `guest::bytes_mut`, by first-touch commits, maps) wakes the sealed pools over the range first.
   A landing (GPU write-back) on a sealed pool unseals it whole first (it is GPU-written now; lazy write-back must not
   protect pages of a sealed pool). A pool registered over a sealed one, and a released pool, are unsealed.
5. **Safety net:** the pool walker peeks (no reset) the sealed pools' write watch now and then; a sealed pool found
   written without a fault is logged red and woken (its bits are then taken by the next scan).
6. **Switch:** `NXGFX_SEAL=0` off (today's behaviour). Linux: not built (no sealing).

Then, measured: the per-pool pass over only the pools that can have work; the walker's walks over only unsealed
pools.

**What would disprove it:** most of OoT's 4.4 GB is rewritten every few frames (then pools never stay sealed); or
faults on sealed pools are frequent enough to cost more than the scan. The pools line gets the sealed count/bytes and
the wakes a frame, so the first run tells.

**Files expected:** `src/graphics/src/vk/seal.rs` (new), `vk/mem.rs`, `vk/mod.rs`, `vk/lazy.rs` (a pure "holds any page
in range" query), `src/os/src/mem.rs` (the commit hook), `docs/env_switches.tsv`, this doc.

## Runs (GPU 1; window [845, 915] s; per-frame means from the frames CSV, pool parts from the `[vk] pools:` lines)

| Run (`build/play/`) | build | FPS / 5% low | replay | prep | pool sync | lock / scan / pass | write-back MB (ms) | GPU | route |
|---|---|---|---|---|---|---|---|---|---|
| `20261126-154635-sc-base-oot` | base `1619d31` (relink 15:46) | 16.2 / 14.0 | 20.5 | 29.6 | 23.6 | 2.7 / 12.2 / 7.7 | 1.2 (13.7) | 10.6 | 6/6 on |
| `20261126-163048-sc-v4-oot` | v4 `f61f20c` (relink 16:30) | **19.4 / 17.3** | 18.9 | **16.1** | **11.9** | 2.5 / **2.6** / 5.9 | 1.0 (9.7) | 9.5 | 6/6 on |
| `20261126-170947-sc-v4-seal0-oot` | v4, `NXGFX_SEAL=0` (control, same build) | 17.7 / 15.2 | 19.8 | 26.3 | 21.3 | 2.6 / 11.7 / 6.5 | 1.0 (12.5) | 10.0 | 6/6 on |

(Pool parts: means of the `[vk] pools:` lines in the window. All three runs overlapped TRANSLATE-REUSE's MKW runs on
GPU 0. The control keeps `Pools::busy`, so its pass is already ~1 ms below the base.)

v4 in the window: 3,646-3,651 pools (3.7 GB) sealed out of ~4,400; 0.00-0.05 pools a frame woken by a store, none by a
host write, none found written without a fault; the sealing pass 0.04 ms a frame under the pool lock once all is
sealed (~30 s after boot). Not sealed (census at 910 s): 683 pools (485 MB) written by the CPU in the last 60 frames,
58 (503 MB) with write-backs. Syncs a frame: 10 replays + 1 present, 9.1 covered by an earlier scan; the walker's walk
1.86 ms (was ~10-14 ms), and it now covers the present's sync more often (88 of the last 600 walks). The per-pool pass
is now mostly the upload skip's copy-and-hash of the ~57 MB a frame of pages the CPU rewrites with the bytes the GPU
already holds (4.8 ms), the iteration ~1 ms (was ~3.5). The frame (51 ms) is no longer bounded by the replay thread
(replay + prep 35 ms); `descriptor_ms` 19 ms a frame and the guest are the larger parts now (outside this wave).

- `20261126-160352-sc-v1-oot` (v1, `420b389` + walker list cache): stopped by PID at ~300 s. Sealing stalled at
  1,250 pools (51 MB): a pass stopped at the first pool larger than its budget (32 MB) and every later pass stopped at
  the same pool. No red line, no fault, no wrong picture up to the stop. Fixed in `6d42235`: a first sealing goes in
  pieces of the budget, one piece a pass (a store into a piece already read-only wakes the pool as any store does).

- `20261126-161318-sc-v2-oot` (v2, `6d42235`), stopped at ~250 s: 3,094 pools (646 MB) sealed, then no more; scan
  ~8 ms; lock wait up to 7 ms. `20261126-162*-sc-v3-oot` (v3: + `Pools::busy`, a census of why pools are not
  sealed), stopped at ~295 s: **81 pools (3.1 GB) failed the "every page committed and read-write" check** (memory
  committed in parts, 64 KiB at a time), 586 (313 MB) had CPU writes in the last 60 frames, 2 (48 MB) write-backs; and
  that check, a VirtualQuery walk over those fragmented pools at every pass, held the pool lock **16.7 ms a frame**
  (the lock-wait rise). Corrections (`47dd5eb`, `f61f20c`): the committed runs of a pool are protected (listed by
  VirtualQuery, a piece at a time), its uncommitted pages need none: they cannot be written before they are committed,
  and every commit now wakes the pools it touches (the OS layer's `commit_rw` and the runtime's own `mem::commit`,
  through a hook the seal module registers); a store into a page that is still uncommitted is left to the runtime's
  first-touch handler (which commits it, waking the pool). No committed check per pass any more.
- `20261126-164609-sc-v4-house1` (`oot-house --faults`, v4): route check **OFF-ROUTE at `04-d01` only** (distance
  0.32, limit 0.3): the opening ran a little later than the reference, so d01-d03 show the end of Link's dressing
  cutscene (correct pictures); d04-d30 and `last` are Link in his house, all correct, and the other four checks are on
  route (FPS of this run not used). 16 fault captures, all read: nightmare lightning (1-2), the known no-target draws
  at the forest load (3-10), scene cuts during Navi's flight and the dressing cutscene (11-16); none in the house
  stretch. No new red line, no sealed pool found written without a fault.
- `20261126-165755-sc-v4-house2` (`oot-house --faults`, v4): **timed out, OFF-ROUTE** (d01, d10): 25 frames of 1-2 s
  at 439-576 s with the guest idle (busiest guest threads 4-6% of a core) and the replay idle (10-15 ms), so the
  timed inputs ran ~60 s ahead of the game; it ended in the Navi-wakes-Link cutscene. Those stalls lie exactly inside
  TRANSLATE-REUSE's `170215-tr-r2-on3t10-mkw` run (MKW with 10 translation threads, 17:02:15-17:07:54; the stalls
  17:05:14-17:07:31) and stop when it ends; seals in that stretch: 0.0-0.18 wakes a frame, nothing unusual. The
  same symptom (frames of seconds, guest idle) was seen by UPSYNC2 under load. Read as machine-side, not this
  change; no house shot from this run counts. Its 33 pictures (forest, Navi's flight, the wake-up) are all correct;
  15 fault captures, the same legitimate set as house1.
- `sc-v1`-`sc-v3`: no red line, no fault, every picture up to the stop correct.

The base run had TRANSLATE-REUSE's MKW relink and my own test compiles beside it in its first minutes (not in the
window); numbers indicative. Sheets (3) correct: title, story, nightmare, house, title card, Deku Tree, Navi, Link
woken, every house and walk shot. Anomalies: the known set (one stand-in at the forest load, no-target, crashed
pipeline, occlusion queries).

**Verification:** `cargo test --release -p graphics` (and `-p os` if its test set is quick); `oot` before/after on GPU 1
(FPS / 5% low vs 59, replay / prep / pool sync parts, write-back); `oot-house --faults` (>= 60 house shots over two
runs, no new faults); every sheet and report read.
