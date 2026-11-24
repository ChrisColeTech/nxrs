# THERMAL-ENGINE — owner hold source correction

## Current verification status — lead update, 2026-11-23

The chronological implementation reports below retain their original scope and test results. Their earlier
"integration uncompiled" statements are superseded: the lead's bounded, single-worker
`cargo check -p graphics --lib --offline --locked --jobs 1` passed in **10.71 s**, using the existing external target
and an independent 60-second job deadline. This checks library integration, not test-only modules or GPU execution.
Latest standalone results: **16 production-helper tests and 2 fatal-fixture tests passed**. Independent review
closed E1/E2 and the other tracked harness findings; see `THERMAL-REVIEW.md` for the final disposition.

No release/game rebuild or GPU verification followed. Live NDVE remains quarantined in source, deployed binaries
have not received these fixes, and the owner hold remains active. Thermal root causality is still unproven.

## File table (recorded before code changes)

| File | Intended correction |
|---|---|
| `src/graphics/src/vk/queue.rs` | Separate one-use descriptor pools from reusable guest pools; bounded completion failure policy |
| `src/graphics/src/vk/submission_safety.rs` (new, std-only) | Production ownership and completion-budget helper with bounded CPU regression tests |
| `src/graphics/src/vk/newndve/kernels.rs` | Transfer successful native pool allocations to submission transient ownership |
| `src/graphics/src/vk/exec.rs` | NDVE counter ownership before recording; quarantine unsafe live integration |
| `src/graphics/src/vk/newndve.rs` | Explicit refusal of uncertified live modes under owner hold |
| `src/graphics/src/vk/newndve/tests.rs` | Update live-mode refusal regression |
| `docs/waves/THERMAL-ENGINE.md` | This report |
| External `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine` | Small CPU test executable and evidence |

Baseline HEAD: `143515097a0fa134c6fc4be839971692a39ac4de`. Extensive pre-existing tracked modifications and
untracked NDVE files. `queue.rs` and `exec.rs`
already contain unrelated changes; preserve them. No harness edits, generators, blanket formatting, commits,
games, Vulkan tests, replays, release builds or Cargo dependency builds. No subagents.

## Built

Implemented source corrections; only the standalone production helper was compiled.

1. **Causal resource correction:** `DescriptorPools<P>` separates `reusable` guest pools from private transient
   native pools. Both native recorder paths register successfully allocated pools before descriptor updates or
   commands. Ordinary `Sub::alloc_set` accesses only `reusable`; completion resets those and destroys/drains the
   native list after successful completion of that exact submission. `dpool_i = 0` still resets the guest reuse
   cursor. Native pools no longer accumulate in recycled submissions. Existing descriptor-set allocation-error
   branches still destroy the local pool immediately, before any command references it.
2. **Counter ownership correction:** `ndve_shadow_compare` captures raw buffer/pointer values, then pushes the
   `Buf` into `sub.transient` before the fill/barrier and either fallible comparison. Failure of either compare
   therefore leaves ownership with the submission; any recorded commands remain backed until completion. The
   readback callback is installed only after both record operations succeed and still precedes buffer destruction.
3. **Finite completion policy:** the production helper uses monotonic elapsed time plus charged timeout slices,
   with a **5-second total budget**, waits of at most **250 ms**, and at most **20 TIMEOUT retries** even if the
   driver returns early. A late success or error at/after the elapsed deadline is rejected. Only driver success
   within budget creates the completion receipt. Deadline/driver failure logs a clear watchdog error and calls
   `std::process::abort()` before CPU writebacks, queries, callbacks, releases, recycling or done publication.
   The failure branch does not invoke `lost::check`, fault queries, readback or destructors. This changes completion
   wait errors to immediate termination without attempting additional driver diagnostics.
