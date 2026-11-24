# Monitored production rebuild — 2026-11-23

**Final outcome: production release rebuilt, promoted and independently verified at `dist\nxrs`.**
Fresh artifact hashes, deployed contract 2, unchanged native inputs/GPU stops and absent root target are verified.
See **Final completion** below. Earlier stopped attempts and their original evidence remain chronological history.

## Built

Owner authorization: one monitored single-worker production release rebuild only. Current AGENTS.md records
the exception. Games/GPU work, launcher GUI, game rebuilds, saves/library changes and stop clearing remain held.
Baseline HEAD: `143515097a0fa134c6fc4be839971692a39ac4de`; extensive existing dirty/untracked work inspected.
`D:\Projects\RecompLauncherEngine\target` is absent before this attempt. Existing `dist\nxrs` is present.

### Intended file table, recorded before code edits

| Absolute path | Scope |
|---|---|
| `D:\Projects\RecompLauncherEngine\scripts\build\build.ps1` | Validated Jobs parameter; bounded-worker build knobs; staged/hash-validated publication; omit unrebuilt GNU artifacts for this attempt |
| `D:\Projects\RecompLauncherEngine\scripts\build_supervisor.py` | Build-specific lease, independently watched job, thermal/resume/overlap/memory checks, durable evidence and cleanup |
| `D:\Projects\RecompLauncherEngine\scripts\build_provenance.py` | CPU-only prerequisite/source/artifact evidence; no rendering or game operations |
| `D:\Projects\RecompLauncherEngine\scripts\test_build_supervisor.py` | Seconds-fast CPU-idle stop/resume/cleanup regressions |
| `D:\Projects\RecompLauncherEngine\docs\waves\THERMAL-BUILD.md` | This file table and final execution report |

Existing build.ps1 dirty changes (lean target cleanup, launcher target-dir and removed license copying) were
inspected and will be preserved. Staged publication retains the previous release for rollback until the
supervisor confirms success; an interrupted promotion is recovered only after positive owned-process drain.
No shared summaries, engine implementation, HWiNFO CSV or existing fault latches are edited.

The five paths in the table are the complete change set for this task. The supervisor now distinguishes a
vanished/exited process from a live non-member using one retained query/synchronize handle. Query/access failures
still refuse. This narrow correction followed the actual stopped build described below; it was CPU-tested,
but **no production workload was restarted after that ownership stop**.

## Tests

`PYTHONDONTWRITEBYTECODE=1; py -m unittest scripts.test_build_supervisor -q`, explicit cwd
`D:\Projects\RecompLauncherEngine`, 30-second bound: **13 passed, 1.932 s**. Real CPU-idle jobs cover hot/stale/
protective/nonfinite stops and durable evidence, suspended refusal, post-evidence age recheck, external versus
owned compiler distinction, independent deadline during a blocked sensor, sticky cleanup uncertainty, memory
cap, no game-frame requirement, and interrupted-publication rollback. No complete unchanged suite was repeated.
PowerShell AST parsing of build.ps1 passed; Jobs/default and new packaging parameters are present.

After the actual ownership refusal, expanded focused tests passed: **15 tests, 2.215 s**. New native coverage
checks a live owned child versus an unowned live process on retained handles and an exited child appearing in a
delayed process snapshot. Injected Win32 errors distinguish vanished PID (87) from access denied (5); the latter
still fails closed. The full unchanged Python suite was not repeated. Final `git diff --check` passed for the
tracked build-script edit, with only the pre-existing LF/CRLF notice.

## Verified

Read protocol, architecture, current authorization/incident and HWiNFO report. Inspected workspace manifests,
launcher config and repository build.rs files. Graphics build.rs invokes glslang source compilation sequentially;
it does not initialize Vulkan. OS build.rs copies existing codec archives on Windows; profiles and launcher
build scripts have no parallel subprocess fanout. Root/user config.toml was absent. Root target is absent.

Prepared command (fixed internal build command; no -Run or -KeepCache):

```powershell
# cwd D:\Projects\RecompLauncherEngine; tool bound 1830 s, independent job hard limit 1800 s
$env:PYTHONDONTWRITEBYTECODE='1'
py "D:\Projects\RecompLauncherEngine\scripts\build_supervisor.py" --evidence "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build"
# supervisor invokes: pwsh -NoProfile -NonInteractive -File D:\Projects\RecompLauncherEngine\scripts\build\build.ps1 -Jobs 1 -SkipGnu -RetainPrevious -PublicationJournal <evidence>\publication.json
```

