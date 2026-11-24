# Audit-01 — independent DG-01 / DH-01 / DS-01 review

## File table (declared before investigation)

| Action | Absolute path | Purpose / pre-existing changes |
|---|---|---|
| New | `D:\Projects\RecompLauncherEngine\docs\waves\Audit-01.md` | Sole writable file; independent findings, proposed regressions and disposition. Confirmed absent before creation. |

All production, test, evidence and other documentation inputs are read-only. Explicit shell cwd:
`D:\Projects\RecompLauncherEngine`. Supplied HEAD: `7688e33ef5ef47e64322c7aed0f84c471e9c8b99`,
dirty baseline. No compiler, test, GPU, game, worker or commit lease. Session identifier unavailable.

## Built (audit only)

**Two P2 findings in DG-01; no P0/P1 finding.** Both affect the promised diagnostic evidence,
not rendering behavior or the cause of the retained MKW fault. No new actionable defect established
in DH-01 or DS-01 within their declared scope. Findings are source-derived, not executed reproductions.

| Wave | Audit disposition | Acceptance consequence / handoff |
|---|---|---|
| DG-01 | Return A01-1 and A01-2 to original graphics agent via lead | Acceptance blockers for the exact-draw/cache-retaining discriminator. Correct and CPU-regress before relying on it for a separately authorized run. No causal fix certified. |
| DH-01 | No new blocking finding; recommend accepting reviewed CPU guard scope | Supplied lead Python result stands as lead evidence. Existing documented successor/OS/query limitations remain; no resumption authorization. |
| DS-01 | No new blocking finding; recommend accepting reviewed CPU/offline arithmetic scope | Host interval 16 reaches both native kernels; replacement remains disabled. Device FP behavior, guest integration and gameplay remain unproven. |

### A01-1 — P2: evidence lifetime is shorter than the descriptor cache lifetime

- **Wave:** DG-01. **Confidence:** high. **Disposition:** acceptance blocker for diagnostic completeness.
- **Primary location:** `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:3706-3711`.
  Related paths: the independent cache at `:120-124`, cache-hit selection at `:3755-3765`, snapshot
  validation at `:3780-3787`, and snapshot collection at `:1327-1339`.
- **Concrete failure path:** record a mesh draw with a cacheable stage, then a compute dispatch in
  the same submission/chunk, then the same mesh draw with that stage's key unchanged. The compute
  call to `bind_sets` (`:4970-4977`) has only stage 5 and clears the graphics snapshots. It does not
  clear `qs.sets`. On return to graphics, `qs.sets[st]` still hits, so `stage_set` is not called and
  no encoding snapshot is restored. The validation branch increments `binding_missing`; subsequent
  unchanged hits continue losing evidence until a real set rewrite. A cacheable fragment stage with
  no `Res::MeshDraw` suffices: its key need not change when a new mesh record is staged (`:220-225`).
  A cacheable set A, an uncacheable set B in the same stage, then A also loses A's evidence: B replaces
  `binding_evidence`, while the `Some(old)` condition at `:3772-3774` leaves cached A intact.
- **Impact:** the report honestly says evidence is missing, but destroys available encoding metadata
  on routine pipeline transitions. This defeats the wave's explicit cache-retention requirement and
  can waste the bounded discriminator. An old draw's `Arc` in history does not help: the hit path
  never looks it up. This is new DG-01 bookkeeping, not a descriptor-cache rendering defect.
- **Why current tests miss it:**
  `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost_bindings.rs:256-271` manually clones
  one `Arc` into two draws. It does not exercise the production clear/hit/restore sequence.
- **Requested correction:** retain encoding evidence with the cached set identity and select only
  the active stages for an issued draw. Keep the declared memory budget explicit if adding a
  separate active view. Preserve missing/unknown reporting for genuinely unavailable evidence.

### A01-2 — P2: fixed three-checkpoint lookback can exclude the retained draw being diagnosed

