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
