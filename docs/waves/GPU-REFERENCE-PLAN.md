# GPU reference alignment plan — 2026-11-23

## Status / file table

**Final planning disposition: source review complete within the bounded scope below. No crash root cause proven.
First implementation: private, prepared NDVE shadow transaction; retain quarantine.**

| File / scope | Disposition |
|---|---|
| `docs/waves/GPU-REFERENCE-PLAN.md` | Sole writable file; incremental planning report |
| nxrs working tree, nxnative current/older, Turbo references | Read-only source and Git metadata review |
| Retained incident evidence and tool manifests | Bounded read-only inspection |
| Builds, tests, games, GPU/Vulkan probes, installs, system settings, saves, library, fault latches | Held; no execution or modification authorized |

Owner explicitly requests resumed planning, with no subagents. The current dirty tree is the implementation baseline;
HEAD alone is insufficient. Read AGENTS, delegation/style guides, THERMAL-INCIDENT/ENGINE/REVIEW/BUILD/WONDER and
roadmaps 33/34. Historical intermediate claims in those reports are superseded by their final dispositions.

### Evidence boundary established before source review

- Owner reports healthy nxnative/Turbo experience. Matching reference executable, title/update, backend, shader
  cache and route identities are not yet available; this is an important reported baseline, not universal proof.
- New owner report: another crash with a “hypervisor error,” a disabled Windows feature and restart. Exact feature,
  bugcheck/dump and causality are unverified. Do not infer Hyper-V or a GPU fix. No event/dump investigation here.
- Historical OoT second incident: native activation 9.02 s after exact comparison 9.00 s; 473 presents; submission
  4764 incomplete after five seconds at 22.24 s. No exact fault instruction or pre-crash temperature series.
- MKW draw 5211, task `fea1d6ca04512de1` / mesh `ddc034e8e009ebfc`, six direct groups, invalid read
  `0xd59256000`: mesh-4 fanout bound alone was insufficient. Mesh-5 fixed 4 KiB initialized payload survived a
  44.32-second follow-up; a full race is unproven. Violet's separate ~232-second display reset has no fault instruction.
- Already corrected: native descriptor-pool lifetime, shadow-counter ownership, buffer partial cleanup, finite
  completion/fatal-72 policy, bounded guest descriptor allocation. Live NDVE on/1/check remains quarantined.
  H1–H4/E1–E2/R1 closed for reviewed CPU/source counterexamples. These are not proven reboot root causes.
- THERMAL-BUILD final disposition records new MSVC release deployment, contract 2 and CPU/GPU peaks 68/61 C;
  external game exe/code DLLs were not rebuilt. Wonder's later attempt stopped before workload creation at 91 C.
  Preboot 93–95 C/foreign Leonida build observations are historical; no assumption those processes or HWiNFO
  logging survived reboot. Idle postboot temperature cannot authorize tests.
- DKB accepted corrections are Omap location/component separation and terminal NDSR host blit. Withdrawn R3/R5
  hypotheses are not fixes. Old `build/DELEGATION`, HARNESS3, NDVE and OOTWAIT citations are historical, not claims
  of retained artifacts.

## Established findings checkpoint

1. **The requested references do not exercise one equivalent OoT operation.** Current nxnative
   `dm/gfx/nvn/nvn_ndve.inc:1–14,125–180,207–254` implements a full Metal native network and resampler, enabled
   by default. Turbo `runtime/nvn/vk/vk_replay.c:1860–1904,1953` instead skips convolution keys and copies/scales
   the picture with blits by default, expressly omitting learned corrections. Thus a healthy Turbo Vulkan run
   cannot certify nxrs's native Vulkan network. Turbo also contains other renderers; the user's actual route is
   not established by the checkout alone.
2. **Confirmed quarantined design defect: shadow execution is not isolated.** nxnative
   `nvn_ndve.inc:58–65,182–186,249–251` uses private shadow images. nxrs
   `src/graphics/src/vk/exec.rs:5896–5954` passes guest storage views into `record_graph`;
   `newndve/kernels.rs:251–300` writes network output before fallible luma allocation/descriptor creation.
   Caller fallback `exec.rs:6096–6104` replays guest commands without undoing those writes. Preserve quarantine;
   split immutable resource preparation, private recording, and explicit commit. This is not newly discovered
   proof of the historical hang; the thermal review already recognized the unsafe transaction.
3. **Confirmed recognition mismatch, occurrence at failed frame unknown.** nxnative selects specific reflected
   registers (`nvn_ndve.inc:111–124,157,235,242`): first picture TEX1, terminal picture TEX4/output IMG0,
   resampler picture TEX0/dither TEX3/output IMG1. nxrs's `ndve_find_image` searches by dimensions/format
   (`exec.rs:5697–5710,5734–5812`) after snapshotting only `(id,serial)` lists. Two same-sized resources can select
   the wrong operand. Full image handles also do not establish memory/subresource nonaliasing
   (`newndve.rs:287–299`); snapshots lack view ranges and memory-generation identity.
4. **Current sequence validation proves too little for replacement.** `newndve.rs:314–340` validates key order,
   weight offsets and nonzero/direct network grids, but not exact model grids; resampler dimensions/indirectness
   are not part of that check (`exec.rs:6049–6077`). The native graph always dispatches its fixed shapes.
   nxnative also ignores `dp` (`nvn_ndve.inc:254`): do not copy this reference weakness. Require the full guest
   dispatch contract, resource versions and side-effect proof before consumption.
5. **Native buffer/image recorders enforce different Vulkan contracts.** `kernels.rs:449–481` checks range cap,
   alignment, checked end, overlap and dispatch limit; `:581–589,665–686` checks only non-null/minimum ranges
   before descriptor writes and dispatch. Current fixed dimensions are within the common 65,535 X-grid minimum
   (largest pack is 64,800 groups; resampler 57,600); this is a missing invariant, not evidence of an oversized
   historical dispatch. Unify immutable validation using cached device limits before any command is recorded.

Git reads reconfirmed nxrs `143515097a0fa134c6fc4be839971692a39ac4de`, current nxnative
`7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f`, older nxnative
`e4b0d9b75d39c41c10fdcfec2f0b070df496d7e5`, Turbo
`11e966f33b1f2aec8b05d0764f795804e807510c`. References have no tracked modifications in the inspected Git status;
nxrs has extensive tracked modifications. Both nxnative LICENSE files are MIT. Turbo LICENSE/NOTICE is AGPL-3.0
with separately licensed components; its Vulkan adaptations are behavioral reference only here. No code copied.
The narrow older/current nxnative Git diff has no changes to `nvn_ndve.inc` or `ndve_engine.metal`; translator and
`nvn_mtl.m` do differ. The already-refreshed nxrs translator builder pins `7ebb76e`; do not re-propose the old
roadmap's stale-checkout refresh as new work.

### Checkpoint 3 — shared upload ordering, MKW and provenance

