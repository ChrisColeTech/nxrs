# VIOLET — Pokemon Violet to first controllable gameplay on the current engine

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
parallel rules: `docs/waves/PARALLEL-RULES.md` (binding). **GPU 1.** Title: "Pokemon Violet" only (scenario `violet`).

## Brief (lead)

**Problem (recorded).** Violet reached a controllable house at 30 FPS on 2026-11-21 on an older engine, with 21 red
lines and a loading-memory slope (`docs/33` row 10, `docs/INTERFACES.md` Violet notes). The ledger lists it blocked
on "first-run progression, display reset". It has not run on today's engine. Every title starts from an empty save;
fresh saves ask for a name (`NXOS_SWKBD=auto` answers it).

**Goal / done.** `py scripts/play.py violet --env NXGFX_GPU=1` reaches first controllable gameplay from a fresh save
(the player character moving under input in the measured window), 30 FPS target, readable picture, no device loss,
red lines fixed or each explained. Done = that run, sheet read, with FPS vs 30, 5% low, red lines.

**How.** Run it first and read where it stops. Route problems (opening dialogue, character creation, menus): extend
the `violet` entry's `steps` and replay files (re-read `scripts/scenarios.toml` right before editing; your entry
only). Engine problems (hang, display reset, red lines, missing textures): fix the cause in the engine as a general
correction, rebuild under the dist lock, relink Violet (`dist\nxrs\nxrs.exe --select-title "Pokemon Violet"
--build`), re-run. If Violet needs a full rebuild, rebuild only it.

**Exclusions:** other titles, no commits, don't edit docs/32. Descriptor, memory or coherence changes: say so in the
report; the lead will audit them.

**Deadline:** 3 hours wall clock. Park with where it stops and why. Report per protocol §5 in this file under
"## Report".

## Resume (lead, 2026-11-25, after SMASH-POLISH)

The wave now runs **alone** (an independent read-only audit of SMASH-POLISH's `os/src/guest.rs` / `os/src/mem.rs`
change may still be running; it edits nothing — if the lead later fixes something there, it will tell you). Start
from the parked report below: relink Violet in full under the dist lock (clears `buildIncomplete`), then build the
~10-minute fresh-save route (`seconds` up to ~700; the measured window after the opening). Deadline 3 hours from now.

## Report (agent, protocol §5; resumed 2026-11-25 11:45, done ~12:20)

(Parked 08:50 after an interrupted relink, see the brief; resumed alone.)

1. **Built.**
   - **Route (the reason it stopped):** the old `violet` entry assumed a save already in the house; from a fresh save
     it never reaches gameplay. Explore run `build/play/20261125-114618-xviolet1` (A every 3 s, shots every 30 s,
     the control pipe driven by hand from a scratch script) found two route stops, both game behaviour, not engine:
     (a) **"Choose a look" has no default**: the cursor sits between the faces and A does nothing until the stick
     picks one (the run sat there from ~60 s until a hand `press LUp`); (b) after "Get Up" the game saves and opens
     the **Adventure Guide; A re-opens it, B closes it**, so it stayed open while A presses continued and closed at
     once when they stopped. Everything else (language, name via `NXOS_SWKBD=auto`, enrolment, the director's
     speech, the academy, the flyover) advances with A. New `[violet]` entry in `scripts/scenarios.toml`: A x12, the
     look (`press LUp~0.5`), A x105 (to ~402 s; "Get Up" comes ~270-330 s after the look), `press LLeft~1.5` (get up),
     B twice, then a warm-up walk and two measured walks with shots inside them; 480 s limit, window [425, 452],
     fps 30. New replay `scripts/inputs/violet_bedroom_walk.txt` (a closed ~7 s square on the bedroom floor:
     left, down, right, up). `violet_gameplay.txt` (the old house route) is now used by no scenario; left in place.
   - **Engine (the run's one red line):** `[RED] a draw or clear with no render target bound` at ~3.8 s. Diagnosis
     (message extended with the slot counts, one run): the game sets **8 colour slots, all null, no depth**, and
     draws once at frame 0; no colour pointer was an unrecognized texture. A GPU draws nothing into no targets, so
     the skip is exact unless the program writes storage or captures transform feedback. Correction in
     `src/graphics/src/vk/exec.rs` `begin_pass`: such a draw (no target set at all, no written storage buffer, no
     image, no xfb) now logs one `[vk]` line ("... its program writes no storage, so it draws nothing, as on the
     GPU"); any other case (a target set that failed to attach, or a program with side effects) stays red, now
     with the slot counts and "skipped". In the `SET_RT` handler, a non-null colour pointer that is not an
     initialized texture gets its own red line (it was silently dropped). Not a descriptor, memory or coherence
     change; behaviour of the replay is unchanged (the same draws are skipped), only the classification.
2. **Tests.** No unit test for the classification (it lives in `begin_pass`, which needs a `Gpu`; the shader
   `Module` has no test constructor); its proof is the game run below (the red line before, the `[vk]` line after).
   `cargo test --release -p graphics`: 202 passed, 0 failed (plus the small binaries), run after the change.
3. **Verified** (all `--env NXGFX_GPU=1`, the wave ran alone, no machine lock needed; not a perf-measurement run,
   but nothing else ran):
   - Relink in full on today's dist (`dist\nxrs\nxrs.exe --select-title "Pokemon Violet" --build`, under the dist
     lock): ok, 35937 shaders reused, `buildIncomplete` now false. Engine rebuilt twice with
     `build.ps1 -KeepCache -SkipGnu` (~27 s each) and Violet relinked after each, under the lock; lock released.
   - **Final: `build/play/20261125-120752-violet-final`** (fixed engine): completed 146/146, normal exit, 449 s.
     Window [425, 452]: **29.9 FPS mean, 5% low 29.5 (target 30)**; min 26.5 is the quit sample at 448.2 s (the
     game samples before it are 29.5-30.5). **Red lines: 0.** Memory flat in the window (7037-7039 MB; 7453 only in
     the quit sample; run peak 7580). Worst pacing 736 ms at ~290 s (the flyover load, outside the window).
     **Sheet read:** language, look screen, classroom, a black transition at 292 s, the desk with "Get Up" at
     403 s, the bedroom with the "Walk" prompt at 412/419 s, then the player in a different place on every walk
     shot (by the stair railing at 429, the stairwell with the plush at 432, by the bed at 434, mid-room at 437,
     the railing again at 439, ...). Picture readable, no missing textures seen.
   - `build/play/20261125-115728-violet-route1` (same route, before the engine change): 29.9 / 29.5 / min 27.1, the
     one red line; same sheet.
   - `build/play/20261125-114618-xviolet1`: the explore run (stops at the look screen and the guide; hand shots
     `x-*.png`).
4. **Unproven.**
   - The route has ~60 s of margin between the latest "Get Up" seen and the get-up step (A presses end ~402 s); an
     opening much slower than today's would need more A presses before `press LLeft`.
   - The walk stays in the bedroom; the old blocker "no way out of the house / Mom missing" (`docs/INTERFACES.md`
     lane X2) was not revisited (not needed for first gameplay).
   - The history's "21 red lines" and the loading-memory slope did not reproduce on this route (0 red, flat memory).
5. **Slips.** Several overlapping `Monitor` waits were started while waiting on the first run (harmless, all
   ended). Explore runs drove the control pipe from a scratch script outside the repo. Deleted: the scratch build /
   relink logs under `build/` and the superseded 15 s diagnostic run `-xviolet-red`.
6. **Questions.** None.