- **Wave:** DG-01. **Confidence:** high for the selector counterexample; occurrence in retained MKW
  run is not claimed. **Disposition:** acceptance blocker for exact-checkpoint association.
- **Primary location:** `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost.rs:122-124`.
  The record contains only the initial token at
  `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:1103-1115,1280-1281,1341-1348`;
  filtering is numeric at `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost_bindings.rs:144-147`.
- **Concrete failure path:** with checkpoints `all`, mesh token 100 is recorded before setup.
  Setup prepares an image with four upload parts (tokens 101-104), then opens its pass (token 105),
  and issues the mesh command with evidence still keyed to 100. The upload loop has no three-event
  bound: `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:3038-3075` calls
  `copy_buf_img` for each part, which emits a checkpoint at `:3228-3239`. Storage-image preparation
  can precede `begin_pass` at `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:3996-4003`;
  the pass checkpoint is at `:1980`. If the GPU reaches pass marker 105 at both reported stages
  and then stalls on its first mesh command, `lo=hi=105` yields `[102,105]`. Retained draw 100 is
  excluded even with zero history evictions. The report cannot recover its ranges and describes
  no mesh snapshot for the selected window.
- **Related ordering case:** uploads are recorded into `sub.pre`, but `sub.pre` is submitted before
  `sub.main` (`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\queue.rs:701-704`). Numeric
  allocation order therefore need not be GPU execution order. A reported started token 100 and
  finished upload token 104 also produce `[101,104]`, dropping the explicitly reported token 100.
  The older text-report conventions are baseline; the newly added binding-window filter is the
  defect under review.
- **Impact:** explicit recording association is correct, but its consumer can discard exactly the
  retained record required to diagnose the loss. Increasing a magic lookback alone cannot establish
  the association. This is not evidence of the game's actual faulting instruction.
- **Why current tests miss it:** the checkpoint test supplies `History::window(102,102)` directly;
  it does not test `on_lost`'s window construction or more than one intervening checkpoint.
- **Requested correction:** carry enough bounded command/setup association to select the issued
  draw from the actual reported markers, preserving explicit driver markers even when their numeric
  order reverses. Distinguish retained-but-omitted evidence from eviction/unavailable evidence when
  imposing an output cap. No additional GPU work is needed to test the selection policy.

### Other reviewed invariants

- **DG:** the actual returned checkpoint token reaches each of the three mesh issue sites, and
  skipped/empty direct draws do not fabricate issue records. Descriptor address/size comes from the
  same locals passed to descriptor encoding, including zero fallback. Submission/chunk/offset/stage/key
  matching prevents stale snapshots from being mislabeled. The per-stage 64-range cap, draw count cap,
  capacity-based payload charging and whole-draw FIFO eviction are real. Allocator/container overhead
  and seven queue-held snapshots are outside the 16 MiB history payload and are disclosed; pre-existing
  address/name histories are not newly bounded by this wave. Half-open checked range classification
  and nearest-outside wording do not assert OOB causation. Texel metadata lacks a buffer identity;
  its zero handle/absent binding IDs cannot establish lifetime. No new lock-order cycle was found:
  issue-time binding lookups release `BINDS` before acquiring `HISTORY`; callback code does not acquire
  `HISTORY`. Disabled paths add cached flag checks/state fields, not range allocations or driver queries.
- **DH:** shared path is repository-fixed and optional paths additive. Malformed/unreadable/pending
  markers refuse before claims/launch; checks repeat after waits. Journal creation precedes replacement,
  first-fault history is retained, and remaining latch paths are attempted after a write failure.
  Exit 71, in-loop reset and late reset have persistence paths. The root handle cleanup and already
  tracked successor cleanup precede ordinary claim release and scenario save restoration. Tests use
  real temporary JSON/journal/save files, including actual `SaveTransaction.finish`; their storage
  assertions are more than mocked calls. Platform/process/time mocks do not prove Windows termination.
  The documented unkillable-process, successor exit-status, query-error, concurrency and scenario-thread
  limitations are not relabeled as new findings.
