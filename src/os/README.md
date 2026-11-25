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
| `svc.rs` | os's SVCs on the runtime's registry (`install`: `nxrt::svc::set_handler` / `set_fallback`): memory, IPC, GetInfo, GetSystemTick; the kernel's through `nxrt::kernel::svc`; a checkpoint after each; ExitThread / ExitProcess / Break to the runtime; an SVC nobody handles answers success with an `UNIMPL Svc` line. The counter: `rt_tick_hz` from the profile, `rt_ticks` on the guest clock (`nxrt::rt::set_clock`) |
| `mem.rs` | lazily committed heap (first touch, by the guest or `prepare`), MapMemory / UnmapMemory, Map/UnmapPhysicalMemory in the alias region, shared memory views, QueryMemory over the runtime's regions, the memory GetInfo ids from the profile |
| `sched.rs` | virtual cores as the runtime kernel's `Scheduler` (thread start / exit, blocking enter / leave, priority and core mask, the core number, spin passes): a guest thread holds one of the title's cores while it runs guest code; handoffs at SVCs and spin waits, steals after 2 / 20 ms |
| `hle.rs` | takeovers by mangled name (`force`, `hle`, `alias`), logging stubs for imports nothing provides, the loaded modules' symbols |
| `ipc/` | HIPC / CMIF messages (`hipc.rs`), host sessions, domains, control requests, `sm:`, services by name |
| `nvn/` | `nvnBootstrapLoader`, the proc table (the graphics layer's procs, a logging stub for a name it lacks; no null device, rule 1.1.6), the hand-off to graphics (`bind_graphics`, the SDK major), the nv:: glue (`nv.rs`) |
| `media/` | the sound card (`sink.rs`: WASAPI through cpal, every stream mixed, smooth slowdown, limiter, statistics, the recording tap), the capture device (`mic.rs`), libopus (`opus.rs`), libvpx VP9 (`vpx.rs`), dav1d AV1 (`av1.rs`), Media Foundation H.264 and AAC (`mf.rs`), pictures and their NV12 / RGBA layouts (`yuv.rs`), the MinGW CRT pieces the codec archives need (`crt.rs`); the archives in `third_party/` |
| `clock.rs` | the guest clock and its rate (`set_rate`, for smooth recordings) |
| `options.rs` | the settings os reads (`docked`, `language`, `keyboard_text`, `audio_slowdown`) through `nxconfig`, applied live |
| `kobj.rs` | kernel events the host owns |
| `profile.rs` | per-title facts (program id, pool, used memory, system resource, counter rate, cores) |
| `input/` | the controllers (lane I): XInput, DualSense / DualSense Edge / DualShock 4 over raw HID (lights, rumble, motion; the owner's `controller_sony*`), Switch Pro / Joy-Con / Online pads over raw HID (handshake, SPI stick calibration, IMU, HD rumble, player lights), WinMM fallback, hot-plug, player slots (nxnative's `pad_slots.h`), `button_layout` / `button_map` (the owner's `pad_layout.h`), motion fusion, scripted input (`NXRS_KEYS`, the control pipe's `press`), record / replay (`NXRS_INPUT_RECORD` / `NXRS_INPUT_REPLAY`); `bin/nxinput.rs` lists, reads and rumbles them without a game |
| `log.rs`, `watch.rs` | `[os]` log, the gap list (`UNIMPL` lines, summary at the first submit, at an abort and at the end), heartbeat and timed exits; a stall (no present for 15 s) logs every guest thread (`nxrt::kernel::dump_threads`: what it waits for, the host function it is in, its guest frames), the control command `threads` prints the same |
| `mods.rs` | the player's mods at run time (`<data>\mods.tsv` from the build): on / off by `NXRS_MOD_<NAME>` or a `disabled` file in the mod's folder, data bytes written into the modules before the game starts, RomFS files layered over the game's (fs) |
| `bin/nxos_dev.rs` | build glue: compile the generated C (the translator's `cpu.h`), link with `nxos.lib` |

## Services

| Module | Implemented | Stubbed / offline |
|---|---|---|
| `sdkinit` | nninitInitializeSdkModule's connections (no-ops) | |
| `sf` | nn::sf::hipc client takeovers onto host sessions (DuplicateSessionEx too); `olsc:u`, `caps:*`, `notif:a`, `mnpp:app` answered; the Switch 2 camera context as a session with no camera plugged in (open / close succeed, its event never fires, the rest 0x7c4) | |
| `fs` | RomFS in place (update applied) through `translator::package`, another program's RomFS by index (`romfs_programs/<n>.img`), add-on content (`dlc::title_romfs`), transactional saves (a Commit of a mount list when it is one save), temporary storage, cache storages (create with a size record, delete, list, size, limits from `control.nacp`), files, directories, IsPathMounted, RomFS working-memory sizes | system data other than the Mii model; a Commit list spanning two saves is refused |
| `account` | one local user (more by marker), sessions, nickname, profile image, the selector picks user 1 | online accounts fail |
| `oe` | operation mode, focus, notification queue and its event, performance configuration, language, region, display version, launch parameter, the switches | relaunch ends the run |
| `time` | the host's clocks, calendar conversions in the host's zone, clock snapshots, contexts, periodic benefit claims (the SDK's detail:: with the two service questions answered); `time:u` / `time:a` / `time:s` over IPC with the service's shared memory (steady clock, system clock contexts, automatic correction, ContinuousAdjustmentTimePoint; layout in `time.rs`), written into every view by a host thread | network clock check fails (offline) |
| `settings` | language codes and comparisons | |
| `vi` | display, layer, native window, crop, frame number, display list; vsync event at 60 Hz (`misc`) | |
| `hid` | the host's controllers (`input/`) as npads: player 1 Handheld or No1, players 2..8 on No2..No8, one-player games merging every pad into player 1, Joy-Con pairs as JoyDual or split JoyLeft / JoyRight (single assignment), style-set events, latched presses, full histories, the controller applet (player counts, players past the maximum left out), six-axis from live motion (DualSense / DS4 / Pro / Joy-Con), vibration to the pad (HD rumble on Nintendo pads), player LEDs, colours | touch, gestures, keyboard, mouse and debug pad read connected and idle |
| `hid_ext` | the Joy-Con 2 mouse sensor (Jcms ring fed by a Joy-Con 2's reports) and its pointer (Nmouse), the npad IMU the SDK's sensor fusion reads (live motion at 5 ms ticks), six-axis IC information, the fusion's calibration store, the console six-axis sensor, Palma (none connected), the private app's scene id, the two-argument touch / gesture initializers | no USB mouse as an Nmouse |
| `audren` | the SDK's own renderer in-process (CPU + manual): its DeviceSink in every SDK's command layout folded to stereo for the sound card, a render thread paced by the sound card (8 blocks of 5 ms in flight), the update lock read off the SDK, catch-up frames, 32 kHz upsampled | the audren:u service for SDK 1.x (no in-process renderer): not served |
| `audio` | AudioOut on the sound card (release events, six channels folded), SDK 21's OutputStream on the sound card (PCM16 / float, its event as blocks play, free / filled counts, purge, gain), AudioIn on the PC's capture device (none: the list is empty and an open fails, as audin answers), device names, volume, auto-tune and notifications | SDK 21's InputStream: no virtual input device listed, an open fails |
| `codec` | hwopus with libopus: the HardwareOpus(MultiStream)Decoder classes, their C interface, SDK 21's decoder manager, and the `hwopus` IPC service | |
| `movie` | movie::Decoder (NintendoSDK_movie): VP9 libvpx, H.264 / AAC Media Foundation, Vorbis lewton; NV12 to the client's buffer or RGBA into its NVN texture; MediaCodec's back-pressure; `NXOS_MOVIE_DUMP=<dir>` writes pictures handed out | VP8 (not in the libvpx build) fails with a red line |
| `codec_video` | the Switch 2's nn::codec video decoders (VP9, H.264, AV1 with dav1d frame threads): image pools, sync handles and nn::syncpt events, submission back-pressure (3603), Stop / Flush, native images for graphics, SDK 19 / 22 names | VP8; encrypted input; HDR read-back (black) |
| `swkbd` | ShowKeyboard and the InlineKeyboard: a text box over the game window (Hind font), `keyboard_text` by default (`NXOS_SWKBD=auto` answers the default without a box) | |
| `net` | `btm:u` (the interface, events that never fire), `bsd:*` on `socket`'s descriptor table | sockets past loopback: no network |
| `socket` | nn::socket / nnsocket* by name, offline: event descriptors (counter, semaphore and non-blocking flags), loopback UDP (bind, connect, ephemeral ports, SO_REUSE*, timeouts, a queue per socket), Poll / Select / libc poll, Fcntl, socket options, per-thread errno | name resolution and everything past loopback: ENETDOWN / EAI_FAIL |
| `offline` | streamplay (no guest: its getters keep what the game set, its event never fires), GameChat's camera storages (blank native images), the shop's purchase and friends notification events (never fire), play statistics (never played), IR camera handle, nifm's request (on hold for one query), nn::web offline pages (the stand-in of a run without a web view: one empty message, closed when it appears) | all offline |
| `ro` | ro::LookupSymbol over the loaded modules, NRO buffer sizes, NRR checks; jit::GetJitEnvironmentInfo | LoadModule fails (no NRO is loaded at run time); jit::Create / GenerateCode / Control / Destroy not served (no plugin loading) |
| `nvdrv` | NvOsDrvOpen fails (only the movie library's hardware decoder opens it); `NXOS_GLSLC_CAPTURE=<dir>` saves the game's run-time GLSL compiles | nvdrv: no device |
| `mii` | the six default Miis, random and default Miis, the editor adds the default Mii; the MiiModel system data stand-in | |
| `misc` | diag log lines and aborts into the log, add-on content list, the shared font (a stand-in TTF; SDK 21's built-in font data and font sets, ScalableFontEngine::LoadFont at the BFTTF header), amiibo (no tag), nfc (switched off), the profanity filter (nothing filtered), error and shop applets logged | |
| `os` | SDK SystemEvents bound and signalled by the host, entropy; with their settings, `game_env` answering getenv and `alloc_big_align` (64 KB alignment of big StandardAllocator blocks) | |
| `simple` | 296 offline stand-ins as nxnative answers them (friends, local wireless, telemetry, IR camera, SSL, parental controls, BCAT, album, name resolution, streamplay, GameChat, the shop's accessors, nvdrv's ioctls, …) | all |

## Build, test, run

    set CARGO_TARGET_DIR=target\os
    cargo test -p os                                   # unit tests, well under a second
    cargo build -p os                                  # nxos.lib and nxos-dev.exe
    nxrs translate <game.nsp> --out build\games\<g>\gen --data build\games\<g>\data --jobs 4
    target\os\debug\nxos-dev build --gen build\games\<g>\gen --out build\games\<g>\<g>.exe --jobs 4
    build\games\<g>\<g>.exe build\games\<g>\data

Switches: `NXOS_TRACE=svc,ipc,fs` (or `all`), `NXOS_EXIT_AFTER=<s>`, `NXOS_EXIT_AFTER_SUBMIT=<s>`,
`NXOS_HEARTBEAT=<s>`, `NXOS_OPMODE=0|1` (else `docked`), `NXOS_LANG` (else `language`), `NXOS_<SETTING>` (any setting os reads: `NXOS_GAME_ENV`, `NXOS_ALLOC_BIG_ALIGN`), `NXOS_MOVIE_DUMP=<dir>`, `NXOS_GLSLC_CAPTURE=<dir>`, `NXOS_SWKBD=auto`, `NXOS_NICK`, `NXOS_FIXED_TIME`, `NXOS_FONT`, `NXOS_SAVE`,
`NXOS_FREE_CORES=1` (no virtual cores), `NXOS_NVN_BT=<nvn proc>` (its first 64 calls with arguments and guest frames: where a game sets its present interval), `NXOS_HOST_CPUS=<n>`, the controller switches in `input/mod.rs` (`NXRS_CONTROLLER_TRACE`, `_SELFTEST`, `_BACKGROUND`, `NXRS_NO_SONY_HID`, ...); the runtime's `NXRT_TRACE` still works.
