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
- `20261126-172511-sc-v4-house3` (`oot-house --faults`, v4): completed, **5/5 on route**, house window 19.3 / 18.0 FPS
  (UPSYNC2's dense runs on the old engine: 15.8-19.3 / 13.9-18.0). All 33 house shots correct (x5, h1, d01-d30,
  last); 16 fault captures, the same legitimate set at the same presents as house1, none in the house; 2 frames over
  1 s (forest load), no new red line.
- `sc-v1`-`sc-v3`: no red line, no fault, every picture up to the stop correct.

The base run had TRANSLATE-REUSE's MKW relink and my own test compiles beside it in its first minutes (not in the
window); numbers indicative. Sheets (3) correct: title, story, nightmare, house, title card, Deku Tree, Navi, Link
woken, every house and walk shot. Anomalies: the known set (one stand-in at the forest load, no-target, crashed
pipeline, occlusion queries).

**Verification:** `cargo test --release -p graphics` (and `-p os` if its test set is quick); `oot` before/after on GPU 1
(FPS / 5% low vs 59, replay / prep / pool sync parts, write-back); `oot-house --faults` (>= 60 house shots over two
runs, no new faults); every sheet and report read.

## Report (agent, protocol §5)

Branch `scan`: `e5e1b4e` (plan), `420b389` (sealing), `f877da9` (walker list cache), `6d42235` (pieces), `1649290`
(`Pools::busy`), `1a327f5` (census), `47dd5eb` + `f61f20c` (committed runs, runtime commit hook), docs after.

### 1. Built

**Diagnosis.** OoT watches ~4,400 mirrored pools (4.4 GB); `GetWriteWatch` walks every page it is asked about
(~1.5 ms a GB, measured here), and a frame asked all of them at least once (the present's sync, or the pool walker,
which walked all 4.4 GB again and again at ~10 ms a walk), plus the non-cold ones at ~2 more scanning syncs: ~12 ms a
frame of scan. Most of those pages are never written after the load. The per-pool pass visited all ~4,500 pools at
each of 11 syncs a frame, also the 9 that scan nothing.