The evidence directory is absent and its existing parent was verified. It is exclusively created under the
machine lease; an existing attempt refuses. status.json retains runner PID, exact named job, watchdog PID,
current phase child PID, deadline, observations and peaks. BUILD-STOP.json is separate from all game/GPU stops.
Thermal limits remain CPU 85 C / GPU 80 C, sensor age 12 s, any protective flag; heartbeat 15 s; job memory
20,000 MiB; free commit 40,000 MiB before phases and 25,000 MiB while running. No game progress check is applied.

Single-worker enforcement is process-local: Cargo --jobs 1 plus CARGO_BUILD_JOBS=1; NUM_JOBS=1 and Cargo jobserver
for cc-rs; CMAKE_BUILD_PARALLEL_LEVEL=1; MAKEFLAGS=-j1; RAYON_NUM_THREADS=1; OMP_NUM_THREADS=1; release and build-
override codegen-units=1; MSVC /CGTHREADS:1. This controls known compilation fanout; it does not
claim that every native tool or OS helper has only one thread. Host is rustc/cargo 1.99.0, x86_64-pc-windows-msvc.
Normal build.ps1 Jobs default stays 4. GNU optional artifacts are explicitly omitted in this authorized attempt.

### Initial metadata-only failure and corrected invocation

At 09:49:11 local time the initial supervised invocation failed **before compilation**: rustc's Cargo target
metadata query rejected the added LLVM `-threads=1` option. This was my supervisor configuration error, not a
source compile or thermal failure. Root target remained absent. No publication was attempted. Result: 19.218 s,
8 observations, maximum CPU 54 C / GPU 51 C, maximum sample gap 3.171 s, positive cleanup. Sampled owned memory
was 0 MiB (the short-lived failed commands exited between samples; not a claim of zero transient memory).
The original `nxrs-thermal-build\BUILD-STOP.json` and logs are retained unchanged.

