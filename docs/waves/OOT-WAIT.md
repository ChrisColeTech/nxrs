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

## The chain (baseline `20261128-010456-wait-base`, `NXRS_WAIT_ACCT=1`, `oot` [845, 915] s)

The `waits:` blocks over 850-910 s (ms a second; per frame at the baseline's 29.7 FPS). OoT's game frame is a main
thread (`0x1001`) and a job system of eleven guest threads: five that the main thread kicks (`0x1042`-`0x1046`, each
woken at its own word `0x...248` by `0x1001`) and six below them (`0x1047`-`0x104c`, woken by the first five). The
render thread `0x1004` waits once a frame for the main thread and spends 0.1 ms a second in its submits and presents:
the replay queue never held the game back (room and publication waits 0.00-1.5 ms a frame in the pipelining lines).

| main thread `0x1001` (baseline) | ms a second | a frame |
|---|---|---|
| waiting for its jobs (address `0x1374b8604`, ended by `0x1042` 43-50 %, `0x1043`-`0x1045`) | 360 | 12.1 |
| pacing in `nvnWindowAcquireTexture` (the hold at 2 vsyncs) | 344 | 11.6 |
| `svcSleepThread` (95-111 a second) | 57-66 | ~2.0 |
| waiting for a virtual core | 31 | 1.1 |
| waiting for job thread `0x1047` (`0x2ad34b61c`) | 25 | 0.8 |
| running (OS CPU time) | 9 % | 3.1 |

The job threads were busy only 10-15 % each (3-4 ms of CPU a frame): the jobs' wall time was not their work. Two
things stretched it, on every one of the twelve threads:

1. **Sleeps.** The second diagnostic run (`20261128-012253-wait-diag2`, sleeps keyed by the time asked; `=2`: host
   function and SVC time too) shows every one of these sleeps asks for **20 us** — the job system's backoff while it
   polls its queue words — and took **0.58 ms** (37.1 ms a second for 64 sleeps on the main thread; the same on each job
   thread, ~3 a frame each): `std::thread::sleep` wakes on Windows' high-resolution waitable timer, ~0.6 ms past the
   time asked. On the console `svcSleepThread` wakes on the kernel's tick (microseconds). A worker that backs off
   once while its next job is posted comes back 0.56 ms late, and the main thread waits for the last of them.
2. **The pool lock.** `nvnMemoryPoolFlushMappedRange` (the main thread and the five first-level job threads, ~300 a
   second each) took the pool lock (`mem::cpu_flushed`), held by the replay thread's pool syncs for ~6 ms a frame:
   15-38 ms a second per thread, up to 5 ms a time (diag2 call times).

The chain therefore ended in the host, not in a guest dependency: the host's sleep granularity and the replay
thread's pool lock, both on the job system's critical path. (diag2's per-call accounting itself slowed the frame, 28.6
FPS: `=2` is kept apart from `=1` for that reason, and diag2's FPS is not used.)

## Changes

### 0. Wait accounting (`NXRS_WAIT_ACCT`, `src/runtime/src/waitacct.rs`; diagnostic)

Every blocking wait of a guest thread goes into one table by (thread, kind, object): kernel waits (lock word,
condition key, address, the handle that ended a WaitSynchronization, sleeps by the time asked), blocking host
functions (`sched::without_core`, keyed by the host function), the wait for a virtual core; with the count, total,
longest, and who ended it (the guest thread that unlocked, signalled or woke it — a condition variable's signaller, not
the lock's later handoff — a host thread by name, or a timeout). `=2` adds the wall time of every host function and
SVC. A `waits:` block every 10 s, per thread with its OS CPU share and the time in no counted wait ("other": 0-7 % in
these runs, so the accounting covers the threads' time).

### 1. SleepThread sleeps as long as asked (`NXRS_SLEEP_SPIN_US`, default 1000; `src/runtime/src/kernel.rs`)

`svcSleepThread(ns)`: the host's sleep for all but the last 1 ms of it, then `yield_now` until the time asked is up
(a sleep of 1 ms or less is all yields). After: 20 us asked, 20 us slept (41 ms a second for ~2,000 sleeps on the main
thread). The cost is host CPU while yielding: the render thread `0x1004` sleeps 1 ms ~250 times a second and now spends
it yielding (30 % of a core).

### 2. Flushes do not wait for the pool lock (`NXGFX_FLUSH_QUEUE`, default on; `src/graphics/src/vk/mem.rs`)

`cpu_flushed` tries the lock; held, the range goes to a queue (`FLUSHED`) that the lock's next taker moves into its
pools' pending ranges before reading any (`take_flushed` at the top of `begin_submit_ex`, `sync_range` and the sealing
pass). A flush made before a submission is queued is in the queue or its pool before that submission's sync takes the
lock, so it is uploaded in that submission as before. In the window 28.5 flushes a frame were queued instead of waiting.

## Runs (`oot`, GPU 0, this worktree's `build/play/`, window [845, 915] s, all on route, sheets read)

| run | build | FPS / 5 % low | frame ms | pacer cost median (hold) | replay + prep (p50, <= 15.8) | pool sync | GPU | draws | peak |
|---|---|---|---|---|---|---|---|---|---|
| **`20261128-010456-wait-base`** | `41c108c` (accounting only) | **29.7 / 27.0** | 33.67 | 1.28-1.30 (2 vsyncs) | 16.46 (15.57, 52 %) | 5.94 | 8.87 | 1,716 | 29,227 MB |
| `20261128-012253-wait-diag2` | `75d018e` (`=2`, per-call accounting) | 28.6 / 20.0 (diagnostic) | | | | | | | 28,572 MB |
| **`20261128-014336-wait-fix1`** | `ab3d2ad` (1 + 2) | **47.1 / 30.0** | 21.10 | 1.24 at 853 s, 1.11 at 888 s (1 vsync, late frames paid back) | 16.64 (16.32, 44 %) | 7.16 | 9.09 | 1,522 | 30,727 MB |

Main thread `0x1001`, window means of the `waits:` blocks (850-910 s), ms a second / a frame:

| | baseline (29.7 FPS) | after (47.1 FPS) |
|---|---|---|
| waiting for its jobs (`0x1374b8604`) | 360 / 12.1 | 328 / 7.0 |
| sleeps | ~60 / 2.0 (0.58 ms each) | 41 / 0.9 (20 us each) |
| `nvnSyncWait` (a GPU fence) | 0 | 226 / 4.8 |
| pacing in the acquire | 344 / 11.6 | 87 / 1.8 |
| virtual core | 31 / 1.1 | 37 / 0.8 |
| running | 9 % / 3.1 | 23 % / 4.9 (polling instead of oversleeping) |

## Where the chain ends now

With the jobs on time the main thread's next wait is `nvnSyncWait` on a GPU fence, 4.8 ms a frame: the fence of work
the replay thread has not finished. The pipelining lines put the replay thread at 17-19 ms busy a frame (idle 2-11 ms)
at ~48 FPS, and its pool sync grew 5.94 -> 7.16 ms a frame: "landings first" 1.8 ms a frame (`queue::land_first`, which
runs only while a game thread waits for a fence — now the main thread does) and the pool lock's wait 1.26 ms (the
completion thread's landings hold it: "completion: pool lock 4.31 ms"). So at 47 FPS the chain ends in the replay
thread and the completion thread's landings: as the brief's disproof case, the next cut is the replay thread's own
~17 ms (pool sync ~7 ms, set building, the lanes) and the landings' time under the pool lock. Not done in this wave
(deadline).

## `oot-house --faults` (final build)

`build/play/20261128-015941-wait-house` (`ab3d2ad`, `NXGFX_FAULT_MAX=200`, `NXGFX_FAULT_PER_REASON=80`): completed,
normal exit, house window [610, 690] **44.4 / 41.0 / min 34.0** (OOT-FPS2's `fps2-final-house` 29.1 / 27.0), peak
28,622 MB, on route (0.027-0.043 at d01/d10/d20/d30/last). Captures: 101 `no-target` (the known empty pixel shader), 2
black-frame captures, the opening's two white-light frames (153 s), and **10 one-frame white flashes in the house**
(420-689 s): `fault-113-p33278.png` is Fault A (floor, shelves and grass blown to white with lightning-like streaks;
Link, the door and the lanterns right). Observation only (3-14 a run before).

## Pictures

Every sheet of the baseline, the after run and the house run read: OOT-FIX's colours (dark interiors lit by the
lanterns, Navi's glow, the title card), Link in his house at every check shot, the walk with each run's own camera
path. The after run's walk goes further in the same time (more frames: Link reaches the door, "Open"). The opening
shots differ in timing only. The baseline's `21-walk1` (832 s) caught a Fault A frame.

## Report (agent, protocol §5)

Branch `ootwait` (base `667fdbe`): `642b612` (plan), `41c108c` (accounting), `75d018e` (calls / SVCs, sleeps by time
asked), `1697d85` (fixes 1 and 2), `a0595fc` (calls only with `=2`), `ab3d2ad` (test), and this document.

### 1. Built

- Diagnosis (accounting, change 0): the main thread waited 12.1 ms a frame for its job threads, which were busy only
  10-15 %; on every one of the twelve threads 20 us sleeps took 0.58 ms (host timer granularity) and pool flushes waited
  up to 5 ms behind the replay thread's pool lock.
- Fix 1 (`NXRS_SLEEP_SPIN_US`): svcSleepThread sleeps as long as asked (host sleep for all but the last 1 ms, then
  yields), as Horizon's tick-precise sleep.
- Fix 2 (`NXGFX_FLUSH_QUEUE`): nvnMemoryPoolFlushMappedRange queues its range when the pool lock is held; the lock's next
  taker moves the queue into the pools before reading pending ranges (same submission as before).

### 2. Tests

- `runtime` `kernel::tests::sleep_thread_sleeps_as_long_as_asked` (new): 20 us sleeps average under 0.3 ms and a 3 ms
  sleep is 3-5 ms. Fails with the host sleep alone (`NXRS_SLEEP_SPIN_US=0`: "20 us sleeps took 892.96 us each").
- `runtime` `waitacct::tests::waits_are_added_by_object_and_reported_with_their_wakers`, `wakers_are_named` (new): the
  table's totals and wakers, the report's order, shares and names, a call's time outside the waits' sum.
- `graphics` `vk::mem::seal_tests::queued_flushes_become_pending_at_the_next_lock` (new): queued flushes become their
  pools' pending ranges and busy entries, a range in no pool is dropped, the queue is emptied, a flush under the lock
  does the same. (New behaviour: no failing-before state.)
- `cargo test --release`: `runtime` 54 lib + other targets pass; `graphics` 325 lib passed, 6 ignored; `os` 181 passed,
  1 ignored.

### 3. Verified

Baseline `20261128-010456-wait-base` **29.7 / 27.0**, frame 33.67 ms, pacer cost median 1.28-1.30 vsyncs (held 2),
replay + prep 16.46 ms (p50 15.57), peak 29,227 MB. After `20261128-014336-wait-fix1` **47.1 / 30.0**, frame 21.10 ms,
pacer cost median 1.24 / 1.11 (held 1, late frames paid back), replay + prep 16.64 ms (p50 16.32), peak 30,727 MB. Wait
chain (main thread, a frame): jobs 12.1 -> 7.0 ms, sleeps 2.0 -> 0.9 ms, a new fence wait 4.8 ms (the replay thread),
acquire pacing 11.6 -> 1.8 ms. All on route, sheets read. House run 44.4 / 41.0, 10 Fault A flashes. Target 59: not
reached. Other load: none of mine during the measured windows except `cargo test` builds during the diag2 run (diagnostic
only).

### 4. Unproven

- The replay thread's ~17 ms and the landings under the pool lock (now the chain's end): next wave.
- Peak memory 30.7 GB in the after run (textures grow with the frame rate, as OOT-FPS2 saw at ~50 FPS): under 32 GB,
  close to it.
- The yields cost host CPU (the render thread's 1 ms sleeps are all yields: 30 % of a core); whether that slows the
  replay thread or its hash threads is not measured apart.
- Other titles with the sleep change: not run (not this wave's titles). Any game that backs off with short sleeps runs
  its threads on time now; any that sleeps 1 ms in a loop spends a host core yielding.

### 5. Slips

- None of §4's editing rules broken. The diag2 run's numbers are perturbed by my own `cargo test` builds during it (used
  only for the sleep times and call times, not as an FPS figure).

### 6. Questions

None.
