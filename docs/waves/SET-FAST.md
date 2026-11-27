# SET-FAST — per-draw descriptor and dynamic-state cost (MKW off its 30 FPS hold)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~21:00, worktree
`D:\Projects\RecompLauncherEngine-setfast`, branch `setfast` (base `bdb066b`, USE-SYNC's chunk tracking included and on).
Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0 (OOT-DESC owns OoT on GPU 1). Deadline
4 h. Design source: `REF-ARCH.md` differences #2, #3, #4 and its SET-FAST wave.

## Plan (written before code)

Step 0: one `mkw` run on the base build (FPS / 5 % low, replay ms, prep ms, lane CPU ms, descriptor ms), then one with
`NXGFX_PROFILE=1` for the set split (whole / patched / reused, ns per part) that REF-ARCH asks for before changing
the path.

1. **Dynamic state only on change** (`exec.rs` `draw_setup_ex`): a shadow per command buffer of every `vkCmdSet*` the
   draw path emits (`DynShadow` in `QState`, keyed by the submission serial: each submission is one freshly begun
   command buffer, child segments included). Every graphics pipeline shares one dynamic list (`pipe.rs`
   `dynamic_states`) except mesh pipelines (no topology / primitive restart / vertex input) and tessellation pipelines
   (patch control points): those states are forgotten when the pipeline class changes, as Vulkan disturbs them. No
   other graphics pipeline is bound into a game command buffer. Also: the per-stage descriptor-buffer offsets set again
   only when they differ (same universal layout for every pipeline, so binds survive pipeline changes).
   `NXGFX_DYN_DEDUPE=0` restores the old behaviour.
2. **Set reuse by version counters** instead of building the key per set: decided from step 0's split.
3. **Texture descriptors by ID** from the bindless pool set's shadow (`tables.rs`) instead of `tex_view` + the images'
   lock + `vkGetDescriptorEXT` per slot: decided from step 0's split.
4. **Lanes 0 / 2 / 6** (`NXGFX_TRANSLATE_THREADS`) on the final build; recommend a default by FPS.

Each behind a switch; default on only once the pictures are clean; rows in `docs/env_switches.tsv`. Files expected:
`vk/exec.rs`, maybe `vk/tables.rs`, `docs/env_switches.tsv`, this doc.

What would disprove it: the set split shows the cost is not in the key, the views or the copies, or the replay thread
is bounded by something these changes do not touch (pool sync, present, waits).

## Step 0: baseline and the set split

`20261126-210150-sf-base-mkw` (base `bdb066b`, GPU 0), window [250, 325] s, frames-CSV means: **32.4 FPS, 5 % low
28.7**; frame 30.8 ms; replay 10.8 + prep 6.7 = **17.5 ms replay thread**; pool sync 4.2; descriptor 12.1 (lanes); GPU
4.8; 1,840 draws. Lanes 14-25 ms CPU a frame, segments' wall 7.1-8.3 ms. Sets a frame: ~2,500-2,850 written (of them
~2,000-2,280 patched, the rest whole) and ~810-930 reused. Route 3/3 on.

Set split (`20261126-210947-sf-s1-prof-mkw`, `NXGFX_PROFILE=1`, race window; ns a set before the bindings / in them /
chunk copy):

| path | share | before | bindings | chunk copy | why |
|---|---|---|---|---|---|
| patched | ~82 % of written sets | 580-610 | 1,390-1,425 | 60-64 | classes remade per 100,000 patches: uniform buffers 93 k, c2 34 k, textures 40 k, c0 27 k |
| whole | ~18 % | 1,010-1,040 | 6,140-6,440 | 63-71 | 95-97 % "no copy of the module": a lane segment starts with empty host copies (`fork_state`) |

Set time in all (summed over the lanes): 9.0 ms a frame: views 1.9, writes (`vkGetDescriptorEXT`) 1.3, banks 1.1,
memory 0.6, **keys 0.00**.

What this says about the brief's items:

- **Item 2's premise (building and hashing a key per set) does not hold here**: `SetKey` is a plain struct compare,
  its cost rounds to 0.00 ms a frame, and the reuse rule is already Turbo's (one last set per stage, compared by
  bind generations). Turbo's `g_lset` has the same shape. What is left of item 2 is the images' lock under the view
  lookups and the tables' generation that throws every kept view away at any texture registration: that part was
  built (step 2 below).
