# MKW-PERF2 — Mario Kart World race frame rate, round 2 (descriptor sets, scans)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Problem (measured — do not re-establish).** After `docs/waves/MKW-PERF.md` (read its report and lead review), MKW's
race runs at 22.9 FPS (5% low 19) against 60: `build/play/20261125-010257-perf-final`, window [250, 325]. The replay
thread is the bottleneck (the game waits ~26 ms a frame on syncs). Left per frame: **descriptor sets ~12 ms**
(~5,000 sets, ~4% reused; per-draw uniform buffers change the set key — `exec.rs` `set_key`/`SetKey`, `stage_set`,
`write_db_set`, `bind_sets`), write-watch scans ~14.5 ms (~3 a frame; the frame's first asks every pool), replay ~8 ms.

**Goal / done.** Cut the frame at these costs, sets first, each change measured before/after in the same scenario
(`py scripts/play.py mkw`, window [250, 325]: FPS, 5% low, frame/sets/pools/replay ms from the `pools:` and
`--diag render` lines). Done = a substantial race-FPS gain with correctness kept (a reused or cheaper set must name
exactly what a freshly written one would; the descriptor-buffer slot fill from `MKW-LOSS.md` stays), CPU tests for
each rule, the race sheet looked at (racing, HUD, no new red lines, no hang).

**How to work (owner's direction).** Start from the evidence and the source; change code first; rebuild engine and MKW
only to measure a change. One `--diag render` profiling run of the baseline is allowed to split the sets time. No
other titles (the lead decides checks of other titles on shared paths).

**Leads (yours to confirm or reject):** where the 12 ms goes (resolve + pool lock per UBO/SSBO, descriptor writes,
hashing); whether per-draw-changing uniform buffers can be bound without rewriting the whole set (e.g. dynamic
offsets / a separate small set or push-address path for the changing binding, so the rest of the set is reused);
batching pool-lock acquisitions per draw; for scans, whether the frame's full first scan can skip pools with no
write-watch hits cheaply.

**Baseline:** branch `gameplay-harness`, HEAD `79d9259` (+ this file), clean tree; `dist/nxrs` = HEAD engine; MKW built
with it.

**Exclusions:** no skipping draws, frame skipping, lower resolution or weakened robustness; the pool-finalize patch
(`POOL-ORDER.patch`) stays shelved; native NDVE off; no commits.

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol §3
(one build or game at a time; never a build or cargo test while a game runs; `--jobs 4`). Foreground runs with
timeouts under 10 minutes, or bounded foreground polls — never end a turn waiting on a background notification.
Engine `pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (~4 min), then
`dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build` (~40 s); run `py scripts/play.py mkw` (~6 min). Tests
`cargo test --jobs 4 -p graphics --lib`. A device loss or hang stops that path: fix it, never re-run unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min; no early hand-off.

## Report (agent, protocol §5)

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with the before/after numbers.)_
