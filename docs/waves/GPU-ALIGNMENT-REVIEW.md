# Independent GPU alignment source review — 2026-11-23

## Built

**Final disposition: CHANGES REQUIRED. Independent source review completed within the assigned scope;
no compilation, CPU-test, GPU or build authorization follows.** Checkpoints 1 and 2 were saved during review.
Baseline supplied by lead: HEAD `143515097a0fa134c6fc4be839971692a39ac4de` plus extensive dirty work.
This review writes only this report with `apply_patch`; source/handoff changes are preserved.
Read root AGENTS, binding delegation protocol, architecture guide, the complete 564-line reference plan,
and the implementation handoff. Findings concern actual code paths, not attribution of the whole dirty diff.

Path convention below: bare renderer filenames are in `src/graphics/src/vk/`; `prepare.rs`,
`private_image.rs`, `planner_tests.rs` and `trace.rs` are in its `newndve/` directory.
Shader-wrapper `mesh.rs`, `prebuild.rs` and `xlate.rs` are in `src/shaders/src/`.

### Confirmed patch blockers

- **GAR-1 / High — valid indirect mesh draws now terminate the process.**
  `src/graphics/src/vk/exec.rs:1318–1322` unconditionally calls fatal72 for `m.ind != 0`, before the
  existing argument-copy/draw paths at `1358–1429,1476–1494`. A valid indirect `{1,1,1}` draw with
  valid stage contracts is sufficient; no invalid count is needed. This violates plan P1-C's explicit
  complete-valid-work requirement (`GPU-REFERENCE-PLAN.md:423–426`). Owed implementation is a correct
  indirect validation/backend path, not blanket rejection or unsafe clamping. Merely deleting the
  new abort also needs care: the old clamp invocation has been removed from the retained branch.
  **Requested correction:** preserve valid direct/indirect/count/stride/first semantics through a
  correct validation/backend path. If that cannot be completed in this wave, split out the incomplete
  indirect behavior change and retain the prior implementation **under the existing execution hold**;
  that is scope containment, not approval of the old clamp or certification of unknown GPU-generated
  arguments. A CPU read of the guest argument address is not proof of its GPU contents.
  **Regression:** production mesh decision/argument-planning tests must admit valid indirect 1D,
  3D and count-buffer cases, preserve first/DrawIndex stride, and reject genuinely invalid totals
  without clamping valid work. A test expecting unconditional fatal72 would encode the regression.
- **GAR-2 / High — raw descriptor cache can mislabel a captured version.**
  `exec.rs:3720` captures table/log G, but `set_key` reads live `POOL_GEN` (`:248`) and the cache stores
  a newly recomputed key after construction (`:4097–4101`). A pool add/replacement between capture and
  that store tags G's descriptors G+1; the next identical draw hits and skips `gmem::bindings`, so it
  keeps the old map despite G+1. Use the captured pair's immutable identity and invalidate/retry when
  the current map differs; exercise the actual cache integration with an injected epoch change.
  **Reachability:** `mem::pool_add` publishes and advances the epoch under `g.mem` (`mem.rs:583–590`)
  without taking the replay's set-cache lock. `set_key` filters only image-generation changes on the
  second read, not a raw-generation/captured-version mismatch. An unrelated newly registered pool
  is enough to make the second key differ without changing `image_gen`. If the intervening change
  replaces a backing, the next draw can keep that stale address map under the new epoch. This is a map/cache counterexample,
  not a claim of immediate use-after-free: release deferral retains the old backing for this Sub.
  **Requested correction:** return/store the actual table/log generation or version token alongside
  the descriptors; use that token in the cache and compare with the current map before permitting a
  hit. Do not re-label captured descriptors with a fresh global load. Cover changes both during
  resource resolution and between paired capture and cache publication, using the production cache.
- **GAR-3 / Medium, blocking integration — oversized-payload rejection does not invalidate cached mesh-5 output.**
  `src/shaders/src/mesh.rs:31,131–136,487` retains `VERSION = "mesh 5"` while adding the rejection.
  `prebuild.rs:700–738` reuses the old successful shader before `to_stage` runs; its identity includes
  the external translator binary, not this changed Rust wrapper (`xlate.rs:137–152`). The supplied
  baseline already included mesh-5. An oversized-prefix shader previously truncated under that
  identity is still reused. Advance the wrapper identity and test the actual cache-hit boundary.
  **Requested correction:** change the mesh-wrapper cache identity for the new admission semantics,
  retaining the 4 KiB allocation, zero initialization and fanout 32. Test an old successful cache
  record for payload 4097: it must miss and propagate the new failure; 4096 must still translate.
  Fresh failures already propagate through `prebuild.rs:860–870,894–895,923–927,960–964`; the missing
  invalidation occurs before those branches. This is narrower than deferred comprehensive D12 provenance.
- **GAR-4 / Medium, blocking safety-check correctness — output array location accounting understates memory.**
  `mesh_contract.rs:101–116` calculates locations as `ceil(element_bytes/16)`. For an output whose
  outer array is per-vertex and whose element is `float[4]`, that gives one location, but the four
  scalar array elements consume four locations. Recursively calculate interface locations (distinct
  from tightly packed Workgroup bytes); test nested scalar/vec2 arrays through `reflect` + `validate`.
  Khronos mesh-output source confirms that memory uses effective locations, not packed byte count.
  **Counterexample:** use an interface declaration `float attribute[256][16]` plus Position, with
  OutputVertices 256, vertex granularity 32, output-memory limit 32768 and otherwise sufficient limits.
  The 16 scalar-array locations plus Position require `(64 + 4) * 4 * 256 = 69632` output bytes;
  the parser counts `(16 + 4) * 4 * 256 = 20480` and incorrectly admits it (`mesh_contract.rs:145–157`).
  This is a structurally valid interface that exceeds this device's resource limit and should be refused.
  The parser expressly accepts nested `OpTypeArray` through `bytes` (`:69–70`); this is not merely an
  unsupported struct being refused. No claim that the retained MKW pair has this layout. Current
  wrapper-produced generic outputs are flat vec4 arrays (`mesh.rs:674–708,717–736`).
  **Requested correction:** a checked recursive interface-location calculation, separate from
  scalar stage-memory bytes. Regression fixtures must pass actual instruction/type/decorations
  through `reflect`, then test device boundary decisions; manually constructing `Stage` misses this bug.

### Additional concrete D6 integration gap (not attributed as a newly introduced regression)

**GAR-5 / High — learned raw-only writes can disappear at the delayed writeback boundary.**
`gmem.rs:238–256` learns a raw write after A completes and schedules its copy in B.
`queue.rs:632–672` takes B's before-image after A's GPU result already exists. If B does not write
that range, before == after; `mem.rs:2089–2090,2128–2144` consequently lands no bytes. Example:
guest byte 0, A raw-only store 1, no ordinary writeback registered in A, B has only the learned
writeback: both B snapshots are 1 and guest remains 0. Original-map log decode is not an end-to-end
writeback test. Preserve the producer's relevant baseline/dirty provenance and the CPU merge policy;
do not solve this with unconditional stale whole-range copies.

Detailed path: a pointer-only destination does not get `note_gpu_write` in A's ordinary descriptor
loop (`exec.rs:3383–3390,3719–3725`). A's completion callback decodes its original map and appends
the identity-tagged range to WRITTEN. `queue.rs:1553–1558` later invokes `gmem::begin` in B, which calls
`note_gpu_write_version` (`mem.rs:1925–1953`); with the same pool identity it registers the copy normally.
Both B baseline and B terminal readbacks then contain 1. This witness assumes successful baseline
allocation, no lazy pages and no intervening CPU/GPU store; it needs neither OOM nor a generation race.
The identity guards do not repair the lost producer delta. If the CPU changes the page before the
learning submission, the producer-result preservation problem is stronger, so a whole-copy shortcut
would be wrong too.

