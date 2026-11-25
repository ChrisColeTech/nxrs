# MK-PAIR — Mario Kart World v1.8.0 and Mario Kart 8 Deluxe on the current engine

Owner: one delegated agent. Lead: main session. Opened 2026-11-24. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Goal / done (each ends with a game result, sheets looked at):**
1. **MKW v1.8.0** (`[mkw180]`): it was blocked only by the base game's device-loss latch, now fixed
   (`docs/waves/MKW-LOSS.md`, `MKW-RACE.md`). Rebuild it (`dist\nxrs\nxrs.exe --select-title "Mario Kart World v1.8.0"
   --build`), bring `scripts/scenarios.toml [mkw180]` to the base game's working route (`[mkw]`: fresh save,
   unskippable opening, A through Grand Prix, drive replays; adjust if 1.8.0's menus differ) and show a driven race.
   Fix whatever stops it (device loss, red lines, a stall) at its cause.
2. **MK8D** (`[mk8d]`): rebuild ("Mario Kart 8 Deluxe"), run, reach a driven race on a fresh save (adjust the
   scenario), then work its recorded residuals (`docs/34-LIBRARY-VALIDATION.md` MK8D sections: resource diagnostics,
   stutters, memory scope): correct the largest one you can prove, with before/after numbers from the same scenario
   (window FPS, 5% low, worst pacing, red lines).

**Baseline:** branch `gameplay-harness`, HEAD `5bb3572` (+ this file), clean tree. `dist/nxrs` = HEAD engine
(CUBE-TARGET); neither title is rebuilt with it. Ledger: `docs/32-REMAINING-WORK-PLAN.md` current section.

