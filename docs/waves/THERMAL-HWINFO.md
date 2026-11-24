# Live HWiNFO harness integration — 2026-11-23

## Built

Baseline inspected before edits: HEAD `143515097a0fa134c6fc4be839971692a39ac4de`;
extensive existing dirty/untracked work preserved. Scope is Python monitoring and CPU-only verification.
The owner hold remains active. No further agents are used.

### Intended file table (recorded before source edits)

All paths below are absolute; new files unless marked existing.

| Path | Responsibility |
|---|---|
| `D:\Projects\RecompLauncherEngine\scripts\hwinfo_snapshot.py` | Bounded read-only CSV snapshot worker, isolated by existing job/deadline primitives |
| `D:\Projects\RecompLauncherEngine\scripts\hwinfo_csv.py` | Schema, duplicate-safe CPU aggregation, sample-time and observed-progression contract |
| `D:\Projects\RecompLauncherEngine\scripts\harness_monitor.py` (existing) | Run-scoped configured provider and fail-closed thermal flags |
| `D:\Projects\RecompLauncherEngine\scripts\run_game.py` (existing) | Pass actual per-run environment to monitor configuration; retain H3 gates |
| `D:\Projects\RecompLauncherEngine\build\harness-monitor.json` | Ignored repository-local owner CSV configuration, automatically used by normal harness |
| `D:\Projects\RecompLauncherEngine\scripts\test_hwinfo_csv.py` | Synthetic parser/progression regressions |
| `D:\Projects\RecompLauncherEngine\scripts\test_hwinfo_integration.py` | Actual runner/scenario/idle-job/save-boundary regressions |
| `D:\Projects\RecompLauncherEngine\scripts\hwinfo_verify.py` | Bounded read-only integration observation; no workload launch |
| `D:\Projects\RecompLauncherEngine\scripts\harness_idle_fixture.py` (existing; scope extension below) | Optional bounded idle duration for temporary-save writer |
| `D:\Projects\RecompLauncherEngine\docs\waves\THERMAL-HWINFO.md` | This implementation/evidence report |

The supplied `D:\Projects\RecompLauncherEngine\docs\cpu_reading.CSV` is read-only user data.
Local configuration follows the existing repository-local `build/` and per-run `NXRS_*` conventions.
No existing stops, shared summaries, engine sources, library metadata or production saves are edited.

Scope extension justified during focused tests: `D:\Projects\RecompLauncherEngine\scripts\harness_idle_fixture.py`
gets an optional bounded idle duration for its temporary-save writer. Its existing 150 ms lifetime ended naturally
during the deliberately blocked 150 ms sensor probe, so that case did not prove termination of a still-live writer.
The default remains unchanged; the new case holds the owned CPU-idle writer for one second.

### Implemented contract

- `run_game.run()` constructs `Monitor(env=env)`. The default monitor constructs one `SensorProvider` and one
  CSV `Source` for the run. The same progression history spans initial preflight, post-provenance preflight,
  suspended-child sampling, final acquisition-age recheck after latch I/O, and every runtime monitoring cycle.
  Existing job/lease/lifecycle/stop implementations and temperature/resource thresholds are retained.
- The installed ignored local file `D:\Projects\RecompLauncherEngine\build\harness-monitor.json` contains:

  ```json
  {
    "schema": "nxrs.harness-monitor.v1",
    "hwinfo_csv": "D:\\Projects\\RecompLauncherEngine\\docs\\cpu_reading.CSV"
  }
  ```

  Thus normal `run_game.py` invocation and `play_scenario.py` → `run_game.run()` automatically select this log,
  without environment setup. An explicit per-run `NXRS_HWINFO_CSV` absolute path takes precedence (used for
  synthetic tests); empty/invalid configuration fails closed. No machine-global environment was changed.
  `git check-ignore` confirms the local config is ignored. The personal CSV remains untouched/untracked.
- File reads execute in an existing `bounded_run` job with an independent **one-second** deadline, parent-death
  cleanup and checked handle/worker finalization. Each snapshot reads at most a **64 KiB header + 256 KiB tail
  or incremental suffix + two 256-byte anchors**. Initial history is bounded; subsequent reads start at the last
  complete record. Excess backlog, oversized header, replacement/truncation or mismatched old bytes refuse.
  CSV reading/parsing does not share or renew the workload watchdog's heartbeat. There is no permanent CSV worker.
- Initial preflight alone can spend up to three seconds between bounded probes observing startup progression.
  Resume/runtime samples never wait for a new logging interval. A source must observe an advancing timestamp in
  **two separate reads**, with sample wall age and observed progression within the existing **12-second** budget.
  Reading/touching/copying an unchanged file never renews its monotonic acquisition time. Wall-clock jumps,
  future/old/nonincreasing timestamps, stalled logging and observation gaps fail closed or require a new baseline
  and subsequent progression. A malformed record cannot fall back to an older cool record.
