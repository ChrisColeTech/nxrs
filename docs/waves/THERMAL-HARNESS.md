# Thermal incident — harness hardening (2026-11-23)

## Current verification status — lead update, 2026-11-23

The lead's final CPU-only run of `py -m unittest discover -s scripts -p "test_*.py"` completed in **9.818 s**:
**120 passed, 1 optional JSON-schema skip**. Independent review closed H1–H4 and the later R1 helper-wait regression.
Earlier counts and open-review statements below are chronological history; see `THERMAL-REVIEW.md` for closure and
`THERMAL-INCIDENT.md` for the consolidated findings, graphics compile check and outstanding incident limits.

Fresh CPU-temperature visibility remains unavailable. No game/GPU workload or production-save operation was
performed during this closeout, and the owner hold remains active.

## Hold and ownership

**The owner hold remains in force. No games, GPU tests/replays or Cargo/release builds were run.**
This work covers Python harness launch, monitoring and cleanup. HEAD at handoff: `1435150`.
The pre-existing dirty/untracked work was inspected and preserved; nothing staged or committed.
Engine/NDVE investigation and the main `THERMAL-INCIDENT.md` remain lead-owned.

## Built — source corrections only

| File | Change / responsibility |
|---|---|
| `scripts/run_game.py` | Atomic machine lease, mandatory build checks, monotonic monitoring, kill-first faults, owned successor verification, thermal/progress checks, prelaunch intent and interrupted-run recovery note |
| `scripts/harness_job.py` (new) | Windows job ownership, atomic creation-time assignment, suspended start, independent deadline/monitor-lease worker, bounded probes, aggregate private memory and native free-commit reading |
| `scripts/harness_monitor.py` (new) | Injectable sample/clock contract, existing sensor queries, unknown/stale handling, temperature/memory/VRAM/progress policy, bounded incremental log-fault detection |
| `scripts/gamectl.py` | Job membership instead of image-name/pending-claim authority; contained pipe helper; maximum two-second exchange |
| `scripts/play_scenario.py` | Cancellable waits, kill-first scenario errors, PresentMon in the owned job, no stop-existing-session action, save restoration withheld if process cleanup is unconfirmed |
| `scripts/harness_idle_fixture.py` (new) | Bounded, CPU-idle child/grandchild, pipe, lease and runner-death fixtures |
| `scripts/test_harness_thermal.py` (new) | Native containment/ownership/deadline failures and injected monitoring failures |
| `scripts/test_gpu_stop.py` | Preserve latch regressions; adapt ownership fixture; add preflight/midrun build, sensor, intent and unowned-successor failures |
| `scripts/test_play_scenario.py` | Unconfirmed process drain retains the original save backup without overwriting a possibly live save |
| `scripts/test_run_triage.py` | Existing pipe deadline/reply tests now use the bounded job-owned server rather than a synchronous in-process server |
| `docs/waves/THERMAL-HARNESS.md` | This file table, evidence, results and limits |

`scripts/gpu_stop.py` was reviewed, retained and exercised. Its shared/additive durable latch and pending-journal
semantics already address persistence failures; this change moves termination ahead of those writes.

### Confirmed defects and corrections

1. **Deadline starvation:** `ps()` and `tasklist` had no timeout; deadline, CIM/event queries and memory checks
   shared one thread and used wall time. Safety stops also waited for diagnostics/quit. A separate Python worker
   now waits only on Windows handles and monotonic deadlines. It terminates the job at the hard deadline, on runner
   exit, or after 15 seconds without a completed monitoring cycle. It performs no telemetry or evidence I/O.
   PowerShell requests have three-second budgets; tasklist/NVIDIA and pipe exchanges have two-second budgets.
   Their helpers have their own jobs, watchdogs and parent-exit handling. Native memory queries replace per-poll
   PowerShell memory/process-liveness queries.
2. **Process lifetime gaps:** direct `Popen`, root-only killing and adoption of a log-provided PID could leave
   descendants/successors behind or target unrelated PIDs. Creation now uses `PROC_THREAD_ATTRIBUTE_JOB_LIST`
   plus `CREATE_SUSPENDED`, assigning the job atomically during `CreateProcessW`, before the first instruction.
   This also closes the parent-death gap between creation and assignment. The job disallows breakaway and enables
   kill-on-close. Only output/input handles are inherited by the workload. The watchdog retains a runner process
   handle, avoiding PID-reuse ambiguity. Logged successors and control targets require actual membership in the
   named run job. Safety termination affects the owned job, never an image-name match or an arbitrary logged PID.