**Requested correction / regression:** carry enough producer-version/baseline or dirty-range evidence
to land the learned write correctly, keeping unchanged concurrent CPU bytes. Test the actual production
learn → schedule → baseline/after → merge decision across A/B, with (1) raw-only GPU 0→1, (2) unrelated
CPU byte changed meanwhile, (3) same-address replacement identity, and (4) ordinary explicit writeback.
GAR-5 blocks declaring D6 writeback integration complete; it is an existing-path defect exposed by this
review, not evidence that this source agent introduced all of that machinery.

### GAR-6 / Medium — captured NDVE operands do not freeze the registered view (quarantined D2 gap)

`exec.rs:5640–5645` discards the `Option<view_t>` returned by `tex_at`, retaining only ID/serial/image
creation. `ndve_image_binding` later fetches the **current** registered view (`:5723,5735–5742`).
A slot captured with a nonzero-mip or swizzled view can be re-registered to the canonical view of the
same texture/image before preflight. Serial and creation ID remain equal, `POOL_GEN` need not change,
and preflight accepts a view the captured operand did not have. Re-registration is an actual path
(`image.rs:1948–1988`, `tables.rs:613–626`), not numeric VkImage reuse speculation. A change after one
operand's observation can also split the native read from replay's current registration.

**Requested correction:** capture registered view interpretation and a registration version, validate
them with creation identity, and establish a stable snapshot/revalidation boundary through preparation
and replay. Respect existing lock order; do not hold the table mutex while acquiring images, because
image paths call table methods under the images lock. Test same ID + same serial + same creation with
changed mip/layer/swizzle/registration, including a change during preparation. The existing metadata
test only mutates the backing/view cache, not this production capture boundary.

This is a concrete incomplete D2 implementation, **not a live execution regression while both quarantine
gates remain**. It joins the already documented D3 activation blockers; it is not a reason to run GPU tests.

### Accepted source properties and bounded dispositions

#### NDVE private preparation and lifetime

- **Semantic roles are correctly mapped for the admitted driver-bank forms.** The operand helper uses
  texture word `8 + 2*r` and image word `72 + 2*r` (`prepare.rs:48–58`), matching `c2_bytes` at
  `exec.rs:2495–2507`, not Vulkan binding ordinals. Packed bank `0x30002` selects low bank 2, as
  `tex_slots` does (`:2516–2521`). Production selection is first TEX1, terminal TEX4/IMG0, resampler
  TEX0/TEX3/IMG1 (`:5794–5877`), matching MIT nxnative's named roles. Duplicate conflicting roles
  refuse. Non-driver-bank forms refuse rather than guess. GAR-6 concerns the selected view's version.
- **Authoritative metadata improved:** `image.rs:270–300,1880–1882` uses backing-owned metadata,
  validates current cache membership/image, clears it on replacement, and always assigns creation IDs.
  Inspected swizzle-reuse invalidations (`:1493–1501,1551–1558`), release (`:2043–2051`) and both
  replacement/follower paths (`:2314–2371,2499–2561`). No diagnostic-ring eviction can manufacture
  production metadata. Canonical admission refuses nonzero mip/layer, nonidentity swizzle, multisample,
  3D, sparse, scaled, alias, cold and replaced images (`exec.rs:5727–5742`). Guest intervals are checked
  for overflow, minimum storage size and pairwise overlap (`newndve.rs:240–300`). Image creation ID
  is physical-image identity, not immutable guest byte/data version or a per-pool allocation receipt.
- **No guest-output storage destination in the new graph:** `ndve_host_graph` allocates two private
  images and wires only their views into terminal/luma/resampler jobs (`exec.rs:6024–6063`;
  `kernels.rs:301–338`). Luma and resample sample the private network output. The real shader accesses
  agree (`shaders/ndve_out.comp:37–55`, `ndve_resample_image.comp:59–66`): the nominal network-output
  buffer is not read instead of the image on those image paths. Copies of private results happen
  after graph recording (`exec.rs:5994–5996`), then guest work runs and comparison copies guest outputs.
- **All fallible native Vulkan preparation inspected precedes native commands:** scratch, both image/
  memory/view allocations, kernel pipelines/sampler/layouts, tensor/dummy/luma scratch, graph descriptor
  pools/sets, comparison scratch/counters and both compare descriptors finish by `exec.rs:6065–6070`.
  `prepared_span` is read-only and checks the actual owning buffer extent (`mem.rs:1354–1366`);
  `ndve_image_binding` does not call `tex_view`, upgrade storage or publish guest write intent.
  This statement covers native graph preparation, not ordinary guest replay's later allocations or
  fallible submission-tail readback construction. CPU Vec/Box allocation remains conventional abort-on-OOM.
- **Ownership/order:** scratch belongs to the current Sub as allocated; native pools register before
  issue (`kernels.rs:567,673`); counters enter `sub.transient` before use (`exec.rs:6087–6091`). Both
  private image owners enter that same Sub's callbacks before the first image transition (`:5988–5991`).
  Partial image-view creation failure drops image then memory locally (`private_image.rs:41–46,69–75`).
  No fallible Result-returning native recording remains after the prepared boundary. Guest snapshots
  are drained once in order and final guest dispatch once on each normal refusal/success branch
  (`exec.rs:5707–5711,6196–6231`). Native stores do not mutate guest outputs before a fallback.
- **Exact receipt on the actual production path:** `Inflight` owns the Sub and its timeline; completion
  constructs its wait from that exact value (`queue.rs:814–837`). Failure terminates without unwinding;
  only success reaches callbacks/transient release/pool recycle (`:873–944`). An earlier timeline
  value cannot satisfy this wait. The standalone descriptor-pool wrong-receipt test proves its helper,
  not a private-image failure-injection integration test; no actual wrong-fence RAII drop path was found.
- **D3 stays OPEN:** native grids and buffer bounds are improved, but guest exact grids/local sizes,
  all constants, immutable weights and intermediate side effects are not proven. `validate_sequence`
  still accepts arbitrary nonzero direct grids (`newndve.rs:323–340`); the constants check inspects only
  selected words (`:305–310`). Pool epoch/creation ID is not a data-version snapshot. There is no
  global native frame/in-flight byte budget. None of these is fixed by compiling the code.
- **Quarantine retained:** `Mode::parse` always returns Off or refuses enabling modes (`newndve.rs:352–369`),
  and `ndve_host_graph` independently calls the unconditional denial before allocation (`exec.rs:6019`;
  `submission_safety.rs:169–178`). No automatic one-frame promotion remains.

#### Raw tables, backing ownership and writeback

`gmem.rs:151–185` closes the old log and creates fresh table/log staging when the epoch changes;
`end` captures original ranges/identities, never the mutable latest TABLE. Log decoding checks both
arrays with `get` and bounds the range (`:203–218`); no OOB bit-decoding witness found. Capacity overflow
is fatal (`:63–65`); granule binding failure is fatal (`:164`). Highest-base overlap clipping remains
at `:74–91`, including holes shadowed by an unbacked higher-base pool.

**The handoff's missing-backing statement needs qualification:** a pool with `da == 0` still supplies no
range (`mem.rs:834–836`), and only already-addressed granulated pools enter the full-backing loop
(`gmem.rs:158–164`). No general refusal/discovery of every pointer-only unbacked target was added.
Do not describe this as complete pointer-target residency proof. Do not replace it by an unbounded pointer scan.