- CRLF/quoted columns, UTF-8/BOM and cp1252 are supported. The Date/Time schema is explicit day.month.year plus
  fractional local time; midnight/month/year rollover is supported. Embedded-newline CSV fields and other
  unrecognized formats are refused. A trailing incomplete write is excluded while the last complete sample is
  still within budget. Complete malformed rows/footer records refuse; repeated headers invalidate progression.
- CPU aggregation retains every recognized CPU/package/core/P-core/E-core occurrence, including duplicate
  package columns and conservatively labelled motherboard CPU temperatures. No fixed column positions or
  invented hardware IDs are used. Each reading identifies the path, schema hash, file identity, complete-record
  byte boundary, original local/Unix sample time, monotonic acquisition/observation times, sensor column/name,
  hottest sensor/sample and aggregation interval. Transient hot/flagged rows between polls are retained.
- Any recognized thermal/critical-temperature/power-limit boolean asserted in the CSV vetoes a run, conservatively
  across duplicate groups (not attributed to a guessed device). All required temperature/boolean fields must be
  valid. GPU CSV temperatures are not mapped to devices; NVIDIA management telemetry remains device-wide maximum
  temperature/load/usage with minimum capacity, now explicitly labelled. Nonfinite NVIDIA rows are refused.
- Without CSV configuration, the existing WMI path remains unknown without acquisition freshness. When configured,
  a failed CSV source cannot be replaced by cached WMI values. Runtime unsafe readings use the existing kill-first
  `monitor-stop` persistence and positive-cleanup/save-restoration contract.

## Tests

All execution commands used explicit cwd `D:\Projects\RecompLauncherEngine`, process-local
`PYTHONDONTWRITEBYTECODE=1` and a **30-second tool bound**. Native fixtures additionally use independent jobs.

| Execution | Result |
|---|---|
| Initial `scripts.test_hwinfo_csv scripts.test_hwinfo_integration` | 35 tests; one fixture-lifetime assertion failed, corrected as described under Slips |
| `scripts.test_hwinfo_csv scripts.test_hwinfo_integration scripts.test_harness_thermal scripts.test_harness_review` | **81 passed**, 10.114 s |
| Final focused `scripts.test_hwinfo_csv scripts.test_hwinfo_integration` after remaining changes | **37 passed**, 2.523 s |
| Full Python scripts discovery, once after fixes | **158 total: 157 passed, 1 optional skip**, 12.308 s |

Final full command:

```powershell
$env:PYTHONDONTWRITEBYTECODE='1'
py -m unittest discover -s "D:\Projects\RecompLauncherEngine\scripts" -p "test_*.py" -q
```

The skip remains the existing optional `jsonschema` validation (package absent). Nothing was installed.

The 23 new parser/config tests cover frozen logs, mtime-only refresh, stale copies, two-observation startup,
partial writes, missing/invalid/nonfinite fields, reordered/quoted/BOM/ANSI headers, duplicate packages and
hybrid cores, all protective flags, transient hot rows, missing files/schema, truncation/replacement/same-size
rewrite, repeated headers/footer, rollover/backward/duplicate/future timestamps, wall-clock movement, observation
gaps, bounded tail/backlog/header sizes, and configuration precedence/failure.

The 14 new boundary tests use the actual runner/scenario/lifecycle with real contained Python idle children,
synthetic CSVs, temporary saves and temporary latches. Management telemetry and other preflight sources are
injected; no actual PresentMon or graphics is started. They verify:

- normal `run()` → real default `Monitor` → per-run configured provider → actual supervised snapshot-worker
  startup/progression and successful idle exit, rather than just a standalone parser;
- frozen/mtime-only logging, duplicate hot CPU, critical/throttling flags, invalid required values, truncation
  and a blocked sensor probe cause kill-first exit **124**, durable monitor stop and restoration after drain;
- delayed preflight refuses before creation; hot CSV at the suspended boundary prevents any child save write;
  expiration during final latch I/O refuses at the existing H3 resume-age check;
- stale/missing CSV preflight leaves the original temporary save unreplaced and creates no child;
- CSV fault plus injected native cleanup uncertainty retains both backup and changed temporary save, withholds
  restoration, and persists both `monitor-stop` and `cleanup-uncertain`.

Existing H1–H4/R1 tests, other native ownership/deadline/lease tests and the full scripts suite pass. These are
new regressions for the previously absent adapter; no unsafe historical workload was reenacted.

## Verified