3. **Competing claims/builds:** sleep-and-sort claim arbitration allowed same-lane and interleaved launch races.
   A Windows global named mutex now serializes cooperating harness runs across processes/threads/sessions.
   `NXRS_CONCURRENT` and `NXRS_WAIT_FOR_BUILDS` no longer opt out. Compiler checks run during preflight, immediately
   before launch and each monitor cycle. A compiler appearing midrun stops our workload; it does not kill the
   compiler. Per-run lane selection is used consistently when excluding the current claim.
4. **Missing monitoring contract:** unknown sensor data no longer means safe. `run()` requires usable CPU/GPU
   temperature, CPU/GPU load and VRAM readings. Sample identity, source, monotonic acquisition/query time, status
   and failure detail are retained. Stale samples become explicitly `stale`; unavailable/unverifiable readings
   are `unknown`. A hot/unknown/stale sample refuses preflight or stops an active run. Direct device-loss/fatal
   log messages and excessive log-reader backlog also stop the workload without a control-pipe request.
5. **Progress/resource evidence:** compare cumulative frame progress with measured temperatures, load and memory.
   No frame progress (including repeated zero frames/FPS-zero cases) for 30 seconds stops validation. Private
   memory covers all live job members. Growth policy reports measured growth and whether progress stalled;
   high utilization alone does not produce a leak verdict. Device-wide NVIDIA VRAM is labelled as such below.
6. **Interrupted runs and cleanup ordering:** `run-intent.json` is flushed/fsynced before launching and carries an
   explicit recovery note. Completion is recorded only after cleanup; an incomplete/malformed intent prevents
   reuse of that artifact directory. It is not a global historical-run scanner. No recorded PID is killed during
   recovery. Jobs are terminated/drained before evidence-thread joins and final observations. Scenario waits
   cancel on stop, and a non-terminating scenario thread is reported rather than silently forgotten. If a process
   cannot drain, `CleanupError` withholds automatic save restoration and leaves `save-before` for reviewed recovery.

All containment is implemented with Python's standard library/Win32 bindings in `scripts`. No compiled helper,
native build, package install or source edit outside `scripts` (apart from this requested document) was needed.

### Monitoring policy / operational limits

| Check | Current stop/refusal threshold |
|---|---|
| Hard deadline | Supplied `hard` budget; finite, positive, under 24 hours; starts when the job/watchdog is created, before guarded launch preflight |
| Monitor heartbeat | 15 seconds without a completed successful observation |
| Sensor age | More than 12 seconds, missing acquisition time or future/nonfinite time is stale/unknown |
| CPU / GPU temperature | At least 85 / 80 °C respectively |
| Owned-job private memory | At least 20,000 MiB; a Windows job commit limit also caps allocations at 20,000 MiB |
| Free commit | At least 40,000 MiB before launch; stop below 25,000 MiB during a run |
| VRAM | At least 20,000 MiB or 90% of reported device capacity |
| Sustained growth | At least 1,024 MiB over at least 30 seconds, with stalled progress or at least 100 MiB/s growth |
| Progress | No advancing cumulative frame counter for 30 seconds; counter regression is unknown progress |

These are conservative harness refusal policies, **not validated hardware-safe operating limits**. CPU load is
the machine-wide `Win32_Processor.LoadPercentage`; NVIDIA temperature/load/VRAM are driver-reported, device-wide
snapshots, not per-process allocation attribution. Multiple NVIDIA devices are treated conservatively using the
maximum observed temperature/load/usage and minimum capacity. No GPU submission count or causal leak claim is
invented. Runtime artifacts include `thermal-preflight.json`, `harness-thermal.jsonl` and fault samples in the latch.

## Tests and verified commands

Every command below used cwd `D:\Projects\RecompLauncherEngine` and `PYTHONDONTWRITEBYTECODE=1`.

```powershell
$env:PYTHONDONTWRITEBYTECODE='1'
py -m unittest scripts.test_run_triage scripts.test_harness_thermal scripts.test_gpu_stop scripts.test_play_scenario -v
# 72 tests, all passed, 4.194 seconds

py -m unittest discover -s scripts -p "test_*.py" -v
# 93 tests, 92 passed, 1 skipped, 4.511 seconds
```