Removed the unsupported LLVM option; retained Cargo jobserver/codegen-unit/native controls above. Added a
metadata-only configured-rustc option check to prerequisites. Explicit `rustc --print cfg -Clink-arg=/CGTHREADS:1`
passed; process census showed no owned worker/compiler remaining. This is a corrected configuration continuation,
not an automatic unchanged retry. Corrected evidence uses the previously absent subdirectory
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\corrected`; the first stop is not cleared.

### Actual compiler execution and stop

Executed with explicit cwd `D:\Projects\RecompLauncherEngine` and 1830-second tool bound:

```powershell
$env:PYTHONDONTWRITEBYTECODE='1'
py "D:\Projects\RecompLauncherEngine\scripts\build_supervisor.py" --evidence "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\corrected"
```

The prerequisite phase passed: tools/codec archives present, metadata-only Rust flag query passed, about
52.8 GiB free on C: and 874 GiB on D:, source hashes/HEAD/dirty status recorded, target absent, existing production
artifact and game-stop hashes retained. Source manifests cover repository `src/`, Cargo manifests/locks and build
scripts, including the previously corrected engine sources; this is input provenance, not a deployment claim.

The supervised build command was:

```powershell
pwsh -NoProfile -NonInteractive -File "D:\Projects\RecompLauncherEngine\scripts\build\build.ps1" -Jobs 1 -SkipGnu -RetainPrevious -PublicationJournal "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\corrected\publication.json"
```

It actually invoked `cargo build --release --locked --jobs 1 --target-dir D:\Projects\RecompLauncherEngine\target
-p nxrs-cli -p front -p runtime`. The log shows compilation of proc-macro2, quote, syn, cc, jobserver and other
dependencies, reaching **zstd-sys 2.1.3+zstd.1.5.7** and windows-sys. No new engine/launcher deliverable completed.

At **09:51:34.620 -05:00**, the supervisor reported:

`external workload overlap: cl.exe PID 31556, C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\bin\HostX64\x64\cl.exe`

It killed its owned job first, wrote a separate durable `BUILD-STOP.json`, drained all owned children and closed
their handles/watchdog. The reported compiler was absent in the subsequent read-only census. **The retained
evidence cannot prove that this particular PID was external.** Inspection found a concrete sampler defect:
`Job.owns(pid)` returns false for both a vanished process and a live non-member, so a delayed CIM snapshot can
misclassify a short-lived native compiler. The log's zstd-sys native build makes this plausible, but no retained
Win32 error/process handle establishes the exact historical case. The corrected classifier now checks exit and
membership on the same retained handle and does not call a proven-exited/vanished process an external workload.

| Actual monitored execution | Result |
|---|---|
| Full supervisor elapsed time (including preflight and cleanup) | **33.437 s** |
| Observation count | **12** (11 ordinary JSONL records plus the fault sample in BUILD-STOP.json) |
| Maximum observation gap | **3.140 s** |
| Peak observed conservative CPU / NVIDIA GPU temperature | **56 / 53 C** |
| Protective flags / freshness | No asserted flag or thermal/stale refusal; live CSV progression retained |
| Maximum sampled aggregate owned private memory | **678 MiB** (sampling peak, not an instantaneous high-water guarantee) |
| Termination | Owned build root **exit 124**, positive job/child/watchdog cleanup confirmed |
| Publication / contract check | **Not reached**; no publication journal or new contract execution |
| Target cleanup | Success-only cleanup **not reached**; partial dependency cache remains in root target |

Execution ownership was runner **28716**, watchdog **25144**, build root **1544**,
job `Local\nxrs-harness-1e26c1b8d83d4a1b863b7d9b8487f13f`. These are historical identifiers only; all owned
resources are released. No PID from this report is authority to terminate a future process.

### Post-stop release and latch verification

Executed (30-second bound, same cwd/environment):

```powershell
py "D:\Projects\RecompLauncherEngine\scripts\build_provenance.py" audit-stopped "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\corrected"
```

Passed. `audit-stopped.json` confirms the following **existing, unchanged** release hashes, not new build hashes:

| Existing production artifact under `D:\Projects\RecompLauncherEngine\dist\nxrs` | SHA-256 |
|---|---|
| `nxrs.exe` | `f3022d396395c2033fcf51df26e09c519b878bdc1cdbdbc0e8049fe9defdf0fb` |
| `nxrs-engine\nxrs-build.exe` | `ab412cf84d25a32f3733112702a86842dc924951d0321e4adc873801a6e19f5c` |
| `nxrs-engine\nxfront.lib` | `feb082cd7e60e492e7baf57cbf70467ae74a3aba360eee52a1c3008c14ab1f60` |
| `nxrs-engine\nxrt.lib` | `716a9752d0fa9ef1ab05724f645a180437af1231c1e1a6b0da186bcccf476e75` |

None of the four corresponding target/release artifacts exists. Both `build\GPU-STOP.json` and its `.pending`
journal match their preflight hashes. No staged/previous release directory was created; GNU optional packaging
was not reached and no stale GNU artifacts were shipped. No library, game executable/code DLL or save was accessed
for modification. Existing external game binaries would not inherit corrected static libraries even after a
future successful engine deployment; they require separately authorized game rebuilds.

Compact evidence is under `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build` (initial metadata
failure) and its `corrected` subdirectory (actual compiler execution): `intent.json`, `preflight.json`,
`preflight.log`, `production-build.log`, `thermal.jsonl`, `status.json`, `BUILD-STOP.json`, `result.json`, and
`corrected\audit-stopped.json`. No owner CSV capture or bulky repository log was created.

**Final cleanup census: 2026-11-23 09:55:22.461 -05:00.** No matching compiler, supervisor, provenance helper,
watchdog, sensor snapshot worker, idle fixture or unittest worker remained. HWiNFO64 **PID 27844** remained
running. Native test finalizers also verified owned child exit/closed handles and reaped watchdogs.

## Unproven

- Production build/deployment is **blocked/stopped, not completed**. The actual stop was an ownership refusal,
  not a thermal failure. The classifier defect is fixed and CPU-tested, but the precise historical PID membership
  remains unproven. The stopped workload was not automatically restarted.
- New release contract 2, final artifact/source matching, staged promotion and successful lean cleanup remain
  unexecuted. The code implements those gates; no successful production result is claimed from source inspection.
- Root target contains only this attempt's partial dependency build (it was absent initially). The clean-source
  prerequisite deliberately refuses a subsequent attempt until that partial cache is explicitly handled; no
  old/unrelated target directory was removed. Existing dist remains untouched.
- Root cause of the two reboots and sustained-load cooling are unproven. No game, Vulkan runtime, GPU test/replay,
  shader pipeline probe, launcher GUI, production game build or firmware/fan/clock/power/TDR change occurred.
- Process polling cannot prove zero overlap with an arbitrary short-lived external compiler between snapshots.
  Live unowned processes and ownership-query errors still stop/refuse; only owned processes or proven exits skip.

## Slips

The initial unsupported LLVM option was caught by rustc before compilation. It is corrected and moved behind
an explicit metadata-only prerequisite check. No thermal stop occurred and no old release was replaced.
The actual build then exposed the delayed-process-snapshot classification gap. I corrected it with retained-
handle checks and two additional regressions, preserved both stop records, and did not restart after that stop.

## Questions

The concrete remaining execution decision is whether to authorize a new monitored attempt after reviewing the
ownership stop and its corrected sampler. Recommendation: retain both stop records, review the narrow classifier
fix, explicitly handle only this attempt's partial root target, then authorize a fresh build-only attempt with
the same limits. Games/GPU/GUI and game rebuilds remain held independently. No further workload was started to
answer this permission question.

## Reviewed corrective continuation — 2026-11-23 10:00 -05:00

The lead reviewed the actual supervisor/provenance/build sources, 15 tests and this report, and explicitly
directed completion of the already-authorized build-only task without asking the owner again for unchanged
scope. This supersedes the prior permission question. Both earlier stop records and all GPU stops remain intact.
No actual thermal/flag/resource/driver failure or confirmed live external workload may be retried automatically.

Before retry, closed the remaining lifetime race in the existing table-owned
`D:\Projects\RecompLauncherEngine\scripts\build_supervisor.py`: if IsProcessInJob reports non-membership or
failure after the first live wait, recheck liveness on **the same retained handle**. Only positive exit overrides
that result. Live non-members still refuse; live query/access failures still refuse with the saved query error.
No PID reopening or ownership inference was added.

Added three deterministic regressions in the existing table-owned
`D:\Projects\RecompLauncherEngine\scripts\test_build_supervisor.py`: a real idle child is terminated exactly
during the membership query, covering both false membership and failed query; a still-live query failure remains
an error. Focused bounded CPU-only command `py -m unittest scripts.test_build_supervisor -q` passed **18 tests in
2.342 s**, cwd `D:\Projects\RecompLauncherEngine`, `PYTHONDONTWRITEBYTECODE=1`, 30-second tool bound.

Lead explicitly approved removal of only the proven-owned partial root target. At **10:00:41.120 -05:00**,
held the same global machine mutex; checked zero current compiler/game matches; validated exact absolute target
resolution and no reparse points in its ancestors or descendants; removed only
`D:\Projects\RecompLauncherEngine\target`: **145,085,024 bytes, 460 entries**. Confirmed target absent afterward.
Existing dist, source work and all evidence/stops were retained.

All source changes are complete before the next provenance collection. Next fixed supervised invocation:

```powershell
# cwd D:\Projects\RecompLauncherEngine, tool bound 1830 s, independent job 1800 s
$env:PYTHONDONTWRITEBYTECODE='1'
py "D:\Projects\RecompLauncherEngine\scripts\build_supervisor.py" --evidence "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\reviewed"
```

Same single-worker configuration and all thermal/freshness/memory/overlap limits apply. This is the reviewed
corrected candidate, not an unchanged retry. The parent evidence directory already exists; `reviewed` will be
exclusively created. Source files will not be edited during execution. Prior release retention through actual
verification remains enabled; no game/GPU/GUI/game-rebuild/save operation is authorized.

### Post-build completion ordering correction (scope unchanged)

The reviewed invocation completed engine release compilation in **5m 57s**, launcher release compilation in
**8m 12s**, contract 2, target-versus-stage SHA-256 validation, promotion and lean target deletion. It then exposed
another supervisor-only defect: `execute()` demanded job drain before requesting termination of remaining owned
children after the successful build root exited. This can reject normal compiler helper/server lifetimes. The
result retained a sticky cleanup failure, rather than silently relabelling it success. Later job termination
completed, and a read-only census found no remaining compiler/helper. The exact lingering process name was not
captured, so MSVC helper/server lifetime is a plausible explanation, not an identified historical process.

Before changing any source again, ran independent verification under the same machine lease and a new **60-second
watched job**, with normal live thermal gates. Confirmed the old named job no longer existed and historical
runner/watchdog/root PIDs were absent/exited. The actual `build_provenance.py verify` **passed** against the original
preflight manifest: all captured sources unchanged, deployed hashes equal freshly built/staged hashes, contract
2 recorded, all game/GPU stops unchanged, no GNU artifacts, root target absent. New verification cleanup was
positively confirmed. Evidence: `nxrs-thermal-build\verification\result.json` and `reviewed\verify.json`.
The original `reviewed\result.json`/`BUILD-STOP.json` are retained unchanged as history.

Narrow follow-up in the existing supervisor/test scope: after successful production-root exit, explicitly stop,
drain and close its job before verification. Only positive cleanup permits a fresh watched verification job,
retaining the same machine lease, sensor progression history and remaining absolute budget. Add a real idle
root/lingering-descendant regression. This controller-only post-build correction does not alter native build
inputs or release bytes; its exact input-manifest delta will be recorded separately rather than pretending it
was present during compilation. No compilation or other source edit is needed for this correction.

## Final completion — 2026-11-23 10:22 -05:00

### Built

Completed the existing owner-authorized build-only task. New release is deployed at
`D:\Projects\RecompLauncherEngine\dist\nxrs`. HEAD remains
`143515097a0fa134c6fc4be839971692a39ac4de`; the build used the captured dirty/untracked source tree, not HEAD alone.
No engine or launcher implementation correction was needed to compile. Existing warnings were an unused `mut`
in graphics and a deprecated atomic method in os. Neither was changed as unrelated work.

Exact source/report change set remains the original five-path table:

- `D:\Projects\RecompLauncherEngine\scripts\build\build.ps1`
- `D:\Projects\RecompLauncherEngine\scripts\build_supervisor.py`
- `D:\Projects\RecompLauncherEngine\scripts\build_provenance.py`
- `D:\Projects\RecompLauncherEngine\scripts\test_build_supervisor.py`
- `D:\Projects\RecompLauncherEngine\docs\waves\THERMAL-BUILD.md`

The final completion-order correction explicitly terminates/drains/closes the successful production phase's job
before starting verification in a new watched job. It retains the machine lease, same sensor provider/history,
summary metrics and remaining original deadline (with a conservative creation margin). Unconfirmed closure
blocks the next phase. Repeated close honors positive proof; sticky uncertainty is not erased.

### Tests

Final focused command, cwd `D:\Projects\RecompLauncherEngine`, `PYTHONDONTWRITEBYTECODE=1`, 30-second bound:

`py -m unittest scripts.test_build_supervisor -q`

**20 passed in 2.748 s.** This includes the three requested exit-during-membership/live-error cases and two
completion-order tests: a real root exits successfully leaving an idle descendant, which is killed/drained before
verification gets a separate watched job; an uncertain completed phase cannot start verification. Tests also
assert the original absolute deadline is not extended and the monitor instance/progression history is retained.
No broad unchanged suite, Rust tests, GPU tests or release smoke/game build was run.

### Verified

The actual fixed production invocation is recorded above (`reviewed` evidence). build.ps1 completed both Cargo
commands, staged artifacts, executed `nxrs-build.exe contract`, compared target/stage SHA-256, promoted the
validated stage and removed root target under continuous thermal/job supervision.

| Actual production execution | Result |
|---|---|
| Engine release Cargo command | **5m 57s**, exit 0 |
| Launcher release Cargo command | **8m 12s**, exit 0 |
| Original supervisor elapsed including preflight, packaging and failed early-drain assessment | **869.094 s** (14m 29.094s) |
| Observations / largest gap | **282 / 4.672 s** |
| Peak observed conservative CPU / NVIDIA GPU | **68 / 61 C** |
| Maximum sampled aggregate owned private memory | **2,243 MiB** |
| Temperature/freshness/protective/VRAM/memory/driver failure | **None observed** |
| Production build root | PID **35532**, exit **0** |
| Owned job during compilation | `Local\nxrs-harness-3bdfee33156b421da78eb0d8ba45d238` |
| Runner / independent watchdog | **24768 / 29412**, subsequently exited |

The first wrapper result after the completed build retained its premature-drain failure. That historical failure
was not overwritten. An independent lease-held **60-second watched verification job** then confirmed the old
job absent, historical processes gone, and executed:

`py D:\Projects\RecompLauncherEngine\scripts\build_provenance.py verify C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\reviewed`

It passed the **entire original input manifest** before any further controller edits, matched deployed artifacts
to the recorded fresh target/stage hashes, verified contract metadata, unchanged GPU stops, omitted GNU artifacts
and absent target. That verification job obtained positive cleanup proof. Evidence:
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\verification\result.json` and
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\reviewed\verify.json`.

After the controller-only completion correction/tests, another lease-held watched job executed the **deployed**
`D:\Projects\RecompLauncherEngine\dist\nxrs\nxrs-engine\nxrs-build.exe contract`: exactly **`nxrs-contract 2`**,
exit 0, positive cleanup. Its independent deadline was 60 seconds; elapsed 10.625 s, 4 observations, CPU/GPU peaks
56/57 C, largest gap 3.063 s. Final source audit proved the only input-manifest delta since compilation was
`scripts/build_supervisor.py` (the described non-native controller correction); **every native source, Cargo
input, packaged tool input and build.ps1 remained unchanged**. The test file is not a native build input.
The exact old/new controller hashes are retained rather than falsely claiming a byte-identical whole tree:

- Before: `757543ffeed514e491a34122aad54060fe3e642660af065b804f6cdad5e2db4f`
- After: `f27013aebabba9a08e9027759b3b8dfdd62b5a12f74b2f360f2345b47e111aca`

Final acceptance evidence:
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-build\final-verification\result.json`.
The subsequent bounded read-only packaged-tool audit also passed: both shipped `dmsh_xlate.exe` and
`spirv-val.exe` match the exact prerequisite SHA-256 values captured before compilation. Its seven-file release
inventory and hashes are in `final-verification\packaged-tools.json`; these tools were copied, not rebuilt or
represented as newly corrected engine binaries.

