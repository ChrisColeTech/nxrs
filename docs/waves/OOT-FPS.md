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
