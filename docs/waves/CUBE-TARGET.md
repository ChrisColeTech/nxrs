# CUBE-TARGET — 2D textures bound where shaders sample cubes (Pokémon Z-A, Mario Kart World)

Owner: one delegated agent. Lead: main session. Opened 2026-11-24. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Goal / done:**
1. **Z-A on this engine:** rebuild Pokémon Legends Z-A (`dist\nxrs\nxrs.exe --select-title "Pokemon Legends Z-A"
   --build`), run `py scripts/play.py plza`, and get the scenario to controllable free movement on a fresh save
   (adjust `scripts/scenarios.toml [plza]`; earlier evidence: continuous A edges advance arrival, starter
   selection and battle tutorials to the Battle Zone with the free-movement HUD). Sheet looked at.
2. **The target mismatch, fixed at its cause:** `texture ID N: target 1 bound where the shader samples target 6`
   (`src/graphics/src/vk/exec.rs` ~2934, which then binds a zero stand-in view, so those samples read black).
   MKW race runs log it for ~415 texture IDs (`build/play/20261124-175216-mkw-audit-fix/run.log`); Z-A had 14 live
   2D->cube mismatches (`docs/34-LIBRARY-VALIDATION.md` PLZA-B02: decode the guest target, compare pool generation,
   inspect the SPIR-V image type, trace alias provenance). Find why the registered texture is 2D (target 1) while
   the shader samples a cube (target 6): the guest's texture/view descriptor (NVN texture views can retarget a
   2D-array image as a cube), the texture-pool decode, image creation flags (Vulkan needs CUBE_COMPATIBLE on the
   image to make a cube view of 2D-array layers), or the shader's declared type. Correct it so the shader gets the
   right view. Done = both games' runs show 0 of these lines (or each remaining one explained with evidence), a CPU
   test of the correction, and before/after screenshots compared where the affected textures appear (reflections,
   lighting, sky). Note: a "generic cube widening" was tried for Z-A earlier and disproven/reverted — do not repeat
   it blind; follow NVN's actual view/target semantics. References (Turbo, mkw-recomp, nxnative) are evidence.
3. **Z-A pacing** if time remains: earlier 57.7 FPS against 60; report the window FPS before/after.

**Baseline:** branch `gameplay-harness`, HEAD `5eaf8f3` (+ this file), clean tree. `dist/nxrs` = HEAD engine, MKW
rebuilt with it; Z-A not yet rebuilt. Ledger: `docs/32-REMAINING-WORK-PLAN.md` current section.

**Exclusions:** no skipping draws, blacklisting or weakening robustness; native NDVE stays off; no titles other
than Z-A and MKW; no commits (lead commits at acceptance).

