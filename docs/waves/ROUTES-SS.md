# ROUTES-SS — Shadow and Sonic routes that keep the player moving through the window

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
parallel rules: `docs/waves/PARALLEL-RULES.md` (binding). **GPU 1.** Title: "SONIC X SHADOW GENERATIONS"
(scenarios `shadow` and `sonic`).

## Brief (lead)

**Problem (recorded).** In the final SONIC-SMASH runs (`build/play/20261125-*-sxsg-final`, `-*-sonic-final`, sheets
read by the lead) Shadow and Sonic stand still in the measured window: the route's input ends before the window,
so the window proves FPS but not control. Shadow 29.99 of 30, Sonic 31.5 of 30 (Green Hill).

**Goal / done.** Both scenarios keep the character moving under input across the whole measured window (run, jump,
spin dash, ...), with `shot` steps inside the window; the sheet shows the character in a different place on each
window shot. Report window FPS vs target, 5% low, red lines for each. Route work only: `scripts/scenarios.toml`
(your two entries; re-read right before editing) and new replay files in `scripts/inputs/`.

**Engine defect found on the way:** record it with evidence, do not change engine code.

**Exclusions:** engine code, other scenarios, no commits, don't edit docs/32.

**Runs:** `py scripts/play.py shadow --env NXGFX_GPU=1`, `... sonic ...`; both may run at once. No build needed.

**Deadline:** 2 hours wall clock. Report per protocol §5 in this file under "## Report".

## Report (agent, protocol §5)

**Built (route only, no engine change).**
- **Why both stood still (diagnosis):** in Sonic X Shadow Generations a stage's jump is **B**, not A (A does nothing in
  a stage). `run_jump.txt` and `spindash.txt` jump/spin dash with A, so Shadow stood at the first "B" jump prompt
  (an A-only obstacle stop) and Sonic stood at Green Hill's first rock wall (needs a spin dash, down + B). The input
  was arriving (the logs show every replay starting); the routes pressed the wrong button. Shadow also had stick
  right in a 3D stage (forward is stick up). Besides, both old runs quit at 249-303 s while the window was
  [250, 390], so it covered only a few seconds anyway.
- **Shadow's menu route was flaky:** at 40 s the game is sometimes on "press any button" and sometimes already on
  the Sonic / Shadow selection (no input given; seen with and without a pad), and on the selection Left and Right
  both **toggle**, so "Right, Right" picked Sonic about half the time (`build/play/20261125-090222-xshadow3`
  started Sonic Generations). **R** passes the title and does nothing on the selection: `press R` twice, then
  exactly one `press Right`, is Shadow either way (`-091344-xmenu3`, and both later runs).
- New replay files (`scripts/inputs/`): `shadow_boost.txt` (12 s: forward + boost ZR held, B then Y — the homing
  attack the "B -> Y" marks over the GUN mechs ask for — every 1.5 s), `shadow_side.txt` (2.5 s: right off the
  first side-on ledge, where forward does nothing, then B, Y), `shadow_rail.txt` (12 s: forward + boost, no jump;
  a jump on the stage's opening rail leaves it into the void), `classic_run_jump.txt` (12 s: a spin dash, then
  right held with B held 0.45 s every 1.2 s). Each is replayed back to back with `shot` steps between them.
- `[shadow]`: R, R, Right, then the old A / Down / A / A and one `skip_hold`; the stage starts ~126 s on its rail;
  `shadow_boost` x2 (B corridor, mechs), 4 s more to the ledge, `shadow_side`, `shadow_rail` to the road's edge.
  The way on from that road (apparently a grind rail to the right) is not found: Shadow falls off soon after
  landing or stands at the edge, where the next `shadow_boost` B jumps him off; either way the stage restarts on
  its rail and the route runs it again. 240 s limit, window [130, 202].
- `[sonic]`: unchanged menus; `classic_run_jump.txt` x10 from 157 s, shots between; 300 s limit, window [185, 275].

**Tests.** No harness code changed. `py scripts/play.py --list` parses the edited file.