- **Item 3's premise (176 slots copied per set) does not hold either**: the chunk copy is 60-70 ns a set (~0.2 ms a
  frame over the lanes). The per-set texture cost is the view lookup (step 2) and `vkGetDescriptorEXT` (1.3 ms a frame
  for every descriptor kind). Copying combined image-sampler descriptors out of the pool set's separate sampled-image
  and sampler descriptors is not something Vulkan defines (driver-private layouts), so it was not built.
- **The frame is additive, not bounded by the replay thread alone**: frame ≈ replay thread + ~13 ms in every run
  (base 30.8 = 17.5 + 13.3; profile run 32.9 = 20.2 + 12.7). Each ms off the replay thread is a ms off the frame; 60
  needs the replay thread near ~3.5 ms, out of reach of this wave.

## Step 1: dynamic state only on change (`NXGFX_DYN_DEDUPE`, commit `102bbc0`)

As planned: `DynShadow` per command buffer (submission serial), every `vkCmdSet*` of `draw_setup_ex` compared with
the shadow and emitted on change; topology / primitive restart forgotten when the pipeline class changes (mesh vs
ordinary); patch control points always set (only tessellation pipelines have it dynamic). The graphics stage sets'
and the pool set's descriptor-buffer offsets are deduplicated the same way (forgotten when the descriptor buffers are
bound again; compute offsets untouched, since compute work in the same command buffer binds classic sets).

It works as intended: **~55,000-66,000 calls a frame are no longer made** (of ~60,000-70,000: about 30 a draw over
~1,900 draws). It does not show in the frame:

| Run | switch | FPS / 5 % low | frame | replay | prep | descriptor | lanes CPU | segments wall |
|---|---|---|---|---|---|---|---|---|
| `20261126-211604-sf-s1-on-mkw` | on | 30.2 / 28.7 | 33.1 | 11.9 | 8.5 | 14.2 | 17.9-23.4 | 7.3-8.8 |
| `20261126-212142-sf-s1-off-mkw` (right after) | `NXGFX_DYN_DEDUPE=0` | 30.4 / 29.0 | 32.8 | 11.6 | 8.1 | 12.2 | 18.9-22.4 | 7.6-8.7 |

On vs off is inside the run-to-run noise (the base run on the same machine gave 32.4 FPS and a 17.5 ms replay thread).
REF-ARCH's estimate was 1-2 ms of CPU spread over the lanes, 0.3-0.7 ms wall; the noise between two MKW runs is
larger than that. Kept on (pictures correct in both runs, route 3/3); it costs nothing measurable either way.

## Step 2: view lookups by per-ID version (`NXGFX_VIEW_BY_ID`, commit after `102bbc0`)

