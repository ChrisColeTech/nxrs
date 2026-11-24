# GPU alignment source patches — 2026-11-23

## File table (before implementation edits)

Baseline: HEAD `143515097a0fa134c6fc4be839971692a39ac4de` plus extensive pre-existing
dirty/untracked work. This task owns source edits and test sources only. Lead owns
independent review and gated execution. No compiler, test or GPU workload is started here.

| File | Intended bounded change |
|---|---|
| `src/graphics/src/vk/exec.rs` | NDVE operand capture/preparation and raw descriptor identity; mesh count validation |
| `src/graphics/src/vk/newndve.rs` | Resource/dispatch contracts |
| `src/graphics/src/vk/newndve/kernels.rs` | Prepared command graph and common native bounds checks |
| `src/graphics/src/vk/newndve/` new bounded siblings | Production planning/ownership helpers and CPU regressions |
| `src/graphics/src/vk/image.rs` | Only authoritative metadata access, if needed |
| `src/graphics/src/vk/queue.rs` | Minimal native receipts and raw table version retirement |
| `src/graphics/src/vk/gmem.rs` | Immutable generation-specific table/log versions |
| `src/graphics/src/vk/tables.rs` | Descriptor-buffer partial construction cleanup |
| `src/graphics/src/vk/mem.rs` | Imported-buffer partial construction cleanup; targeted generation ownership |
| `src/graphics/src/vk/submission_safety.rs` | Extend existing production cleanup/capability policy with CPU failure injection |
| `src/graphics/src/vk/shader.rs`, `pipe.rs`, `device.rs` | Capability contracts only where actual reflection supplies facts |
| `docs/waves/GPU-ALIGNMENT-PATCHES.md` | Sole owned report; checkpoint each wave |

Provenance/build producers and upstream shader translation are not yet selected for edits;
any extension will be recorded here first. No reference code copied from GPL/AGPL sources.

## Built

Initial source checkpoint: required guides, complete 564-line reference plan and final
thermal dispositions read. Primary MIT NDVE reference independently inspected. Current
source confirms shape-first recognition and output writes before fallible luma/descriptor
allocation. Reflection stores actual constant-bank/handle mappings; remapped Vulkan binding
ordinals are not NVN register numbers. Ordinary storage `tex_view` publishes write intent
and cannot be used as a read-only preflight accessor.

## Tests (not run)

### Implementation checkpoint 1 — P0 and raw generation source edits

- D1: production `PreparedNdveFrame` now separates all native graph/compare allocations
  and descriptors from command recording. Dedicated private images are RAII-owned before
  recording and submission callback-owned before first use. Resampling binds the private
  network output. Guest snapshots still replay once, in order. No live permission changed.
- D2: semantic TEX1/TEX4/IMG0/TEX0/TEX3/IMG1 selection uses reflected bank/handle words,
  retaining duplicate roles and rejecting disagreements. Preflight uses already-created
  authoritative view metadata; no `tex_view`, upload, storage upgrade or write intent.
  Cold/missing/aliased/replaced views refuse. Physical creation identity and guest byte
  intervals supplement handles. Noncanonical mip/layer/swizzle interpretations refuse.
- D3 partial: common buffer overflow/alignment/range/overlap and device dispatch checks
  now cover image kernels too; cached device limits; compare preparation precedes recording.
  Exact *guest* grids/local sizes and all consumed push constants are not established by
  the reference host (which ignores `dp`). Native kernel sizes cannot be substituted as
  evidence of the guest contract. This remains an activation blocker, not certified parity.
- D14 partial: 256-entry try-lock native operation ring, explicit overwritten/omitted
  counts, shader-content fingerprints, descriptor spans, views, recording serial and
  attempted/successful timeline links. Optional `NXGFX_NATIVE_TRACE=1` exports through the
  existing logger before submit; no fatal72 path changed. Individual view allocation
  generations and queue/source provenance are not yet fully represented; global pool epoch
  is labelled as such, not an individual allocation generation.
- D13: existing create/bind ownership helper now covers descriptor memory-type/allocation/
  bind failures and imported buffers; map failure uses shared ordered destroy/free helper.
  No unmap is issued on failed mapping. Success owners/accounting are preserved.
- D6: table/log pairs resolve together after ordinary stage resources; each changed pool
  generation gets fresh immutable staging and its own original-range log callback. Pool
  registration identity survives into log writeback and completion so guest address reuse
  cannot target a replacement pool. Descriptor cache key includes pool epoch. Existing
  `release` moves old Buf/granule owners into `defer_after_use`; recording closes into the
  actual timeline before those callbacks may run. No in-place table rewrite.