**Rules / commands:** `AGENTS.md` (editor tools only; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by PID), protocol
§3 (one build or game at a time, never a build while a game runs; `--jobs 4`). Engine: `pwsh -NoProfile -File
scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (background, ~4 min); rebuild each title after an engine build.
Run: `py scripts/play.py plza|mkw [--env K=V] [--seconds N] [--diag render]`; read `report.md`, look at every
`sheet-*.png`. Tests: `cargo test --jobs 4 -p graphics --lib` (and `-p shaders` if touched). A device loss stops
that path: fix it, never re-run it unchanged.

**Deadline:** 2 hours wall clock. Park a blocker after ~90 min and continue with the next item; no early hand-off.

**Files expected:** `src/graphics/src/vk/{exec,image,tables}.rs`, `src/graphics/src/tex/*`, possibly
`src/shaders/src/*`; `scripts/scenarios.toml`; this file for the report.

## Report (agent, protocol §5)

**Verified working now:** Z-A goes from a fresh save to controllable free movement (the station platform with the
"Move / Control Camera" HUD, then walking into the station hall) at 59.6 FPS; its 2D-as-cube lines go from 34 to 0. MKW's
race run goes from 414 `target 1 ... target 6` lines to 0 and races normally (15.6 FPS vs 14.0 before). One unrelated
MKW line is left (3D sampled as 2D, below).

**The brief's premise was wrong.** NVN target 6 is `TEXTURE_2D_MULTISAMPLE_ARRAY`, not a cube. The cube is 8
(`nvn/types.rs`, the same numbering as mkw-recomp's `nvn_tex.c`). So MKW and Z-A had two separate causes.

### Built
- **MKW (target 1 where the shader declares 2D multisample array).** The Ampere decoder emits `TLD.MS` with a 2D-array
  dimension (`InstEmitAmpereMem.cs` bit 78), so reflection declares `2D | Array | Multisample`. The game binds a plain
  2D texture there; on the console the texture descriptor (TIC), not the instruction, decides the fetch. The view
  check had no rule for 2D to 2D-MS-array, so a zero stand-in was bound. Fix: `view_compatible` (pulled out of
  `tex_view_uncached`, `vk/exec.rs`) accepts `(T2D | T2D_MS) -> T2D_MS_ARRAY` and gives a one-layer `TYPE_2D_ARRAY`
  view of the 2D image. This is the array form of the existing `(T2D, T2D_MS)` rule.
- **Z-A (target 1 where the shader samples a cube, 8).** Every mismatch was one texture: a 32x32 BC7 texture with 6
  levels and 1 layer (ser25641, later ser25635/ser29642). The game registers it into about 11 IDs whose shaders sample
  cubes, with a registered view of target 1. It is not a 2D array, so CUBE_COMPATIBLE and "generic cube widening" do
  not apply. Fix (`vk/image.rs` `cube_twin_view`): a cube-compatible six-layer twin image gets every level of the
  texture's one layer copied into each face, in the submission's `pre` buffer. Following the same "descriptor decides"
  semantics, a 2D descriptor has one layer and every face reads it. The copy runs again when the source's
  `(rewrites, gpu_seq)` changes, or once per submission for a GPU-written source. The twin is freed when the texture
  is released (`tex_release_now`). `NXGFX_CUBE_TWIN=0` turns it off for A/B runs. Other 2D-to-cube cases (not square,
  layered, depth, storage) keep the logged stand-in.
- Diagnostics: the mismatch line now also logs (at most 40 times) the registered and live serial, NVN descriptor,
  Vulkan image and view.
- `scripts/scenarios.toml [plza]`: on a fresh save, A through the opening. On the appearance grid A alone selects
  nothing, so LRight first, then A. Confirm Choices opens on "Change hair color", so Down x2 to Ready!, then A through
  the train arrival to the platform, and walk. The bubbles on the platform are ambient: 100 A presses do not clear
  them, which explains the earlier "station dialogue stall". New `[plza-explore]` (boot and wait) for driving by hand.

### Tests
`cargo test --jobs 4 -p graphics --lib`: 181 passed, 0 failed. New tests:
`a_2d_texture_takes_a_one_layer_view_for_a_multisample_array_fetch` (would fail before: there was no such rule) and
`a_square_one_layer_2d_colour_image_gets_a_cube_twin` (shape gate). The twin copy itself has no CPU test.

### Verified (sheets looked at)
- MKW before: `build/play/20261124-175216-mkw-audit-fix` (414 lines of 1->6, 14.0 FPS). After:
  `build/play/20261124-182126-mkw-cube` (0 lines of 1->6, 15.6 FPS, race driven, no device loss). The grid and race
  shots match in image quality. I could not tie a visible element to the fixed fetch: the lines start in the opening
  (~102 s), and the shots are not of matching moments.
- Z-A after: `build/play/20261124-182844-plza-final`, a full scenario run. Its log has 0 mismatch lines and 2 twins
  made. Window FPS 59.6, 5% low 57.9, against the earlier 57.7 FPS.
- Z-A before (same build, `NXGFX_CUBE_TWIN=0`): `build/play/20261124-183244-plza-twin-off` (34 lines of 1->8, 59.7 FPS).
  The scenes match frame for frame. The full-resolution `08-walk_again` shots are indistinguishable, so the fallback
  cube is not visibly significant in these scenes.
- Route finding: `build/play/20261124-181155-plza-explore`. Old scenario stuck at Appearance: `20261124-180105-plza-base`.

### Unproven
- MKW still has 1 kind of line: `texture ID 337: target 2 bound where the shader samples target 1`. This is a 64x64x64
  3D noise texture (ser194, fmt 0x49, swizzle RRRR) sampled by a 2D instruction. What the hardware fetches for a 2D
  instruction on a 3D TIC is not established. Next step: a 2D view of slice 0 (`VK_EXT_image_2d_view_of_3d`) only once
  the semantics are confirmed against a reference.
- The twin copy is recorded in `pre`, so a source the GPU writes in the same submission is read one submission late.
  Z-A's source is a static BC7 texture.
- Z-A reached free movement on the platform and in the station hall. I did not drive it to the Battle Zone.
- I added no pacing work: Z-A is at 59.6 to 59.7 FPS.

### Slips
- I killed three of my own Z-A runs by PID on stuck scenarios: two at Appearance or Hair Color, and the exploration
  session. Two superseded run folders were deleted.
- `[plza]` picks a different look each run (the stick timing varies), so the before/after character shots differ.

### Questions
None.

## Lead review (2026-11-24 18:45)

Accepted. Lead re-ran graphics lib (181 passed) and harness tests; read `20261124-182844-plza-final/sheet-1.png`
(appearance, confirm, train arrival, platform with the Move HUD, walking into the hall; 166/166 steps, 59.6 FPS).
The brief's "target 6 = cube" was wrong (6 = 2D multisample array; cube = 8); the agent found two causes and fixed
each at its source. Recorded risk: the MKW rule binds a single-sample 2D-array view where the SPIR-V declares a
multisampled image (as the existing 2D -> 2D_MS rule already does); NVIDIA handles it, but strict Vulkan calls the
sample-count mismatch undefined, so non-NVIDIA drivers need checking (or a translator-side fetch from a
non-MS image when the TIC is single-sample). Open: MKW `texture ID 337` (3D noise sampled by a 2D instruction).
