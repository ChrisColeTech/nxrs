# FRESH-SAVE — Arceus, Arceus base and Jamboree routes to first gameplay from a fresh save

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Problem (recorded).** Every title runs from an empty save (none has one; fresh saves ask for a name, answered by
`NXOS_SWKBD=auto`). `arceus`, `arceus-base` and `jamboree` in `scripts/scenarios.toml` render correctly and exit
normally but stop inside their fresh-save openings: Arceus in its opening dialogue/cutscenes
(`build/play/20261125-025127-arceus-base`: autosave notice, falling scene, Arceus light at 124 s), Jamboree in the
first-run rules tutorial. Doc 34's Jamboree route (owner-era `library_play.py`: wait 260 s, hold L+R 3 s at the title,
advance first-run dialogue) reached the plaza on 2026-11-22.

**Goal / done.** Each of the three scenarios reaches **first controllable gameplay** (the player character moving under
the scenario's input) and its measured window covers that gameplay. Done = for each title a completed run whose sheet
you looked at shows the character in a different place across the window's shots under input, plus window FPS vs
target, 5% low, red lines. The validation bar is first gameplay, not story progress.

**How to work.** This is route work: read the sheets and the logs to see where each run stops, extend `steps` (and the
replay files they use, next to the existing `arceus_gameplay.txt` / `hold_lr.txt` / `run_jump.txt`), raise `seconds`
/ move `window` as needed. Use `shot` steps generously while finding the route, then trim to a few. Iterating runs is
the job here; keep each run purposeful (change the route between runs, never re-run unchanged).

**Engine defect found on the way** (hang, device loss, a dialogue that never advances under input, the known 15 s
Arceus freeze — `docs/32` "no preemption inside long guest loops"): record it with evidence and work around it in the
route only if the game itself allows (e.g. wait it out); do not change engine code in this wave.

**Machine (rules changed 2026-11-25).** Games and builds may run concurrently. Run on GPU 1: `py scripts/play.py
<scenario> --env NXGFX_GPU=1`. Two titles at once is fine. No engine build is needed (the titles are built); if one
says it must be rebuilt, rebuild only that title. Another agent (CLEANUP-S2R) is renaming `SWR_*` variables and
editing `scripts/` and engine files: re-read `scripts/scenarios.toml` right before editing it and change only your
scenarios' entries; if `play.py` breaks mid-rename, wait a few minutes and retry rather than fixing it yourself.

**Exclusions:** no engine changes; no real saves (runs use a save copy; never `--real-save`); no other titles; don't
edit `docs/32-REMAINING-WORK-PLAN.md`; **no commits**.

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID, never by
image name), protocol §3. Foreground commands under 10 minutes; never end a turn waiting on a background notification.
`py scripts/play.py --list`, `py scripts/play.py <scenario>`, `py scripts/play.py report build/play/<run>`;
`py -m unittest scripts.test_play` if you touch the harness.

**Deadline:** 2 hours wall clock. Park a title after ~90 min with where it stops and why; no early hand-off.

## Report (agent, protocol §5)

**Built (route only, no engine change).** All three scenarios now reach first controllable gameplay from a fresh
save and measure the window over the player walking.
- `scripts/scenarios.toml` `[arceus]` and `[arceus-base]` (same route): A every 3 s (`press A ; 3 * 90` and
  `* 40`) takes every default (English, first look, Yes, first answers; the name comes from `NXOS_SWKBD=auto`)
  through Arceus' questions, the fall, the title card and its load, and Laventon's beach talk to "Wait, you
  three!", where the player gets Move / Dash (~370-400 s; each press step really takes ~3.16 s). A does nothing
  in free roam. Then three walk replays run before the window, because the first walk up the path loads the next
  area and that load shows one to three ~5 s present stalls (see Unproven). Then a measured walk with shots
  inside it (up, right, down, left) and an end shot. 600 s limit, window [485, 504].
