# Reboot / overheating incident — 2026-11-23

## Current hold

Owner reports two reboots during testing, pegged GPU/fans before failure and a CPU-overtemperature message after
reboot. No further games, GPU tests, replays or stress builds are authorized while this is investigated. Existing
evidence, read-only telemetry, source review and bounded CPU-only failure-injection tests are permitted.

**Latest status, reference-alignment candidate:** source patches and independent review corrections are written;
gated CPU compilation/tests are pending. HWiNFO is running, but its live CSV has a mismatched header/row schema,
and another session's four-worker Leonida build remains present. A new matching log was requested. GAR-5's
pre-existing raw-writeback delta defect remains open; native NDVE stays quarantined. The new candidate is not
compiled/deployed and no game/GPU test has run. See the final section and the alignment reports for exact scope.

## Observations (not yet a causal conclusion)

- Windows records unexpected shutdown around 01:43:48, then Kernel-Power 41 at 01:45:35 and EventLog 6008 at
  01:45:52 on November 23. The previously inspected OoT 01:28 run has an exit-0 result after 115.2 seconds; that
  alone does not rule out later testing or another workload causing the restart.
- Latest incident: clustered `nvlddmkm` events 14 and 153 at 02:41:35–02:41:40, then Kernel-Power 41 at 02:43:19
  and EventLog 6008 at 02:43:35. Event 6008 describes the previous unexpected shutdown at 02:40:54. Treat these
  timestamps as separate event observations; do not invent a precise ordering from buffered event delivery.
- At 02:47:50, system uptime began 02:43:14; process inspection found no game, Python worker or compiler from
  the test workflow running. System reported about 114 GiB available physical memory after reboot.
- No pre-restart CPU temperature trace has yet been recovered. A reported firmware overtemperature warning is
  significant, but post-reboot free memory/temperature cannot establish or exclude a prior leak or runaway load.

## Required work

1. Correlate both incident windows with actual harness, build and engine evidence, including interrupted runs.
2. Audit launch paths, independent deadlines, subprocess timeouts, process-tree cleanup, compiler overlap,
   telemetry failures and missing thermal monitoring.
3. Audit interrupted NDVE work for cumulative/transient allocations, lifetime and unbounded dispatch/compilation.
4. Fix source-proven defects, verify with CPU-only failure injection, and obtain an independent source review.
5. State what is fixed and what remains unproven before proposing any controlled resumption to the owner.

No claim that the machine is now safe to stress, no automatic resumption, and no change to firmware, fans, clocks,
power limits or Windows GPU timeout settings is part of this work.

## Corrected incident correlation

There were later OoT workloads, not just the successfully completed 01:28 shadow run. The statement that the
completed run ruled out OoT involvement was incorrect.

| Interrupted run (external `nxrs-agent2/runs/`) | Retained observations |
|---|---|
| `20261123-014306-oot-production-gameplay` | `NXGFX_NDVE=on`; native replacement activated at 9.19 engine seconds after a tolerance-based comparison. Last frame row 2113 at guest_s 56.3669; final engine metric FPS 0.9 at 59.6 s; peak valid private-memory sample 12,275 MiB. No result or save-restoration completion record. Overlaps the first unexpected shutdown. |
| `20261123-024113-oot-ndve-production-gameplay-final` | `NXGFX_NDVE=on`; native replacement activated at 9.02 s after an exact-match log. Frame progress stopped at 473; submission 4764 had not completed after 5 s (log line 256). Harness persisted display-reset detection at 02:41:39, but did not produce a result/cleanup completion record. Final valid engine metric FPS 0 at 22.8 s, private memory 7,833 MiB; peak valid sample 7,864 MiB. |

Second incident event window contains **21 NVIDIA event-14 records and 47 event-153 records**, not 47 proven
independent crashes. Event payload includes `GpuRcReset TDR occurred on GPUID:200`. Both Kernel-Power records have
BugcheckCode 0; neither provides a faulting instruction or a measured CPU temperature.

Both harness-memory CSVs contain NUL-corrupted/zero-filled lines. The first has no usable memory samples in that
file; the second retains one early sample (2.889 s, private 1,152 MiB, free commit 124,702 MiB). These missing rows
are not evidence of stable memory. Engine metrics provide the figures above but not per-process VRAM/temperature.
The second run's reset-diagnostic requests also all failed with `NXRS_LANE is not set`.

