# nxrs feature audit against nxnative and Turbo

Date: 2026-11-20. Auditor: independent audit lane (read-only; no engine code changed, no game run).

References audited:

- **Turbo**: `D:\Projects\Switch2RECOMP-main`, branch `turbo`. Covers the 124 owner commits (`git log upstream/main..turbo`), `runtime/` (C runtime, services, Vulkan back end under `runtime/nvn/vk`, OpenGL renderer), and `switchrecomp.py` with the build scripts.
- **nxnative**: `D:\Projects\nxnative`, at the 7ebb76e import point. The local checkout is at e4b0d9b, 131 commits behind, but the 7ebb76e objects are present. Covers `dm/host`, `dm/gfx`, `dm/games`, `dm/tools`, `scripts` and `docs`.
- **nxrs**: `D:\Projects\RecompLauncherEngine` at d865ca4 plus the working tree.
- **Launcher**: `D:\Projects\RecompLauncher`, `docs/LAUNCHER.md` (contract v2).

Method:

- Every feature of the two references was located in nxrs by file and function, and classified.
- **Done** means implemented and evidenced: by a unit or GPU test, or by a run cited in PORT_PLAN or INTERFACES. Every cited run folder under `build/` was checked to exist.
- Statuses were then compared with PORT_PLAN.
- Evidence was gathered by reading code, with `git log` / `git log -S`, `nxrs-build contract`/`help`, `cargo test -p profiles` (14/14 passed) and `scripts/port_map.py --check` (0 problems).

## Summary

Status key:

- **Done**: implemented and evidenced.
- **Better**: exceeds the reference.
- **Partial**: implemented, but something is missing (named in the row).
- **Stub**: answers success or a constant, or writes a file nobody reads, where the reference does the work. This breaks the owner's no-fake-features rule unless the reference stubs it too.
- **Missing**: no implementation.
- **N/A**: dropped by design, Apple/iOS-only, Linux-only (M10), or the reference stubs it as well (parity).

Matrix counts (one row per feature below):

| Done | Better | Partial | Stub | Missing | N/A | Total |
|---|---|---|---|---|---|---|
| 120 | 39 | 59 | 21 | 85 | 22 | 346 |

### Headline findings

1. **The plan is mostly accurate on what it lists, but stale in wording.**
   - Every "Done (uncommitted, lane T/S/P/G3)" row has in fact been committed: 8f627a6, 7360d85, f0bd36f, de2a969, cb4b426 and c76a0f5.
   - The 5A.3 ini row's list of "still unread" options is out of date: all of them are now read.
   - The M0 profile count is wrong: there are 207 of 207 profiles and 29 of 29 `patches.py` files, not 187 of 192 and 27 of 27.
2. **Claimed Done but not Done:**
   - **Metrics (5A.3):** the counter registry has no producers, and `frames-*.csv` is always empty.
   - **The Trinity rendezvous fence (4.5):** the plan says it was "verified on Violet and Z-A". No code exists for it.
   - **XCI as the game's package (5A.1):** never built and run as a game.
   - **Control pipe (5A.3):** about 20 of Turbo's commands are missing.
   - **Hotkeys (5A.3):** F3 and F7 grab the window, not the game frame; F5 writes no thread states.
   - **PORT_MAP's Done rows** for `pso_offline.py` (profile prewarm), `nvn_deswizzle_gpu`, `nvn_rt.h` (MMCSS) and `nvn_builtin.h`: none has an implementation.
3. **Stubs and fake successes found:**
   - **Any SVC nxrs does not handle answers success.** This covers resource limits 0x30/0x31/0x37, GetSystemInfo 0x6F and SetGpuMemoryAttribute 0x80; Turbo returns real values or NOTIMPL / NOTFOUND.
   - SetMemoryPermission and SetMemoryAttribute (0x02/0x03) record nothing.
   - Touch input is always idle.
   - The focus message 15 is never sent, because `oe::FOCUS` is never written.
   - **Alpha test** is recorded but only logged red (hit in Smash).
   - **Profile tables are written into every build and read by nothing:** `pso_manifest`, `shader_facts`, `prewarm_shaders`, `player_settings`.
   - The nxnative frame-step, MATCH, stub and observation-gate patch entries are parsed and round-trip tested, but never applied.
   - **`renderer=opengl` silently becomes vulkan**, while the launcher still offers OpenGL.
   - **DLC titles whose data is missing are still offered to the game.** Turbo fixed this in 2cdeabd.
   - Unhandled IPC commands answer zeroed success. This matches Turbo and nxnative, and is logged as a gap.
4. **Biggest gaps:**
   - nxnative's cheats engine and panel (`hle_cheats.c`).
   - The engine frame lock and frame step (`hle_fpslock.c`, `nvn_fstep.h`, FRAME_STEP stubs). OoT and MKW need these.
   - `nso_match` MATCH rules. These carry Arceus's crash fix and OoT's movie cuts.
   - Graphics shader edits: dual-source blending, integer output variants, alpha test, and the RTX 50 ballot fix. These are the leads for Jamboree's white layer and Smash.
   - Pipeline prewarm from the profile on a first run.
   - Capture and replay.
   - Linear render-target readback, VRAM eviction, the sharp present upscale, and the NDSR / movie network run by the host.
   - Front-end prompts: Home quit, controller join, user select. Mouse-as-touch.
   - Turbo's per-slot shared fonts.
   - Engine CI.
   - Turbo saves are not found by nxrs builds: `save\user` against `save\user<N>`.
   - The launcher keys fallback path is stale.
5. **Regressions (features switched off by default or bypassed):** none in nxrs's own switches. All `NXGFX_*` feature switches default on, and nothing was disabled after a crash. Two caveats:
   - Built-in frame generation is on by default (`lsfg=1`), although it garbles Arceus's village and hangs a Jamboree screenshot. Lane W runs with `lsfg=0`.
   - Four Turbo defaults are not ported: the ballot fix, the sharp upscale, linear render-target readback, and storage use without the IMAGE flag (now on by default in Turbo).
6. **Better than the reference (39 rows).** The main ones:
   - Resolution scale, host dynamic resolution, shadows, fps limit / skip, LSFG and recording all work on Vulkan; Turbo has them in GL only.
   - Built-in frame generation.
   - Pipeline libraries and descriptor buffers on by default.
   - CPU texture rewrites detected in every title, not two.
   - GPU ASTC decoding.
   - 231 NVN procs that Turbo only stubs.
   - The in-process SDK audio renderer, real AudioIn, and motion.
   - Switch Pro, Joy-Con and Switch 2 BLE pads, 8 players.
   - A virtual-core scheduler.
   - Real ExecuteProgram, the Mii database, and bsd event descriptors.
   - Mods and patches with a run-time switch.
   - 40+ shader archive formats.
   - Honest refusal of unknown services.

## Feature matrix

Columns: feature | nxnative | Turbo | nxrs file / function | audit status | plan status | note. "—" means the reference
has no such feature. Plan status "absent" means no plan row covers it.