Diagnosis from the code: `tex_view`'s per-submission memo and a stage's kept texture part (`TexPart`) were keyed by
the tables' `generation`, which moves at every texture registration, every pool descriptor write and every placeholder
mark anywhere; each such event threw away every kept view, and the next lookups took the tables' and images' locks
again. Built: `Tables::view_version(id)` (lock-free: an image-level epoch, moved by `invalidate_images` as before, plus
65,536 per-ID buckets moved by that ID's registration, placeholder mark, buffer-texture registration, pool descriptor
write, and the lazy registration's park; samplers as `!sid`). The memo key and `TexPart` use the versions of the IDs a
set binds. `SetKey`'s `image_gen` is unchanged (still the generation).

Measured: **no effect.** Profile split, race window means (`NXGFX_PROFILE=1`, 38 lines each):

| Run | sets (lanes, ms a frame) | views | banks | writes | memory | whole set ns (before / bindings / copy) | patched |
|---|---|---|---|---|---|---|---|
| `20261126-210947-sf-s1-prof-mkw` (step 1 build) | 14.86 | 3.79 | 1.56 | 2.22 | 0.75 | 992 / 8,320 / 63 | 599 / 2,009 / 58 |
| `20261126-214625-sf-s2-prof-mkw` (step 2, on) | 15.33 | 3.74 | 1.57 | 2.23 | 0.80 | 968 / 8,363 / 61 | 589 / 2,006 / 59 |

So the generation churn was not what made the views cost: the misses are each lane segment's own submission
starting with an empty memo (a child submission per segment; its first use of an image must run `prepare_image` for
that submission, so the memo cannot outlive it), and the whole sets at each segment's start (95-97 % "no copy of the
module") look every texture up again.

With the by-ID keys, the run-shared memo (`NXGFX_VIEW_SHARE=1`, `20261126-220513-sf-s2-share-mkw`) found 40-44 k of
0.92-1.06 M lookups in 600 frames (~4 %, as TRANSLATE-COST's ~3 % before): parallel segments look the same textures
up at the same time, before any of them has put one. (That run's FPS, 6.0, is not the switch: a web wave's `wasm-opt`
took ~23 cores from ~22:05 on; replay 11 ms and GPU 11-13 ms in 150-200 ms frames.)

Since it changed nothing measurable, **step 2 was taken out again** (the files restored to `102bbc0`; the switch row
removed). Its commit stays in the branch history as the record.

## Step 3: texture descriptors by ID from the pool set: not built

The measured premise is absent (step 0: the 176-slot copy is 60-70 ns a set), and the remaining per-texture work
(`vkGetDescriptorEXT` for a combined image sampler) cannot be replaced by copying the pool set's separate
sampled-image and sampler descriptor bytes: Vulkan does not define a combined descriptor's bytes in terms of the two.
The texture cost that matters is the per-segment view lookups (step 2's diagnosis), which this would not remove.

## Step 4: lanes 0 / 2 / 6 (`NXGFX_TRANSLATE_THREADS`)

Step-2 build (its switch neutral), runs right after each other, window [250, 325] s:

| Run | lanes | FPS / 5 % low | frame | replay + prep = replay thread | descriptor | lanes CPU a frame | segments wall |
|---|---|---|---|---|---|---|---|
| `20261126-214047-sf-s2-on2-mkw` | 6 (default) | 33.6 / 29.5 | 29.8 | 10.2 + 6.3 = 16.6 | 12.2 | 12.1-24.4 | 6.9-7.8 |
| `20261126-221053-sf-lanes2-mkw` | 2 | 33.1 / 28.7 | 30.2 | 11.6 + 6.3 = 17.8 | 9.0 | 11.9-15.8 | 8.3-9.8 |
| `20261126-221631-sf-lanes0-mkw` | 0 (one piece) | 30.0 / 27.0 | 33.3 | 15.5 + 7.4 = 22.9 | 8.7 | - | - |

One piece on the replay thread costs ~5-6 ms of replay thread and ~3 FPS. Two lanes hold the same FPS as six within the
noise (+1.2 ms replay thread) for about two thirds of the lane CPU.

**Recommendation: keep 6 as the default for now.** Six is ahead on the replay thread (−1.2 ms) and 2 is not ahead on
FPS; 2 frees ~6-8 ms of CPU a frame, which matters only when something else needs the cores (OoT, the web waves). If
the lead's quiet runs confirm 2 = 6 in FPS, 2 is the better default (less lock contention, less CPU for the same
frame). 0 is clearly worse.

## Final build (`e9b2cc4`: step 1 only; relinked 22:23)

| Run | switches | FPS / 5 % low | frame | replay + prep | descriptor | lanes CPU | segments wall | route |
|---|---|---|---|---|---|---|---|---|
| `20261126-222743-sf-final-on-mkw` | defaults | 28.8 / 13.9 | 34.4 | 13.2 + 11.1 | 14.4 | 18.3-24.4 | 7.7-9.2 | 3/3 on |
| `20261126-223319-sf-final-off-mkw` (right after) | `NXGFX_DYN_DEDUPE=0` | 28.8 / 11.3 | 34.4 | 11.9 + 7.9 | 13.9 | 20.3-25.3 | 7.8-9.0 | 3/3 on |
| `20261126-223857-sf-final-lanes2-mkw` | 2 lanes | 17.2 / 1.8 (**contaminated**: OOT-DESC's OoT started 22:38:47) | 58.2 | 23.4 + 11.4 | 13.7 | 17.4-21.8 | 11.8-26.3 | 3/3 on |
| `20261126-224436-sf-final-turn` (`mkw-turn`, [250, 290]) | defaults | 23.7 / 6.1 (OoT running) | | | | | | 4/4 on |

Both mkw runs have a stretch of 4-13 FPS frames at 245-255 s (in the on run with prep at ~11 ms over the window, in the
off run without); the same kind of stretch hit `s2-on`, `s2-off` and `s2-off4` at 258-275 or 322-329 s, with switches
on and off, and with replay ~10-15 ms and GPU 5-15 ms inside 150-250 ms frames (the frame lost outside the replay
thread). They come and go with what else the machine runs (`wasm-opt` on ~23 cores 22:05-22:11, OoT from 22:38), not
with these switches. All numbers here are indicative (shared machine); the clean pairs are `s1-on`/`s1-off` and
`s2-on2`/`lanes2`/`lanes0`.

**Before vs after, on the cleanest runs:** base 32.4 / 28.7 FPS, replay thread 17.5 ms (`210150`) against 33.6 / 29.5,
16.6 ms (`214047`, step 1 on). That 0.9 ms is inside the spread of same-build runs (16.6-20.3 ms). **MKW stays held
near 30; this wave does not lift it.**

Pictures (all sheets read, every run): boot, title / course flyover, menus, grid, race and lap 2 drawn correctly in
every run; no pop-in, no stale or wrong textures, no missing geometry; `mkw-turn` 10 shots through the banked turn
under the arch and onto the steel deck (t37-t46), lit and textured correctly (`build/play/20261126-224436-sf-final-turn/
sheet-1.png`, `sheet-2.png`; before: `build/play/20261126-210150-sf-base-mkw/sheet-1.png`; after:
`build/play/20261126-222743-sf-final-on-mkw/sheet-1.png`). Anomalies: the known set only (stand-ins at loads, the
timestamp-query and crashed-pipeline red lines, gate counts as before).

## What is left (by the measured split)

- **Whole sets at each lane segment's start**: ~700 a frame at ~9.3 µs (1.0 before + 8.3 bindings) ≈ 6.5 ms of lane
  CPU. 95-97 % have no host copy of their module because a segment's `QState` starts empty and patching is tied to the
  submission (staged banks, `prepare_image`, `note_gpu_write` are per submission). Carrying copies across segments
  needs every per-submission side effect redone for the classes that have one; not attempted in this deadline (the
  risk is a set naming another submission's staging).
- **Patched sets**: ~2,300-2,600 a frame at ~2.6 µs ≈ 6.5 ms of lane CPU; 93 % remake uniform buffers. The untimed
  part of the bindings loop (~half of `sets` ms: the profile's views + banks + writes + memory are 8.3 of 15 ms) is not
  attributed; a stack profile of the lanes (`NXRS_HOST_PROF_STACK=1` with a PDB relink) is the next step before
  changing it.
- The frame is replay thread + ~13 ms in every run; the other 13 ms (game side, waiting on the replay) is outside
  SET-FAST and decides as much of MKW's frame as the replay thread does.

## Report (agent, protocol §5)

Branch `setfast`: `102bbc0` (plan, step 1), `22766a0` (step 2), `e9b2cc4` (step 2 taken out, doc), this commit (doc).

### 1. Built
- Step 1 (`NXGFX_DYN_DEDUPE`, default on): dynamic state and graphics set offsets emitted only on change per command
  buffer. Removes ~55-66 k driver calls a frame; no measurable FPS or replay-thread change (on/off pairs above).
- Step 2 (per-ID view versions): built, measured neutral (views 3.79 → 3.74 ms), taken out. Diagnosis kept: view
  lookups miss because each lane segment is its own submission, not because of the tables' generation.
- Steps 2's key premise and 3 do not hold on HEAD (key 0.00 ms, chunk copy 60-70 ns a set): said with numbers, not
  built.
- Step 4: lanes measured; recommendation above (keep 6 pending the lead's quiet A/B with 2; 0 is worse).

### 2. Tests (`cargo test --release -p graphics`: 273 passed, 6 ignored)
- `dyn_shadow_tests::states_are_set_once_per_command_buffer_and_value`: equal state skipped; a new command buffer, a
  changed value, or a mesh/ordinary class change (topology) sets it again.
- `dyn_shadow_tests::set_offsets_are_bound_once_until_the_buffers_move`: an offset is bound once per command buffer
  and set, again after the descriptor buffers are bound again or in a new command buffer.
- New behaviour, so no failing-before state. (Step 2's test went out with step 2.)

### 3. Verified
Runs, numbers and sheets above. Route checks on in every mkw / mkw-turn run (3/3, 4/4). FPS 60 not reached; MKW
stays near 30.

### 4. Unproven
- A quiet same-build A/B (the lead's): step 1's effect is below run-to-run noise here.
- Lanes 2 vs 6 on a quiet machine (the final-build lanes-2 run was contaminated by OoT).
- Validation-layer run with step 1 (dynamic state set before a pipeline with the same dynamic list; mesh / tess class
  rules) was not done.

### 5. Slips
- Three of my runs were cut short by their background shell being stopped (not by me, as far as I can tell;
  `20261126-215210-sf-s2-off2-mkw`, `20261126-215539-sf-s2-off3-mkw`: the harness died, the game ran on; I closed
  that game by PID 39236). After that I used one background chain at a time and no `TaskStop` on waiters (my one
  `TaskStop` on a waiter coincided with the second cut).
- Relinks ran this worktree's `nxrs-build` directly with the launcher's own argument list (as USE-SYNC did).
- Step 2's files were restored with `git checkout 102bbc0 -- <three files>` (a git restore, not a script edit).

### 6. Questions
None.