Bounded CPU-only parsing and raw hashes:
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-incident\incident-run-summary.json`.
Original runs and fault marker `nxrs-agent2/OOT-GPU-STOP.json` were not modified.

## Confirmed code defects corrected

1. Native NDVE allocated fresh descriptor pools per dispatch into the reusable guest pool list. Completion reset
   rather than destroyed these pools, permitting unbounded retained-pool growth across submission reuse. Native
   one-use pools now have separate submission-owned lifetime and are drained/destroyed after the exact completion.
2. Shadow counters were registered for cleanup only after fallible recording. Ownership now precedes the first
   command/fallible use. `Gpu::buffer` also cleans up partial create/allocate/bind failures correctly.
3. Completion retried five-second waits indefinitely. It now has a finite five-second total budget with bounded
   slices/retries. Failure cannot publish completion or free in-flight resources; immediate non-unwinding status
   **72** reports a GPU completion/resource safety stop without blocking on logging or querying the driver again.
4. Guest descriptor allocation retried every API error by making more pools. It now retries only explicit pool
   exhaustion/fragmentation, traverses existing pools finitely and creates at most one fresh pool per request.
5. Live NDVE `on`, `1` and `check` are quarantined in source; the global one-frame certification latch/automatic
   replacement branch was removed. Synthetic CPU/kernel code remains available for later controlled verification.

These are source-proven resource/fail-stop defects. They are **not proof of the exact GPU instruction that hung,
the total leaked bytes in the interrupted runs, or a causal explanation of the reported CPU overtemperature**.

## Harness corrections and verification

- Windows creation-time job assignment, descendant containment, independent monotonic deadline/heartbeat worker,
  parent-death cleanup, bounded query/control helpers and kill-before-evidence fault handling.
- Positive cleanup confirmation includes actual job drain and scenario/capture/helper joins. Save restoration is
  withheld on any uncertainty; the same machine lease spans save backup, execution and restoration.
- Fresh thermal checks after expensive preflight and while suspended before resume; missing/stale/hot data refuses
  launch. Compiler overlap checked before launch and during a run. Direct GPU fatal logs and exit statuses 71/72,
  including owned successor status and final log tails, produce durable stops.
- Native private-memory, VRAM, progress/stall and growth monitoring; high utilization alone is not labelled a leak.
- PresentMon-style repeated waits now use a confirmed cached exit status instead of a closed handle.

Lead verification, CPU-only: `py -m unittest discover -s scripts -p "test_*.py"`: **120 passed / 1 optional skip**,
9.818 s. Engine's exact std-only production helper: **16 tests passed**; fatal-path fixture: **2 tests passed**,
including three owned idle child processes exiting 72 despite blocked/panicking logger or locked stderr.
The pool-lifetime regression ran 10,000 mock cycles: **3 reusable guest pools + 0 native pools retained** after each
completion; 180,000 native pools released exactly once. Peak mock handles 21.

Lead integration check: cached `cargo check -p graphics --lib --offline --locked --jobs 1`, in the existing external
target, passed in **10.71 s** under an independent 60-second job deadline. One pre-existing unused-mut warning.
This was a CPU type-check, not a GPU test, release build or production game rebuild.

Independent source review initially returned H1-H4, E1-E2 and subsequently R1; all are now closed in
`THERMAL-REVIEW.md`. Detailed implementation/test records: `THERMAL-HARNESS.md`, `THERMAL-ENGINE.md`.

Final read-only process census at **2026-11-23 05:03:18 -05:00** returned **zero matching processes** for the
known game/build paths, compiler names, Python/harness workers, PresentMon and standalone safety/fatal fixtures.
No process was terminated or launched for this census. This is a point-in-time cleanup observation, not proof
against future unrelated workloads or a claim that the prior interrupted saves were restored.

## Still blocked / requirements before considering resumption

- No trustworthy, fresh CPU temperature source is available through the installed supported monitoring providers.
  CPU temperature remains **unknown**, not assumed cool. The harness currently refuses unattended validation.
- No pre-crash temperature or full VRAM history exists; no exact faulting NDVE instruction is established. Kernel
  and driver hangs can defeat user-space termination even with the corrected harness.
- Production game binaries/dist have **not** been rebuilt with these incident corrections. Existing hashes/runs
  remain historical; source quarantine is not a claim that all deployed binaries changed.
- The owner's no-game/no-GPU hold remains active. A reliable CPU sensor source, review of the overheating condition,
  controlled deployment of these corrections, and explicit owner resumption are still required. No firmware,
  clock, fan, power, TDR, sensor-driver installation or renewed workload was performed here.

## Owner-authorized monitor installation — 2026-11-23 08:34 -05:00

The owner subsequently requested installation of a temperature monitor. Installed **HWiNFO64 8.60** from the
official `https://www.hwinfo.com/files/hwi64_860.exe` installer after Windows Authenticode verification returned
Valid with publisher **REALiX, s.r.o.** Installation returned exit 0, registered
`C:\Program Files\HWiNFO64\`, and reported no restart required. The installer launched HWiNFO; a read-only process
query confirmed it running. Installer log: `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\hwinfo-install.log`.

This supersedes the earlier no-monitor-installation scope for this explicitly authorized action only. No sensor
window or CPU temperature reading has yet been verified, and HWiNFO has not been integrated into the harness's
freshness contract. Installation alone does not clear the missing-temperature gate or the game/GPU hold.

## Owner-provided HWiNFO idle log — 2026-11-23

The owner supplied `docs/cpu_reading.CSV` after starting sensor logging. Read-only analysis of the snapshot ending
**08:58:06.288** found **385 valid samples**, 958 columns, beginning **08:45:15.693** (12 min 50.595 s).
Intervals were 1.973–2.036 s (median 2.005 s), with no nonincreasing timestamps, malformed sample rows, NUL corruption
or intervals over 4 s. The file continued to grow during inspection; these statistics describe this bounded prefix.

- DTS CPU Package: **45–54 C**, mean **47.943 C**; Core Max **44–54 C**. The separately logged Enhanced package
  sensor was **46–52 C**, mean **47.748 C**. Duplicate header names were kept separate by column index.
- Total CPU usage: mean **1.777%**, maximum **10.2%**. CPU package power: mean **14.153 W**, maximum **21.850 W**.
- Motherboard `CPU [RPM]`: **1,440–1,690 RPM**. This label alone does not establish which fan/pump is connected.
- All **91** boolean thermal/critical-temperature/power-limit-exceeded fields checked remained `No`.
- Two GPU sensor groups were present: core-temperature peaks **50.4 C** and **60.9 C**, memory-junction peaks
  **58 C** and **72 C**, hotspot peaks **54.8 C** and **66.4 C**. Their device identities were not independently
  mapped in this analysis; duplicate names were not collapsed or assigned guessed device IDs.

These readings establish an idle/light-load baseline without observed overheating during this interval. They
do not establish temperatures at either reboot or adequate cooling under sustained load. CPU readings are now
visible and timestamped in the owner-provided log; the remaining monitoring task is a tested live harness adapter
that rejects stale/stopped logging, ambiguous sensor identity and invalid samples. No adapter, game/GPU run,
production rebuild, cooling adjustment or lifting of the owner hold occurred during this read-only analysis.

## Live monitor integration completed — 2026-11-23

Owner requested an implementation agent. Session `ses_df748c0d1ffeNBIDicHO542s0O` completed the source changes,
37 focused CPU tests and the full Python suite (**157 passed / 1 optional skip**, 12.308 s). Report:
`docs/waves/THERMAL-HWINFO.md`. Lead inspected the actual adapter, bounded snapshot worker, configured provider,
runner call sites, regression sources and retained live evidence; accepted this monitoring/CPU-test scope.

- Normal harness runs now select `docs/cpu_reading.CSV` through ignored local `build/harness-monitor.json`;
  per-run `NXRS_HWINFO_CSV` can select another absolute path without changing machine-global settings.
- One progression history spans initial preflight, the suspended-launch boundary and runtime monitoring. Sample
  timestamp plus observed append progression is required; touching or rereading old data cannot renew freshness.
- Duplicate CPU columns remain separate; maximum CPU readings and protective flags are checked conservatively.
  Complete hot/flagged rows between polls are retained. GPU monitoring remains labelled device-wide NVIDIA telemetry.
- CSV reads are bounded in size and run in supervised one-second helper jobs. Stopped logging, invalid data,
  untrusted time, replacement/truncation or expired readings refuse/stop through the existing cleanup/save gates.
- Native CPU-idle regressions verify exit 124, durable stops and temporary-save protection; no game or GPU work.

Actual configured-provider samples at **09:17:20.328, 09:17:22.346 and 09:17:26.356** advanced successfully and
reported conservative CPU maxima **50, 52 and 50 C**; all thermal checks passed for these idle observations.
Evidence: `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\hwinfo-live-integration.json`.
The owner CSV and all existing stops remained untouched. Normal launches still refuse because
`build/GPU-STOP.json` has a pending fault journal requiring review; this was not cleared or bypassed.

Lead process census at **09:21:59 -05:00** found no matching compiler/game/harness/test workers. Owner HWiNFO
PID **27844** remained running. No implementation agent remains active for this assignment.

Monitoring integration is complete, but it does not establish thermal root cause or sustained-load cooling.
Remaining deployment work is a separately authorized monitored production rebuild, followed by any explicitly
authorized short game verification after review of the persistent stop. No release or game authorization is
implied by this source/CPU acceptance.

## Authorized production rebuild completed — 2026-11-23

The owner explicitly approved the monitored, single-worker production rebuild only. The same implementation
agent completed it after correcting source-proven supervision defects; lead reviewed source, test/evidence
reports and final verification, then independently compared all four deployed SHA-256 values. Full record:
`docs/waves/THERMAL-BUILD.md`. Native source baseline remains HEAD `143515097a0fa134c6fc4be839971692a39ac4de`
plus the captured dirty/untracked inputs, including incident fixes and live NDVE quarantine.

- Engine release: **5m 57s**, exit 0. Launcher release: **8m 12s**, exit 0. Supervised execution including
  packaging and cleanup assessment: **869.094 s**, **282 observations**, maximum observation gap **4.672 s**.
- Observed CPU/GPU peaks: **68/61 C**; maximum sampled aggregate owned memory **2,243 MiB**. No thermal,
  freshness, protective-flag or memory-limit refusal during this completed compilation.
- Staging compared newly compiled target bytes with packaged bytes before promotion. Deployed `nxrs-build.exe
  contract` subsequently returned **nxrs-contract 2**, exit 0, in a watched CPU-only verification job.
- Final verifier confirmed all native/packaging inputs against the original manifest, artifact/tool hashes,
  unchanged GPU-stop files, omitted unrebuilt GNU artifacts and absent root target/staging directory.
- **20 focused CPU-only supervisor tests passed**, including compiler-exit ownership races and positive
  completed-phase cleanup before starting a new verification job. No game/GPU workload or game rebuild occurred.

The first metadata-only invocation rejected an unsupported added LLVM option. The first actual compiler attempt
then stopped on an ownership-classification refusal. Both defects were corrected before the reviewed continuation.
After successful packaging, the wrapper's premature drain check produced a further historical failure record;
independent verification established release integrity and process cleanup before a narrow controller correction.
These failures were preserved, not relabelled as successful runs. Only the Python controller changed after the
compiled-input verification; native inputs and deployed bytes did not. The report records the exact delta.

Final evidence: `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\final-verification\result.json`.

| New deployed artifact in `dist/nxrs` | SHA-256 |
|---|---|
| `nxrs.exe` | `7a300ab37dc0a07be75f2e74f4e09ed72f00418a0c0ab2654cb73223fdf76b88` |
| `nxrs-engine/nxrs-build.exe` | `0a543e47e812805e4edff039831e1be99ff8c9bcae2765d2656dfbc7d9ef7cd1` |
| `nxrs-engine/nxfront.lib` | `cc723594c7fa3ca62287203bf84e4bc1dd5a9d01c3672ce7c525e3994cdc90bb` |
| `nxrs-engine/nxrt.lib` | `da51277f21f3d00c1009e6394b932dc1ddb74747a63fc97fad5edf55fe836220` |

One validated previous release remains for rollback at
`dist/nxrs-previous-8ebdf3d94c054a9cabcd005304b4db67` (**261,871,546 bytes**, about 249.7 MiB).
Agent final census found no build/test workers and HWiNFO PID 27844 still running.

This completes the build-only authorization. Existing library game executables/code DLLs do **not** inherit the
new static libraries until rebuilt. Their rebuild and any game/GPU verification remain separate held work;
cooling under sustained game load and the original reboot/overtemperature cause are still unproven.

## Lead decision — one Wonder rebuild and validation authorized

The owner replied "ok use an agent to complete the remaining work" to the completed-build report identifying
the next stage as one selected game rebuild and short monitored validation. Scope is the real library entry
`5afdfd1c-3c79-4576-ab73-761b7b1852b8` (Super Mario Bros Wonders), using the verified production release above.
Permit one single-worker monitored title rebuild followed by one gameplay attempt, hard process lifetime no
more than **330 seconds**, with an external copied save. Other titles, NDVE activation, broad GPU tests and
unchanged retries after a safety fault remain excluded.

Lead read the entire current `build/GPU-STOP.json` and its pending journal. They retain the historical MKW reset,
DC-02 pending/recovery history, DKB R1B/R2–R6 review entries and a later bounded MKW result; the pending file is the
DC-02 review marker. That history does not establish current Wonder faulting behavior. Wonder previously had
responsive 59.91 FPS course gameplay without red diagnostics or device loss, documented in
`34-LIBRARY-VALIDATION.md`. The newly built release now includes resource/fatal-watchdog corrections; the harness
has native job supervision and live HWiNFO checks. These changes justify this bounded discriminator, not a claim
that MKW/OoT/Violet faults or thermal root cause are solved.

The implementation agent may preserve exact shared-stop/journal bytes and hashes in external evidence and
perform a documented, lease-held resumption immediately around this one authorized title. Keep normal
`require_clear` and every thermal/lifecycle check operational; no monkeypatch or disabled gate is authorized.
New fault records must be retained and merged with history, never overwritten by an old backup. Reinstate a
blocking shared gate after the attempt/cleanup, with recovery intent if interrupted. External title-specific
OoT/Violet/MKW stops remain intact. This is not general fault clearance or permission to run another title.

## Wonder preflight refused; separate active build observed — 2026-11-23

The delegated attempt ended before launcher/compiler/game creation. HWiNFO sample **10:58:02.164** contained CPU
package **91 C**, second package **90 C**, Core Max **90 C**, against the unchanged 85 C policy. Progression was
not yet established on this initial observation; the provider correctly declined to wait/retry a hot sample.
The 1.843-second preflight had no workload child and positive cleanup. Shared-stop resumption never occurred.

Agent report: `waves/THERMAL-WONDER.md`; evidence:
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-wonder\build\BUILD-STOP.json` and
`post-refusal-audit.json`. Wonder exe/DLL, library metadata, production-save absence and both shared stop hashes
were unchanged. No new gameplay, input/audio/screenshot or rebuilt-title claim is made. Prepared orchestration
passed CPU-only tests; its actual title build/play transport remains unexecuted.

