# 33 - Reference-delta outcome roadmap

**Status: active roadmap, 2026-11-21.** Baseline HEAD is
`7688e33ef5ef47e64322c7aed0f84c471e9c8b99`; the extensive dirty worktree is part of the baseline and must be
preserved. This document supersedes `docs/32-REMAINING-WORK-PLAN.md` as the product/outcome roadmap. Document 32 is
retained as procedural history and the execution ledger. The safety, lease, evidence and review rules in
`docs/31-DELEGATION-PROTOCOL.md` remain binding. A diagnostic, audit, build or test is a subtask: a milestone closes
only with a source correction and a guarded game result.

## Executive diagnosis

Prior work correctly improved observability, safety and several renderer invariants, but then treated accepted
diagnostic waves as forward milestones. DG-01 did not fix MKW, DG-02 did not fix DKB, and DS-01 did not replace OoT's
shader. Meanwhile the source tree itself says the shipped `dmsh_xlate` predates the current MIT nxnative reference,
and `ampere_orin.rs` already enumerates 35 operations that differ or refuse. Reference deltas were therefore left
behind while captures became more detailed.

The new ordering is source first and game outcome second. Existing evidence answers the next DKB question: frame
5716 pass 18 has fully non-clear D32 depth, while pass 27 leaves its first two colour targets at exact constants.
No more geometry/depth discriminator is justified before correcting or disproving the material-output path. MKW's
native mesh path is not the only reference design: current mkw-recomp has a submission-owned mesh-as-compute fallback
that avoids driver mesh execution and bounds its scratch lifetime. OoT's nxnative replacement is a complete
recognition, graph, resource and shadow-comparison state machine; nxrs currently has arithmetic kernels and a warning,
not an integration.

References remain fallible. A change needs agreement between current source, retained evidence and at least one
reference or a focused invariant test. `PORT` means MIT source may be adapted; `REIMPLEMENT` means behavior is used
without copying incompatible or unpinned code; `TEST/REFERENCE` means no production logic is copied.

## References and licensing

| Reference | Absolute root and version actually read | License / allowed use | Limit |
|---|---|---|---|
| nxrs | `D:\Projects\RecompLauncherEngine`, HEAD `7688e33ef5ef47e64322c7aed0f84c471e9c8b99` | Project source | Dirty working tree, not a reproducible release identity |
| nxnative current | `D:\Projects\RecompLauncherEngine\build\Q\nxnative-up`, branch `main`, `7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f` | MIT; **PORT** or reimplement | Apple/Metal behavior is evidence, not automatic Vulkan correctness |
| nxnative older | `D:\Projects\nxnative`, branch `main`, `e4b0d9b75d39c41c10fdcfec2f0b070df496d7e5` | MIT; **PORT** or reimplement | 131 commits older than `7ebb76e`; spot/reference use only |
| Turbo | `D:\Projects\Switch2RECOMP-main`, branch `turbo`, `11e966f33b1f2aec8b05d0764f795804e807510c` | Root AGPL-3.0; **TEST/REFERENCE** unless local ownership of a specific hunk is established | Behavior evidence only by default |
| mkw-recomp | `D:\Projects\mkw-recomp-main`; root `LICENSE` is MIT | MIT in the located export; **TEST/REFERENCE** until provenance is pinned, then eligible donor code may be **PORT** | The directory has no `.git`; exact commit/branch is **unknown**. Do not claim a commit or copy a hunk until provenance is supplied |
| Eglantine | Recorded in `DECISIONS.md` as `D:\Projects\Eglantine-nx2`, `cf6cfe9caa`, 0.1.0-rc4 | GPL-3.0; **TEST/REFERENCE** only | Not reopened in this research pass |

`DECISIONS.md` sections 1-10 remain **Awaiting owner** proposals. They are not approved rules. Existing approved
rules used here are Vulkan-only nxrs, real I/O, global player settings where already decided, reference-license
boundaries, and the protocol's fault-stop behavior.

## Actual library inventory

This is the inventory represented by repository build directories, scripts, scenarios and campaign evidence. It is
not the owner's launcher inventory; `%LOCALAPPDATA%\nxrs` was not accessed. "Installed artifact" means a named game
build directory exists under `build/games`, not that ownership, source media or launcher installation was inspected.