The skip is the pre-existing JSON-schema validation test because `jsonschema` is not installed. No installation
was attempted. `git diff --check` on the modified tracked harness files passed (only existing CRLF notices).

Meaningful failures exercised:

- blocked telemetry, connected-but-nonreplying real named pipe, and preservation of a normal pipe reply;
- deadline while the observer does no polling; wall-clock change cannot postpone that deadline;
- expired monitor heartbeat, abrupt runner death, and runner death during suspended launch;
- child/grandchild cleanup, descendants surviving root exit, atomic claim exclusion across thread/process,
  failed job-attribute setup before any child code executes, and rejection of an unowned PID;
- absent/unverifiably cached/stale/hot/NaN sensors, stalled zero frame counter, high load with healthy progress,
  private/VRAM growth and absolute caps, split log faults and bounded reader backlog;
- compiler present initially, appearing at launch, or starting midrun, including obsolete opt-out environments;
- intent present before launch, interrupted intent refusing reuse, kill-before-latch persistence, failed latch
  writes, successor ownership, save restoration after a confirmed stop and withheld restoration after uncertain stop.

New containment tests use sleeping Python processes and OS handles only, with sub-five-second fixture lifetimes
and watchdog budgets. All save/library-like writes in tests use temporary folders. Native tests assert child,
grandchild and watchdog exit; `Job.close()` waits for helper exit and job drain. A final read-only
`Win32_Process` check found **no remaining unittest runner, harness watchdog, idle fixture or pipe-worker process**.

These tests were added against source-proven gaps; the unsafe old harness was not rerun with a game to establish
pre-fix failures. Existing latch/save/observability tests remain in the suite.

### Sensor visibility observed (read-only, no workload)

One bounded `harness_monitor.sensors()` query returned:

- CPU temperature: **unknown**. Both `root/LibreHardwareMonitor` and `root/OpenHardwareMonitor` returned
  `Invalid namespace`. No supported usable CPU temperature source was established.
- CPU load: 1% (`Win32_Processor.LoadPercentage`).
- NVIDIA GPU: 49 °C, 4% load, 1,965 MiB used / 32,607 MiB capacity (`nvidia-smi` query only).

These are post-incident point observations, not a pre-crash trace or proof of safe stress operation. No ACPI
thermal-zone reading was used as CPU temperature. Even if a hardware-monitor WMI temperature is present, its
cached value without a verifiable acquisition timestamp is retained as unknown, not re-stamped as fresh.
The current adapter therefore **fails closed for unattended game/stress validation on this machine**.

### Retained evidence read (no writes)

- `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-agent2\runs\20261123-014128-oot-ndve-on\result.json`:
  `why="hard timeout 30 s"`, exit 0, reported harness elapsed 44.2 seconds. This corroborates a deadline/evidence
  timing gap; it does not prove the GPU was busy for every one of those 44.2 seconds.
- `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-library-validation\runs\02-mkw\result.json`:
  recorded reset times 13:34:39/13:34:36, exit 71, elapsed 30.8 seconds. No new incident attribution inferred.
- Agent2 `metrics-1795412999.csv`: retained zero-frame/FPS-zero rows with nonzero CPU activity.
- Library-validation `runs\07-beach\metrics-1795374417.csv`: confirms the harness-consumed `frames` column is
  cumulative in retained evidence (61, 121, 171), rather than an interval FPS count.

## Unproven / remaining required work

1. **Owner hold remains.** CPU-only tests establish harness behavior, not driver/engine correctness or readiness to
   stress. Lead-owned incident evidence correlation, engine/NDVE fixes and independent source review remain required.
2. A usable, demonstrably fresh CPU temperature source is still missing from the supported installed providers.
   Establish such an existing provider before considering validation; do not use unrelated ACPI zones, cached
   unversioned values, or an environment bypass as a substitute. No sensor software/driver was installed here.
3. Default preflight and midrun build exclusion are enforced by the harness. **Zero overlap with an arbitrary
   external/manual compiler start cannot be guaranteed by process polling.** Build entrypoints would need to
   participate in the same machine lease for cooperative atomic exclusion. Uncooperative starts can overlap until
   detected; only the owned workload is stopped. This work does not police or kill other users' compiler processes.