New source siblings: `newndve/prepare.rs`, `newndve/private_image.rs`, `newndve/trace.rs`.
Before P1-C edits: add `src/graphics/src/vk/mesh_contract.rs`, included by `exec.rs`,
for std-only checked count and actual SPIR-V scalar/array stage-memory reflection.
Before translator edit: `src/shaders/src/mesh.rs::to_stage` will reject a declared
live payload prefix above the existing 4 KiB allocation instead of silently truncating
it with `.min(TASK_PAYLOAD_WORDS)`. Payload allocation/fanout/primitive layout stay mesh-5.
Internal-only writeback tuples append pool registration identity; no guest/wire format changes.
All claims here are source implementation, not successful compilation or tests.

All compilation and test execution held for lead's fresh telemetry/concurrency gate.

## Verified (source/Git only)

Read-only Git status, HEAD and diff statistics from explicit project cwd. Existing changes
are the baseline, not attributed to this assignment. No index or Git configuration changes.

## Unproven

Implementation, compiler acceptance and runtime behavior are pending. No crash root cause
or post-reboot temperature is inferred. Live NDVE quarantine remains unconditional.

## Slips

None. Initial `D:\AGENTS.md` lookup was absent; project-root AGENTS was then read first.

## Questions

None at this checkpoint. Owned workloads: none.

## Checkpoint 2 — P1 device contracts and synchronization audit

**D5 source counterexample (not a historical fault attribution):** submission A's
`submit_sub` records pool-to-readback copies after the render/compute post-barriers.
Its final ALL_COMMANDS/MEMORY_WRITE -> HOST/HOST_READ barrier does not order those
transfer reads against B's transfer writes. A cold watched mirror need not have had
a CPU write for 60 frames even when A writes it on the GPU. B can skip that pool in
`begin_submit`, then see new CPU-written pages in `sync_range`; that path calls
`Sub::upload` directly without the INFLIGHT overlap split used by `begin_submit`.
B records those writes in `pre`, before the former end-pre/main-entry dependencies.
The new `barrier_all(g, s.pre)` immediately after begin-pre supplies an ALL_COMMANDS
execution dependency before any upload, covering A's terminal transfer read. The
generic shader-to-upload witness remains withdrawn: prior post-shader barriers can
already cover it. No queue idle, wait-budget or completion-policy changes.

**D9:** direct grid checks now choose task versus taskless mesh axes and total limits,
with checked multiplication and explicit negative-count refusal. SPIR-V admission
reads actual LocalSize, fixed scalar-array Workgroup/TaskPayload storage, OutputVertices,
OutputPrimitives, output locations and per-primitive decorations. Unknown padded stage
layouts, specialization-local sizes and multiview layouts refuse. Output allocation
uses effective locations and device vertex/primitive granularities; primitive indices
are excluded. Checks include local invocation count, payload, shared, combined memory,
output components/counts/bytes and payload+output bytes. A bounded 128-module cache avoids
re-parsing each draw. Source basis for the output formula:
`https://docs.vulkan.org/spec/latest/chapters/VK_NV_mesh_shader/mesh.html#mesh-output`
and the official `VkPhysicalDeviceMeshShaderPropertiesEXT` reference page (read-only).
No reference implementation code copied.

**Deliberate unsupported-path refusal:** indirect mesh counts are GPU-generated and
the existing clamp neither validates total count nor preserves workload. This patch
fails with existing resource-safety exit 72 before issuing an indirect mesh draw.
It does not claim an equivalent indirect backend. This is the most significant
runtime behavior change requiring lead review. Existing blacklist files/logic remain.
The translator now rejects declared live payload >4096 bytes instead of truncating it;
fixed initialized 4 KiB allocation, fanout 32 and primitive blocks remain unchanged.

Native trace follow-up now records physical image creation IDs for image kernels and
copies. The global pool epoch is still separately labelled; individual buffer allocation
IDs and GPU last-completed-operation markers are not invented. The ring is a CPU-issued
operation history, not decoded GPU fault attribution. No crash-time queries were added.

# Final source handoff

## Built

**Source patches implemented; nothing compiled or deployed in this phase.** This is
a review candidate with the remaining blockers below, not a complete GPU certification.

Exact assignment-modified files (all paths relative to the project root):