**Shared synchronization discrepancy: no unconditional upload-entry dependency; causal classification refined below.**
`queue.rs:550–570` begins `sub.pre` and `sub.main` but inserts the entry full barrier only into `main`.
`queue.rs:1530–1544` then calls `mem::begin_submit`; ordinary dirty mirrored-pool uploads follow
`mem.rs:1869–1878` → `upload_now:1114–1119` → `Sub::upload:363–381`, which records `vkCmdCopyBuffer`
in `pre` with no entry dependency. `queue.rs:694` places a barrier at the **end** of `pre`, and `:705–740`
submits `[pre,main]` with a timeline signal but no ordinary wait on the previous timeline value (sparse/present
waits are conditional). Queue submission order alone does not make prior GPU accesses finish before those
earlier upload writes. The main-entry/end-pre barriers are too late to protect the first upload.

Candidate counterexample: submission A accesses existing mirrored buffer B; submission B uploads a CPU-dirty
range of that buffer without a dependency covering A's **last** access. This is a confirmed entry-policy difference,
but a missing entry barrier alone does not prove invalid synchronization: `exec.rs:1734,6214` already adds broad
barriers after render/compute, whose destination scopes can cover later submissions. The initial checkpoint's
generic shader→upload example was therefore insufficient and is withdrawn. Audit A's terminal writeback copies
(`queue.rs:650–656`) after those barriers and B's ordinary versus late cold-pool upload paths separately;
`mem.rs:1843–1861` defers ordinary writes overlapping in-flight readbacks, whereas `sync_range:743–770` needs its
own proof. Do not assert a reachable fault without that full ordering/ownership trace.
Turbo `runtime/nvn/vk/vk_submit.c:47–59` explicitly
adds ALL_COMMANDS/MEMORY_WRITE → ALL_COMMANDS/MEMORY_READ|MEMORY_WRITE at the upload-buffer entry for exactly
this counterexample. nxnative `nvn_mtl.m:1912` likewise starts compute encoding with a queue dependency;
Metal resource coherence itself is not a substitute for Vulkan ordering. A narrow unconditional upload-entry
barrier is a reasonable conservative candidate only after the predecessor-tail audit; keep it separate from
the confirmed NDVE resource/transaction defects. The final first assignment below prioritizes those defects.

MKW reference outcome: Turbo's native Vulkan path also issues native mesh draws (`vk_replay.c:1553,1816`), but
its `tools/nxshader/nxs_spv.py:752–755,823–826` allocates the live payload prefix and allows 1,024 launches.
That is **not** stronger evidence than nxrs mesh-5's fixed initialized 4 KiB and 32-launch contract
(`src/shaders/src/mesh.rs:128–132,619–624,831–889`). Do not regress to Turbo's limits. Turbo's default OpenGL
route has a distinct task/mesh-as-compute implementation (`runtime/gpu_gl.c:2893–2927`); backend selection is
explicit at `runtime/main.c:252` and `backend_win32.c:68–77`. User's healthy route is not yet identified.
The retained MKW log was independently read at `nxrs-library-validation/runs/02-mkw/run.log:601–659` and
confirms draw 5211 and invalid read `0xd59256000`; it also gives an instruction **address** `0x2012237f0`,
not a decoded faulting instruction. No allocation in the recorded range tracker held the invalid address;
that negative tracker result does not prove which allocation or instruction caused it.

Tool provenance is resolved to the available evidence: R1A report `docs/waves/R1A.md:25–30` records old/new
binary hashes but the same `--source-sha e11dc43d4ddc5b58`. Upstream `Program.cs:204–208` hashes only embedded
Program.cs + csproj, excluding Ryujinx sources; unchanged source-sha therefore does **not** mean an old translator.
Retained `nxrs-thermal-build/final-verification/packaged-tools.json:1–14` confirms packaged translator SHA-256
`7a28092f4702c5b067e351cbe105ac4b23aed4b9bb1ce5bd09ccfacfb17dc30e` and validator
`bd7296d3f4f469642d65a1d135901912483e7e3b661f7fda36bc932729759c0f`. These are manifest reads, not a new
binary hash computation. Original disposable R1A build directory is not asserted to survive.

Installed SDK metadata: `C:/VulkanSDK/1.3.268.0/components.xml:1–25` records SDK/core 1.3.268.0;
`Bin/VkLayer_khronos_validation.json:1–9` declares API 1.3.268, implementation 1; binaries for glslang,
spirv-val/dis/opt/as, vkconfig and validation are present. `.NET` SDK directory contains 10.0.401, matching
the tool project's net10.0 requirement; no package installation is demonstrated necessary for the first fix.

## Baseline / path / commit / license

Aliases in the matrices below: **R** = `D:/Projects/RecompLauncherEngine`; **N** =
`R/build/Q/nxnative-up`; **O** = `D:/Projects/nxnative`; **T** = `D:/Projects/Switch2RECOMP-main`.
`vk/` in an R anchor abbreviates `src/graphics/src/vk/`. All line anchors describe the actual working files read,
not a claim that HEAD contains the changes. Line numbers will move with future edits.
For short filename anchors: R renderer `.rs` files are under `src/graphics/src/vk/`; R `kernels.rs` and
`contract_tests.rs` under its `newndve/`; R `mesh.rs`/`job.rs`/`reflect.rs`/`xlate.rs` under `src/shaders/src/`;
R native `.comp` files under `src/graphics/shaders/`. N `nvn_mtl.m`/`nvn_ndve.inc` are under `dm/gfx/nvn/`,
`ndve_engine.metal` under `dm/gfx/msl/`, `dmsh_meshstage.py` under `dm/gfx/tools/`, and `dmsh_xlate.csproj`
under `dm/gfx/tools/dmsh_xlate/`. T `vk_*.c` are under `runtime/nvn/vk/`, `gpu_gl.c` under `runtime/`,
and `nxs_spv.py` under `tools/nxshader/`. External `nxrs-*` evidence roots in this report are under
`C:/Users/RISKYB~1/AppData/Local/Temp/opencode/`.

| Baseline | Verified identity / license evidence | Actual comparable operation / limit |
|---|---|---|
| R current source | HEAD `143515097a0fa134c6fc4be839971692a39ac4de`; extensive tracked modifications observed; new NDVE files read directly. Graphics manifest MIT (`src/graphics/Cargo.toml:1–7`). | Vulkan renderer plus Rust shader wrapper and native GLSL. Not identical to any previously deployed game binary. No untracked tree census needed to preserve work. |
| R deployed release | `dist/nxrs/nxrs-engine/engine.json:1–6`: contract 2, version `143515097a0f`, windows-x64. Final THERMAL-BUILD receipt records engine `0a543e47e812805e4edff039831e1be99ff8c9bcae2765d2656dfbc7d9ef7cd1`, front lib `cc723594c7fa3ca62287203bf84e4bc1dd5a9d01c3672ce7c525e3994cdc90bb`. | Incident corrections compiled in MSVC static libs; external title exe/code DLL deployment still owed. No new hash execution here. |
| N primary | `7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f`, clean tracked status. `LICENSE:1–21` MIT/seedds; `dm/third_party/ryujinx/LICENSE.txt:1–9` MIT/Ryujinx. | Full Metal NDVE, Metal object/mesh stages, Apple memory model. MIT source may inform independent Rust/Vulkan implementation with existing project attribution rules. |
| O older | `e4b0d9b75d39c41c10fdcfec2f0b070df496d7e5`, clean tracked status; `LICENSE:1–21` MIT. | Narrow Git comparison found **no NDVE host/kernel delta** to N; translator and `nvn_mtl.m` differ. Not a separate known-good execution trace. |
| T Turbo | `11e966f33b1f2aec8b05d0764f795804e807510c`, clean tracked status. `LICENSE.txt:1–6`, `NOTICE.txt:23–31,52–72,84–89`. | Root AGPL-3.0; native Vulkan adaptation is behavioral reference only. Vendored Eden is GPL-family; do not copy. Separately MIT-labelled files do not relicense neighboring code. |
| T native Vulkan | `runtime/backend_win32.c:68–77`; `runtime/nvn/vk/{vk_replay,vk_submit,vk_mem,vk_dev}.c`; `tools/nxshader/nxs_spv.py`. | `renderer=vulkan` selects NVN Vulkan if built with it; absent NVN can select Eden. NDVE default blits omit the network. Native mesh remains native mesh. |
| T default OpenGL / Eden | `runtime/main.c:252`, `runtime/gpu_gl.c:2893–2927`; vendor location `runtime/edenvk`, licensing `NOTICE.txt:65–72`. | OpenGL task/mesh executes as compute and indexed rendering. Eden has a different emulated GPU stack. Neither is established as the user's actual healthy route. No broad vendor review or code import. |

