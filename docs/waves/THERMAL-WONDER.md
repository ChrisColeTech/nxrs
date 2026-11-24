# One Wonder rebuild and guarded validation — 2026-11-23

**Final outcome: stopped at initial thermal preflight. HWiNFO logged CPU Package 91 C before any build/game
child launched. No title rebuild, resumption or gameplay occurred; no retry was performed.**

## Built

Authorized scope: real-library UUID `5afdfd1c-3c79-4576-ab73-761b7b1852b8`, one supervised single-worker
rebuild using verified dist, followed by one guarded gameplay attempt with a maximum 330-second process lifetime.
Read current AGENTS/lead incident decision, delegation/style rules, Wonder history and actual build/run sources.
HEAD is `143515097a0fa134c6fc4be839971692a39ac4de`; extensive dirty/untracked work preserved.

### Intended file table (before edits)

| Absolute path | Scope |
|---|---|
| `D:\Projects\RecompLauncherEngine\scripts\wonder_validation.py` | Fixed-title build/run orchestration, immutable production-save manifests and external shadow save, provenance |
| `D:\Projects\RecompLauncherEngine\scripts\wonder_tools.py` | Normal launcher engine forwarding with jobs=1; shared serialization of real CPU compiler/shader tools |
| `D:\Projects\RecompLauncherEngine\scripts\wonder_engine.cmd` | Supported NXRS_BUILD entrypoint shim to verified production engine |
| `D:\Projects\RecompLauncherEngine\scripts\wonder_clang.cmd` | NXRS_CLANG serialization shim |
| `D:\Projects\RecompLauncherEngine\scripts\wonder_xlate.cmd` | NXSHADERS_XLATE serialization shim |
| `D:\Projects\RecompLauncherEngine\scripts\wonder_spirv.cmd` | NXSHADERS_SPIRV_VAL serialization shim |
| `D:\Projects\RecompLauncherEngine\scripts\wonder_resume.py` | Exact reviewed-history archive, one lease-held Wonder resumption, additive restore |
| `D:\Projects\RecompLauncherEngine\scripts\gpu_stop.py` (existing) | Additional interruption/recovery-intent check; original stop/pending checks retained |
| `D:\Projects\RecompLauncherEngine\scripts\test_wonder_validation.py` | CPU-only forwarding, serialization, resumption/fault/interruption regressions |
| `D:\Projects\RecompLauncherEngine\docs\waves\THERMAL-WONDER.md` | This report/evidence record |

The current real library selects `P:\Games\Nintendo\DUMPS\Super Mario Bros Wonders Switch 2 Edition [010015100B514000]\SwitchRecomp Builds\Super Mario Bros Wonders Switch 2 Edition [nxrs]`,
output name `Super Mario Bros Wonders Switch 2 Edition`; global jobs=8 and entry workers=0. CLI has no worker
override. Its supported NXRS_BUILD/NXRS_CLANG/NXSHADERS_* process-local tool overrides permit a narrow shim while
retaining the production launcher build flow and its normal atomic metadata updates. The shim will replace only
the forwarded jobs count, validate the exact title, and serialize compiler/shader/validator calls in one mutex.
This matters because shader prebuild overlaps C compilation and validator fanout is jobs*2 in current production.
No engine/launcher rebuild, manual library edit, game copy or production-save transaction is planned.

Shared reviewed SHA-256 values are still the previously verified values:
`GPU-STOP.json` = `22072e11589fb20bfaac1b44bf9c4514b9f3dec8cb811b5aa096cb8f2c79fbc4`;
pending = `3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7`.
Any different bytes refuse. The full historical fault lists/pending document were read, not declared resolved.

## Tests

All commands used cwd `D:\Projects\RecompLauncherEngine`, `PYTHONDONTWRITEBYTECODE=1`, 30-second bounds.
- `py -m unittest scripts.test_wonder_validation scripts.test_gpu_stop -q`: **51 passed**, 0.845 s.
- `py -m unittest scripts.test_wonder_validation scripts.test_build_supervisor -q`: **31 passed**, 3.196 s.
The 11 new tests exercise exact history preservation, normal gate retention, other-thread refusal, additive new
faults, untouched new partial journals, changed-history/failed-archive refusal, failed-restore recovery gate,
actual owner-process death leaving a blocking intent, poisoned/tampered gate refusal, worker forwarding, native
tool serialization and external shadow-save isolation. Existing stop/build tests remain intact.
Installed LLVM 21.1.8 metadata confirms `/threads:1` disables linker multithreading. No compiler work was started
by these metadata queries. The actual engine adapter contract will be checked inside the supervised build job.