| File | Implemented delta |
|---|---|
| `src/graphics/src/vk/exec.rs` | Semantic NDVE operands, nonmutating view preflight, prepared private frame and compare wiring, raw paired descriptors/cache epoch, mesh admission |
| `src/graphics/src/vk/newndve.rs` | Image creation/guest-interval contracts; new modules |
| `src/graphics/src/vk/newndve/kernels.rs` | Immutable scratch/job/descriptor preparation; common bounds; operation records |
| `src/graphics/src/vk/newndve/prepare.rs` (new) | Production operand, buffer, preparation and replay policies with CPU tests |
| `src/graphics/src/vk/newndve/private_image.rs` (new) | Dedicated RAII private outputs and completion-owned transfer |
| `src/graphics/src/vk/newndve/trace.rs` (new) | Fixed ring and nonblocking recording/snapshot/link policies with CPU tests |
| `src/graphics/src/vk/newndve/planner_tests.rs` (new) | Actual native graph plan/private routing and partial preparation tests |
| `src/graphics/src/vk/newndve/tests.rs` | Guest-byte alias/unknown-generation regression |
| `src/graphics/src/vk/image.rs` | Always-present creation identity, backing-owned final view metadata, invalidation and regression |
| `src/graphics/src/vk/queue.rs` | Native attempted/submitted/completed links, optional bounded pre-submit export, raw Version type, writeback identity, pre-upload entry dependency |
| `src/graphics/src/vk/gmem.rs` | Immutable generation versions, paired logs, original-map decode, missing-backing/table-overflow refusal and regression |
| `src/graphics/src/vk/mem.rs` | Imported allocation cleanup, pure prepared-span lookup, pool identities and generation-safe writeback selection/completion |
| `src/graphics/src/vk/tables.rs` | Descriptor-buffer memory-type/allocate/bind/map cleanup |
| `src/graphics/src/vk/submission_safety.rs` | Production mapped-pair cleanup and every-stage injected-failure regression |
| `src/graphics/src/vk/mesh_contract.rs` (new) | CPU SPIR-V/capability admission and checked grid/output allocation tests |
| `src/shaders/src/mesh.rs` | Declared payload-prefix refusal and boundary/overflow regression |
| `docs/waves/GPU-ALIGNMENT-PATCHES.md` (new) | This file table, checkpoints and handoff |

No other source, producer, manifest, shared status document, game or library file was
changed by this assignment. Pre-existing modifications/untracked files remain in place.

### D-ID disposition

| ID | Disposition |
|---|---|
| D1 | Implemented private preparation/record boundary and comparison plumbing in retained production route; activation remains blocked |
| D2 | Implemented exact driver-bank semantic roles, duplicate handling, authoritative canonical views, creation and guest-interval rejection; unknown/noncanonical operands refuse |
| D3 | **Partial**: native device/buffer checks unified; indirect/empty resampler and backing changes refused. Exact guest grids/local sizes, complete push-constant/data-version and side-effect contract remain unproven |
| D4 | Retained sixteen-product target and zero borders; Metal-rounding/blit substitution rejected as unsupported changes |
| D5 | Implemented narrow pre-upload entry dependency for the specific terminal-readback/late-upload witness above; no historical-cause claim |
| D6 | Implemented per-generation immutable table/log descriptors and pool-identity writeback. Existing queue-deferred allocation owners retained; exact integration/retirement still needs review and execution |
| D7 | Deferred placement change: no actual chosen memory flags/performance witness |
| D8 | Retained mesh-5 payload/fanout/primitive layout; added rejection of oversized declared live prefix |
| D9 | Implemented direct/taskless/total and reflected stage-resource limits. Indirect mesh is explicitly unsupported/refused; exact fault-pair instruction/range proof and equivalent GPU indirect validation remain open |
| D10 | Retained exact completion, native pool retirement, Gpu::buffer cleanup, five-second budget and immediate fatal72; no duplicate replacement |
| D11 | Retained immutable descriptor snapshots; added raw cache generation and pool identities, not a replacement cache |
| D12 | Deferred comprehensive shipped provenance: binary-hash cache exists; producer/source-digest/relink work spans a separate cross-package contract and is not needed to compile these CPU regressions. Short upstream source-sha is still insufficient |
| D13 | Implemented narrow descriptor/import partial-construction cleanup using actual production helpers |
| D14 | Implemented bounded CPU-issued native records and timeline/retirement links; **partial attribution**: no GPU progress checkpoint, per-buffer allocation identity or full run/source receipt in this ring |

## Tests (not run)

**No tests, builds, compiler/version commands, shader tools or GPU initialization ran.**
No pre-fix red or post-fix green result is claimed. Added tests exercise the actual
production helpers/plans and include: reflected decoys/duplicates/remapped bank handles;
buffer overflow/alignment/range/alias/device limits; each planned scratch/descriptor
preparation position; private destination/resampler routing; replay order; guest-memory
aliasing; metadata invalidation on view/backing reuse; every descriptor construction
failure stage; original-map raw log decoding under late backing/address reuse; ring wrap,
contention and wrong/attempt-only retirement; taskless/total overflow and reflected payload,
shared/output allocation boundaries; oversized live payload rejection.