**Exclusions:** no skipping draws, blacklisting or weakening robustness; native NDVE stays off; only these two titles
(MKW base runs only to check a shared change didn't break it); no commits (lead commits at acceptance).

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol §3
(one build or game at a time, never a build or cargo test while a game runs; `--jobs 4`). Engine:
`pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (background, ~4 min); rebuild each title
after an engine build. Run: `py scripts/play.py <scenario> [--env K=V] [--seconds N] [--diag render]`; read
`report.md`, look at every `sheet-*.png`. Tests: `cargo test --jobs 4 -p graphics --lib` (+ `-p shaders`/`-p os` if
touched); `py -m unittest scripts.test_play` if the harness is touched. A device loss stops that path: fix it,
never re-run it unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min and continue with the next item; no early hand-off.

**Files expected:** `scripts/scenarios.toml`; engine files wherever the causes live; this file for the report.

## Report (agent, protocol §5)

**Verified working now (final tree, engine + all three titles rebuilt 19:29-19:31):** MKW v1.8.0 reaches and drives a
Grand Prix race on Mario Bros. Circuit with the base game's route (normal exit, 43/43 steps). MK8D reaches and drives a
Grand Prix race on a fresh save through lap 2/3 at 59.9 FPS. MK8D's red lines are **0, were 49**, and its zero-stand-in
texture binds are **0, were 4314** (same scenario). MKW base still races. No device loss in any run.

### Built
1. **MKW v1.8.0 route** (`scripts/scenarios.toml` `[mkw180]`): the old steps (L+R at 44 s, 8 A) were for a skippable
   opening. This entry builds the same base NUP as `[mkw]` (same `gamefix: Mario Kart World 1.8.0` patch set, title
   reads "Ver. 1.0.0"), so it now uses `[mkw]`'s steps, 360 s, window [250, 325]. Nothing stopped it on the current
   engine: no engine change was needed for item 1.
2. **MK8D route** (`[mk8d]`): the old route reached the race, but it ended at 117 s, so the [100, 140] window held 17 s.
   Now two 40 s drive replays, 200 s, window [95, 175] (race grid at ~76 s).
3. **MK8D residual: uniform bank c3 "in no memory pool: zeros bound"** (all 49 red lines in `-before`, 3 shaders, at the
   menu-to-race change). Cause: `mem::pool_del` (nvnMemoryPoolFinalize) removed the pool from the address map at once
   on the game thread, while replay ops the game queued before the finalize still resolved addresses in it (async
   replay runs behind the game). Textures already get queue-order release (`tex_release`); pools did not.
   Correction (`vk/mem.rs`): `pool_del` reads the pool's `identity` and, while the replay queue is busy, carries out
   the removal through `queue::after_queued` (`pool_del_now`). `take_same` removes the entry only if it is still that
   registration, so a pool registered again at the same base meanwhile stays. `NXGFX_POOL_DEL_ORDER=0`: at once, as
   before (A/B).
4. **MK8D: 2D-array view of a 2D texture sampled as a cube** (2 red lines, race start, `texture ID 4594: target 4 bound
   where the shader samples target 8`, a 1024x1024 BC3 2D image with a one-layer 2D-array view). `vk/exec.rs`
   `cube_twin_target`: HEAD's cube twin now also takes a `T2D_ARRAY` base. `cube_twin_view` already requires one square
   2D layer. Log: `texture ser142618 ... sampled as a cube: a six-face twin`.
5. **MK8D: finalized textures read as the zero stand-in** (4314 log lines a run, the largest residual by count, menus
   only, 0-50 s: `slot names texture serN, which has no image now (released or never made)`). Diagnosis:
   `NXGFX_LAZY_IMAGES=0` gives 0 of them (`-192542-mk8d-eager60`), so lazy registration causes them. The menus
   register textures and finalize the objects (serials grow by thousands a second) before the first draw that uses
   them. `tex_release_now` then dropped the waiting registration (`lazy::forget_serial`), so every later draw read
   zeros. On the console the pool descriptor outlives the object, and in eager mode the image stays parked with its
   slots. Correction (`vk/image.rs` `lazy`): `finalized(serial)` keeps those registrations, marked finalized. `take`
   and `take_all` report the mark, and `materialize`/`materialize_all` make the image for that use and release it
   after the submission (`release_after_use`, `defer_after_use` to `tex_release_now`): the same parked state eager
   mode has. `NXGFX_LAZY_KEEP=0`: dropped, as before.

### Tests
- `mem::pool_del_tests::a_queued_finalize_takes_only_the_pool_it_named` asserts that a queued finalize leaves a pool
  registered again at the same base. New, so there was no "before" to fail.
- `exec::cube_twin_target_tests::two_d_and_one_layer_array_views_sampled_as_cubes_get_a_twin`: the `T2D_ARRAY` case
  fails on HEAD's condition.
- `image::lazy::tests::a_finalized_textures_waiting_registration_stays_marked`: a waiting registration survives
  finalize, marked; it is taken once; an unfinalized one is not marked; the mark goes with the last registration. It
  fails on the old `forget_serial` (the registration was gone).
- `cargo test --jobs 4 -p graphics --lib`: 184 passed, 0 failed (final tree). `py -m unittest scripts.test_play`: 4 OK.

### Verified (all `build/play/20261124-*`, sheets looked at)
| run | result |
|---|---|
| `-184022-mkw180-route` (cold caches) | race at 303 s, coins 5, lap 1/3, 24th; 75 "compilation stopped" lines at exit |
| `-184621-mkw180-warm` (HEAD engine) | grid at 250 s, driving off-track with triple mushroom, coins 5 to 7; 13.0 FPS, 1 red |
| `-190822-mkw180-after` (final) | grid 250 s, driving, coins 5 to 8; 14.8 FPS (5% low 12), 1 red |
| `-191408-mkw-base-check` (pool fix) | world map, grid, race, coins 5 to 9; 17.7 FPS; 2 red (known); 41 stand-in lines |
| `-193353-mkw-lazykeep` (final) | world map, grid, race with triple mushroom, coins 5 to 7; 18.0 FPS; 2 red (known); 23 stand-in lines; peak 15366 MB (was 15824) |
| `-193941-mkw180-final` (final) | grid 250 s, racing with triple mushroom, item boxes, coins 5 to 9; 16.9 FPS (5% low 13); 1 red; 22 stand-in lines |

MK8D, same scenario, window [95, 175]:

| run | FPS / 5% low / min | red lines | >50 ms frames | memory |
|---|---|---|---|---|
| `-185525-mk8d-before` (HEAD) | 59.8 / 58 / 55 | 49 (5 kinds) | 4 | 4754 to 5016 MB |
| `-190217-mk8d-after` | 59.8 / 57 / 55.8 | **0** | 5 (1 is 3 pipelines compiled) | 4754 to 5022 |
| `-191951-mk8d-diag` (`--diag render`) | 59.8 / 59 / 56 | **0** | 4 | 4984 to 5200 |
| `-190521-mk8d-control-order0` (pool-fix build, `NXGFX_POOL_DEL_ORDER=0`, 100 s) | n/a | 29: c3 no-pool lines are back | | |
| `-193052-mk8d-lazykeep` (final) | 59.9 / 59 / 57 | **0** | 4 (all captures) | 4736 to 4741 (peak 4950, was 5228) |

Zero-stand-in lines ("released or never made"): `-before` 4390, `-after` 4314, `-192324-mk8d-menus` 7285,
`-192542-mk8d-eager60` (`NXGFX_LAZY_IMAGES=0`) 0, `-193052-mk8d-lazykeep` **0**. Six other lines remain: ID 0 with
nothing registered, a different kind.

Sheets: Mii, menus, grid, driving 4th to 1st to 3rd, items, lap 2/3 (`-before`, `-after`, `-diag`).

**Stutters** (the "75 stutters" residual): the remaining >50 ms race frames line up with the harness's own
captures. CSV hitches at 96.4/117.5/159.6/167.8 s match the shots at 98/119/161/169 s, with a steady 1.2-1.6 s clock
offset, in all three runs. A capture's frame shows low replay (4-13 ms) and GPU (2-6 ms), with a sync wait in the
game. The pacing line is 59.7 FPS, max 24.6 ms between shots. **Memory:** one 256 MB allocator block at ~105 s (blocks
1664 to 1920 MB), then flat to 175 s: a step, not growth.

### Unproven
- **The finalized-texture fix has no visible before/after:** the menus looked complete before it too
  (`-192324-mk8d-menus`: Mii, cc, characters, kart parts, cup). What is proven: the stand-ins (4314 to 0), unchanged
  race FPS, and lower peak memory. Which menu elements read zeros before was not identified.
- MK8D's ID 0 multisample bind with nothing registered (6 lines a run) was not looked at.
- MKW (both entries): `texture ID 337: target 2 bound where the shader samples target 1` (a 64^3 3D texture read as
  2D, once a run) and 13-18 FPS. Not in this wave's scope.
- Both corrections change memory lifetimes (protocol §4), so an audit is recommended. Pool ordering: while queued ops
  remain, a finalized pool still resolves on the game thread too (`addr_known`/`room`). Lazy keep: a finalized
  texture's image is made from its memory at first use, which may by then hold other data, as it would on the
  console. Kept registrations stay until their ID is registered again (`registered without one yet` 6331-6800 in
  MK8D, was ~4500).

### Slips
- `-185259-mk8d-first` was deleted (superseded by `-before`). A temporary `[mk8d-menus]` scenario was added for one
  run and removed. `-192542-mk8d-eager60` (60 s diagnostic) was killed at its time limit, outside any compile.
  `pipeline-crashed.bin` was not touched. No device loss or driver reset. Changed files: `scripts/scenarios.toml`,
  `src/graphics/src/vk/{mem,exec,image}.rs`, this file (`image.rs` was not in the expected list).

### Questions
- "Mario Kart World v1.8.0" builds the same base NUP as "Mario Kart World", with no update attached and no 1.8.0
  update on disk. Should it get a real 1.8.0 update, or be merged with the base entry? I recommend attaching the
  update when the owner has one; until then it duplicates `[mkw]`.

## Lead review (2026-11-24 20:00)

Lead re-ran graphics lib (184 passed) and read `-193052-mk8d-lazykeep/sheet-1.png` (title, Grand Prix start, racing
through lap 2/3, 59.9 FPS, 0 red lines). Independent audit (protocol §4, read-only) of the two lifetime changes found
no use-after-free, and:
- **Pool finalize ordering (`vk/mem.rs`): not accepted yet, left uncommitted in the tree for a follow-up.**
  (1) Ineffective for same-base reuse: `pool_add` (`mem.rs` ~583-590) inserts the new pool and releases the old one at
  once, so queued work resolves the old pool's addresses to the new buffer; the deferred removal then finds a
  different identity and does nothing. (2) Work in parked queues is not "queued before" (`queue.rs` `aq_main`
  ~1458: a parked queue's stream is appended, `done` advances; the deferred call runs first). Same gap exists for
  `tex_release`. (3) New risk: while a finalized pool stays mapped, a new overlapping pool at a lower base is shadowed
  for game-thread lookups (`find` -> `addr_known` in `nvn/prog.rs`, `room`), and both pools are write-watched over
  the same pages, so the batched scan may hand a CPU write to the old one and leave the new one stale (plausible,
  not traced to the end).
- **Lazy keep (`vk/image.rs`): accepted.** Bookkeeping, ID reuse and release-after-use checked correct. Cost to fix
  later: kept registrations stay until their ID is reused or `materialize_all` (first pool-indexing shader) makes
  images for all of them at once, a VRAM/upload spike that can read reused guest memory; `N > 0` defeats `take`'s
  fast path. Edge: a re-registered stale copy loses the finalized mark (as in eager mode).
- Cube-twin target (`exec.rs`) and the scenarios: accepted.
Committed: everything except `vk/mem.rs`.