**Correction 1, sealed pools (`vk/seal.rs`, new; `vk/mem.rs`).** A watched mirror with no CPU write and no write-back
landing for 60 frames (`NXGFX_SEAL_FRAMES`), sharing no page with another pool, in one allocation, with no page held by
lazy write-back, is made read-only (its committed runs, 32 MB a pass at most, `NXGFX_SEAL_MB`, by the pool walker
under the pool and watch locks), then the write watch over it is taken into its pending writes (a store made before
the protection), and it counts as sealed while no store woke it. A sealed pool is out of every scan, walk and per-pool
pass. Its first store faults: a vectored handler (registry lock only; never waits for anything else) opens the 64 KiB
chunk around it, marks the pool woken and lets the store go on (the write watch records it). Every holder of the watch
lock, and every scan again **after reading the replay queue's tail**, and the walker after reading its tail, drain the
woken pools (`sealed` off, `dirty_frame` = now so the cold rule cannot hold their writes back): a store made before an
op a scan covers woke its pool before that op was queued. So a write into a sealed pool is seen the moment it is made;
nothing is delayed (also closing the cold rule's one-frame residual of OOT-FLASH for these pools).
Paths that make sealed pages writable without a fault wake the pool first: commits (`os::mem::commit_rw`, which
`guest::bytes_mut` / `zero` / `write_bytes`, first-touch and maps use; the runtime's `mem::commit` through a new hook,
`nxrt::mem::set_commit_hook`) — checked here that `VirtualAlloc(MEM_COMMIT)` over a read-only committed page makes it
read-write. A write-back landing unseals its pool whole (lazy write-back owns its pages' protection); a pool registered
over a sealed one, and a released pool, are unsealed. Lazy write-back's handler leaves a fault on a sealed page to the
seal handler (it would retry a readable page for ever). Safety net: every 30 frames the walker peeks the sealed pools'
write watch; a page written without a fault is a red line and wakes the pool. `NXGFX_SEAL=0`: off. Windows only.

**Correction 2, `Pools::busy` (`vk/mem.rs`).** A sync that does not scan visits only the pools with work of their own
(pending, deferred, texture-dirty ranges; mirrors without a write watch). Every place that gives a pool such work adds
it (flush, `sync_range`, the walker's drain, landings, sealing, a pool without a watch); a scanning sync visits every
pool and rebuilds the set.

**Correction 3, the walker keeps its list** (`walker_main`) while no pool came, went, got a buffer, was sealed or woken
(`POOL_GEN`, `SEAL_GEN`).

Diagnostics: the pools line's `sealed pools:` part (sealed count and MB, wakes a frame by store / host write, armed,
protection time, the sealing pass's lock time, unsealed whole, found written without a fault); every 300 frames a
census of why the watched mirrors not sealed are not.

### 2. Tests (`cargo test --release -p graphics`: lib 269 passed, 6 ignored, other targets pass; `-p runtime` 50 + 2
pass; `-p os` 166 + 1 pass, 1 ignored)

- `seal::tests::a_store_into_a_sealed_pool_wakes_it_and_is_watched` (real VirtualAlloc + write watch + the handler): a
  store faults once, wakes, opens only its chunk, lands, is in the write watch; another chunk opens without a second
  wake; re-arming closes only the opened chunks; a store wakes it again.
- `seal::tests::host_writes_and_removal_leave_nothing_sealed`: a host write announced before it wakes the pool and
  opens the chunks it spans; removal leaves the range read-write (no fault).
- `seal::tests::overlapping_and_uncommitted_ranges_are_not_sealed`: overlap refused; the walker's leak check wakes
  once; a range committed in part is armed over its committed page (a store wakes it), and a commit of the rest
  announced as the runtime does wakes it and the page is written without a fault.
- `seal::tests::a_large_pool_is_armed_in_pieces`: pieces of the budget; a store in a protected piece wakes it; past
  them no fault; the last piece also closes the chunk opened meanwhile; both stores in the write watch.
- `mem::seal_tests::pools_sharing_a_page_are_found` (aliases, nested pools, two pools touching one page),
  `only_quiet_watched_mirrors_are_sealed` (60 quiet frames, no landing, nothing waiting, watched mirror in one
  allocation), `a_woken_pool_is_asked_again_and_warm` (drain: `sealed` off, not cold, the store in the watch),
  `work_handed_to_a_pool_makes_it_busy` (the walker's drain marks the pool; what counts as busy).
- All are new mechanisms (they do not exist in the old code). Two tests first read the shared woken queue and raced
  each other; they now read the entry (5 repeated runs pass).

### 3. Verified (GPU 1; TRANSLATE-REUSE's MKW runs on GPU 0 overlapped every run)

`oot`, window [845, 915] s, target 59 (table above): base 16.2 / 14.0 FPS -> **19.4 / 17.3**; prep 29.6 -> 16.1 ms;
pool sync 23.6 -> 11.9 ms; **write-watch scan 12.1 -> 2.6 ms**; per-pool pass 7.7 -> 5.9 ms; lock wait 2.8 -> 2.5 ms;
completion write-back 13.6 -> 9.7 ms; replay 20.5 -> 18.9 ms. Same build with `NXGFX_SEAL=0`: 17.7 / 15.2, scan 11.7,
prep 26.3. The FPS gain is smaller than the prep cut: the frame (51 ms) is no longer bounded by the replay thread
(replay + prep 35 ms). Target 59 not reached.
Route: 6/6 on in all three `oot` runs. `oot-house --faults`: house1 (OFF-ROUTE at d01 only, a timing offset; 28 house
shots correct), house2 (machine-side stalls during another wave's heaviest run, desynced; no house shot counted, all
pictures correct), house3 (5/5 on route, 33 house shots correct). **House shots on the sealing build: 79 (house1 28,
house3 33, `sc-v4-oot` 18), all correct**; no fault capture in a house stretch; every capture read (lightning, the
known forest-load no-target draws, scene cuts). Sheets read: all of base, v4, seal0, house1-3 (and v1-v3 up to the
stops). Red lines: the known set only; no `sealed pool ... written without a fault` in any run. No device loss.

### 4. Unproven

- Write paths the protection cannot see other than commits: a kernel-mode write into guest memory (an OS call given a
  guest buffer) would fail on a sealed page instead of faulting. None found by reading (file, socket, audio paths copy
  through host buffers and `guest::bytes_mut`, which commits first); the walker's leak check would show one (0 in all
  runs). The debug command `wpage`/`wtrace` (`os/wwatch.rs`) changes page protection itself and is not seal-aware.
- Not sealed: pools with write-backs (503 MB in the house) and pools the CPU writes (485 MB); they are what the 2.6 ms
  of scan still walks. Next: let a landing write through a sealed pool (open, copy, close, retake the watch) and keep
  lazy pages compatible, instead of unsealing.
- The per-pool pass is now mostly the upload skip's copy-and-hash of ~57 MB a frame the CPU rewrites with unchanged
  bytes (4.8 ms); hashing all of a sync's candidate pages on the hash threads at once would cut it.
- What bounds OoT's frame now (51 ms with replay + prep 35 ms; `descriptor_ms` 19 ms a frame): outside this wave.
- Linux: no sealing (the switch is Windows only).
- Perf indicative (shared machine); the lead's quiet runs decide. Other titles (MKW, DKB, Smash) not run (not owned).

### 5. Slips

- Relinks ran this worktree's `nxrs-build` directly with the launcher's own command line, not
  `dist\nxrs\nxrs.exe --select-title ... --build`: TRANSLATE-REUSE's launcher (`--select-title ... --build`) was
  running, and UPSYNC2 recorded that a second launcher hands its build to the running one. DLL time checked after each
  (15:46:23, 16:02:55, 16:21:36, 16:30:01).
- Runs v1, v2, v3 were stopped early by PID (harness, then game) to rebuild with a correction.
- `cargo test` of graphics/os ran during the base run's first minutes (before its window).

### 6. Questions

None.