| # | Title | Repository representation | Evidence state and current acceptance |
|---:|---|---|---|
| 1 | Donkey Kong Bananza (DKB) | `build/games/dkb`; scenario `dkb` | **Current, hard blocker.** DC-02 exit 0 but black world, 20.068 FPS, 91 unique red diagnostics; no controllable visible gameplay |
| 2 | Mario Kart World (MKW) | `build/games/mkw`; `mkw-race` | **Current, hard blocker.** reset at direct mesh task `6098504f5d12ea03` / mesh `1de24686294997d8`; prior haze and 6.551 FPS are separate open defects |
| 3 | Ocarina of Time (OoT) | `build/games/oot`; `oot-default-boot` | **Current, hard blocker.** first present, then `vkCreateComputePipelines` does not return for `d15370a382630d4e`; no gameplay |
| 4 | Mario Kart 8 Deluxe (MK8D) | `build/games/mk8d`, `mk8dw`, `mk8d-O` | **Current, correction verified but not clean.** cyan shadow fixed, 59.52 FPS; 6 resource issues and 149 missing-shader entries remain |
| 5 | Pokemon Legends: Arceus | `build/games/arceus` | **Current, correction verified but not clean.** bridge overexposure fixed, native 30 FPS; six missing-shader diagnostics remain |
| 6 | Pokemon Legends: Z-A | `build/games/plza` | **Current, correction verified but not clean.** controllable station at 59.807 FPS; bright doorway and 14 red issues remain |
| 7 | Shadow Generations | `build/games/sonic`; `shadow_skip` | **Current, correction verified but not clean.** readable movement at 29.99 FPS; 219 red lines / 5 normalized issues and incomplete window |
| 8 | Super Mario Party Jamboree | `build/games/jamboree` | **Current, playable but not clean.** controllable plaza at 59.671 FPS; 36 red diagnostics; no board/minigame result |
| 9 | Super Smash Bros. Ultimate | `build/games/smash` | **Current, playable but not clean.** 56.105 FPS, 39 stutters; comparable pacing and long memory evidence owed |
| 10 | Pokemon Violet | `build/games/violet` | **Historical-only.** controllable house at 30 FPS on 2026-11-21 early campaign, but 21 red issues and short loading-memory slope; no final-candidate pass |
| 11 | Super Mario Bros. Wonder | `build/games/wonder` | **Historical-only.** 59.68 FPS, 6 stutters, 1 gameplay underrun; two blacklists and incomplete/loading-heavy memory window |
| 12 | Anime Girls: Sun of a Beach | `build/games/agsob` | **Historical-only.** gameplay near 60 FPS; colour banding unresolved and no trustworthy matched reference |
| 13 | Sonic Generations | `build/games/sonic` program 1; `sonic_gens2` | **Current, playable.** production successor tracked into readable Green Hill gameplay at about 31.5 FPS; no program-1 red diagnostic or device loss |
| 14 | Beach Bounce Remastered | `build/games/bbounce`; scenario `vn` | **Historical-only.** 900 s exit 0, 59.60 FPS; capture-induced stutters mean no clean pacing baseline |

**Count: 14 titles, all with repository build artifacts; 9 current-evidence states and 5 historical-only states;
0 clean current whole-title or whole-library passes.** There is no additional title in current scenario files or
game build directories. Loose logs such as `P-eng.log` are engine/build logs, not another represented game install.

## Reference-delta matrix

Confidence: **H** direct source/evidence agreement, **M** concrete source difference with incomplete causal proof,
**L** plausible shared path without matching title evidence.