Versions themselves contain handles/maps, not Arc backing owners. Actual retention remains the existing
queue policy: `release` moves all window buffers, sparse allocations and the primary buffer into
`defer_after_use` (`mem.rs:609–632`); queued/open work is retained (`queue.rs:248–262`), callbacks are
assigned the just-submitted timeline (`:760–766`), and deferred frees run after the corresponding
completion (`:958–973`). This supports the inspected single-queue/open-recording lifetime; no concrete
new early-free path was established. It does not repair GAR-2's semantic stale-map reuse or GAR-5's lost
CPU writeback. Same-base replacement is checked under the memory lock when scheduling and again when
landing (`mem.rs:1945–1953,1992–1998`), and an identity mismatch does not prevent INFLIGHT removal
(`:1989–1990`). New log tests do not exercise these production guards or queue retention.

#### D5 upload-entry dependency — specific witness accepted

1. A records its terminal mirrored-buffer readback reads after the last ordinary shader/pass barrier
   (`queue.rs:623,651–657`). Choose no texture-writeback tail, no sparse/present wait and no positive
   CPU completion before recording/submitting B. A's final barrier targets HOST reads (`:684–693`),
   not B's transfer writes. Its timestamp does not supply the missing memory/execution dependency.
2. A mirror can be CPU-cold (>60 frames) despite A's GPU store: GPU writeback registration does not
   update `dirty_frame`. B mid-frame can skip it in `begin_submit` (`mem.rs:1772,1856–1857`). A CPU
   write discovered later by COPY_B2T reaches `sync_range` (`exec.rs:7178–7181`), which directly calls
   `Sub::upload` without INFLIGHT splitting (`mem.rs:754–784`). Ordinary begin-submit ranges instead
   use the guard (`:1873–1889`). Choose a ready destination texture so no unrelated `pre` barrier
   precedes this upload. `Sub::upload` records transfer writes into B.pre (`queue.rs:359–377`).
3. The added `barrier_all(g, s.pre)` immediately after begin-pre (`queue.rs:556–562`) therefore closes
   the A terminal-read → B upload-write execution hazard. ALL_COMMANDS in both stage scopes includes
   the earlier transfer read even though the source access mask is MEMORY_WRITE: this is a WAR
   execution dependency. Its memory scopes additionally cover prior writes. B end-pre and main-entry
   alone were too late. No queue-idle, wait-budget or ordinary receipt change was added.

The generic prior-shader → upload witness remains withdrawn: existing post-shader/pass barriers can
already cover it. The accepted witness is not historical fault attribution. The new barrier also
does **not** merge stale CPU page bytes with A's GPU result; that is a separate data-coherence issue
in the pre-existing late-upload bypass, not proof that adding this barrier regressed coherence.
The planned actual recording-order/scope CPU regression was not found in the patch's tests.

#### Cleanup, mesh contract and trace

- **D13 cleanup accepted for inspected branches:** descriptor type/allocation failure destroys only
  the created buffer; bind/map failure destroys buffer then frees allocation; a failed map is not
  unmapped (`tables.rs:63–96`, `submission_safety.rs:186–221`). Imported-memory bind failure uses the
  same ownership discipline (`mem.rs:1294–1318`). Success transfers ownership; imported guest pointers
  were not vkMapMemory mappings and are not spuriously unmapped. No new allocator-mutex reentry or
  duplicate-free path found. Existing `Gpu::buffer` cleanup/accounting remains (`mod.rs:182–219`).
- **Mesh portions that hold:** direct checked axis/product/total validation chooses task or taskless
  mesh properties (`exec.rs:1089–1104,1461–1473`); scalar fixed Workgroup/TaskPayload arrays are counted
  with checked multiplication and addition (`mesh_contract.rs:63–89`), explicit stride/unknown layouts
  refuse, task/mesh payload sizes must match (`exec.rs:1311–1314`). Fixed initialized 4096-byte payload
  and fanout 32 remain (`mesh.rs:128–152,640–647,852–909`). None proves dynamic stage-memory indices.
  Module cache keys are immutable runtime module IDs and words are post-rewrite (`shader.rs:1224–1337`),
  which is appropriate in the actual process-global Gpu/Library (`mod.rs:89–95,126–137`). The cache and
  static device limits are not keyed for multiple/recreated devices; no such production lifecycle was
  found, so that is not promoted to a reachable regression. The parser does not select an entry point
  when reading execution modes and is not general SPIR-V validation. Extend admitted shapes carefully;
  exact MKW pair/index evidence remains missing. GAR-1, GAR-3 and GAR-4 are the concrete corrections.
- **D14 partial accepted as CPU-issued history:** fixed 256 slots, unique nonwrapping IDs until exhaustion,
  overwrite/omission counts, try-lock record/link/retire/snapshot, serial+timeline checks and attempted
  versus submitted state are present (`trace.rs:25–75`). Link or retire contention can lose metadata;
  omission counters mean unknown, not GPU failure/completion. Native retirement is emitted after Sub
  callbacks/transients/pools, but before the separate deferred-free list (`queue.rs:944,958–973`), so it
  cannot certify retirement of every guest backing allocation. No GPU-executed marker, per-buffer
  allocation receipt or full source/run identity is claimed.
- **Optional logger can block normal submission:** `queue.rs:744–753` snapshots outside the ring lock,
  then logs while holding the Vulkan queue/optional driver locks. This is bounded record volume, not
  bounded sink latency. Completion timeout for this not-yet-submitted item has not started; an external
  supervised wall-clock/stall watchdog is still necessary. Fatal72 itself remains immediate and
  logger/query/lock-free. This is documented optional diagnostic overhead, not a newly introduced
  fatal-path wait or proof of the root fault. No telemetry qualification was performed by this review.

### Minimal return-to-implementer list

1. Correct/scope out the blanket indirect fatal regression (GAR-1), preserving the held baseline if
   the complete backend is deferred; do not leave an unvalidated branch by simply deleting the abort.
2. Tie raw descriptor-cache identity to the pair actually captured and test epoch interleavings (GAR-2).
3. Advance mesh-wrapper cache identity for payload rejection and test cache/error propagation (GAR-3).
4. Correct recursive mesh output location accounting with parser-to-device-limit fixtures (GAR-4).
5. Close the producer-baseline/raw-learning writeback gap before declaring D6 complete (GAR-5).
6. Freeze NDVE registered views as well as image creation before any future activation (GAR-6); keep
   D3 and both quarantine gates explicitly open/active. Add missing frame-level preparation failure
   coverage without enabling GPU execution.

## Tests

**Not run.** No build, test, compiler, shader/Vulkan/GPU tool, probe or worker launched.
No pre-fix red or post-fix green was observed. Test source was reviewed as follows:

| Source tests | What they actually exercise / missing boundary |
|---|---|
| `newndve/prepare.rs:81–121` | Real operand/buffer helpers; generic collect/drop and replay loop. Does not call capture, frame preparation, private-image allocator or guest dispatch integration. |
| `newndve/planner_tests.rs:18–55` | Actual `graph_jobs` routing and buffer validator; failure positions use plan lengths with generic owners. Does not inject image, pipeline, counter or ash descriptor failures into `PreparedNdveFrame`. |
| `newndve/tests.rs:184–227` | Mode refusal and real FrameResources interval validation. Does not freeze registration versions or invoke authoritative accessor against changing tables. |
| `image.rs:444–460` | Real cache/creation metadata invalidation; not capture-to-preflight re-registration (GAR-6). |
| `gmem.rs:959–973,999–1051` | Actual encode/visible-range/log-decode helpers, old-map log meaning, overlaps/windows. No production cache epoch interleaving, backing-owner receipt or learned writeback merge. |
| `submission_safety.rs:224–248,374–464,476–582` | Actual cleanup and completion/pool helpers, ordered once-only frees, mutex reentry, wrong receipt, finite timeout and quarantine. Preserved valuable coverage, not end-to-end private-image retirement. |
| `mesh_contract.rs:167–195` | Grid helper and small instruction fragment for scalar storage; output test manually constructs Stage. No nested-output reflection, selected-entry, valid indirect dispatch or actual task/mesh pair fixture. |
| `mesh.rs:138–148` | Real payload bound helper; no prebuild cache-hit/error publication boundary. |
| `trace.rs:80–107` | Actual ring wrap, wrong/attempt-only receipt and producer contention. No executed GPU progress attribution, as documented. |

