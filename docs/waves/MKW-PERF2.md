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

**Race window [250, 325] s, same scenario: 22.9 FPS (5% low 19.0) -> 28.1 FPS (5% low 23.7)**; descriptor sets
12.2 -> 7.8 ms a frame, replay CPU 19.9 -> 16.0 ms, sets built whole ~3,450 -> ~540 a frame (the rest patched or bound
again). Averages of the 5 s / 60-frame log lines in the window (plain runs; `render` = `--diag render`, its own scale):

| run | FPS / 5% low | sets ms | whole / patched / reused a frame | scan ms |
|---|---|---|---|---|
| `20261125-010257-perf-final` (HEAD, round 1) | 22.9 / 19.0 | 12.2 | 3449 / 0 / 221 | 14.6 |
| `-012617-perf2-patch` (1: exact key, host copy, uniform/push patch) | 25.6 / 22.0 | 8.3 | 1687 / 1031 / 985 | 14.5 |
| `-013620-perf2-banks` (1 + 2: lazy banks, `room` cache) | 26.6 / 23.0 | 7.3 | 1722 / 1086 / 1019 | 14.6 |
| `-020524-perf2-final` (final tree: + 3, 4) | **28.1 / 23.7** | **7.8** | 536 / 2497 / 1181 | 11.1 |
| render `-011544-perf2-base-profile` (HEAD) | 23.8 / 18.2 | 13.5 (memory 2.0, views 1.6, banks 2.7, writes 4.9) | | 11.1 |
| render `-015916-perf2-classes-profile` (final) | 27.6 / 24.0 | 8.0 (memory 1.6, views 1.7, banks 1.2, writes 1.5) | | 11.4 |

### Built
Diagnosis (baseline profile): ~3,500 sets a frame at ~3.5 µs each; `write_db_set` alone 4.9 ms (descriptors written
one by one into the BAR-mapped chunk), banks 2.7 ms (c0 holds the base instance: rebuilt and staged per stage per
draw), and the key held state the set does not read (all 16 uniform buffers, push generation, extent, c0/c2
generations), so ~4% hit. A later profile counted why whole sets were built: c0 38%, c2 26%, no copy of the
module's last set 33% (8 copies a stage thrashed). Corrections, all in `vk/exec.rs` unless named:
1. **Exact key** (`set_deps`, `masked_ubos`): the key keeps only what the module's set reads (bound/address-read
   uniform buffers, push generation only with c24..29, c0/c2 generations and base instance/dispatch only when it reads
   c0/c2, extent only with the support buffer). **Host copy** (`Shadow`): a set is made in host memory (fill
   template copied whole, zeros past the buffer slots) and copied into the chunk once; the copy is kept.
2. **Lazy banks** (`bank_needs`, `QState::banks(.., need)`): c0/c2 built only when read, staged only when bound.
   `mem::room` keeps a thread-local window cache under `POOL_GEN` (as `resolve` does) instead of the pool lock per
   read-only storage buffer. `lost::buf_name` returns at once while nothing is named (it took a lock per descriptor).
3. **One copy per module** (32 a stage, `shadow_slot`): a set whose key equals a copy's (same submission/chunk) is
   bound again at its offset.
4. **Patch by class** (`res_class`, `patch_classes`, `patch_set`, `stage_set(.., only)`): a key differing from the
   module's copy (same module and submission, same `POOL_GEN`) remakes only the slots whose class changed (each
   uniform buffer, push, c0 + storage read from it, c2 + textures, support, draw records) through the normal
   `stage_set` code; `write_db_set(.., patch)` writes them over the copy. A pool change during a patch discards it
   (whole set built). Off with `NXGFX_SET_PATCH=0`, and whenever `NXGFX_DB_FILL=0`, `NXGFX_DB_POISON` or
   `NXGFX_MESH_BINDINGS` is on (they read the chunk). The descriptor-buffer fill stays: every set starts from it.
5. `perf.rs`: `sets_patched` in the replay line; `NXGFX_PROFILE=1` logs why whole sets were built (`note_set_miss`).