N's OoT profile names **v131072**, main build ID `e0f3ccfaf50065ae4ea01eb5aeb13b1a`
(`dm/games/0400486024DEC000/game.tsv:15–18`), matching the *reported package version* in R's campaign;
matching executable/module hashes and route still need receipts. N's MKW baseline is **v0**, main
`9e244438c3d8ef1c703a6a85ef99c4f0` (`dm/games/0400C3F00006E000/game.tsv:1,15–23`). R's two MKW labels
both used the same configured v0 package in document 34; “v1.8.0” in an entry name is not update proof.
N's own OoT README records platform-specific hangs and inconclusive comparisons (`:190–196`), as well as
successful Metal native-network measurements (`:64–66`). References are fallible, not universal clean baselines.
The optional mkw-recomp export was not used as an implementation donor or substituted for N/T.

## Causal-path review

### OoT: what can actually be reconstructed

Retained `nxrs-agent2/runs/20261123-024113-oot-ndve-production-gameplay-final/run.log:193–203` establishes
capture, two storage-image upgrades (serials 8364 and 8369), a seven-range raw global-memory table, exact
comparison and native activation. `:245–257` shows continued presentation through 473, parked guest event waits,
then timeline submission 4764 unfinished. These are observations, **not a command list for submission 4764**.
The summary's final frame row is guest time 15.5052; log clocks/guest time must not be conflated. Neither the
queue's heartbeat submit count 2845 nor checkpoint IDs are interchangeable with Vulkan timeline value 4764.
The first run's comparison was tolerance-based; the second's exact match applies to that observed frame only.

Current source path, for reviewing the historical design while quarantine stays effective:

1. `exec.rs:5532–5604` resolves reflection handles into flat texture/image ID lists and snapshots guest bindings,
   push bytes and owner serials. `:6025–6077` delays twelve network dispatches and recognizes the resampler.
   Non-state commands flush pending work (`:6595–6609`); end-of-stream flushes it (`:1802–1805`). This preserves
   a fallback path, but does not freeze pointed-to guest bytes or all resource/subresource interpretations.
2. `newndve.rs:314–340` checks ordered keys and weight/bias spans against 0x3c320. It cannot prove the bindings'
   data is unchanged between capture and execution. Exact grids, local size, terminal resampler indirectness,
   and intermediate side effects need an explicit model contract, not only key equality.
3. Resource preparation (`exec.rs:5724–5832`) can upgrade image storage, then finds images by size/format. It
   must instead identify the exact reflected operand and its registered view: image extent is not view extent.
   `ndve_copy_image_to_buffer:5861–5888` always copies color mip 0/layer 0 with the whole recorded extent.
   A nonzero mip, layer, swizzle or replacement backing is not represented in `ImageBinding` today.
4. `ndve_host_graph:5896–5954` copies picture/dither, resolves the twelve weight/bias slices through
   `mem::resolve:1340–1420` (which checks pool/window coverage), then records the native graph.
   The missing proof is not that resolve intentionally truncates spans: it refuses cross-window ranges. It is
   generation/ownership stability, actual bound view identity and complete preallocation before writes.
5. `kernels.rs:202–300` allocates eight zero-filled bordered NHWC tensors including sixteen spare rows, records
   pack + eleven internal layers + terminal output, then luma/resample. Shapes/offsets agree with N
   `nvn_ndve.inc:20–45,80–96`. Current scalar index arithmetic is bounded by fixed job counts
   (`ndve_layer.comp:22–24,38–69`); resampler workgroups cover exact 8×8 tiles with uniform barriers
   (`ndve_resample_image.comp:22–55`). No concrete native OOB index was found in these inspected fixed shapes.
   Buffers are checked more strongly than image-path buffers; output writes can precede later failure.
6. Every scalar kernel has a COMPUTE/SHADER_WRITE → ALL_COMMANDS/MEMORY_READ|WRITE barrier
   (`kernels.rs:557–568`); image kernels use full barriers (`:688`). Queue pre/main ordering, sparse waits and
   HOST_READ tail are present (`queue.rs:682–740`). These do not establish alias correctness, make a recording
   transaction atomic, or prove the unrecorded last faulting graph valid. Adding more barriers indiscriminately
   is not a resource identity fix.
7. Current cleanup requires the exact timeline receipt before writebacks, callbacks, pool destruction and scratch
   recycle (`queue.rs:808–930`). `Gpu::buffer` partial cleanup is present (`mod.rs:182–211`). Native pools register
   before use (`kernels.rs:518–520,623`); counter ownership precedes fill (`exec.rs:5977–5983`). Retain all of it.
   The graph still allocates all tensors each attempt; a whole-graph/in-flight byte budget is a separate missing
   integration property. N reuses process-owned tensors, but blindly sharing those across Vulkan in-flight frames
   would introduce a new race.

**Arithmetic contract is intentionally different:** N's Metal HMMA rounds after each eight-product half
(`dm/gfx/msl/ndve_engine.metal:16–27`). R preserves its retained translated Vulkan helper's sixteen-product
rounding (`ndve_layer.comp:33–51`, `kernels.rs:140–150`, `contract_tests.rs:20–70`). Historical isolated GPU
parity in document 34 supports R's translator-target contract, not universal console/Metal parity. Switching
rounding or tolerances now would erase the comparison target without explaining a hang.

### MKW: exact pair versus nearby mechanisms

The direct draw is six **task workgroups**; the emitted mesh count inside each task is a different quantity.
`exec.rs:1383–1427` stages `{count,first,0,0}` (first was 0 in the fault), binds setup, then issues native EXT
mesh. `mesh.rs:843–889` copies the live payload prefix, sets the mesh workgroup number, synchronizes and emits
at most 32 groups. `job.rs:120–125` already separates vertex/primitive component blocks and reserves index bytes
for whole 32-primitive blocks, agreeing with N `dmsh_meshstage.py:72–89`. Do not re-propose that as missing.