The new CPU helper tests are useful but do not satisfy every preparation position or all integration
claims by themselves. Preserve them and add only the missing production-boundary regressions above.
The existing NDVE Vulkan certification test is ignored (`newndve/gpu_tests.rs:91–96`); no ignored test
was run. Future broad shader tests can invoke external validation when configured (`mesh.rs:1314+`);
the lead must still select genuinely authorized execution after its fresh supervision gate.

## Verified

HEAD was independently reconfirmed with read-only `git rev-parse HEAD`. Narrow Git diffs of mesh and
tables were inspected solely as context; their complete dirty deltas are not this assignment's changes.
Read-only inspection includes NDVE capture/preparation/record/replay, native graph planning,
private image ownership, queue submission/completion, raw table/log construction and decode,
pool registration/release and writeback identity, mesh admission/parser, descriptor/import cleanup,
and nonblocking native trace.

| File/scope read | Review result / access |
|---|---|
| `AGENTS.md`; `docs/31-DELEGATION-PROTOCOL.md`; `docs/ARCHITECTURE-AND-STYLE-GUIDE.md` | Read latest scoped authority; no execution permission inferred from general commands. |
| `docs/waves/GPU-REFERENCE-PLAN.md` (all 564 lines); `GPU-ALIGNMENT-PATCHES.md` (all 294 lines) | Read plan and actual implementation claims; incomplete D3/D14 and indirect change explicitly distinguished. |
| `docs/waves/THERMAL-REVIEW.md:496–558` and closed-disposition anchors | H1–H4/E1–E2/R1 closures retained. No broad harness audit reopened. |
| `src/graphics/src/vk/exec.rs` | Capture/roles/view/prepare/replay, native compare, mesh admission/draw, raw cache/bindings, COPY_B2T call paths. |
| `src/graphics/src/vk/gmem.rs` | Version/build/bind/end/begin, log decode, overlap helpers and relevant tests; probe context read. |
| `src/graphics/src/vk/image.rs` | Production metadata, creation/replacement/eviction/release/registration paths and tests. |
| `src/graphics/src/vk/mem.rs` | Pool identity/release, buffer ranges, sparse binding, import, prepared spans, cold uploads, INFLIGHT and actual landing. |
| `src/graphics/src/vk/queue.rs` | Sub fields, staging/upload, entry/tail barriers, submit/exact completion/deferred retirement and run_stream order. |
| `src/graphics/src/vk/tables.rs`; `submission_safety.rs` | Descriptor cleanup/snapshot/registration context and real cleanup/completion/allocation helpers/tests. |
| `src/graphics/src/vk/mesh_contract.rs` | Complete parser, validation and tests; GAR-4. |
| `src/graphics/src/vk/newndve.rs` | Module wiring, image/frame/sequence contracts and quarantine. |
| `src/graphics/src/vk/newndve/{kernels,prepare,private_image,trace,planner_tests,tests}.rs` | New production planning/recording/owner paths and tests inspected. |
| `src/shaders/src/mesh.rs`; `prebuild.rs`; `xlate.rs` | Payload boundary, actual generated interfaces, cache identity and failure propagation. |
| `src/graphics/src/vk/{mod,shader}.rs`; `src/graphics/shaders/{ndve_out,ndve_resample_image}.comp` | Allocator/singleton/module identity and actual native image accesses. |
| `build/Q/nxnative-up/dm/gfx/nvn/nvn_ndve.inc:95–255`; Ryujinx `CodeGen/Spirv/Declarations.cs` relevant memory declarations | MIT reference role/private-output behavior and scalar shared-array source; read-only, no source copied. Reference behavior is not a universal specification. |
| Khronos Vulkan-Docs `chapters/VK_NV_mesh_shader/mesh.adoc` | Read-only HTTPS source fetch for effective-location output-memory formula; no tool installation/execution. |
| `docs/waves/GPU-ALIGNMENT-REVIEW.md` | Sole file written, with apply_patch. |

Read-only commands executed from `D:\Projects\RecompLauncherEngine`: `git rev-parse HEAD`,
`git diff -- src/shaders/src/mesh.rs`, and `git diff --unified=5 -- src/graphics/src/vk/tables.rs`.
HEAD matches the supplied identity. Git reported existing LF/CRLF notices; no index/config/source operation
was requested. Turbo was not used as an implementation donor or independently re-audited; its behavioral
comparison is attributed to the already-read plan. No new gameplay/incident evidence was generated.

**Compile/API assessment:** no definite new symbol/field/tuple mismatch was found in the inspected
call sites (including six-field writebacks and GraphBuffers initializers). This is not compiler
acceptance: no Cargo, rustc, build.rs, validator or test invocation ran. The new helper parser tests
also cannot certify SPIR-V validity or actual device capability coverage.

## Unproven

Compilation, CPU test results, GPU execution and gameplay remain unverified. GAR-1–4 require source
correction before accepting the patch; GAR-5 prevents complete D6 writeback closure; GAR-6 and documented
D3 prevent NDVE activation. D7 placement, exact MKW pair/dynamic index proof, full D12 provenance and
GPU-executed D14 progress attribution remain future evidence/engineering work, not invented compile fixes.

No finding proves the cause of timeline 4764, MKW's recorded invalid read, reboot, hypervisor error or
thermal incident. The owner/lead's HWiNFO-running, malformed-CSV and competing Leonida-build reports
are supplied context, not a fresh machine census by this reviewer. Healthy monitoring has not been
qualified here. Existing workload, fault-latch, save and library holds remain in force.

## Slips

None. No source writes, tests, builds, probes, installs, settings/save/library/latch changes or subagents.
Read-only Git commands and a specification-source fetch were used; no execution workload was created.

## Questions

No unresolved product choice is needed to return these findings to the original implementer. P1-C
already requires correct valid-work behavior; it does not require asking the owner to accept blanket
indirect refusal. Recommendation: correct the bounded source defects, re-review those changed paths,
then let the lead determine whether fresh telemetry/concurrency permits its authorized supervised build
and subsequent CPU tests. This report grants neither execution nor GPU readiness.

**Owned workloads: none started, none left running.** All read-only Git commands completed. Foreign
processes were not inspected, stopped or otherwise affected.

---

## Scoped independent GAR recheck — 2026-11-23

### Built — source reviewed, not compiled

**Disposition: GAR-1 closed by authorized scope containment; GAR-2/3/4/6 closed for their original
source counterexamples. GAR-5 remains OPEN (High), a separable existing raw-writeback defect. D5's
source-coupled recording regression is accepted. No additional concrete corrective-pass regression
or definite compile/API mismatch was found in this scoped recheck.**

This appendix supersedes the earlier open dispositions only to the extent stated below; earlier
findings and evidence are preserved. Read the corrective handoff at `GPU-ALIGNMENT-PATCHES.md:296–493`
and the actual changed production/test boundaries. The original incident, harness, allocator and
whole-engine reviews were not repeated. H1–H4/E1–E2/R1 closed dispositions remain unchanged.