4. Job termination is a user-space request to Windows, not a hardware interlock. Kernel/driver hangs, a wedged OS
   scheduler, firmware faults or power loss can outlive/prevent user-space termination and evidence writes. Cleanup
   failure is explicit; save restoration must wait for independently established exit. Driver/provider accuracy
   and hardware-appropriate thermal thresholds remain outside these CPU-only proofs.
5. New creation-time job-list containment requires Windows 10+ and compatible job nesting; unsupported/denied
   assignment fails closed. Review the Win32 ABI bindings, inheritance and lifetime paths before any resumption.
   Arbitrary custom `during` callbacks must honor cancellation; an unjoined callback is reported as an error.

## Slips

The first full-suite attempt hit its 60-second tool timeout in the old CPU-only pipe test's synchronous server
cleanup: that fixture patched the superseded subprocess boundary. It was replaced by the bounded job-owned
fixture, rerun focused, then the full suite passed in 4.511 seconds. A process check after that timeout and again
after the successful suite found no leftover harness/test helpers. No games, graphics workloads, builds, sensor
installs, OS/firmware/fan/power/TDR changes or production-save writes occurred.

## Questions

None blocking this CPU-only harness correction. The remaining review/sensor/build-coordination work above does
not authorize resuming games.

## H1–H4 review send-back — corrections and verification (2026-11-23)

The independent `THERMAL-REVIEW.md` identified real gaps in the first implementation. The history above is
retained; this section supersedes its cleanup/save-boundary and fatal-status claims. **Hold unchanged.** This
iteration used only Python CPU-idle failure injection, temporary saves and a read-only process-exit check.
No games, GPU/Vulkan activity, Cargo/heavy builds, sensor installation, engine edits or production-save accesses.

### Send-back file table

| File | Correction in this iteration |
|---|---|
| `scripts/harness_lifecycle.py` (new) | Explicit per-run launch/cleanup proof, sticky uncertainty, job/process drain, both callback joins and unconditional child-handle finalization |
| `scripts/harness_job.py` | Checked/sticky cleanup failures; attempt-all cleanup; positive `close()` result; explicit same-thread lease borrowing; suspended-resume hooks; ownership-verified retained successor handles |
| `scripts/run_game.py` | Full cleanup proof, cleanup-uncertain stop persistence, late thermal sampling/revalidation, exit 72 contract, successor status collection and final log scan |
| `scripts/play_scenario.py` | Lease surrounds save inspection through restoration; proof-gated finish; no save replacement without an actual resume; stop gate before save inspection |
| `scripts/harness_monitor.py` | Recognize `GPU completion watchdog` and `ERROR_DEVICE_LOST`, including `Driver(ERROR_DEVICE_LOST)` |
| `scripts/harness_idle_fixture.py` | CPU-only temp-save writer, direct exit-code, fatal-text and successor fixtures |
| `scripts/test_harness_review.py` (new) | 25 H1–H4 regression/integration cases using real Windows jobs, temporary saves and injected sources/errors |
| `scripts/test_gpu_stop.py` | Retained regressions adapted to resume callbacks, extra fresh-sample gates and retained successor handles; no-launch refusal now asserts no restoration transaction |
| `scripts/test_play_scenario.py` | Existing outer-boundary save test isolates the new pre-save latch check to its temporary directory |
| `docs/waves/THERMAL-HARNESS.md` | Appended correction history, contracts, commands and outcomes |

### H1 — positive cleanup result, not exception-type inference

- `RunState.cleanup_confirmed` starts false. It becomes true only after checked job termination/drain, retained
  process waits, both scenario **and capture** joins/liveness checks, watchdog reaping and handle finalization.
  Save finish additionally requires `launched=true`. Successful later cleanup cannot erase an earlier uncertain
  termination/join/close result: uncertainty is sticky at both the native job and run levels.
- Cleanup termination/query/wait/close failures and unjoined/failed joins surface as `CleanupError`. Cleanup
  attempts continue through later steps and every retained child handle, even when an earlier operation fails.
  `bounded_run` has nested finalization so a failed `job.close()` cannot skip the child handle.
- Unconfirmed run cleanup writes a `cleanup-uncertain` shared stop before the scenario releases its lease. A new
  scenario checks that gate before save inspection/backup, including when using a different artifact directory.
  Persistence failure remains a `CleanupError`; the existing pending-journal/in-process poison semantics apply.