T's Vulkan wrapper has older payload/count/primitive-interface choices (`nxs_spv.py:752–775,823–849`). T's
OpenGL compute route bounds scratch with a different capacity scheme (`gpu_gl.c:2906–2924`). Neither proves
the same `fea1.../ddc...` instruction stream or reflection was executed. Exact pre-fix and mesh-5 SPIR-V,
reflection, translated-runtime rewrites and allocation map for draw 5211 were not recovered in the bounded
retained run files read here; document 34's pair hash prefixes are historical summaries, not fresh disassembly.

Device contract gap: `exec.rs:1066–1080,1357–1359,1412–1427` uses only per-axis **task** workgroup limits.
It does not account there for `maxTaskWorkGroupTotalCount` or choose mesh limits for taskless draws. Six-by-one
does not demonstrate this gap triggered the retained fault. `device.rs:514–516,697–700` gates both stage
features; boolean support is insufficient proof of payload/shared/output/local-size limits for each module.
Plan a reflected pipeline contract, not larger clamps, skipped draws or blacklist clearing.

### Shared memory, descriptors and lifetime

- Bindless image/sampler pool snapshots already exist: `tables.rs:26–46,302–368` creates immutable descriptor
  versions and retains them via `defer_after_use`; `queue.rs:250–266,748–754` covers queued/open replay.
  Reimplementing “immutable descriptor pools” wholesale would duplicate accepted work. This is distinct from
  freezing the **backing allocation** a physical address names and the native frame's texture view interpretation.
- Raw-gmem table R `gmem.rs:127–177` is staged once per submission; a later POOL_GEN change does not refresh
  `sub.gmem`. N explicitly discovers fixed pointer targets and finalizes table references at commit
  (`nvn_mtl.m:12753–12806`). A newly backed pool after first binding is a concrete table-visibility mismatch;
  a stale delta after pool-address reuse is a plausible fault path, not established for 5211 or 4764.
  R's highest-base overlap rule (`gmem.rs:62–79`) and table/log pairing must be retained. Do not copy N's
  final-table patching without proving how address reuse inside one submission preserves command order.
- R table/log reads use CPU-to-GPU staging (`gmem.rs:161–163`, `queue.rs:327–347`). T puts them in device-local
  buffers and documents an earlier PCIe-heavy translated OoT timeout (`vk_mem.c:1033–1057`). R's allocations
  might be device-local host-visible memory on this GPU; the allocator's actual chosen flags are not in the
  retained incident evidence. This is a concrete placement-policy difference and possible cost, **not proof
  of PCIe traffic or a cause in the replacement network**, which avoids most translated raw-global work.
- R's write-watch batching/distribution, upload deferral and in-flight writeback merge are already present
  (`mem.rs:1794–1861`; `queue.rs:631–675` records the baseline copies).
  Relevant proof obligation: CPU dirty writes, GPU writes, sampled image aliases and late `sync_range` calls
  must refer to the same generation/range. No evidence supports disabling write-watch or importing Apple's
  unified-memory behavior. Ordinary buffer resolution uses full requested spans, not guessed enlarged ranges.
- Shared partial-construction cleanup still has narrow independent gaps: `tables.rs:63–99` can return after
  buffer creation on memory-type/allocate/bind/map errors without releasing all earlier handles;
  imported-memory `mem.rs:1305` propagates bind failure without releasing the created/imported pair.
  This is outside the **already fixed** `Gpu::buffer` helper. Correct with the same ownership discipline,
  CPU failure injection, and no claim that such failures occurred in the retained incidents.

Shared feature comparison: R already requires synchronization2 and enables supported task/mesh features
(`device.rs:421,514–516,697–700`); it enables supported robustness2 by default with base robustness, normally
on NVIDIA (`:142–156,728–744`). T defaults to NVIDIA base buffer robustness but makes image/robustness2
diagnostic opt-ins (`vk_dev.c:227–257`). R is not missing those guards; preserve the stronger current policy.
Descriptor robustness does not supply bounds for arbitrary physical-storage-buffer pointers. Device-address
support, per-shader capabilities and physical allocation ranges must agree independently. R native NDVE uses
packed u32 fp16 storage and scalar FP32 operations (`ndve_layer.comp:3–21`); CUDA, cuDNN, TensorRT or a tensor-core
package is not a dependency or evidenced missing crash fix. No blanket feature enablement or feature-disable
experiment is proposed from this static comparison.

## Actionable reference-to-nxrs discrepancy matrix

Classification distinguishes source correctness from historical causality. **Confirmed defect** means a concrete
contract counterexample; it never means the reboot or exact GPU fault is explained.

