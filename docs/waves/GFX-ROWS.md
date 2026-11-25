# GFX-ROWS — open graphics rows from the ledger (descriptor limits, texture 337, MS views, materialize spike)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Rows (from `docs/32-REMAINING-WORK-PLAN.md`, current section; read each row and the wave doc it names):**
1. **Per-stage descriptor limits.** Every graphics stage sees all five stage sets (~880 descriptors) through the shared
   layout: fine on NVIDIA, unchecked elsewhere. Add a startup check against `maxPerStageDescriptor*` /
   `maxPerStageResources` (and the descriptor-buffer limits the layout relies on) that fails with a clear red line and
   status instead of undefined behaviour; if the shared layout can be sized from the limits, do that. CPU tests with
   fake limits (NVIDIA's, an AMD/Intel-like set).
2. **MKW `texture ID 337: target 2 bound where the shader samples target 1`** (a 3D noise texture read by a 2D
   instruction). Decide the fetch semantics from source and documentation (what NVN/Maxwell does when a 2D fetch hits a
   3D descriptor), then bind what the console would sample (e.g. slice 0 / a 2D view of the 3D image) instead of a
   mismatch. The new one-slice 3D twin (`docs/waves/SONIC-SMASH.md`) is the inverse case; reuse its machinery where it
   fits.
3. **Single-sample view bound where SPIR-V declares MS** (non-NVIDIA risk): find where this happens, make the view match
   the declaration (or the declaration match the view) so a strict driver gets a valid pairing. CPU test.
4. **Lazy-keep `materialize_all` spike** (`docs/waves/MK-PAIR.md` audit): measure from source where the spike happens
   and bound it (spread over frames or cap per call) without dropping registrations.

**Done:** each row ends with a source correction and its test; rows 2 and 4 also with one MKW run
(`py scripts/play.py mkw --env NXGFX_GPU=1`) showing the 337 line gone / the spike bounded, the race sheet looked at,
FPS not lower than 28 (the 30 FPS hold), no new red lines. Fix first, build and run only to measure.

**Exclusions:** no skipping draws or weakened robustness; blacklists stay; `POOL-ORDER.patch` stays shelved; native NDVE
off; MKW only for game runs; no commits.

**Machine / tree.** Builds and games may overlap at full parallelism; run on GPU 1. Another agent (FRESH-SAVE) runs
Arceus/Jamboree routes (no engine edits). The tree carries accepted uncommitted work from earlier waves (MKW-PERF3/4,
SONIC-SMASH, CLEANUP-S2R, RENHDRUM): leave those hunks alone. Don't edit `docs/32-REMAINING-WORK-PLAN.md` or
`scripts/scenarios.toml`.

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol §3.
Foreground commands under 10 minutes; never end a turn waiting on a background notification. Engine
`pwsh -NoProfile -File scripts/build/build.ps1 -SkipGnu -KeepCache`, then
`dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build`. Tests `cargo test -p graphics --lib`.
A device loss or hang stops that path: fix it, never re-run unchanged.

**Deadline:** 2 hours wall clock; park a row after ~90 min with evidence; no early hand-off.

## Report (agent, protocol §5)

### Built

**Row 1, per-stage descriptor limits (`src/graphics/src/vk/layout.rs`; `vk/queue.rs` `DB_CHUNK` made `pub(crate)`).**
Diagnosis: `Layout::new` sized the pool set as half of `maxPerStageDescriptorSampledImages` / `Samplers` and checked
nothing else. It did not count the five stage sets every graphics stage sees (160 UBO, 160 SSBO, 320 samplers, 400
sampled images, 160 storage images, 880 resources), the pool set's storage images and texel buffers, resources, the
per-layout `maxDescriptorSet*` totals, or the descriptor-buffer limits. Correction: `fit` runs before any layout is
made. It counts the stage sets from `stage_binding_type`, so the count follows the layout, and adds the pool set
(`pool_set_count`) and the fragment stage's colour attachments. It counts against the limits the spec applies: with
descriptor buffers no set is update-after-bind, so everything counts against the plain limits; without them the stage
sets count against the plain limits and all sets against the `UpdateAfterBind` ones, read from
`PhysicalDeviceVulkan12Properties`. It also checks `maxBoundDescriptorSets` >= 7 and `maxPushConstantsSize` >= 128.
The pool set is sized from the room the stage sets leave: the sampled, storage, resource and per-set limits each
bound `tex_cap`, and `NXGFX_TEX_CAP`/`NXGFX_SAMP_CAP` still lower it. If even the floor (256 / 64) does not fit, it
returns an error that names every broken limit with needed and allowed counts. `Gpu::create` turns that into the red
line `no Vulkan device: the shared pipeline layout ... exceeds this device's descriptor limits: ...` and has no
device, so there is no undefined behaviour. With descriptor buffers, `db_fit` checks 2 bound buffers against the three
binding limits, the pool set's buffer and a stage-set chunk against both buffer ranges, and both together against the
three address spaces. On NVIDIA nothing changes: 262144 textures / 16384 samplers, same log line.

**Row 2, MKW texture ID 337 (`vk/image.rs`, `vk/exec.rs` `cube_twin_target`).** Diagnosis: ID 337 is ser193, a
64x64x64 BC4 3D texture with 7 levels, NVN target 2, read by a 2D instruction. The 3D descriptor decides the fetch,
as in the cube and one-slice cases (CUBE-TARGET, SONIC-SMASH). The 2D instruction supplies no r coordinate, so the
read is r = 0, the first slice at every level. A Vulkan 3D image takes no 2D view without
`VK_EXT_image_2d_view_of_3d` and an image created for it, so HEAD bound the zero stand-in. Correction: the twin
machinery gains a kind (`TwinKind::{Cube, Volume, Slice}`; the map was keyed by a bool). `Slice` is a 2D image of the
source's width and height. `twin_levels` caps its levels at what a 2D image of that size can hold. It holds slice
z = 0 of each level, copied 3D -> 2D with the same version/once-per-submission rule as the other twins, and is
forgotten with the texture. `cube_twin_target` sends `(T3D, T2D | T2D_ARRAY)` to it.

**Row 4, lazy-keep `materialize_all` spike (`vk/image.rs` `lazy`, `Gpu::materialize_all`; `docs/env_switches.tsv`
row).** Measured from source and existing logs: the first draw of a shader that indexes the texture pool
(`prepare_pool_images`) took every waiting registration at once (`take_all`). Super Mario Party Jamboree:
`build/play/20261125-045848-jamboree-fresh2/run.log` has 3173 images made at 5.11 s, then `slow frame 178: 745 ms:
replay 700.8 ms ... uploads 418.13 ms ... pool preparation 1.0 passes / 2653.0 images`. MKW keeps ~37,800 waiting,
7.2 GB, and would take them all in one frame if such a shader appeared. Correction: `lazy::take_for_frame(frame)` takes
at most a frame's budget: 256 images and 32 MB of guest bytes by default (`NXGFX_LAZY_ALL_FRAME=<n>`; `0` keeps the
old all-at-once behaviour for A/B). Live textures go first: a finalized one is made only to be released after that
use. The rest keep waiting and are not dropped. Later frames' pool-indexing draws, or their own first bound use
(`materialize(id)`, unchanged), make them. `take` and `pick` share `remove`, so the by-serial lists and finalized marks
stay consistent.