**Coverage limits:** CPU mock-owner tests do not inject ash driver failures, initialize a
Vulkan allocator, or prove every frame-level image/pipeline/counter failure path. Existing
`submission_safety` exact-receipt/partial-record/lock-reentry tests are retained. The full
captured guest transaction and actual raw-table descriptor cache/receipt integration have
not been executed. No test-only parallel implementation is substituted for those results.

Lead execution sequence, only after independent review and fresh advancing HWiNFO/no
competing-workload gates; explicit cwd `D:\Projects\RecompLauncherEngine`, one worker:

```powershell
cargo metadata --offline --locked --no-deps --format-version 1
cargo check -p graphics -p shaders --lib --tests --offline --locked --jobs 1
# Then the lead's thermally supervised production build, before test execution.
# Under that supervisor the engine build targets are:
cargo build --release --offline --locked --jobs 1 -p nxrs-cli -p front -p runtime

cargo test -p graphics --lib --offline --locked --jobs 1 vk::newndve::prepare::tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::newndve::kernels::planner_tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::newndve::trace::tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 frame_contract_rejects -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 production_metadata_cannot -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 mapped_construction_tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::gmem::tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::exec::mesh_contract::tests -- --test-threads=1
cargo test -p shaders --lib --offline --locked --jobs 1 mesh::payload_contract_tests -- --test-threads=1
# Affected CPU library suites, without gpu-tests or --ignored:
cargo test -p graphics -p shaders --lib --offline --locked --jobs 1 -- --test-threads=1
```

Focused commands are exact selectable targets, not a requirement to rerun the same tests
again after a passing affected-suite run. Graphics build.rs does invoke CPU shader compilation;
`cargo check` therefore also requires the execution gates. Existing ignored NDVE Vulkan
certification and feature-gated graphics GPU suites remain excluded.

## Verified (source/Git only)

- Required sources and final thermal dispositions, full plan, actual MIT reference host,
  relevant reference arithmetic-test source, actual renderer paths and official Vulkan mesh
  output/capability definitions read. No AGPL/GPL implementation copied.
- HEAD reconfirmed `143515097a0fa134c6fc4be839971692a39ac4de` with explicit project cwd.
  Scoped `git diff --check` passed at checkpoint (existing LF/CRLF notices only).
  Git does not include untracked sibling contents in that whitespace check.
- Source review found and corrected missing metadata fields in both old-image replacement
  owners, and ensured both private image owners transfer before any graph command.
- Final scoped Git whitespace check also passed; the status census still shows the
  pre-existing untracked NDVE/submission-safety files as untracked. Native trace retirement
  is published after post callbacks/transient cleanup/native pool recycling, not merely
  when the timeline wait returns. Canonical image admission also rejects multisampling,
  3D backing, scaling and undersized guest storage intervals.

Review-sensitive invariants: guest outputs never appear as native storage destinations;
preparation does not call side-effectful texture acquisition; native descriptor pools remain
submission-owned on later failure; private images are freed only in local pre-record failure
or post-completion callbacks; old raw table/log pairs are never rewritten; pool identity is
checked under the memory lock and again at writeback completion; fatal72 has no new logger,
driver query, flush or lock. Optional trace export is before submission, outside the ring lock.

## Unproven

Compilation compatibility and all test results remain unverified. D3 is a real activation
blocker: neither the host reference nor its arithmetic tests establish the full guest
dispatch/constant contract, immutable weight data or captured intermediate side effects.
The current raw handles/pool ownership architecture also still needs independent concurrency
review; no CPU helper pass can certify Vulkan lifetime. Native trace identifies CPU-issued
operations, not the last GPU-executed instruction. Exact MKW pair/live index proof, indirect
backend equivalence, shader capability coverage beyond the admitted forms and full shipped
provenance remain open. No global per-frame/in-flight NDVE memory budget was certified.
The existing `lost::checkpoint` recorder takes a blocking mutex and lazily grows its large
history; directly calling it from the new try-lock ring would invalidate the blocked-consumer
property. A nonblocking GPU-marker transport extension needs separate integration review;
this patch deliberately reports CPU-issued history rather than claiming that missing marker.

Both live gates remain unconditional: `Mode::parse` refuses on/1/check and host preparation
calls `live_ndve_permission` before allocation. No runtime shadow route or automatic
certification was enabled. Existing dist and all real game executables remain their prior
builds. No claim about submission 4764's cause, reboot/hypervisor causality, thermal safety,
GPU readiness or gameplay follows from this patch set.

## Slips

No execution/scope hold violation. Source work proceeded in coherent implementation chunks,
but the complete D3 contract and D14 attribution/provenance deliverables are not finished;
they are explicitly partial rather than reported as green. Two speculative documentation
URLs returned 404 before following the official reference-page link to the mesh formula.