| ID / classification | Exact R anchors | Exact reference anchors | Semantic difference; relevance and action |
|---|---|---|---|
| D1 **confirmed defect, quarantined** | `vk/exec.rs:5896–5954,6096–6114`; `vk/newndve/kernels.rs:251–300` | N `dm/gfx/nvn/nvn_ndve.inc:58–65,182–186,249–251` | Guest output writes before later allocation/record errors versus private shadow images. Fix private preparation/recording; no fallback after guest mutation. Directly on historical OoT native integration, exact hang causality unknown. |
| D2 **confirmed recognition defect, quarantined** | `vk/exec.rs:5532–5604,5697–5812`; `vk/newndve.rs:207–218,287–299` | N `nvn_ndve.inc:111–124,157,235,242` | Dimensions/format search loses semantic binding register and view range. Same-size decoy counterexample; capture exact operands, views, backing generations and alias intervals. No evidence decoy occurred at frame 473. |
| D3 **confirmed validation gap, quarantined** | `vk/newndve.rs:314–340`; `vk/exec.rs:6049–6077`; `vk/newndve/kernels.rs:449–481,581–589,686` | N `nvn_ndve.inc:11–14,238–254` (also weak grids); T `vk_replay.c:1890–1904` (different operation) | Arbitrary nonzero network grids and unvalidated final dispatch can select a fixed graph; image recorder omits scalar path's alignment/max-range/overflow/device-grid checks. Central prepared contract, including actual VkBuffer extent and cached limits. |
| D4 **intentional platform/backend difference** | `vk/newndve.rs:84–163`; `shaders/ndve_layer.comp:33–51` | N `ndve_engine.metal:16–27`; T `vk_replay.c:1860–1904` | Sixteen-product translated compatibility versus eight-product Metal; Turbo blits skip correction. Preserve target arithmetic; no speculative reduced-quality shortcut or tolerance change. |
| D5 **plausible synchronization hypothesis** | `vk/queue.rs:550–570,650–740`; `vk/mem.rs:743–770,1114–1119`; counterevidence `vk/exec.rs:1734,6214` | T `runtime/nvn/vk/vk_submit.c:47–59`; N `nvn_mtl.m:1912` | Upload entry lacks universal dependency; prior broad barriers sometimes cover it. Audit final readback/late upload range before claiming reachable race. Candidate pre-entry barrier only, no global queue-idle serialization. |
| D6 **confirmed visibility delta; fault hypothesis** | `vk/gmem.rs:127–177,180–223`; `vk/mem.rs:1374–1392` | N `nvn_mtl.m:12753–12806`; T `vk_mem.c:1062–1096` | Table fixed on first use misses later backing changes; log must stay paired with exact table. Add generation-aware per-use immutable versions or a proven commit-finalization design. Never mutate in-flight versions. |
| D7 **plausible performance/watchdog hypothesis** | `vk/gmem.rs:161–175`; `vk/queue.rs:327–347` | T `vk_mem.c:1033–1057` | Host-visible staging versus explicitly device-local shader lookup table/log. Record actual memory flags; only then move hot lookup storage with submission-owned copy/readback. No immediate driver/package remedy. |
| D8 **already fixed, scope limited** | `src/shaders/src/mesh.rs:31,128–132,619–624,831–889`; `src/shaders/src/job.rs:120–125` | N `dm/gfx/tools/dmsh_meshstage.py:72–89`; T `tools/nxshader/nxs_spv.py:752–755,823–826` | Fixed initialized 4 KiB, 32 fanout and primitive-block allocation are present. T Vulkan is not a safer payload donor. Preserve mesh-5 and validate exact pair; 44.32 s is not full race stability. |
| D9 **confirmed device-contract gap; fault relevance unproven** | `vk/exec.rs:1066–1080,1357–1359,1412–1427`; `vk/device.rs:514–516,697–700` | T `vk_dev.c:254–255`, `vk_replay.c:1553,1816`; N `dmsh_meshstage.py:30–33` | Task-only per-axis limits do not cover total/taskless or module resource limits. Reference paths are also partial/platform-specific. Build capability contract; no stronger claim about six-group fault. |
| D10 **already fixed / retain** | `vk/queue.rs:808–930`; `vk/newndve/kernels.rs:518–520,623`; `vk/mod.rs:182–211` | T `vk_submit.c:123–138`; N `nvn_mtl.m:655–711` | Completion-before-effects/retirement is present; native-pool leak and buffer partial cleanup closed. Exact per-submission resource/retirement evidence is still missing at historical fault, not permission to repeat fixes. |
| D11 **already present; evidence gap** | `vk/tables.rs:302–368`; `vk/queue.rs:250–266,748–754`; `vk/lost_bindings.rs:52–105` | N `nvn_mtl.m:12782–12806`; T `vk_submit.c:177–185` | Immutable descriptor versions differ from allocation ownership. Existing mesh evidence's global pool epoch is not individual allocation generation. Extend ownership receipts rather than replacing caches. |
| D12 **confirmed provenance limitation** | `src/shaders/src/xlate.rs:137–152`; `src/shaders/tools/build_dmsh_xlate.ps1:18–46`; `dist/nxrs/nxrs-engine/engine.json:1–6` | N `dm/gfx/tools/dmsh_xlate/Program.cs:204–208`; `dmsh_xlate.csproj:16–20` | Short source-sha excludes Ryujinx; binary hash already helps, but shipped engine manifest names HEAD only. Add full source/tool/patch/GLSL/SPIR-V and relink receipts; do not infer stale translator from unchanged short hash. |
| D13 **confirmed separate failure cleanup defect** | `vk/tables.rs:63–99`; `vk/mem.rs:1305` | N ownership discipline `nvn_mtl.m:655–711`; T `vk_mem.c:1044–1049` itself has partial-failure weakness | Error branches can leak constructed resources. Reference not infallible; independently use R's existing staged ownership helper pattern. No retained evidence these error branches caused crashes. |
| D14 **evidence gap, diagnostic-only difference** | `vk/lost.rs:51–78,103–118,191–213`; `vk/queue.rs:815–825`; native `kernels.rs:556,686` | T `vk_aftermath.c:1–7,70–99` | Native kernels lack per-layer checkpoint/resource receipts; fatal72 deliberately cannot block on fault queries. Add bounded pre-fault records; Aftermath only if machine-code attribution remains necessary and separately authorized. |

## Ranked patch waves — no execution in this assignment

All proposed changes are independently written Rust/Vulkan from invariants/specification. MIT N facts may be
adapted with existing license notices; T/AGPL and Eden/GPL implementation is not copied. Every lifetime,
synchronization or shader-arithmetic candidate requires **fresh source review after implementation**, then
separate execution authorization. Keep blacklists, existing guards, exact comparisons and fatal policies intact.

### P0-A — restore a real NDVE preparation/transaction boundary (first assignment)

**Files:** `vk/exec.rs` NDVE-only functions and snapshot fields, `vk/newndve.rs`,
`vk/newndve/kernels.rs`, `vk/image.rs` only if a production view/backing metadata accessor is necessary,
`vk/newndve/contract_tests.rs` (or one dedicated contract-test sibling). No shader arithmetic change.

**Change/deliverable:** a `PreparedNdveFrame` owning the resolved contract, scratch resources and private network
and resampler output images. Resolve first TEX1, terminal TEX4/IMG0, resampler TEX0/TEX3/IMG1 through actual
reflection mappings, not by “first image of the right size.” Record registered mip/layer/format/swizzle,
image-creation identity, guest storage interval/backing generation, exact buffer spans, direct grids/local sizes
and relevant constants. Refuse ambiguous views/aliases and unknown side effects. Complete all allocations,
pipeline lookups, descriptor creation and validated dispatch planning **before** native recording begins.

The shadow graph writes only private images; its resampler reads its **private network output**, not the guest
network output. Run guest work exactly once in original order and compare independently produced results after
their own dependencies. Register private image/view/buffer/pool ownership before recording and release only on
the exact submission receipt. Preparation failure leaves guest images and pending guest order untouched;
recording failure cannot replay over partially mutated guest outputs. No automatic live-on promotion. A future
commit-to-guest operation is a separate reviewed step after whole-frame resources and parity are proven.

**Dependencies:** existing completion helper and quarantine; truthful view metadata (diagnostic-only
`image.rs:157–165` may be evicted/absent and is not sufficient production authority); device limits snapshot.
Separate observation from write intent: ordinary `tex_view` storage acquisition already calls
`note_storage_write` (`exec.rs:2574–2593,2677`), which marks guest image ownership. A side-effect-free native
preflight cannot use that call as if it merely returned metadata. Preserve that accounting for actual guest
writes and future committed native writes; acquire private shadow views without publishing guest write intent.
Retain sixteen-product arithmetic, zero borders/spare rows, exact output comparison, current CPU-write merge,
and no guest tensor dispatch consumption without full side-effect proof.

**Meaningful CPU tests, later:** use an injectable recording/allocation sink exercised by the production planner,
not a duplicate graph. Feed same-size decoy TEX0/TEX1 and remapped handles; shared storage behind distinct image
handles; nonzero mip/layer; reused VkImage handle with changed creation epoch; weights moved after snapshot;
overflow/alignment/short/max-range limits; indirect/changed-grid terminal dispatch. Each refuses before commands.
Inject failure at **every** allocation/descriptor stage: no command references released owners, no guest write,
guest replay once in original order. For success, assert graph writes private owners only, resampler reads the
private network output, and retirement cannot occur on an earlier or failed fence. Tests must fail on the old
planner for the concrete decoy/partial-record examples; no broad stress suite needed.

**Later GPU discriminator:** separately authorized private-resource synthetic graph with exact tensor/output
comparison, guard regions, validation and resource-count receipts, followed by one bounded actual guest shadow
frame only after a fresh review of live integration. A 60-second maximum OoT title-movie process window would
cover the old second fault window; authorization must specify this separately and preserve quarantine until
resource/transaction proof permits a narrowly reviewed shadow route. No run of the old `on` path.
Accept zero guest mutation by shadow, exact comparison, no validation/fault/stall and exact receipt-owned
retirement. Neither a synthetic pass nor a single matching frame authorizes later automatic replacement.

