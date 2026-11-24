> **Current disposition (2026-11-23):** H1–H4, E1–E2 and R1 are closed for their reviewed
> source counterexamples. Graphics library integration passed the lead's bounded CPU-only compile check.
> See [Final focused recheck](#final-focused-recheck--r1-only-2026-11-23) for the latest independent disposition.
> Earlier findings and verification limits below are retained as chronological history.
> **Games/GPU workloads remain on hold; thermal root cause and runtime safety remain unproven.**

| Scope / file table | Access in this review |
|---|---|
| Project root: `D:\Projects\RecompLauncherEngine`; `AGENTS.md` | Read-only; current owner hold applied |
| `docs/waves/THERMAL-INCIDENT.md`, `THERMAL-HARNESS.md`, `THERMAL-ENGINE.md` | Read-only |
| `scripts/harness_job.py`, `harness_monitor.py`, `run_game.py`, `gamectl.py`, `play_scenario.py` | Read-only integration audit |
| `scripts/test_harness_thermal.py`, `test_gpu_stop.py`, `test_play_scenario.py`, `test_run_triage.py` | Read-only test-coverage audit; no execution |
| `scripts/harness_idle_fixture.py`, `scripts/gpu_stop.py` | Read-only supporting ownership/test/persistence trace |
| `src/graphics/src/vk/queue.rs`, `newndve.rs`, `newndve/kernels.rs`, NDVE portions of `exec.rs`, `mod.rs::Gpu::buffer` | Read-only engine integration audit |
| `src/graphics/src/vk/submission_safety.rs` | Exact production CPU helper discovered from the engine report; read-only |
| `src/graphics/src/vk/newndve/tests.rs`, `src/graphics/src/log.rs` | Read-only mode-regression and fatal-path trace |
| External incident summary, interrupted run log, OOT stop marker, engine evidence README (paths below) | Read-only retained evidence |
| `docs/waves/THERMAL-REVIEW.md` | **Sole file written** |

# Independent thermal safety source review — 2026-11-23

## Disposition

**CHANGES REQUIRED: six blocking source findings remain. The owner hold remains in force.**

This is a fresh review of actual source and retained evidence, not adoption of the implementing agents' conclusions.
No Cargo, compiler, build, test, game, replay, telemetry query, GPU workload or Vulkan initialization was executed.
No agents, commits, source edits or production-save accesses were performed. All filesystem paths used for reads
were explicit; relative paths in this report are relative to the project root in the table.

The principal corrections are real: creation-time job assignment, independent process-handle supervision,
submission-owned native pools, early counter-buffer ownership, live NDVE quarantine and partial buffer cleanup
are present in production wiring. They do not close the failure sequences below.

Severity **High / blocking** means a concrete failure of a claimed containment, fail-stop, save-protection or
resource-bound invariant. It does not mean the defect is proven to have caused either reboot. Some findings are
latent behind the current missing-sensor refusal; that refusal must remain intact.

## Blocking findings

### H1 — High: cleanup uncertainty does not reliably withhold save restoration

**Locations:** `scripts/run_game.py:232–243,511–523`; `scripts/harness_job.py:258–294,363–366`;
`scripts/play_scenario.py:357–379`.

**Failure sequence:**

1. `_run` cancels and drains the owned game job, then joins the `during` callback for three seconds.
2. A callback still alive at that point causes `RuntimeError('scenario worker did not stop; cleanup unconfirmed')`,
   not `CleanupError`. The capture thread is also joined for three seconds but its continued liveness is not checked.
3. `scenario()` only changes `cleanup_confirmed` to false for `CleanupError`. A `RuntimeError` therefore reaches
   `SaveTransaction.finish()` with that flag still true. Restoration can occur while the callback is still running.
4. There is a second route through the same incorrect proof boundary: `TerminateJobObject` or
   `QueryInformationJobObject` failure raises ordinary `OSError` through `checked()`. `Job.close()` closes handles
   in its `finally` even without a successful drain. Handle closure requests termination; it does not establish
   that a kernel-held process has exited. An ordinary error can then allow restoration without exit confirmation.

This is stronger than the documented limitation that custom callbacks must cooperate: detecting noncooperation
must actually prevent restoration. It also prevents an honest claim that all callbacks/helpers have been released.
In `bounded_run`, an exception from `job.close()` additionally skips `child.close()`; the retained child handle
needs its own unconditional cleanup.

**Minimal correction:** gate restoration on an explicit positive cleanup result covering job drain and worker
joins, rather than the absence of one exception class. At minimum, normalize every unconfirmed termination,
query, wait or callback-join outcome into `CleanupError`, preserve that outcome through outer cleanup, check both
threads, and close child handles in nested `finally` blocks. Preserve `save-before` and report recovery required
on every unconfirmed path. Keep job termination ahead of joins and evidence.

**Coverage gap:** `test_play_scenario.py:15–34` injects the already-correct exception class directly; it does not
exercise the real `RuntimeError`/Win32-error paths. Runner tests mock threads and jobs.

### H2 — High: the machine lease excludes the save transaction

**Locations:** `scripts/play_scenario.py:116–181,357–377`; `scripts/run_game.py:246–288`;
`scripts/harness_job.py:158–169`.

**Failure sequence:**

1. Scenario A owns the machine lease and its game is writing a save.
2. Scenario B, targeting the same save, calls `SaveTransaction(...).begin()` **before** entering `run_game.run()`
   and before acquiring the lease. Its backup can therefore be inconsistent.
3. B is refused by `MachineLease.__enter__` with `SystemExit`. B nevertheless calls `finish()` in `finally`,
   renaming/removing/restoring the save while A is still using it.
4. Even without a refusal, A releases the lease when `run_game.run()` returns, before A's `finish()`. A subsequent
   cooperating launch can race A's restoration. A thermal/latch refusal also currently performs a restoration
   transaction despite never having started a game.

The named mutex correctly serializes cooperating **runs**. Its current extent does not serialize the save-writing
transaction that surrounds those runs. This is independent of arbitrary noncooperating manual compiler starts.

**Minimal correction:** acquire the machine lease before inspecting/backing up the save and retain it through
confirmed process/callback cleanup and restoration. A lease-acquisition refusal must never begin or finish a save
transaction. Track whether this transaction launched a workload so a prelaunch refusal does not unnecessarily
replace production saves. Keep the same lease ownership through the runner boundary; do not introduce a gap
between releasing one lease and acquiring another.

**Coverage gap:** the existing refusal regression checks preservation of an absent save, not a refusal while a
different lease holder is writing an existing save. No inspected test asserts lease ownership during `finish()`.

### H3 — High: a thermal sample can expire before the workload is launched

**Locations:** `scripts/run_game.py:268–276,298–319,333–341,383–395`;
`scripts/harness_monitor.py:80–104`.

**Failure sequence:**

1. `run()` samples temperatures and validates sample age once.
2. Before `Job.launch`, the runner performs two event-log queries, claim/process checks, compiler checks,
   provenance work and filesystem writes. These occur after the thermal decision. Individually bounded,
   successful queries plus other preflight work can consume more than the twelve-second sample-age limit.
3. Immediately before launch, only the compiler/latch/heartbeat gates run. There is no new temperature sample
   or age check. `job.heartbeat()` can renew the monitoring lease and `Job.launch()` can resume the child using
   an expired thermal decision. A temperature that has become hot is similarly not observed at the launch gate.

The watchdog's fifteen-second monitor lease does not enforce the separate twelve-second sensor-age contract.
The sensor read also predates creation of that watchdog. Today's adapter always returns unknown CPU temperature,
so it refuses earlier; the defect would become reachable once a usable provider is supplied.

**Minimal correction:** perform a fresh fail-closed thermal check after expensive preflight/provenance work and
immediately before guarded launch. Check sample age again at the final resume boundary if creation can consume
the freshness budget. A launch heartbeat is not evidence of a fresh safe observation. Retain the absolute job
deadline and no-override policy.

**Coverage gap:** tests inject initial refusal and midrun sensor loss, but not a valid initial sample that becomes
stale/hot while launch preflight runs.

### H4 — High: the new engine fatal path is not connected to the durable harness stop gate

**Locations:** `src/graphics/src/vk/queue.rs:812–829`; `scripts/harness_monitor.py:138–157`;
`scripts/run_game.py:424–440,481–510,527–557,279–283`.

**Failure sequence:**

1. A completion wait exhausts its budget or returns a driver error. The engine emits
   `GPU completion watchdog: submission ... failed (...)` and aborts.
2. `RunLog.FAULT` does not recognize that fatal message. A `Driver(ERROR_DEVICE_LOST)` debug value is also not
   the regex's `VK_ERROR_DEVICE_LOST` spelling. Deadline errors need not produce any driver-reset event.
3. The runner specifically latches exit **71**, but `abort()` establishes no exit-71 protocol. If the process exits
   before the next `while running()` iteration, the log is not scanned again on exit at all.
4. With no matching reset event, the runner can return `why='exited'` with a nonzero abort status, mark the intent
   completed, and leave the shared stop latch absent. A new artifact directory is not prevented from launching
   by this fatal GPU event. The nonzero exit is retained; the problem is missing durable refusal, not an assertion
   that every consumer interprets the exit as success.

The handover path has a related hole: a successor is monitored by `alive(pid)`, while `rc` and final exit checks
remain those of the original `p`. A successor that exits 71 between observations can likewise evade the latch.
Job-wide final cleanup still kills descendants; it does not recover the successor's exit status.

**Minimal correction:** define a stable non-unwinding engine-fatal status/signal understood by the harness,
recognize the watchdog message, and inspect the bounded remaining log after job termination on every exit.
Persist the GPU fatal latch before returning/completing the intent. Retain an ownership-verified process handle
for each adopted successor and collect that process's status; do not substitute the root's status or kill a
reopened arbitrary PID. Do not rely solely on log delivery for fatal classification, and do not turn an ordinary
guest API timeout into a fatal status.

**Coverage gap:** current tests cover root exit 71, reset events and mocked successor liveness, not the new
watchdog message/abort protocol or an actual successor's fatal exit without a reset event.

### E1 — High: fatal completion termination is still behind synchronous logging

**Locations:** `src/graphics/src/vk/queue.rs:821–828`;
`src/graphics/src/log.rs:8–14,21–31`.

**Failure sequence:** after the finite wait helper returns an error, the completion thread calls `log::red` before
`process::abort`. `red` calls a synchronous arbitrary sink, or `eprintln!` when no sink is installed. A blocked
sink, stderr lock or output write prevents reaching `abort`; a panicking sink can unwind before it. This is not
the already-documented case of a Vulkan call failing to return: the driver has returned and the fatal decision
has already been made. Other threads can remain alive while the supposed five-second fatal path is stuck in I/O.

The source correctly avoids GPU cleanup, writebacks and completion publication on the ordinary error branch.
It does **not** yet make termination independent of diagnostics. An external job deadline limits some consequences
when the harness is used, but is not the claimed engine fail-stop.

**Minimal correction:** make the fatal decision lead to unconditional non-unwinding termination without entering
the normal logger or waiting on evidence. If a cause must be retained, publish a fixed fatal code through an
already-established nonblocking mechanism and let the external owner record evidence. A logger `try_lock`
alone is insufficient if the subsequent write can block. Coordinate the fatal code with H4.

**Coverage gap:** standalone helper tests end at `CompletionError`; they do not execute the production fatal
callback. They cannot prove that the post-helper path terminates with a blocked/panicking log sink.

### E2 — High: guest descriptor allocation retries every API error without a bound

**Location:** `src/graphics/src/vk/queue.rs:383–429`, especially `Err(_) => self.dpool_i += 1`.

**Failure sequence:** `allocate_descriptor_sets` fails for a terminal error such as host/device memory exhaustion
or device loss. The loop treats it as ordinary pool exhaustion, advances the cursor and creates another large
pool. If pool creation continues succeeding while set allocation continues failing, it retains each new pool
and repeats without a retry/resource bound. A request that cannot fit even a fresh pool has the same problem.
This happens during recording, before the current submission reaches the completion watchdog; that watchdog
does not bound this loop. With NDVE quarantined, ordinary translated guest dispatches still use this path.

This is a **pre-existing unresolved error-policy defect**, not introduced by the native/reusable split and not
evidence of thermal root causality. It is directly in the reviewed allocation path and leaves a concrete
resource-runaway route despite the new completion bound.

**Minimal correction:** advance pools only for explicitly recoverable pool-exhaustion/fragmentation errors,
bound the fresh-pool retry for a single request, and propagate or fail-stop on other errors. Failure on a fresh
adequately sized pool must terminate the attempt, not allocate indefinitely. Preserve ownership of resources
already referenced by recorded/submitted commands; avoid a cleanup that assumes GPU completion on fatal errors.

**Coverage gap:** the pool-helper tests model ownership/recycling, not the `Sub::alloc_set` Vulkan-error dispatch.

## Source checks that support the corrections

### Harness ownership and supervision

- `harness_job.py:67–99` supplies both `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` and
  `PROC_THREAD_ATTRIBUTE_JOB_LIST` to `CreateProcessW`, with `CREATE_SUSPENDED` and extended startup information.
  Job membership is established by creation, rather than a subsequent assign call. Attribute/setup/create
  failure precedes execution. The normal launch path checks supervision again before `ResumeThread`.
- The inspected ctypes layouts/signatures are consistent with the Windows x64 structures used here. Workload
  inheritance is restricted to input/output handles. The job disables breakaway and enables kill-on-close;
  the watchdog intentionally retains the job handle plus a real runner process handle and events.
- `watchdog()` waits on parent/stop/pulse handles and monotonic deadlines, with no telemetry/evidence calls.
  Parent death and stale monitoring therefore do not depend on the observer thread making another query.
  `Job.close()` attempts termination/drain before signaling and joining the watchdog. Error-proof propagation
  and handle-finalization weaknesses are H1, not an objection to the creation-time assignment itself.
- `bounded_run` contains query/pipe helpers in their own watched jobs; helper descendants remain covered even
  when their root exits. `gamectl.pipe` caps the exchange at two seconds and the worker rechecks job membership.
  Main safety stops call `job.kill()` before latch writes/diagnostic work. No image-name or unowned logged-PID
  kill was found in these paths. PID checks for read-only observations are not termination authority.
- `MachineLease` uses a global Windows mutex and refuses competing threads/processes. The old build/concurrency
  environment opt-outs do not bypass checks. Its save-transaction extent is H2.
- Missing `NXRS_LANE` is resolved and injected into the child environment at `run_game.py:310–324`; claim paths
  use that selected lane. In-process control checks use `CURRENT` job ownership; standalone control requires
  a lane claim with actual named-job membership. Missing lane is not an executable-name ownership fallback.
- Required readings are checked for status, finiteness, age and temperature/resource caps. No environment
  override for unknown/stale/hot readings was found. The present CPU adapter returns cached provider values as
  unknown because it cannot establish acquisition time. Launch refusal on this machine is consistent with source.

### Engine ownership, quarantine and error distinctions

- The exact helper is `src/graphics/src/vk/submission_safety.rs`, included by `queue.rs:26–27` and used by the
  actual completion, pool and buffer-allocation paths; it is not a separate test-only reimplementation.
- Both native descriptor recorders register their successfully allocated pools in `retain_transient` before
  updates/commands (`kernels.rs:511–535,610–669`). Set-allocation failure destroys only the unreferenced local
  pool. Guest allocation uses only `dpools.reusable`. Completion obtains a receipt for `item.value` before
  recycling; transient pools are drained/destroyed and the guest cursor resets. No new double-destroy or
  cross-thread ownership race was identified in this separation.
- `exec.rs:5971–6008` captures the counter handle/pointer and moves its owning `Buf` into `sub.transient` before
  fill/barrier and either fallible compare. The callback is installed only after both compare recordings succeed.
  `queue.rs:863–907` invokes callbacks before freeing transient buffers, after successful completion.
- The completion helper has a five-second elapsed budget, maximum 250 ms waits and a twenty-timeout charged
  fallback. Late success/error is rejected. Production maps only `TIMEOUT` to retry; other wait errors are fatal.
  No wait failure reaches readbacks, callbacks, resource recycling or done publication on the normal branch.
  E1 is the intervening fatal-logging defect. Ordinary `wait_fence` timeout still returns false
  (`queue.rs:1800–1825`); it is distinct from completion-worker failure and not fabricated fence success.
- `Mode::parse` always returns `Off` for allowed values and refuses `on`, `1`, `check` and unknown values.
  `mode()` logs refusal and resolves to `Off`; `ndve_dispatch` returns before capture/consumption. The host-graph
  entry independently calls unconditional `live_ndve_permission()` before its first allocation/recording.
  No environment route around the live gate or remaining `NDVE_CERTIFIED` latch was found. The retained
  `NXGFX_NDVE_COMPARE_8` arithmetic switch cannot enable this gated production path. Low-level synthetic kernel
  APIs remain available; this is not a claim that they are disabled or authorized to run.
- `Kernels::from_sources` uses a partially initialized owner whose Drop releases created layouts, sampler and
  pipelines. Shader modules are destroyed after pipeline creation on success/error, and returned partial
  pipelines are destroyed on error. Successfully shared kernels are intentionally process-lifetime owners.
  Scratch allocations enter `sub.arena` before their handles are returned. No additional partial-initialization
  leak was identified in these inspected native ownership paths.
- `Gpu::buffer:182–211` invokes the production create/allocate/bind helper. Allocation error destroys the buffer;
  bind error destroys it and returns its allocation. The allocator guard ends within the allocation callback
  before bind/error cleanup can reacquire the lock. Host accounting starts only after success; failed construction
  does not call `free_buffer` and incorrectly decrement it. No visible new type/borrow incompatibility was found
  in these callback or completion-receipt call sites. **That is source inspection, not a compile result.**

## Test evidence: what it does and does not establish

`THERMAL-HARNESS.md` reports 93 tests: **92 passed, one skipped**, 4.511 seconds, with bounded CPU-idle children.
The skipped test is reported as the pre-existing optional JSON-schema check. These results were not rerun.

The inspected suite is **not mocks alone**: `test_harness_thermal.py` exercises real Windows jobs, deadline and
heartbeat expiration, runner death, death during suspended launch, descendants surviving root exit, membership,
mutex exclusion and named-pipe timeouts. `test_run_triage.py` uses a real contained pipe server. This is meaningful
primitive-level shutdown coverage. In contrast, the runner safety tests replace `Job`, `MachineLease`, `Monitor`
and threads, and save-cleanup failure is injected at the outer `run` boundary. There is no inspected end-to-end
test tying a real owned idle process tree to a monitor/terminal fault, actual runner cleanup and the save/lease
boundary. The specific failure cases listed above remain uncovered.

External `nxrs-thermal-incident/engine/README.md` retains output reporting **10 passing tests** of the exact
production std-only helper. These establish the helper's finite wait, ownership decisions and partial-allocation
cleanup with mock resources, including lock-availability checks. They do not compile the ash/gpu-allocator wiring,
exercise the fatal logger/termination callback, or exercise guest descriptor-allocation error policy.

When separately authorized, focused CPU-only checks should cover H1/H2 with real idle containment and temporary
saves, delayed preflight for H3, root/successor fatal-status propagation for H4, blocked logging for E1 and
bounded terminal allocation failures for E2. **No such check, compilation or test is authorized or performed by
this review.** The reported passes do not supersede the source findings.

## Already-documented limitations, kept separate from findings

- No supported installed source establishes fresh CPU temperature; the adapter currently refuses launch. This
  is the correct fail-closed result, not a reason to weaken checks or add a user override. No sensor provider was
  installed or queried in this review.
- CPU/GPU temperature, memory and progress thresholds are refusal policy, not hardware-safe operating guarantees.
  Device-wide driver telemetry is not per-process VRAM attribution or a pre-crash thermal trace.
- The changed graphics integration is uncompiled and unexecuted. Standalone helper success is not integration
  verification. A source review cannot establish compiler acceptance, Vulkan validity or driver behavior.
- Native graph writes are not transactional with guest fallback; frame/resource-specific certification and whole
  graph/in-flight resource bounds remain unproven. Quarantine is effective and must remain. Per-submission native
  pool drainage is not a whole-engine memory bound; ordinary reusable pools retain their high-water allocation.
- Polling cannot provide zero overlap with an arbitrary external compiler. Cooperative build entrypoints would
  need the same lease. This limitation differs from H2's race between already-cooperating harness scenarios.
- Userspace termination cannot guarantee stopping a wedged driver/OS, prevent already-submitted work, or prevent
  firmware/power faults. A driver call that never returns defeats an in-thread timer. This limitation differs
  from E1's avoidable synchronous logging after the driver has already returned.
- Recovery intent is per artifact directory, not a global interrupted-run scanner. No historical PID is killed
  on recovery. Save recovery requires established writer exit; neither a timeout nor a closed handle proves it.

## Incident evidence boundary

Read-only retained evidence:

- `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\incident-run-summary.json`
- `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-agent2\runs\20261123-024113-oot-ndve-production-gameplay-final\run.log`
- `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-agent2\OOT-GPU-STOP.json`
- `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\engine\README.md`

Both owner-reported reboots occurred during the actual 01:43 and 02:41 runs, not merely earlier completed probes.
The summary names `20261123-014306-oot-production-gameplay` and
`20261123-024113-oot-ndve-production-gameplay-final`; both lack result/restoration records. The latter actual log
records exact shadow match at 9.00 s, native host activation at 9.02 s, 473 presents, and **submission 4764 not
completed after five seconds at 22.24 s** (`run.log:202–203,255–257`). The OOT stop marker records the matching
run and clustered display-reset observations around 02:41:35–39. Buffered event times do not establish a precise
GPU instruction ordering.

There is no pre-crash CPU-temperature trace in this evidence. Corrupt/missing memory rows, rising private memory,
native activation and a submission timeout establish neither the thermal root cause nor exclusion of hardware
or cooling contributors. No deep causality investigation or incident reenactment was performed here.

## Ownership at completion

**Owned execution resources: none created, none left running.** No child process, watchdog, thread, job, mutex,
GPU/Vulkan object or test/compiler worker was started by this reviewer. No production save was read or changed.
Only this report was written; all other inspected material was read-only. This statement describes this review's
ownership, not a fresh machine-wide process census or independent verification of earlier agents' cleanup.

**Final disposition: changes required; hold maintained. This CPU/source review is not GPU hardware certification,
authorization for an integration compile, or permission to resume games/GPU testing.**

---

## Independent recheck — H1–H4 / E1–E2 (2026-11-23)

| Recheck scope / file table | Access |
|---|---|
| `AGENTS.md`, updated `THERMAL-HARNESS.md` and `THERMAL-ENGINE.md`, original review | Read-only |
| Actual `scripts/harness_job.py`, `harness_lifecycle.py`, `harness_monitor.py`, `run_game.py`, `play_scenario.py`, `gamectl.py`, `gpu_stop.py` | Read-only implementation and cross-boundary tracing |
| `scripts/test_harness_review.py`, relevant existing harness/stop/scenario/triage regressions, `harness_idle_fixture.py` | Read-only coverage review |
| `src/graphics/src/vk/submission_safety.rs`, allocation/completion wiring in `queue.rs`, `mod.rs::Gpu::buffer`, NDVE ownership and quarantine in `exec.rs`, `newndve.rs`, `newndve/kernels.rs` | Read-only source recheck |
| External `nxrs-thermal-incident/engine/fatal_logger_tests.rs` and appended `engine/README.md` | Read-only fixture and retained results |
| `docs/waves/THERMAL-REVIEW.md` | This appendix only; original findings preserved |

Project root remains `D:\Projects\RecompLauncherEngine`. No shell/helper process, build, compiler, Cargo,
test, telemetry query, game, GPU workload or Vulkan initialization was run by this reviewer. No agent was
spawned. Latches, external evidence, library and saves were not changed; production saves were not accessed.

### Recheck disposition

**All six original failure sequences are closed at the CPU/source-review level. One concrete newly introduced
PresentMon/save-finalization regression, R1 below, prevents an unconditional integration acceptance.**

“Closed” applies to the original counterexample, not to GPU readiness, hardware safety, compilation or permission
to execute anything. The original review above remains the historical record; this appendix supplies the updated
six-ID disposition. The owner hold is unchanged.

| Original ID | Disposition | Actual closure trace |
|---|---|---|
| **H1** | **Closed for original unsafe-restoration paths**; see new R1 | `harness_lifecycle.py:9–53` requires positive job-close and worker-join proof, with sticky uncertainty. `run_game.py:287–301,568–614` performs cleanup, then persists `cleanup-uncertain` if final proof fails. `play_scenario.py:367–391` requires both actual resume and confirmed cleanup before restoration. |
| **H2** | **Closed** | `play_scenario.py:271–275` holds the machine lease around the entire `_scenario`, including save construction/begin and finish. `run_game.py:251–256` explicitly borrows that lease. `harness_job.py:180–212` checks same-thread/live borrowing and refuses implicit recursion. |
| **H3** | **Closed** | `run_game.py:423–451` samples after expensive preflight, samples again while the child is suspended, and rechecks sample age after the final latch I/O. `harness_job.py:301–339` invokes this gate before resume and retains the independent deadline check. |
| **H4** | **Closed** | Production engine fatal exit is **72**; `run_game.py:410–421` latches 71/72. Successors have same-handle verified ownership and retained exit status (`harness_job.py:278–294`; `run_game.py:542–580`). Final bounded log scanning runs even after fast exits/status-processing failure (`run_game.py:581–591`). |
| **E1** | **Closed** | `queue.rs:815–826` passes completion errors directly to `resource_safety_result`; `submission_safety.rs:8–45` invokes Windows self-termination with 72 without normal logging, formatting, locks, cleanup or unwind. |
| **E2** | **Closed** | `queue.rs:383–431` uses `allocate_descriptor_set` and the exact raw-result classifier. `submission_safety.rs:48–86` visits remaining old pools once, permits only two capacity-error codes to advance, and creates/tries at most one fresh pool. Every returned failure reaches immediate fatal 72. |

### Closure details and cross-boundary checks

**H1/H2:** `CleanupTasks` attempts later cleanup steps after errors. Native job termination/query/close failures
are retained in `Job.cleanup_errors`; run-level failures cannot be erased by a later successful job retry.
`stop_and_join` checks both capture and scenario thread liveness, waits retained processes, and attempts every
child-handle close. `finish_job` requires a true close result, worker confirmation and no sticky uncertainty.
`bounded_run` now finalizes the retained child even when job cleanup raises. These close the original
RuntimeError-versus-CleanupError and skipped-child-close counterexamples.

The scenario acquires its lease before save inspection and checks the persistent stop before constructing the
save transaction. A competing scenario therefore cannot inspect or replace another cooperating run's save.
The lease stays held through restoration; a prelaunch or suspended-resume refusal leaves `state.launched` false
and does not perform restoration. The new native error, blocked callback and lease-competition regressions
exercise those actual runner/scenario boundaries with temporary saves and real idle jobs. R1 is a distinct
successful-cleanup regression in the subsequent PresentMon finalizer, not persistence of the old unsafe restore.

**H3:** the original initial-sample-only decision is no longer used as launch permission. There are explicit
post-provenance and suspended-child samples, followed by an age check after the last potentially blocking latch
operation. The actual `ResumeThread` follows the gate and watchdog heartbeat. The current sensor adapter still
returns unverified CPU temperature as unknown; no environment override was added. This closes the reviewed
preflight/creation freshness gap without claiming atomic hardware temperature observation or scheduler guarantees.

**H4/E1:** the Windows fatal producer and harness consumer agree on **72**, with existing device-lost **71** and
harness-owned **124** distinct. `Job.kill()` records termination requests against still-live retained children;
an ambiguous subsequent 71/72 latches `fatal-status-origin-unknown` rather than relabeling a known harness stop
as an engine-origin stop. Unknown/unowned successor status is durably refused. Final unread-log data is bounded
to 256 KiB plus one byte; excessive backlog or read failure becomes a fault rather than being treated as clean.
The new watchdog and `ERROR_DEVICE_LOST` spellings match the reader. Fault persistence precedes completed run
intent, and write failures preserve the existing pending-journal/in-process refusal behavior.

The completion error path now has no intervening logger. On Windows, `TerminateProcess(GetCurrentProcess(), 72)`
uses only this process's pseudo-handle. Abort is the fallback if native self-termination returns; it is not the
normal fatal-status protocol. Unsupported targets/fallback execution remain documented limits. Ordinary guest
wait timeout is not mapped to 72. No new mismatch between the normal Windows producer and consumer was found.

**E2:** with K old pools and valid cursor i, the production helper makes at most K-i+1 set-allocation calls and
one pool creation. A terminal error on the first attempted old pool makes one allocation call and zero creates.
Pool-create failure, invalid cursor and any fresh-pool allocation failure terminate the attempt. The actual
queue callback supplies `error.as_raw()` to the classifier accepting only -12 and -1000069000. A new pool is
registered before its first set attempt; failure does not destroy resources that recorded work may reference.
The caller immediately terminates instead of retrying the helper. No visible type/borrow issue was found in
this wiring; compilation is still unverified.

**Retained corrections:** `Gpu::buffer` still releases its allocator guard before the helper can destroy/free
on allocation/bind error, with successful-only host accounting. Both native descriptor recorders still register
transient pools before use. Counter buffers still enter submission ownership before fill/compare and are freed
after callbacks and confirmed completion. Native live modes remain unconditionally quarantined at parsing and
host-graph entry. No additional concrete blocker was found in those inspected unchanged paths.

### R1 — Medium / blocking integration regression: PresentMon is waited again after its handle is closed

**Locations:** `scripts/play_scenario.py:313–319,374–391`;
`scripts/harness_job.py:314–315,354–385,432–464`;
`scripts/run_game.py:287–304`.

**Source-proven failure sequence, conditional on PresentMon being enabled, present and successfully launched:**

1. The scenario launches PresentMon through the run's `Job.launch`. Its `Child` is registered in `job.children`
   and also retained in the scenario's global `PM` list.
2. Normal runner shutdown terminates/drains the job and joins the scenario callback. `finish_job` calls
   `Job.close()`, which waits every remaining job child, caches its exit status, and calls `child.close()`.
   PresentMon's `handle` is now `None`, with its actual exit status retained.
3. The runner confirms cleanup, marks the run intent completed and returns to `_scenario`.
4. `_scenario` then iterates `PM` and calls `pm.wait(timeout=2)` again. Unlike `poll()`, `Child.wait()` has no
   cached-returncode fast path: it unconditionally calls `WaitForSingleObject(self.handle, ...)`.
   Passing the now-null handle fails. `job.uncertain(...)` raises `CleanupError` despite the already-confirmed exit.
5. The scenario collects that error, sets `state.cleanup_confirmed` false and raises at `helpers.require()`
   **before `save_tx.finish()`**. A successful save-protected run therefore leaves the game's modified save in
   place with the original backup retained instead of restoring it. This occurs after the runner's completed
   intent and outside its `cleanup-uncertain` persistence block.

This is a deterministic lifetime mismatch in the inspected source, not evidence of a live orphan or unsafe
restoration. The positive gate fails closed, but a normal supported helper path no longer completes the promised
save transaction. The machine lease remains correctly held during these operations and is then released on error.
Consequently I cannot confirm the requested absence of a new cross-boundary save-cleanup gap.

**Minimal correction:** make the retained-child wait contract honor an already-cached terminal status without
calling Windows on a closed handle (including status zero), or remove the redundant scenario wait in favor of
the already-positive job/child completion proof. A closed handle **without** a cached exit must still fail closed;
do not turn missing status into success or suppress genuine sticky cleanup errors. Keep a single authoritative
helper-finalization result through the save boundary. If real post-run cleanup uncertainty remains possible,
route it through the same refusal/recovery reporting before releasing the lease.

**Missing regression:** populate `PM` using a real job-owned CPU-idle stand-in, run it through normal runner
shutdown/handle closure, then execute the scenario's actual helper finalizer and temporary-save restoration.
Assert that cached status remains usable, cleanup stays confirmed and restoration completes under the lease.
No real PresentMon, game or GPU activity is necessary for that test, but this reviewer did not execute it.

The new `test_harness_review.py:107–118` fixture replaces `steps` with its injected callback or `None`, so it
does not enter the production PresentMon launch branch. The older save tests use an empty `PM` list. The reported
full-suite pass therefore does not exercise this failure sequence.

### Evidence assessment and remaining requirements

- The updated harness report records **117 passed, one skipped, 9.301 seconds**. Its 25 added regressions were
  read, not rerun. They genuinely use the production runner/lifecycle, real Windows jobs and idle children for
  the main cleanup, lease, thermal-gate and root/successor-fatal cases; injected telemetry avoids real sensors.
  This materially improves on the first review's primitive-only integration coverage. R1 remains uncovered.
- The retained engine README reports **16 production-helper tests passed** and **two fatal-fixture test entries
  passed**, the latter comprising a parent regression with three sequential children and its entry wrapper.
  The inspected fixture includes the exact production helper and real logger by absolute source paths.
  Blocked sink, panicking sink and held stderr cases invoke the actual fatal helper and assert exit 72 under
  bounded parent supervision. These are meaningful CPU fatal-path results, not a Vulkan integration compile.
- **Required correction:** close R1 with the narrow handle/finalization change and, when separately authorized,
  the focused CPU-idle helper/save regression. Review this boundary again before claiming complete scenario
  cleanup. Do not weaken the positive cleanup gate to make the test pass.
- Graphics integration remains uncompiled/unexecuted and requires separate explicit authorization for any
  subsequent verification. Neither this review nor the reported helper passes grants that authorization.
- Fresh CPU-temperature visibility remains absent. Current launch refusal and all latches/quarantine must remain.
  Thresholds remain policy, not hardware guarantees; the two reboots still lack a pre-crash thermal trace or a
  demonstrated thermal root cause. Userspace supervision cannot certify driver/firmware/hardware behavior.

**Recheck final disposition:** H1, H2, H3, H4, E1 and E2 closed for their original CPU/source counterexamples;
**changes still required for new R1**. Owner hold unchanged. No GPU-readiness or permission-to-resume claim.

**Owned resources:** none created by this reviewer, none left running. No helper process, job, thread, lease,
compiler, test worker or Vulkan/GPU object was started. Only this appendix was written; all prior findings and
external evidence were preserved.

---

## Final focused recheck — R1 only (2026-11-23)

| Scope | Access |
|---|---|
| `scripts/harness_job.py::Child.wait`, new `RetainedHelperTests` in `scripts/test_harness_review.py`, R1 appendix in `THERMAL-HARNESS.md` | Read-only |
| `docs/waves/THERMAL-REVIEW.md` | Append only; previous findings and recheck preserved |

**R1 CLOSED at the CPU/source-review level. No remaining concrete defect was found in the narrow correction.**
The existing **H1, H2, H3, H4, E1 and E2 closed dispositions stand**. This was not a broad re-review of unchanged
source. This appendix supersedes the preceding R1-open disposition and updates the compilation-verification state.

### Exact counterexample closure

- `harness_job.py:444–449` now returns `self.returncode` when it **is not None**, including zero, before any
  Windows wait. After confirmed `Job.close()` caches PresentMon's status and closes its handle, the scenario's
  repeated `pm.wait()` therefore returns that actual status without passing a null handle to Windows.
- If both the handle and cached status are missing, the function calls the existing `job.uncertain` path.
  It raises `CleanupError` rather than inventing success. The fast path does not clear sticky job/run errors,
  set cleanup confirmation or weaken the separate positive restoration gate.
- The normal cached-status path no longer creates the spurious helper-finalization error that previously
  prevented `SaveTransaction.finish()`. Repeated `close()` remains idempotent.

### Regression evidence inspected, not executed by this reviewer

`test_harness_review.py:458–522` adds the missing boundary coverage:

1. Real job-owned idle children exit with **0 and 37**; `Job.close()` obtains their actual status and closes their
   handles. Repeated waits return the exact cached code, with native wait/close patched to fail if called again.
2. Missing handle **and** missing status raises `CleanupError`, makes no native wait and leaves job cleanup
   unconfirmed with sticky errors.
3. A real idle helper is registered in both the run job and `PM`. The production scenario finalizer waits it
   after confirmed job closure, observes cached **124**, retains positive cleanup proof, and restores a temporary
   save while a competing lease acquisition is refused. Assertions also check restored contents, retained
   `save-after` contents, an empty `PM` list and the closed helper handle.

The appended harness report records a pre-fix reproduction followed by **100 focused tests passed** and
**120 passed / one skipped in 9.835 seconds** for the full suite. These are reported execution results; this
review independently inspected the correction and regression source and did not rerun them.

### Updated graphics verification state

The lead reports successful execution of:

`cargo check -p graphics --lib --offline --locked --jobs 1`

using the existing external target, a single worker and an independent 60-second job deadline. Reported duration:
**10.71 seconds**, with an existing `unused_mut` warning. This is lead-provided verification, not a command run or
output independently inspected by this reviewer. Accordingly the prior “graphics integration uncompiled” status
is superseded by **graphics library integration compile-checked; not GPU-tested**. It does not establish linked
game behavior, Vulkan runtime correctness, thermal safety or permission for further execution.

### Final disposition and ownership

**All tracked findings H1–H4, E1–E2 and R1 are closed for their reviewed source counterexamples.** No additional
source correction is requested by this focused recheck. Fresh CPU-temperature visibility remains unavailable;
launch refusal, latches and NDVE quarantine remain. Thresholds are policy, not hardware guarantees.

**The owner hold on all games/GPU workloads remains unchanged. No resumption is authorized.** Any further
verification requires its own authorization; this closure is not GPU hardware certification.

Reviewer execution resources: **none created, none left running**. No tests, builds, Cargo, sensors, helper
processes, GPU/Vulkan activity or subagents were used. Only this appendix was written; latches, external evidence,
library and production saves were unchanged.