### Execution candidate

Fixed build command: `py D:\Projects\RecompLauncherEngine\scripts\wonder_validation.py build`, external evidence
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-wonder\build`, 1800-second independent job.
The normal production launcher command is `dist\nxrs\nxrs.exe --library <real library.json> --select-title
"Super Mario Bros Wonders" --build`. Process-local supported tool overrides forward to the verified production
engine with `--jobs 1`; real clang/xlate/spirv-val calls share a single native mutex across both build branches.
No manual library edit or temporary library is used. The shared stop/journal remain untouched throughout build.

Fixed later run command: `py D:\Projects\RecompLauncherEngine\scripts\wonder_validation.py play`. The normal
Wonder scenario and replay are reused, with an additional early jump image and longer final settle to cover
115–260 s, five scheduled frames total, no periodic captures/PresentMon. Hard lifetime is 330 seconds. Its
production SaveTransaction is not constructed: NXOS_SAVE names the external shadow, whose before/after contents
are retained; the production save tree is hashed read-only. One machine lease covers shadow preparation,
resumption, run, cleanup, stop restoration and save audit. A durable recovery intent blocks every other caller
and survives abrupt owner death; original stop/pending/in-process poison checks still run normally.

## Verified

Current source/buildDir/outputName read from `C:\Users\Risky Biz\AppData\Local\nxrs\library.json`.
Historical Wonder run and shadow-save provenance located under external `nxrs-agent2` evidence. Production data
directory currently has no `save` entry. Production-save absence/content/identity will be verified before/after.

### Actual attempt and thermal refusal

Executed from `D:\Projects\RecompLauncherEngine` with `PYTHONDONTWRITEBYTECODE=1`, tool bound 1830 s and an
independent 1800-second job:

```powershell
py "D:\Projects\RecompLauncherEngine\scripts\wonder_validation.py" build
```

At **2026-11-23 10:58:05.661 -05:00**, initial preflight refused before the normal launcher, engine adapter,
compiler or game could execute. Result: `child_pid=null`, **1.843 seconds**, **one observation**, positive cleanup.
Runner **14000**, watchdog **4740**, job `Local\nxrs-harness-49c5dca4d84f477092669a063f7061fa` are historical
identifiers only; no PID is future termination authority.

The exact persisted refusal is:

`thermal refusal: cpu_c unknown: awaiting observed sample progression (two separate reads required)`

**Why this is a genuine safety refusal, not a routine setup error:** the CPU reading already contained **91 C**.
SensorProvider deliberately does not wait for startup progression when the first reading is at least 85 C (or
has an asserted protective flag). Source had accepted its recent sample timestamp but had not yet observed a
second advancing read; status therefore remained unknown. Both lack of established progression and the above-cap
logged value prevent launch. Nothing was re-stamped as fresh, lowered, ignored or retried to force a pass.

| Retained preflight observation | Value / scope |
|---|---|
| HWiNFO local sample | **10:58:02.164** on 23.11.2026; about 1.77 s old when acquired |
| CPU package occurrence at column 222 | **91 C** |
| Other CPU package occurrence / IA cores | **90 / 90 C** |
| Core Max / aggregate core temperatures | **90 / 87 C** |
| Motherboard CPU-labelled / socket | **89 / 59.5 C** |
| NVIDIA device-wide maximum GPU | **52 C** |
| Machine CPU load | **22%**, management-query sample |
| Protective boolean fields | 91 validated fields; no asserted flag in this sample |
| Observed CSV progression | Not established; hot first reading prevented startup warm-up |
| Refusal policies | CPU **85 C**, GPU **80 C**, age **12 s**, unchanged |
| Owned workload memory | 0 MiB sampled; no workload child was created |
| Sample gaps | Not applicable: one observation only |

The 91 C reading predates all title build/game execution in this attempt. This does not identify the cause of
the temperature or attribute it to another process. No claim of a steady-state/live progression-verified thermal
series is made from this one sample. Per the explicit instruction, the workload attempt ended here.

Evidence directory:
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-wonder\build`