**Disproof:** if immutable receipts for the faulting frame show the intended registers, nonaliasing views,
stable weights, complete isolated graph and all resource lifetimes valid, D1/D2 are not its cause. They still
must be corrected before this integration can be enabled. If the defect-free shadow never reaches the original
operation within the authorized window, the result is inconclusive rather than permission to extend/retry.

### P0-B — make the last native submission identifiable without weakening fatal72

**Files:** `vk/newndve/kernels.rs`, `vk/exec.rs` NDVE issue sites, `vk/queue.rs` submit/receipt correlation,
and a bounded sibling to `vk/lost_bindings.rs`; reuse existing checkpoint facility.

Publish preallocated fixed-size records **before** each native layer/copy and submission: run/source identity,
queue logical ID, submission serial, assigned timeline value, operation ordinal, native shader/SPIR-V hash,
grid/local sizes, descriptor spans, image creation/view/subresource IDs, table and allocation generations,
resource bytes and last-use/retire receipt. Link retained recording serial to timeline at submit. Export periodic
bounded snapshots through existing supervised evidence transport; fatal72 must remain immediate and logger-free.
No crash-time driver call, wait-for-dump, unbounded file flush or new fatal-path mutex.

CPU verification: wrapped-ring IDs never join different generations; overflow/omission is explicit; submit
failure never publishes completion; blocked consumer cannot block recorder/fatal. Later GPU acceptance is correct
correlation on an authorized normal synthetic submission, not a deliberate hang. Needed because retained
submission 4764 cannot be reconstructed from coarse frame metrics. This instrument is diagnosis, not a crash fix.

### P0-C — finish narrow partial-construction ownership cleanup

**Files:** `vk/tables.rs::desc_buffer`, `vk/mem.rs` imported-buffer bind path; reuse/extend
`vk/submission_safety.rs` only for actual shared production cleanup policy. Keep successful allocations and
deferred retirement unchanged. On memory-type/allocate failure destroy the local buffer; on bind/map failure
destroy local buffer and release local memory in valid order; do not free an object already published/in flight.
CPU failure injection at each stage, exact once-only release/order and mutex reentry checks. Fresh review before
any graphics integration execution. No special GPU fault reenactment required; later normal regression validates
successful paths. OOM-branch correction cannot be called the cause of OoT without error/allocation receipts.

### P1-A — shared synchronization proof, then smallest justified barrier change

**Files:** `vk/queue.rs::{new_sub,submit_sub}`, `vk/mem.rs::{begin_submit,sync_range,upload_deferred}`, and a
CPU recording-order test. Trace terminal writebacks/readbacks, presentation and oneoff commands after the last
ALL_COMMANDS barrier. Verify exactly which prior dependency protects each next-submission transfer; separate
ordinary in-flight deferral from late cold-pool uploads. If an uncovered legal access remains, put an unconditional
`barrier_all(g, sub.pre)` immediately after beginning `pre`, before its first upload. Preserve main-entry/end-pre,
sparse waits, timeline commit order and receipt effects; no queue-idle wait or lowered safety cap.

CPU test: record two submissions with a terminal transfer-read followed by upload-write, inspect actual barrier
stage/access scopes and placement; also model prior broad barriers so the test does not falsely call a covered
shader→upload edge unsafe. Future GPU test: two small buffers, deterministic data, A shader/transfer tail and B
upload/consumer, sync validation enabled, bounded to two submissions; zero hazard reports and exact bytes.
**Disproof:** an existing dependency/positive completion proof covers every relevant A→B access (including the
historical resources), making this missing-entry hypothesis noncausal. Do not sell a redundant barrier as a fix.

### P1-B — raw-gmem generation and allocation proof

**Files:** `vk/gmem.rs`, `vk/mem.rs`, relevant `exec.rs` raw bindings, `queue.rs` retirement; reflection plumbing
only if fixed-pointer discovery is shown necessary. First CPU counterexample: bind table G, create/back pool P,
bind again in the same Sub; current `sub.gmem` returns G with P absent. Second: reuse guest address with a new
allocation while old commands still reference old backing. Produce per-use immutable table versions and log
pairing, or a commit-finalization design that proves those semantics; prefer per-use versions when epochs differ.
Ensure the descriptor cache key changes with raw-table version. Retain every named allocation through its last
receipt. A table byte snapshot without retained owners is insufficient.

CPU tests: overlapping pools/highest-base semantics, sparse missing page, late backing, same-address reuse,
wrong-fence retirement and log-bit decoding against the original map. Only if available exact reflection identifies
a fixed pointer source, add N-like discovery with readable-range proof and bounded target resolution. Never scan
arbitrary pointer forests or assume CPU bytes represent GPU-generated pointers.
Later GPU: small two-generation pointer-table fixture with bounded indirect accesses, then exact MKW pair.
Acceptance: no missing/stale range, exact outputs, no fault. **Disproof:** retained issue-time maps already contain
all reachable allocations with correct deltas and lifetimes; then do not change raw discovery to explain that fault.

### P1-C — MKW native interface/device contract; fallback only if still needed

**Files:** `src/shaders/src/{job,mesh,reflect}.rs`, `vk/{shader,device,pipe,exec}.rs`, bounded pair fixtures.
Do not change mesh-5's payload/fanout or primitive-block layout absent a specific failing witness. Retain exact
pair before/after SPIR-V/reflection and all runtime rewrites; decode all reachable stage-memory indices and
physical-address loads for the six-task-group record. Add checks for task/mesh payload agreement, declared live
prefix ≤4 KiB, stage-memory extent, output counts/indices, required features and device local-size/shared/payload/
combined-output limits. Check total grid count with checked multiplication and choose taskless mesh limits where
appropriate. Unsupported valid work needs an explicit correct backend, not silent truncation or skip.

CPU tests: exact pair layouts and taskless/3D/indirect boundaries, payload reads at prefix/4 KiB boundaries,
zero primitive count, max_p not a multiple of 32, first/DrawIndex stride and overflow. Later focused GPU test:
exact task/mesh pair with valid synthetic range-backed inputs and output oracle, then a separately authorized
≤60-second original map-loading route. Accept passing old 26.87-second point **and the same semantic draw**,
no invalid reads/reset/validation error, readable unchanged output and no skipped work; a different draw number
after instrumentation is correlated by keys/arguments, not forced to equal 5211.

If native-specific failure persists with correct inputs and device contract, independently implement a Vulkan
task/mesh-as-compute backend using R's translated body: bounded task payload → prefix/count planning → mesh
compute outputs → indexed vertex consumption, with exact compute/indirect/vertex barriers and submission-owned
scratch. T OpenGL supplies only behavioral evidence. Split this larger backend into its own design after the
pair proof; do not use the optional unpinned mkw export as donor. Device capability selects a proven equivalent
route; keep blacklists. **Disproof of native-driver hypothesis:** same bad address appears before native emission,
or compute backend reproduces with the same table/inputs. A clean fallback alone does not prove a driver defect.

### P2 — provenance and measured efficiency/parity