### A. Build, packages and the launcher contract

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| Folder input (exefs + romfs) | `build_ipa_from_nsp.py` | `nxloader.load_program` | `translator::package::open_game` (`package/mod.rs:169-174`) | Partial | 5A.1 Done | A flat folder looks for `romfs/` in the parent folder. A folder without a RomFS is a hard error, where Turbo treats the RomFS as optional |
| Folder control data (`control.nacp`, icon) | — | `switchrecomp.py:497-501` | none | Missing | absent | The launcher shows no version and no cover for folder builds |
| NSP in place (SWRNSP1) | — | `nsp.py`, b100066 | `package/nsp.rs`, `app.rs` | Done | 5A.1 / M0 | |
| Combined and encrypted NSP, SWRNSP2, BKTR, AES-CTR | — | 45fdc8a, 301ae2a, `runtime/nsp.c` | `app.rs`, `bktr.rs`, `crypto.rs`, `marker.rs` | Done | absent | MK8D 1G+1U+1D `_data` is byte-identical to Turbo's |
| UTF-8 marker paths, ASCII output names | — | 87cd6f1 | `pipeline::output_name` (unit test) | Done | absent | |
| NUP in place, `romfs.nup` | `pack_nup.py` | 36c67b3, `nup.py` | `package/nup.rs` | Done | M0 | |
| XCI and its secure partition as the game's package | — | `nca.open_ncas` | `package/container.rs`, `compressed.rs` | Partial | 5A.1 Done | Unit-tested only; never built and run as a game. The launcher's `validSource` refuses `.xci` |
| Bare NCA + `.tik` as an update | — | `nca.py` | `app.rs` | Done | absent | Test `separate_update_nsp_xci_and_bare_nca` |
| NRO homebrew input | — | `load_nro` | `Detected::Nro` | Better | 5A.1 | The RomFS is read in place |
| ZBIC NSOs, one decoder for translator and shaders | — | 3eac6c2, b196d14 | `loader/zbic.rs`, `shaders/formats/zbic.rs` re-export | Done | 5A.1 Done | Committed in 8f627a6. OoT builds |
| Code ABI bump: never pair new code with an old runtime | — | e440705 | runtime linked statically into `_code.dll` | Better | absent | |
| DLC install: NSP, NSZ, XCI, XCZ, NCA | — | 8f6603e | `package/dlc.rs` | Done | M9 | Tests `dlc_from_nsz_solid_and_block`, `dlc_from_bare_nca_and_xci` |
| DLC: several packages, shared title stays with the installed one | — | 8db1151 | `dlc::install` / `sync` | Done | absent | Done by reading the code; no dedicated test |
| DLC whose data is missing is not offered to the game | — | 2cdeabd, `dlc.c dlc_has_data` | `os/services/fs.rs:778 aoc_indices` offers every manifest title | Stub | absent | The regression that hung MK8D in Turbo |
| Updates in their own package: `--update`, `--no-update`, `--update-info` | — | 40df1f9 | `UpdateChoice`, `package::update_info` | Done | M0 / 6.1 | `--update-info` is byte-identical to Turbo's |
| Execution hints (`.hints`) and the interpreter fallback (`--interp-only`) | — | `program_hints`, `swr_interp_entry` | `dispatch.rs:229`: a miss is fatal | Missing | absent | Dropped without a recorded owner decision. `discovered.tsv` (section 3.1) does not exist either |
| `-o`, `--outdir`, `--chunk`, `--native`, `--no-build`, `--keep-c`, `-q` | — | `switchrecomp.py:629-663` | `cli/main.rs`, `pipeline::build` | Done | 5A.1 Done | |
| `--jobs` default | — | `cpu_count` | 4 | Partial | absent | The launcher always passes it |
| `--cc`, `-O`, `--backend`, `--runtime`, `--build-runtime` | — | real | accepted and ignored | N/A | 6.1 agreed | Agreed in LAUNCHER_CONTRACT v1 |
| `--target linux` | — | real | refused | Missing | M10 | |
| `--no-shaders` | — | real | `pipeline::shaders_step` | Done | 5A.1 | The message at `pipeline.rs:312` wrongly says old shaders only (a live translator exists) |
| Shader prebuild: parallel with the compile, never fatal | — | 6d04b3b `start_shader_prebuild` | `shaders_step`: serial, a failure fails the build | Partial | 5A.1 Done | Exit 1 where Turbo exits 0 |
| `SWR_CFLAGS` | — | `switchrecomp.py:768` | none | Missing | absent | |
| Reserved Windows names in ExeFS output | — | `switchrecomp.py:440` | `mod.rs:286` partial | Partial | absent | CON, NUL, COM1 and trailing dot or space are not checked |
| Built-in game patches off switch | — | `SWR_NO_BUILTIN_PATCHES` | `NXRS_NO_PROFILE_PATCHES` | Done | absent | |
| Z-A 60 FPS patch | — | ac8e422 `nxloader` | `profiles/games/0100F43008C44000/turbo.toml` | Better | 5A.6 | Switchable at run time |
| OoT full-resolution built-in patch | — | `nxloader.py:56` | not ported | Partial | absent | The OoT profile carries nxnative's own; equivalence not shown |
| Log of how the RomFS is read and the title's version | — | 13a2693 | `fs.rs:110` (RomFS source); the version is not logged | Partial | 5A.2 | |
| Toolchain without Visual Studio (bundled llvm-mingw, gnullvm) | — | a1c5d81 | `cc::llvm_mingw_clang` | Done | 5A.1 Done | |
| Release packaging of the engine | — | 091decf…a601ce6, moved to the launcher (7f23568) | launcher `package-release.py --nxrs-engine` | Partial | M9 | The launcher's `release.yml` never passes `--nxrs-engine` |
| Engine CI (tests, native builds) | — | a9a4add `.github/workflows/engine-ci.yml` | none | Missing | M0 open | `cli/README` says there is no remote; that is stale (origin exists) |
| `PORT_MAP.tsv` complete, with a fast check | — | — | `PORT_MAP.tsv`, `scripts/port_map.py --check` (uncommitted) | Partial | M0 open | The check is not in `cargo test`, and several rows say Done wrongly (section C) |
| Contract v2: `contract` / `--contract`, subcommands, `@progress` / `@result` | — | — | `cli/main.rs`, `out.rs` | Partial | M9 Done | `@progress data` is never sent; `shaders` is sent once as 0/0 |
| Contract v1 progress lines, UPDATE and DLC lines, exit codes | — | `switchrecomp.py` | `pipeline.rs`, `cc.rs:412`, `mod.rs:332-339` | Done | 6.1 | |
| `builtFiles` layout, `nxrs.gen.key` stamp | — | — | `pipeline::build` | Done | M9 | Section 6.1's table is stale: `nxrt.dll`, `nx-run` and `discovered.tsv` |
| `engine.json` | — | — | written by the launcher's packager, not the engine | N/A | 6.2 | 6.2's wording should change |
| `doctor` | — | — | `cli doctor` | Done | 5A.1 Done | The launcher does not use it (M9 open) |
| Keys: `SWR_PROD_KEYS`, `--keys`, default path | — | — | `package/keys.rs:104` | Partial | absent | The fallback is `%LOCALAPPDATA%\RecompLauncher\RecompLauncher`; the launcher moved to `%LOCALAPPDATA%\nxrs` (launcher d538b31). This also affects `doctor` |
| Save location compatible with Turbo builds | — | `svc_fs.c:511` `save\user` | `fs.rs:937-980` `save\user<N>` | Missing | 6.3 task | Nothing migrates the saves. This answers 6.3's open task |
| ini keys the launcher reads and writes | — | `main.c` | `config/options.rs` | Partial | 5A.3 | `renderer=opengl` / `eden` is silently turned into vulkan (`options.rs:201`) |
| Mods: IPS, IPS32, `.pchtxt`, `mod.json`, RomFS layering, run-time switch | — | — | `profiles::mods`, `cli/mods.rs`, `os/mods.rs` | Better | 5A.6 Done | Committed (7360d85, cb4b426); the plan says uncommitted |
| Exclusive monitor for Switch 1 only; LDAR / LDAPR / STLR / LDXR as atomics | — | b89e9e5, efa9b2a | `runtime/cpu.rs:52`, `process.rs:82`, `xlate/lifter.rs:489-497` | Done | 5A.1 Done | |
| Translator: byte-identical to mkw-recomp, coverage | — | — | `translator/xlate`, `aot` | Done | M1 Done | |
| 6 SDK crypto seeds; `bignum.rs` workaround removed | — | — | aot seeds | Done | 5A.1 Done | "left to lane O" is stale: `bignum.rs` is gone |
| dynarmic differential lane | — | — | none | Missing | T-lane Not started | |
| Host sampling profiler (`SWR_HOST_PROF`, `host_prof.py`) | — | `tools/host_prof.py` | `rt.rs:225 fnprof_hit` empty, `g_fnprof` never set | Stub | absent | `modules.c` declares the hook |
| Scenario harness, guard, PresentMon | `regress.py`, `bench.py` | `play_scenario.py`, `guard.py` | `scripts/play_scenario.py`, `run_game.py`, `library_play.py` | Done | 5A.3 Done | |
| Bug report, run-saves, triage tools | `bug_report.py`, `run_saves.py`, `triage_report.py` | — | none | Missing | 4.5 / 4.6 PORT | |

### B. OS, kernel and memory

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| HLE registry: force, hle, alias, weak imports | `hle.c`, `hle.h` | — | `os/hle.rs` Registry | Done | M3 | |
| Unimplemented import → logging stub returning 0 | `hle.c unimpl_c` | `nxrt_stub_log` | `hle.rs` UNIMPL Import | Done | M3 | Same policy in all three |
| HLE_FORCE_DIRECT (naked or x8 hidden-result) | `hle.h` | — | none | Missing | absent | Only prepo `PlayReport::Send` (ACNH) needs it |
| Memory SVCs 0x01, 0x04-06, 0x13-15, 0x2C/2D, map limit | `hle_svc.c` | `kernel.c`, b316a9f | `os/svc.rs:177-195`, `os/mem.rs:267,412` | Done | 5A.1 Done | |
| SetMemoryPermission / SetMemoryAttribute (0x02/0x03) | `hle_svc.c` | `mem_set_state` / `mem_set_attr` | `os/svc.rs:184` returns 0 and records nothing | Stub | absent | QueryMemory never shows the change |
| Thread and sync SVCs, WaitForAddress type 3 (WaitIfEqual64), SignalToAddress | `hle_thread.c` | 362b172 | `runtime/kernel.rs:707-866` | Done | 5A.1 Done | |
| SetThreadActivity, GetThreadContext3 | — | — | `kernel.rs` | Done | 5B | |
| GetInfo, including ids 29/30/33 | `hle_svc.c` | `svc_get_info` | `os/svc.rs:269` | Done | 5A.1 | An unknown id answers 0 with a gap line |
| Resource limits 0x30 / 0x31 / 0x37 | — | `kernel.c:1118-1119` (real values) | `os/svc.rs:152 unhandled`: success, x1 untouched | Stub | absent | |
| GetSystemInfo 0x6F | — | `kernel.c:1130` | `unhandled` success | Stub | absent | Turbo notes that newer SDKs abort on 0 |
| SetGpuMemoryAttribute 0x80 (Switch 2) | — | `kernel.c:1013` | `unhandled` success; outside the `COUNTS` array | Stub | absent | |
| Unknown-SVC policy (0x2E, 0x2F, 0x4B, 0x40, 0x43, 0x50-52) | — | NOTIMPL / NOTFOUND + WARN | success + gap | Stub | absent | Fake success hides failures from the game |
| Tick 19.2 / 31.25 MHz, `SWR_TICK_HZ` | `hle_os.c` | 78ece7b | `process.rs:77`, `rt.rs` | Done | 5A.1 | |
| SDK 22 thread ID at TLS+0x110 | — | 78ece7b | `kernel.rs:650` | Done | 5A.1 Done | |
| Process memory cap (job object) | — | 8a4618c | `process.rs:139-183` | Better | 5A.1 Done | Raised to the pool plus 8 GB |
| Core scheduler with migration | `hle_thread.c` | ade68bf, cb516e7 (experimental, off) | `os/sched.rs` virtual cores (on) | Better | M2 | |
| Condvar wait race | — | dca1eb8 | own kernel design | N/A | absent | Not comparable |
| Stuck-mutex healer (each frame) | — | `kernel.c:410 kernel_heal_mutexes` | none | Missing | absent | A safety net |
| Host thread priority / QoS / MMCSS (DM_QOS_MAP, `nvn_rt.h`) | `hle_thread.c`, `nvn_rt.h` | — | only the kernel's `priority_changed` hook | Missing | 4.3 PORT (PORT_MAP says Done) | |
| Fibers, setjmp / longjmp, paused fiber stacks freed | — | — | `runtime/fiber.rs`, `sjlj.rs`, 9182594 | Done | M2 Done | |
| Guest clock steps per present (DM_DETCLOCK) | `hle_os.c` | — | none | Missing | absent | Diagnostic: lets replays be compared byte for byte |
| getentropy, csrng, game_env, big-alignment allocator | `hle_os.c` | — | `services/os.rs` | Done | M3 | |
| Fault report mapped to guest functions, stall watchdog, fatal peek | `crash.c` | `main.c crash_report` | `runtime/fault.rs`, `os/watch.rs` | Better | 4.2 | Unwinds the host stack |
| Minidump | `crash.c` (plan) | — | none | Missing | 4.2 REWRITE | |
| Probes: breakpoint at a guest address, heap dump, write watch | `probe.c` | — | `watch.rs peek`, `threads` | Partial | 4.2 | Breakpoints need a translator hook |
| Graceful exit: audio pause, graphics drain, console handler | `hle_lifecycle.c` | — | `front/lib.rs quit()` | Partial | 4.2 PORT | No audio pause, no drain, no SetConsoleCtrlHandler |
| HLE call counts dumped at exit (DM_HLECOUNT) | `hle_count.c` | — | `hostfn::count_call` (no dump) | Partial | 4.2 PORT | |
| Handle / fd statistics | `hle_fdstat.c` | — | none | Missing | 4.2 PORT | |
| Memory statistics; CPU per thread | `memstat.m` | `kernel.c` metrics | `front/metrics.rs` (private MB) | Partial | 4.2 | No per-thread CPU (DM_THRCPU) |
| File-backed guest memory (DM_GUEST_FILE, DM_ALIAS, DM_REGIONS) | `hle_svc.c` | — | — | N/A | absent | iOS-only |
| Big stack, `thread.c`, `entry.S` | `dm/host` | — | — | N/A | 4.2 DROP | |