| Area / symptom | Exact nxrs path | Exact reference path | Concrete delta and evidence | Conf. | Action / order | Game verification |
|---|---|---|---|:---:|---|---|
| Shared Ampere translation: 35 operations refuse/differ | `src/graphics/tests/ampere_orin.rs:62-183`; translator selected by `src/shaders/src/xlate.rs:48-113`; build defaults to old tree in `src/shaders/tools/build_dmsh_xlate.ps1:9-20` | nxnative `dm/third_party/ryujinx/.../InstEmitAmpereAlu.cs:18-105,111-180`; `InstEmitAluHelper.cs:38-132`; `docs/reference/shaders.md:1771-1800,2124-2128` | nxrs's own test says its binary predates current HFMA2.MMA/opcode `0x435`, DP2A/DP4A and directed FADD/FMUL/FFMA. Build script defaults to older `D:\Projects\nxnative`, not pinned current clone. This is an explicit upstream delta and pre-fix oracle. | **H** | **PORT 1.** Pin/rebuild from `7ebb76e`; retain Vulkan patch; remove `HERE` entries only when they match Orin. | Rebuild affected shader libraries, then short DKB/MKW/Z-A/Shadow route; no new missing/refused keys, no picture regression |
| DKB pass 27 writes constants despite valid depth | `vk/exec.rs:2006-2175` attachments, `4420-4585` depth/write masks, `3001-3618` descriptors; `vk/image.rs:1041,1552-1637`; `vk/gmem.rs:1-18,120-218`; `vk/present.rs:443-553`, `vk/upscale.rs:193-260` | nxnative `nvn_mtl.m:6134-6305` attachment/view handling, `3534` push constants; `dmsh_abi.h:154-165` gmem; `nvn_owner.*` aliases; Turbo behavior only | DC-02 pass 18 depth is finite `0.810413..0.943018`, none at clear 1.0. Pass 27 uses VS `327992a9765e5b2b`, FS `3d98baee7872e4b7`, depth compare 3, write 0, channel mask all, output map `0x6f`; slot 0 stays RGB zero, slot 1 stays `(0,0,1,0)`, slot 2 is nonzero `0.000427..0.001652`; depth is unchanged. Geometry, raw pointer join and final upscale are downstream/previously evidenced. Highest candidates are output-map-to-attachment/component mapping, fragment translation, and depth compare semantics; not another capture. | **M-H boundary**, fix candidate **M** | **REIMPLEMENT 2.** First retranslate/diff the two shader keys after Ampere sync. Then correct output-location/component mapping or compare mapping only if SPIR-V/reference proves mismatch. Validate `omap 0x6f` maps output 0 RGBA and output 1 RG, not raw nibble masks applied to physical slots incorrectly. | Same saved route: pass-27 slot 0 gains finite nonzero HDR coverage, visible world appears, controls change scene; then uninstrumented short gameplay with no major graphics error |
| DKB image/view/alias and composition | `vk/image.rs:273-292,498-581,1041,1552-1637`; `vk/render_chain.rs`; `vk/present.rs:469-553`; `vk/upscale.rs` | nxnative `nvn_mtl.m:4990-5007,6134-6213`; `nvn_owner.h:15-17,82-93`; `hle_nvn.c:3471-3487` | nxrs now carries audited physical creation provenance and explicit view swizzle replacement. DC-02 actual view metadata matches backing formats and shows failure before final composition. Do not prioritize upscale/compose unless corrected pass-27 colour later disappears. | **H negative** | **TEST/REFERENCE**, order after material output | Check raw scene target and final screenshot in same corrected run; no speculative composition rewrite |
| DKB raw global memory | `vk/gmem.rs:1-218`; binding at `vk/exec.rs:3200-3222` | nxnative `dmsh_abi.h:154-165`, `nvn_mtl.m:978-1012,1658,2756-2762`; Turbo only behavior evidence | Current table is 8,192 ranges with hints/log and forced whole-pool visibility. Earlier pointer probe joined all 1,482 pointers and 16 consumers. This path is exercised but is no longer the leading pass-27 cause. Large-pool write-back over 64 MiB remains an explicit shared limitation. | **M shared**, **L DKB cause** | **REIMPLEMENT** later: range-granular write-back instead of refusing a whole >64 MiB pool when a title proves CPU consumption | DKB/MKW no raw-gmem red line and correct scene; CPU-visible write invariant test |
| MKW reset at direct mesh draw | `vk/exec.rs:1019-1439`; descriptor construction `3001-3618`; `vk/tables.rs:1-29,190-240`; lifetime `vk/queue.rs:792-933` | mkw `runtime/gfx/vk/exec/vk_exec_ops.c:63-75,439-513,608-618`; `vk_mesh_cs.c:1-123`; `sm86/.../head.py:153-294`; `mem.py:134-267` | nxrs issues native `vkCmdDrawMeshTasksEXT` for direct draws and stages a 16-byte `{count,first}` record. mkw can run task and mesh as compute into submission-fenced device-local scratch, compact requested groups, then indexed draw; its native path ignores nonzero `first` just as nxrs can. The retained fault address is recycled staging/pool history, but robustness2 cannot protect physical-address/global-memory loads. Exact faulting instruction remains unknown. | **M** | **REIMPLEMENT 3**, not copy until mkw commit pinned. Add a generic task/mesh-as-compute backend using nxrs translator/reflection; select it as a guarded NVIDIA discriminator only after CPU/offline parity. Before that, audit task/mesh raw-global loads and descriptor ranges for these two keys against mkw reflection. | 35 s same workload, no reset/invalid read and identical draw count/picture relative to native path; only then extend to title/race |
| MKW task payload semantics | `vk/exec.rs:1336-1371`; `src/shaders/src/job.rs:284-289`; generated task/mesh SPIR-V | mkw `sm86/.../thread.py:185-232`; `head.py:153-209` | mkw payload is 4 KiB / 1,024 words, initialized task count, barriers before `EmitMeshTasksEXT`, zeroed mesh shared outputs. nxrs supplies draw-record data separately; retained fault shaders must be checked for equal ISBE offsets, workgroup index/`first`, output initialization and limits. | **M** | **TEST/REFERENCE** first; **REIMPLEMENT** any demonstrated semantic difference | Offline reflection/SPIR-V fixture for both keys, then guarded MKW safety run |
| MKW descriptor range / global search | `vk/exec.rs:3200-3292`; `vk/device.rs:886-921`; `vk/gmem.rs`; `vk/tables.rs` | mkw `sm86/.../mem.py:134-267`; `runtime/gfx/vk/exec/vk_exec_bind.c:60-201,551`; `vk_guard.c` | mkw traced LDG/STG uses the exact SSBO range; lost tracking searches nearest lower bound only if `relative < descriptor length`. nxrs raw-gmem uses physical pointers and a pool table; robust descriptor sizes are correct, but a stale table delta or too-wide pool range can still expose recycled memory. Validate generation snapshot at draw issue, not just descriptor bytes. | **M** | **REIMPLEMENT** immutable gmem table/range generation per submission if audit finds mutation/reuse; otherwise mesh-as-compute first | CPU generation/lifetime regression plus no fault in bounded MKW workload |
| MKW haze | `vk/image.rs`, `vk/shader.rs`, `vk/spvfix.rs`, translated fragment shaders | mkw current final image behavior; nxnative shader/texture paths | Separate from safety. Near-white clipping with silhouettes likely shader arithmetic/texture view or colour-space, and the Ampere sync is a prerequisite. Do not tune fog/colour globally. | **M-L** | **PORT Ampere first**, then key-specific shader/image correction | Matched race/menu image readable with preserved UI and no clipping |
| MKW 6-7 FPS | `vk/mem.rs`, `vk/exec.rs`, `vk/queue.rs`, `vk/pipe.rs` | mkw `vk_mesh_cs.c:1-6`, memory/upload/parallel replay paths | Separate from safety and haze. Existing 65-79 ms pool-sync history and 3,144 skipped draws dominate; current source has write-watch fix but no post-fix clean measurement. Profile only after stable rendering. | **L until safe** | Source correction must follow measured owner, likely pool upload granularity or mesh path; no performance-only milestone | Stable readable race first, then >= native-rate target with no unexplained skips |
| OoT translated NDVE pipeline hang | `vk/exec.rs:5048-5110`; `vk/pipe.rs:854-863`; `vk/newndve.rs`; `vk/newndve/kernels.rs` | nxnative `nvn_ndve.inc:1-65,67-180,206-255`; `dm/gfx/msl/ndve_engine.metal` | nxrs only recognizes keys and warns before compiling; kernels use a synthetic all-storage-buffer ABI and cannot consume guest dispatches. nxnative recognizes the 14-key frame sequence, 12 replaced dispatches, weights/bias offsets, textures/formats/constants, allocates zero-bordered NHWC tensors, synchronizes 13 kernels, replaces network/resampler, and has shadow compare. | **H** | **PORT 4** graph/recognition from MIT; **REIMPLEMENT** Vulkan resource/descriptor/sync integration. Keep replacement off. | Synthetic GPU parity, complete guest preflight, shadow output comparison, then OoT first movie/gameplay |
| OoT recognition/prefix policy | `newndve.rs:15-21,92-119,136-155` | `nvn_ndve.inc:63-66,206-255` | nxrs correctly rejects "on" and requires complete immutable graph, avoiding nxnative's unsafe skip-prefix/fail-later behavior. Missing: state capture from actual dispatch descriptors and safe delayed consumption. | **H** | **PORT concepts, REIMPLEMENT fail-closed transaction**: observe full frame sequence first; execute host graph only when all 12 dispatches and final output/resampler bindings preflight; consume atomically, never prefix | CPU sequence tests with repeated keys/out-of-order/reset; no guest dispatch consumed on any miss |
| OoT layout/residency/alias | `newndve.rs:47-79`; `kernels.rs:27-52,117-165` | `nvn_ndve.inc:20-45,80-99,110-180,233-250` | Shapes/offsets agree, but nxrs tensor stride is merely `(W+2)*(H+2)*C`; nxnative allocates `(W+2)*(H+2+16)*C*2`, zeroed once. Guest images are real textures, not storage buffers; residency, alias and output ownership are absent. | **H** | **PORT** exact tensor allocation/args; **REIMPLEMENT** image descriptors, ownership and queue barriers; retain kernels until submission completion | Extent/offset tests; alias refusal; current GPU parity; shadow image exact/declared-tolerance comparison |
| Shared image/view/attachment semantics | `vk/image.rs`, `vk/exec.rs:2006-2175`, `vk/tables.rs` | nxnative `nvn_mtl.m:4990-5007,6134-6453`, `nvn_owner.*`; mkw `vk_exec_view.c` | Major lifecycle, swizzle and provenance fixes are present. Lower-confidence gaps: same attachment in multiple slots, depth-as-colour/block aliases, layered/3D render targets and descriptor snapshot generations. Tie work to diagnostics before changing. | **M-L** | **PORT** MIT semantics where a title hits them; focused regression and affected-title result | MK8D/Z-A/Shadow diagnostics close without regressing fixed scenes |
| Shared descriptor buffers/pools | `vk/tables.rs`, `vk/exec.rs`, `vk/layout.rs`, `vk/device.rs` | mkw `vk_exec_bind.c`; nxnative argument tables | Snapshot generation exists, but mutable pool registration and buffer-address reuse must be immutable for an issued submission. Audit every destruction/re-registration path against `pool_generation`; classic and descriptor-buffer results must agree. | **M** | **REIMPLEMENT/TEST** generation invariant | Both descriptor backends for synthetic test; affected game has no stale/out-of-pool binding |
| Shared synchronization/services | `vk/queue.rs`, `vk/mem.rs`, `src/runtime`, `src/os` | nxnative host/service implementations; Turbo behavior only | No current evidence supports a new blanket sync/service rewrite. Keep known open items: partial-allocation cleanup, crop overflow, real microphone, residual services only when a title logs them. | **L** | **PORT** MIT service deltas or **REIMPLEMENT** AGPL behavior only when an observed title depends on them | Named title progresses and gap disappears; no fake success |

