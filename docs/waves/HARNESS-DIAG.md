# HARNESS-DIAG — the gameplay harness says what went wrong (2026-11-26)

Owner's ask: diagnostics tell agents exactly what is wrong instead of agents finding faults by luck in sparse
screenshots. Harness = gameplay troubleshooting only (AGENTS.md); no thermal/supervisor/latch/lease/gate scaffolding.

## Failures the harness should have caught

1. MKW dark banked turn: "a zero stand-in is bound" logged at 290.4 s just before `05-race.png`; report.md silent.
2. OoT one-frame white flash (herbs/pots white, `20261126-023504-v2-oot/15-h2.png`): seen only by luck.
3. Route desync: `20261126-054320-v5-oot` sat in the options/amiibo menu all run; its FPS was reported as if valid.
4. `[RED] a draw with no render target bound (... depth set)`: printed once, no frame/pass/draw/texture context.

## Plan

**A. Anomaly timeline (report.md).** `play_report.py` classifies the engine's log into anomaly events (red lines,
zero stand-ins, missing render targets, gate give-ups / re-translations, CPU writes held back, skipped draws from the
periodic `replay up to frame` totals, pipeline compiler crashes, query exhaustion, device/fault lines, fault
captures), each with its engine time and frame. The report gets a per-kind count table (whole run vs the scenario
window) and, per screenshot, the anomalies in the seconds before it. Shot times come from the engine's own
`screenshot: present N ... written to` line (engine clock and present number), not the harness clock.
Engine detail added where missing: the no-render-target skip gets a rate-limited context line (frame, submission,
pixel shader, each target's serial/size/format/target) every time it happens, not just the first red line.

**B. Route checkpoints.** A scenario declares `checks = { <shot> = "what it shows", ... }`. References are tiny
signatures (24x14 grey thumbnail + 4x4x4 colour histogram) of a known-good run's shots, kept in
`scripts/checkpoints.json` and recorded with `py scripts/play.py checkpoint <run>` after reading that run's sheets.
The report compares each declared shot with its reference; past the threshold the run is **OFF-ROUTE** (heading and
FPS line marked invalid). PNGs are decoded in pure Python (no PIL here).

**C. Fault-triggered capture (engine, opt-in `NXGFX_FAULT_SHOTS=<dir>`).** New module `vk/shot/fault.rs`: each
present is copied to a host buffer (async, like the F7 frame sink); on completion a half-resolution sample is
checked against the previous frame (newly fully-white / fully-black area). A trip, or an anomaly noted by the log
layer (any red line; the zero stand-in and no-target hooks), saves that presented frame (full resolution) and the
previous one (half resolution) plus a log line naming the reason. Captures are capped per run. `play.py --faults`
sets it to `<run>/faults`; the report lists the captures in the timeline. Documented in `docs/env_switches.tsv`.

**D. (if time) `--ab-lanes`:** the scenario run twice, `NXGFX_TRANSLATE_THREADS=0` then default, diffing anomaly
counts and checkpoint distances.

## Files

Mine: `scripts/play.py`, `scripts/play_report.py`, `scripts/test_play.py`, `scripts/scenarios.toml`,
`scripts/checkpoints.json` (new), `src/graphics/src/vk/shot.rs`, `src/graphics/src/vk/shot/fault.rs` (new),
`src/graphics/src/log.rs`, `docs/env_switches.tsv`, this file.
One-line hooks in others' files (listed in the report): `vk/exec.rs` at the zero stand-in and no-render-target log
sites.

## Report

### Built

- **A, anomaly timeline** (`scripts/play_events.py`, `play_report.py`): report.md gets `## Anomalies` (per kind: run
  total, window total, first/last s, steady/peak per period, example) and `## Near each screenshot` (non-periodic
  events in the 5 s before each shot; periodic counts only above twice their rate over the previous 12 periods).
  Shots are timed by the engine's own `screenshot: present N ... written to` line (engine clock and present number).
  Kinds: stand-in, no-target, skip-no-target / -no-pipeline / -no-shader / dropped-work (from the periodic replay
  totals), pipeline, gate (gave up + translated again), held-back, held-back-frame, query, fault, capture, red.
  Stand-in lines are annotated with the NVN target name (`target 4 = 2D array`). Engine side: a draw with no render
  target now logs `no render target: draw skipped #N (...): after present P, submission S, pixel shader K: colour i
  serX WxHxD fmt F target T, ...; depth ...` for the first 16 and then every power of two (the red line stays
  once-only).
- **B, route checks** (`scripts/play_route.py`, `scripts/checkpoints.json`, `[<scenario>.checks]` in
  scenarios.toml): a histogram + thumbnail signature per declared shot, compared with a known-good run's.
  Threshold 0.3 (OoT calibration: on-route 0.01-0.20, the desynced amiibo-menu run 0.42-0.70). A miss gives
  `# <s>: completed - OFF-ROUTE`, an OFF-ROUTE bullet and `**INVALID (off route)** FPS`. References recorded with
  `py scripts/play.py checkpoint <run>` for oot (v2-oot), oot-house (base-diag-oot-house), mkw (v5-mkw),
  mkw-turn (after-mkw-turn).
- **C, fault captures** (`src/graphics/src/vk/shot/fault.rs`, opt-in `NXGFX_FAULT_SHOTS=<dir>`, `play.py
  --faults`): each present copied back (shared `shot::copy_presented`, also used by the F7 sink now); on completion
  a ~480-column sample is compared with the previous present's. A trip (>= 2% newly fully white, >= 25% newly near
  black; `NXGFX_FAULT_FLASH`), or an anomaly noted since the last present (every red line via `log::red`, zero
  stand-in, no-target), writes `fault-NN-pP.png` (full size) and `fault-NN-p(P-1)-before.png` (sampled) and logs
  `fault capture N: <why> at present P: ...`; the next present logs whether the area was gone again ("a one-frame
  flash" / "it stays"). Cap 32 per run, 4 per reason (`NXGFX_FAULT_MAX`). Muted (thrown-away) translations note
  nothing (`log::muted()`). Rows in docs/env_switches.tsv.
- **D, `--ab-lanes`**: the scenario run with `NXGFX_TRANSLATE_THREADS=0`, then the default; `ab.md` in the second
  run compares outcome, off-route, FPS, anomaly counts and route distances. Unit-tested, **not run on a game**
  (lead's instruction to stop game runs).
- **Contact sheets now pure Python** (`scripts/play_sheet.py`): Windows PowerShell 5 hangs while starting on this
  machine since ~07:09 (even `powershell -Command 1+1` needs a kill; cmd and Python start fine); it held the
  System.Drawing sheet step and with it report.md for minutes. Sheets no longer need PowerShell.

### Tests

`py -m unittest scripts.test_play`: 9 tests in 0.05 s. New: event kinds/times/present from a log, steady vs spike
near a shot, filtered vs plain PNG decode, reference + OFF-ROUTE report, ab.md table, every scenario's checks name
shots its steps take. `cargo test -p graphics --lib shot`: 12 pass, incl. `fault::tests` (new white area, its
disappearance, black trip, dark gone).

### Verified (runs read: sheets and captures)

- B: `20261126-054320-v5-oot` (amiibo menu all run) regenerated: OFF-ROUTE on x5/h1/h4/h7/walk1/last (0.42-0.70), FPS
  marked invalid. `20261126-053140-v5-diag-oot-house` (Link left the house early): OFF-ROUTE on all five checks
  (0.58-0.89). Fresh `20261126-064040-hd-oot-house` and `-065807-hd-faults-oot-house`: on route (0.03-0.05).
  `20261126-064044-hd-mkw-turn` and `-065808-hd-faults-mkw-turn`: OFF-ROUTE, correctly: at ~25-30 FPS the kart is
  in the canyon / on the sand straight when the t37-t46 shots are taken, not at the banked turn (sheets read).
- A: `20261126-043138-v4-oot` regenerated: 72 stand-ins, steady 50 no-target skips a period, gate spikes to 464 at
  867 s near `25-walk5`, stand-ins near `24-walk4`. hd-faults-oot-house: stand-in at 681.8 s listed near d24-d26.
- C: hd-faults-mkw-turn: 11 captures (red lines, stand-ins, boot white fades flagged "it stays").
  hd-faults-oot-house: 19 captures, among them **present 12170 entirely black (max 0) between two bright Kokiri
  forest frames, light again at the next present, with 20 zero-stand-in notes at frame 12169** (`faults/fault-14-*`,
  `fault-15-*`), a real one-frame fault caught on its own. The nightmare lightning (`fault-02`) is a false positive:
  the cutscene flashes on purpose. no-target context lines: 24 in that run (pixel shader e55f45d0df50c572, 0 colour
  slots, depth none).

### Unproven / next

- The OoT white herbs/pots flash did not recur in hd-faults-oot-house (it is intermittent); C will capture it when it
  does (`--faults`). The black frame at present 12170 is new and unexplained (stand-ins at 12169): owner of OoT.
- The `(... depth set)` no-target variant did not happen in these runs; its line names the targets' serials/sizes
  when it does.
- mkw-turn's t37-t46 shots depend on FPS (game time): the route fails whenever MKW runs well below 60. Re-time the
  shots or record references at the FPS it runs at once the turn is reached again.
- `--ab-lanes` not run on a game.
- Windows PowerShell 5 start hang on the machine (not the harness's).

### Hooks in others' files

- `src/graphics/src/vk/exec.rs` (no-render-target site, before `red_once("{what}: skipped")`):
  `super::shot::fault::no_target(&what, qs.rt[..qs.rt_n].iter().map(|r| r.tex.as_ref()), qs.rt_depth.tex.as_ref(), qs.prog[1].as_deref().map(|m| m.key), sub.serial);`
- `src/graphics/src/vk/exec.rs` (zero stand-in log, inside its rate-limited block):
  `super::shot::fault::note("stand-in", &format!("texture ID {id} (target {want}) at frame {f}: {what}"));`
- Also edited (mine per brief, shared): `src/graphics/src/log.rs` (`muted()`, `red()` notes the fault watcher),
  `src/graphics/src/vk/shot.rs` (`copy_presented` split out of `record_sink`), AGENTS.md harness paragraph.

### Slips

- **dist.lock:** at 06:49 my `New-Item` failed (another wave held dist.lock) and the same command went on to relink
  MKW anyway (exe 06:49:37). Once the lock was free I relinked MKW properly under my lock (06:50:36) and OoT
  (06:52:50).
- **Perf-run overlap (for the lead to give to the affected agents):** hd-oot-house 06:40:40-06:52 (GPU 1) and
  hd-mkw-turn 06:40:44-06:46 (GPU 0), both started with no machine.lock. hd-faults-oot-house: game 06:58:07-07:09:43
  (GPU 1); its report step then hung on PowerShell until ~07:21 with no game running, which the lead saw as
  overlapping TRANSLATE-COST tc-r2-mkw 07:09-07:20 (the game itself ended 07:09:43). hd-faults-mkw-turn
  06:58:08-~07:03 (GPU 0). An aborted oot run: game 07:22:13-~07:24 (its driver died at ~07:23, the game quit by
  pipe ~07:24). The oot rerun 07:23:28-07:25:55 (GPU 1) overlapped AUDIT-FIX's machine.lock (from 07:23:54); I quit it
  as soon as told and deleted both aborted folders.
- AGENTS.md got a short harness paragraph (shared file).

### Questions

None.