### C. Services (HLE and IPC)

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| sm: | — | `sm_disp` 0-4 | `ipc` Sm 0, 1, 4 | Done | M3 | Commands 2 and 3 answer zeros |
| Unknown service refused | — | answers success | `ipc/mod.rs:182` ResultNotRegistered | Better | absent | |
| Unhandled command on a known service | same | same | `ipc/mod.rs:118`: zeroed success + gap | N/A | absent | Parity with both references; it is logged |
| oe / am: operation mode, language, docked / TV, performance mode | `hle_oe.c` (60) | d5b36a1 | `services/oe.rs` (60/60) | Done | 5A.2 Done | |
| ExecuteProgram successor | `hle_oe.c` | stub | `oe.rs:287` | Better | 5B | Sonic, both programs |
| RequestToRelaunchApplication | `hle_oe.c` restarts program 0 | stub | `oe.rs:245` ends the run | Partial | absent | |
| Focus change, message 15, applet focus in / out | `hle_oe.c dm_focus_set`, `dm_applet_enter` | — | `oe::FOCUS` is never written | Missing | absent | The game never hears about focus loss |
| Library applet IPC path | — | `svc_core.c:280-330` | SDK entry takeovers only | Partial | 5A.2 | |
| account: 8 users, UIDs, nickname | `hle_account*.c` (41) | — | `services/account.rs` (41/41) | Better | M3 | |
| User selector | `user_select_mac.h` dialog | — | `account.rs:150` always picks user 1 | Partial | 4.2 REWRITE | |
| ShowUserCreator | `hle_account.c` | — | `ok` | Stub | absent | |
| fs: transactional saves, cache, AOC, system data, program RomFS | `hle_fs.c` (74) | `svc_fs.c` | `services/fs.rs` (74/74) | Done | M3 / 5B | Violet's save commits |
| fs: SD card, BCAT save, save-data info reader, DeviceOperator (fsp 18, 60-62, 400, 500) | — | `svc_fs.c` | none | Missing | absent | |
| fs fault injection (DM_TEST_FS_FAIL) | `hle_fs.c` | — | none | Missing | absent | Test only |
| bcat delivery cache | — | `svc_core` bcat (empty cache mounted) | `simple_table.rs:138` mount fails 0x2A2 | Partial | absent | |
| aoc: count, list, prepare, purchase managers (aoc 4-13, 50, 100, 101) | `hle_misc` | `aoc_disp` | `misc.rs`, `fs.rs` | Partial | absent | |
| time: clocks, zones, shared page entries of 0x38 bytes | `hle_time.c` (37) | 1f12977 | `services/time.rs` (37/37), shared page `time.rs:367-399` | Done | 5A.2 Partial ("not served") | The plan is wrong: the page is served |
| time IPC commands 300 / 401, steady 3 / 300, tz 3 / 5 | — | `svc_core` | answer zeros | Partial | absent | |
| set: language, region, firmware | `hle_misc` | `set_disp` | `settings.rs`, `oe.rs` | Partial | absent | No GetKeyCodeMap, GetQuestFlag or GetDeviceNickName |
| set:sys | — | `setsys_disp` | none | Missing | absent | |
| psm battery / charger | — | `psm_disp`, `plat.c:196` | none | Missing | absent | |
| fatal:u | — | `svc_core.c:1048` | `diag AbortImpl` only | Partial | absent | |
| spl:, mm:u | — | `svc_core.c:1069,1240` | none | Missing | absent | Rarely reached |
| ns, ectx, fsp-pr, fsp-ldr, hid:sys | — | quiet stand-ins | absent | N/A | absent | Turbo stubs these too |
| sf hipc client takeover; olsc, caps, notif, mnpp | `hle_sf.c` (13) | — | `services/sf.rs` (13/13) | Done | M3 | |
| SDK init, SDK 16 helpers | `hle_sdkinit.c`, `hle_sdk16.c` | — | `sdkinit.rs`, `misc.rs`, `oe.rs` | Done | M3 | |
| pl:u shared fonts, one per slot (CJK, Nintendo button icons) | `make_shared_fonts.py`, `make_ext_font.py` | `main.c:676-707` | `misc.rs:239-270`: one stand-in TTF for all six slots | Partial | 4.5 PORT | Button glyphs and CJK are lost |
| lm, diag | — | — | `misc.rs` | Done | M3 | |
| pctl, nifm, friends, ssl, prepo, irs, ldn, ngc, nfp, caps (constant answers) | `hle_misc`, `hle_friends`, `hle_offline`, `hle_net` | — | `simple_table.rs` (296 rows) | Done | M3 | All match nxnative's constants |
| bluetooth BLE (Violet 3.0.1), btm:u | `hle_offline.c` (7 names) | 13a2693 | `net.rs` bt / btm:u over IPC | Partial | 5A.2 Partial | Not exercised by a game |
| bsd sockets, event descriptors, loopback UDP | `hle_net.c` (94) | 362b172 | `net.rs`, `socket.rs` | Better | 5A.2 Partial | |
| ssl `BuiltInManager::GetBuiltInCertificates` | `hle_net.c` | — | none | Missing | absent | |
| PlayReport::Send, ngc China × 2, album `GetSharePostedLastUrl` | `hle_offline.c` | — | none | Missing | absent | |
| Mii database: default Miis, UpdateLatest / GetIndex NotFound | `hle_mii.c` (17) | 2a42369, 5f80160 | `mii.rs` (17/17) | Better | 5A.2 | Real search |
| MiiModel system data synthesised | — | 9d0acd6, b7cde65 | `mii.rs system_data` | Done | 5A.2 | MK8D reaches races |
| MiiEdit applet | `hle_mii.c` | 9ae9441 answers "cancelled" | `mii.rs:300` ShowMiiEdit `ok`, AppendMii adds a default Mii | Partial | 5A.2 Partial | Diverges from Turbo |
| Software keyboard: ShowKeyboard, InlineKeyboard core, `keyboard_text`, font | `hle_swkbd.c` (31) | `svc_ns2.c`, `swkbd_font.h` | `swkbd.rs` (21/31), Win32 box | Partial | 5A.2 Partial | Not run by a game |
| InlineKeyboard: 10 methods not taken over (SetMovedCursorCallback, SetWindowMode, alpha / scale / translate, touch, hardware keyboard, GetImage, GetWindowSize) | `hle_swkbd.c` | — | none | Missing | absent | GetImage reads applet shared memory, so this is a crash risk |
| ro / NRO loading | `hle_ro.c` (10) | — | `ro.rs` | Better | 4.2 | Jamboree's 139 NROs |
| nn::jit | `hle_jit.c` | — | 4 of 7 names | N/A | 4.2 DEFER | No owner game |
| web applet / offline HTML | `hle_web.c`, `web_apple.m` | — | `offline.rs:149-333` closes immediately | Stub | 4.2 DEFER | The DEFER is allowed, but the stand-in is a fake under the owner's rule |
| Oodle | `hle_oodle.c` | — | `shaders/formats/archive.rs`, `gamefreak.rs` | Done | 4.2 DEFER (stale) | |
| GLSLC capture | `hle_glslc.c` | — | `nvdrv.rs:4-83`, `NXOS_GLSLC_CAPTURE` | Done | 4.2 DEFER (stale) | |
| Unreal cvars and malloc | `hle_uecvar.c`, `hle_uemalloc.c` | — | none | N/A | 4.2 DEFER | No owner game is Unreal |
| nvdrv / nv HLE | `hle_nvdrv.c`, `hle_nv.c` | `nvdrv.c`, `host1x.c` | `nvdrv.rs`, `nvn/nv.rs` | Done | M3 | Open fails honestly; NVDEC / VIC channels by design |
| vi: displays, layers, indirect layer | `hle_vi.c` (32) | `svc_vi.c` | `vi.rs` (31/32) | Partial | absent | No `ListFrameHistory` (UE5 on Switch 2); no IHOSBinderDriver path; 2450/2451 has the size query only |
| Switch 2 services (sync, worker, gpusess, vi2, gsv, vdec), with a catch-all | — | `svc_ns2.c` | `codec_video.rs`, `vi.rs`, graphics | Partial | 5A.2 | Never run; no catch-all for unknown Switch 2 services |
| chat:u (Switch 2) | — | `svc_ns2` | `offline.rs` | Better | absent | |
| Palma (Poké Ball Plus) failures | `hle_hid.c` (24) | — | 4 of 24 | Missing | absent | No owner game |
| Cheats: Atmosphère `.txt`, dmnt VM, master codes, `.enabled`, `.meta.tsv`, apply thread | `hle_cheats.c` | — | none | Missing | 4.2 PORT | No 5A row. Code cheats need build-time image patches (risk R2) |
| Engine frame lock (Unity / UE / NieR / fb paths, DM_FPS_TARGET hold) | `hle_fpslock.c` | — | none | Missing | 4.2 PORT | The profile's `fps_target` is unread at run time. The ini `fps_target` means Turbo's frame skip: a name clash |
| SDK-variant `InitializeH264Decoder` overload | `hle_codec_video.c` | — | none (25/26) | Missing | absent | |
| `movie::Extractor::SetDataSource` | `hle_movie.c` | — | none (26/27) | Missing | absent | |

