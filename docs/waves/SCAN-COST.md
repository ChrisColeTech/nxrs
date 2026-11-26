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

The base run had TRANSLATE-REUSE's MKW relink and my own test compiles beside it in its first minutes (not in the
window); numbers indicative. Sheets (3) correct: title, story, nightmare, house, title card, Deku Tree, Navi, Link
woken, every house and walk shot. Anomalies: the known set (one stand-in at the forest load, no-target, crashed
pipeline, occlusion queries).

**Verification:** `cargo test --release -p graphics` (and `-p os` if its test set is quick); `oot` before/after on GPU 1
(FPS / 5% low vs 59, replay / prep / pool sync parts, write-back); `oot-house --faults` (>= 60 house shots over two
runs, no new faults); every sheet and report read.