**Row 3, single-sample views where SPIR-V declares MS: parked, no source change.** Where it happens:
`view_compatible` (`vk/exec.rs`) accepts `(T2D | T2D_MS) -> T2D_MS` and `(T2D_ARRAY | T2D_MS_ARRAY) -> T2D_MS_ARRAY`.
The CUBE-TARGET rule `(T2D | T2D_MS) -> T2D_MS_ARRAY` covers MKW's ~414 Ampere `TLD.MS` fetches of plain 2D textures.
Each binds a view of a single-sample image to an `OpTypeImage ... MS=1` variable. Neither side can be matched at bind
time with what exists:
- The view side needs a multisampled twin. `vkCmdCopyImage` needs equal sample counts and blits need single-sample
  images, so a twin needs a draw or a storage-multisample compute pass per format. Block-compressed formats, which many
  of these textures use, cannot be multisampled at all.
- The declaration side is fixed when the module loads (offline-translated SPIR-V; modules and pipelines are per program
  key), but whether the bound texture is single-sample is known only at the draw. yuzu reads the TIC when it compiles a
  shader and keys its pipelines on it.

The fix this needs: a module variant per "MS-declared binding k is single-sample" mask, made by a `spvfix` pass that
rewrites those image types to MS=0 and their `Sample` operands to `Lod 0` (the console ignores the sample index on a
1x TIC). The pipeline key would carry the mask, taken from the bound images at the draw. Alternatively, an aliased
non-MS variable at the same binding with a push-constant mask and a structured branch around each fetch. Both touch
module loading, the pipeline key and the parts library, too much to finish and verify in this wave's remaining time.

### Tests
- `vk::layout::tests::five_stage_sets_count_as_the_limits_do`: one graphics stage sees 160 / 160 / 320 / 400 / 160
  / 880.
- `nvidia_fits_the_whole_pool_set`: the RTX 5090's limits (vulkaninfo on this machine) keep 262144 / 16384, with
  and without descriptor buffers.
- `the_pool_set_is_sized_from_the_limits_the_stage_sets_leave`: with a tighter set (524288 per stage, 1,000,000
  resources), `tex_cap` = (1,000,000 - 888) / 4 and the totals fit. The old rule gave 262144: 1,049,464 resources,
  over the limit. An explicit lower cap stays.