Lead read-only follow-up:

- At **11:04:50**, Windows counters reported **64% machine CPU use** across 24 logical processors, including
  Python shader workers and SPIR-V tools. This is a later observation, not the refusal's 22% CPU-load sample.
- At **11:05**, command lines showed another OpenCode session (PID 29252) running
  `N:\Games\Leonida\scripts\build_game.ps1`. Python root PID 17140 was executing `make_game.py` with
  `--stages check,shaders,runtime,pipelines --jobs 12`. Twelve `rdna2spirv.py` shards and SPIR-V child tools were
  observed. Wrapper start was around 10:50 and shard start around 10:53, before Wonder preflight. These historical
  identifiers are not future termination authority; this session does not own those processes.
- Direct bounded CSV snapshots at **11:05:15.591** and **11:05:19.601** showed conservative CPU maxima **93 C**
  and **95 C**, with append progression verified on the second read. No protective boolean was asserted.
  Source status `ok` means acquisition freshness, not thermal permission; both values exceed the launch threshold.

This establishes competing CPU work and contemporaneous high temperature, not proof of the original reboots'
cause or a cooling diagnosis. No foreign process was terminated. Owner input is needed before stopping that
other session's build. The Wonder attempt remains stopped; no automatic cooldown retry is queued.

## User-reported hypervisor error and resumed reference planning

