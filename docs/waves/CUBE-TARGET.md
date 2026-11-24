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

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with what is verified working now.)_
