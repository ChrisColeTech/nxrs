# DIAGFIX — two diagnostic tools agents can trust (2026-11-26)

Wave agent, worktree `D:\Projects\RecompLauncherEngine-diagfix` (branch `diagfix`, base `b554235`). Title: Mario Kart
World only, GPU 0. Not touched: `vk/mem.rs`, `upskip.rs`, `lazy.rs` (UPSYNC2), OoT / DKB / Smash.

## Problems

1. **Route checks too strict for races.** `play_route.py` compares a whole-frame colour histogram with a reference
   shot. A race shot never lands at the same spot of the track (game time drifts with FPS), so the lead's
   `20261126-083101-lead-tc-off` / `-083639-lead-tc-on` (race on course, sheets read) were OFF-ROUTE at 0.63-0.78.
2. **Host sampler hazards** (`runtime/src/hostprof.rs`, only with `NXRS_HOST_PROF`): M1 a `Vec` push (possible
   reallocation) while a target thread is suspended (it may hold the heap lock: wedge); M2 unwinding
   (`RtlLookupFunctionEntry` / `RtlVirtualUnwind`, loader locks, raw stack reads) while the target is suspended;
   L2 the sampler could suspend itself; L3 lanes started after the sampler are never sampled.
3. **Graphics L1:** `addr_cache_on()` stays on in `NXGFX_TRANSLATE_GATE=all` reference mode.

## Plan

1. **Region checks.** A check may be a table: `race = { about = "...", regions = ["coins", "place"], max = 0.3 }`.
   Named HUD regions (fractions of the frame) are declared once per scenario family in `play_route.py`'s region
   table (or inline as `[x0, y0, x1, y1]`); each region's signature (histogram + small grey thumbnail) is recorded
   with the reference and compared alone; the shot is off route when any region is past its threshold. Plain-string
   checks keep whole-frame behaviour (OoT unchanged: the amiibo-menu run must stay OFF-ROUTE). Calibrate on every
   MKW run in `build/play` (on-route race shots vs boot/title/OoT shots), re-report the lead's runs and v5-oot with
   `py scripts/play.py report`. mkw-turn: decide (below) whether "reached the turn" can be told from the HUD.
   Reports of old runs use the current scenarios.toml spec for a shot when it has one (run.json's copy holds only
   the strings).
2. **Sampler.** Copy the context and a bounded stack slice (64 KB from Rsp, clipped to the thread's stack limits
   read once from its TEB at registration) while suspended, nothing else (no allocation: buffers made before);
   resume; then unwind against the copy (frame reads from the copy only; an address outside it ends the walk).
   Samples pushed after resume into a Vec reserved up front; stacks kept only with `NXRS_HOST_PROF_STACK=1`.
   Skip the sampler's own thread id; re-scan the thread list every ~0.5 s for new matching threads.
3. `addr_cache_on()` false under `gate::all()`.

Files: `scripts/play_route.py`, `scripts/play_report.py`, `scripts/play.py` (if needed), `scripts/test_play.py`,
`scripts/scenarios.toml`, `scripts/checkpoints.json`, `src/runtime/src/hostprof.rs`, `src/graphics/src/vk/exec.rs`
(one function), this file.

Verify: `py -m unittest scripts.test_play` (< 1 s), `cargo test --release -p runtime` and `-p graphics`; a profiled
MKW run (`NXRS_HOST_PROF=1 NXRS_HOST_PROF_THREAD="vk replay,vk translate" NXRS_HOST_PROF_STACK=1`) completes and
names the lanes; one plain `mkw` run ON-ROUTE with valid FPS; sheets read.

## As built

**1. Region route checks.** Calibration first (scratch scripts over every MKW and OoT run in `build/play`):
whole-region colour histograms and mean grey differences bleed the scene through the HUD's edges (the lead's
`083101-lead-tc-off/06-race_later` place badge over a white road line: 46.8 grey against a threshold that had to be
~42). What separates cleanly is the **share of a region's 12x6 sample points that match** the reference (mean RGB
difference <= 40 per point): coin icon 0.90-1.00 in a race vs at most 0.28 on boot/title/menu/OoT shots; place
badge (box kept inside the numerals) 0.88-1.00 vs at most 0.32. Threshold 0.6 for both (`REGION_SETS['kart-hud']`).
- `play_route.py`: `spec()` reads a check as a string (whole frame, as before) or `{ about, regions }` with a named
  set or inline `[x0, y0, x1, y1, min]` boxes (validated); `record` stores each region's sample points with the
  reference; `check` compares them (`regions: {name: (share, min)}`); a reference recorded without the regions gives
  `n/a (... record it again)`; `describe()` formats the distance column.