- **DS:** `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\newndve\kernels.rs:55-70,135-159`
  serializes `Job::args()[6]=16` into the actual 32-byte push-constant payload. Both shaders declare
  `rounding` as the seventh uint and use it at the FMA-to-half boundary:
  `D:\Projects\RecompLauncherEngine\src\graphics\shaders\ndve_layer.comp:12-15,48-51` and
  `D:\Projects\RecompLauncherEngine\src\graphics\shaders\ndve_resample.comp:9-12,40-54`.
  CPU packed-word low/high folding is separate from the shader channel/modulo loop. Literal midpoint,
  subnormal, overflow-cancellation and FP32 order examples are consistent with the declared CPU target:
  `1 + 2^-11 + 2^-11 -> 0x3c01`, separate half-boundary ties -> `0x3c00`, and
  `2^24 + 1 - 2^24 -> 0`. Tests would reject the old host 8, rounding only after 64, or reassociation.
  Both resampler stages round at each 16-product helper boundary. Off/check behavior still returns
  no consume decision; `exec.rs:4947-4951` continues into the translated pipeline. No hardware or
  console equivalence follows from this source-level contract.

## Tests written (none, proposed tests)

None written or executed. Focused regression proposals follow.

Lead should return these exact proposed regressions to DG-01 (names are proposals, not existing tests):

1. **`cached_mesh_evidence_survives_compute_and_uncacheable_interludes`**: exercise the production
   CPU cache/evidence transition helper (extract one if needed), not a parallel mock implementation.
   Cache stage-1 A with serial 7/chunk 0/offset 64, shader key A, range and write ID 17. Select compute
   stage 5 without changing A's key, then select A by cache hit. Require A's original range/write ID,
   fresh issue ID 18 and zero missing stages; ensure stage 5 is absent from the mesh draw. Repeat
   A -> uncacheable B -> cached A. Also change serial/chunk/key and require mismatch to stay unknown.
   Present code clears A on the compute case and replaces A on the B case.
2. **`loss_selection_keeps_draw_across_setup_markers_and_pre_main_reordering`**: test the actual
   production report-selection helper with retained draw token 100, four upload tokens 101-104,
   pass token 105, and no evictions. For driver `(bottom,top)=(105,105)`, require the associated draw
   100 in bounded output. Separately use `(104,100)` for pre/main ordering and require explicit
   top draw 100. Cover one intervening checkpoint, skipped/unissued draw, genuinely evicted draw,
   and output-cap overflow with an explicit omission count/status. Present selector drops token 100
   in both main counterexamples.

Proposed lead execution after correction, serially from `D:\Projects\RecompLauncherEngine`, with
`CARGO_TARGET_DIR=D:\Projects\RecompLauncherEngine\target\OOTWAIT`:

```text
cargo test -p graphics --lib cached_mesh_evidence_survives_compute_and_uncacheable_interludes --jobs 4
cargo test -p graphics --lib loss_selection_keeps_draw_across_setup_markers_and_pre_main_reordering --jobs 4
cargo test -p graphics --lib --jobs 4
```

These are CPU-only proposals with no `--ignored`. The original agent owns implementation/compile
under a renewed lease; lead owns actual execution. No redundant new DH/DS tests are demanded here.

## Gates

Audit commands, all with explicit cwd `D:\Projects\RecompLauncherEngine`:

| Command / gate | Audit result |
|---|---|
| `git rev-parse HEAD && git status --short` | Exit 0; supplied HEAD confirmed; extensive dirty baseline, including untracked NDVE sources, observed. |
| `git diff -- src/graphics/src/vk/lost.rs scripts/run_game.py` | Exit 0; reviewed changes against HEAD with dirty-baseline caveat; LF/CRLF advisories only. |
| Dedicated reads/searches of source, saved pre-wave diff and DS scoped diff | Completed; findings isolated to new DG bookkeeping, not unrelated baseline changes. |
| `git diff --no-index --check -- /dev/null docs/waves/Audit-01.md` | Passed; no whitespace diagnostics, only Git LF/CRLF advisory. Checks the new/untracked report; no production/test whitespace gate run by auditor. |
| Compiler, tests, shader tools, GPU, game, release build | NOT RUN; no corresponding lease. |