## Questions

No product question is required to continue source review. Lead should specifically review
the explicit indirect-mesh refusal and the late-upload dependency/ownership proof before
the gated compile. Later activation requires missing guest-contract evidence, not a request
to bypass the quarantine. **Owned workloads: none started, none left running.** Read-only
Git commands completed; this is not a fresh machine-wide process/thermal census.

# GAR corrective source phase — file table before corrective edits

Read the complete independent `GPU-ALIGNMENT-REVIEW.md` (359 lines). Its six findings
supersede the affected completion claims above. Execution remains held; no compiler,
test, GPU workload or additional agent is authorized in this source phase.

| File | Corrective scope |
|---|---|
| `src/graphics/src/vk/exec.rs` | GAR-1 restore held indirect baseline plumbing only; GAR-2 captured raw cache identity; GAR-6 registration capture and revalidation |
| `src/graphics/src/vk/gmem.rs` | GAR-2 immutable pair token and GAR-5 production-boundary evidence |
| `src/graphics/src/vk/lost_bindings.rs` | GAR-2 actual descriptor-cache boundary API/tests (scope extension, before edits) |
| `src/graphics/src/vk/mesh_contract.rs` | GAR-4 recursive interface locations and parser-to-limit regressions; indirect argument-plan regression if needed |
| `src/shaders/src/mesh.rs` | GAR-3 advance wrapper admission identity |
| `src/shaders/src/prebuild.rs` | GAR-3 extract/use actual cache decision and add old-cache/admission regression (scope extension, before edits) |
| `src/graphics/src/vk/tables.rs` | GAR-6 atomic registration snapshots and production capture/version tests |
| `src/graphics/src/vk/newndve/prepare.rs` | GAR-6 preparation-boundary policy if required |
| `src/graphics/src/vk/queue.rs`, `submission_safety.rs` | Production upload-entry recording policy/scope regression; bounded GAR-5 baseline-plan assessment |
| `src/graphics/src/vk/mem.rs` | GAR-5 actual merge/identity boundary extraction only unless a safe bounded producer-provenance fix is established |
| `docs/waves/GPU-ALIGNMENT-PATCHES.md` | Append corrections, test-source coverage and truthful remaining blockers |

GAR-5 is not assumed solved by identity tracking: the pool bitmap reports destinations
only after producer execution, while the current baseline belongs to a later submission.
The correction must not snapshot every visible pool or overwrite concurrent CPU bytes.

## GAR checkpoint — corrective source and regression boundaries

- **GAR-1 scoped out as directed:** removed only the new blanket indirect fatal and
  restored the former `clamped_indirect(... task axes ..., total=0)` invocation before
  the existing indirect/count issue paths. Direct/taskless/checked-total admission stays.
  The source-shared `indirect_layout` now supplies the actual copy span/count bytes and
  DrawIndex stride; its CPU regression covers 1D, 3D and multi/count-buffer record layout.
  GPU-generated count/total validation remains unresolved, not rebranded as corrected.
- **GAR-2 corrected candidate:** `gmem::bindings` returns descriptors plus the exact
  immutable table/log token (generation and both staging spans). `stage_set` carries
  that token to `SetCache::store_version`; no new global load becomes descriptor identity.
  Publication refuses stale tokens and hits require the same token plus a current epoch.
  CPU tests exercise the actual production `SetCache` at resolution/capture/publication/
  lookup interleavings and distinguish a newly captured pair from an unencoded pair.
- **GAR-3 corrected candidate:** wrapper identity is `mesh 6`. Prebuild's actual hash,
  cache-hit, mesh conversion and publication decisions are shared with a CPU regression.
  A successful `mesh 5` cache entry with unchanged external translator/input misses;
  4097 reaches the actual wrapper failure and publishes no stale SPIR-V. 4096 runs the
  actual Rust wrapper on a tiny in-memory compute fixture. No shader tool/device is used.
- **GAR-4 corrected candidate:** interface-location recursion is separate from packed
  Workgroup bytes. Array lengths multiply element locations, including nested vec2 arrays.
  Occupied custom locations use interval union, bounded by declaration count instead of
  iterating huge declared arrays. Parser-to-validate fixtures require 69632 bytes for
  `float[256][16]` plus Position, rejecting 32768; exact boundary and nested vec2 cases added.
