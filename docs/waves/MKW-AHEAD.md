# MKW-AHEAD — the frame op's later parts translated ahead, off the replay thread

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~00:30, worktree
`D:\Projects\RecompLauncherEngine-mkwahead`, branch `mkwahead` (base `mkwpipe` `ded6464`: USE-SYNC chunk tracking,
SET-FAST step 1, MKW-PIPE park chain). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0.
Deadline 4 h. Other waves' work on the machine during these runs: not known to me beyond OOT-OCC (OoT, GPU 1).

## Starting point (MKW-PIPE)

The race frame is the game's own fence chain: period 18.6 ms = replay of the frame's submit op 15.2 + GPU tail 1.8 +
the next submit after the main thread's release 1.5 (hold 1, medians). 60 at the default pacer needs the op under
~12.5 ms. Inside the op the two NVN queues hand work to each other (A1, B1, A2, B2, A3, B3, A4), each part a Vulkan
submission of its own, translated one after another on the replay thread (each long part's segments over the lanes).

## Plan (written before code)

1. **Split the op.** Chain-trace notes for each part (`xlated`: bytes, segments) beside the existing `prep`, `chain`,
   `park`, `vk_submit`, over one race stretch at hold 1: per part, prep / translation / submit and whether its
   translation used the lanes. That says what translate-ahead can move off the replay thread.
2. **Translate-ahead** (`NXGFX_AHEAD`, 0 = old): plan the later parts through the waits, translate them on the lanes
   while earlier parts are translated, submit each in queue order. Built only if step 1 shows the lanes idle for part
   of the op.
3. **Smaller levers:** OOT-DESC's single image walk (cherry-picked: `f1a363a`, no switch, the same behaviour); the
   use-driven half of USE-SYNC if the first part's sync is still ~2.5 ms.
4. Verify: `mkw`, `mkw-turn` (routes, every sheet), default pacer and `NXOS_FPS_HOLD=off`, same-build switch-off
   controls back to back; `cargo test --release -p graphics`.

What would disprove it: the parts' translation already fills the lanes, or the gate gives up on most ahead segments.

## 1. The split: the lanes idled inside each long part

`20261127-003801-ah-split-hold1` (base + image walk + notes, hold 1, `NXGFX_CHAIN_TRACE=280:0.3`, 12 frame ops). Each
row is one part as the replay thread does it (stream bytes left in the queue when it starts / segments / ms):

| part | A1 | B1 | A2 | B2 | A3 | B3 | A4 |
|---|---|---|---|---|---|---|---|
| bytes left / segments | 1.2 MB / 1 | 12 KB / serial | 1.15 MB / **3** | 5 KB / serial | 0.56 MB / **3** | 3 KB / serial | 74 KB / serial |
| translation, ms (typical) | 1.4 | 0.5-1.5 | **4.5-6.2** | 0.25 | **4.3-7.1** | 0.25 | 0.9-1.0 |

Op median 18.8 ms = prep 2.5 (first part's pool sync 1.75) + translation 14.5 + submits 1.6. A2 and A3 (~700-850
draws each up to their waits) were cut into 3 segments of 256 draws on **6** lanes: half the lanes idle, and the part
lasted as long as its slowest 256-draw segment (~4-5 ms at ~16-20 µs a draw on a lane). The other five parts are
small. So the lanes were idle inside the long parts, not only between them; the cheaper fix comes first.

## 2. Built

### Balanced segments (`NXGFX_TRANSLATE_BALANCE`, default on; `exec.rs` `balanced_seg`)

A stream translated in parallel is cut into about one segment per lane, of equal draws: its draws up to its first event
wait that does not pass now, over the lanes, at least 32, at most `NXGFX_TRANSLATE_SEG` (`balanced_target`). The cuts
are the existing ones (`cut_before`: at a record that ends the pass, or before a draw inside a long pass); only the
size changes. `NXGFX_TRANSLATE_BALANCE=0`: the fixed 256-draw segments as before. A2 and A3 now go in 6-7 segments of
~110-140 draws each.

A fixed 128 first (`20261127-004354-ah-seg128-hold1`, env only): op 18.8 → 16.0, 42.2 → 47.4 FPS at hold 1, but 7
segments on 6 lanes left one lane a second round; balancing to the lane count did better (below).

### Translate-ahead (`NXGFX_AHEAD=1`): built, measured, taken out

Built in `d97ac0a` (history only): the parked queues a chain runs on were planned in queue order first
(`exec::ahead_plan`: the pre-pass with its signals and fence syncs named `k` submissions ahead), then all parts'
segments were translated at once through one gate run over the parts (`gate::Run::with_parts`; a first-use
initialization only by a segment of the frontier's part, since each part's upload buffer runs before its own work
only), each part submitted in order once joined, from its own command pools (pseudo-lanes) with its lanes' children
ended on their lanes.

It stalled MKW: `20261127-011110-ah-ahead2-hold1` **OFF-ROUTE, 22.8 FPS (invalid)**, frames 230-300 ms from ~20 s on.
Segments of later parts gave up at first-use initializations about once a chain (6,717 give-ups in 3,233 chains); each
give-up stopped the parallel run and the rest was translated again on the replay thread (29,354 segments translated
again, ~9 a chain); planning the parts in series cost ~3 ms of the replay thread; one traced op took 60 ms (a part's
translation 20 ms, a submit 14 ms), and the per-frame write-backs rose from ~0.3 MB to 30-77 MB (55-122 ms on the
completion thread) with dispatches per frame climbing from ~110 to ~850 (not diagnosed further). The pictures it drew
were correct (`20261127-010429-ah-ahead-hold1/sheet-1.png`, the first build, also off route). An earlier build
(`010429`) gave up even on calls that change nothing (`pass_pre(false)`): fixed in the second, same result.

Why it was not pursued: after balancing, the long parts fill all lanes by themselves (lanes ~26 ms CPU a frame, the
replay thread waits ~2 ms a frame for them); translating A2 and A3 together would spread the same ~12 segments over the
same 6 lanes, so its ceiling is the small parts and submits it could hide (~1.5-2 ms), not the 4-5 ms the split first
suggested. Taken out in `348b7ff` (files restored to `f1a363a`, balance and notes applied again).

### Not built

- Use-driven half of USE-SYNC: the first part's prep is now ~1.9-2.0 ms (pool sync 1.2-1.4); at most that much, with
  the stage-B risks USE-SYNC lists. Not reached in this deadline.
- Lanes: 8 measured equal to 6 (below); the default stays 6.

## 3. Measured effect

### Hold 1 (`NXOS_FPS_HOLD=off`: the frame not quantized), window [250, 325] s, chain trace 280-280.3 s

| Run (`build/play/`) | build / switches | FPS / 5 % low | frame op (median) | translation | main thread waits on the frame fence | frame period | route |
|---|---|---|---|---|---|---|---|
| `20261127-003801-ah-split-hold1` | base + notes (fixed segments) | 42.2 / 32.0 | 18.8 | 14.5 | 7.5 | 23.6 | 3/3 on |
| `20261127-004354-ah-seg128-hold1` | `NXGFX_TRANSLATE_SEG=128`, `_MIDCUT=128` | 47.4 / 32.5 | 16.0 | 11.9 | 5.2 | 21.5 | 3/3 on |
| `20261127-005408-ah-bal-hold1` | balanced | **56.8 / 49.0** | **12.6** | 9.3 | **0.0** | **16.65** | 3/3 on |
| `20261127-013058-ah-fin-off-hold1` | final `348b7ff`, `NXGFX_TRANSLATE_BALANCE=0` | 47.6 / 39.3 | 15.1 | 11.5 | 5.5 | 18.5 | 3/3 on |
| `20261127-013637-ah-fin-on-hold1` (right after) | final, balanced | **53.6 / 47.0** | **13.0** | 9.7 | 0.7 | 16.6 | 3/3 on |
| `20261127-020522-ah-l6-hold1b` (repeat) | final, balanced | 53.0 / 43.0 | 11.5 | 8.8 | 1.0 | 16.5 | 3/3 on |
| `20261127-014232-ah-l8-hold1` | final, 8 lanes | 55.5 / 47.0 | 10.6 | 7.7 | 0.0 | 16.66 | 3/3 on |
| `20261127-015943-ah-l8-hold1b` (repeat) | final, 8 lanes | 53.3 / 45.0 | 12.6 | 9.1 | 4.6 | 16.3 | 3/3 on |
| `20261127-014810-ah-l10-hold1` | final, 10 lanes | 52.2 / 44.0 | 13.5 | 9.7 | | | 3/3 on |

Per part, balanced (`fin-on-hold1`): A1 1.0-2.0 (2 segments), B1 0.35-0.4, **A2 2.2-3.5**, B2 0.15-0.2, **A3 2.7-4.4**,
B3 0.15-0.2, A4 0.6-0.75 ms; against the same build switched off (`fin-off-hold1`) A2 3.6-4.9 and A3 3.7-4.7. GPU time
a frame unchanged (4.5-4.7 ms; the extra cuts inside passes cost nothing measurable).

**The game's main thread no longer waits for the frame op in the traced stretches**: it waited 5.5-7.5 ms a frame on
the frame fence (`nvnSyncWait`, main+0x69d034) with fixed segments and 0-1 ms balanced; the traced frames then run at
the vsync (16.5-16.7 ms period), the main thread's own update plus the frame step being the frame. The 5 % low
(43-49) is the stretches where the op still exceeds what the main thread hides (ops of 13-15 ms in the same traces).

### Default pacer (the profile's `fps_hold`)

| Run (`build/play/`) | switches | FPS / 5 % low | frames held 1 / 2 vsyncs (last summary) | route |
|---|---|---|---|---|
| `20261127-011940-ah-fin-on` | final, balanced | 32.4 / 29.0 | 294 / 1,506 | 3/3 on |
| `20261127-012519-ah-fin-off` (right after) | `NXGFX_TRANSLATE_BALANCE=0` | 31.9 / 28.0 | 261 / 1,539 | 3/3 on |
| `20261127-015405-ah-l8-def` | 8 lanes | 41.5 / 29.5 | 1,153 / 647 | 3/3 on |
| `20261127-021607-ah-fin-off2` | `NXGFX_TRANSLATE_BALANCE=0` | 33.9 / 29.9 | 601 / 1,199 | 3/3 on |
| `20261127-022145-ah-fin-on2` (right after) | final, balanced | **40.0 / 30.0** | 880 / 920 | 3/3 on |
| `20261127-021110-ah-fin-turn` (`mkw-turn`, [250, 290]) | final, balanced | 41.4 / 30.0 | | 4/4 on |

At the default pacer MKW holds 1 vsync for more of the race than before (on: 294-880 of 1,800 frames; off: 261-601),
so the mean moves between 32 and 40 FPS from run to run, but the 5 % low stays at the 30 line: its 1-vsync trials need
30 frames costing ≤ 0.95 vsync (15.8 ms) and fail within a second (22-29 failed trials a minute); frames near one
vsync are counted late often enough to step back. The first on / off pair differed by 0.5 FPS, the second by 6.1:
within the spread of this machine, the default-pacer gain is not proven; the hold-1 gain (op −2 ms, +6 FPS, main
thread's fence wait 5.5 → 0.7 ms) is.

Pictures (every sheet read): boot, title, menus, grid, race and lap 2 drawn correctly in every run of both switch
states, lanes 6 / 8 / 10, both pacers; no pop-in, stale textures, flicker or missing geometry. `mkw-turn` (`021110-ah-fin-turn`, `sheet-1.png`, `sheet-2.png`):
t37-t46 on the banked desert slope, lit and textured, coins, items and HUD present. The `02-title` shot (138.6 s) was
black in `fin-off2` (switch off) and a dim fade-in in `fin-on2`: the opening-to-title cut MKW-PIPE described, in both
switch states.
Anomalies: the known set only (zero / released-image stand-ins at loads, the timestamp-query and crashed-pipeline red
lines, skipped draws of the crashed pipeline, gate counts at loads). In most runs of both states the `02-title` shot
shows the race grid already: the scenario's shot timing, not this change.

## 4. What is left to 60

- **The frame is now mostly the game's own CPU plus the pacer.** In the traced stretches the main thread does not wait
  for the GPU fence any more; the period is its update (~12-16 ms) and the handoff. Holding 60 needs the op under the
  main thread's update **every** frame (the 5 % low comes from ops of 13-15 ms) and the pacer's 1-vsync trials to pass
  (≤ 15.8 ms cost over 30 frames).
- Inside the op (balanced, ~11.5-13 ms): A2 + A3 translation ~6 ms on the lanes (~20 µs a draw per lane against ~7.5
  serial: SET-FAST's per-set cost and lock contention are what is left there), the first part's prep ~2 ms (pool sync
  ~1.3: USE-SYNC stage B), A1 ~1-2 ms, the small parts and seven submits ~2 ms.
- Translate-ahead as built does not work (above); a working one would hide ~1.5-2 ms at most now.
- The `fps_hold` decision (steady 30 vs uneven ~53-57 at hold 1) is the owner's; at hold 1 the race now runs 53-57
  FPS mean with a 43-49 5 % low.

## Report (agent, protocol §5)

Branch `mkwahead` (base `mkwpipe` `ded6464`): `f1a363a` (OOT-DESC's single image walk, cherry-picked), `d97ac0a` (plan,
per-part notes, balanced segments, translate-ahead off), `348b7ff` (translate-ahead taken out), this commit (doc).

### 1. Built
- **Diagnosis:** per-part chain notes (`xlated`) split the 18.8 ms op: translation 14.5 ms, of it A2 and A3 4.5-7 ms
  each on 3 segments while 6 lanes were available.
- **Correction:** `NXGFX_TRANSLATE_BALANCE` (default on): about one segment per lane, of equal draws up to the stream's
  park. Op 15.1 → 13.0 ms and 47.6 → 53.6 FPS at hold 1 (same build, back to back); the main thread's wait on the
  frame fence 5.5 → 0.7 ms.
- OOT-DESC's single image walk (cherry-pick, no behaviour change).
- Translate-ahead: built, measured (stalls, off route), taken out (§2). Use-driven sync: not built.

### 2. Tests (`cargo test --release -p graphics`: lib 275 passed, 6 ignored; other targets pass)
- `exec::balance_tests::segments_are_one_per_lane_within_bounds`: draws over lanes, clamped to [32, the fixed size];
  no lanes → the fixed size. New function, no failing-before state.
- (Translate-ahead's `gate::tests::first_use_inits_belong_to_the_frontier_part` failed before the `pass_pre(false)`
  fix and passed after; it went out with that code.)

### 3. Verified
Runs, numbers and sheets in §3. Route checks on in every run except the translate-ahead ones. Target 60 not reached at
the default pacer (final build: 32.4 / 29.0 and 40.0 / 30.0; switch off 31.9 / 28.0 and 33.9 / 29.9; `mkw-turn` 41.4 /
30.0); at hold 1 53-57 FPS mean, 43-49 5 % low (off: 47.6 / 39.3).

### 4. Unproven
- Lanes 8 vs 6: one 8-lane default-pacer run gave 41.5 FPS against 32.4 at 6, but the hold-1 repeats put 8 and 6 level
  (53.3 vs 53.0); the lead's quiet A/B should decide before any default change.
- Balanced segments for other titles (it applies to every stream cut into segments): OoT, DKB not run (not owned).
- Translate-ahead's write-back / dispatch growth was not diagnosed (code taken out).
- Validation layers not run.

### 5. Slips
- Relinks ran this worktree's `target\release\nxrs-build.exe` directly with the launcher's argument list and
  `NXRS_LINK_LIBS=-Wl,/DEBUG` (as USE-SYNC, SET-FAST and MKW-PIPE did). The MKW DLL in the library is this branch's
  `348b7ff` build.
- Translate-ahead was taken out with `git checkout f1a363a -- exec.rs gate.rs queue.rs perf.rs` (a git restore, not a
  script edit), the kept edits then made again with the editor.
- One TaskStop on a shell waiter of mine (a broken `until` loop), not on a harness run.

### 6. Questions
None.