**Actual new deployed SHA-256 hashes** (all equal the fresh target/staging receipt):

| Artifact under `D:\Projects\RecompLauncherEngine\dist\nxrs` | SHA-256 |
|---|---|
| `nxrs.exe` | `7a300ab37dc0a07be75f2e74f4e09ed72f00418a0c0ab2654cb73223fdf76b88` |
| `nxrs-engine\nxrs-build.exe` | `0a543e47e812805e4edff039831e1be99ff8c9bcae2765d2656dfbc7d9ef7cd1` |
| `nxrs-engine\nxfront.lib` | `cc723594c7fa3ca62287203bf84e4bc1dd5a9d01c3672ce7c525e3994cdc90bb` |
| `nxrs-engine\nxrt.lib` | `da51277f21f3d00c1009e6394b932dc1ddb74747a63fc97fad5edf55fe836220` |

Fresh-source proof: root target was absent at collection/start; compile log records the actual runtime, graphics,
os, front, CLI and launcher crates; preflight captures source SHA-256 including the incident fixes; all those
native inputs remained unchanged through final verification; packaged bytes match the freshly produced artifacts.
The previous-release hashes are different and are recorded above. No old GNU libraries were shipped.

**Lean cleanup and retained recovery copy:** root target is absent and the staging path is absent. Retained one
explicit rollback directory, validated against all four pre-build artifact hashes:
`D:\Projects\RecompLauncherEngine\dist\nxrs-previous-8ebdf3d94c054a9cabcd005304b4db67`,
**261,871,546 bytes** (about 249.7 MiB). This is a rename-retained previous release, not a game copy, and is not
the active `dist\nxrs`. It is retained deliberately after the wrapper's historical cleanup-order failure; no
other staged/previous folder was created by this successful invocation.