**Source-review acceptance permits handing this candidate to the lead's already-authorized, gated
CPU compilation/build and subsequent CPU-test phase. It is not permission to execute now, deployment
acceptance, complete D6 closure, or GPU/production readiness.** The supplied malformed HWiNFO header
(928 fields versus 930 data fields), absent requested `cpu_reading_live.CSV`, and competing four-worker
Leonida build remain execution blockers until the lead independently qualifies the required gates.
No monitor/process state was queried by this reviewer.

| Finding | Recheck disposition | Actual correction / remaining boundary |
|---|---|---|
| GAR-1 | **Closed by scope-out** | Blanket indirect fatal removed; prior copy/clamp/count plumbing restored. GPU-generated total/taskless indirect correctness remains unresolved under hold. |
| GAR-2 | **Closed, source level** | Immutable pair token travels with the encoded descriptors; stale publication/hits refuse rather than relabel an old map. |
| GAR-3 | **Closed, source level** | `mesh 6` changes the real prebuild hash; old success misses and fresh wrapper failure cannot publish old SPIR-V. |
| GAR-4 | **Closed, source level** | Recursive interface-location counting fixes nested scalar/vector arrays; parser-to-limit fixtures cover the exact counterexample. |
| GAR-5 | **OPEN — High; not fixed or green** | Raw log still learns after producer completion; later baseline loses producer delta. Ignored tests preserve expected failing behavior. Separable from the version/cache corrections. |
| GAR-6 | **Closed for captured-view hole, source level** | Registered interpretation/version captured and revalidated around preparation and replay observation. This remains optimistic shadow validation, not immutable guest execution or D3 closure. |
| D5 coverage | **Accepted, source level** | Production upload-entry sink and exact shared Vulkan dependency are exercised by the recording-order test. No execution result claimed. |

### Exact correction checks

#### GAR-1 — held indirect baseline restored, not certified

`exec.rs:1316–1319` no longer aborts merely because `m.ind != 0`. The actual branch at `:1355–1402`
resolves the original record span, copies X or XYZ into 12-byte Vulkan records initialized with Y/Z=1,
separately copies original records for `first`, inserts the existing barrier, and then calls
`clamped_indirect(..., n, 12, 12, [task_x, task_y, task_z, 0])`. The returned clamped buffer/offset,
not the pre-clamp pair, is placed in `args` and consumed by **both** indirect-count and ordinary
indirect calls (`:1458–1475`). No newly restored issue branch bypasses that invocation.

`mesh_contract.rs:13–19` preserves the former layout arithmetic: count `max(1)`, positive explicit
stride or 8/12-byte default, span `stride*(count-1)+record_size`, 4/12 copied count bytes, and zero
3D shader stride versus 1D stride/4. `exec.rs:1406–1412` keeps the original first-record behavior and
the 3D zero record. The new layout test (`mesh_contract.rs:213–224`) exercises this actual helper,
including the final first-word offset 36 in a three-record 40-byte 1D span. It is a record-layout
test, not a GPU count-buffer validation test.

Important qualification: the old helper's `NXGFX_CLAMP_INDIRECT=0`, missing clamp pipeline and missing
scratch-address fallbacks still return unclamped arguments (`exec.rs:1196–1205`); those are retained
baseline limitations, not a newly omitted branch guard. Task-only axes, total=0, GPU-generated counts
and truncation semantics are **not** corrected by this scope-out. Existing direct checked-total and
taskless checks remain (`:1443–1452`). This satisfies the specific scope-containment alternative in
GAR-1; it does not authorize using the old path on the GPU.

#### GAR-2 — captured descriptor identity survives cache publication

- `gmem::Version::token` derives generation and both `(buffer, offset)` spans directly from the
  immutable Version (`gmem.rs:50–64`). `bindings` takes the descriptor fields and token from that
  same `sub.gmem` value (`:203–216`), with fixed table/log descriptor ranges. It does not construct
  the token from a later global epoch.
- `stage_set` takes `p.token` and `p.descriptors` from the same returned pair (`exec.rs:3701–3709`)
  and returns that token with the encoded set (`:3729–3731`, also classic-set returns). No fallible
  cache identity reconstruction substitutes the latest global value.
- The ordinary SetKey no longer contains a live raw epoch (`exec.rs:184–253`). The actual cache
  entry has a separate `raw: Option<RawVersion>` (`lost_bindings.rs:129–142`). `store_version`
  refuses cacheability if the captured token's generation differs from the current epoch; it never
  modifies the token (`:197–207`). `hit_version` checks both current epoch and exact cached token
  (`:176–182`). The production caller provides `captured_raw` on store and the current Sub token
  on lookup (`exec.rs:4069–4092`). A raw shader with no captured pair cannot cache/hit.
- Thus change during ordinary resolution can legitimately produce a new captured version; change
  after capture discards cacheability; change after publication invalidates the next hit. The original
  G-descriptors-labelled-G+1 sequence is closed. A concurrent update after the last lookup can still
  occur, but it does not relabel the snapshot or grant a later stale cache hit; this correction does
  not claim to serialize arbitrary guest pool mutations with GPU execution.

`lost_bindings.rs:249–280` exercises the **production** SetCache store/hit APIs at these boundaries,
and refuses an unencoded replacement pair even with an equal ordinary key. Tokens and epoch changes
are injected CPU fixtures; it does not execute Vulkan or `stage_set`. Direct inspection above verifies
that the production encoder passes the paired token the test policy requires. Original-map callbacks,
staging ownership and existing backing release deferral are unchanged by this corrective pass.

#### GAR-3 — cache invalidation and failure propagation are now connected

`src/shaders/src/mesh.rs:31` is now `mesh 6`. `prebuild.rs:731–744` supplies that version to the real
`job_identity` and `cached_status` helpers (`:101–114`). The hash still removes only the path-like
`code` field and includes external chain identity, fixes version, mesh version, reflection identity,
job JSON and actual code bytes. Old mesh-5 successes therefore miss even with the same translator.

On a miss, production invokes `mesh_stage_output` (`:116–119,870–883`), which calls the actual Rust
wrapper and its payload bound. Wrapper errors enter `outcomes` (`:906–908`), and production
`publish_outcome` returns failed/empty bytes without invoking its loader (`:121–129,935–937`). The
existing publication branch removes stale `.spv` for empty bytes (`:957–961`). Valid and invalid
validator-result handling remains equivalent to the previous branches; no stale-success fallback
was introduced by the helper extraction.

The new test at `prebuild.rs:985–1029` exercises these exact helpers with unchanged translator/input
identity, an old successful cache record, and actual wrapper conversion of an in-memory compute
fixture. 4096 must return nonempty staged bytes; 4097 must return an error and failed/empty publication,
with a panic loader proving the failure branch never reads old success bytes. It does not run a file
publication transaction or SPIR-V validator; source inspection verifies the production empty-byte
removal branch. The 4 KiB initialized payload/fanout-32 constants remain unchanged; their identity
assertion now expects mesh 6 (`mesh.rs:129,152,1302–1306`).

#### GAR-4 — recursive locations, not packed-byte division

`mesh_contract.rs:110–129` gives each supported scalar/vector one interface location and multiplies
array length by the recursive element location count with checked arithmetic. Only the outer
per-vertex/per-primitive dimension is stripped (`:137–143`). Workgroup/payload byte counting remains
separate. Location ranges use checked ends and declaration-bounded interval union (`:145–162`), so
a very large array does not cause iteration once per declared location. Builtins and primitive indices
retain their distinct treatment. No new over-allocation loop was found.

