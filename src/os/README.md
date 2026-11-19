# os (`nxos`)

The engine's OS services: what the Switch's system software does for a game, around the translated code and the
runtime. It is a Rust port of nxnative's `dm/host` HLE layer: the SDK's service clients are replaced by name, so
their IPC never runs, with an IPC framework and `sm` underneath for the clients a game links itself. Sources and
licenses: the "Credits and licenses" section of the repository README. The interface for other parts:
`docs/INTERFACES.md`, os.

## Layout

| File | Part |
|---|---|
| `boot.rs` | `nxos_main`: profile, address space, module load with the host's bindings, the main thread running nn::init's steps (`_init_libc0`, `nnosInitialize`, `nninitInitializeSdkModule`, the host arguments, `nninitStartup`, `_init_libc2`, every DT_INIT, `nnMain`) without rtld |
| `svc.rs` | `nxos_svc` (the generated code's SVC entry through `include/cpu.h`): memory, IPC, GetInfo, the counter at the title's rate (`nxos_ticks`); the rest to the runtime's `nxrt_svc_impl`; an SVC nobody handles answers success with an `UNIMPL Svc` line |
| `mem.rs` | lazily committed heap (first touch, by the guest or `prepare`), MapMemory / UnmapMemory, Map/UnmapPhysicalMemory in the alias region, shared memory views, QueryMemory over the runtime's regions, the memory GetInfo ids from the profile |
| `sched.rs` | virtual cores: a guest thread holds one of the title's cores while it runs guest code; handoffs at SVCs and spin waits, steals after 2 / 20 ms, the core number |
| `hle.rs` | takeovers by mangled name (`force`, `hle`, `alias`), logging stubs for imports nothing provides, the loaded modules' symbols |
| `ipc/` | HIPC / CMIF messages (`hipc.rs`), host sessions, domains, control requests, `sm:`, services by name |
| `nvn/` | `nvnBootstrapLoader` and the proc table (graphics layer, null device, logging stubs); the null device (`null.rs`) and texture storage sizes (`tex.rs`) |
| `kobj.rs` | kernel events the host owns |
| `profile.rs` | per-title facts (program id, pool, used memory, system resource, counter rate, cores) |
| `log.rs`, `watch.rs` | `[os]` log, the gap list (`UNIMPL` lines, summary at the first submit, at an abort and at the end), heartbeat and timed exits |
| `bin/nxos_dev.rs` | build glue: compile the generated C with the overlay, link with `nxos.lib` |

## Services

| Module | Implemented | Stubbed / offline |
|---|---|---|
| `sdkinit` | nninitInitializeSdkModule's connections (no-ops) | |
| `sf` | nn::sf::hipc client takeovers onto host sessions; `olsc:u`, `caps:*`, `notif:a`, `mnpp:app` answered | |
| `fs` | RomFS in place (update applied) through `translator::package`, add-on content (`dlc::title_romfs`), transactional saves, temporary and cache storage, files, directories, RomFS working-memory sizes | system data other than the Mii model |
| `account` | one local user (more by marker), sessions, nickname, profile image, the selector picks user 1 | online accounts fail |
| `oe` | operation mode, focus, notification queue and its event, performance configuration, language, region, display version, launch parameter, the switches | relaunch ends the run |
| `time` | the host's clocks, calendar conversions in the host's zone, clock snapshots, contexts | network clock check fails (offline) |
| `settings` | language codes and comparisons | |
| `vi` | display, layer, native window, crop, frame number, display list; vsync event at 60 Hz (`misc`) | |
| `hid` | one controller (Pro Controller docked, handheld pair handheld), states with full histories, style events, LEDs, colours, controller applet, six-axis at rest, vibration accepted | touch, gestures, keyboard, mouse, debug pad read empty |
| `audio` | the SDK's own renderer in-process (CPU + manual) with a 5 ms render thread; AudioOut accepting and releasing buffers; device names and volume | output is silent (no sink yet); AudioIn fails |
| `mii` | the six default Miis, random and default Miis, the editor adds the default Mii; the MiiModel system data stand-in | |
| `misc` | diag log lines and aborts into the log, add-on content list, the shared font (a stand-in TTF), amiibo (no tag), the profanity filter (nothing filtered), error and shop applets logged | |
| `os` | SDK SystemEvents bound and signalled by the host, entropy | |
| `bignum` | the SDK's untyped big-number routines the translator does not seed | |
| `simple` | 159 offline stand-ins as nxnative answers them (friends, local wireless, telemetry, IR camera, SSL, parental controls, BCAT, album, sockets, …) | all |
| `nvn` (null device) | device integers, pools, buffers, textures and their storage sizes, texture / sampler pools, programs, syncs, events, queues, command memory, windows, the first submit and present marks, 60 Hz presents | command recording and state objects: counted, nothing drawn |

## Build, test, run

    set CARGO_TARGET_DIR=target\os
    cargo test -p os                                   # unit tests, well under a second
    cargo build -p os                                  # nxos.lib and nxos-dev.exe
    nxrs translate <game.nsp> --out build\games\<g>\gen --data build\games\<g>\data --jobs 4
    target\os\debug\nxos-dev build --gen build\games\<g>\gen --out build\games\<g>\<g>.exe --jobs 4
    build\games\<g>\<g>.exe build\games\<g>\data

Switches: `NXOS_TRACE=svc,ipc,fs` (or `all`), `NXOS_EXIT_AFTER=<s>`, `NXOS_EXIT_AFTER_SUBMIT=<s>`,
`NXOS_HEARTBEAT=<s>`, `NXOS_OPMODE=0|1`, `NXOS_LANG`, `NXOS_NICK`, `NXOS_FIXED_TIME`, `NXOS_FONT`, `NXOS_SAVE`,
`NXOS_FREE_CORES=1` (no virtual cores), `NXOS_HOST_CPUS=<n>`; the runtime's `NXRT_TRACE` still works.