**Final resource census at 10:22:52.680 -05:00:** zero matching compiler, MSVC server, supervisor, watchdog,
provenance/snapshot helper, idle fixture, unittest worker, nxrs or nxrs-build process. HWiNFO64 PID **27844** remains
running. The verification jobs each positively confirmed cleanup; all test jobs/descendants were reaped. Both
original stop records, the additional historical completion-order stop, and all existing GPU stops are retained.
GPU-stop and pending-journal hashes still match the preflight snapshot. No source was staged/committed/pushed.

### Unproven

No claim of solved thermal root cause, sustained-load cooling certification or GPU runtime readiness. No game,
Vulkan runtime initialization, replay, GPU pipeline probe, launcher GUI or production game rebuild occurred.
External game executables/code DLLs remain unchanged and **do not inherit the new static libraries** until a
separately authorized game rebuild. The game/GPU hold and persistent journals remain active.

### Slips

Retained earlier unsupported-flag and ownership-sampler history. This continuation closed the lead's exact race,
then encountered the supervisor-only premature-drain assessment after successful packaging. It was independently
resolved and narrowly corrected/tested without rebuilding or altering native sources/binaries. The post-build
controller edit is explicitly separated in provenance; no historical failure record was relabelled or removed.
The earlier permission question was unnecessary for the unchanged build-only scope; the lead's continuation
decision superseded it and the task was completed.

### Questions

None blocking this build-only task. The explicit retained rollback copy is the only optional housekeeping item;
future game/GPU authorization and review of persistent game stops remain separate owner/lead decisions.