Read the required protocol, architecture, incident, harness, final review dispositions, interface contract
and actual monitor/job/lifecycle/runner/scenario/stop/test sources. Existing H1–H4/R1 fixes are retained.

Final source read-only observation command:

```powershell
$env:PYTHONDONTWRITEBYTECODE='1'
py "D:\Projects\RecompLauncherEngine\scripts\hwinfo_verify.py" --evidence "C:\Users\RISKYB~1\AppData\Local\Temp\opencode\hwinfo-live-integration.json"
```

This invokes the actual default configured `Monitor`, queries normal read-only CPU/NVIDIA management telemetry,
and reads the existing stop gate without launching a workload. The compact external JSON contains full source,
sample/sensor metadata, other telemetry, thermal check result and persistent refusal reason; no CSV is copied.

**Final observations, 2026-11-23 local time (-05:00):**

| CSV sample time | Complete-record end | Conservative CPU maximum | Result |
|---|---:|---:|---|
| 09:17:20.328 | 3,934,147 | 50 °C | `ok`, progression verified |
| 09:17:22.346 | 3,938,244 | 52 °C | `ok`, progression verified |
| 09:17:26.356 | 3,946,446 | 50 °C | `ok`, progression verified; two appended samples considered |

The source schema hash is `2a7bd3800fd9087948ba3c700142d7e13bd50706242d288adf6d7653d9cc0b84`,
encoding cp1252. **32 CPU-related temperature occurrences and 91 protective boolean fields** were recognized;
none of those flags was asserted. Duplicate package occurrences remain separate. No hardcoded observed indices
are in production selection. Earlier live inspection also encountered an actual 3,290-byte partial final write,
correctly retaining the preceding complete sample without re-stamping it.

**Feed status: live and advancing during verification; no logging restart/owner action was needed.** All three
final full thermal checks returned no thermal refusal. This is an idle/light-load observation only.

**Actual launch refusal remains:**

`GPU STOP: unreadable/invalid latch D:\Projects\RecompLauncherEngine\build\GPU-STOP.json: pending fault journal requires review; recorded corrective/discriminator decision required`

The stop is checked before normal runner monitor construction. Neither that journal nor any other existing stop
was altered or bypassed. The default provider was verified independently in read-only mode, and the real launch
boundary was tested only with synthetic telemetry, temporary stops and CPU-idle fixtures.

`git diff --check` on the existing changed tracked runner path passed (only the existing LF/CRLF notice).
Final source list is exactly the ten paths in the file table above. `harness_job.py`, `harness_lifecycle.py`,
`play_scenario.py`, `gpu_stop.py` and existing thermal/review tests were inspected/exercised but not edited.

**Owned resource cleanup:** native integration finalizers assert child exit, closed handles, joined callback/writer
threads and reaped watchdogs. At **2026-11-23 09:17:56.349 -05:00**, a final read-only process census returned
**zero** matching unittest runners, idle fixtures, harness watchdogs, CSV snapshot/verification helpers or pipe
workers. Existing HWiNFO64 **PID 27844** remained running. No user process was terminated. Test temporary saves
and synthetic CSVs were removed by test finalizers.

## Unproven

- The reported machine thermal cause is not solved. No pre-crash thermal trace, sustained-load cooling result,
  driver/GPU-readiness result or hardware safety guarantee follows from these CPU-only tests and idle readings.
- The pending/shared stop and the owner's no-game/no-GPU/no-release-build hold remain active. Production engine
  binaries were not rebuilt/deployed. Controlled deployment and game verification are future work requiring
  explicit owner resumption; this implementation does not lift either gate.
- This source trusts HWiNFO's recorded acquisition timestamp and monitored append progression, not file mtime.
  It cannot independently prove the sensor driver itself is truthful. Log stop, inaccessible/invalid schema,
  bad record, unsupported encoding/calendar format, reset or expired progression causes refusal. Some malformed
  history may require a clean logging session before a bounded tail can establish a valid baseline again.
- User-space jobs cannot guarantee termination through a wedged kernel/driver/OS, firmware failure or power loss.
  Existing sticky uncertainty and save-withholding behavior remain the response to unconfirmed cleanup.

## Slips

One initial focused test asserted exit 124 while the 150 ms temporary-save writer naturally exited 0 during a
150 ms blocked-probe test. The fixture gained a bounded optional duration; that test now keeps the writer alive
for one second and proves actual termination with 124. The final focused and full runs passed. No hold/scope
violation, build/install, workload resumption, production save/library write, stop clearing, stage, commit or push.

## Questions

None blocking this implementation. Review of the existing stop journal and any eventual resumption/deployment
remain owner/lead decisions under the current hold, not automatic actions from a cool reading.