4. **Quarantine, not a parity/fallback repair:** `on`, `1` and `check` explicitly return a hold error and resolve
   to `Off` with a refusal log. `ndve_dispatch` therefore returns false before capturing/consuming guest work;
   ordinary guest dispatch remains intact. A second, unconditional production permission check at host-graph entry
   rejects before its first allocation or recording. Removed the global `NDVE_CERTIFIED` boolean and the branch
   that consumed later guest frames based on one successful comparison. A retained comparison callback describes
   an observation only. The allowed live graph/shadow attempt budget is **zero**, including after mismatch.
   Offline/synthetic kernel APIs and arithmetic are retained. No math or parity threshold was changed.

### Narrow partial-error and integration audit

- `record_graph` tensor/dummy/luma scratch uses `Sub::gpu_scratch`, which registers successful buffers in
  `sub.arena` before returning handles. A later preflight/record error does not orphan those successful buffers.
  Arena completion still keeps at most one ordinary 64 MiB scratch chunk per recycled submission; this is existing
  policy, not a measured whole-engine memory bound.
- `Kernels::from_sources` owns layout/sampler/pipelines in its local `Kernels`, whose Drop releases partial
  initialization. Shader modules are destroyed after pipeline creation, including error; partial pipeline results
  are also destroyed. The shared successful kernels remain intentionally process-owned.
- NDVE image preflight resolves views through existing image ownership (`image.rs` inserts created views into
  `im.views`); it is not a private untracked NDVE allocation list.
- **Unsafe live ordering confirmed:** `record_graph` receives guest storage views and writes network/final output
  images while later operations can still fail. The caller's replay-after-error branch cannot undo those writes.
  A complete graph is built on each frame, with shadow work while uncertified and no mismatch attempt budget in
  the baseline. A process-global one-frame boolean cannot certify changing resources/weights/frames. These paths
  are quarantined rather than represented as fixed transaction/certification logic.
- **Additional source-proven shared allocator gap, outside writable scope:**
  `src/graphics/src/vk/mod.rs::Gpu::buffer`, lines 182–198, creates a Vulkan buffer and then propagates allocator
  failure with `?` without destroying it. Bind failure similarly propagates without destroying the buffer or
  explicitly returning its allocation to `self.alloc.free`. NDVE scratch/counter calls use this allocator.
  This file was not edited; scope extension is requested below. The successful-buffer ownership fix does not
  repair failures internal to this shared allocator.

## Tests

**6 passed, 0 failed, 0 ignored**, single test process, `--test-threads=1`, reported test time **0.01 s**.
No device initialization, dependency builds, sleeps, large buffers or background workers.

| Production-helper regression | Result / invariant |
|---|---|
| 10,000 submission cycles, 18 native pools each, 3 fixed guest pools | Every completed cycle retains exactly **3 guest + 0 native**; peak live mock handles **21**; **180,000 native destroys**, exactly once; **30,000 guest resets** |
| Earlier/wrong fence; repeated cleanup | Earlier receipt cannot reset or destroy anything; duplicate cleanup cannot destroy a native handle twice |
| Partial recording failure | Previously registered pool remains owned until its own successful completion |
| Persistent TIMEOUT, including spuriously early driver return | Stops after 20 waits; retained ownership unchanged; no completion receipt |
| Monotonic deadline and late success / timeout / driver error | Deadline is terminal; no further wait, and late success is not accepted |
| Successful wait after timeout / immediate driver error | Only real in-budget success returns a receipt for the requested fence |
| All live environment modes | `on`, `1`, `check` refused; absent/off/0 permitted as off; host-entry permission denied |

The bound is **per recycled submission with the tested fixed guest demand**. Generally post-completion native
retention is zero and retained guest pools remain at their ordinary high-water mark. This does not establish a
whole-queue/in-flight allocation bound or eliminate per-dispatch creation cost. The old append-and-retain rule
would retain 180,003 handles for that modeled workload (arithmetic prediction, not an executed baseline test).
The new helper did not exist before this change; no claim of running a pre-fix red test is made.