## Per-title bug queue

| Title | Current evidence / symptom | Reference or shared dependency | Next code correction | Short current-build acceptance / done criterion |
|---|---|---|---|---|
| DKB | Pass 18 depth valid; pass 27 HDR/RGBA constant; black world, 20 FPS, 78 missing keys, resource diagnostics | Ampere sync; output mapping, attachment/view, raw gmem | Retranslate keys `327992a9765e5b2b` / `3d98baee7872e4b7`; fix proven output/component or compare semantic delta | Saved Continue/walk/punch: visible lit world and avatar, input changes scene, no loss/major issue; raw scene colour nonconstant; then uninstrumented 100-150 s route |
| MKW | reset at direct mesh pair; prior haze and 6.551 FPS | task/mesh payload, gmem/ranges/lifetime; mkw compute fallback | Safety first: immutable range/gmem correction if proven, otherwise generic mesh-as-compute backend; then shader/image haze; then pool cost | Same 35 s fault workload no reset, then readable controllable race, then native-rate pacing; each is a separate milestone |
| OoT | huge `d153...` compile does not return | NDVE host graph | Implement fail-closed complete-sequence Vulkan host graph and shadow mode; replacement remains off | Current guest path still intact until parity; after gates, first movie/frame and controllable gameplay without fake sync completion |
| MK8D | fixed cyan shadow; 6 resource issues / 149 missing entries | Ampere/shared descriptor/image | Ampere refresh, then classify remaining keys and exact resource ownership | Same race/kart scene at ~60 FPS, cyan fix retained, no major red resource issue/device loss |
| Arceus | fixed overexposure; six startup missing shaders | shared translation | Refresh translator/library; resolve only still-missing keys | Same bridge route at native 30 FPS, correct lighting, no missing/refused shaders or loss |
| Z-A | fixed station shading; bright doorway, 14 red issues | Ampere/image colour | Refresh translator; correlate doorway shader/resource before correction | Controllable station 120-190 s, readable doorway and characters, ~60 FPS, no major diagnostics |
| Shadow Generations | fixed washout; residual 5 normalized issues | Ampere/image lifecycle | Refresh translator; close exact five categories, not broad tint tuning | Existing first-stage run/jump route, readable scene/HUD, native 30 FPS, complete window |
| Jamboree | controllable plaza; 36 red diagnostics | shared shaders/resources/services | Classify 36; correct first repeated source-backed shader/resource issue | Short plaza dialogue + movement, no fade/progression regression, ~60 FPS, no major red issue; board/minigame out of scope unless selected |
| Smash | 56.105 FPS, 39 stutters; memory unknown | shared pipeline/descriptor/lifetime | Refresh shaders; fix any recurring missing/alpha/resource key; measure same stage only after correction | Same stage/opponent 150-240 s controllable match, readable effects, no loss; pacing target and declared long-memory scope pass |
| Violet | historical 30 FPS, 21 red issues | shared shader/service/resource | Refresh shaders; close missing/conservative/target diagnostics reached by house route | Current build: house stairs to living room + interaction, native 30 FPS, readable picture, no major red issue |
| Wonder | historical 59.68 FPS; blacklists, loading trend, underrun | pipeline cache/translation/lifetime | Refresh shaders and remove corrected blacklist entries only through normal successful compile; fix first retained key if still failing | Saved course route through first gameplay, 115-260 s fully observed, ~60 FPS, no gameplay underrun; loading and steady memory reported separately |
| Anime Girls | banding with gameplay near 60 | fragment translation/format; capture shader keys | Refresh translator; compare captured LUT/program outputs with nxnative behavior; fix exact conversion/precision delta, never global dither | Beach shooter route, readable smooth gradients, ~60 FPS, no major diagnostics; matched reference or invariant establishes banding fixed |
| Sonic Generations | production Green Hill gameplay at about 31.5 FPS; successor and colour current | shared title-menu blacklist only | No Sonic-specific correction open; retain program-1 route while program 0 diagnostics are resolved | Program-1 Green Hill spindash route remains tracked, readable, native-rate and free of major diagnostics/loss |
| Beach Bounce | historical long survivor; capture-induced stutters | shared lifecycle/present/audio | Re-run only after shared corrections; code change only for reproduced source defect | Short VN route: advance text, readable image, real non-silent audio, ~60 FPS, no major diagnostics; uncaptured pacing window |