The owner reported that the computer crashed with a "hypervisor error", that they disabled a Windows feature,
and that they restarted. The exact feature, stop code, dump and timing were not independently established in this
source-review task. No causal link to the earlier GPU resets, CPU temperature, or a particular workload is claimed;
the feature change is not treated as proof of repair. Prior HWiNFO liveness/process observations do not survive
the reboot as evidence of current state.

The interrupted planning invocation had not saved its report. Fresh planning session
`ses_df69d14e5ffemcb1w3x2275zNt` resumed the same assignment and completed
`docs/waves/GPU-REFERENCE-PLAN.md`, with verified reference commits/licenses, source line anchors, ranked patch
waves, tooling inventory, test designs and future controlled discriminators. Lead read the plan and spot-checked
the primary NDVE paths in nxrs, nxnative and Turbo.

Key result: nxnative's Metal path executes the full native NDVE network with private comparison outputs;
Turbo's native Vulkan path skips convolution work and substitutes blits. These are not equivalent reference
workloads. User-reported reference stability must be matched to backend/title/build before causal A/B claims.
The first proposed correction is exact reflected resource binding plus a fully prepared private NDVE shadow
transaction: current quarantined code selects some operands by shape and records guest-output writes before
later operations can fail. Retain quarantine, arithmetic target and submission-owned lifetime protections.
Native-operation/resource records and remaining partial-construction cleanup follow; shared synchronization,
raw-address-table generations and MKW device/interface checks have separate evidence and proof obligations.