- Tests used real idle children writing a temporary slot, real blocked scenario/capture callbacks, and injected
  `TerminateJobObject`, job-drain query, process-wait and child-close errors. They assert preservation of both the
  original backup and the child's current save, no restoration record, false cleanup proof, and eventual test
  resource release. A separate plain-`OSError` termination wrapper test proves that a successful native retry
  cannot turn the run proof back into confirmed cleanup. Plain join errors and nested probe finalization are covered.

### H2 — one lease for the complete save transaction

- Public `scenario()` acquires the machine lease before `SaveTransaction` construction (which inspects/hashes the
  save). It retains that exact lease through backup, the run, process/callback/helper cleanup and `finish()`.
  `run()` borrows the caller's live same-thread lease explicitly; standalone `run()` still acquires its own.
  Implicit recursive acquisition is refused, and borrowing a released/wrong-thread lease is invalid.
- Lease refusal cannot construct/begin/finish a save transaction. Latch refusal occurs before save inspection.
  Thermal refusal may leave an evidence backup but performs **no rename/remove/restore** of the original save
  because no workload was resumed. A child created suspended and refused at its resume gate also counts as no launch.
- Tests cover a second scenario targeting an existing save while another thread owns the actual Windows lease,
  zero transaction construction on that refusal, competing acquisition attempts before and after real restoration,
  explicit borrowing, and unchanged save/directory identity on latch or missing-sensor refusal. Real idle-process
  writes are restored only after the positive cleanup proof, while the same lease is still held.

### H3 — fresh thermal decision at the actual launch boundary

- Sample and validate again after expensive preflight/provenance work. `Job.launch` also runs a fresh sample/check
  while its child remains suspended, then rechecks that sample's age after the last latch I/O before resuming.
  The independent absolute deadline remains active. A heartbeat never substitutes for the sensor decision.
- Tests advance the injected sensor clock across delayed preflight, return hot/stale samples before creation
  and at suspended resume, and expire a newly fresh sample during the last latch check. Actual contained Python
  children never execute their temp-save write when the gate refuses. No production sensor bypass was added;
  absent/unverifiable CPU temperature still fails closed.

### H4 — stable fatal contract and correct successor status

- **71 = GPU device lost; 72 = GPU completion/resource safety fatal.** Either observed engine exit durably latches
  before completed intent, including immediate root exit with no log or Windows reset event. The harness's own
  termination status remains **124**. It records whether termination was requested while a child was still live;
  an ambiguous 71/72 observed after that request is latched as `fatal-status-origin-unknown`, without replacing
  the harness stop reason with an engine-origin claim.
- Successor adoption opens only query/synchronize rights, verifies membership on that **same retained handle**,
  then waits/reads that process's exit status. It never reuses the root's status for the successor or acts on an
  unverified PID. If the logged process has already vanished or cannot be verified/adopted, a durable
  `successor-status-unknown` refusal replaces any inference of success from root exit 0.
- Every opened run log gets a bounded final unread-data scan after termination/drain attempts, before completed
  intent, including fast exits and exceptions before the first monitoring iteration. Fatal-status processing
  failure does not skip that scan. Excess backlog or an unreadable final log is fail-closed. The new watchdog text
  and `ERROR_DEVICE_LOST` spellings are recognized.
- Tests cover a real Python root exiting 72 immediately with no log/event, fatal text emitted by an already-exited
  child, real adopted successors exiting **71 and 72 after root 0** without reset events, unowned adoption refusal,
  latch persistence while intent is still incomplete, and preservation of the harness stop reason under an injected
  ambiguous 72 status. These test the harness contract; the separate engine agent owns E1/E2 production wiring.

### Latest commands and outcomes

Explicit cwd: `D:\Projects\RecompLauncherEngine`. Each test invocation had a 30-second tool bound.

```powershell
$env:PYTHONDONTWRITEBYTECODE='1'
py -m unittest scripts.test_harness_review scripts.test_harness_thermal scripts.test_gpu_stop scripts.test_play_scenario scripts.test_run_triage -q
# 97 tests passed, 8.951 seconds

py -m unittest discover -s scripts -p "test_*.py" -v
# 118 tests: 117 passed, 1 skipped, 9.301 seconds
```

