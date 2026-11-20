# Frame rate

How fast a game runs and how evenly its frames reach the screen. Five mechanisms meet here:

| Mechanism | Where | What it changes |
|---|---|---|
| Present pacing | `src/graphics/src/nvn/window.rs`, `src/graphics/src/vk/present.rs` | when an acquire returns and which presents are shown |
| Frame step | `src/os/src/fstep.rs` + the profile's FRAME_STEP stubs | the time step the game advances by, and how long a frame stays on screen |
| Engine frame lock | `src/os/src/fpslock.rs`, `src/cli/src/fpsfind.rs` | an engine's own frame cap, raised in game memory |
| Forced present interval | `window.rs` (`present_interval`), profile key | the vsyncs a frame lasts, whatever the game asks |
| Frame generation | `src/graphics/src/vk/framegen.rs`, `lsfg.rs`, `src/front/src/lsfg.rs` | extra images between the game's frames |

The ini options are in [docs/settings.md](../settings.md) (Performance, Frame generation).

## Present interval and display rate

### Acquire pacing

Vsync blocks in the window acquire, not in present: a game often presents while holding a lock
(`nvn::window`). The pacer keeps a slot cadence: present slot = previous slot + period; early waits until the slot,
late by less than a period presents now and keeps the phase, behind by a period or more restarts the cadence (no
burst to catch up). The period is the present interval times one display slot (`vk::present::pace_period`):

- `fps_limit=60` (default): a slot is 1/60 s. `fps_limit=0` (F4 toggles 60 / unlimited): no wait at all. Above 60 the
  slot is that much shorter.
- `fps_limit=30`: the game keeps its 60 Hz steps and every other image is not shown (`show`).
- Waits use a high-resolution waitable timer, then a short spin.

The swap chain runs MAILBOX when offered so the display adds no second clock; `forced_vsync=1` makes it wait for the
display's refresh (FIFO), which can cost a lot on slow PCs. `NXGFX_PRESENT=fifo|mailbox|immediate` overrides.

### Forced present interval