### D. Audio and video

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| WASAPI sink, limiter, `audio_slowdown`, wall-clock release | `hle_audio.c` | `audio.c`, `audio_queue.h`, `audio_limit.h` | `media/sink.rs` | Done | 5A.2 Done | MK8D 0 underruns |
| AudioOut | `hle_audio.c` (63) | — | `audio.rs` (63/63) | Done | 5A.2 | |
| Audio renderer in process (the SDK's mixer) | `hle_audren.c` (23) | `main.c audren_update` (host mixer) | `audren.rs` | Better | 5A.2 Partial | Supersedes Turbo's mix fixes a22b102 and 8a265ab |
| audren:u for SDK 1.x | `hle_audren_sv.c` | `svc_core.c:740-898` | none | Missing | 5A.2 Partial | No owner game needs it |
| IPC aud / audout / audin (SDK 1.x) | — | `svc_core.c` | none | Missing | absent | |
| AudioIn on a real device, or "no device" | — | stand-in (AudioOut) | `audio.rs`, `media/mic.rs` | Better | 5A.2 Partial | No game opens it |
| AudioIn C API | `hle_audio.c` | — | `simple_table` StartAudioIn fails 0x2A2 | N/A | absent | Same as nxnative |
| hwopus with libopus | `hle_codec.c` (62) | c406ae6 | `codec.rs`, `media/opus.rs` | Partial | 5A.2 Partial | Unit-tested; no game run. A bad packet becomes 20 ms of silence (`codec.rs:348`), where Turbo returns an error |
| RMS / peak log line every 5 s | — | `audio.c:149-160` (9ce9b8e) | none | Missing | absent | |
| Audio device choice, mute, WAV dump | `hle_audio.c` DM_AUDIO_* | — | none | Missing | absent | |
| H.264 (Media Foundation) | `hle_movie_avc.c` | `oh264` | `media/mf.rs`, `movie.rs` | Done | 5A.2 Partial | MK8D 926/926 pictures. The panel now plays (`-gp3`); the plan's "panel still draws black" is stale |
| VP9 (libvpx), Switch 2 GSV | `hle_movie.c` | f9c8469, 64237e0 | `media/vpx.rs` | Done | 5A.2 "not run" | Shadow Generations played it for 18 min; stale |
| AV1 (dav1d) | `hle_movie_av1.c` | 6c4578c, 5093a7d | `media/av1.rs` | Partial | 5A.2 Partial | Not run |
| Switch 2 decoder: fences that wait, re-release, back-pressure | — | 0ae160a | `codec_video.rs` | Partial | 5A.2 Partial | Not run |
| Movie / NDVE / NDSR network run by the host (OoT movies, agl AA) | `nvn_ndve.inc` | 9ce9b8e, `vk_replay.c:1918` | none | Missing | absent | |
| Vorbis / AAC movie audio | `hle_movie.c` | — | `movie.rs` | Better | absent | |

### E. Input

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| Npad states, styles, assignment, LEDs, colours, debug pad, keyboard, mouse | `hle_hid.c` (157) | `svc_hid.c` | `hid.rs`, `hid_ext.rs` (137) | Done | 5A.3 Done | |
| XInput with rumble | — | `backend_win32` | `input/xinput.rs` | Done | 5A.3 Done | Fixtures only |
| DualSense / DS4 lights, rumble, motion | `pad_hid.h` | 88401aa, `controller_sony*.h` | `input/sony.rs` | Done | 5A.3 Done | Confirmed by hand on hardware |
| Switch Pro, Joy-Con 1, NSO pads | `pad_pro.h`, `pad_jc1.h` | — | `input/nintendo.rs` | Better | 5A.3 Done | Fixtures only |
| Joy-Con 2 / Pro Controller 2 over BLE | `switch2_proto.h`, `pad_switch2_apple.m` | — | `input/switch2*.rs` | Better | 5A.3 Done | Fixtures only |
| WinMM fallback, hot-plug, 8 players | — | 6aace78 | `winmm.rs`, `input/mod.rs` | Done | 5A.3 Done | |
| Motion fusion | `motion_fusion.h` | stub (pad at rest) | `input/motion.rs` | Better | 5A.3 | |
| Game rumble reaches the pad | `hle_haptics.c` | 64237e0 | `hid.rs vib_send` | Done | 5A.3 Done | MK8D 1,138 calls |
| Vibration permitted / mounted / init (idle vibration on init) | `hle_hid.c` | — | constants; InitializeVibrationDevice does nothing | Partial | absent | |
| Rumble setting, gain, cutoff (DM_RUMBLE, _GAIN, _MIN) | `settings.md` | — | none | Missing | absent | |
| Press to join, pad merge, keyboard player (DM_PAD_*) | `settings.md` | — | none | Missing | absent | |
| Joy-Con side / Switch 2 motion side / map (DM_JC1_SIDE, DM_SW2_*) | `dm/host` | — | none | Missing | absent | Minor |
| `button_layout`, `button_map` | — | 08130c4, 10e1ef4, ac1bd36 | `input/layout.rs` | Done | 5A.3 Done | |
| Keyboard as a controller | `front_mac.m` | `backend_win32.c` | `front/keys.rs` | Done | 5A.3 Partial | MK8D |
| Mouse as the right stick (sensitivity, invert) | — | `backend_win32.c` | `keys.rs Mouse::stick` | Partial | 5A.3 Partial | Unit-tested, not used in a game |
| Touch screen (mouse = touch, short-press latch, scripted touch), gestures | `hle_touch.c` | idle | `hid.rs:793 touch_fill` always idle | Stub | absent (4.2 PORT) | Jamboree handheld, Beach Bounce menus |
| ShowControllerSupport waits for joins (controller prompt) | `ctrl_prompt.h` | — | `hid.rs:526` answers at once | Partial | 4.2 PORT | MK8D, Smash and Jamboree multiplayer |
| ShowControllerStrapGuide / FirmwareUpdate with applet focus | `hle_hid.c` | — | `ok` | Partial | absent | |
| Input record / replay | DM_REC | 8f5410f, `SWR_INPUT_FILE` | `input/script.rs` | Done | 5A.3 Done | MK8D |
| IMU axes, signs, noise, delay (DM_IMU_*) | `dm_input.h` | — | none | Missing | absent | Diagnostic |
| hid IPC and shared-memory LIFOs | — | `svc_hid.c:80-264` | takeovers instead | N/A | absent | By design (SDK-level takeover) |

### F. Front end: window, hotkeys, recording, control pipe, metrics

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| Window, fullscreen / windowed, F11 / Alt+Enter kept in the ini, Alt+F4 | `front_mac.m` | `backend_win32.c`, 9ce9b8e | `front/window.rs:94,233,260` | Done | 5A.3 Done | |
| Esc quit, F1 help, title-bar states, GUI-subsystem loader | — | `backend_win32.c` | `window.rs`, `help.rs` | Done | 5A.3 Done | |
| Focus loss releases keys, repeat filter, cursor hidden / clipped | `front_mac.m` | `backend_win32.c` | `window.rs:210` | Partial | 5A.3 | Releases the keyboard only, not held pad input |
| Session-end and power logs | — | `backend_win32.c:442-445` | `window.rs:195-201` | Done | absent | |
| F3 screenshot of the game frame | — | `g_shot_req` (frame readback) | `capture.rs` PrintWindow (window grab) | Partial | 5A.3 Done | F8 / `shot` go through graphics' `vk::shot` (Done) |
| F4 FPS limit | — | `backend_win32.c:465` | `fps_limit_toggle` | Done | 5A.3 Done | |
| F5 diagnostic dump (threads + render targets) | — | `kernel_dump_threads`, `gpu_gl_dump_rts` | `window.rs:152 diag_dump`: gaps and pads only | Stub | absent | `nxrt::kernel::dump_threads` exists but is not called |
| F7 recording: H.264 + AAC MP4 | — | `rec_win32.c` (GL only) | `front/rec.rs` | Partial | 5A.3 Done | Better because it works on Vulkan. Partial because frames are window grabs and the audio is taken after slowdown and limiter |
| Smooth recording, virtual clock | — | `main.c vt_*` | `os/clock.rs` | Done | 5A.3 Done | Clock 0.30 verified |
| Recording stall watchdog, original-timeline audio | — | `rec_flow.h`, `rec_audio_time.h` | none / `sink.rs set_tap` | Missing | absent | |
| Recording debug switches (SWR_REC, _RAW, _MP4_CLASICO) | — | `backend_win32.c:517`, `rec_win32.c` | none | Missing | absent | |
| Control pipe: status, press, pads, shot, quit, peek, threads, counters, fullscreen, run | DM_CTL | 0b0d902, d54f04e, `vk_ctl.c` | `front/ctl.rs`, `watch.rs`, `input/script.rs` | Done | 5A.3 Done | |
| Control pipe: Turbo's graphics debug commands (flight, flashes, pause, resume, step, hide, show, showall, hidden, gputime, rawdump, passview, watch, drawstats, texdump, wpage, wtrace, bp, rdoc, inpool, texat, switches) | — | `vk_ctl.c:93-159` | `passdump`, `a2c`, `pcache`, `gfx` only | Missing | 5A.3 Done | |
| Control pipe: nxnative's memdump, cvarf / cvari, drag, env, addr, program, texlist / texraw, bufsnap, webshot, atrace | DM_CTL | — | none | Missing | absent | |
| Control pipe `set` / `get` | — | graphics live switches | ini values | Partial | absent | Same names, different meaning |
| Front's `shot` handler overridden by graphics | — | — | `ctl.rs:90` dead code (`graphics/lib.rs:63` wins) | N/A | absent | Cleanup |
| Metrics CSV per second | DM_MEMSTAT | `kernel.c:925-985` | `front/metrics.rs` (seconds, frames, fps, cpu%, MB) | Partial | 5A.3 Done | No guest CPU, busiest threads, waits by kind or xmon |
| Dropped-work counter registry | — | 8f5410f | `config/metrics.rs counter/add`: no producer in any crate | Stub | 5A.3 Done | Only the readers exist |
| frames CSV turned on by `metrics=1` | — | `kernel.c` sets SWR_FRAME_CSV | `front/metrics.rs:59` creates a file nobody fills | Stub | 5A.3 Done | Graphics' frame CSV needs `NXGFX_FRAME_CSV` |
| threads.txt every 15 s, stall dumps at 6 / 15 s, driver-crash fatal after 8 s frozen | — | `main.c:512-524` | `watch.rs`: one dump at 15 s | Partial | absent | |
| Heartbeat, exit-after | — | — | `os/watch.rs` | Better | absent | |
| Pacing line at least every 5 s, Turbo's format | — | 4f0f731 | `vk/present.rs:384` | Done | 5A.3 / 5A.4 Done | |
| Mouse as touch, Home held → quit prompt, user select, cheats panel | `front_mac.m`, `home_quit.h`, `user_select_mac.h` | — | `input::home_held` has no caller; the rest is absent | Missing | 4.2 PORT / REWRITE | |
| Screen refresh match, keep the display awake while focused, window on top, focus keep | DM_SCREEN_MATCH, DM_WINDOW_TOP, DM_FOCUS_KEEP | — | none | Missing | absent | No SetThreadExecutionState |
| On-screen HUD | DM_METAL_HUD | — | none | Missing | absent | |
| Optimus / AMD PowerXpress exports | — | `entry.c` (6aace78) | none | Missing | absent | Cheap |
| Startup splash | — | e24b2a9 (off unless SWR_STARTUP_SPLASH) | none | N/A | absent | Parity: Turbo disables it |

### G. ini and command-line options

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| ini written with comments, sorted, live reload, Spanish aliases | `settings.py` | `main.c ini_*` | `config/lib.rs`, `options.rs` | Done | 5A.3 Partial | |
| `settings_version` 2 / 3 migrations | — | 01f9119, d5b36a1 | `config/lib.rs:252` | Done | 5A.1 Done | |
| `--key=value` for one run | — | e4b388d | `config::apply_arg` | Done | 5A.1 Done | |
| `-v` (log=2) | — | `main.c:449` | none | Missing | absent | `--log=2` works |
| `docked`, `language`, `fullscreen`, `button_*`, `mouse_*` | `settings.tsv` | `main.c` | readers in os / front | Done | 5A.3 | |
| `keyboard_text` | — | `svc_ns2` | `swkbd.rs` (`NXOS_SWKBD=auto` or headless only) | Partial | 5A.2 | |
| `renderer` (opengl / vulkan / eden) | — | `backend_ini` | `options.rs:201` forces vulkan | Stub | absent | The launcher still offers OpenGL for nxrs games |
| `resolution`, `dynamic_resolution(_min)`, `game_dynamic_resolution`, `shadows` | — | `main.c`, 81a1d95 | `vk/scale.rs`, `vk/counters.rs:34`, `exec.rs:1816` | Done | 5A.3 "unread" (stale) | |
| `fps_target` (30 / 20 frame skip, 120 = LSFG x2), `fps_limit`, `forced_vsync` | — | `main.c:193` | `config::frame_skip`, `present.rs:56-156` | Done | 5A.3 "unread" (stale) | |
| `present_interval` | — | — | `nvn/window.rs:207` | Better | 5A.6 | |
| `null_trap`, `memory_gb`, `log`, `profile` | — | `main.c` | `process.rs:91-117`, `rt.rs:40`, `runtime/profile.rs` | Done | 5A.3 "unread" (stale) | |
| `driver_exclusive`, `shader_thread`, `low_priority_shader_compiler` | — | `gpu_gl` | `vk/pipe.rs:378,861`, `graphics/lib.rs:37` | Done | 5A.3 "unread" (stale) | |
| `lsfg`, `lsfg_target`, `lsfg_multiplier`, `lsfg_flow`, `lsfg_dll`, `frame_gen_type` | — | `lsfg.c` (GL only) | `front/lsfg.rs`, `vk/framegen.rs`, `vk/lsfg.rs` | Better | 5A.3 Partial | |
| `lsfg_resolution` | — | `gpu_gl.c:3434` | `front/lsfg.rs:277` logged only | Stub | absent | |
| `lossless_scaling`, `lossless_path` | — | `ls_launch` | `front/lsfg.rs launch_app` | Done | 5A.3 | |
| `audio_slowdown`, `smooth_recording`, `recording_fps` | — | `main.c` | `sink.rs`, `clock.rs`, `rec.rs` | Done | 5A.3 | |
| `diag` (render-target dump at 20 / 40 s, thread dump) | — | `main.c` | `window::diag_dump` (gaps and pads) | Partial | 5A.3 "applied" | |
| `metrics` | — | `kernel.c` | `front/metrics.rs` | Partial | 5A.3 | See F |
| Hidden keys `seconds`, `shot` | — | `backend_ini` | `front_loop`, `NXRS_SECONDS` | Done | absent | |
| Hidden keys `script`, `heap_max_mb`, `loop_limit`, `show_game_rt`, `shader_process` | — | `main.c:203-214`, `backend_win32.c:67` | `shader_process` normalised but unread; the others absent | Missing | absent | |
| nxnative player settings (rumble, join, audio device, …) via `settings.tsv` | `settings.py`, `settings.tsv` | — | `profiles::player_settings()` has no caller | Stub | 4.6 PORT | nxrs uses Turbo's ini instead |

### H. Graphics: NVN layer and Vulkan replay

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| NVN proc table | `hle_nvn.c` (540 implemented) | `hle_nvn_registry.gen.c` (746: 457 real, 289 stubs) | `nvn/entry.rs procs()` (692) + 8 native-image procs | Better | G-lane Partial | 231 procs that Turbo stubs are real here |
| Ray tracing / acceleration-structure procs (7) and `nvnProgramGetShaderGroupHandle` | `nvn_accel.*`, `nvn_raytrace*` | stubs | none | N/A | 4.3 DEFER | No owner game. The shader side is done |
| `nvnCommandBufferSetConservativeRasterEnable` | `hle_nvn.c:3822` no-op + log | logged | none: the lookup returns 0, so a call jumps to null | Missing | absent | Add it as a logged no-op |
| `nvnCommandBufferSetShaderScratchMemory` | — | no-op | none | Missing | absent | |
| `nvnMemoryPoolBuilderSetNativeHandleNVX`, `nvnSyncInitializeFromNvRmSyncNVX` | — | real | none | Missing | absent | Only if the SDK's own native-image init runs |
| RegisterFastClearColor / Colori / Colorui / Depth return 1 | `hle_nvn.c:3278` | — | `nops.rs:11-14` returns nothing (x0 happens to read true) | Partial | absent | Bug by accident |
| Debug, ZCull, fast-clear and tiled-cache no-ops | `hle_nvn.c` | `hle_nvn.c` | `nops.rs`, `cmd2.rs:274-300` | Done | absent | |
| SDK major → NVN API version; NVN2 device answers from the SDK | — | 2f8f44e | `os/nvn/mod.rs:164`, `nvn/device.rs` | Done | 5A.2 Done | |
| Native image (video picture as a texture) | — | NVX procs | `nvn/native.rs` | Done | 5A.2 | Different design |
| NVN call census | — | 7b5a28c `nvn_hook.c` | os stub counts | Partial | absent | |
| Pool placement: mirror / VRAM / host | — | `vk_mem.c` | `vk/mem.rs placement_for` | Done | G-lane | |
| Dirty pages via write watch | `nvn_dirty.c` | `nvn_dirty.c` | `vk/mem.rs` | Done | G-lane | |
| Write-back landing | — | 5758e24 | `vk/queue.rs:461` | Done | 5A.4 | |
| Write-back on demand (lazy, on by default) | — | 6e86483 | `vk/lazy.rs` | Done | 5A.4 Partial | Violet `v12`, Z-A `z8` / `z9`. Only DKB is unrun |
| Joined write-back ranges | — | ad91c4d | `mem::note_gpu_write` | Done | 5A.4 Partial | |
| Raw global memory: 8,192-entry table, write log in device memory | — | 78ece7b, 1f12977, 4bb959d | `vk/gmem.rs` | Partial | 5A.4 Partial | No game reaches those shaders |
| Virtual pools | `hle_nvn.c` | — | `nvn/vpool.rs` | Done | 5A.4 | |
| Sparse images | — | `vk_sparse.inc` | `vk/sparse.rs` | Done | 5A.4 Partial | MKW 31 sparse images, peak 8.4-8.9 GB |
| Pools backed by sparse granules on first use | — | `vk_dev.c:271` | whole buffers | Missing | absent | |
| Pool windows over maxBufferSize (AMD 2 GB) | — | `vk_mem.c:1192` | none | Missing | absent | Off-NVIDIA only |
| Per-frame upload budget, staging ring | — | `vk_mem.c:1178` | none | Missing | absent | Minor |
| VRAM budget eviction, freeing superseded render targets | — | `vk_tex.c:570`, `vk_dev.c vk_budget` (on by default) | parking only | Missing | absent | Relevant to Jamboree's 11.6 GB of textures and Smash's memory growth |
| Linear render targets read back to the CPU | — | `vk_replay.c:2340` (on) | none | Missing | absent | Turbo needed it for MKW's crowds |
| System-memory cap | — | `NXRT_VK_HOST_MB` | `NXGFX_HOST_MB` (`vk/mod.rs:199`) | Done | absent | |
| Block-linear deswizzle; Tegra X1 vs Switch 2 GOB order from the profile | `nvn_tex.c`, `nvn_deswizzle.c` | b3bae4d, `s2tex` | `tex/deswizzle.rs`, `os/nvn/mod.rs:213` | Done | absent | |
| GPU deswizzle | `nvn_deswizzle_gpu.*` | — | CPU only | Missing | 4.3 REWRITE (PORT_MAP says Done) | |
| Format table | `nvn_fmt_mtl.c` | `vk_fmt.c` | `tex/format.rs` | Done | G-lane | |
| ASTC decode → BC7 on the GPU | Apple native | `vk_astc.cpp` (CPU astcenc → bc7enc, 32 GB disk pack, prefetch) | `tex/astc.rs`, `astc_decode.comp`, `bc7_encode.comp` | Better | 3.5 | Mode 6 only (possible banding); no disk cache |
| One image per storage: sRGB twin, shared aliased images | — | `vk_tex.c:97` | `vk/image.rs alias_of` | Done | G-lane | |
| CPU texture rewrites reload, all titles | — | 362b172 (two titles) | `image_begin_submit`, WriteTexels | Better | 5A.4 Partial | |
| Storage use for images without the IMAGE flag | — | caa07ad, now on by default (`vk_tex.c:951`) | IMAGE flag only (`vk/image.rs:421`) | Missing | 5A.4 Not started | The plan says "(off by default)"; Turbo now defaults it on |
| Cube views of square 2D arrays | — | 78ece7b | `vk/image.rs:432` | Done | absent | |
| Bindless samplers and texture pools | `nvn_mtl.m` | `vk_tex.c` | `vk/tables.rs`, `layout.rs` | Done | G-lane | |
| ReadTexels / WriteTexels | `nvn_texio` | `nvn_texio.c` | `nvn/tex.rs` | Done | 5A.4 | |
| Pipeline keys, dynamic state | `nvn_pso_*.h` | `vk_pipe.c` | `vk/pipe.rs` | Done | G-lane | |
| Graphics pipeline libraries | — | off by default | on (`NXGFX_GPL`) | Better | G-lane | |
| Descriptor buffers | — | — | on (`NXGFX_DESCBUF`) | Better | G-lane | |
| Pipeline cache on disk | — | `vk_pcache.c` | `vk/pcache.rs` | Done | 5A.4 Done | |
| Prewarm from earlier runs | — | `vk_pipe.c:588` | `vk/pipe.rs:894` (`pipeline-keys.bin`) | Done | 5A.4 | |
| Prewarm from the profile's pipeline manifest on a first run | `pso_offline.py`, `pso_manifest.tsv` | 6d04b3b seed list | `cli/pipeline.rs:308` writes the tables; `profiles::prewarm_shaders` has no caller | Stub | 4.4 REWRITE (PORT_MAP says Done) | Wonder's first-entry stutter, DKB |
| Live translation: a draw waits for its shader | — | 336dcb7 | `vk/live.rs` | Done | 5A.4 Done | |
| Cap on a frame's total shader waits (1.5 s) | — | 283c6b6 | per-set 3 s cap only | Missing | 5A.4 Done | |
| Clip / cull distance trim | — | `vkspv_trim_cull` | `shaders::link::trim_cull` | Done | S-lane Open (stale) | |
| Vertex input retype | — | `vkspv_retype_inputs` | `vk/shader.rs InKind` | Done | absent | |
| Dual-source blending (SPIR-V edit) | `dmsh_*` | `vk_shader.c:611 vkspv_dual_source` | blend factors only (`exec.rs:1021`) | Missing | absent | Lead for Jamboree's white layer |
| Integer output variants | — | 3d77358, `vkspv_int_outputs` (`:625`) | none | Missing | 5A.4 Not started | |
| Alpha test | `dmsh_alpha.py` | `vk_shader.c:747 vkspv_alpha_test` | `exec.rs:2073`: recorded, red line, not applied | Stub | absent | Hit in Smash (`-smash-match5`) |
| Subgroup ballot made per lane (RTX 50 hang) | — | `vk_shader.c:199`, on by default | none | Missing | absent | |
| Uniform index clamp | — | `vk_shader.c:367`, off | none | Missing | absent | Off in Turbo too |
| SPIR-V override directory, invariant strip, highlight (debug switches) | — | `vk_shader.c` | none | Missing | absent | |
| Late vertex outputs; link_io; per-primitive variants | — | — | `vk/spvfix.rs`, `pipe.rs:735`, `shader.rs:477` | Better | G-lane | DKB driver crash |
| Transform feedback | `nvn_feedback.*` | 78ece7b | `vk/xfb.rs` | Partial | 5A.4 Partial | Unit-tested; no game captures yet |
| Layered passes | — | 78ece7b | `vk/exec.rs` | Done | 5A.4 Partial | |
| Push constants as bank 24, NVN2 banks 25-29 | — | 9ce9b8e | `vk/exec.rs` | Done | 5A.4 Partial | |
| Append counters as indirect arguments; size-0 storage buffers | — | 7b5a28c | `exec.rs:332-419,1520` | Done | 5A.4 Partial | |
| Buffer textures (texel buffer views) | — | 8097152 | `vk/exec.rs` | Partial | 5A.4 Partial | Not exercised |
| Counters with the GPU's own time | — | 8097152 | `vk/counters.rs` | Partial | 5A.4 Partial | No idle-gap exclusion (283c6b6) |
| Timestamp record counter field written as 0 | — | `vk_replay.c:107`, `vk_submit.c:94` | `vk/queue.rs:605` writes t | Partial | absent | Breaks agl's "already cleared" check |
| Conditional render (addr and addr+16; reports pending in the same submission) | `nvn_mtl.m` | `vk_replay.c:2235-2390` | `exec.rs:2956` reads addr and addr+8, memory only | Partial | absent | Likely bug |
| Occlusion samples | — | `vk_replay.c` | `vk/counters.rs` | Done | 5A.4 | |
| Game's own dynamic resolution | — | 81a1d95 | `counters::drs()` | Done | 5A.4 | |
| Mesh draws | — | 9f19968 | `exec.rs:610 mesh_draw` | Partial | 5A.4 / 5A.5 | Bound in replay (5A.5's "graphics must bind" is stale); not reached in a game |
| Event waits park a queue; parked submissions | — | 362b172, 1316538 | `vk/queue.rs` | Done | 5A.4 Partial | |
| Async replay thread (on by default) | — | 5758e24 | `vk/queue.rs` | Done | 5A.4 Partial | Z-A `z6` |
| Recording one submission on several threads | `nvn_rec.h` | `vk_defer.c` (off) | none | Missing | 5A.4 Partial | |
| Live alpha-to-coverage switch, read once | — | 0b0d902, b89e9e5 | `lib.rs:59`, `exec.rs:1793` | Done | 5A.4 Partial | |
| Present ticket race | — | a22b102 | receipts (`nvn/window.rs:69`) | N/A | 5A.4 Partial | Impossible by design. The plan's reason ("no translation thread") is stale |
| Tessellation, geometry | `nvn_tess.inc`, `nvn_geometry.*` | — | native Vulkan stages | Done | 4.3 | |
| Quads | `nvn_primitive.h` | — | `exec.rs:2171` | Done | 4.3 | |
| Border colours, depth clear, wide lines | `nvn_border.h` etc. | — | Vulkan native | N/A | 4.3 DROP | |
| Built-in counter kernels | `nvn_builtin.h` | — | none found | Missing | 4.3 REWRITE (PORT_MAP says Done) | Possibly covered by loose control sections; not shown |
| DLSS evaluate | — | stub | `exec.rs:3046`: filtered upscale | Better | absent | Not DLSS (it says so in its log) |
| Device message / command buffers | — | stub | `nvn/devbuf.rs` | Better | absent | |
| Begin / End / SubmitDeviceCommands, DirtyTileMap, cooperative vectors, exclusive scissor and similar | — | stubs | absent (58 names) | N/A | absent | Parity (Turbo stubs) |
| Capture and replay without the game | `nvn_capture.c/.h/.inc` | `gfx/cap` design | `vk/shot.rs`, passdump only | Missing | 4.3 PORT | For offline debugging of the colour bands, cyan ground and white layer |
| Frame step (`fstep`), present_itv tick | `nvn_fstep.h`, `hle_nvn` | — | none | Missing | 4.3 PORT | OoT, MKW |
| Pacing | `nvn_pace.h` | `vk_present` | `nvn/window.rs Pacer` | Done | 4.3 | |
| Display-link drain / latch | `nvn_link_*.h` | — | — | N/A | 4.3 PORT (should be DROP) | Apple display link |
| Swapchain, letterbox | — | `vk_present.c` | `vk/present.rs` | Done | G-lane | |
| Sharp (contrast-adaptive) upscale at present | — | `vk_present.c:230,333` (default) | linear blit (`present.rs:343`) | Missing | absent | |
| Resolution scale, host dynamic resolution, `shadows=0` | — | GL only (`gpu_gl.c`, `render_scale.h`) | `vk/scale.rs` | Better | 5A.4 Partial | MK8D at 2x (`m3`); host dynamic resolution has no game run |
| fps_limit, frame skip, forced_vsync | — | GL only | `present.rs:56-85` | Better | 5A.4 Partial | |
| Forced present interval (profile + ini) | — | — | `nvn/window.rs:194`, wired from `os/nvn/mod.rs:206` | Better | 5A.6 Partial ("not wired") | Wired: stale |
| Screenshot, perf lines, render sizes, frame CSV | — | `vk_shot.c`, `vk_perf.c`, 8097152 | `vk/shot.rs`, `vk/perf.rs` | Done | 5A.4 Done | |
| Pass / texture dumps | `nvn_capture` | `vk_replay.c` | `exec.rs:425-563` | Done | absent | No raw / ID mode |
| Flight recorder | — | `vk_flight.c` | none | Missing | absent | |
| Aftermath, checkpoints, device-lost report | — | `vk_aftermath.c`, `vk_dev.c:184`, `vk_submit.c:85,273` | panic at `queue.rs:445,514`; red line at 5 s (`:569`) | Missing | absent | Smash device-lost; MK8D `r9` lost device undiagnosed |
| RenderDoc link | — | `vk_ctl.c:64` | none | Missing | absent | |
| Memory report, pipeline statistics | — | `vk_dev.c:285`, `vk_pipe.c:415` | none | Missing | absent | |
| Validation layer, GPU choice | — | `gfx_vk.c` | `vk/device.rs` | Done | absent | |
| LSFG passes from the player's Lossless.dll | — | `lsfg.c`, `lsfg_spvc.cpp` (GL) | `vk/lsfg.rs` | Better | 5A.3 Partial / 5A.6 In progress | MK8D 59.0 → 117.2 FPS (`r26`) |
| Built-in frame generation | — | — | `vk/framegen.rs` | Better | 5A.6 In progress | On by default. Garbles Arceus; hangs a Jamboree screenshot |
| OpenGL / Maxwell GPFIFO renderer (`renderer=opengl`, Turbo's default) | — | `gpu*.c`, `gpu_gl.c`, `glq*`, `shader.c`, `sm86.c` | none | N/A | absent | Not a goal under the Vulkan-only plan; owner decision needed because the option is silently remapped |
| Eden renderer | — | `edenvk` | none | N/A | 8 (GPL, never copied) | |
| Null back end | — | `gfx_null.c` | deleted | N/A | 5A.2 Done | Rule 1.1.6 |

### I. Shaders and shader tools

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| Shaders translated at build | `shader_split.py`, `dmsh_build.py` | 6d04b3b | `shaders/prebuild.rs` | Better | S-lane Done | 184k shaders over 7 games, 0 failed |
| Container readers (19 in `shader_split`) | `shader_split.py` | — | `shaders/formats/*` (18 + many more) | Better | 5A.5 Done | TT Games `ttdat` (new in nxnative 7ebb76e) is missing; no owner game uses it |
| dmsh build, reflect, entry, side, vin, abi, facts, fetch | `gfx/tools/dmsh_*` | — | `job.rs`, `reflect.rs`, `nvn.rs` | Done | 4.4 PORT | |
| dmsh fixes (rewrite, alpha, atomic, logic, wide, ampere) | `dmsh_*.py` | — | `shaders/fixes.rs` | Partial | 4.4 | Alpha is not applied in replay (H) |
| Geometry, mesh and tess modules; real task / mesh | `dmsh_mesh.py`, `dmsh_tess.py` | 9f19968 | `shaders/mesh.rs` | Done | 5A.5 Done | spirv-val clean |
| Ray-tracing library | `dmsh_rt*.py` | — | `shaders/rt.rs` | Partial | 5A.5 Done | Synthetic only; no owner game |
| Translation facts tables (`shader_facts.tsv`, manual facts) | `gfx/tools` | — | written to `<data>\shaders\profile\`, read by nothing | Stub | absent | Link's Awakening and FF Resonance need the manual facts |
| The 3 translator failures (MKW SPH, dynamic offsets) | — | — | `job::is_ampere_sph`, `fixes::dynamic_offsets` | Done | 5A.5 Done | |
| Translator chain build (`dmsh_xlate` NativeAOT) | `dmsh_chain.py`, `tool_lock.py` | 4bb959d | `tools/build_dmsh_xlate.ps1`, `xlate.rs` | Done | 4.4 | |
| sm86 cross-check | — | `sm86.c` | `tools/crosscheck_sm86.py` | Done | 3.4 | |
| c1_guard, cb_extent, shader_regressions, passgpu_top, tex_census | `gfx/tools` | — | none | Missing | 4.4 PORT | |
| ampere_orin tests | `gfx/tools/ampere_orin` | — | none | Missing | 4.4 PORT | |

### J. Profiles and the patch machinery

| Feature | nxnative | Turbo | nxrs | Audit | Plan | Note |
|---|---|---|---|---|---|---|
| Profiles imported | `dm/games` (207 at 7ebb76e) | — | `profiles/games` (207; 20 uncommitted) | Done | M0 "187 of 192" | Wrong: the "5 missing" were 5 loose files. INTERFACES says 187 |
| `patches.py` exported to `patches.toml` | 29 | — | 29 | Done | M0 "27/27" | Stale count |
| Build-applied cuts (`apply = "build"`) | `patch_nso.py` | — | `profiles::translator_patches` → `aot/patches.rs` | Done | 5A.6 | Z-A, MKW |
| Exported cuts not applied (74 per profile, 3 shared) | `patch_nso.py` | — | data only | Missing | 4.5 PORT | Smash's address-region cuts are Apple-VA workarounds; the rest need review |
| MATCH rules (19 per game, 14 shared): fingerprint search | `nso_match.py` | — | parsed (`patches.rs:185`), never applied | Stub | 4.5 PORT | Arceus's capture-demo crash fix; OoT's 3 movie cuts |
| FRAME_STEP and STUBS entries, capabilities | `patch_nso.py:1474` | — | parsed and round-trip tested, never applied | Stub | 4.5 PORT | MKW, OoT |
| Observation gates | `patch_nso.py site_obs` | — | none | Missing | 4.5 PORT | |
| Trinity rendezvous fence | `patch_nso.py:889-917` | — | none: no detection, no `trinity` in any `.rs` | Missing | 4.5 "verified on Violet and Z-A" | Claimed but absent. Violet and Z-A run without it so far |
| Spin-wait gates | `patch_nso.py spin_*` | — | `aot/emit.rs rt_spin_gate`, `patches.rs::spins_for` | Done | 3.1 | |
| Store-release / load-acquire ordering (Unity 2018 engine patch) | `engine_patches.py` | — | `cpu.h stl` SEQ_CST | Better | absent | |
| game.tsv fields consumed: title, program, build, modules, mem_used, app_pool, sys_resource, mem_map_limit, present_interval | `game.py` | — | `profiles/lib.rs` + os / runtime | Done | M0 | |
| game.tsv fields not consumed: fps_target (43 profiles), fps_max, fps_hold, op_mode, perf_mode, core_preempt, alloc_big_align, game_env, launch_param, programs / program_of, tex_dyn_slots, shader_key_skip, draw_barriers, stdalloc_cap | `game.py` | — | parsed only | Missing | absent | Apple-only fields (mem_reserve, mem_alias, mem_stack, depth_mode, screen_aspect and others) are rightly unused |
| `profile_guess.py` (games without a profile) | `dm/games` | — | none | Missing | 4.6 PORT | Not needed by the owner's three unprofiled games |
| `duo_anchor.c` (iPhone Duo layout) | `dm/games` | — | — | N/A | absent | Should be a DROP row |
| fps_target / monolith_fps tools | `dm/tools` | — | none | Missing | 4.5 PORT | |
| `jump_solver` cases as translator tests | `test_jump_solver.py` | — | none | Partial | 4.5 / T5 | |
| check_thread_cuts, audio_channel_check, gpuprof, usage, proc_group | `dm/tools` | — | none | Missing | 4.5 PORT as needed | |
| docs_patches, docs_symptoms, env_switches, issue_areas, pr_areas | `dm/tools` | — | none | Missing | 4.5 PORT | |
| nvn_native_sizes, nvn_slots_got | `dm/tools` | — | size asserts in `nvn/obj.rs` | Partial | 4.5 PORT | |
| Fuzz corpus | `dm/tools/fuzz` | — | none | Missing | 4.5 PORT | |
| `docs/reference/*.md` (13 files), `env_switches.tsv`, `settings.md` | `docs/` | — | none | Missing | 4.6 DATA | |
| `save_sync.c` (folder sync, not only iCloud) | `save_sync.c` | — | — | Missing | 4.2 DROP "iCloud" | The DROP reason is wrong: `DM_SAVE_SYNC` also takes a plain folder |

## (a) Corrections to PORT_PLAN statuses

| Row | Plan says | Should say | Why |
|---|---|---|---|
| M0 | 187 of 192 profiles; 27/27 patches | 207 of 207 (20 uncommitted), 29/29; CI and PORT_MAP still open | Counted at nxnative 7ebb76e |
| M1 | Done | Done, but XCI as the game's package is unrun | See 5A.1 XCI |
| M9 | "Done for MK8D" | Partial | Not a defined status. Launcher CI does not package the engine; no `doctor` in the launcher; no save copy; stale keys path; save folders differ |
| 5A.1 every "Done (uncommitted, lane T)" | uncommitted | committed (8f627a6, 7360d85, c76a0f5) | Stale wording |
| 5A.1 XCI as the game's package | Done | Partial | Never built and run, against the section's own rule; the launcher refuses `.xci` |
| 5A.1 crypto seeds | "removing `bignum.rs` is left to lane O" | Done | `bignum.rs` is gone |
| 5A.1 every `switchrecomp.py` option | Done | Partial | Flat folders, folders without a RomFS, folder control data, `SWR_CFLAGS`, shader failure fatal; hints / `--interp-only` dropped without an owner decision |
| 5A.1 runtime: the 5 requests | Partial | Partial; add the SVC fake success | Unknown SVCs answer success; 0x30/0x31/0x37/0x6F/0x80 are stubs |
| 5A.2 services Turbo answers | "time shared page not served" | served | `time.rs:367-399` |
| 5A.2 video | VP9 not run; panel black | VP9 run (Shadow Generations 18 min); MK8D panel plays (`-gp3`) | Stale |
| 5A.2 renderer mix fixes | Partial | Done (superseded) | The in-process SDK renderer makes Turbo's host-mixer fixes unnecessary; the unrouted-voice check waits for OoT |
| 5A.3 metrics, counters, record / replay | Done | Partial | Record / replay is Done. The counter registry has no producers; frames CSV is always empty; Turbo's guest-CPU, thread and wait columns are absent |
| 5A.3 control pipe | Done | Partial | About 20 Turbo `vk_ctl` commands and nxnative's memdump / cvar / drag are missing; `set` / `get` mean something else |
| 5A.3 hotkeys | Done | Partial | F3 and F7 use window grabs (F7 audio is post-slowdown); F5's dump has no thread states |
| 5A.3 the ini | Partial: "still unread: resolution …" | Partial: only `lsfg_resolution` is unused, `diag` is thin, `renderer` is forced, and hidden keys and `-v` are missing | Every listed option is read now |
| 5A.3 window | Done | Done; add Optimus / AMD exports and focus-to-game as gaps | |
| 5A.4 live a2c, texture rewrites, ticket race | Partial | Done | All three addressed; the race is impossible by receipts |
| 5A.4 storage use without IMAGE | Not started "(off by default)" | Not started; Turbo now defaults it on | `vk_tex.c:951` |
| 5A.4 resolution scale … | Partial | Done; only host dynamic resolution lacks a game run | |
| 5A.4 missing shader waits | Done | Partial | No per-frame wait cap (283c6b6) |
| 5A.4 buffer textures / counters | Partial | Partial; name the open items | Idle-gap exclusion, timestamp counter field, render-condition offset |
| 5A.4 write-back on demand | Partial | Done | On by default, evidenced on Violet and Z-A; only DKB (the case it is for) unrun |
| 5A.5 mesh | "graphics must bind mesh_draw" | Done in replay; game evidence missing | `exec.rs:610-680` |
| 5A.6 forced present interval | Partial: "profile not wired" | Partial: wired (`os/nvn/mod.rs:206`); no 30 FPS game run | |
| 5A.6 mods | Done "(uncommitted)" | Done, committed | 7360d85, cb4b426 |
| S-lane | Open: clip / cull distances | Trimming is done at load | `shaders::link::trim_cull` |
| 4.2 `hle_oodle.c`, `hle_glslc.c` | DEFER | Done | Shaders crate; `nvdrv.rs` capture |
| 4.2 `save_sync.c` | DROP "iCloud" | Reconsider | Folder sync is platform-neutral |
| 4.3 `nvn_link_drain.h`, `nvn_link_latch.h` | PORT | DROP | Apple display link |
| 4.5 Trinity rendezvous fence | "verified on Violet and Z-A in M6" | Not started | No code exists |
| PORT_MAP rows | Done for `pso_offline.py`, `nvn_deswizzle_gpu`, `nvn_rt.h`, `nvn_builtin.h`, `hle_nvn.c`, `game.py`, `settings.py` | Not started / Partial | No implementation, or no consumer |
| 6.1 output layout | `nxrt.dll`, `nx-run`, `discovered.tsv`; `--native` / `--chunk` ignored | 11 KB loader + runtime static in `_code.dll`; no `discovered.tsv`; `--native` / `--chunk` implemented | Stale |
| 6.2 | `engine.json` beside the engine (from the engine) | written by the launcher's packager | |
| 6.3 | "Task: confirm Turbo's save location" | Answered: Turbo `<data>\save\user`, nxrs `save\user<N>`; a migration is needed | |

## (b) Gaps

Each gap is added to PORT_PLAN 5A as a Not started row with its reference location. ★ marks gaps an owner game needs.

**5A.1 Package, build, runtime**

1. ★ DLC whose data is missing must not be offered (`runtime/dlc.c dlc_has_data`, 2cdeabd).
2. Folder input: flat folders, optional RomFS, control data (`switchrecomp.py:497-501`, `nxloader.py:431`).
3. ★ Turbo save compatibility: `save\user` → `save\user<N>` migration (`svc_fs.c:511`).
4. ★ Keys fallback path `%LOCALAPPDATA%\nxrs` (launcher d538b31).
5. Engine CI (a9a4add `.github/workflows/engine-ci.yml`); PORT_MAP check as a test.
6. Shader prebuild parallel and non-fatal (6d04b3b).
7. `SWR_CFLAGS`, reserved Windows names, `--jobs` default (`switchrecomp.py:440,768`).
8. Execution hints / interpreter fallback: owner decision (`switchrecomp.py:583`, `main.c:38-50`).
9. Host sampling profiler (`tools/host_prof.py`, SWR_HOST_PROF).
10. Unknown SVCs answer NOTIMPL / NOTFOUND as Turbo does. Real 0x30/0x31/0x37/0x6F/0x80 and 0x02/0x03 state (`kernel.c:1013,1116-1130`).
11. Stuck-mutex healer (`kernel.c:410`).
12. ★ nxnative patch machinery: MATCH rules (`nso_match.py`), FRAME_STEP / STUBS, observation gates, Trinity rendezvous fence (`patch_nso.py:889-917,1474`). Arceus, OoT, MKW, Violet, Z-A.
13. ★ Profile game.tsv fields at run time: fps_target / fps_max / fps_hold, op_mode, perf_mode, alloc_big_align, game_env, launch_param, tex_dyn_slots, shader_key_skip, draw_barriers, stdalloc_cap (`dm/games/game.py`).
14. Title version logged at boot (13a2693).
15. OoT full-resolution built-in patch equivalence (`nxloader.py:56`).
16. Engine release packaging passed in the launcher's `release.yml` (`--nxrs-engine`).

**5A.2 Services, audio, video**

17. ★ Focus change (message 15) and applet focus enter / leave (`hle_oe.c dm_focus_set`).
18. RequestToRelaunchApplication restarts program 0 (`hle_oe.c`).
19. ★ InlineKeyboard's 10 missing methods (`hle_swkbd.c`).
20. ★ Per-slot shared fonts, CJK and the button-icon font (`make_shared_fonts.py`, `make_ext_font.py`, Turbo `main.c:676-707`).
21. psm, set:sys, set 7/8/9/11, fatal:u, spl:, mm:u (`svc_core.c`).
22. bcat empty cache; aoc 4-13 / 50 / 100 / 101; fsp 18 / 60-62 / 400 / 500 (`svc_core.c`, `svc_fs.c`).
23. vi `ListFrameHistory`, IHOSBinderDriver, 2450/2451 (`hle_vi.c`, `svc_vi.c`).
24. Missing SDK names: H.264 init overload, `Extractor::SetDataSource`, ssl certificates, PlayReport::Send (HLE_FORCE_DIRECT), ngc China, album URL, Palma × 20.
25. IPC audren / aud / audout / audin for SDK 1.x (`svc_core.c:740-898`, `hle_audren_sv.c`).
26. Audio RMS line; device choice, mute, WAV dump (`audio.c:149`, DM_AUDIO_*).
27. ★ Host movie / NDVE / NDSR network (OoT movies, agl AA) (`nvn_ndve.inc`, `vk_replay.c:1918`).
28. Unknown Switch 2 services catch-all (`svc_ns2.c`).
29. ★ Engine frame lock (`hle_fpslock.c`), and resolving the `fps_target` name clash.
30. Cheats: Atmosphère loader, dmnt VM, panel (`hle_cheats.c`).
31. MiiEdit answers "cancelled" as Turbo does (9ae9441); ShowUserCreator.

**5A.3 Input, front end, harness**

32. ★ Touch screen from the mouse, gestures, scripted touch (`hle_touch.c`).
33. ★ Controller-support prompt that waits for joins (`ctrl_prompt.h`).
34. Home held → quit prompt; user-select dialog (`home_quit.h`, `user_select_mac.h`).
35. Rumble setting / gain / cutoff, press to join, pad merge, vibration-device init (settings.md, `hle_hid.c`).
36. F3 / F7 from the game frame; F7 audio before slowdown; recording watchdog (`backend_win32.c`, `rec_flow.h`, `rec_audio_time.h`).
37. F5 / `diag`: real thread dump and render targets; threads.txt every 15 s; stall dump at 6 s; driver-crash fatal (`backend_win32.c:461`, `main.c:512-524`).
38. Metrics: counter producers (waits by kind, dropped work), guest CPU per thread, frames CSV fed (`kernel.c:925-985`).
39. Control-pipe commands from `vk_ctl.c:93-159` and nxnative DM_CTL (memdump, cvar, drag, texlist).
40. `lsfg_resolution`, hidden ini keys (`script`, `heap_max_mb`, `loop_limit`, `show_game_rt`, `shader_process`), `-v` (`main.c:203-214,449`, `gpu_gl.c:3434`).
41. Optimus / AMD PowerXpress exports (`entry.c`); keep the display awake; screen refresh match.
42. Graceful exit order and console control handler (`hle_lifecycle.c`).
43. Minidump on crash; probe breakpoints (`crash.c`, `probe.c`).
44. `renderer` option: owner decision on OpenGL / Eden; stop silently remapping it (`config/options.rs:201`).

**5A.4 Graphics**

45. ★ Alpha test applied (`vk_shader.c:747`; nxnative `dmsh_alpha.py`). Smash.
46. ★ Dual-source blending SPIR-V edit (`vk_shader.c:611`). Jamboree lead.
47. Subgroup ballot per lane, RTX 50 hang (`vk_shader.c:199`).
48. ★ Prewarm from the profile's pipeline manifest on a first run; read `shader_facts` (`pso_offline.py`, 6d04b3b).
49. Linear render-target readback (`vk_replay.c:2340`). MKW.
50. VRAM budget eviction, superseded render targets (`vk_tex.c:570`, `vk_dev.c`). Jamboree, Smash.
51. Sharp present upscale (`vk_present.c:230,333`).
52. Render-condition semantics, timestamp counter field 0, idle-gap exclusion, per-frame shader-wait cap (`vk_replay.c:107,2235-2390`, 283c6b6).
53. Missing NVN procs: SetConservativeRasterEnable, SetShaderScratchMemory, two NVX procs; FastClear return value.
54. Flight recorder, Aftermath, checkpoints, device-lost report, RenderDoc, memory report, pipeline statistics (`vk_flight.c`, `vk_aftermath.c`, `vk_dev.c:184,285`, `vk_pipe.c:415`).
55. ★ Capture and replay (`nvn_capture.*`). Colour bands, cyan ground, white layer.
56. ★ Frame step (`nvn_fstep.h`). OoT, MKW.
57. Pool windows, sparse pool granules, upload budget (`vk_mem.c:1178,1192`, `vk_dev.c:271`).
58. ASTC BC7 quality (modes beyond 6) and disk cache (`vk_astc.cpp:180-330`).
59. GPU deswizzle (`nvn_deswizzle_gpu.*`); built-in counter kernels (`nvn_builtin.h`); real-time thread class (`nvn_rt.h`).

**Tools and docs (lane Q)**

60. nxnative `docs/reference` (13 files), `env_switches.tsv`, `settings.md`; tools: c1_guard, cb_extent, shader_regressions, tex_census, check_thread_cuts, gpuprof, fps_target, fuzz corpus, docs tools, bug_report, run_saves, ampere_orin tests, jump_solver cases.

## (c) Enhancements

**Where nxrs already exceeds the references:**

- Resolution scale, host dynamic resolution, shadows, fps limit and skip, recording and LSFG on Vulkan.
- Built-in frame generation.
- Pipeline libraries and descriptor buffers.
- Late vertex outputs.
- Texture rewrites in every title.
- GPU ASTC.
- 231 more real NVN procs than Turbo.
- Forced present interval.
- Run-time switchable mods.
- 40+ shader archive formats.
- The in-process SDK audio renderer.
- Real AudioIn.
- Motion.
- Nintendo and Switch 2 pads, 8 players.
- The virtual-core scheduler.
- Real ExecuteProgram and the Mii database.
- bsd event descriptors with loopback UDP.
- Vorbis and AAC movie audio.
- Unknown services refused honestly.
- Host-stack crash unwinding.
- Memory cap scaled with `memory_gb`.
- The heartbeat and exit-after watchdog.
- `press` / `pads` / `threads` / `counters` pipe commands.
- Static runtime (no new-code / old-runtime mismatch).
- AES-CTR DLC read in place.
- NRO input.

**Improvements worth making:**

1. **Make fake successes visible.** Unknown SVCs should answer NOTIMPL, as Turbo does. Add an optional strict IPC mode, in which unhandled commands fail with a result code instead of zeroed success, to surface hidden gaps per game.
2. **Wire the dead outputs:** profile prewarm, `shader_facts`, `player_settings`, `metrics::counter` producers, the frames CSV, and `home_held`. Each is half-built.
3. **One overlay** (egui or Win32) for the Home-quit, controller-join and user-select prompts and the cheats panel.
4. **Rename the ini's frame-skip key** so the profile's `fps_target` can mean the engine frame lock again.
5. **Port `nso_match` as a pure crate** with nxnative's tests. Apply MATCH, FRAME_STEP and STUBS sets as switchable patch sets, like mods.
6. **Turn frame generation off by default** until Arceus's garbling is fixed (lane W already runs with `lsfg=0`), or gate it per profile.
7. **Dump hostfn call counts to a TSV at exit** (the DM_HLECOUNT equivalent) to measure coverage per game.
8. **Launcher side:** accept `.xci`; hide OpenGL and `-O` for nxrs games; run `doctor` in the health check; pass `--nxrs-engine` in `release.yml`.
9. **Refresh the stale counts** in INTERFACES (187 profiles) and the profiles README (shader tables "not imported").