1. **Reproducible tools and title linkage:** `src/shaders/tools/build_dmsh_xlate.{ps1,sh}`, `xlate.rs`,
   `src/graphics/build.rs`, packaging manifest producer. Record full N commit, content hash of all Ryujinx/tool
   sources + patch, tool executable SHA, SDK/tool pins and generated native SPIR-V hashes; propagate dirty-source
   digest through engine and game relink receipts. Keep binary-hash cache identity. CPU fixture: changing only a
   Ryujinx source changes provenance despite unchanged `--source-sha`; no-op rebuild does not invalidate keys.
   Required for trustworthy comparison, not a demonstrated GPU fix.
2. **Placement/resource economy after correctness:** measure table/log memory-type flags and native graph bytes/
   in-flight high water before adopting device-local tables or a bounded reusable NDVE arena. Reuse requires fence
   exclusion per slot; do not copy N's singleton tensors into overlapping Vulkan frames. CPU lifetime/cap tests,
   later bounded workload metrics and identical output; reject any retained-growth trend after warm-up.
3. **Console/Metal math alignment, separate from hang repair:** exact eight-versus-sixteen-product witnesses,
   translator output and native output must be updated together if console semantics warrant it. Preserve the
   current oracle, record expected changed bits and run independent arithmetic checks; no epsilon relaxation,
   forced certification or frame skipping. Shader-arithmetic changes need fresh review.

## Tools / packages — evidence-driven matrix

No compiler, validator, profiler, Vulkan loader or tool executable was invoked. Presence below means a directory,
manifest or source configuration was read; it does not certify postboot loading or the current PATH. Installed
SDK component subversions not exposed in retained manifests remain **unknown**, not guessed from the SDK number.

| Category / tool | Actual presence/version evidence | Needed action / fixes versus diagnoses | Upstream/license/pin; overhead and constraints |
|---|---|---|---|
| **Installed/use correctly — Vulkan SDK** | `C:/VulkanSDK/1.3.268.0/components.xml:1–25`; Bin contains glslangValidator, spirv-val/dis/as/opt, validation DLL/JSON and vkconfig. | Already enough to build/inspect existing Vulkan 1.3 shaders when authorized. No SDK reinstall is justified as crash repair. | LunarG/Khronos SDK; component licenses, mostly MIT/Apache per `Licenses/LICENSE.txt:5`. Pin actual used binaries. No execution now. |
| **Installed/use correctly — glslang** | R `src/graphics/build.rs:7–25,65–75,94–100` selects NXGFX_GLSLANG, VULKAN_SDK, then SDK 1.3.268.0; compiles target vulkan1.3. Binary present; exact glslang component version not independently exposed here. | Compile native GLSL after authorized source change; add binary/version receipt and compiler-input invalidation rather than broad upgrade. | Khronos glslang, mixed permissive licenses; obtain exact component pin from approved bundle manifest. CPU-only but compilation held and later supervised single-worker. |
| **Installed/use correctly — SPIRV-Tools** | SDK Bin tools present; packaged spirv-val hash in retained packaged-tools.json matches build prerequisite. R `xlate.rs:103–130` prefers release tools over SDK fallback. Exact component tag unknown here. | spirv-val for structural legality, spirv-dis for exact-pair review, spirv-opt only for a justified transformation. Validator success does not prove resource lifetime, aliasing, shader termination or valid physical addresses. | Khronos SPIRV-Tools Apache-2.0; pin binary SHA and actual tool version in receipt before future execution. No optimization sweep. |
| **Installed/use correctly — dmsh_xlate** | build/tools contains exe/PDB; source pin `7ebb76e`; retained shipped hash `7a28092f...dc30e`; R1A explains short hash `e11dc43d4ddc5b58`. | Refreshed translator is already installed. Add full-source provenance; do not repeat stale-checkout refresh. | MIT nxnative/Ryujinx; existing Vulkan patch hash must be recorded. NativeAOT executable needs no .NET runtime to translate. |
| **Installed — .NET SDK / existing native build stack** | `C:/Program Files/dotnet/sdk/10.0.401/.version:1–4`; N csproj net10.0/PublishAot. THERMAL-BUILD records Rust/Cargo 1.99.0 and successful MSVC build; not re-executed. | .NET needed only to rebuild translator, not to run shipped NativeAOT. No evidence missing MSVC/Rust package causes GPU failures. | .NET SDK servicing pin 10.0.401 as read; Rust/MSVC exact executable receipts from future authorized build. Native compiler fanout held/supervised. |
| **Pinned dependencies — ash/gpu-allocator** | R Cargo.lock `:182–183` ash `0.38.0+1.3.281`; `:978–979` gpu-allocator `0.27.0`; graphics manifest `:20–21`. | Keep lock while fixing ownership. Header/API version newer than SDK layer is not itself a missing runtime feature. | ash MIT/Apache-2.0, gpu-allocator permissive upstream licensing to retain on packaging. No dependency update without specific missing contract/bug evidence. |
| **Installed diagnostics; targeted future pin/update — validation layer** | JSON API 1.3.268 / implementation 1 and DLL present. R `device.rs:182–193` requests layer only when selected. No runtime discovery performed. | Future process-local core + sync validation for D5; GPU-assisted validation only after checking its mesh/BDA/descriptor-buffer coverage on the selected version. Layer revision may need a separately pinned newer bundle if the relevant check is absent; no speculative version recommendation. | Khronos Vulkan-ValidationLayers Apache-2.0. CPU/VRAM/timing overhead can be high and may change race behavior; one bounded synthetic run, never a blanket game sweep. Not a production fix. |
| **Genuinely missing engineering tool — native submission/resource receipt** | Existing `lost.rs` records guest checkpoints; `lost_bindings.rs:1–12,52–105` is optional mesh descriptor evidence, not native layer trace or individual allocation epoch. Native recorders `kernels.rs:556,686` have no layer checkpoint. | Implement P0-B. Required to identify the last native graph and join its addresses to owners without crash-time logging. | In-repo MIT implementation, no external package. Fixed memory/disk bounds; external snapshots cannot delay fatal72. |
| **Genuinely missing engineering artifact — comprehensive tool/source/relink manifest** | R engine.json names HEAD, upstream source-sha covers two files only. Retained build evidence supplies more, but not a self-contained shipped provenance contract. | P2 manifest; prevents comparing the wrong deployed title/tool/shader path. Diagnosis/reproducibility, not direct GPU repair. | No package required; existing SHA/JSON dependencies suffice. Hash only declared inputs during an authorized build, not every large folder. |
| **Optional diagnostics — Nsight Aftermath** | T integration exists (`vk_aftermath.c:13–20,70–99`), requires headers and DLL. No matching nxrs integration/dependency in reviewed graphics sources/manifest; installed NVIDIA SDK version/presence outside these paths not established. | Concrete reason: MKW reports a machine instruction address with no decoded instruction; OoT has no per-layer fault attribution. Consider only if P0-B + CPU/pair review leave attribution unresolved. Separate owner authorization and licensing review before acquisition/integration. | NVIDIA proprietary SDK/EULA, version unknown/unpinned. No copying AGPL adapter. Requires shader-hash/debug correlation and supported device/driver; dump capture can fail on hard reboot. Never delay fatal72 waiting for dumps. |
| **Optional/defer — Nsight Graphics, RenderDoc, gfxreconstruct** | SDK gfxreconstruct binaries present; Nsight/RenderDoc installed versions not established. T includes RenderDoc API header, not proof a profiler is installed. | Whole-frame capture is not the first missing tool: large capture/serialization can alter the failure and consume memory. Use only after stable bounded workloads for a specific unresolved pipeline/data question. | Vendor or per-component licenses; exact pin deferred. GPU/VRAM/disk overhead significant; no capture/profiler launch now. |
| **Already integrated historical telemetry — HWiNFO** | THERMAL-INCIDENT installation record says 8.60; existing harness freshness adapter documented. No live sensor/process query here. | Restarted/stopped logging must be qualified by fresh advancing samples later; historical CSV cannot pass preflight. Monitor diagnoses/refuses heat, does not fix GPU code. | REALiX terms; no install/update recommended. Keep supervised bounded CSV reads and device identity. |

