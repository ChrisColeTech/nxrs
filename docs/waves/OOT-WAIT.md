# OOT-WAIT — Ocarina of Time: what the game's threads wait on (target 59)

Owner: one delegated agent. Lead: main session. Opened 2026-11-28, worktree `D:\Projects\RecompLauncherEngine-ootwait`,
branch `ootwait` (base `gameplay-harness` `667fdbe`: OOT-FPS2 merged). Protocol `docs/31-DELEGATION-PROTOCOL.md`.
Title: Ocarina of Time only, GPU 0. Deadline 5 h.

## Plan (written first)

Starting point: OOT-FPS2's final run `D:\Projects\RecompLauncherEngine\build\play\20261128-000649-fps2-final-prof`
(`oot`, [845, 915] s): 29.8 FPS / 5 % low 28.0; replay + prep 17.2 ms a frame; the pacer's cost (`fstep::present`:
the frame's time less the pacing wait in the acquire) a median 1.26-1.34 vsyncs (~21 ms), so frames are held two
vsyncs. A host profile of the game's main thread (`fps2-v2-gameprof`, `guest 0x1001`) shows it busy ~10 % of the time,
~40 % of its samples blocked in a kernel wait (`kernel::block`) and ~36 % pacing in `nvnWindowAcquireTexture`. With the
pacing wait taken out, the main thread's frame is mostly a wait on other guest threads.

Hypothesis: the main thread waits on a guest worker (render, audio, or a job thread), which waits on a GPU fence, a
present, a host service or a virtual core; that chain sets the frame.

0. **Wait accounting (diagnostic, `NXRS_WAIT_ACCT`)**, light enough to run on every guest thread (OOT-FPS2's
   every-thread stack sampling ran out of memory): for each guest thread, every blocking wait is added to a small table
   by what it waited on — the kernel wait's kind and object (lock word, condition key, address, the handle that
   ended a WaitSynchronization), a blocking host function (fence, present and queue waits, services: through
   `sched::without_core`), and the wait for a virtual core — with its count, total time, and **who ended it** (the
   guest thread that signalled, unlocked or wrote the address, or the host thread by name). A `waits:` block in the
   log every 10 s: per thread its CPU share, its entry function, its top waits in ms a second with their wakers. From
   it the chain: what the main thread waits on, who ends that wait, what that thread waits on, down to a host cost.
1. **Baseline** with the accounting on: this worktree's dist, OoT relinked, `oot` on GPU 0, the frames CSV and the
   pools / pacer lines as OOT-FPS2 read them, plus the `waits:` blocks over the [845, 915] s window.
2. **The fix**: in source, at the end of the chain (expected candidates: a host wait that wakes late — a timeout or a
   polling loop where a signal should wake it; a guest thread waiting for a virtual core held by another; a fence or
   present wait standing behind the replay thread; an audio or service wait that is slower than the console's). The
   change is chosen from the numbers, written here with them, behind a switch (row in `docs/env_switches.tsv`).
3. **Disproof**: if the chain ends in the replay thread's own ~17 ms, that is said here with the accounting's numbers,
   and the replay thread's top cost is cut instead.

Excluded: Fault A (white house floor), skipped draws or hidden work, faked completions, weaker checks; OOT-FIX's
colours stay; live native NDVE off.

Expected files: `src/runtime/src/waitacct.rs` (new), `src/runtime/src/kernel.rs`, `src/runtime/src/lib.rs`,
`src/os/src/sched.rs`, then the fix's own files (unknown until the chain is found: likely `src/os/src/sched.rs`,
`src/runtime/src/kernel.rs`, `src/graphics/src/nvn/sync.rs` / `window.rs` / `queue.rs`, or a service under
`src/os/src/services/`), `docs/env_switches.tsv`, this document.

Verification: one normal `oot` run after the fix against the baseline and its diagnostics (no switch-off controls),
every sheet read, report.md's anomaly table and route checks; one `oot-house --faults` run (Fault A count as an
observation). `cargo test --release -p <crate>` for each changed crate, with tests for the new behaviour. Numbers: FPS
mean / 5 % low, the game frame's vsyncs (median, pacer line), replay + prep ms, the wait chain's times before and
after, peak memory, against the baseline and 59.
