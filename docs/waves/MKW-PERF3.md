# MKW-PERF3 — Mario Kart World race frame rate, round 3 (per-set cost, scans, replay)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Problem (measured — do not re-establish).** After `docs/waves/MKW-PERF2.md` (read its report, "Unproven" and lead
review), MKW's race runs at **28.8 FPS (5% low 25.0) of 60**: `build/play/20261125-022020-mkw-perf2-audit-fix`,
window [250, 325]. The replay thread is still the bottleneck. Left per frame: descriptor sets ~7.8 ms (~4,200 sets:
~540 whole, ~2,500 patched, ~1,200 re-bound; a patched set costs nearly as much as a whole one, ~2.5 µs), write-watch
scans ~11-14.5 ms (~3 a frame; the frame's first walks every mirror), replay ~8 ms.

**Goal / done.** A substantial further race-FPS gain with correctness kept, each change measured before/after in the
same scenario (`py scripts/play.py mkw`, window [250, 325]: FPS, 5% low, frame/sets/pools/replay ms from the `pools:`
and `--diag render` lines). Done = the gain, CPU tests for each new rule, the race sheet looked at yourself (racing,
HUD, lap/coins advancing, no new red lines, no hang or device loss).

**Leads (yours to confirm or reject):**
1. **Per-set fixed cost.** Profile `stage_set`'s common path (what a patch still pays: key hashing, `set_deps`,
   resolve/room per binding, the fill-template copy) and the ~1.5 KB host->BAR copy per set (`write_db_set`). Candidate
   cuts: write only the changed slots of a patch into the chunk instead of the whole set; avoid rebuilding the key from
   scratch when one class changed; keep the shadow in place and bind an offset.
2. **Scans.** The frame's first write-watch scan walks every mirror. MKW-PERF2 found no cheaper OS query for "any page
   written"; look again from the source (per-pool dirty generation from the guest-write side, pools with no
   write-watch hits for N frames, `GetWriteWatch` batching). Excluded: delaying mid-frame CPU writes (a lukewarm tier).
3. **Replay ~8 ms.** Only after 1-2; profile before changing.