Supplied **lead-executed** acceptance, also recorded at
`D:\Projects\RecompLauncherEngine\docs\32-REMAINING-WORK-PLAN.md:70-72`: after DG, graphics
100 passed / 3 ignored; after DS, graphics 105 passed / 3 ignored; after DH, full Python 64 run /
1 skip, all passed. These were not independently rerun. Wave-agent reports saying tests awaited lead
execution describe their earlier handoffs, not a contradiction of the current lead results.

Graphics clippy remains **blocked/unassessed, not passed** at
`D:\Projects\RecompLauncherEngine\src\runtime\src\rt.rs:119` (`not_unsafe_ptr_arg_deref`),
as retained in `D:\Projects\RecompLauncherEngine\build\DELEGATION\DG-01\clippy.log:95-105`.
Baseline formatting/unused-mut warnings are not acceptance demands for unrelated cleanup.

## Evidence

Required protocol, architecture, execution ledger and all three wave reports read before investigation.
Key evidence examined (all read-only):

- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DG-01\README.md`, `result.json`, clippy log,
  and its referenced pre-DG diff
  `C:\Users\Risky Biz\.local\share\opencode\tool-output\tool_1ff8bc9b3001dhWfnfk35ynn95:3021-3124`.
  The saved pre-wave cache path has no `binding_evidence` clearing/validation. `lost.rs` was clean
  before DG per the file table; the new selector is visible directly in its HEAD diff.
- `D:\Projects\RecompLauncherEngine\build\HARNESS3\20261121-135645-mkw-baseline\run.log:2182-2234,2284-2293`:
  started #94396, bottom #94366, fresh stage-set writes in submission 1598, invalid read 0xd434b0000
  with precision 0x1000 and zero containing-use entries. These establish the old evidence gap;
  neither audit counterexample is asserted to have occurred in that run. No causal investigation repeated.
- `D:\Projects\RecompLauncherEngine\build\GPU-STOP.json` and
  `D:\Projects\RecompLauncherEngine\build\HARNESS3\GPU-STOP.json`: both active; shared seed retains
  source paths, 13:57:10 reset, exit 71, peak 12372 MB and 33.3 s. Both were only read.
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DH-01\review.json`: baseline identity,
  declared changes and limitations. Full `gpu_stop.py`, `run_game.py`, `test_gpu_stop.py` and the
  read-only scenario/save-transaction source reviewed against those claims.
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DS-01\scoped.diff`, `review.json`,
  `compile-gates.json`, `offline-gates.json`: pre-existing untracked prototype versus DS changes,
  recorded successful compile/validation commands and preserved read-only identities. These are
  inspected retained records, not tools executed by this audit.
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DS-01\derive.py:21-31,34-116`:
  symbolically seeded lane inputs, supported-operation interpretation and verification of each
  output's 16 ordered A/B products and C seed. Its unsupported-op rejection and explicit FP limits
  avoid equating instruction counts with hardware proof. The artifact-generation script was not run.
- `D:\Projects\RecompLauncherEngine\build\DELEGATION\DS-01\helper-contract.json:1-240` and
  sampled disassembly in `helper.spvasm:408-729,1300-1399,2100-2171`: lane/scratch addressing,
  unpacked C, low/high FMA dependence and terminal packs agree with the symbolic contract. The JSON
  records 128 chains; this audit reviewed the verifying algorithm and representative instruction
  chains, rather than independently re-executing all chains. The unchanged fragment oracle at
  `D:\Projects\RecompLauncherEngine\src\graphics\tests\shaders\ndve_hmma_translated.comp:25-37`
  has the same ordered packed-pair fold. All five current NDVE Rust submodule files and both native
  shaders were read.