- `play_report.py`: the route table shows region shares; `scenario_checks` now prefers scenarios.toml's current
  checks (they go with the current references), falling back to run.json's copy.
- `scenarios.toml`: `mkw` and `mkw-turn` grid / race / t-shots are `regions = "kart-hud"`. The grid too: the HUD is up
  at the grid and a fast run is already racing there (`20261125-085542-mkw-perf5-scan4` was off route by whole frame).
  References re-recorded from the same runs as before (`055832-v5-mkw`, `031507-after-mkw-turn`).
- **mkw-turn judgement:** the HUD cannot tell whether the kart reached the banked turn by t37-t46 (that depends on
  FPS); its checks now mean "in the race", and the "about" text says where the kart is at ~60 FPS. The earlier
  `064044-hd-mkw-turn` / `065808-hd-faults-mkw-turn` (kart not yet at the turn) are now on route; whether the turn was
  shown is read from the sheets. Telling track position would need the minimap's Mario icon (not done).

Sweep of every run (`check` on all mkw/mkw-turn/oot/oot-house folders): every 11-25/11-26 MKW race run on route except
`20261125-000839-mkw` (race shot during the start countdown, no place badge: the run was behind; off, rightly) and the
11-24 `151327-mkw` / `plain1` / `plain2` (menus at the race shots: off, rightly). OoT unchanged: `054320-v5-oot`
OFF-ROUTE (0.42-0.70), `053140-v5-diag-oot-house` OFF-ROUTE.

**2. Host sampler** (`runtime/src/hostprof.rs`):
- M1: while a thread is suspended nothing allocates. Context read, `VirtualQuery` (kernel) for the stack region's top,
  then `rsp .. min(top, rsp + 64 KB)` (4 KB without stacks) copied into a buffer made before sampling starts; resume;
  only then the sample is built and pushed. Samples are 24 bytes (`Sample { ip, ret, first, depth }`), stacks go into
  a side `Vec<u64>` only with `NXRS_HOST_PROF_STACK=1`.
- M2: unwinding (`RtlLookupFunctionEntry` / `RtlVirtualUnwind`) runs after resume, on the copy: registers pointing into
  the copied range are moved into the buffer (again after each step, for frame pointers restored from the stack); the
  walk stops before a step whose rsp is outside the copied bytes; 64 KB of zeros follow the copy so a frame's saved
  registers read past the copied bytes are zeros inside our buffer (a zero return address ends the walk). The
  first-return-address scan (`ret`) reads the copy too.
- L2: `find_threads` never returns the calling thread. L3: the thread list is read again every 250 ms; threads that
  ended (`GetExitCodeThread`) or fail `SuspendThread` are dropped (before: a failed suspend ended all sampling). The
  head names every thread (`# thread <tid> "<name>": N samples, from T s[, ended]`).

**3.** `exec.rs` `addr_cache_on()`: false under `gate::all()` (`addr_cache_wanted(switch, gate_all)`).

**Lead's add (CI flake):** `samples_a_named_thread` / `samples_carry_the_unwound_stack` asserted > 50 samples in
0.2 s (a CI runner gave 37-38). They now assert a floor (`MIN_SAMPLES` = 10) plus the ratio checks (most samples in
this image / with a stack); the new late-thread test uses the same floor.

## Report

### Built

See "As built" (1-3 and the CI add). Diagnosis (calibration, sweep) kept apart from the corrections there.

### Tests

- `py -m unittest scripts.test_play`: 12 tests, 0.06 s. New: `test_region_check_follows_the_hud_not_the_scene` (same
  HUD over another scene is on route though the whole-frame histogram is past 0.3; a HUD 3/4 visible matches 0.67 and
  passes; no HUD is off route, `describe` text), `test_region_check_needs_a_reference_with_regions`,
  `test_bad_region_specs_are_refused`; the scenario test now parses every check with `spec()`. The region functions
  are new, so these cannot run against the old code; the old whole-frame result for the same case is asserted in the
  first test.