The test's instruction/type/decorations fixture is passed through real `reflect` and `validate`
(`:257–302`), rather than constructing only a Stage. The original scalar[16]+Position example yields
68 effective components: 32768 refuses, 69632 admits, 69631 refuses. Nested vec2[3][2] yields 28
components and tests the 28672-byte boundary; multiplied overflow refuses. These fixtures validate
this parser/resource calculation, not full structural SPIR-V legality or the retained MKW pair.

#### GAR-6 — view/version capture hole closed within the shadow-only scope

`TextureRegistration` now includes ID, serial, complete `Option<view_t>` and per-slot version;
`CapturedTexture` adds creation identity (`tables.rs:155–167`). `note_tex` advances the monotonic
registry epoch even for same-value registration (`:718–724`), preventing ABA from restoring capture
permission. Buffer-slot replacement and placeholders also invalidate registration (`:472–482,793–797`).

`capture_texture` releases the table lock for image observation and rechecks the registration afterward
(`:758–765`; production callback `exec.rs:5631–5636`). `ndve_image_binding` uses the **captured view**,
checks current registration and creation, and checks registration again after releasing images
(`exec.rs:5713–5751`). It cannot replace a captured noncanonical view with a newly registered canonical
view. `ndve_registration_captures` selects the six exact semantic operands (`:5771–5781`).

The actual preparation callback is wrapped in `prepare_captured` (`exec.rs:6210–6216`), which validates
all registrations, captures registry and image epochs, releases the mutex, prepares resources, and
refuses if either epoch changed (`tables.rs:770–788`). A discarded prepared value has not recorded
native commands. Immediately before private issue the caller rechecks; on failure it replays guest
work once. After private issue it replays guest work once and checks again before comparison
(`exec.rs:6231–6244`). A replay-window change withholds comparison, not repeats guest work.

Lock order is preserved: no table mutex spans an image callback, frame preparation or guest replay;
the per-image resolver drops `imgs` before its final registration read. Tests at `tables.rs:216–268`
use actual Tables APIs and cover mip/layer/swizzle/same-value re-registration, capture-window mutation,
ABA during preparation, dropped unissued owner, injected preparation error, replay-window invalidation,
and image invalidation without re-registration. They explicitly try-lock tables in image/preparation
callbacks. `view_t` supplies the required Copy/Debug/Eq traits (`nvn/obj.rs:197–210`).

**Boundary:** these checks are optimistic observations, not locks freezing guest execution/data.
Guest weights/constants/intermediate side effects remain D3; replay is neither undone nor certified
as an immutable transaction. Conservative invalidation can withhold a shadow observation. Both live
quarantine gates remain unconditional (`newndve.rs:352–369`, `submission_safety.rs:169–178`, and
`exec.rs:6025`). No live route is enabled by this closure.

#### D5 — actual production recording sink checked

`queue.rs:473–490` factors the exact Vulkan ALL_COMMANDS/MEMORY_WRITE →
ALL_COMMANDS/MEMORY_READ|MEMORY_WRITE dependency and emits Begin then Dependency. `new_sub` directly
uses that sink to begin `s.pre` and immediately call `cmd_pipeline_barrier2` before any upload
(`:642–656`). The same `full_dependency` remains used by ordinary full barriers (`:502–505`).

The test at `queue.rs:538–568` records that actual sink between an A terminal transfer read and a B
transfer write, includes A's HOST-only tail, verifies stage ordering and access masks, removes the
entry dependency to expose the old failure, and proves an appended end-pre dependency is too late.
It retains the already-covered shader→upload case. Its small `ordered` helper models the relevant
execution scopes, not all Vulkan synchronization; source placement and memory-mask assertions couple
it to production. This closes the requested source-level regression gap without changing the accepted
specific witness or claiming sync-validation/GPU execution.

### GAR-5 — explicit open defect and bounded next-step assessment

**Still reproducible by source reasoning; ignored is not passed.** Production still decodes only a
per-table-entry bit at A completion (`gmem.rs:219–264`), schedules learned writebacks in B
(`:267–273`), and records B.main result/B.pre baseline (`queue.rs:742–758`). `mem::land_changed`
still requires before/after difference (`mem.rs:2147–2158`). For A's raw-only 0→1 with unchanged B,
both B snapshots are 1 and the guest byte remains 0. New versions/cache tokens do not restore the
missing before-image; the bug also occurs with a single unchanged table/pool generation.

The new ignored tests are honest CPU counterexample fixtures:

- `gmem.rs:975–990` calls the real `logged_ranges`, real scheduling/completion identity predicate,
  actual `record_writeback_pair` policy, and actual `merge_changed_bytes`.
- `queue.rs:497–500,748–755` confirms the extracted pair policy is also used by actual Vulkan copy
  recording. `mem.rs:1948,1998,2118–2132,2158` confirms both identity and byte-merge helpers are real
  production decisions, with the old merge semantics preserved.
- The tests at `gmem.rs:993–1008` supply the two B copy results at the GPU boundary and assert the
  **owed** results `[1,0]` and `[1,9]`. Reading the helper shows they instead retain `[0,0]` and `[0,9]`,
  so those assertions are expected to fail when individually selected. No red/green run occurred.
- These tests do **not** invoke the asynchronous `end`/`begin`, actual Pools lookup, INFLIGHT, Vulkan
  copies or `land_changed` page/watch loop. Their orchestration and B bytes are injected fixtures;
  their merge/decode logic is not a copied test implementation. The inspected production call trace
  supplies the missing scheduling/timing connection. Active identity/ordinary-producer tests
  (`:1011–1032`) are useful companion coverage, not a fix for the two ignored failures.

**Assessment of the implementer's “ABI impossible” claim:** the narrow missing-information claim is
supported; an absolute claim that all fixes require a new raw-store ABI would be too strong. The MIT
reference raw helper actually logs `selected >> 5` and `1 << (selected & 31)`, with no write offset,
old byte or before-image (`build/Q/nxnative-up/.../Translation/Optimizations/GlobalToStorage.cs:984–987,
1099–1107`). The current host decode matches this bitmap. Pool identity proves lifetime selection,
not prior contents. `page_gen` records CPU-dirty pages (`mem.rs:1254–1259`), not old GPU bytes; watched
mirrors have no CPU snapshot (`:952–958,1009–1015`). Unwatched snapshots are mutable CPU change-detection
data (`:1221–1231`), not an immutable producer baseline. B's mem begin runs before gmem begin
(`queue.rs:1646–1651`), so current CPU bytes may already have been considered for upload as well.

Two histories can therefore have the same pool identity, dirty-page indication, entry bit, current
CPU bytes and B GPU bytes yet require different byte merges. A later whole-range read, a generation
comparison, or a page-level “CPU changed” flag cannot determine which neighbor bytes A changed.
Keeping B's baseline longer cannot reconstruct A's baseline.

**Concrete limited patch route available in existing infrastructure:** if production reflection or
another authoritative pre-submit contract establishes a finite exact destination superset for a raw
producer, register those version-matched backing ranges in **A**, not after its log in B, through
`mem::note_gpu_write_version`/`note_writeback` before `submit_sub` plans copies. A's existing
`queue.rs:742–758` pre/main pair and `mem::land_changed` merge then supply the needed producer before/
after evidence and existing receipt/identity handling. The target proof must include the actual
backing/version used and complete destination coverage; arbitrary CPU pointer guesses or last-frame
WRITTEN entries are not that proof. Existing explicit producer writebacks already demonstrate this
case. No complete target proof for arbitrary raw-only stores was found in this scoped source, so
this is a bounded subset route, **not a proposed blanket GAR-5 fix**.