- `a_strict_device_is_refused_with_the_limits_named`: an older-Intel-like plain set (64 samplers, 16 storage images,
  200 resources) is refused naming `maxPerStageDescriptorStorageImages: needs 160, the device allows 16`, `Samplers`
  and `Resources`, without and with descriptor buffers. `maxBoundDescriptorSets` 4 is refused. Before: no check at all,
  so these could not fail.
- `descriptor_buffer_limits_are_checked`: NVIDIA's descriptor-buffer limits pass. A set with 1 sampler binding, an
  8 MB sampler range and a 12 MB sampler address space is refused naming those three and no resource limit.
- `vk::image::slice_twin_tests::a_3d_image_sampled_as_2d_gets_a_slice_twin` (`TwinKind::of`, `slice_twin_fits`)
  and the new assertions in `vk::exec::cube_twin_target_tests` (`T3D -> T2D / T2D_ARRAY` twin, `T3D -> T2D_MS` not).
  The `T3D -> T2D` assertion fails on HEAD's `cube_twin_target`.
- `vk::image::lazy::tests::pool_indexing_makes_waiting_images_within_a_frame_budget`: 1000 waiting (10 finalized)
  -> 256 taken, all live; 744 still wait; a 1 MB byte cap gives 16; a tiny cap still gives 1; every registration is
  taken eventually and the serial/finalized bookkeeping empties; the budget is spent within a frame and renews on the
  next. HEAD's `take_all` took all 1000.
- `cargo test -p graphics --lib`: 200 passed, 0 failed, 5 ignored (final tree). `py scripts/env_switches.py --check`:
  288 switches read and 288 rows, so the new row is in. Its 16 "no meaning" problems were there before and are not from
  this wave.

### Verified
- Build: `pwsh -NoProfile -File scripts/build/build.ps1 -SkipGnu -KeepCache` (ok), then
  `dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build` (ok, game rebuilt 05:10).
- Run: `py scripts/play.py mkw --env NXGFX_GPU=1` -> `build/play/20261125-051113-mkw`. Completed 43/43 steps,
  normal exit. FPS in [250, 325] s: 29.4 mean, 5% low 26.0, min 25.0. HEAD's `20261125-043019-mkw-perf4-final` had
  28.7 / 24.3 / 20.2. This was not a machine-to-itself perf run: other agents' work may have overlapped. Memory peak
  16536 MB (HEAD 16845).
- Red lines: **1** (the known `pipeline-crashed.bin` line). HEAD had 2: the `texture ID 337: target 2 bound where the
  shader samples target 1` line is gone, as are its 40 `target mismatch` lines and its stand-in line. In their place:
  `[71.42] texture ser193 (64x64x64 BC4_UNORM_BLOCK, 7 levels, target 2) sampled as a 2D texture: a 2D twin of its
  first slice`. No validation or device errors.
- Start-up: `shared pipeline layout: 6 stage sets of 176 bindings, pool set of 262144 textures / 16384 samplers;
  descriptor buffers: a stage set 1600 bytes, the pool set 10304 KB`, the same as HEAD, so the check passes on the
  RTX 5090 and the sizes are unchanged.
- Row 4: no `lazy images:` line, as at HEAD (see Unproven). Zero-stand-in lines: 21 (HEAD 31). They are the existing
  `released, its image parked` kind at ~71 s, the same IDs (30786...) as at HEAD.
- Sheet `build/play/20261125-051113-mkw/sheet-1.png`, read by me: boot drive, title, race countdown "3", grid with
  the golden mushroom, racing lap 1/3 with coins 9, then lap 2/3 with coins 12 (24th). Nothing black or missing in
  the shots.

### Unproven
- Row 3 (above): parked with the diagnosis and the two possible designs. NVIDIA still accepts the pairing, as before.
- Row 2's fetch semantics come from the descriptor-decides rule and r = 0, not from a hardware capture. If the console
  filters at r = 0 with wrap addressing, it blends slices 63 and 0, while the twin reads slice 0 only. The texture is
  noise, so the difference cannot be seen in a shot.
- Row 4 in MKW: no MKW shader indexes the texture pool, so `materialize_all` never runs there. Neither the HEAD run
  (`20261125-043019-mkw-perf4-final`) nor this one has a `lazy images:` line. The MKW run shows no regression. The
  bound itself is shown by the CPU test and by the Jamboree measurement above (Jamboree is FRESH-SAVE's title, and
  this wave runs MKW only). Next step: one Jamboree run, expecting the 745 ms frame at ~5.1 s to become ~13 frames of
  at most 256 images each.
- Row 1 on a non-NVIDIA device: only the CPU tests and the NVIDIA start-up line. The Intel iGPU on this machine
  (33,554,432 per stage) would fit.

### Slips
- I edited three lines of the row 1 test helpers in `layout.rs` with a `sed` command instead of the editor tool
  (`all(...)` -> `set_of(...)`). The change is in the reviewed diff. I made no other scripted edits.
- `docs/env_switches.tsv` was not in the brief's file list; it gained one row for the new switch.

### Questions
None.