No screenshots viewed; this audit has no visual claim. Last candidate library remains the supplied
`D:\Projects\RecompLauncherEngine\target\OOTWAIT\release\nxfront.lib`, SHA-256
`6827ce497d42a30b199ae82a659666330ca9390dd03afd8977ba28f43ba83895`, build completion time unrecorded.
No new release binary was built or independently hashed during the audit.

## Unproven

- No GPU, hardware, gameplay, performance, release-build or independent test-pass claim.
- DG findings have source-level counterexamples only. The exact MKW faulting instruction, resource
  lifetime versus arithmetic cause, and diagnostic operation on a device remain unknown. Existing
  range precision, classic-set/image/global-pool exclusions and absent driver IDs remain limitations.
- DH tests do not prove OS process-control success, successor exit status or total-storage-denial
  durability across interpreter restart. Its already documented limits remain explicit, not passes.
- DS has no universal FP parity guarantee: retained helper lacks explicit FP-mode preservation;
  half subnormal behavior/special values remain device gates. Original callers, guest tensor tiling,
  sampling, binding/residency/alias preflight, shadow frames and OoT progression remain open DS-02 work.
- Shared and HARNESS3 fault-stop latches remain active. This audit is not a GPU-resumption decision
  and proposes no new GPU experiment.

## Not built

No implementation/test fixes, compiler execution, tests, shader derivation rerun, GPU/game/replay,
release build/relink, commit, staging or shared-ledger edits. Next action: lead returns A01-1/A01-2
to the original DG-01 agent, grants its scoped correction/compile lease, executes the focused CPU
regressions and reviews the corrected discriminator. DH-01/DS-01 may be accepted at their restricted
reviewed scope without interpreting acceptance as device/game verification.

**Scope/resource release:** sole file written is
`D:\Projects\RecompLauncherEngine\docs\waves\Audit-01.md`, via `apply_patch`. Production, tests,
artifacts, latches and other docs remained read-only. No subagents, compiler, test, game/GPU processes
or claims were created. Synchronous git commands finished; no audit-owned worker/resource remains.
Compiler/GPU/test leases were never acquired. Audit report ownership returns to the lead at handoff;
no assertion is made about unrelated owner processes.

## Slips

None.

## Questions

None requiring an owner choice. The diagnostic corrections are owed implementation through the
original graphics agent; execution and acceptance remain lead-owned.

## Focused DG send-back recheck — file table before review

| Action | Absolute path | Purpose / existing history |
|---|---|---|
| Update (append only) | `D:\Projects\RecompLauncherEngine\docs\waves\Audit-01.md` | Recheck A01-1/A01-2; preserve the original findings and prior dispositions above. Sole writable file. |

Session `ses_e004e33b2ffek9Z6WAgDtzmPF4` (ledger). Explicit shell cwd:
`D:\Projects\RecompLauncherEngine`. DG production/tests/evidence remain read-only. No compiler,
test, GPU, game or subagent lease. Earlier DH-01/DS-01 dispositions stand.

### Built (recheck audit only — current disposition)

**A01-1 CLOSED; A01-2 CLOSED. No remaining actionable finding established in the DG send-back.**
Confidence is high for resolution of the source-level counterexamples. Recommend accepting DG-01's
corrected CPU/reviewed diagnostic scope, based on this integration review and the lead's supplied
107 passed / 3 ignored result. This supersedes the original DG return disposition above; the original
findings and their historical line references are preserved. DH-01/DS-01 earlier dispositions stand.

**A01-1 — cache ownership and active selection verified:**

- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:120-121` now uses
  `SetCache<SetKey>` as the actual descriptor cache. The key policy at `:154-223` still covers the
  original module/state/submission/chunk identity.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost_bindings.rs:129-183` stores
  key, offset and evidence together. `begin` clears only `active`/`required`; it does not erase
  `cached`. `hit` returns the offset and that entry's snapshot together.
