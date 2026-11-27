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