- **GAR-6 corrected candidate, shadow-only contract:** captures now contain the complete
  registered `view_t`, per-slot monotonic registration version, serial, ID and creation ID.
  Capture observes the image outside the table lock and revalidates the registration.
  `prepare_captured` checks all six semantic captures before preparation and a monotonic
  registry epoch afterward; execution rechecks before private issue and after guest replay.
  A change during preparation drops the unissued prepared owners. A change during replay
  withholds the shadow comparison/observation; it never causes a second guest replay.
  No table lock is held across image work or replay. Actual capture/prepare API tests cover
  mip, layer, swizzle, same-value re-registration, capture-window mutation and ABA during
  preparation, injected preparation error, and replay-window invalidation. This optimistic
  observation boundary does NOT claim to freeze arbitrary guest data or solve D3.
- **D5 coverage added:** actual `record_upload_entry` emits Begin then the same Vulkan
  dependency used by production. CPU recording-sink tests cover terminal transfer-read WAR,
  prove an end-pre dependency is too late, and retain the already-covered shader case.

### GAR-5 genuine blocker and narrowed D6 claim

**GAR-5 remains unresolved. D6 is not complete.** The correction would require producer
provenance absent from the current ABI: each raw write log is only one bit per table entry;
it has no byte/page delta, old value or producer before-image. `gmem::end` learns the range
after A completes; `gmem::begin` registers a normal writeback in B; the actual queue plan
copies B.pre and B.main. Both are already 1 for A's raw-only 0→1 store. The byte merge
therefore has no evidence distinguishing GPU-changed from concurrently CPU-changed bytes.
CPU write-watch generations and pool registration identity do not reconstruct A's baseline.

Capturing all possible mirrored destinations before A is not a bounded narrow correction:
the ABI can name 8191 nonempty ranges, each up to the existing 64 MiB whole-range writeback
limit. Two full captures at that envelope approach 1 TiB per table version. This is a
structural worst-case calculation, not a measurement or proposed allocation. Capping such
capture without a correct overflow/backend contract would either lose writes or refuse
otherwise valid work. Fine-grained producer delta logging requires translator/raw-store
ABI work and an overflow/CPU-merge design; that contract is not established in this wave.

The current behavior is retained, with no unconditional whole-copy shortcut or new capture
allocation. Extracted the actual queue pair-recording and byte-merge/identity decisions
for CPU boundary regressions. **Two explicitly ignored CPU-only known-failure tests assert
the required raw 0→1 result, with and without an unrelated CPU byte change. They are expected
to fail on this candidate if explicitly selected.** Companion active regressions cover
same-address replacement at scheduling/completion and normal explicit producer writeback.
No red/green execution is claimed. These tests preserve the owed behavior instead of
asserting the incorrect zero as success. Earlier D6 writeback-complete implications and
the general missing-pointer-backing claim are withdrawn: unbacked pointer-only targets
remain undiscovered; only known addressed granule-bind failure/table overflow refuse.

## Built — GAR corrective candidate

Source corrections are ready for a scoped re-review. **No build/deployment was performed.**
Disposition at this handoff:

| Finding | Current disposition |
|---|---|
| GAR-1 | Source regression removed by the explicitly authorized scope-out; prior indirect clamp/copy/count path restored. Full indirect correctness remains held/unresolved |
| GAR-2 | Captured immutable raw pair token reaches the production cache; epoch changes invalidate publication/hit rather than relabel descriptors |
| GAR-3 | `mesh 6` invalidates old wrapper successes; actual prebuild cache/conversion/publication boundary regression added |
| GAR-4 | Recursive interface locations and checked declaration-bounded interval accounting implemented; parser-to-device-limit fixtures added |
| GAR-5 | **Blocked/unresolved** for the producer-provenance reason above; D6 completion claim narrowed. Two explicitly ignored, expected-failing CPU regressions retain the required behavior |
| GAR-6 | Registration/view/version captured and revalidated around capture/preparation/private issue/guest replay. Re-registration or backing-generation change invalidates the shadow observation. Both live quarantine gates remain unconditional |
| D5 coverage | Production upload-entry sink regression added; accepted dependency placement/scopes retained |

The GAR-6 preparation token includes both the registry epoch and the existing image/backing
generation, so replacement without slot re-registration also invalidates preparation/replay
observation. It is an optimistic shadow-observation boundary: it does not serialize or undo
guest replay, and it does not claim that D3's mutable guest bytes are frozen.

Exact files changed **in this corrective pass**, preserving earlier source patches:

1. `src/graphics/src/vk/exec.rs`
2. `src/graphics/src/vk/gmem.rs`
3. `src/graphics/src/vk/lost_bindings.rs`
4. `src/graphics/src/vk/mesh_contract.rs`
5. `src/graphics/src/vk/mem.rs`
6. `src/graphics/src/vk/queue.rs`
7. `src/graphics/src/vk/tables.rs`
8. `src/shaders/src/mesh.rs`
9. `src/shaders/src/prebuild.rs`
10. `docs/waves/GPU-ALIGNMENT-PATCHES.md`

