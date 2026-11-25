# MKW-PERF — Mario Kart World race frame rate

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Problem (already measured — do not re-establish).** MKW races at 14-18 FPS against 60. A race frame is 61-71 ms
(`docs/waves/MKW-RACE.md` report table; latest `build/play/20261125-001723-mkw-head-control`: 17.7 FPS in
[250, 325] s). Per frame: **pool sync 34-41 ms**, descriptor sets ~12 ms, replay ~21 ms. Pool sync is ~11 write-watch
scans a frame of ~2.8 GB of mirrors at ~4 ms each; ~6 of them are parked-queue resumes, which always scan
(`vk/mem.rs` `begin_submit`, `scan_covers`; `vk/queue.rs` parked queues). The `[vk] pools:` log line and
`--diag render` break these down per frame.

**Goal / done.** Make the race playable: cut the frame time at its largest proven costs, in order (pool sync first),
each change measured before/after in the same scenario (`py scripts/play.py mkw`, window [250, 325]: FPS mean,
5% low, frame ms, pool sync / sets / replay ms). Done = race-window FPS raised substantially and every change kept
correct: CPU writes the GPU needs are still uploaded before it reads them (the scan-skip audit in `MKW-RACE.md`
shows the hazards: CPU event signals, failed watch reads), with a CPU test per correctness rule you rely on, and the
race sheet looked at (racing, HUD, no new red lines, no hang).