No genuinely missing external package was established. SDK shader/validation tools, translator and .NET SDK
are present by source/manifest evidence. Missing native-operation attribution and complete build/link provenance
are in-repository engineering work. Aftermath or a newer diagnostic-layer bundle is conditional, not a demonstrated
crash fix. The exact fault at submission 4764, MKW's faulting instruction, Violet's reset and the hypervisor error
remain unexplained. No tests, builds, GPU probes, installs, feature changes or fault-stop clearing occurred here.

## Reference-alignment implementation and gated execution status

The owner authorized applying the plan's patches and running tests after building. Implementation session
`ses_df6763ff3ffey6Q3EHUCcHhGwN` produced the candidate and regression sources in `GPU-ALIGNMENT-PATCHES.md`.
Independent reviewer `ses_df650e263ffe4Dd0lemSXGP0HS` returned GAR-1–6 and later GAR-R1; the original implementer
corrected the introduced defects and the reviewer rechecked them in `GPU-ALIGNMENT-REVIEW.md`.

Current source includes private prepared NDVE outputs, exact operand/view registration checks, shared buffer
validation, additional partial-construction cleanup, immutable raw-table/log versions, a captured ordinary-buffer
epoch plus raw-pair cache identity, a justified pre-upload dependency, mesh resource/grid checks, mesh-6 cache
invalidation and bounded native operation records. The blanket indirect-mesh fatal regression was removed by
restoring the held prior path; indirect total-count correctness is not claimed fixed. Native NDVE stays quarantined.

