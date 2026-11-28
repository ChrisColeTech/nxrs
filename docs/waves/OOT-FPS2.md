# OOT-FPS2 — Ocarina of Time: the replay thread's frame under one vsync (target 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~21:45, worktree
`D:\Projects\RecompLauncherEngine-ootfps2`, branch `ootfps2` (base `gameplay-harness` `a25c3c3`: OOT-FIX and OOT-FPS
merged). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 0. Deadline 5 h.

## Plan (written first)

Starting point: OOT-FPS's final run `D:\Projects\RecompLauncherEngine\build\play\20261127-210135-fps-final2-prof`
(`oot`, [845, 915] s): 29.6 FPS / 5 % low 27.3; replay + prep 19.49 ms a frame (p50 19.5; 31 % of frames under
15.8 ms). The frame-step pacer (`src/os/src/fstep.rs`) holds whole vsyncs, so a frame over ~15.8 ms of replay-thread
work holds two (69 % of frames). The GPU is 9.0 ms and the game no longer waits for fences: the replay thread's own
work decides the frame. Its profile (`vk replay`, ~0.34 ms a frame per 1 % of samples): set building `bind_sets`
15.6 % (~5.2 ms), the pool sync `begin_submit_ex` 18.8 % (of it `prehash` 5.4 % + `upload_changed` 4.7 % hashing
~57 MB a frame of same-byte CPU rewrites, `watch::take` 3.6 %), the pool table `gmem::bindings` 3.8 % (~1.3 ms, every
submission under the pool lock).

0. **Baseline** with the diagnostics on, as OOT-FPS took `fps-final2-prof`: this worktree's dist, OoT relinked with a
   PDB (`NXRS_LINK_LIBS=-Wl,/DEBUG`), host profiler on `vk replay` with stacks over 850-890 s
   (`NXRS_HOST_PROF=850,40`, `NXRS_HOST_PROF_STACK=1`). Split `bind_sets`, `begin_submit_ex` and `gmem::bindings`
   by callee before any change; whichever is largest is cut first (said with the numbers here).
1. **(c) The pool table per submission** (`gmem::bindings`): the table bytes are already cached per pool generation,
   but every submission still walks every pool (~4,500) three times under `g.mem` (pools to back for the table,
   granule pools to bind whole, deferred uploads) and stages the 256 KB table again. Walk only when something those
   walks look at changed (a generation of its own, moved where a pool is made, gets a buffer or gets deferred work),
   and stage the table bytes once per generation if the profile shows the staging.
2. **(a) Same-byte CPU rewrites** (`upskip`, `mem::begin_submit_ex`): find what makes the ~57 MB a frame "written"
   (the game's stores, or the engine's own landings and bring-backs) and whether a cheaper signal than a hash of every
   byte decides them; cut the hash work where a page provably needs no look (or make the look cheaper), never skipping
   an upload of bytes the GPU copy does not hold.
3. **(b) Set building** (`exec::bind_sets`, `stage_set`, `patch_set`): reuse the previous set where its inputs match,
   from the profile's split (SET-FAST found the key compare free and the cost in views, `vkGetDescriptorEXT` and whole
   sets at segment starts; its view-version reuse moved nothing and was taken out).

Each change behind a switch (rows in `docs/env_switches.tsv`), default on once its pictures are right; a change that
leaves its stage's ms unchanged in the profile is taken out. Excluded: Fault A (white house floor), skipped draws or
hidden work, weaker checks; OOT-FIX's colours stay; live native NDVE off.

Expected files: `src/graphics/src/vk/gmem.rs`, `mem.rs`, `upskip.rs`, `exec.rs`, maybe `queue.rs`,
`docs/env_switches.tsv`, this document.

Verification: one normal `oot` run after the work against the baseline and its diagnostics (no switch-off controls),
every sheet read against the baseline's, report.md's anomaly table and route checks; one `oot-house --faults` run
(Fault A count as an observation). `cargo test --release -p graphics` (and each other changed crate) with new tests.
Numbers: FPS mean / 5 % low, replay + prep ms, each targeted stage's ms, peak memory, against the baseline and 59.

What would disprove the plan: the profile shows the replay thread's ~19.5 ms is not in these stages (then the larger
cost is cut first), or the cuts leave replay + prep where it was.

## Baseline (this wave's first run, with diagnostics)

