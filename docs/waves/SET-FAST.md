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
