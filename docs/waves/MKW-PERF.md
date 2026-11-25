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

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with the before/after numbers.)_