GAR-1/2/3/4/6 and GAR-R1 are closed for their reviewed source counterexamples (GAR-1 by scope containment).
**GAR-5 remains High/open and separable:** delayed raw-only GPU write learning can lose the producer's delta;
the current bitmap does not preserve a producer baseline. Two explicitly ignored CPU regression sources document
the required behavior and are expected to fail until that mechanism is repaired; they have not been executed.
D3 activation contracts, full GPU-executed fault attribution and comprehensive shipped provenance also remain open.
Reviewer accepts handoff to gated CPU compilation/tests, not production/GPU readiness.

Execution is still blocked. At 12:49 the configured log was stale; lead reopened HWiNFO (new PID 15304), and the
owner confirmed it running. Subsequent bounded reads found advancing samples but a **928-column header and
930-field rows**, with nonempty extra fields. The guard refuses this ambiguous schema rather than guessing.
The owner was asked to restart logging to `docs/cpu_reading_live.CSV`; the latest file search found only the old
`cpu_reading.CSV`. Configuration has not yet been pointed at a nonexistent file. A separate session's Leonida
`make_game.py --jobs 4` process (PID 18680 at the latest read-only census) also remains present.

**No new patch candidate was compiled, tested or deployed.** The earlier verified `dist/nxrs` release remains
the deployed build. No game/GPU execution or fault-stop change occurred. Implementation/review agents are idle;
their source work is retained while fresh well-formed monitoring and the no-competing-build gate are pending.