`build/play/20261127-220112-fps2-base-prof` (this worktree's dist at `8b432ae` = `a25c3c3`, OoT relinked with a PDB;
host profiler `NXRS_HOST_PROF=850,40` on `vk replay` with stacks; GPU 0; nothing else of this agent's running in the
window). report.md: **FPS in [845, 915] s mean 29.5, 5 % low 26.0, min 21.0**; run peak 28,334 MB; route checks on
route (0.013-0.054). Frames CSV over the window (1,713 frames): frame 33.92 ms, GPU 8.95, replay 12.13 + prep 7.17 =
**19.30 ms** (p50 18.42; 36 % of frames under 15.8 ms), pool sync 7.04 ms, 1,478 draws. Pools lines in the window:
write-watch scan 2.54 ms, per-pool pass 4.28 ms, copy and hash 3.37 ms (57 MB a frame of CPU-written pages left out
as the GPU copy's bytes). Lanes: the replay thread waited 3.07 ms a frame for them; they waited 2.75 ms a frame for the
pool lock at resolves.

Host profile of `vk replay` (53,889 samples over 850-890 s, 1,174 frames; symbolized with llvm-symbolizer against the
PDB; ms a frame = share x 40 s / frames): `begin_submit_ex` 7.04 ms (`prehash` 2.18, `upload_changed` 1.65,
`watch::take` 1.29), command translation `record` 7.50 ms (`bind_sets` 5.35: `stage_set` 4.43 of which `tex_view`
1.48 and **`gmem::bindings` 1.25**, `patch_set` 2.21), waiting at lane joins (`par::run`) 2.98 ms, contended locks
2.31 ms. `gmem::bindings` split: 2.00 % of samples waiting for a lock (`g.mem`), 1.48 % its own walks, 0.17 % the 256 KB
table copy: the staging is not the cost, the walks under the pool lock are.

## Where the ~57 MB a frame comes from (hypothesis a)

The pools lines say the same in every run: ~57 MB a frame of CPU-written pages found to hold the GPU copy's bytes,
~58 MB re-armed at landings, write-backs over ~67 MB of ranges. These pages are written by the game (the engine's own
landings write only the GPU-changed words and reset the watch; lazy bring-backs are ~0.1 pages a frame). No signal
cheaper than reading them says they are unchanged; every attempt here kept the per-page check and moved or overlapped it:

- **Taken by whom.** The pool walker (which runs beside the replay thread) takes only ~2-3.6 MB a frame of them; the
  replay thread's own scanning syncs (2 of 3 a frame) take the rest (`fps2-v2-prof`, counters below). So hashing on the
  walker cannot take this off the replay thread (built, measured, taken out: change B).
- **When.** The rewritten pages come in a few large write-watch takes near the end of a sync's scan; hashing them
  while the scan goes on overlapped ~1.6-2.1 MB a frame (built, measured, taken out: change C).
- **How fast.** Hashing is memory-bound: page hashing from memory measured 10.8 GB/s on one thread, 52 GB/s on 7,
  69 GB/s on 12 on this machine (scratch benchmark). The hash threads were a quarter of the machine's threads, at most 6
  (7 with the caller): raised to half, at most 12 (change A2, kept).

## Changes

### A1. The pool table skips the pool walks while the pool generation stands (`gmem::bindings`, `NXGFX_GMEM_WALK_SKIP`)

Every submission and every lane segment that binds raw global memory makes its own table version; each walked every
pool (~4,500) three times under the pool lock (pools to give a buffer for the table, granule pools to bind whole,
deferred CPU writes) before taking the cached table bytes. Those walks find work only after a pool is made, goes or gets
its buffer, or a CPU write is deferred, and each of those moves `POOL_GEN` (under the pool lock). After a walk that left
nothing for later (budget not used up, no failure) its generation is kept; a table of the same generation skips the
walks and does not take the pool lock at all. A change in progress elsewhere is seen by the next table, as for a table
made just before that change. Counter: "raw global memory: N pool tables, M of them without walking the pools"
(`fps2-v1-prof`: 259,331 of 260,000).

### A2. `prehash`'s hashes found by one walk a batch; more page-hash threads (`upskip`, `NXGFX_HASH_THREADS`)

`upload_changed` looked each candidate page up in `prehash`'s list by a binary search (~14,000 pages a frame, ~0.8 % of
the replay thread); now one search a 1 MB batch and a walk along the list (the batch's pages ascend). The `vk page hash`
threads: half the machine's threads, 2 to 12 (12 here), was a quarter, at most 6.

### B. Walker-side hashing: built, taken out (`c06820f`)

The walker hashed the pages it took after its walk; the sync whose drain moved them used those hashes for pages its own
scan did not take again. `fps2-v2-prof`: 2.2-3.6 MB a frame hashed by the walker, 0.02-0.07 MB used: the pages are
written again after the walk and the sync's scan takes them itself. Moved nothing; removed.

### C. Hashing while the scan goes on: built, taken out (`af0ebb6`)

Every 256 pages a sync's scan took started a hash job on the hash threads (jobs queued oldest first). `fps2-v4-prof`:
1.6-2.1 MB a frame hashed ahead, copy and hash 2.6 ms a frame (2.3-2.5 without it), `prehash` 1.74 ms: moved nothing;
removed.

### D. Lazy images waited for by ID: built, taken out (`aae004c`)

A bound texture use took the lazy images' make lock whenever any thread was making a batch; with the IDs in flight
recorded, it waited only for its own. `fps2-v5-prof`: `tex_view`'s wait at the make lock 1.76 % -> 0.26 % of samples,
its wait at the images' lock 0.98 % -> 2.41 %: `tex_view` 4.35 % -> 4.30 %, unchanged (the same makers hold both).
Removed.

(Hypothesis b, set reuse: not built. The baseline profile puts `bind_sets` at 5.35 ms a frame, of it `patch_set`
2.21 ms (patches, mostly uniform-buffer classes, as SET-FAST measured) and `tex_view` 1.48 ms (lock waits, see D); the
whole-set builds that could be reused across submissions are the raw-memory sets, whose cost was `gmem::bindings`'s
walks (A1). No reuse rule beyond SET-FAST's per-stage key was found that the profile says would pay.)

## Runs (all `oot`, GPU 0, this worktree's `build/play/`, window [845, 915] s)

FPS / 5 % low from report.md; the rest from the frames CSV over the window (means; replay + prep with its p50 and the
share of frames under 15.8 ms) and the pools / translate lines of run.log in the window (means). All on route; every
run but v3 sampled `vk replay` over 850-890 s with stacks, as the baseline.

| run | build | FPS / 5 % low | replay + prep (p50, <= 15.8) | pool sync | copy and hash / write-watch scan / per-pool pass | replay waits for lanes / lanes wait at resolves | GPU | draws | peak |
|---|---|---|---|---|---|---|---|---|---|
| **`20261127-220112-fps2-base-prof`** | `8b432ae` (base) | **29.5 / 26.0** | **19.30 (18.42, 36 %)** | 7.04 | 3.37 / 2.54 / 4.28 | 3.07 / 2.75 | 8.95 | 1,478 | 28,334 MB |
| `20261127-222528-fps2-v1-prof` | `53d3664` (A1, A2's lookup) | 20.9 / 6.0 * | 20.00 (18.61, 36 %) | 7.24 | 3.31 / 3.02 / 4.39 | 2.50 / 2.12 | 9.69 | 1,700 | 28,736 MB |
| `20261127-224206-fps2-v2-prof` | `e0459d0` (+ B, 12 hash threads) | 46.9 / 30.0 | 13.98 (13.04, 62 %) | 5.61 | 2.31 / 2.04 / 3.19 | 1.55 / 0.96 | 8.08 | 1,074 | 30,248 MB |
| `20261127-225728-fps2-v2-gameprof` | `e0459d0`, sampling the game's main thread | 29.1 / 24.0 | 17.14 (16.18, 47 %) | 5.77 | 2.47 / 2.28 / 3.28 | 2.60 / 1.98 | 9.06 | 1,946 | 29,638 MB |
| `20261127-231546-fps2-v3-guestprof` | `c06820f`, sampling every guest thread | (crashed at 868 s: the sampler's buffers, see Slips) | | | | | | | 32,651 MB |
| `20261127-233219-fps2-v4-prof` | `5aaa995` (+ C) | 29.6 / 27.0 | 16.86 (16.31, 47 %) | 6.06 | 2.65 / 2.42 / 3.49 | 2.30 / 1.98 | 8.77 | 1,679 | 28,926 MB |
| `20261127-234848-fps2-v5-prof` | `df2a180` (+ D) | 29.6 / 27.0 | 16.77 (16.00, 49 %) | 5.97 | 2.63 / 2.36 / 3.45 | 2.55 / 2.33 | 8.77 | 1,629 | 29,260 MB |
| **`20261128-000649-fps2-final-prof`** | **`aae004c` (final: A1 + A2)** | **29.8 / 28.0** | **17.22 (16.47, 45 %)** | 6.01 | 2.55 / 2.40 / 3.42 | 2.90 / 2.58 | 9.12 | 1,652 | 29,107 MB |

\* v1: from 871 s the game's own frames took 120-300 ms (the replay thread idle ~80 ms a frame, the pacer at 3
vsyncs; the camera at the door, "Open" prompt); over [845, 870] its replay + prep was 16.31 ms (p50 15.71, 51 %) at
1,451 draws. Not an engine stall the logs show; v1's window FPS is not used.

**The walk is not the same in every run.** The `oot` walk's camera and path differ from run to run (the sheets: Link at
the door, by the window, under the bed's canopy), and the draws a frame with them (1,074-1,946 in the window); replay +
prep follows the draws. Same-draw comparisons, from the baseline's own halves: base [845, 870] 1,970 draws 22.48 ms,
[870, 915] 1,113 draws 16.95 ms; with A1 + A2: 1,946 draws 17.14 ms (`v2-gameprof`), 1,652 draws 17.22 ms (final),
1,074 draws 13.98 ms (`v2-prof`). At about the same draws the replay thread's frame is ~3 ms shorter at ~1,100 draws and
~5 ms shorter at ~1,950.

## Before / after (report.md, `oot`, [845, 915] s)

- Baseline `fps2-base-prof` (`8b432ae` = `a25c3c3`): **29.5 / 26.0 / min 21.0**, replay + prep 19.30 ms (p50 18.42,
  36 % under 15.8), pool sync 7.04 ms, 1,478 draws, peak 28,334 MB.
- Final `fps2-final-prof` (`aae004c`): **29.8 / 28.0 / min 27.0**, replay + prep 17.22 ms (p50 16.47, 45 % under
  15.8), pool sync 6.01 ms, 1,652 draws (+12 %), peak 29,107 MB.
- Lighter walk with the same changes (`fps2-v2-prof`): 46.9 / 30.0, replay + prep 13.98 ms (62 % under 15.8).
- Target 59: not reached.

Replay thread, host profile over 850-890 s (both `vk replay` only, 1,174 / 1,186 frames), ms a frame:

| stage | base | final |
|---|---|---|
| `run_stream_due` (all the replay's work) | 10.01 | 8.48 |
| pool sync `begin_submit_ex` | 7.04 | 5.92 |
| of it `prehash` / `upload_changed` / `watch::take` | 2.18 / 1.65 / 1.29 | 1.65 / 1.32 / 1.25 |
| translation `record` | 7.50 | 6.37 |
| `bind_sets` / `stage_set` / `patch_set` / `tex_view` | 5.35 / 4.43 / 2.21 / 1.48 | 4.19 / 3.31 / 2.36 / 1.46 |
| **`gmem::bindings`** | **1.25** | **0.07** |
| waiting at lane joins `par::run` | 2.98 | 3.32 |
| contended locks | 2.31 | 1.60 |

What each kept change moved: A1: `gmem::bindings` 1.25 -> 0.06-0.14 ms a frame in every run after it (259,356 of
260,000 tables without a walk); its pool-lock waits went with it (the replay thread's contended locks 2.31 -> 1.60 ms;
the lanes' waits at resolves 2.75 -> 0.96-2.58 ms with the draws). A2: `upload_changed`'s own time 0.68 -> 0.47 ms a
frame in `v1` (the per-page binary searches gone from the line profile); copy and hash 3.37 -> 2.31-2.65 ms and
`prehash` 2.18 -> 1.52-1.74 ms a frame with 12 hash threads.

## Why the FPS stays at 30 in the heavier walks

The pacer decides on the game's frame, not the replay thread's: `cost = shown - pacing wait in the acquire`
(`fstep::present`). Its line ("cost a frame", over the last 1,800 presents) gives medians of 1.20-1.36 vsyncs (20-23 ms)
from 640 s on in every run, fitting one vsync in 3-25 % of frames; the baseline's line at 887 s 1.45 (8 %), the
final's at 866 s 1.29 (9 %). In `fps2-v2-prof` (13.98 ms replay + prep, 1,074 draws, 46.9 FPS) its line at 885 s was
the lowest, 1.20 (18 %): the game's frame shortens with the replay thread's, but at ~17 ms of replay thread it stays
over one vsync. A profile of the game's
main thread (`fps2-v2-gameprof`, `guest 0x1001`) shows it busy 10 % of the time (4.1 s of CPU in 40 s): 36 % of its
samples pacing in `nvnWindowAcquireTexture`, 40 % blocked in a kernel wait (`kernel::block`, waiting for other guest
threads), 3.7 % in `nvnMemoryPoolFlushMappedRange` (`mem::cpu_flushed`, the pool lock). The other guest threads'
profile crashed the run (Slips); what they wait for is the next thing to find out.

## `oot-house --faults` (final build)

`build/play/20261128-002206-fps2-final-house` (`aae004c`, `NXGFX_FAULT_MAX=200`, `NXGFX_FAULT_PER_REASON=80` as
OOT-FPS's house run): completed, normal exit, house window [610, 690] **29.1 / 27.0 / min 27.0** (OOT-FPS's
`fps-final2-house` 29.1 / 27.0), peak 28,745 MB, on route (0.014-0.053 at d01/d10/d20/d30/last). Red lines: one, the
known no-target kind. Fault captures: the opening's two white-light frames (presents 7885/7893), two black-frame
captures that are scene cuts (15177, 16394), 100 `no-target` captures (the known empty pixel shader), and **14
one-frame white flashes in the house** at 444-690 s (7.2-7.8 % of the picture). `fault-114-p25754.png` (660 s): the floor,
the shelves and the grass blown to white with lightning-like streaks, Link, the door and the lanterns right: Fault A,
as OOT-FPS described it (10 in its run, from 552 s). The harness shot `32-d29.png` (691 s) caught one too. Observation
only: Fault A is not this wave's; its rate has varied 3-10 a run before.

## Pictures

Every sheet of the baseline, v2 (sheets 2-3) and the final run (sheets 1-3) read against the baseline's: the colours as
OOT-FIX left them (dark interiors lit by the lanterns, deep blacks, Navi's glow, the blue title card), Link in his house
at every check shot, the walk through the house (with each run's own camera path). No picture changed with these
changes. The opening shots differ in timing only (the title's camera on the grass instead of the sky, the story shot on
Link asleep instead of his hands).

## Observations (not this wave's)

- **Memory:** peaks 28.3-30.2 GB; `fps2-v2-prof` rose 28.4 -> 30.2 GB inside its window (textures 6.4 -> 7.5 GB at
  ~50 FPS: the house's texture growth runs with the frame rate, as the ledger notes ~7 images a second). The final
  run 29.1 GB.
- **CPU writes held back past the upload budget** during the opening (~33-38 s, before any check shot): base one
  5-second line at 16 MB a frame; v1 on two lines (14 + 14 MB), the final 2.5 + 138 MB. Same line counts in every run,
  none after 680 s. Whether A1 lengthens these (a deferral moves `POOL_GEN`, so the next raw table still uploads every
  deferred write; before A1 every raw table did) is not shown; the pictures of the opening are right.
- The `oot` walk's path varies run to run (see Runs); a fixed-camera window would make these numbers comparable.

## What is left to 59 FPS

1. The game's own frame (the pacer's cost, ~21 ms median): which guest threads the main thread waits for, and what
   they wait on (a profile of a few named guest threads; sampling all ~90 with stacks exhausts memory).
2. The replay thread at ~17 ms in the heavier walk: lane joins 3.3 ms a frame, the pool sync 5.9 (hashing ~57 MB a
   frame of rewritten pages is memory-bound at ~1.7 ms with 13 threads; the write-watch calls 1.25 ms over ~500 MB of
   write-back pools scanned whole), set building 4.2 (`tex_view` waiting 1.5 ms for the images' lock behind the lanes'
   image making).

## Report (agent, protocol §5)

Branch `ootfps2` (base `a25c3c3`): `8b432ae` (plan), `53d3664` (A1, A2 lookup), `e0459d0` (B, hash threads),
`c06820f` (B out), `5aaa995` (C), `df2a180` (D), `af0ebb6` (C out), `aae004c` (D out), and this document. The final
code is `c06820f`'s: `gmem.rs` (A1), `upskip.rs` (A2), `docs/env_switches.tsv` (2 rows).

### 1. Built

- Diagnosis: the baseline profile puts 1.25 ms a frame of the replay thread in `gmem::bindings`, almost all waits for
  and walks under the pool lock (the 256 KB table copy 0.06 ms); the ~57 MB a frame of same-byte rewrites are the
  game's stores, taken by the replay thread's own scans (the walker takes ~3 MB), and hashing them is memory-bound.
- A1 (`NXGFX_GMEM_WALK_SKIP`, kept): raw-memory tables skip the pool walks and the pool lock while `POOL_GEN` stands
  since a walk that left nothing for later. `gmem::bindings` 1.25 -> 0.07 ms a frame.
- A2 (`NXGFX_HASH_THREADS`, kept): `prehash` lookups by one search a batch; 12 page-hash threads instead of 6. Copy
  and hash 3.37 -> 2.3-2.6 ms a frame, `prehash` 2.18 -> 1.65 ms, `upload_changed` 1.65 -> 1.32 ms.
- B, C, D: built, measured, taken out (moved nothing: walker hashes 0.02-0.07 MB used; hashing ahead 2 MB a frame,
  copy and hash unchanged; the lazy-image wait moved to the images' lock).
- Hypothesis (b), set reuse: not built (the profile shows no reuse rule that would pay; see Changes).

### 2. Tests (`cargo test --release -p graphics`: lib 324 passed, 6 ignored; other targets pass)

- `gmem::tests::tables_walk_the_pools_only_when_the_generation_moved_since_a_clean_walk` (new): no clean walk yet
  walks; the same generation does not; a moved one does.
- `upskip::tests::prehashed_pages_are_found_by_one_walk_for_ascending_pages` (new): the walk finds each page's hash,
  0 for missing pages, and a batch's search starts at its first page.
- `upskip::tests::prehashed_pages_decide_the_skip_of_their_sync_only` (existing, its last check moved to the new
  lookup): passes.
- New behaviour, so no failing-before state. B, C and D had tests of their own (drain hands the latest walker hashes,
  usable walker hashes, ahead hashes equal hashes after, lazy IDs pending until made); they went out with their code.
- `seal::tests::a_large_pool_is_armed_in_pieces` failed once in a full parallel run (`seal.rs:751`) and passed on the
  rerun and in every run after: intermittent, in code this wave did not touch.

### 3. Verified

Runs in this worktree's `build/play/` (table above), GPU 0, all on route, sheets read. Baseline
`20261127-220112-fps2-base-prof` 29.5 / 26.0, replay + prep 19.30 ms (1,478 draws); final
`20261128-000649-fps2-final-prof` **29.8 / 28.0**, replay + prep **17.22 ms** (p50 16.47, 45 % under 15.8; 1,652
draws), pool sync 7.04 -> 6.01 ms, peak 28,334 -> 29,107 MB; `fps2-v2-prof` (same kept changes, lighter walk, 1,074
draws) 46.9 / 30.0 at 13.98 ms. Target 59: not reached. `oot-house --faults` 29.1 / 27.0, 14 Fault A flashes
(observation). Other load on the machine: another wave's cargo ran in short bursts (the build script twice refused to
start, ~23:15 and ~23:18); no other game ran on the machine during my runs as far as I saw.

### 4. Unproven

- What the game's frame (~21 ms by the pacer's cost) waits on: the next step is a profile of a few named guest
  threads.
- A fair FPS comparison: the walk's draws vary 1,074-1,946 between runs; the same-draw estimate above (-3 to -5 ms) is
  from window halves, not a fixed camera.
- Whether A1 lengthens the opening's held-back CPU writes (Observations).
- The lead's quiet-machine numbers.

### 5. Slips

- B, C and D were taken out with `git checkout <commit> -- <file>` (`mem.rs` to `53d3664` in `c06820f`;
  `mem.rs` and `upskip.rs` to `c06820f` in `af0ebb6`; `image.rs` to `c06820f` in `aae004c`): a git restore of whole
  files, not an editor edit; each restored file is identical to the named commit's.
- `fps2-v3-guestprof` crashed at 868 s (`memory allocation of 25165824 bytes failed`, fast fail): my profiler setting
  sampled every guest thread with stacks (`NXRS_HOST_PROF_THREAD=guest`), whose buffers took private memory from 27.5
  to 32.5 GB in 3 s. A diagnostic's crash, not the build's; killed nothing (the game exited itself).
- A `5aaa995` edit lost the tab after `switch` in the `NXGFX_HASH_THREADS` row of `docs/env_switches.tsv`; fixed in
  `af0ebb6` (the row is as in `c06820f`).
- Test builds and a dist build ran during some runs' boot (never inside an 845-915 s window).

### 6. Questions

None.