The skip remains optional JSON-schema validation (`jsonschema` absent); nothing was installed. Existing tests
remain, with contract-aware fixtures rather than weakening the new gates. One intermediate run exposed a retained
child's exit code not being cached before `Job.close()` closed its handle; finalization now waits/caches status
before closing, and the existing pipe regression passes. The final focused and full runs above both passed.
Tracked-script `git diff --check` passed (only existing CRLF notices).

**Resources released:** test finalizers explicitly released/joined failure-injected blocked callbacks and asserted
all real idle processes and watchdogs exited. Normal lease release/handle closure completed. The final read-only
`Win32_Process` check returned no unittest runner, harness watchdog, idle fixture or pipe worker. Temporary test
saves were removed by test cleanup; no production data or external evidence was modified.

### Remaining limits / review disposition

H1–H4 corrections are implemented and CPU-tested, pending independent re-review. The initial hardware/sensor,
kernel/driver-hang, storage-failure and uncooperative external-compiler limitations still apply. An exited process
whose handle was never retained cannot have its exact exit code recovered reliably; the harness now refuses
durably instead of substituting another process's code. No new CPU temperature visibility was established in
this iteration. The owner hold remains strict, and no result here authorizes games/GPU testing or an engine build.

## Final send-back R1 — retained helper wait (2026-11-23)

The last `THERMAL-REVIEW.md` appendix closes the original H1–H4 source counterexamples and identifies R1:
the scenario waits its retained PM child after `Job.close()` has already cached the exit code and closed its
handle. The new CPU-only regression reproduced the null-handle error and withheld restoration before this fix.

| Corrected path | Narrow change |
|---|---|
| `scripts/harness_job.py` | Five added lines in `Child.wait`: return an existing terminal `returncode` (including zero) without calling Windows; a missing handle with no cached status explicitly raises `CleanupError` through the existing uncertainty path |
| `scripts/test_harness_review.py` | Three R1 regressions; existing wait-error injection now targets an actual uncached native wait, including polling, rather than demanding a redundant wait on a cached exit |
| `docs/waves/THERMAL-HARNESS.md` | This result/history appendix |

`Child.close()` was already idempotent and remains so. Repeated waits/closes preserve the actual cached status;
they do not clear sticky errors or establish a new cleanup proof. No lifecycle, lease, sensor, latch or scenario
protocol was changed. No engine source was edited, and existing dirty work was preserved.

Regression coverage:

- Real job-owned Python children exit with **0 and 37**. The job obtains the status and closes the handle, then
  repeated `wait()`/`close()` calls return the exact cached status without any native wait or double-close.
- No handle **and** no cached status remains uncertain: no success is fabricated, and job cleanup remains failed.
- A real CPU-idle helper is registered in both the run job and `PM`, without running PresentMon. Normal runner
  cleanup stops it with **124**, caches that status and closes its handle. The **actual scenario PM finalizer**
  waits it again successfully, keeps cleanup confirmed, clears `PM`, and restores a temporary save while the
  same machine lease remains held. The child's changed save is retained in temporary `save-after` evidence.

Commands used explicit cwd `D:\Projects\RecompLauncherEngine`, `PYTHONDONTWRITEBYTECODE=1`, and a 30-second
execution bound per invocation:

```powershell
$env:PYTHONDONTWRITEBYTECODE='1'
py -m unittest scripts.test_harness_review scripts.test_harness_thermal scripts.test_gpu_stop scripts.test_play_scenario scripts.test_run_triage -q
# 100 tests passed, 9.536 seconds

py -m unittest discover -s scripts -p "test_*.py" -q
# 121 tests: 120 passed, 1 skipped, 9.835 seconds
```

The skip remains the existing optional `jsonschema` check. The first post-fix focused attempt identified that
the old injected wait error was no longer reached once status was cached; its injection was moved to an actual
uncached native wait, preserving the failure/withheld-restoration assertion. Both final runs above passed.

**Cleanup:** native tests confirmed process/watchdog exit, closed helper handles, joined callback threads and
cleared PM references. Temporary saves were cleaned up. A final read-only process check found no unittest runner,
harness watchdog, idle fixture or pipe worker remaining. No real PresentMon capture, game, GPU/Vulkan workload,
Cargo/build command, sensor installation or production-save access occurred. R1 is ready for the lead's narrow
hunk review; the strict owner hold and missing-sensor refusal remain in force.
