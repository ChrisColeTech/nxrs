# OOT-FPS — Ocarina of Time toward 59 FPS with the corrected picture kept

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 18:55, worktree
`D:\Projects\RecompLauncherEngine-ootfps`, branch `ootfps` (base `gameplay-harness` `14b8c34`, OOT-FIX merged, oot /
oot-house route references re-taken from OOT-FIX's correct pictures). Protocol `docs/31-DELEGATION-PROTOCOL.md`.
Title: Ocarina of Time only, GPU 0. Deadline 5 h (23:55).

## Plan (written first)

Reference numbers: OOT-FIX's final run `D:\Projects\RecompLauncherEngine\build\play\20261127-182603-of-final2`
(report.md: FPS in [845, 915] s mean 27.7, 5 % low 19.3, min 9.0; frames CSV over the window: frame 35.7 ms, replay
10.44 + prep 11.04 ms, pool sync 10.65 ms, GPU 10.1 ms; peak 28.6 GB). The wave's baseline is its own first run with
diagnostics (the host profiler on `vk replay` and the completion thread over the walk window).

1. **Write-backs (~59 MB a frame).** The pools line of `of-final2` at 854.6 s: `write-back 1.31 MB` landed a frame but
   `58.83 MB re-armed at landings` and `re-arming ... 5.73 ms before [the pool lock]` (`upskip::landing_pre` on the
   completion thread). So ~59 MB a frame of ranges noted as GPU-written are copied to host memory twice (result and
   baseline) and compared page by page, and ~98 % of it is unchanged. Find which ranges these are (bindings, pools),
   whether the GPU changes them at all, whether the CPU reads them; keep on the GPU what needs no copy-back, and cut the
   compare cost of what must land (candidate: compare result and baseline on the GPU, copy back only changed pages).
2. **Replay thread** (replay + prep 21.5 ms against 15.8): the host profile names the biggest per-frame costs; cut the
   largest.
3. **Memory** (+~2 GB since the table pools got buffers): host-import or lazily bind the table-only pools if it is
   straightforward.

Expected files: `src/graphics/src/vk/queue.rs`, `mem.rs`, `upskip.rs`, maybe `gmem.rs`, a new compute shader in
`src/graphics/shaders/`, `docs/env_switches.tsv`, this doc. Verification: `oot` and `oot-house --faults` on GPU 0,
route checks against the new references, every sheet read against `of-final2`'s, numbers from report.md / run.log.
What would disprove (1): the write-backs' unchanged pages are not where the frame's time goes (cutting them leaves
replay + prep, the game's fence waits and FPS where they were).

## Baseline (this wave's first run, with diagnostics)

`build/play/20261127-185906-fps-base-prof` (this worktree; build `14b8c34` relinked with a PDB,
`NXRS_LINK_LIBS=-Wl,/DEBUG`; host profiler `NXRS_HOST_PROF=850,40` on `vk replay` with stacks; GPU 0; nothing else
running but this agent's shell). report.md: **FPS in [845, 915] s mean 27.3, 5 % low 22.6, min 21.0**; run peak
29,616 MB; route checks on route (0.026-0.152). Frames CSV over the window (1,602 frames): frame 36.63 ms, GPU 9.75,
replay 11.01 + prep 13.59 = **24.60 ms** (p95 36.68; 13 % of frames under 15.8 ms), pool sync 13.15 ms, draws 1,770.

Pools line at 857 s: `landings first 1.55 syncs, 3.377 ms`, `lock wait 2.04 ms`, `write-watch scan 2.14 ms`,
`per-pool pass 4.40 ms`; completion: `write-back 1.51 MB in 8.483 ms`, `re-arming ... 7.24 ms before it`
(`landing_pre`), `58.65 MB re-armed at landings`, `57.62 MB of CPU-written pages not uploaded (same bytes)`,
`copy and hash 3.45 ms`. The log's pacing lines in the window: the game waited 6.5-12.7 ms a frame for fences.

Host profile of `vk replay` (53,565 samples over 850-890 s; symbolized with llvm-symbolizer against the PDB; shares
of all samples, idle included): the pool sync (`mem::begin_submit_ex`) **37.8 %** (~13.7 ms a frame), of it
`queue::land_first` (waiting for the completion thread's landings) **10.4 %**, a contended `g.mem` lock in
`begin_submit_ex` (held by the completion thread's landing) **9.3 %**, `upskip::prehash` 6.4 %, the upload pass
(`upload_changed`) 5.8 %, `watch::take` 2.7 %; command translation (`exec::record`) 19.2 %, of it set building
(`bind_sets`) 13.0 %. So ~7 ms a frame of the replay thread was waiting for the completion thread's write-back work.

## 1. The write-backs: what is copied back, and why

A storage buffer bound writable may be written anywhere in its bound range, so every such range a submission binds is
written back at its completion (`mem::note_gpu_write` -> `queue::submit_sub`): the range as the GPU left it (result)
and as it had it before the submission's work (baseline, copied in the submission's `pre`), both into host memory,
compared page by page on the completion thread, and only the words the GPU changed are merged into guest memory
(`mem::land_changed`: the CPU writes the same pages too, so a whole-range landing would undo them). In OoT the ranges
are a few buffers bound writable every frame that the CPU also rewrites every frame with the same bytes (57.6 MB a frame
of same-byte CPU writes; 58.7 MB re-armed at landings): ~59-68 MB a frame of ranges, 140-155 ranges a frame, the
largest `0x195eea000+48.2 MB` in the opening (~60 s, 0.87 landings a frame) and in the walk `0x25ed06000+9.5 MB`,
`0x29da46000+15.2 MB`, `0x29e97b000+15.2 MB`, `0x29cd9a000+6.3 MB` (run `fps-v1-prof`, pools lines). Of these the
GPU changes ~0.4-3 MB a frame (`write-back 1.51 MB`, baseline; 2.99 MB in `fps-v1-prof`'s window).

Does the game read them on the CPU? The pages the GPU changes are pages the CPU itself writes every frame, so they
are not handed to the device by the lazy write-back (`lazy`: a page the CPU writes is never held there; 4,000 other
pages are, 11-12 MB a frame not copied back). Whether the game reads the GPU-changed words is not shown; they are
merged so that a read sees them. What needs no copy-back is the ~98 % the GPU leaves as it was: that is what this wave
keeps on the GPU.

## Changes

### A. Write-backs compared on the GPU (`vk/wbdiff.rs`, `shaders/wb_diff.comp`, `queue.rs`, `upskip.rs`, `mem.rs`)

The baseline of each write-back range is copied into a device buffer (in the submission's `pre`, where the host copy
was made) instead of host memory; at the end of the submission's `main` a kernel compares the range with it, one
workgroup per guest page, and copies into the host result and baseline buffers only the pages the GPU changed and the
range's partial first and last pages, with a flag a page (1 left as it was, 2 copied). At completion the landing
takes its unchanged pages from the flags (`upskip::landing_pre_flags`; they are handed to `land_changed` as known
unchanged whatever `NXGFX_LAND_PRE` / `NXGFX_LAND_SKIP_SAME` say, so their garbage bytes in the recycled host buffers
are never read) and hashes only the changed whole pages. An unchanged page with no kept hash becomes unknown (hash 0:
its next CPU write is uploaded) instead of hashing bytes that did not come back. Ranges the kernel cannot read word
by word (address, size or buffer offset not a multiple of 4, over 65,535 pages) and devices without buffer device
addresses keep the old copy. Switch `NXGFX_WB_DIFF=0` (old path). The host buffers stay the same size (recycled).

### B. A landing's changed pages land in runs (`mem::land_changed`)

`land_changed` took the write watch (`GetWriteWatch` with reset) and reset it (`ResetWriteWatch`) once per changed
page: two system calls a page under the pool lock. With the compare cost gone this was the landing's cost
(`whole write-backs landed 6.69 ms` a frame in `fps-v1-prof`, ~730 changed pages a frame there). The changed pages are
now found first (`changed_pages`) and landed in runs of adjacent pages (at most 16), one take and one reset a run.

### C. The raw global memory write log read once from host-cached memory (`gmem::end_submit`)

The completion thread's `Post::Early` closure of every submission and segment read the table's write log (256 words)
twice, word by word, from the staging buffer the shaders write it in: device-local host-visible memory that the CPU
reads across the bus. Profile `fps-v2-prof2`: that closure was **7.4 %** of all samples of the two sampled threads
(~5 ms a frame of the completion thread, before the write-backs land and the game's events are published). At
submission (and at a segment's end, outside its pass) the GPU now copies the log into a recycled host-cached buffer
and the closure reads it once; a log ended mid-recording (a table replaced) is read once from where it is.

### D. Faster landing merge, lazy choice before the pool lock (`mem::merge_changed_bytes`, `apply_writebacks`)

Profile `fps-v3-prof2`: `land` 5.1 % of samples, 3.9 % of it `merge_changed_bytes`' byte loop, and
`lazy::take_lazy` 1.7 % (every page of every range, under the pool lock the replay thread's table builds wait on:
`gmem::bindings` waited 3.2 %). The merge now skips eight bytes at a time where the producer changed none and the
guest holds the baseline, and goes byte by byte (storing only changed bytes, as before) elsewhere; the pages each
write-back may hand to the device are chosen before the pool lock (their own lock), and used for watched pools only.
Profile `fps-v4-prof2`: `land` still 4.8 % of samples, almost all in the byte loop (the changed pages' words mostly
mix changed and unchanged bytes); the merge now works on whole words with byte masks (a word the producer changed
entirely is stored whole; a partly changed one stores only its changed bytes, as before; the kept count from the
masks), checked against the byte loop by a test.

## Runs (all `oot`, GPU 0, this worktree's `build/play/`, window [845, 915] s)

FPS from report.md; the rest from the frames CSV over the window (means) and the pools line at ~857 s of run.log.
Every run on route (route distances 0.007-0.152 against the re-taken references).

| run | build | FPS / 5 % low | replay + prep (p50, <= 15.8 ms) | pool sync | GPU | draws | completion write-back a frame | landings first / lock wait | peak |
|---|---|---|---|---|---|---|---|---|---|
| `20261127-185906-fps-base-prof` | `14b8c34` | 27.3 / 22.6 | 24.60 (24.78, 13 %) | 13.15 | 9.75 | 1,770 | 1.51 MB in 8.48 ms, re-arm 7.24 ms before the lock | 3.38 / 2.04 ms | 29,616 MB |
| `20261127-191632-fps-v1-prof` | + A | 29.4 / 27.0 | 22.31 (23.16, 23 %) | 6.58 | 8.94 | 2,013 | 2.99 MB in 10.58 ms (landed 6.69), re-arm 0.68 | 0.88 / 0.17 | 28,684 MB |
| `20261127-193426-fps-v2-prof2` | + B | 29.6 / 27.9 | 18.08 (16.58, 46 %) | 6.22 | 8.87 | 1,561 | 1.41 MB in 4.31 ms (landed 2.45), re-arm 0.40 | 0.00 / 0.25 | 28,380 MB |
| `20261127-195444-fps-v3-prof2` | + C | 29.7 / 27.0 | 19.25 (18.98, 32 %) | 6.05 | 8.57 | 1,514 | 1.52 MB in 4.78 ms (landed 2.61), re-arm 0.43 | 0.00 / 0.14 | 29,113 MB |
| `20261127-201413-fps-v4-prof2` | + D (word skip, lazy before lock) | 29.6 / 27.0 | 18.65 (17.72, 37 %) | 6.27 | 8.86 | 1,757 | 1.46 MB in 3.41 ms (landed 2.39), re-arm 0.41 | 0.00 / 0.19 | 29,498 MB |
| `20261127-203246-fps-final-prof` | `b63e3fc` (+ word masks) | 30.7 / 28.0 | 15.99 (15.91, 49 %) | 6.13 | 8.44 | **1,120** | 1.57 MB in 3.40 ms (landed 2.45), re-arm 0.42 | 0.00 / 0.19 | 28,705 MB |
| **`20261127-210135-fps-final2-prof`** | **`e48b6e7` (final: + small buffers kept)** | **29.6 / 27.3** | **19.49 (19.51, 31 %)** | 6.43 | 9.01 | 1,787 | 1.53 MB in 3.37 ms, re-arm 0.41 | 0.00 / 0.19 | 29,505 MB |

`fps-final-prof` and `fps-final2-prof` sample `vk replay` only, as the baseline. `fps-final-prof`'s window held fewer
draws (1,120 a frame against 1,514-2,013 in the others: the walk's camera took in less of the room), so its 15.99 ms
is not comparable; the final build's own run is `fps-final2-prof`. Between v3 and `fps-final-prof` the write-back host
buffers were made again ~7 times a second in the walk (`write-back buffers 9 made` -> 368 at 893 s; baseline 4 -> 14):
C's 64 KiB log copies passed the shelf's 64-a-size limit; `e48b6e7` keeps small sizes up to 16 MB a size
(`fps-final2-prof`: 9 -> 80 made over the run).

Profilers: base and v1 sample `vk replay` only (`NXRS_HOST_PROF_THREAD=vk replay,completion` matched one thread);
v2-v4 sample `vk replay` and `nvn-completion` (two threads, more sampler load).

What each change moved (its own stage): A: pool sync 13.15 -> 6.58 ms a frame, landings first 3.38 -> 0.88 ms,
`landing_pre` 7.24 -> 0.68 ms, the game's fence waits 6.5-12.7 -> 3.0-5.4 ms a frame (pacing lines), 5 % low
22.6 -> 27.0. B: the landing under the pool lock 6.69 -> 2.45 ms, write-back time 10.58 -> 4.31 ms, landings first
0.88 -> 0.00 ms; the game's fence waits 0.0-0.2 ms a frame. C: the closure's 7.4 % of samples is gone from the
profile (`fps-v3-prof2`: `wait_for_completion`, the completion thread idle on the GPU, 8.3 -> 14.5 %). D: write-back
time 4.78 -> 3.41 ms a frame. E (word masks): `land`'s merge in `fps-final2-prof` 3.37 ms of write-back a frame
against 3.41 in v4 (not separable from run noise; the merge's samples are not in the final profile, which samples the
replay thread only). FPS is 29.4-29.7 in every run from A on (30.7 in `fps-final-prof`'s lighter window): the pacer
holds two vsyncs while the replay thread's frame stays over ~15.8 ms on most frames (p50 16.6-19.5 ms).

## Before / after (report.md, `oot`, [845, 915] s)

- Baseline `fps-base-prof` (`14b8c34`): **27.3 / 22.6 / min 21.0**, replay + prep 24.60 ms (p95 36.68), pool sync
  13.15 ms, peak 29,616 MB. (OOT-FIX's `of-final2`, main checkout: 27.7 / 19.3 / min 9.0, replay + prep 21.48.)
- Final `fps-final2-prof` (`e48b6e7`): **29.6 / 27.3 / min 27.0**, replay + prep 19.49 ms (p95 30.65), pool sync
  6.43 ms, peak 29,505 MB; the game's fence waits 0.00 ms a frame in the window's pacing lines (baseline 6.5-12.7).
- Target 59: not reached.

Replay thread, host profile over 850-890 s, share of its samples (both `vk replay` only): pool sync
(`begin_submit_ex`) 37.8 % -> 18.8 %; of it `land_first` 10.4 % -> 0 (not in the profile), the contended pool lock
9.3 % -> 0.4 %, `prehash` 6.4 -> 5.4 %, `upload_changed` 5.8 -> 4.7 %, `watch::take` 2.7 -> 3.6 %; command translation
(`exec::record`) 19.2 % -> 22.3 % (set building `bind_sets` 13.0 -> 15.6 %); idle (`Condvar::wait`) 43 %.

## `oot-house --faults` (final build)

`build/play/20261127-211650-fps-final2-house` (`e48b6e7`, `NXGFX_FAULT_MAX=200`, `NXGFX_FAULT_PER_REASON=80` as
OOT-FIX's house runs): completed, normal exit, house window [610, 690] **29.1 / 27.0 / min 25.2** (OOT-FIX's
`of-final2-house` 29.1 / 27.0), peak 28,410 MB, on route (0.029-0.042 at d01/d10/d20/d30/last). Red lines: the two
known no-target kinds. Fault captures: the opening's two white-light frames (presents 7879/7887, the nightmare), two
black-frame captures that stay (13805, 15023: scene cuts), 97 `no-target` captures (the known empty pixel shader
`e55f45d0df50c572`), and **10 one-frame white flashes in the house** at 552.6, 562.2, 574.9, 608.2, 621.9, 641.2,
658.3, 673.5, 690.5, 691.1 s (7.3-9.3 % of the picture, each 100 % gone at the next present). `fault-108-p24229.png`
(658 s): floor, shelves and grass blown to white with lightning-like streaks, Link, the door and the lanterns correct:
OOT-FLASH's Fault A, as OOT-FIX described its 3 (of-final2-house, from 400 s) and 7 (of-final-house, from 482 s).
Ten in 140 s is more than OOT-FIX's last run showed; whether this wave's changes raise its rate is not shown (Fault
A's cause is open, and its rate varied 3-7 a run before this wave). The first house run on `b63e3fc`
(`20261127-204801-fps-final-house`, default fault limits) caught it once in a harness shot (`10-d07.png`, 650 s);
its house window 29.2 / 27.7, on route (0.008-0.045).

## Pictures

Every sheet of the baseline (sheet 1 read), v1, v3, `fps-final-prof`, `fps-final2-prof` (sheets 1-3) and both house
runs (sheets 1-3) read against `of-final2`'s sheets: the corrected colours as OOT-FIX left them (dark interiors lit by
the lanterns, deep blacks, Navi's glow, the blue nightmare title card, the moonlit title), Link in his house at every
check shot, the walk through the house. No picture changed with the write-back changes; the only wrong frames are
Fault A's one-frame white flashes above (house) and the harness shot `10-d07.png` of the first house run.

## Memory (brief item 3): not done

Peak 29.6 GB (baseline) -> 29.5 GB (final); the GPU compare adds 256 MB of device baselines (`write-back base
(device)`), its host buffers stay the size they were. Host-importing the table-only pools was not done: a pool is
backed for the table before any binding names it (pools made during play are backed at the next table, ~every pool
the game creates), so a pool imported for the table would stay host memory when a draw later binds its vertex,
index or storage data, and the GPU would read it across the bus; making that safe needs a switch from imported to
mirrored at the first real binding (a new buffer address, the table rebuilt), not straightforward. Lazy binding is
not possible for the table: a shader reaches these pools only through loaded pointers, which the engine never sees.

## What is left to 59 FPS

The frame is now the replay thread's own work: 19.5 ms mean (p50 19.5) against ~15.8 ms for one vsync (59 FPS needs
every frame under it; 31 % are in `fps-final2-prof`). The GPU is 9.0 ms; the game no longer waits for fences. From
the final profile (`vk replay`, ~33.6 ms of wall time a frame, 1 % of samples ~0.34 ms a frame):

1. Set building (`bind_sets` 15.6 %, ~5.2 ms a frame) and command translation on the replay thread; the translate
   lanes' share was not sampled. The pool table (`gmem::bindings`) still waits on the pool lock 2.3 % (~0.8 ms): every
   submission and segment builds its table under `g.mem` (iterating ~4,500 pools, staging 256 KB); a table kept per
   pool generation would remove that.
2. The pool sync's same-byte CPU rewrites: ~57.6 MB a frame hashed to find them unchanged (`prehash` 5.4 % +
   `upload_changed` 4.7 %, ~3.4 ms) and the write-watch scan (`watch::take` 3.6 %, ~1.2 ms; pools line 2.42 ms).
   These pages are the same ~67 MB the GPU write-backs cover; the game rewrites them every frame with the bytes the
   GPU copy already has.
3. The completion thread: landings 3.4 ms a frame (`landed 2.45` under the pool lock, `pages chosen 1.29` before it);
   `take_lazy` walks every page of every range (~16,000 a frame).
4. Fault A (white house flashes) is still open (OOT-FLASH).

## Report (agent, protocol §5)

Branch `ootfps` (base `14b8c34`): `9869f0b` (plan, A), `8945656` (B), `49c4116` (C), `bcd937f` (D, doc draft),
`b63e3fc` (word masks), `e48b6e7` (small buffers kept), and this document.

### 1. Built

- Diagnosis (write-backs): ~59-68 MB a frame of storage ranges bound writable, which the CPU also rewrites every
  frame with the same bytes, were copied to host memory twice (result, baseline) and compared on the completion
  thread; the GPU changes ~1.5-3 MB of them. The replay thread waited ~7 ms a frame for that work (`land_first`,
  the pool lock), the game's fences behind it.
- A: write-backs compared on the GPU; only changed and partial pages come back (`wbdiff.rs`, `wb_diff.comp`).
- B: a landing's changed pages land in runs of up to 16 (one write-watch take and reset a run).
- C: the raw global memory write log copied to host-cached memory and read once (it was read word by word from
  device-local memory, ~5 ms a frame of the completion thread).
- D / E: the landing merge eight bytes at a time (word masks; changed bytes alone still stored one by one where a
  word is mixed), the lazy pages chosen before the pool lock.
- Small write-back host buffers kept up to 16 MB a size (C's log copies made buffers again ~7 a second).
- Memory (item 3): not done (reason above).

### 2. Tests (`cargo test --release -p graphics`: lib 322 passed, 6 ignored; other targets pass)

- `upskip::tests::a_landing_compared_on_the_gpu_decides_as_one_compared_on_the_host` (new): flags with garbage
  bytes for unchanged pages give the same unchanged pages and hashes as the host compare; a page without a kept hash
  becomes 0. Without the `absent` rule it would fail (page 2 hashed from garbage; reasoned, not run against the old code:
  the function is new).
- `wbdiff::tests::chunks_follow_guest_pages_and_unaligned_ranges_do_not_fit` (new): chunk counts and the kernel's limits.
- `tests/wbdiff.rs::only_changed_and_partial_pages_come_back` (new, `--features gpu-tests`, run on GPU 1 while a game
  ran on GPU 0): the kernel on a range starting mid-page over six pages with two changed: flags `[2,1,2,1,2,2]`, the
  right bytes in the copied pages, untouched bytes elsewhere; no validation messages. Passed.
- `mem::changed_pages_tests::changed_pages_are_the_differing_pages_in_order` (new): the pieces a landing writes, a
  known-unchanged garbage page not compared.
- `mem::changed_pages_tests::the_word_merge_is_the_byte_merge` (new): the word merge against the byte loop on random
  data, lengths 0-4,099, both versions of the merge (D and E).

### 3. Verified

Runs in this worktree's `build/play/` (table above; all `oot` on route, sheets read); final build `e48b6e7`:
`fps-final2-prof` 29.6 / 27.3 (baseline 27.3 / 22.6; target 59) and `fps-final2-house` 29.1 / 27.0, on route, ten
Fault A flashes (above). Red lines in every run: only the known no-target kinds (and an `audio: no output device`
line from the machine in some runs). As far as this agent knows, nothing else ran on GPU 0 but its own builds, tests and symbol
lookups (see Slips).

### 4. Unproven

- Whether the changes raise Fault A's rate (10 in the final house run against 3-7 in OOT-FIX's): needs Fault A's cause.
- Whether the game reads the GPU-changed words on the CPU (they are merged as before).
- E's own effect (the final profile samples only the replay thread; write-back time 3.41 -> 3.37 ms).
- FPS on a quiet machine (the lead's run).
- The GPU compare on devices without buffer device addresses or with unaligned ranges falls back to the old copy (not
  exercised in OoT: every range there fit).

### 5. Slips

- Builds or test compiles ran during the boot of some runs (`fps-final-house`, `fps-final2-house`), never inside a
  measured window; symbol lookups (llvm-symbolizer, single-threaded) ran during runs v1-v4's boots; one ran while v1
  was at ~262 s and that run's `04-house.png` screenshot took 11.8 s to write (not in the window).
- The first two scratch files of this wave (`relink.ps1`, `sym.py`) were written into the shared scratchpad root
  where other sessions' files live (`relink.ps1` may have replaced an older file of the same name); later ones are in
  `scratchpad/ootfps/`. No repository file was written by a script.
- The v2 profile's DLL and PDB were replaced by the v3 relink before its line-level analysis (only its function-level
  table, made before, is used here).

### 6. Questions

None.