**Genuinely missing external packages established by this review: none.** The immediate missing pieces are
resource/transaction contracts and fault-correlated provenance. A newer diagnostics bundle or Aftermath is
conditional, not a prerequisite for the first source correction. No driver, firmware, Windows feature or TDR
setting change follows from this comparison.

## Small future verification matrix — all held

Common requirements for **every** later execution: explicit new authorization; no competing compiler/game;
one workload and one worker build policy through the reviewed supervisor; fresh CPU and GPU device-identified
telemetry with observed CSV progression (not mtime), conservative CPU **<85 C**, GPU **<80 C**, sample age
**≤12 s**, no protective flag. Missing/stale/ambiguous/hot readings refuse, including the suspended-resume
boundary. Honor any stricter current harness cap for private/VRAM/free-commit/growth/stall; do not substitute
new limits or disable them. Record exact threshold configuration and telemetry timestamps.

Use an **external copied temporary save** selected by `NXOS_SAVE`, not production save transactions. Under one
machine lease retain before/after manifests of the copied save and read-only production-save identity/absence.
No game/library copy, manual metadata change, shared-stop clearance or save restore is part of this plan.
The lead owns any later reviewed resumption/history handling. Cleanup must positively prove owned job/children/
callbacks drained before any save finalization; uncertainty retains recovery intent. Historical interrupted OoT
save restoration is unknown and must be resolved separately before using that copy as a baseline.

| Title / question | Candidate prerequisites and bounded later discriminator | Acceptance / what remains unknown |
|---|---|---|
| OoT original native fault | P0-A/B + fresh review and current title relink receipt; private synthetic graph first, then separately authorized ≤60 s title-movie shadow window including former 9–23 s transition. Capture native operation/timeline/owner records before failure, image comparison and last progress. | Zero guest mutation from shadow, exact outputs, resource bounds/retirement valid, no fault/reset/stall/validation error. Native replacement stays quarantined until whole transaction/resource proof; no global certification. One frame/window cannot prove a long opening or gameplay. |
| MKW exact draw | Preserve mesh-5; P1 pair/reflection/range/device proof and corrected candidate only. ≤60 s same map-loading sequence with semantic draw `fea1.../ddc...`, six task groups, first 0; retain fragment `3cc3501255bb2d74` too. | Exact draw reached and completed, readable map, no invalid read/reset/new fault, no skipped work. Then separately authorize a bounded race route; 44.32 s history does not satisfy race stability. |
| Wonder regression | Only after shared candidate is relinked and fresh preflight passes; normal course route with reviewed prior 330 s hard ceiling, 115–260 s gameplay window, external save. | Responsive movement/jump, readable scene, zero new graphics diagnostics/reset, native ~60 FPS with recorded p95/p99/audio/memory; no claim from the stopped 91 C attempt. |
| Violet unknown reset | Keep independent latch; before another route obtain corrected candidate or a specifically reviewed discriminator and backend/driver/time correlation. At most prior 330 s-type bounded window only if explicitly granted, covering ~232 s event; do not repeat invalid broad-B route unchanged. | Reset classification and workload correlation first; still distinguish first-run creator/progression from GPU issue. No common-root-cause claim merely because OoT/MKW improve. |

Immediate stop criteria: engine 71/72, display reset/device loss, submission deadline/progress stall, validation
hazard, resource cap/growth violation, hot/stale/missing telemetry or protective flag, competing workload,
deadline or uncertain ownership. Terminate owned work before diagnostic I/O, persist additive fault history,
retain last bounded evidence, no automatic retry. A validation or thermal refusal is not a gameplay pass.

## Built (planning only)

Completed this reference-to-source plan, saved in three substantive checkpoints plus final synthesis. Sole file
written: `D:/Projects/RecompLauncherEngine/docs/waves/GPU-REFERENCE-PLAN.md`. No implementation, package,
settings, save, library or fault-latch change. No agents spawned.

## Tests (not run)

**None.** No test/build/compiler/validator/tool-version executable/game/GPU probe ran. Proposed CPU tests and
later GPU discriminators above are future work, not results. Earlier report passes are historical attributed
evidence only.

## Verified (reads/Git only)

- Required guides and final incident/build/review dispositions, actual dirty-tree NDVE/shaders/queue/memory/
  descriptor/device source, MIT N/O licenses, T route selection/native Vulkan/OpenGL/vendor licensing.
- `git rev-parse HEAD` and `git status --short --untracked-files=no` with explicit cwd at R/N/O/T; narrow read-only
  Git diff between O/N commits for NDVE, renderer and translator. No config/index/worktree modification command.
- Bounded retained OoT summary, second-run log and OOT stop; original MKW fault-log tail; known mesh-5 run
  directory; final packaged-tools manifest and installed SDK/.NET version manifests. No recursive incident scan.
- Tool API/source-sha provenance limitation confirmed from source and retained receipts. Reference runtime
  healthy-build identities, current tool binary bytes and historical faulting shaders were not independently
  regenerated or rehashed here.

**Owned execution resources:** no game, compiler, test, profiler, helper or background workload started;
read-only Git commands completed. None left running by this assignment. This is not a postboot machine census.

## Unproven

- Exact faulting native operation/instruction and allocation at timeline 4764; pre-crash temperatures and full
  VRAM history; mechanism of the reboot or separate user-reported hypervisor error.
- Whether the same-size operand/view, table-generation or final-transfer/upload hypotheses occurred at the
  retained faults. Source defects/contract gaps are not causal measurements.
- Correct matching N/T executable, title/module/update, backend, settings and shader pipeline for the user's
  healthy experience. Backend and operation equivalence must precede any A/B inference.
- Full MKW race stability, Violet reset cause, current rebuilt-title regressions, sustained-load cooling and
  postboot telemetry progression. No hold is lifted by this report.

## Slips

No scope violation. During the review the upload-entry discrepancy was initially framed too strongly; further
inspection found existing cross-submission-capable post-dispatch/post-pass barriers. The initial generic
counterexample was explicitly withdrawn in checkpoint 3, and D5 is a hypothesis requiring predecessor-tail
proof. No source change or execution was based on that preliminary conclusion.

## Questions

No routine source question blocks P0-A implementation planning. The only external evidence request for a later
matched comparison is the user's known-good reference run identity/backend/title-update/settings (prefer an
existing receipt/log, no new run). Recommendation: lead begins the bounded private-transaction assignment,
keeps all execution holds and quarantine, and treats any eventual reference or GPU run as separate authorization.