- `cargo test --release -p runtime`: 49 + 2 pass. New: `the_sampler_skips_its_own_thread` (the old code suspended
  its own thread and hung), `threads_started_later_are_sampled` (the old code sampled only threads present at start:
  1 thread, fails `(2 threads)`); `report_tables_as_host_prof_py` and the stack test adapted to the side list.
- `cargo test --release -p graphics`: 250 lib pass (6 ignored) + integration. New
  `addr_cache_tests::the_gates_reference_mode_turns_the_address_memos_off` (the old `addr_cache_on` ignored the gate).

### Verified (GPU 0, worktree dist, MKW relinked 09:10)

- `build/play/20261126-091111-df-prof-mkw` (worktree `build/play`), `NXRS_HOST_PROF=255,40,<file>`
  `NXRS_HOST_PROF_THREAD=vk replay,vk translate` `NXRS_HOST_PROF_STACK=1`: completed, normal exit, no hang; the log
  says `host profile: written`; head: 6 threads named (`vk replay`, `vk translate 1`..`5`, 35,533 samples each),
  213,198 samples in 40 s, 212,287 with a stack. FPS 29.8 / 5 % low 28.0 (sampler running). Route on (0.94-1.0).
  Sheet read: boot, title, menus, grid, race, lap 2 all drawn correctly.
- `20261126-091712-df-mkw` (plain): **on route** (coin 0.97-1.0, place 0.88-0.93), FPS valid: 29.6 mean / 25.5 5 %
  low / min 18.0 (target 60; MKW's 30 cap, as the lead's 29.4-30.7). Anomalies as the lead's runs: 21 boot stand-ins
  (none in the window), the known `pipeline-crashed.bin` skip (1080 draws a period, same as `083639-lead-tc-on`), gate
  29-96 a period. Sheet read: race and lap 2 correct. Another wave's OoT ran on GPU 1 during both runs.
- Re-reported with `py scripts/play.py report` (main checkout's run folders, reports overwritten):
  `083101-lead-tc-off` and `083639-lead-tc-on` now on route with valid FPS (29.4 / 30.7); `054320-v5-oot` still
  OFF-ROUTE (0.42-0.70, FPS invalid); `064044-hd-mkw-turn`, `065808-hd-faults-mkw-turn` on route (judgement above).

### Needs audit

- `hostprof.rs` `run_ex` / `unwind_copy`: what happens while a thread is suspended (threading, lifetimes of the copy
  buffer, register relocation into the copy, the zero pad as the read bound). Diagnostic-only code (`NXRS_HOST_PROF`).
- `exec.rs` `addr_cache_on`: trivial, but on the translation path's reference mode.

### Unproven

- M2 residual: a frame that uses a frame register (`UWOP_SET_FPREG`) whose value points above the copied 64 KB is not
  moved into the copy, so that one unwind step reads the live (committed, still-running) stack before the walk stops;
  no fault is possible there (committed stack memory), only a wrong frame. A frame with more than 64 KB of locals
  near the copy's end could read past the zero pad: not seen on these threads.
- The late-lane path ran only in the unit test; in MKW all lanes exist by 255 s.
- Telling track position (the banked turn) needs another signal (minimap icon); mkw-turn's route check only says "in
  the race".
- Sampling rate with stacks is ~890 samples/s per thread for 6 threads (64 KB copies), lower than the leaf-only rate.

### Slips

- `cargo fmt -p runtime -p graphics` reformatted 31 files in both crates (they are not fmt-clean), among them
  UPSYNC2's `mem.rs`, `upskip.rs`, `lazy.rs`. Reverted with `git checkout` before any commit (only `hostprof.rs`,
  mine, kept formatted); `exec.rs` restored and my edit re-applied with the editor. Nothing of it was committed.
- Re-reporting old runs rewrote their `report.md` / `report.json` / sheets in the main checkout's `build/play` (asked
  for by the brief).

### Questions

None.