Scans: not changed. Lead rejected for now: the first scan must still walk every mirror (no cheaper OS query found
for "any page written"); a lukewarm tier would delay mid-frame CPU writes (excluded).

### Tests
- `exec::set_patch_tests` (4): the key covers only what the set reads (unbound uniform buffer leaves it; bindless
  gives no key); per-module copies, newest first, another module's copy never keyed; banks built/staged only when
  used; `patch_classes`/`res_class` map every key field to the slots made from it, never across module/submission.
- `mem::room_cache_tests`: the window cache answers only for the same `POOL_GEN`, inside the window, below the next
  pool. New rules: no old behaviour to fail against (the old code had no such rules).
- `cargo test --jobs 4 -p graphics --lib`: 188 passed, 0 failed (final tree).

### Verified
Engine `build.ps1 -Jobs 4 -SkipGnu -KeepCache` and MKW `--build` before each run; `py scripts/play.py mkw`. All seven
runs: 43/43 steps, normal exit, no device loss or hang, only the 2 known red lines. Sheets read:
`build/play/20261125-020524-perf2-final/sheet-1.png` and `-015916-perf2-classes-profile/sheet-1.png` against HEAD's
`-011544-perf2-base-profile/sheet-1.png`: grid, racing with HUD (coins 5 -> 7 -> 12, lap 1/3 -> 2/3, items, map,
other racers), no missing or wrong surfaces seen.

### Unproven
- Per-set fixed cost: patched sets cost nearly as much as whole ones (~2.5 µs; doubling patches, 1,151 -> 2,294 a
  frame, moved sets 8.1 -> 8.0 ms in profile). Not split (no sampling profiler run); next: profile `stage_set`'s common path and the 1.5 KB
  BAR copy per set.
- Scan 14.6 -> 11.1 ms in the final run is not from these changes (none touch scans; profile runs show 11 both
  before and after).
- No audit of the patch rule (protocol §4: descriptor change). Known gaps shared with cache hits, not new: a storage
  address read from uniform memory, or a sparse (`MapVirtual`) remap, without a bind or `POOL_GEN` change keeps the
  old descriptor. Unbound image slots are now zeros (before: whatever the chunk held).
- Other titles share these paths (all descriptor-buffer sets): not run (lead's call).

### Slips
- Two `--diag render` runs of my changes beyond the allowed baseline profile (`-014356`, `-015146`), used to split
  the new sets time and count whole-set causes before change 4. Screenshots of four intermediate runs deleted
  (sheets and logs kept).

### Questions
- None.

## Lead review (2026-11-25 02:30)

Accepted after an audit fix. Independent audit (protocol §4, read-only): every traced path gives the descriptors a
freshly built set would, except **one defect (high impact, rare), fixed by the lead**: a patch discarded on a
`POOL_GEN` change has already taken its offset, and the whole-set fallback takes another, so a draw could write one set
more than `bind_sets` reserved and run past a full 4 MiB chunk (host write outside the mapping; GPU read outside the
bound descriptor buffer). Fix: `bind_sets` reserves two sets a module; `Sub::db_alloc` refuses (red line, status 72)
any set that would end past its chunk instead of writing out of bounds. Recorded limits: (a) two pre-existing gaps
now hit more often because reuse is common — a storage address read from uniform memory, and a sparse `MapVirtual`
remap, without a bind or `POOL_GEN` change keep the old descriptor (within one submission); (b) a missing written
storage buffer's zero scratch is shared by reused/patched copies (already a red-logged path); (c) unwritten image /
texel slots are all-zero bytes, not a driver `nullDescriptor`; (d) patched re-binds log as `W` not `H`.
Verified after the fix: graphics lib 188 passed; engine + MKW rebuilt,
`build/play/20261125-022020-mkw-perf2-audit-fix`: 43/43, normal exit, **28.8 FPS (5% low 25.0)** in [250, 325], the two
known red lines, sheet read (grid, racing on Bullet Bill, lap 2/3, coins 7 -> 8).