Updated `newndve/tests.rs` mode regression is **uncompiled/unrun** as graphics integration. Buffer error ordering
and the Vulkan call sites were source-reviewed, not executed. No graphics integration compilation is authorized
by this task; standalone tests/source review cannot substitute for it.

## Verified

Read owner hold, incident report, architecture guide, delegation protocol, relevant interface/port status and
current NDVE/queue/allocator sources. `git diff --check` on the modified tracked source returned exit 0; Git
reported existing working-copy LF/CRLF conversion notices, no whitespace errors.

Compiler: `rustc 1.99.0 (b940084d7 2026-09-28)`.

```powershell
rustc --edition=2021 --test "D:\Projects\RecompLauncherEngine\src\graphics\src\vk\submission_safety.rs" -o "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\submission-safety-tests.exe"
& "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\submission-safety-tests.exe" --test-threads=1 --nocapture
```

Compile and test ran sequentially in one shell command with a **30,000 ms tool timeout**, exit 0. External
`engine/README.md` records the captured output. No owned worker remains; subsequent process inspection found no
`rustc` or `submission-safety-tests` process. No agent was spawned or commit made.

### Existing incident evidence inspected (no rerun)

Under `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-agent2\runs\`:

- `20261123-014306-oot-production-gameplay/provenance.json:18` records `NXGFX_NDVE=on`.
  Its `run.log:197–198` records the older relaxed comparison (133,105 network texels and 49,491 resampler texels
  differing by at most one step) at 9.18 s and host activation at 9.19 s. This is distinct from the latest exact
  comparison build; no threshold relaxation was introduced here.
- `20261123-024113-oot-ndve-production-gameplay-final/provenance.json:18` also records `on`.
  `run.log:202–203` records exact match at 9.00 s and host activation at 9.02 s; `:255–257` records **473 presents**,
  submission **4764** not completing after 5 s at **22.24 s**, and the no-new-image warning.
- The incident report records NVIDIA events 14/153 at **02:41:35–02:41:40** and subsequent unexpected-shutdown
  events. Event timing is correlation, not an exact GPU instruction trace or proof of resource/thermal causality.

## Unproven

- **Changed graphics integration has not been compiled.** Next verification needs lead authorization for an
  appropriately safe CPU-only integration compile and independent source review. This helper pass is not a GPU
  certification and does not authorize lifting the hold.
- Shared `Gpu::buffer` partial-allocation cleanup remains open pending the explicit file-scope extension.
- GPU arithmetic/parity, guest image transactional isolation, per-frame certification identity/invalidation,
  bounded whole-graph resources and safe failure/fallback policy remain unproven. The prototype stays guarded off.
- No exact faulting instruction or leak-to-temperature/reboot causal link has been established.
- The userspace guard cannot interrupt a driver call that fails to return, prevent already-submitted GPU work,
  or guarantee reset/TDR avoidance. Its bound applies when control returns from the driver and the thread is
  scheduled; it is not a replacement for external supervision. No TDR/power/clock/fan settings were changed.
- No GPU readiness, performance, gameplay or thermal-safety claim.

## Slips

None.

## Questions

Authorize a narrow extension to `src/graphics/src/vk/mod.rs::Gpu::buffer` to clean up buffer creation on allocator
failure and both buffer/allocation on bind failure? Recommended: yes, with a small std-only production failure-
injection helper if needed; graphics integration compile remains separately gated. Source: lines 182–198 and the
explicit `free_buffer` ownership protocol at 225–231. No other product choice is outstanding.

## Approved allocator extension — file table (before extension code changes)

The owner approved the requested extension. The earlier open-scope question is superseded by this addendum.

| File | Authorized extension |
|---|---|
| `src/graphics/src/vk/mod.rs::Gpu::buffer` only | Clean up partial creation/allocation/binding errors, preserving successful ownership and allocation policies |
| `src/graphics/src/vk/submission_safety.rs` | Std-only production create/allocate/bind cleanup helper and injected-failure tests |
| `docs/waves/THERMAL-ENGINE.md` | Append implementation/results |
| External `engine/README.md` and standalone test executable | Updated small CPU-only evidence |

`mod.rs` was already dirty before this extension; its current buffer implementation was reread. The owner hold
continues: no Vulkan initialization, games, Cargo or release builds, harness edits, generators or blanket format.

### Built

The approved `Gpu::buffer` correction is implemented. Only its create/allocate/bind block changed in `mod.rs`.
The shared std-only production `create_bound_buffer` helper now owns partial construction:

- Create error returns the original error with no destroy/free calls.
- Allocation error destroys the created buffer once, then returns the original allocation error.
- Bind error destroys the buffer once, then returns its allocation to the allocator once, then returns the
  original bind error. Buffer destruction precedes memory release.
- Success moves both owners out of the helper into the existing `Buf` construction without cleanup.

The allocation callback uses an explicit block-local allocator guard. That guard is dropped before returning
to the helper, so the bind-failure cleanup can safely acquire the allocator mutex. Cleanup uses the allocator's
`free` directly, following the existing `free_buffer` convention, rather than calling `free_buffer`: `HOST_LIVE`
has not yet been incremented on a failed construction. Original error strings, allocation descriptors, memory
location, size/cap policy, usage flags, host accounting, successful return fields and eventual owner cleanup are
preserved. The helper uses moved, non-Clone allocation ownership and one-shot cleanup callbacks.

This supersedes the earlier open allocator-scope finding: the source gap is corrected, with Vulkan integration
verification still pending.

### Tests

**10 passed, 0 failed, 0 ignored**, including four new buffer-creation regressions and the six existing safety
tests. Reported test time **0.01 s**. Compiled the exact production `submission_safety.rs`, with no Vulkan imports
or initialization, and ran a single test process using `--test-threads=1 --nocapture`.

| Injected outcome | Verified cleanup / ownership |
|---|---|
| Create failure | No allocation, bind, destroy or free; original error preserved |
| Allocation failure | Created buffer destroyed exactly once; no bind/free; allocator mutex available during destroy |
| Bind failure | Exact event sequence: buffer destroy once, allocation free once; original error preserved; allocator mutex available during bind/destroy/free and afterward |
| Success | Exact create/allocate/bind order; both non-Copy/non-Clone owners returned intact; neither cleanup callback runs |

The lock regressions use `Mutex::try_lock` for bind/cleanup so a leaked guard causes an immediate test failure,
not a hanging test. No GPU or baseline pre-fix execution is claimed. The existing 10,000-cycle descriptor test
also remained green: retained **3 guest + 0 native**, peak **21**, **180,000** exactly-once native destroys.

### Verified

Reviewed the current `Gpu::buffer` and `free_buffer` ownership/accounting paths and the pre-existing `mod.rs`
diff before editing. Post-edit source review confirms the allocator guard ends before any helper cleanup, and
`HOST_LIVE` is updated only after successful binding, as before.

Commands (executed sequentially with `&&` in one shell call, **30,000 ms timeout**, exit 0):

```powershell
rustc --edition=2021 --test "D:\Projects\RecompLauncherEngine\src\graphics\src\vk\submission_safety.rs" -o "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\submission-safety-tests.exe"
& "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\submission-safety-tests.exe" --test-threads=1 --nocapture
```

`git diff --check -- src/graphics/src/vk/mod.rs` returned exit 0, with only the working-copy LF/CRLF notice.
Captured output is appended to external `engine/README.md`. Process inspection after testing found no
`rustc` or `submission-safety-tests` process. **Owned execution resources released; no running workers.**

### Unproven

Vulkan integration remains **uncompiled and unexecuted**. These tests verify the actual production cleanup
decision helper with mock resources; they do not compile the ash/gpu-allocator callback wiring or prove driver
behavior. Successful release is observed for mock owners; no real GPU resources were created in this work.
Graphics integration compilation still requires lead authorization. Native NDVE remains quarantined, the owner
hold remains active, and no GPU-readiness or thermal-safety claim is made.

### Slips

None.

### Questions

None; the requested file-scope extension is approved.

## E1/E2 independent-review send-back — file table (before code changes)

Read `THERMAL-REVIEW.md` E1/E2 failure sequences. The following work supersedes the original synchronous
watchdog logging/abort design and closes the previously unbounded guest pool retry path. Owner hold unchanged.

| File | Intended correction |
|---|---|
| `src/graphics/src/vk/queue.rs` | Immediate fatal-72 handling; bounded guest descriptor allocation using production policy runner |
| `src/graphics/src/vk/submission_safety.rs` | Std-only/native-OS fatal helper, bounded descriptor policy and CPU regressions |
| External `engine/fatal_logger_tests.rs` | Bounded CPU children invoking the exact production helper with the real existing `log.rs` sink blocked/panicking; no graphics crate build |
| External test executables and `engine/README.md` | Small CPU-only test evidence |
| `docs/waves/THERMAL-ENGINE.md` | This history and results |

Harness read-only coordination: `scripts/run_game.py` now classifies child exit 72 as
`GPU COMPLETION/RESOURCE SAFETY FATAL`, alongside 71. No engine dependency on log delivery will be required.

### Built

**E1 implemented:** completion failures now go directly through the production `resource_safety_result` to
`resource_safety_fatal`. This path performs no formatting, logging, lock acquisition, driver fault queries,
cleanup, panic/unwind, or fence publication. On Windows it calls
`TerminateProcess(GetCurrentProcess(), 72)`, which does not return on successful self-termination; only a failed
native termination reaches the abort fallback. Unix uses `_exit(72)` (no atexit/stdio flush); other unsupported
targets fall back to abort. The descriptor receipt-mismatch fatal branch uses the same helper with no logger.
No diagnostic message is promised: **72 is the stable GPU completion/resource safety fatal protocol**, as
distinct from ordinary guest wait timeout, existing device-lost 71, or harness-owned termination 124.

**E2 implemented:** `Sub::alloc_set` now invokes the std-only production `allocate_descriptor_set` policy runner.
It traverses the remaining existing reusable pools at most once, advances only for Vulkan
`ERROR_FRAGMENTED_POOL` (-12) or `ERROR_OUT_OF_POOL_MEMORY` (-1000069000), then creates **at most one fresh pool**.
Any allocation failure on that fresh pool, any terminal error on an old pool, pool-creation failure or invalid
cursor returns an error to the immediate fatal-72 handler. The same production raw-VkResult classifier is used
by the tests and queue wiring. There is no general `Err(_)` capacity retry.

The bound per request starting with K reusable pools at cursor i is at most **K-i+1 set allocation calls and
one pool creation** (i <= K). A terminal error on the first attempted old pool uses **one allocation call and
zero creates**. Previously exhausted pools before the cursor are not revisited. Created pools are registered
before their first set allocation and remain owned on error; there is no early destruction of recorded/in-flight
resources. Guest pool sizes/reuse and the guest/native completion-owned lifecycle split are preserved.

### Tests

1. **Production helper suite: 16 passed, 0 failed, 0 ignored, 0.01 s.** Exact source compiled with
   `rustc --test`, one test thread. Six new descriptor tests cover:
   - Three exhausted/fragmented old pools, then one successful fresh pool: exactly **1 create / 4 attempts**.
   - Existing later pool succeeds: no fresh create, no revisiting pools before the cursor.
   - Persistent fresh-pool errors (capacity, host/device OOM, device lost, unknown), with zero or three old pools:
     exactly **1 create / 1 or 4 attempts**, no further retry; all created owners remain retained.
   - Terminal/wrong errors on an old pool: exactly **0 creates / 1 attempt**, unchanged pool ownership.
   - Pool-create failure and invalid cursor: no invalid subsequent driver work or owner removal.
   The prior ten buffer/ownership/watchdog/quarantine tests also pass. The 10,000-cycle result remains exactly
   **3 guest + 0 native retained**, peak **21**, **180,000 native pools freed once**.
2. **Fatal fixture: 2 reported tests passed, 0 failed, 0.04 s** (one parent regression and its child-entry wrapper;
   16 helper tests filtered out in this separate fixture run). The fixture includes the exact production helper
   and the **real unchanged `src/graphics/src/log.rs`**, not a copied logger/fatal implementation. It spawned
   three CPU-only children sequentially, each with a **3-second parent deadline**:
   - Real normal sink already blocked on a held mutex: completion deadline failure exited **72**.
   - Real normal sink configured to panic if entered: terminal descriptor device-loss error exited **72**.
   - No sink installed, stderr lock held by another thread: completion deadline failure exited **72**.
   A panic hook would exit 91, an unwound destructor 93, and a returned fatal path 94; none occurred. The fixtures
   block on mutexes/channels, with no CPU spin stress. Parent polling sleeps 10 ms. Only the retained owned child
   handle is eligible for timeout cleanup; all three exited 72 normally for this protocol and were reaped.

Both compile/test commands used **30,000 ms tool timeouts**, ran sequentially, and returned exit 0. No Vulkan or
GPU allocator initialization. No pre-fix failure execution claimed; the source review supplied the failure traces.

### Verified

Read the actual completion error branch, guest `alloc_set`, production logger and harness 71/72 classification.
`run_game.py::engine_status` (lines 410–421 at inspection) consumes retained child status 72 and records `exit-72`
as GPU completion/resource safety fatal. Engine behavior now matches that protocol without requiring log delivery.
This is read-only coordination; the harness was not edited or run by this work.

Executed from `D:\Projects\RecompLauncherEngine` (each compile followed by its test using `&&`):

```powershell
rustc --edition=2021 --test "D:\Projects\RecompLauncherEngine\src\graphics\src\vk\submission_safety.rs" -o "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\submission-safety-tests.exe"
& "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\submission-safety-tests.exe" --test-threads=1 --nocapture
rustc --edition=2021 --test "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\fatal_logger_tests.rs" -o "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\fatal-logger-tests.exe"
& "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\fatal-logger-tests.exe" fatal_ --test-threads=1 --nocapture
```

`git diff --check -- src/graphics/src/vk/queue.rs` returned exit 0 with the LF/CRLF notice only. Post-test process
inspection found no `rustc`, `submission-safety-tests` or `fatal-logger-tests` process. **All owned test children,
their blocking fixture threads, process handles and compiler/test execution resources are released.**
Evidence/output is appended to external `engine/README.md`; fixture source is retained there for reproducibility.

### Unproven

Changed Vulkan integration remains **uncompiled and unexecuted**; source inspection and std-only policy tests
cannot certify ash wiring, Vulkan validity or hardware behavior. The Windows native fatal binding itself was
compiled and executed by these CPU children; Unix `_exit` and the native-call-failure abort fallback were not
executed. Harness durable-latch integration is source-coordinated, not end-to-end rerun by this engine task.

Immediate userspace self-termination still cannot guarantee progress of a wedged OS/driver, cancel already-issued
GPU work, or prevent TDR/reset/thermal faults. This limitation is separate from the now-removed blocking normal
logger after the fatal decision. The descriptor bound is per allocation request, not a global memory cap.
Live NDVE quarantine and the owner hold remain active. No GPU readiness or thermal-safety claim; no games, GPU
workloads, Cargo, release builds, production data/latches or commits.

### Slips

None.

### Questions

None.