**How to work (owner's direction).** Start from this evidence and the source. Change code first; rebuild the engine
and MKW only to measure a change. No exploratory rebuild/re-run loops, no other titles (the lead decides any check
of another title that shares a changed path).

**Leads (yours to confirm or reject from the source):** make a parked queue's resume reuse a scan that already covers
its work (it needs the same event-signal guard); scan only pools that have mirrors the queued work can touch, or
only pages written since the last scan (GetWriteWatch already resets: why does a scan cost ~4 ms of walking?);
then sets (12 ms) and replay (21 ms). Shelved/forbidden: the pool-finalize redesign (`docs/waves/POOL-ORDER.patch`
hangs MKW).

**Baseline:** branch `gameplay-harness`, HEAD `9f0ebb2` (+ this file), clean tree; `dist/nxrs` = HEAD engine; MKW
built with it.

**Exclusions:** no skipping draws, blacklisting, frame skipping, lowering resolution or weakening robustness to gain
frames; native NDVE off; no commits.

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol
§3 (one build or game at a time; never a build or cargo test while a game runs; `--jobs 4`). Run builds and games
in the foreground with timeouts under 10 minutes; never end a turn waiting on a background notification. Engine:
`pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (~4 min); then
`dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build` (~40 s). Run: `py scripts/play.py mkw` (~6 min).
Tests: `cargo test --jobs 4 -p graphics --lib`. A device loss or hang stops that path: fix it, never re-run unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min; no early hand-off.

## Report (agent, protocol §5)

**Race window [250, 325] s, same scenario: 17.7 FPS (5% low 13.0) -> 22.9 FPS (5% low 19.0)**; pool sync 29.2 ->
16.8 ms a frame (write-watch scan 28.0 -> 14.5 ms), syncs covered by an earlier scan 1.9 -> 7.0 of 10 a frame.
Sets (~12.5 ms) and replay are unchanged. Averages of the run's 5 s log lines in the window:

| run | FPS / 5% low | scan | pools (prep) | sets | covered |
|---|---|---|---|---|---|
| `-001723-mkw-head-control` (HEAD) | 17.7 / 13.0 | 28.0 | 29.2 | 12.5 | 1.9 |
| `-004215-perf-resume` (change 1) | 18.6 / 12.0 | 26.2 | 28.2 | 11.8 | 2.1 |
| `-005427-perf-gpuland` (1 + 2) | 23.9 / 17.0 | 11.2 | 13.4 | 12.8 | 7.0 |
| `-010257-perf-final` (final tree) | 22.9 / 19.0 | 14.5 | 16.8 | 12.2 | 7.0 |

### Built
Diagnosis (from the new counters in the `pools:` line): the ~6 parked-queue resumes a frame were released by **GPU**
event signals landing at completion, and every landing went through `event_publish`, which bumped the audit's
CPU-signal counter (`note_cpu_signal`). So no scan ever covered a resume, and the counter moved several times a frame,
also voiding coverage for ordinary ops. Corrections:
1. `vk/queue.rs`: a parked queue's rest (and its presents) syncs as replay op `resume_due()` = the newest op the
   replay thread has taken (`AQ_TAKEN`, set before each op runs), instead of `None` (always scan). What the rest may read
   was written by the CPU before the game queued it or the op whose GPU signal/receipt released it (both <= that op), or
   before a CPU signal (still refused by `scan_covers`). A stream that starts at a wait that does not pass now parks
   without a submission or pool sync (1 a frame in MKW); its queue state is created so its origin is kept.
2. `nvn/sync.rs` + `vk/queue.rs`: `event_publish(.., cpu)`; the completion thread's GPU event landings pass `false` and
   no longer count as CPU signals. `nvnEventSignal` still does (the audit's case is unchanged).
3. `vk/mem.rs`: the `pools:` line adds resumes a frame and submits parked at once.

Rejected from the source and a seconds-long CPU benchmark (`GetWriteWatch` on a 2.8 GB `MEM_WRITE_WATCH` block,
no title run): the ~4 ms is walking the range, ~1 ms/GB even with nothing written, and 4 or 8 threads are no faster
(the kernel serializes them), so splitting scans across threads does not help. A signal-scanner thread (scan at the
CPU signal) was written, then removed unmeasured: MKW has no CPU-released resumes (0 signal scans in `-005427`).

### Tests
- `mem::scan_cover_tests::a_scan_after_the_op_was_queued_covers_it` extended: a rest released by op 15's GPU signal is
  covered by op 15's scan, not by an earlier one for op 16, never after a CPU signal; a GPU landing
  (`event_publish(.., false)`) keeps coverage, `nvnEventSignal`'s (`true`) voids it. The landing assertion fails on the
  old `event_publish` (it always counted).
- `cargo test --jobs 4 -p graphics --lib`: 183 passed, 0 failed (final tree).

### Verified
Engine (`build.ps1 -Jobs 4 -SkipGnu -KeepCache`) and MKW rebuilt before each run; `py scripts/play.py mkw`. All four
runs: 43/43 steps, normal exit, no device loss or hang, the same 2 red lines as HEAD (`pipeline-crashed.bin` skip,
texture ID 337 target). Sheets read: `build/play/20261125-005427-perf-gpuland/sheet-1.png` and
`build/play/20261125-010257-perf-final/sheet-1.png`: grid, racing with HUD (position, coins 5 -> 6/7, lap 1/3 and 2/3,
items), compared with the HEAD sheet (`-001723`); dark close shrubs appear in HEAD's sheet too.

### Unproven
- Still ~23 FPS: the replay thread is the bottleneck (game waits ~26 ms a frame on syncs). Left a frame: ~3 scans
  (~14.5 ms; the frame's first one must ask every pool), sets ~12 ms (~5,000 sets, ~4% reused: per-draw uniform
  buffers change the key), replay ~8 ms. Next: an `NXGFX_PROFILE=1` race run to split sets (memory/views/banks/writes;
  every UBO/SSBO resolve takes the pool lock), not done here (no exploratory runs).
- No independent audit of the GPU-landing exemption (protocol §4 calls it a coverage-rule change): its argument is in
  `queue::resume_due` and `event_publish`.
- Other titles sharing `event_publish`/parking (DKB, PLZA, Violet) not run: lead's call.

### Slips
- A chained command started `nxrs --build` of MKW with the old engine after a failed first `build.ps1` call (00:52; ran
  to completion, then the engine and MKW were rebuilt). The harness moved game runs to background at 180 s; I waited on
  them with foreground polls, no build ran meanwhile.
- `-005427` ran a binary that still held the (disabled, `NXGFX_SIGNAL_SCAN=0`) scanner; `-010257` is the final tree.
- Screenshots of `-004215`/`-005427` deleted (sheets and logs kept).

### Questions
- None.

## Lead review (2026-11-25 01:25)

Accepted. Lead re-ran graphics lib (183 passed), read `-010257-perf-final/sheet-1.png` (grid, racing with HUD, coins
5 -> 6, items) and its report (43/43, 22.9 FPS, the two known red lines). Independent audit (protocol §4, read-only):
no case where the diff lets GPU work read a stale mirror that HEAD would have scanned, provided every CPU release of
an event wait goes through `nvnEventSignal`. Recorded limits: (1) a guest CPU store straight to an event word (not via
the API) now releases parked work without a scan — it widens the gap already recorded after the MKW-RACE audit, not a
new class; (2) the new `RESUMES` counter over-counts (every `resume_due()` call, incl. presents and re-parks):
diagnostics only. The GPU-landing exemption's happens-before chain (`AQ_TAKEN` AcqRel -> submission -> word Release
store -> Acquire read in the retry) was checked correct. Other titles sharing the path not re-run (owner: work the
broken titles).