For unconstrained GPU-derived targets, a complete bounded design must obtain producer-side delta/
before-image information (or another proven coherent backend) with defined overflow and CPU-merge
semantics. Simply snapshotting every visible pool, silently truncating a capture budget, or landing
the whole learned range is not accepted. No narrow universally correct patch can be assembled solely
from the currently retained identities/page generations/B baseline. A focused producer-provenance
design is owed; an unbounded engine refactor or another GPU fault reproduction is not requested.

**Separation/hold:** GAR-5 predates this corrective pass and is independent of the new immutable table/
log version and cache-token fix. It does not block compiler acceptance work or the selected CPU helper
tests once the lead's execution gates pass. It blocks D6 writeback-complete claims and production/GPU
correctness sign-off for raw-only learned-write paths. Existing overall GPU/gameplay holds, NDVE
quarantine and D3/indirect-count limitations remain. No current crash/root-cause attribution is inferred.

### Tests — not run

No compiler, build, test, shader validator/tool, Vulkan/GPU call, probe, process-control operation or
agent ran during this recheck. The findings above are source analysis, not fresh test evidence.
The two exact ignored GAR-5 tests must remain reported as known failures; a future suite's green
summary excluding them cannot close GAR-5. Broad `--ignored` is not endorsed.

### Verified — read-only file table

| Read scope | Recheck purpose |
|---|---|
| Root `AGENTS.md:40–51`; corrective handoff `GPU-ALIGNMENT-PATCHES.md:296–493`; existing review history | Current scope, holds, requested corrections and prior counterexamples |
| `src/graphics/src/vk/exec.rs` relevant mesh/cache/capture/preparation/replay blocks | Actual GAR-1/2/6 wiring and quarantine call |
| `src/graphics/src/vk/gmem.rs` version/bind/end/begin and new tests | Token/descriptor identity and GAR-5 decode/scheduling witness |
| `src/graphics/src/vk/lost_bindings.rs:129–282` | Production cache publication/hit policy and interleaving tests |
| `src/graphics/src/vk/mesh_contract.rs` | Complete changed interface-location calculation, held layout helper, parser/grid tests |
| `src/graphics/src/vk/tables.rs` registration structs, APIs and tests | Capture version/ABA/image-epoch checks and lock boundaries |
| `src/graphics/src/vk/queue.rs` upload-entry/pair policy/tests, new_sub/tail wiring, begin order | D5 production coupling and GAR-5 baseline timing |
| `src/graphics/src/vk/mem.rs` snapshot/page-generation, identity and merge paths | Whether existing provenance can repair GAR-5; semantics-preserving extraction |
| `src/shaders/src/{mesh,prebuild}.rs` relevant version/hash/cache/wrapper/publication/tests | GAR-3 actual invalidation/error path and preserved 4 KiB/fanout 32 |
| `src/graphics/src/nvn/obj.rs:197–210`; `vk/newndve.rs:345–369`; `vk/submission_safety.rs:169–178` | Captured-view trait/API compatibility and retained quarantine |
| MIT nxnative `GlobalToStorage.cs:974–1118` | Bounded source confirmation of bitmap granularity; no source copied |
| `docs/waves/GPU-ALIGNMENT-REVIEW.md` | Sole write: this appendix with apply_patch; history preserved |

No shell/Git/tool executable was invoked in this recheck. Baseline HEAD remains the previously verified
identity supplied for this task, not a newly executed Git result. No definite new type/API/borrow
mismatch was found in these changed call sites; compilation remains unverified.

### Unproven

All new test results and compiler acceptance; full indirect GPU counts/total behavior; GAR-5 producer
provenance; D3 guest-data/dispatch contract; GPU runtime lifetime/parity; thermal readiness and incident
causality. The earlier D7/D12/D14 deferred evidence boundaries remain as recorded, without reopening
unchanged engine scope.

### Slips

None. Append-only report change; no source writes or execution. No subagents or owned workloads.

### Questions / lead handoff

No further routine source correction is requested for the scoped GAR-1/2/3/4/6 changes or D5 test.
Accept those narrowly for the lead's gated CPU compile/build/test phase, while keeping GAR-5 explicitly
open and commissioning only the bounded producer-provenance work needed to close it. The telemetry/
competing-build gate must be resolved before any execution; this appendix does not relax it.

**Owned workloads: none created, none left running. No foreign processes inspected or affected.**

### Final cache-boundary follow-up — GAR-R1 / High, corrective regression; CHANGES REQUIRED

**This final follow-up supersedes this appendix's “no additional regression” and immediate source-review
acceptance statements.** The original GAR-2 raw-pair relabeling counterexample is closed, but its correction
also removes pool-change invalidation from **non-raw** buffer descriptor sets. This was found by checking
the `raw=None` branch of the same changed production cache, not by reopening unrelated engine scope.

**Exact sites:** `exec.rs:184–253,4069–4075`; `lost_bindings.rs:176–182,199–207`.
The previously reviewed candidate put `POOL_GEN` in every SetKey (historical GAR-2 anchor `exec.rs:248`).
The corrective version removes that field entirely. Only stages with sentinel raw Sbuf resources receive
a `RawVersion`; an ordinary Sbuf/UBO stage supplies `None`. Both new cache methods then ignore
`current_generation` because their predicate is conditional on `raw.is_some()`.

**Reachable counterexample:**

1. With descriptor buffers and the default set cache enabled, a shader containing an ordinary storage
   buffer and **no** sentinel raw-table resource resolves guest address A to pool backing P and caches
   that descriptor. The actual ordinary resolution is `exec.rs:3412–3430`.
2. During the same recording, a guest thread replaces/registers the pool at A. `mem::pool_add` publishes
   it and increments `POOL_GEN` (`mem.rs:583–592`); old P remains lifetime-retained by existing deferral.
   No shader/binding-state change is necessary: the guest address and range can remain identical.
3. Choose no texture registrations/buffer textures involved. `requeue_buf_texs` only enqueues affected
   texel IDs and does not increment image generation (`tables.rs:486–495`). `prepare_storage_bindings`
   only handles non-buffer storage images and skips this stage (`exec.rs:2658–2665`). Thus the module,
   bgen, UBO addresses, pgen, image generation, Sub serial and descriptor chunk in SetKey remain equal.
4. On the next draw/dispatch, `uses_raw == false`, token is None, and `hit_version` returns the cached
   P descriptor despite the changed epoch. `stage_set`/`mem::resolve` is bypassed, so the newly registered
   pool is not selected/materialized for this use. Reads see old backing and writes target old backing.

This is stale resource selection, **not** a claim that P is already freed. The prior candidate's global
epoch key would miss on this between-use replacement. Removing it newly re-exposes the counterexample
relative to that candidate, even if an equivalent weakness existed before the alignment wave.

**Minimal requested correction:** retain a separate stable memory-pool dependency identity/invalidation
for all sets that encode guest-buffer backing, while retaining the new immutable raw pair token. Do not
restore the old raw relabeling bug by simply loading a new epoch after encoding. A conservative bounded
implementation can capture an epoch before ordinary resource resolution, verify it is unchanged before
cache publication, store that epoch, and require equality on later hits; if resolution itself creates
backing or a concurrent mutation advances it, decline to cache that encoding. A later stable encoding
can cache normally. Immutable per-backing tokens are another valid implementation, but a broad cache
rewrite is not requested.