No new source file was needed in this pass. Planned `submission_safety.rs` and
`newndve/prepare.rs` extensions were unnecessary and were not made. The independent review
and lead-owned shared docs remain untouched. Descriptor/import cleanup, private output
routing, exact receipt retirement, zero borders, existing payload/fanout/primitive layouts
and logger-free fatal72 remain intact.

## Tests (not run) — GAR corrections

No compilation, tests, validators or external shader tools executed. The new regressions
are CPU-only; no Vulkan instance/device is constructed. The prebuild test calls only the
Rust wrapper with an in-memory fixture. The two known GAR-5 failures are explicitly ignored,
not presented as passing tests. All command examples below are future lead commands after
review, telemetry/concurrency gates and the authorized build.

Explicit cwd for all commands: `D:\Projects\RecompLauncherEngine`.

```powershell
# Gated metadata/type-check first, followed by the lead's monitored single-worker build.
cargo metadata --offline --locked --no-deps --format-version 1
cargo check -p graphics -p shaders --lib --tests --offline --locked --jobs 1

# After that build: narrowly selected new CPU regression sources.
cargo test -p graphics --lib --offline --locked --jobs 1 vk::exec::mesh_contract::tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::lost::bindings::raw_cache_boundary_tests -- --test-threads=1
cargo test -p shaders --lib --offline --locked --jobs 1 prebuild::regression_tests::mesh5_success_cannot_bypass_new_payload_admission_or_publish_stale_output -- --exact --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::tables::registration_boundary_tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::queue::upload_entry_tests -- --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::gmem::tests -- --test-threads=1
```

The affected library suites remain `graphics` and `shaders`. Do not enable `gpu-tests` or
broad `--ignored`. Existing shader build tests can invoke a validator when configured;
the new prebuild regression above has no environment-controlled tool invocation.

Optional **known-failure demonstrations**, only if lead chooses to record the unresolved
GAR-5 counterexample after the build. Each exact filter names a CPU-only test, and each is
expected to fail until producer provenance is implemented:

```powershell
cargo test -p graphics --lib --offline --locked --jobs 1 vk::gmem::tests::raw_only_producer_delta_must_survive_learning_in_a_later_submission -- --exact --ignored --test-threads=1
cargo test -p graphics --lib --offline --locked --jobs 1 vk::gmem::tests::raw_only_delta_must_preserve_an_unrelated_concurrent_cpu_write -- --exact --ignored --test-threads=1
```

## Verified (source/Git only) — GAR corrections

Full independent review read and cited production boundaries traced. Latest root authorization
re-read. Read-only `git rev-parse HEAD` returned
`143515097a0fa134c6fc4be839971692a39ac4de`; scoped `git diff --check` passed with existing
LF/CRLF notices only. Untracked files are not covered by Git's whitespace check. No compiler
or test result is inferred from source inspection. No source formatting, index/config edits,
staging, commit, push, package install, process stop or monitor-setting change occurred.

## Unproven — GAR corrections

Compiler compatibility and every regression result remain unverified. GAR-5 blocks complete
D6 writeback closure. Indirect GPU total-count validation is still the held baseline gap,
not fixed by removing the blanket refusal. GAR-6 revalidation is not guest data immutability
or a replacement transaction; D3 remains an activation blocker. Full source/tool/relink
provenance and GPU-executed fault attribution remain deferred. No thermal, reboot, hypervisor,
GPU fault or gameplay root-cause claim. Supplied malformed HWiNFO CSV/foreign-build status
is lead context, not a fresh census or permission to execute.

## Slips — GAR corrections

None against the corrective authorization. GAR-1's original behavior regression is contained
as explicitly directed. GAR-5 is reported as a real architecture/provenance blocker rather
than hidden by a test that asserts lost writes, an unbounded capture, or a false completion claim.

## Questions — GAR corrections

No product choice blocks scope re-review of GAR-1/2/3/4/6 and D5 tests. GAR-5 requires a bounded
producer-delta/overflow/merge design before claiming D6 complete; current source alone cannot
recover information that was never retained. **Owned workloads: none started, none left running.**

# GAR-R1 scoped follow-up — file table before edits

Read the scoped review and final GAR-R1 appendix through line 703. The accepted GAR-1/2/3/4/6
and D5 paths remain the baseline. This correction restores ordinary buffer cache invalidation
without relabelling captured raw descriptors or changing held execution paths.

| File | Intended narrow change |
|---|---|
| `src/graphics/src/vk/exec.rs` | Capture pool epoch at `stage_set` entry, before resource encoding, and carry it to cache publication |
| `src/graphics/src/vk/lost_bindings.rs` | Store/check ordinary encoding epoch independently of optional raw token; production SetCache regressions |
| `docs/waves/GPU-ALIGNMENT-PATCHES.md` | This scope, source proof, test names and remaining limits |