## Ordered outcome roadmap

1. **R1 - Current Ampere translator, one Switch 2 scene.** Port nxnative `7ebb76e` translator changes, pin source
   identity, and reduce `HERE` only from actual oracle matches. End with a source correction plus one guarded affected
   title result (prefer DKB because current raw evidence names its pass-27 keys; if those keys are unchanged, use the
   first title whose previously refused key is reached). An oracle/build alone does not close R1.
2. **R2 - DKB material pass produces a world.** Diff pass-27 shader SPIR-V/reflection after R1, correct the proven
   output-map/component/depth-compare or descriptor/view delta, and end with visible input-responsive saved gameplay.
   Pass 18/27 capture is evidence input, not a new milestone.
3. **R3 - MKW safety.** Correct immutable descriptor/gmem lifetime if source audit proves it; otherwise implement the
   generic mesh-as-compute path from independently written nxrs components. End with the retained fault workload
   surviving without invalid read/reset. A recorder report cannot close R3.
4. **R4 - OoT host NDVE in shadow mode.** Port graph/offset/layout facts and reimplement Vulkan resources,
   synchronization and complete-sequence transaction. End with current GPU synthetic parity and an actual guest-frame
   shadow comparison; replacement remains off on mismatch or absent evidence.
5. **R5 - OoT first controllable gameplay.** Enable replacement only after R4 audit/gates, remove the `d153...`
   translated compile from the consumed sequence without faking completion, and end at first controllable readable
   gameplay/movie output.