**How to work (owner's direction).** Fix first, rebuild only to measure. Start from the evidence and source; one
`--diag render` / `NXGFX_PROFILE=1` profiling run of the baseline is allowed; no exploratory rebuild/re-run loops. No
other titles (the lead decides checks of other titles on shared paths).

**Baseline:** branch `gameplay-harness`, HEAD at brief time (pull first); `dist/nxrs` = HEAD engine; MKW built with it.
Tree: `docs/32-REMAINING-WORK-PLAN.md` and this file are the lead's uncommitted edits — leave them.

**Files you may not edit** (another session owns them until CI is green): `.github/workflows/*`,
`src/launcher/**`, `src/runtime/tests/abi_layout.rs`. Expected files: `src/graphics/src/vk/exec.rs`, `vk/mem.rs`,
`perf.rs`, the write-watch/pool scan code, their tests.

**Exclusions:** no skipping draws, frame skipping, lower resolution or weakened robustness; the descriptor-buffer slot
fill (`MKW-LOSS.md`) and the `bind_sets` two-sets-a-module reservation / `db_alloc` bound check (MKW-PERF2 audit fix)
stay; `POOL-ORDER.patch` stays shelved; native NDVE off; **no commits**.

**Rules / commands:** `AGENTS.md` (editor tools only, no scripts that rewrite files; `py` with
`PYTHONDONTWRITEBYTECODE=1`; kill by PID, never by image name), protocol §3. **One build or game at a time, `--jobs 4`;
another session may run short `cargo test`s in this checkout — check `tasklist` for cargo/rustc before a build or a
game run and wait for them to finish.** Foreground commands with timeouts under 10 minutes, or bounded foreground
polls — never end a turn waiting on a background notification. Engine
`pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (~4 min), then
`dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build` (~40 s); run `py scripts/play.py mkw` (~6 min). Tests
`cargo test --jobs 4 -p graphics --lib`. A device loss or hang stops that path: fix it, never re-run unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min; no early hand-off.

## Report (agent, protocol §5)

**Race window [250, 325] s: 28.8 FPS (5% low 25.0) -> 29.1 FPS (5% low 25.2); write-watch scan 11.0 -> 8.5 ms a
frame.** The scan cut is real and repeatable (8.5-8.7 ms in all three runs of it, 11.0-11.4 in every earlier run);
the FPS gain is within run-to-run noise (draws a frame vary 1,860-2,020 between runs of the same scenario). Sets cost
is unchanged per draw (~3.8-3.9 us a draw). Main finding: **the game now waits only ~7.4 ms a frame on syncs** (26 ms
in round 1, 9.9 ms at baseline): the replay thread is no longer the frame's main limit; the game's own threads take
~27 ms of a 34 ms frame, so replay-side cuts can now buy at most ~7 ms (~37 FPS).

Averages of the 5 s / 60-frame log lines in the window (plain runs; `profile` = `--diag render` + `NXGFX_PROFILE=1`):

| run | FPS / 5% low | sets ms | whole / patched / reused a frame | scan ms | draws a frame | game waited ms |
|---|---|---|---|---|---|---|
| `20261125-022020-mkw-perf2-audit-fix` (baseline) | 28.8 / 25.0 | 7.33 | 508 / 2307 / 1091 | 11.04 | 1861 | 9.9 |
| `-031247-mkw-perf3-a` (1 draw words + 2 scan gaps + SHADOWS 128) | 29.5 / 26.3 | 7.57 | 518 / 2382 / 1151 | 8.55 | 1932 | 8.7 |
| `-032654-mkw-perf3-b` (a + 3 facts + raw-memory patches; raw and 128 reverted after) | 28.3 / 24.0 | 8.24 | 541 / 2505 / 1182 | 8.69 | 2022 | 12.4 |
| profile `-033631-mkw-perf3-profile` (final tree) | 28.1 / 23.5 | 9.04 (memory 2.0, views 1.7, banks 1.2, writes 1.6) | 557 / 2421 / 1188 | 8.42 | 1988 | 14.1 |
| **`-034357-mkw-perf3-final` (final tree)** | **29.1 / 25.2** | **7.16** | 517 / 2365 / 1104 | **8.52** | 1901 | **7.4** |

### Built
Diagnosis first (evidence in the profile run): a scratch benchmark of `GetWriteWatch` on this machine (2 GB
write-watched, Python ctypes) showed the cost is per page of the range asked, written or not (~1 ms/GB resident,
~0.75 ms/GB untouched, ~1.5-2 us a call), and that calls from several threads serialize in the kernel (1-8 threads:
2.5-2.8 ms for the same 2 GB). So a scan can only get cheaper by walking fewer bytes. Per stage set (profile, ns):
whole sets 810 before the bindings / 3,700 in them / 40 chunk copy; patched 550 / 1,250 / 40. The ~1.5 KB copy into
the chunk is not a cost (40 ns). Patches remade uniform buffers in 92%, c0 in 31%, c2 + textures in 40-45%.
Corrections:
1. **Scan gaps** (`vk/mem.rs` `watch_batch_end`, `WATCH_GAP`): two watched pools of one allocation share a
   `GetWriteWatch` call only when the gap between them is at most 1 MiB (about what one extra call costs to walk). The old
   rule let a run's gaps grow to half its pools' bytes (or 4 MiB), so runs over large pools walked hundreds of MB no
   pool holds at every scan. Same pages reported (distribution is by pool, unchanged). Profile: 219 calls / 2.37 ms a
   scanning submission (was 131-159 calls / 2.7-4.1 ms), 121 MB of gaps still walked a submission.
2. **Draw words** (`vk/exec.rs` `reads_draw_words`, `SetDeps::draw`, `QState::banks(.., draw0)`): the base instance
   (c0 + 0x644) and compute's dispatch size (c0 + 0x200..0x20c) are in the set key, and make c0 be built again, only
   when the set binds c0 or reads a storage record / handle overlapping those words. A set reading other storage
   records from c0 was patched (storage buffers resolved again, c0 rebuilt) at every new base instance. The c0 bank
   keeps the draw words it was built with, so a set that does read them still builds it again. No measurable sets-time
   change in MKW (its c0 patches are mostly storage-binding changes, `bgen[0]`).
3. **Module facts** (`exec::Facts`, `Module::facts` OnceLock in `vk/shader.rs`, constructors in `shader.rs`/`xfb.rs`):
   the set's deps, bank needs, draw words, texture-cacheability and raw-memory use are read from the bindings once per
   module instead of six passes per set.
4. Diagnosis only (`NXGFX_PROFILE=1`): `stage-set patches` line (classes remade; per-set prologue / bindings / chunk
   copy ns, whole vs patched), the whole-set "no last set" reason split, `MB of gaps` in the pool-sync line.

Tried and reverted (measured, no effect): 128 host copies a stage instead of 32 (whole sets 508 -> 518), and patching
sets with raw global memory (whole sets 541). The profile split says why: ~18,000 of every 20,000 whole sets have **no
copy of their module in the stage at all**, i.e. they are a module's first set in its submission (patches never cross
submissions), not capacity or raw-memory misses.

### Tests
- `exec::set_patch_tests::draw_words_count_only_for_sets_that_read_them`: records over / beside 0x644 (both sides,
  12-byte record), compute's 0x200..0x20c, c0 bound, a handle from c0, another bank at the same offset. New rule (no
  old behaviour to fail against: the old key always held the base instance).
- `exec::set_patch_tests::module_facts_match_its_bindings`: facts equal the per-call answers and are read once.
- `mem::watch_batch_tests::a_run_spans_only_small_gaps` (1 MiB joined, 1 MiB + 4 KiB split, each gap judged alone,
  other allocation split); `individual_gap_allowances_cannot_accumulate_into_a_giant_scan` updated (a ~4 MiB gap now
  splits; it joined before, so it failed against the old rule).
- `cargo test -p graphics --lib`: 193 passed, 0 failed (final tree).

### Verified
Engine `build.ps1 -SkipGnu -KeepCache` + MKW `--build` before each run; `py scripts/play.py mkw`. All four runs 43/43,
normal exit, no device loss or hang; only the 2 known red lines (run `a` also had 10x "pipeline compilation stopped
for clean exit" at 336 s, at exit, outside the window; not seen again). Sheet read:
`build/play/20261125-034357-mkw-perf3-final/sheet-1.png` (and `-031247-mkw-perf3-a`): grid, racing with HUD, coins
5 -> 7 -> 12, lap 1/3 -> 2/3, items, map, other racers, no missing or wrong surfaces. Overlap: during the final run
another agent's `nxrs-build` ran in its first ~3 minutes (before the window; none seen in 3:46:52-3:49:37, which covers
the window); runs a and b had no other game; the profile run overlapped the SONIC-SMASH agent's Shadow run for ~1 min
(diagnostic only). The measured builds include the SONIC-SMASH agent's uncommitted edits (`nvn/vpool.rs`,
`vk/image.rs`, cube-twin hunks in `vk/exec.rs`) and the lead's `vk/device.rs` change from the same tree.

### Unproven
- **The frame is now mostly the game's own CPU time** (~27 ms of ~34 ms; waits 7.4 ms). Further replay cuts are capped
  at ~7 ms. Next step: find where the game threads' time goes (engine calls on the game thread such as command
  recording / queue submit, locks it waits on — e.g. the pool lock the completion thread holds 6 ms a frame — vs the
  game's own recompiled code).
- Whole sets (~520 a frame, 3.7 us of bindings each, ~2 ms) are first-per-submission sets; patching across submissions
  would need every staged slot (push, c0/c2 bound, support, c1, mesh records, missing-storage scratch) remade: not done.
- Scan: still ~8.5 ms, now mostly bytes genuinely in pools (~1.6 GB lukewarm a mid-frame scan). Only a finer cold rule
  (per region of a pool) would cut it further, and that delays mid-frame writes (see Questions).
- Per-set fixed cost: `pool_bases` takes two mutexes per call (set key, set, push constants), ~0.3 ms a frame est.; not
  changed.
- No audit of change 2 (descriptor key rule); the rule is the set's exact reads, test-covered.

### Slips
- Ran `cargo test` once (03:20) while the SONIC-SMASH agent's game was running (old rule; since lifted).
- Two runs beyond the plain before/after pair: run b (two changes that were then reverted) and one profile run of the
  final tree (the brief allowed one baseline profile; PERF2's profiles stood in for the baseline). Intermediate runs'
  screenshots deleted, sheets and logs kept.
- Builds used `-Jobs 4` until the rule was lifted mid-wave.

### Questions
- Should a pool region (e.g. 2 MiB of a large pool) with no CPU write for 60 frames be treated like a cold pool (asked
  once a frame)? It would cut the mid-frame scans (~1.6 GB walked each) but delays a write to such a region by up to a
  frame, the same trade the existing cold-pool rule makes (MKW's glyph bug #250 came from that trade). Recommendation:
  not now: the scans are no longer the frame's limit; first find the game-thread time.
