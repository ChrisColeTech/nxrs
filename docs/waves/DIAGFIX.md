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