`present_interval` (ini; 0 = the game's own, 1 / 2 / 3 forced) wins over the profile's `present_interval` key
(`set_profile_interval`, called by the OS layer at start). Forcing 1 on a 30 FPS game draws it at 60; only games that
keep their speed tolerate it (PORT_PLAN 5A.6, "Forced present interval", and its "Which games are safe at 60" row,
which records results per game). Off by default.

### Screen refresh match

In fullscreen, a monitor whose rate is not a multiple of the game's is switched to the mode of the same size with the
highest rate that is (144 -> 120 Hz for 60 fps) (`vk::screen`). The switch is temporary (`CDS_FULLSCREEN`: Windows
restores it when the process ends, a crash included); leaving fullscreen restores it at once. `NXGFX_SCREEN_MATCH=0`
keeps the rate.

### Frame skip

`fps_target=30` or `20` (ini) draws only every second or third frame while the game keeps its speed: the replay skips
the draws of the frames in between and they are not shown (`exec.rs` `frame_skip`; log `fps_target: frame skip <n>`).
Effects updated on skipped frames can flicker. `fps_target=120` asks frame generation for x2 at 120.

## Frame step

nxnative's frame step: the game advances by a time step the host chooses, so a game whose frame cannot hold 60 runs
at 30 (or 20) with each frame worth two (three) vsyncs instead of slowing down. The build places nxnative's
FRAME_STEP / MATCH_FRAME_STEP stubs into a patch area after main (`<main>_stubs`, sets `fstep`, `NXRS_PATCH_FSTEP`)
and writes `<data>\frame_step.txt` (`step_max`, `render_scale`, `capabilities`); see [rewriter.md](rewriter.md).

State page (first page of the stubs module, written by the host at each present):

| Offset | Value |
|---|---|
| +0 | target fps (f32; 0 = off: the stubs keep the game's own timing) |
| +0x10 | render scale (u32) |
| +0x20 | the step for the game's next frame, in vsyncs |
| +0x24 | a step the game fixes itself (written by the stubs; 0 = none) |
| +0x30 | vsyncs stepped (counted by the stubs) |
| +0x40 | dynamic resolution allowed (u32) |

The present tick (`fstep::present`, once per present):

- `shown` is the time since the previous frame's start (the first acquire after a present,
  `window::frame_start_take`); `cost` is max(CPU = shown less that acquire's pacing wait, GPU = GPU time of the
  submissions finished since the last tick).
- The controller (`fstep::Fstep::next`) gives the hold `k` (vsyncs the frame stays on screen, which becomes the
  present interval) and the next step. At a steady rate the step is the time on screen; real time the game falls
  behind is a debt paid back one vsync a frame, so game time equals real time.
- `k` rises after three late frames in a row, to the cheapest of them, when frames at `k` also fall short on average
  over the last `RUN` frames (the longest left out: one stall is not a rate). `RUN` on-time frames whose cost fits
  `k - 1` try it; a failed trial waits twice as long before the next (1 s up to 8 s). A GPU whose clock drops with a
  longer hold starts a probe back to `kmin`.
- The target sets the shortest hold: 60 -> 1, 30 -> 2 (`kmin_of`). The profile's `fps_target`, `NXOS_FPS_TARGET`
  overrides; 0 turns it off. The profile's `fps_hold` off keeps the hold at the target's vsyncs.

Log lines: `frame step: target <n> fps`, `frame step: hold <a> -> <b> vsyncs at present <p> (cost .., GPU ..)`,
`frame step: the game fixes its own step at <n> vsyncs`, `frame step: frames held <n> vsync(s)`.

Profiles with stubs today: Ocarina of Time (frame ratio + dynamic resolution) and Mario Kart World (frame interval);
PORT_PLAN 5A.4 ("Frame step and its present tick") is Done in code but not yet run in a game.

## Engine frame limiters

Some engines cap their own frame rate with a value in memory the host can raise to the profile's `fps_target`
(`NXOS_FPS_TARGET` overrides; 0 leaves the game's limit). The values are written at each present whenever they differ,
since games set them after boot and again later (`src/os/src/fpslock.rs`):

- fixed rows per main build (FPSLocker's evaluate-write entries, nxnative's table): a rate as float, double or int,
  vsyncs per frame, a frame time in ms, ticks or seconds, or an engine's 60 fps switch;
- NieR:Automata 1.0.3's display object (present interval, interval mode) and its resolution floor;
- Frostbite's `SimulationTimeSettings` (ForceSimRate / MaxSimFps), found by name in any build;
- places `nxrs-build` finds in main and writes to `<data>\fps_lock.txt` (`src/cli/src/fpsfind.rs`): Unity's
  `targetFrameRate` and `vSyncCount`, Unreal's `t.MaxFPS`, `rhi.SyncInterval`, `nvn.SyncInterval`,
  `Project.FrameRateLimit` and `UEngine::FixedFrameRate`, Monolith's frame interval.

Log lines: `fps lock: <what> <old> -> <new> fps`, `fps lock: <what> left as the game sets it (no fps_target)`,
`[RED] fps lock: ...` when a place cannot be written.

Mario Kart World's own frame rate is its game layer's: `NXRS_MKW_FPS=game|auto|60|30|20|15`
(`src/os/src/gamefix/mkw.rs`). Pokémon Legends: Z-A's 60 FPS patch is the owner's `turbo.toml` cut. Player mods with
60 FPS patches load as code patches ([cheats.md](cheats.md#mods)).

## The game's own dynamic resolution

Games time their GPU work with two timestamp reports and size their frame by it. `game_dynamic_resolution` (ini:
`auto` / 1 = the GPU's real times, 0 = a clock 256 times slower, so the game always picks its largest size)
(`vk::counters`). By default idle time between a frame's submissions is left out (`NXGFX_REP_GAPS`); Donkey Kong
Bananza sat at its smallest size before that. The host's own resolution scale is separate (`resolution`,
`dynamic_resolution`; [rendering.md](rendering.md#render-targets-passes-and-clears)).

## Frame generation

`lsfg=1` with `frame_gen_type`:

- **builtin** (`vk::framegen`): each presented frame is copied into a history image; motion between the previous and
  current frame is estimated on a luma pyramid (`fg_flow.comp`) and in-between frames are warped from both and blended
  (`fg_interp.comp`), presented before the real one by a thread of their own. The pacer measures the game's interval
  and makes enough frames to reach `lsfg_target` (at most `lsfg_multiplier - 1` per frame, carrying the fraction), none
  when the game is at or above the target, below 10 FPS, or in a hitch (then a second's pause). Identical frames and
  scene cuts get none (`fg_stats.comp`).
- **lsfg**: LSFG 3.1's passes read from the player's own `Lossless.dll` (`lsfg_dll`, beside the game, or Steam);
  another version is refused with its reason (`src/front/src/lsfg.rs`), and a missing or unusable DLL falls back to
  builtin, said once. Run on the present path by `vk::lsfg` (`lsfg_flow`, `lsfg_resolution`).

`NXGFX_FRAMEGEN=0` turns it off whatever the settings say; `NXGFX_FG_SHOT=<s>` saves one generated frame and its
neighbours. Off by default (owner decision, DECISIONS.md section 1); it garbled Arceus's village in an earlier run
(AUDIT.md headline 5).

## CPU cost and pacing

- **The pacing line** (`vk::present`, the log contract the launcher and harness parse):
  `[  t.tt] [vk] pacing: F FPS | p95 a p99 b max c ms | >16.67 n >33/50/100 x/y/z (last N intervals)`, every
  `NXGFX_PACE_N` presents (300).
- `[vk] slow frame` lines say where a frame over `NXGFX_SLOW_MS` went (pipelines, uploads, waits);
  `NXGFX_FRAME_CSV=<path>` and the ini's `metrics=1` (`metrics-<time>.csv`, `frames-<time>.csv`) give per-second and
  per-frame figures.
- **Frame threads** join MMCSS "Games" (`vk::rtclass`, `NXGFX_RT_THREAD=0`).
- **Replay thread**: the replay runs on its own thread so the game's thread only records (`NXGFX_ASYNC=0` to compare).
- **Spin waits** without a checkpoint cost a whole core; see [threads.md](threads.md#yields-and-spin-waits).
- **Profiling**: `profile=1` (guest functions, `profile.txt`), `SWR_HOST_PROF` (host thread samples), `NXGFX_PROFILE=1`
  (replay sections).

## Problems found on games

### One stall raised the hold

- **Symptom:** frames held for two vsyncs after a single long frame (nxnative issue #1823, Mario Kart World's
  near-misses).
- **Cause:** a lone stall counted as a rate.
- **Fix:** the longest frame is left out of the `RUN` average (nxnative issue #1823; test
  `near_misses_and_lone_stalls_keep_hold_1`).

### Idle gaps read as GPU time

- **Symptom:** Donkey Kong Bananza stays at its smallest render size.
- **Cause:** the idle gaps between its 8 submissions a frame counted as GPU time in its two-report measure.
- **Fix:** gaps between submissions are left out by default (`NXGFX_REP_GAPS=2`).

### Slow frames at every new place

- **Symptom:** Super Mario Bros. Wonder stutters (60-2,000 ms frames) entering new places.
- **Fix:** pipeline libraries and profile prewarm ([shaders.md](shaders.md#pipelines-and-caches)); first-run stutter
  remains (PORT_PLAN G-lane).

## Known limits

- PORT_PLAN 5A.6: frame generation and the forced interval are in; the per-game safety table is still being filled.
- nxnative's display-link and ProMotion handling has no counterpart; refresh match and MAILBOX are the Windows
  equivalents.