- `BUILD-STOP.json`: exact refusal plus all sensor identities, timestamp, CPU occurrences and management samples.
- `result.json`: refusal, elapsed time, job/runner/watchdog identities, null child and positive cleanup.
- `save-audit.json`: production save absent before and after, unchanged.
- `post-refusal-audit.json`: current title metadata/artifact hashes, verified production release/tool hashes,
  original shared-stop/pending hashes, absent root target and confirmation that resumption was never entered.

### Preservation and final blocking state

The bounded read-only audit passed. The normal library entry still has
`lastBuilt="2026-11-22T18:17:15"`, `buildIncomplete=false`, workers=0, and its original source/buildDir/name.
No launcher process ran, so no build metadata transaction was started. There are **no new title artifact hashes**.
The current old artifacts exactly match the historical successful Wonder provenance:

| Existing artifact in the configured Wonder `[nxrs]` folder | SHA-256 (old, unchanged) |
|---|---|
| `Super Mario Bros Wonders Switch 2 Edition.exe` | `dc1cc43078b6a2007fb2e15aac286982070cc1894138049442b955996260549d` |
| `Super Mario Bros Wonders Switch 2 Edition_code.dll` | `50154213cbed501c01089c94d26d502a2006bf789ac465b62d74e2241bd7b5c8` |

The verified dist executable/library/tool hashes still match the completed production rebuild. Root Cargo
target is absent. Production save manifests before/after are both `{exists:false, files:{}, directories:[]}`;
there is no save-file hash to report for an absent tree. No shadow save was copied or changed because play was
not reached. No production save rename/restore or game copy occurred.

**Shared gate never cleared.** Resumption/history-archive code was exercised only against temporary test files.
Actual `GPU-STOP.json` and `.pending` remain byte-for-byte equal to the reviewed SHA-256 values above; no
`WONDER-RESUMPTION.json` or `GAME_LOCK.thermal-wonder` exists. A direct call to the actual `require_clear(paths())`
still refuses with `pending fault journal requires review`. External title-specific stops were not touched.

At **10:58:36 -05:00**, read-only census found no build/game/supervisor/test/watchdog/snapshot workers.
At **11:01:09.701 -05:00**, the expanded census likewise found no owned workers or game/claim, but did observe
`mspdbsrv.exe` PID **31516**; it was absent when queried specifically immediately afterward. This attempt started
no compiler and does not claim ownership or causality for that transient server. It was not terminated.
HWiNFO64 **PID 27844** remained running. All our native test/monitor jobs were positively closed and reaped.

Exact files changed are the ten paths in the intended table. Shared lead summaries, engine/launcher sources,
library metadata, original CSV and all existing fault histories remain untouched. No stage/commit/push.

## Unproven

- **Rebuild and gameplay validation are blocked by the actual preflight refusal.** No ordinary setup correction
  or unchanged retry can substitute for review of this above-cap reading and missing progression qualification.
- Real launcher-to-adapter build transport, new static-library relink, actual worker counts during compilation,
  new title hashes/build time/metadata, and real resumption restoration remain unexecuted. CPU tests establish
  the candidate boundaries; they do not establish that the real title was rebuilt or played.
- No new screenshots, movement/jump result, gameplay time window, FPS/p95/p99/stutters, audio/non-silence/underruns,
  runtime memory/VRAM trend, red-diagnostic or device-reset outcome exists. Historical Wonder PASS is not this
  run's result. There is no new screenshot path for visual acceptance and no gameplay PASS claim.
- Machine thermal cause remains unproven. No broad library resumption, other GPU workload, NDVE activation,
  fan/clock/power/firmware/TDR change or HWiNFO GUI/logging manipulation was performed.

## Slips

No policy deviation or fault retry. The generic first-sample status text emphasized unestablished progression;
the retained evidence additionally exposed the decisive above-cap CPU value. The workload was not restarted.

## Questions

No routine permission/setup question. The genuine blocker is the logged **91 C CPU** preflight sample and its
unqualified progression status. Lead/owner thermal review is required before any later workload attempt; no
automatic resumption is queued, even if subsequent idle temperatures fall.