**Meaningful regression:** add the `raw=None` ordinary-buffer case to the real SetCache boundary tests:
cache a set at epoch G, keep ordinary key equal, advance epoch to G+1, and require a miss. Today
`store_version(stage, Some(key), off, None, None, G)` followed by
`hit_version(stage, key, None, G+1)` returns a hit. Also test mutation during ordinary encoding so the
fix cannot relabel an old SSBO descriptor with G+1. Preserve the raw-token interleaving tests and
same-epoch non-raw cache hits. Trace the new ordinary-memory identity from actual `stage_set` construction
through production `bind_sets`, as was done for raw tokens.

**Final dispositions:** GAR-1 remains closed by scope-out; GAR-2's original witness and GAR-3/4/6 remain
closed at the stated source-only boundaries; D5 coverage remains accepted; GAR-5 remains open/separable.
**GAR-R1 is a new blocking source correction before accepting this corrective candidate for the lead's
gated compile/build/test phase.** After its narrow recheck, GAR-5 alone need not prevent CPU compilation
or its explicitly scoped CPU tests, but still prevents D6/production/GPU readiness claims. Current
telemetry/concurrency blockers independently forbid execution now.

Additional verified reads for this follow-up: `exec.rs:2645–2709,3410–3437`, `mem.rs:580–596`,
`tables.rs:484–504`, against the already-read cache definitions/callers. No test, compiler, shell,
process probe, Vulkan/GPU activity or source edit occurred. Only this report was appended. No owned
workloads or subagents were created or left running.

---

## Final narrow GAR-R1 recheck — 2026-11-23

### Built — source review only

**GAR-R1 CLOSED for its reviewed source counterexample. No new concrete defect was found in this
narrow correction. The candidate is accepted at source-review level for handoff to the lead's gated
CPU compile/build and subsequent CPU-test phase.** This supersedes the preceding GAR-R1-open/source
acceptance blocker, preserving its history. It is not an executed compile/test result, immediate
execution permission, or production/GPU readiness.

Scope was limited to the changed `stage_set`/`bind_sets` boundary, actual SetCache implementation and
its regressions, plus the implementer's appended GAR-R1 report (`GPU-ALIGNMENT-PATCHES.md:495–591`).
The existing GAR-1 scope-containment, GAR-2/3/4/6 closures and D5 disposition stand. **GAR-5 remains
High, OPEN and separable**; D6 completeness, D3 activation and held indirect-count limitations remain
unresolved. No unchanged-engine audit was reopened.

### Verified — exact production boundary

1. **Capture is before encoding:** `src/graphics/src/vk/exec.rs:3171–3183` captures `POOL_GEN` with
   Acquire at `stage_set` entry, before zero-buffer/bank preparation or ordinary resource resolution.
   It is not a publication-time load disguised as captured identity.
2. **All returns preserve that capture:** descriptor-buffer return `exec.rs:3735`, classic memo-hit
   return `:3762`, and newly allocated classic-set return `:3795` carry the original
   `memory_generation`. The actual raw descriptor pair still independently supplies its token and
   descriptor fields together (`:3705–3712`).
3. **Production passes the capture through:** the cache-miss path destructures `captured_memory`
   from `stage_set` (`exec.rs:4088–4090`) and passes it unchanged to `store_version` (`:4095–4097`).
   The fresh global load is a separate validation argument. Recomputing the ordinary key for
   descriptor-chunk/image checks does not replace either captured memory epoch or raw token.
   The classic path's four-field destructuring is updated (`:4141–4144`).
4. **Non-raw sets are covered:** `CachedSet` retains `memory_generation`
   (`src/graphics/src/vk/lost_bindings.rs:137–143`). `hit_version` requires stored epoch == current
   epoch unconditionally, including `raw=None`, in addition to ordinary-key and exact raw-token
   equality (`:178–184`). Production passes the current epoch on every cache lookup
   (`exec.rs:4073–4080`). Thus the original unchanged-key/non-raw SSBO pool replacement now misses.
5. **Mid-encoding mutation cannot be relabelled:** publication requires captured memory epoch ==
   current, and any raw token's generation must independently equal current
   (`lost_bindings.rs:199–213`). If ordinary resolution creates backing or a concurrent pool change
   advances the epoch, that encoding is not cached. A later stable encoding can cache normally.
   A newer raw pair cannot rescue stale ordinary descriptors, and a current ordinary epoch cannot
   relabel an old raw pair. The GAR-2 fix is preserved.

Scoped call-site searches found the two production `stage_set` callers updated for the four-value
return and the production cache caller updated for the separate captured/current epoch arguments.
The zero-epoch convenience `hit`/`store` wrappers remain internally consistent
(`lost_bindings.rs:174–175,189–196`); production `bind_sets` uses the version-aware APIs. No definite
signature/tuple/borrow mismatch was observed. Compiler acceptance remains unverified.

This is conservative cache admission/invalidation, not serialization of arbitrary guest pool changes
with command issue. A mutation after a successful observation does not acquire a new epoch label for
that cached entry; a subsequent lookup observes the mismatch. Existing recorded-resource ownership
and the wider execution contract are unchanged by this fix.

### Tests — source inspected, NOT RUN

All four tests use the **actual production SetCache methods**, not a separate cache implementation:

| `lost_bindings.rs` test | Verified assertion boundary |
|---|---|
| `:291–300` — ordinary buffer without raw token | Stable G hit; same-key lookup at G+1 misses; new stable G+1 encoding hits. Directly covers GAR-R1. |
| `:303–316` — mutation during ordinary encoding | Capture G, publish at G+1: neither epoch can hit the discarded encoding; subsequent stable encoding restores a normal hit. |
| `:319–333` — independent raw/ordinary epochs | Stale ordinary/current raw and current ordinary/stale raw both refuse; matching identities hit; later epoch misses. |
| `:255–287` — existing raw interleavings | Preserves raw-token identity cases with a separately captured entry epoch; backing creation during resolution now conservatively refuses caching even if the raw pair was captured later. |

Epoch mutations and descriptor offsets are CPU fixtures; these tests do not execute `stage_set`,
Vulkan encoding or concurrent guest threads. The inspected production capture/return/publication
chain above establishes their coupling to the real bug. No pre-fix red or post-fix green result is
claimed. GAR-5's previously documented ignored known failures remain open, not converted into passes.

### File table / access

| File | Narrow recheck |
|---|---|
| `src/graphics/src/vk/exec.rs:3164–3211,3698–3795,4064–4144` | Read capture, all returns and production hit/store/destructure paths; scoped call-site searches |
| `src/graphics/src/vk/lost_bindings.rs:129–333` | Read cached identity, admission/invalidation APIs and affected tests; scoped call-site searches |
| `docs/waves/GPU-ALIGNMENT-PATCHES.md:495–591` | Read implementer's exact correction/coverage claims |
| `docs/waves/GPU-ALIGNMENT-REVIEW.md:643–703` | Read original GAR-R1 witness; sole write is this append-only closure |

### Unproven / execution boundary

No compiler, build, tests, executable tools, shell/Git commands, Vulkan/GPU activity, process probes
or subagents ran in this recheck. Fresh telemetry/concurrency qualification remains the lead's gate;
this closure does not establish it. **No remaining source-review blocker from GAR-R1 prevents that
gated CPU phase.** GAR-5 remains an existing separable correctness defect and blocks D6/production
writeback-complete claims; D3 and indirect limitations remain held. No GPU activation, deployment or
historical crash-causality conclusion follows.

### Slips

None. Only this review report was appended; source and prior review history were preserved.

### Questions / disposition

No further correction or product question is requested for GAR-R1. Proceed only through the lead's
already-authorized execution gates for CPU compilation/build and subsequent scoped CPU tests.
Previous safety holds and the explicitly open GAR-5 boundary remain in force.

**Owned resources/workloads: none created, none left running.**