6. **R6 - MKW readable race.** With safety established, correct haze from named shaders/views and end with readable
   control. Then correct measured pool/descriptor cost and end at target pacing. Rendering and speed are separate.
7. **R7 - Current Switch 2 regressions.** In small outcomes, reverify and close residual diagnostics for MK8D,
   Arceus, Z-A and Shadow, preserving their already fixed scenes.
8. **R8 - Current Switch 1 gameplay.** Jamboree then Smash: one source-backed diagnostic/pacing correction per
   milestone, each ending in the declared short route.
9. **R9 - Historical-only titles become current.** Violet, Wonder, Anime Girls, Sonic Generations and Beach Bounce,
   one at a time. Each milestone corrects a reproduced source defect or, if none reproduces, records a current clean
   result without manufacturing code work.
10. **R10 - Whole-library acceptance.** Sequential 14-title sweep on one final candidate. Historical evidence never
    counts as current pass. Resolve failures through new corrective milestones; do not turn the sweep into a blanket
    stop after one title fault.

## First implementation assignment (separate agent)

**Assignment R1A: pin and refresh the MIT nxnative Ampere translator.** This is the highest-confidence source delta.

Writable files, after the agent records their existing dirty state:

- `D:\Projects\RecompLauncherEngine\src\shaders\tools\build_dmsh_xlate.ps1`
- `D:\Projects\RecompLauncherEngine\src\shaders\tools\build_dmsh_xlate.sh`
- `D:\Projects\RecompLauncherEngine\src\shaders\tools\dmsh_xlate.patch` only if `7ebb76e` requires a narrowly
  reviewed Vulkan adaptation
- `D:\Projects\RecompLauncherEngine\src\graphics\tests\ampere_orin.rs`
- A new wave record and its own evidence directory; no game files, latches, reference tree or unrelated source.

Concrete change:

1. Change both builder defaults/verification to source exactly
   `D:\Projects\RecompLauncherEngine\build\Q\nxnative-up` at
   `7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f`; fail if commit differs rather than silently using old
   `D:\Projects\nxnative` (`e4b0d9b`). Keep the existing per-stage descriptor-set/Vulkan-target patch.
2. Build identity must continue through `--source-sha` and `Tools::identity`; record old/new executable hashes.
3. Run the existing `ampere_ops_match_the_orin` gate. Remove a `HERE` row only when the test reports "matches now";
   do not move failures into `KNOWN`/`APPROX` or relax bit comparisons. Expected removals include DP2A/DP4A,
   directed FADD/FMUL/FFMA and newer opcode support; actual output decides.