- `[jamboree]`: `wait 305` (the title "Press L+R" is up by then), `press L+R~1`, A every 3 s × 30 (1 player,
  Mario, Ready!, Toad's first-run talk to "hop on the Hot-Air Balloon"), then Mario walks on the plaza floor;
  shots inside the walk. 520 s limit, window [405, 445].
- New replay files: `scripts/inputs/arceus_beach_walk.txt` (a closed ~11 s square: up the path, right, down, left;
  closed so repeats stay short of the beeping-object event further up the path, which starts a dialogue) and
  `scripts/inputs/jamboree_walk.txt` (down, right, left, down-left, up ~16 s, kept off the balloon stairs, since
  walking up them boards the balloon).
- How the route was found: explore runs (`build/play/20261125-043913-xjam`, `-043927-xarc`) using play.py's own
  `start()` with a long wait, driving the control pipe by hand from scratch scripts (not in the repo).

**Tests.** No harness code changed, so `scripts.test_play` was not needed. `py scripts/play.py --list` parses the
edited file.

**Verified** (every sheet below looked at; all on `--env NXGFX_GPU=1`, two titles at once, 0 red lines in all):
- **arceus** `build/play/20261125-053050-arceus-final2`: completed 149/149, normal exit. Window [485, 504]:
  **30.0 FPS mean, 5% low 30.0, min 30.0 (target 30)**. Sheet: language, beach talk at 314 s, free roam with the
  Move/Dash prompt at 441 s, then the player in a different place on every walk shot (path, grass right, by the
  rock, sand left, end).
- **arceus-base** `build/play/20261125-053830-arceus-base-final2` (with `--exe`, see Unproven): completed 149/149,
  normal exit. Window: **30.0 FPS mean, 5% low 29.5, min 29.5 (target 30)**. The sheet shows the same route with
  the player moving under input.
- **jamboree** `build/play/20261125-045848-jamboree-fresh2` (before the lead's relink): completed 48/48, window
  59.7 FPS, 5% low 58.0, min 57.0 (target 60). **After the relink** (`dist\nxrs\nxrs.exe --select-title "Super Mario
  Party Jamboree" --build`, exit 0 in 31 s) `build/play/20261125-053049-jamboree-relinked`: completed 48/48, normal
  exit, window **59.6 FPS, 5% low 58.0, min 58.0 (target 60)**. Sheet: title, player count, plaza at 406 s,
  then Mario in a different place on each shot (bottom, right, left by Toad, up, right, left).
- **Lead's relink questions (jamboree-relinked against jamboree-fresh2):**
  - Boot slow frame: the 745 ms frame (frame 178 at 5.57 s, 700.8 ms of replay) is smaller but not gone: now
    432 ms at 2.90 s (frame 2) and 598 ms at 7.55 s (frame 174). Slow-frame lines in all rose from 798 to 1665.
  - **The opening got slower:** mean FPS in 30 s buckets over 0-210 s went from 36/42/46/42/41/42/47 to
    13/24/28/28/27/27/30. From 210 s on (title, menus, plaza) it is unchanged (~60). Other spikes: 1353 ms at
    204 s, 658 ms at 308 s. Both runs had a second title running on the same GPU (not a perf-measurement run).
  - Red lines: none before, none after.
  - Textures: no missing or black textures in the title, the player-count menu or the plaza on the sheet (the
    opening itself has no shots).
  - Memory: run peak 12.4 GB → 7.2 GB.

**Unproven / found on the way.**
- **Engine defect (known, docs/32 "no preemption inside long guest loops"), seen in every Arceus run:** loader
  thread 0x1004 runs guest code with no checkpoint, and better threads take its core only after `STEAL_GUEST`
  (`scheduler: thread 0x1005 took core 2 from thread 0x1004, which ran guest code with no checkpoint for 5000 ms`).
  In the opening this gives 5-25 s with no present (worst 25 s at 222 s, `-052152-arceus-base-final`). On the
  beach, the first walk up the path gives one to three 5 s stalls within ~22 s (e.g. metrics 444-447, 453-456,
  459-462 s in `build/play/20261125-052153-arceus-final`, where it dragged that window to 22.1 FPS). There were
  more stalls when both Arceus builds ran together than when the base ran alone (`-051356-arceus-base-alone`: one).
  The route waits it out (three walks before the window); the fix is the engine row in docs/32.
- `arceus-base`'s title "Pokemon Legends Arceus base" is not in the launcher library (`library.json` has one Arceus
  entry, the 1G+1U one), so `py scripts/play.py arceus-base` stops with "matches 0 entries". The runs used
  `--exe "P:\Games\Nintendo\DUMPS\Pokemon Legends Arceus base [01001F5010DFA000][v0]\SwitchRecomp Builds\Pokemon
  Legends Arceus base [nxrs]\Pokemon Legends Arceus base.exe"`. A note in the entry says so. Fixing it needs a
  library entry or an `exe` scenario key; I left that to the lead.
- `scripts/inputs/hold_lr.txt` (the old Jamboree route's file, now unused) writes `192`, which replay parses as hex
  0x192 (B+LS+R+ZL), not L+R (`c0`). Left as it is.
- Timing margin: Arceus control came at ~370 s (1G+1U) and ~395 s (base, alone); the beach shot is at ~441 s.
  An opening stall much longer than 25 s would cut into that margin.

**Slips.** Some runs overlapped another agent's Mario Kart World run on GPU 1. None of mine was a
perf-measurement run. My scratch explore driver wrote nothing into the repo. Exploration screenshots were deleted
afterwards, and so were the superseded runs `-045023-jamboree-fresh`, `-050617-arceus-fresh2` and
`-050150-arceus-base-fresh`.

**Questions.** None.