**Verified** (sheets looked at; `--env NXGFX_GPU=1`; none was a perf-measurement run):
- **shadow** `build/play/20261125-095656-shadow-final`: completed 29/29, normal exit. Window [130, 202]:
  **30.0 FPS mean, 5% low 29.4, min 29.0 (target 30)**. Red lines: 2x `texture ID 356: target 1 bound where the
  shader samples target 2` (139 s, the stage load) and the known `pipeline-crashed.bin` skip notice. Sheet: rail
  (0:03), B jump ring (0:23), side ledge approach, on the ledge, the road after it, the fall and restarted rail
  (0:03), the road at 0:17 boosting, the ledge area at 0:29: Shadow in a different place on every window shot
  (two pairs are the same stretch on the two passes of the loop, at different moments).
  Earlier: `-094632-shadow-route3` (window [140, 240]: 30.0 / 29.5 / 29.4) showed the stall the final route
  removes (two shots at the ledge in the second pass).
- **sonic** `build/play/20261125-100648-sonic-final`: completed 31/31, normal exit. Window [185, 275]:
  **31.5 FPS mean, 5% low 31.0, min 30.8 (target 30)**. Red lines: only the known `pipeline-crashed.bin` skip
  notice. Sheet: Sonic in a different place on all eight window shots (rope bridge, totem slope, waterfall slope,
  rolling through the quarter pipe, sunflowers, the tunnel wall the old route stopped at — passed by the spin
  dash —, the loop, past it at 1:23 with 55 rings). He lost a life once (timer 0:55 → 0:10 between the first two
  shots) and went on from the restart. `-085055-sonic-route1` (ran together with shadow-route1) gave the same
  31.5 / 31.0.

**Unproven / found on the way.**
- **Engine defect (pipeline crash list, shared folders):** both scenarios run the same exe from the same build
  folder, so two runs at once share `SONIC X SHADOW GENERATIONS_data\pipeline-*.bin`. When `shadow-route1` and
  `sonic-route1` started together (08:50:55), the second process read the first one's in-flight compile list as
  "the last session ended while compiling": `[RED] 2 pipeline(s) were compiling when the last session ended (a
  compiler crash): not compiled again` (`build/play/20261125-085055-sonic-route1/run.log` line 26; the code is
  `load_crashed` in `src/graphics/src/vk/pipe.rs`), and `pipeline-crashed.bin` was rewritten at 08:50:55.99 (553
  bytes now). Those 2 pipelines' draws are skipped from now on although no compiler crashed; the same pair of runs
  also gave `pipeline cache ... could not be written`. I did not touch the file (blacklists stay); the lead should
  decide whether to drop those 2 entries. The brief's "both may run at once" is unsafe for one title until the
  in-flight file is per process; every later run here was alone.
- **Possible text defect:** the dialog that sometimes follows the title and the Shadow Generations main menu show
  their A / B glyphs with no labels and no menu text (`build/play/20261125-090643-xmenu/x-n1.png`,
  `-091344-xmenu3/x-s3.png`); in-stage subtitles render. Not chased (out of scope).
- Shadow's route needs the way past the road's edge to stay in one pass; the next step is an explore from the road
  (`shot road`, ~170 s) trying a jump to the right onto the grind rail.
- A DualSense is plugged into the machine and every run reads it in the background (`NXRS_CONTROLLER_BACKGROUND=1`);
  a touch would reach scripted runs.

**Slips.**
- Two of my runs started while `build/locks/machine.lock` (MKW-PERF5) existed: `-085653-xshadow2` (~30 s, quit at
  boot) and `-092648-xroad` (09:26:48-09:29:42, lock taken 09:25:58; quit when I saw it). The perf numbers MKW-PERF5
  took in those windows may include my load. MKW-PERF5 also took the lock at 09:57:12 while `shadow-final` was
  running (to 10:00:21) and at 10:08:30 while `sonic-final` was running (to ~10:11:30).
- `run_jump.txt` and `spindash.txt` are now used by no scenario; left in place.
- My first two scenario runs (`-085055-*-route1`) ran together (the brief allowed it) and caused the false crash
  entries above.
- Explore runs drove the control pipe by hand from scratch scripts (not in the repo). Kept: `-084105-xsonic`,
  `-090222-xshadow3`, `-090643-xmenu`, `-091344-xmenu3` (cited above); deleted the superseded `-084105-xshadow`,
  `-085653-xshadow2`, `-090913-xmenu2`, `-092648-xroad`.

**Questions.** None.