- Production `bind_sets` at `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:3689-3692,3732-3759`
  begins the active view, restores cached evidence on hits, and stores fresh evidence only when
  the write had a cacheable key. A freshly uncacheable B therefore leaves cached A intact. A key
  invalidated during an initially cacheable write still clears its old cache entry. Fresh evidence
  is returned directly by `stage_set` at `:3397-3399`, rather than overwriting a separate cache slot.
- `SetCache::select/capture` at
  `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost_bindings.rs:186-211` verifies
  serial/chunk/offset/stage/shader and samples fresh issue IDs only for required stages. Actual mesh
  issue calls `capture` at `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:1322-1325`.
  Thus A -> compute -> cached A and A -> uncacheable B -> cached A both recover A's write metadata
  without leaking compute/B into the active mesh view. Classic descriptor sets still report required
  stages as missing, rather than borrowing cached descriptor-buffer evidence.

**A01-2 — producer, selector and report integration verified:**

- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost.rs:32-39,62-72` updates a
  thread-local last-recorded token after recording each checkpoint. The mesh retains its initial
  token at `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:1097-1110` and records
  `setup_last = last_recorded_checkpoint().max(checkpoint)` at `:1326-1328`. All three actual command
  issue paths call this recorder immediately before issuing (`:1274-1276,1281-1285,1293-1296`).
  Therefore the original setup sequence 100 -> uploads 101-104 -> pass 105 produces span [100,105]
  in production; there is no three-checkpoint bound or dependence on another worker's latest token.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost.rs:104-117,194` preserves each
  distinct nonzero returned driver marker and passes that collection directly to the binding report.
  The old general text-report lookbacks remain, but no longer constrain binding selection.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost_bindings.rs:267-297` prioritizes
  exact draw markers, then setup-span associations, then conservative numeric-envelope candidates.
  Marker 105 selects draw 100 through its setup span; markers [104,100] select it through the exact
  100 token regardless of bottom/top numeric order. Both original exclusions are resolved.
- Actual reporting calls this selector with cap 64 at `:328-336`. `marker_evidence` at `:300-314`
  and output at `:338-345` distinguish retained-but-output-capped associations from unavailable
  associations. Total candidate omissions and history evictions are separately reported. Missing
  records still do not establish an empty binding set or a lifetime/OOB conclusion.

**Bounds/integration check:** seven cached plus seven active snapshot references per queue are
visible in `SetCache`'s fixed arrays, and the updated fourteen-reference limit is disclosed in
`D:\Projects\RecompLauncherEngine\docs\waves\DG-01.md:244-248`. Per-stage ranges and history
count/byte budgets remain bounded; `size_of::<Draw>()` charges the new setup endpoint. Selection
allocates at most the output cap's references and scans bounded history. Its conservative envelope
can include interleaved workers' tokens; that limitation is disclosed and is not causal attribution.
No new lock-order issue was found in the correction. Disabled diagnostics retain optional empty
evidence/state overhead and a thread-local checkpoint update, without snapshot/range allocation.

### Tests written (none by auditor; send-back regressions reviewed)

No tests written, compiled or executed by this recheck. Reviewed both added tests in
`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost_bindings.rs`:

- `cached_mesh_evidence_survives_compute_and_uncacheable_interludes` (`:519-580`) exercises the
  actual `SetCache` methods now used by `QState`/`bind_sets`/mesh issue. It asserts original range
  and write ID 17, newly observed issue ID 18, zero missing stages and active-stage isolation after
  both interludes. It also checks identity mismatches and invalidation. This is a real state-transition
  regression, not a copy of cache logic or the original direct-Arc history test. Its simplified key
  is appropriate to the generic helper; production's real `SetKey` wiring was reviewed separately.
- `loss_selection_keeps_draw_across_setup_markers_and_pre_main_reordering` (`:582-668`) invokes
  the actual `History::select` and `marker_evidence` used by `report`. It covers both original marker
  cases, a one-marker span, unissued/evicted evidence, exact-marker prioritization, retained omissions,
  zero cap and empty markers. It supplies the recorded span as CPU data; the actual thread-local
  producer and all mesh issue sites were checked in source as described above. It does not duplicate
  the selection algorithm. No Vulkan callback/command execution is claimed by this test.

These regressions address the original tests' missing production transitions. No further regression
or repeat suite is requested absent another change/failure; lead already executed both in the suite.

### Gates

- **Lead-executed, supplied and corroborated by ledger `:70`:**
  `cargo test -p graphics --lib --jobs 4`, cwd `D:\Projects\RecompLauncherEngine`,
  `CARGO_TARGET_DIR=D:\Projects\RecompLauncherEngine\target\OOTWAIT`: **107 passed / 3 ignored**,
  including both named new regressions. Not independently rerun; no new raw execution log inspected.
- **Agent compile evidence, not auditor execution:** `sendback-result.json` records exit 0 for
  graphics all-targets check and lib test/no-run. Its `tests_executed: 0` describes the original
  agent handoff, before the supplied lead execution, and does not contradict that result.
- **Auditor:** dedicated source/artifact reads and searches; scoped
  `git diff --no-index --check -- /dev/null docs/waves/Audit-01.md` from explicit cwd
  `D:\Projects\RecompLauncherEngine` found no whitespace errors (LF/CRLF advisory only).
  No production diff/check command, test, Cargo, shader tool, GPU or game command executed during
  this recheck.
- **Unrun/blocked:** GPU/device and release gates; clippy remains blocked/unassessed at
  `D:\Projects\RecompLauncherEngine\src\runtime\src\rt.rs:119`, not passed. Existing formatting
  differences and `spvfix.rs:593` warning remain recorded baseline matters, not new recheck defects.

### Evidence

Read the current protocol and ledger, preserved original audit, DG send-back file table/report
(`D:\Projects\RecompLauncherEngine\docs\waves\DG-01.md:212-345`) and
`D:\Projects\RecompLauncherEngine\build\DELEGATION\DG-01\sendback-result.json` in full.
Read the full current `lost_bindings.rs`, current `lost.rs` checkpoint/loss-report path, and current
`exec.rs` real cache identity, stage encoding returns, bind selection and all mesh evidence issue sites.
The closure is based on those production paths and tests, not the agent's corrected-status assertion.
No new screenshots, game evidence or fault-cause investigation. HEAD/dirty baseline and last candidate
library identity remain as supplied above; no fresh release build or hash verification performed.

### Unproven

Device operation of the recorder, exact faulting instruction, stale lifetime versus arithmetic cause,
and gameplay/rendering correctness remain unproven. Setup spans are conservative recording associations,
not GPU execution order; interleaved tokens can produce extra candidates. Output caps/evictions and
unsupported or unobserved metadata remain explicitly incomplete evidence. Fourteen queue-held references
and allocator/container overhead are outside the 16 MiB history payload budget. Original descriptor-buffer
mesh-buffer scope and driver-address precision limitations stand.

Both shared and HARNESS3 stop latches remain active per the current brief/ledger; neither was edited.
No GPU verification or resumption authorization follows from closing these diagnostic findings.

### Not built

No source/test/evidence changes, compilers, tests, GPU/game/replay, release build/relink, subagents,
staging or commits. No fixes remain requested for A01-1/A01-2. Next handoff is lead recording acceptance
of the corrected CPU/reviewed diagnostic scope; prior DH/DS dispositions stand.

**Release:** only this report addendum was written via `apply_patch`; original findings/history remain
intact. Compiler/test/GPU leases were never acquired. No audit-owned worker, background process, game,
helper or resource claim was created; synchronous report-check command completes before handoff. Report
ownership returns to the lead. This does not assert state of unrelated owner processes.

### Slips

None.

### Questions

None. No additional owner choice or GPU experiment is needed to close the two source-level findings.
