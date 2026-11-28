# OOT-FPS2 — Ocarina of Time: the replay thread's frame under one vsync (target 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~22:00, worktree
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