4. Retranslate the two DKB pass-27 keys and MKW fault pair offline, validate SPIR-V, and record reflection/hash deltas.
   Do not claim a game fix from changed SPIR-V.

Agent test/build concept, subject to a lead-granted compiler/GPU lease: builder script; shader package unit tests;
`cargo test -p graphics --features gpu-tests --test ampere_orin ampere_ops_match_the_orin -- --ignored --exact
--nocapture --test-threads=1`; focused translation/`spirv-val` for the four keys; then lead release/relink. The lead,
not the implementation agent, authorizes one sequential DKB route. Expected game evidence is pass-27 nonconstant colour
and visible readable input-responsive world, or a precise unchanged result that sends R2 to output/attachment mapping.

**Stop conditions:** patch does not apply cleanly; source commit is not exact; any formerly matching non-platform
operation regresses; new SPIR-V fails validation; GPU reset/device loss; DKB unchanged after a valid candidate. A
failure stops that workload/title and preserves evidence; it does not authorize exception-list growth or automatic
retry.

## GPU and game-test policy

- Tests remain sequential: one compiler command, one game/GPU workload, one title at a time under explicit leases.
- Preserve blacklists, starting caches, binary/input hashes, saves and both historical fault records. Never clear a
  blacklist or retry automatically to manufacture a pass.
- A reset/device loss stops and latches the offending workload/title. It blocks another run of that title/path until a
  reviewed correction or bounded causal discriminator exists. It does **not** impose a blanket halt on unrelated
  titles whose workloads are safe and separately authorized.
- Preserve first-fault history and no automatic retry. Cleanup is by owned PID/process tree. Build and GPU work never
  overlap. Missing evidence is unknown, not pass.
- Synthetic GPU tests need the GPU lease but are not gameplay. A title result requires real screenshots reviewed with
  input response, logs, pacing/audio/memory over the declared window, diagnostics, save restoration and cleanup.

## Acceptance definition

A title passes the current candidate only when its declared short route reaches **first controllable gameplay** with
correct readable rendering, no major graphics issue or device loss, and no unexplained skipped work/resource/service
error in the observed window. Pacing uses the title's native target; audio must be real/non-silent where expected;
memory scope says whether it is a short loading window or steady-state; diagnostic scope lists any accepted benign
line. A short pass does not certify story progression, every board/minigame, online play or a long session. Historical
evidence is never a current pass.

| Status | Count now | Exit requirement |
|---|---:|---|
| Hard blocker: no usable gameplay | 3 | DKB visible control; MKW safe/readable control; OoT first control |
| Current playable/correction evidenced, not clean | 6 | MK8D, Arceus, Z-A, Shadow, Jamboree, Smash repeat current route with residual major issues closed |
| Historical-only | 5 | Violet, Wonder, Anime Girls, Sonic Generations, Beach Bounce run current candidate |
| Current clean | 0 | Meets all declared criteria above |

## Current execution update - DKB R1/R2 outcome

The first implementation sequence has now produced and game-verified two source corrections:

- Pinned/current nxnative Ampere translation improved the Orin oracle from 124/193 to 153/193 exact matches and
  changed DKB's pass-27 fragment arithmetic, but a full refreshed-shader DKB run remained black.
- nxrs incorrectly reapplied raw Omap component bits as Vulkan ROP write masks after translated SPIR-V had already
  encoded scalar component stores. Separating location gating from NVN channel masks changed DKB `0x6f` from
  `[RGBA, GB, none]` to `[RGBA, RGBA, none]`; pass-27 colour/depth attachments then changed and contained meaningful
  HDR values (10,307 nonzero pixels, finite range 0..54).
- The remaining black final image was isolated to translated NDSR `14a8aaf3e0a9edbd`: scene input owner 53355 was
  populated and correctly bound, while output owner 4856 remained black and was correctly sampled by final pass
  1062. The MIT-reference-aligned terminal NDSR host blit now prepares both images, barriers, scales 1440x810
  B10G11R11 into 2880x1620 B10G11R11, barriers again and marks the output GPU-owned.

Lead R6 game evidence: `build/DELEGATION/R6-DKB/run`, exit 0, 166.3 s, no device loss, save/cleanup restored. The
final gameplay screenshot shows the mining tunnel rather than a black world, establishing the major visual blocker
as corrected. The diagnostic run remains ~20.85 FPS with 14 gameplay audio underruns, and visibility occurs late in
the scripted route; DKB therefore moves from **hard black-world blocker** to **playable visual correction evidenced,
performance/audio/current uninstrumented acceptance still open**. Do not call it current clean.