No build, compiler, CPU test, Vulkan/GPU execution or additional agent will run in this pass.

## Built — GAR-R1 source correction

**GAR-R1's cache counterexample is addressed in source; scoped independent recheck and
compiler/test acceptance remain pending.** Exactly the three tabled files changed in this pass.

Source proof:

1. `exec::stage_set` captures `POOL_GEN` with Acquire at function entry, before zero-buffer,
   bank, UBO/SSBO or other resource resolution. Every return carries this original
   `memory_generation` alongside the set, evidence and captured immutable raw token.
2. `bind_sets` passes that returned `captured_memory` into `SetCache::store_version`.
   Its fresh epoch load is only a validation input; it never replaces captured identity.
   Recomputing the ordinary binding-state key for descriptor chunk/image checks cannot
   relabel memory or the raw pair.
3. `CachedSet` stores a separate `memory_generation`. Publication requires both
   `captured_memory == current_generation` and, when present, the raw token's own epoch
   matching current generation. If resolution creates backing or a concurrent pool mutation
   occurs during encoding, that encoding is not cached. A subsequent stable encoding may cache.
4. Cache hits always require the stored ordinary memory epoch to equal current generation,
   **including `raw=None`**, plus the existing exact raw-token checks. Equal guest addresses,
   binding-state key and descriptor chunk therefore cannot reuse an old ordinary backing
   after pool replacement. A current raw pair cannot rescue stale ordinary descriptors,
   and a current ordinary epoch cannot relabel an old raw pair.

This is conservative cache admission/invalidation, not serialization of guest pool changes
with command issue. Existing ownership deferral still retains an already-recorded backing.
The classic-set path carries/ignores the extra return metadata as appropriate; its existing
descriptor-content memo remains unchanged. No held indirect, NDVE, writeback, barrier, shader
translation or allocation behavior was reworked.

## Tests (not run) — GAR-R1

Three new CPU-only regressions exercise the actual production SetCache methods:

- `ordinary_buffer_without_raw_token_hits_only_its_captured_pool_epoch`
  — stable ordinary hit, same-key replacement miss, new stable encoding hit.
- `ordinary_encoding_mutation_cannot_publish_old_backing_as_new_epoch`
  — a mutation between the pre-encoding capture and publication rejects both old/new-epoch
  hits; a later stable encoding recovers normal caching.
- `raw_pair_and_ordinary_encoding_epochs_are_independent_admission_checks`
  — stale ordinary/current raw and current ordinary/stale raw both refuse; matching captured
  identities hit, and later replacement misses.

The existing `captured_pairs_cannot_be_relabelled_during_resolution_publication_or_hit`
test retains its raw-pair interleavings and now passes a separately captured pre-encoding
epoch. Its during-resolution mutation must conservatively decline caching even if the raw
pair was captured afterward; a subsequent stable encoding still succeeds. Existing generic
cache/evidence tests retain their zero-epoch convenience APIs.

**None compiled or ran.** No Vulkan initialization or external tool is called by these tests.
Future focused command, only in the lead's gated post-build CPU-test phase, cwd
`D:\Projects\RecompLauncherEngine`:

```powershell
cargo test -p graphics --lib --offline --locked --jobs 1 vk::lost::bindings::raw_cache_boundary_tests -- --test-threads=1
```

## Verified (source/Git only) — GAR-R1

Read the final review appendix, actual `stage_set`/`bind_sets` and SetCache boundaries;
checked all `store_version` call sites and all stage-set return/destructure sites after
the signature update. Read-only Git reconfirmed HEAD
`143515097a0fa134c6fc4be839971692a39ac4de`. Scoped `git diff --check` passed with the existing
LF/CRLF notice for `exec.rs`; `lost_bindings.rs` remains pre-existing untracked work, so Git's
diff check does not validate that file's contents. No index, configuration or unrelated
source changes. Only this owned report was appended; the independent review was not edited.

## Unproven — GAR-R1

Compiler compatibility and regression results are unverified; no green claim. GAR-5 remains
open/separable and both ignored known-failure tests remain visible and unchanged: D6 is not
complete. The indirect baseline remains held, D3 remains incomplete, and both NDVE quarantine
gates remain unconditional. The malformed monitor CSV and competing build still prohibit
execution under the supplied lead status; no fresh telemetry/process census was performed.

## Slips — GAR-R1

None. The scoped cache correction did not reopen accepted unchanged paths.

## Questions — GAR-R1

None. Ready for the requested narrow source recheck, not execution permission.
**Owned workloads: none started, none left running.**