Accepted source state retains the refreshed translator, Omap correction, corrected prewarm availability logging and
terminal NDSR host blit. The unsupported conditional-render hypothesis was reverted. The serial-specific parked
texture hypothesis did not fix the game and was reverted before R6. Evidence/report: `docs/waves/R1A.md` and
`build/DELEGATION/R1A`, `R1B`, `R2-DKB`, `R3-DKB`, `R4-DKB`, `R5-DKB`, `R6-DKB`.

## Current execution update - MKW R3 safety outcome

Reference comparison found that nxrs's NVN2 task translator allowed up to 65,535 emitted mesh groups although its
translated task payload carries only 32 records. `src/shaders/src/mesh.rs` now clamps requested groups to 32 before
`OpEmitMeshTasksEXT` and uses shader identity `mesh 4`. Focused SPIR-V, shader tests (73/1), graphics tests (126/3),
full 11,609-shader translation/validation, release build and relink passed.

Lead R3-MKW evidence: `build/DELEGATION/R3-MKW/run`. The exact bounded workload reached the 35-second ceiling and
exited 0 without device loss, invalid-read/fault-address diagnostics or reset; the earlier matching baseline reset
near 23 seconds at direct mesh draw #94396. The map screenshot is readable and normally coloured, not the historical
near-white/green haze. Work skipped in the measured rows is zero; the retained blacklist warning appears during clean
exit. Saves restored and cleanup passed.

This closes **R3 safety for the bounded workload only**. It does not certify a race or long stability. Startup/prewarm
performance remains poor (9.43 FPS frame-derived / 6.15 metrics mean over 0-35 s, large pipeline stalls), six audio
underruns occurred and peak private memory reached 10.8 GB during loading. Next MKW outcome is a longer route into a
controllable race, followed by steady-state haze/pacing/memory work from measured owners rather than startup totals.

## Questions and blockers

### Answered by this roadmap

- DKB pass 18 is not empty depth: every captured pixel is finite/non-clear. Pass 27 is the first captured failed colour
  producer. Another depth/geometry diagnostic is not next.
- The highest-confidence first assignment is the stale Ampere translator, not DKB by priority assumption.
- MKW safety, haze and performance are three different outcomes; a reset fix cannot claim the other two.
- OoT replacement must be a complete transaction and remain off until guest layout, current GPU parity and shadow
  frame gates pass.
- A reset pauses its offending workload/title, not all library testing. Fault history and no-retry rules remain.
- The represented library has 14 titles. No owner-launcher access or extra inventory claim is needed.

### Engineering blockers/questions

- **MKW reference provenance:** `D:\Projects\mkw-recomp-main` has no `.git`; obtain its exact commit before any code
  port. Behavior may still guide an independent reimplementation.
- **MKW faulting instruction:** current evidence identifies the shader pair and draw, not the exact load. Resolve by
  offline SPIR-V/reflection and immutable range/lifetime audit; a new game recorder is not the default next step.
- **DKB pass-27 shader semantics:** determine whether `omap 0x6f` and fragment outputs are correct after translator
  refresh. Existing raw data cannot by itself distinguish a shader producing constants from a write-location mapping
  error.
- **NDVE device arithmetic:** DS-01 CPU proof is current, but corrected 16-product GPU parity has not run. This is a
  technical gate, not an owner choice.

### Genuine owner decisions

None are required to begin R1-R9. The ten proposals in `DECISIONS.md` remain awaiting owner but do not block these
correctness outcomes. Release/distribution, commit/push, and any expansion beyond short first-gameplay acceptance
remain owner-controlled.

## Evidence appendix

Primary current evidence:

- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DC-02\run\capture-inventory.json`
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DC-02\run\run.log`, especially 2412 onward for frame 5716,
  attachment lines 15169-15172, pass-27 state beginning 15312, and seven completion lines in the inventory
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DC-02\run\raw\chain-5716-*`
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DC-02\run\summary.json`
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DC-02\save-recovery-v1\recovery-result.json`
- `D:\Projects\RecompLauncherEngine\build\HARNESS3\20261121-135645-mkw-baseline\{run.log,triage.json,summary.json}`
- `D:\Projects\RecompLauncherEngine\build\OOTWAIT\investigation.json` and named OOTWAIT run directories
- `D:\Projects\RecompLauncherEngine\build\CAMPAIGN2\campaign-results.json` and current follow-up outcomes named in
  `PORT_PLAN.md`

ROADMAP-01 artifacts:

- `D:\Projects\RecompLauncherEngine\build\DELEGATION\ROADMAP-01\dkb-raw-analysis.json` - bounded streamed
  statistics for seven raw files, 30,326,400 bytes total; no view swizzle/sRGB conversion
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\ROADMAP-01\sources-read.txt` - sources actually read and
  bounded-read limits

Research limits: no Cargo/test/build/game/GPU/replay command was run; no owner data was read; no current mkw-recomp
commit could be recovered from the source export; no unsupported clean-reference or clean-game claim is made.
