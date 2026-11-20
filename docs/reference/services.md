# Services and SDK calls

The Switch's system software around the game, as the OS layer (`src/os/`) stands in for it. Two routes reach it:

- **Takeovers by name** (nxnative's HLE design): the SDK's service clients are replaced at their mangled names, so
  their IPC never runs (`src/os/src/hle.rs`, one module per area under `src/os/src/services/`).
- **IPC**: SDK clients that are not taken over talk HIPC / CMIF to host sessions behind `sm`
  (`src/os/src/ipc/mod.rs`, `src/os/src/ipc/hipc.rs`), through the SVCs or through the SDK's own HIPC entry points
  taken over by name (`src/os/src/services/sf.rs`).

Every service is offline and single-user by design; where the console would show UI, the engine shows host UI only
where it serves the player (error notices, the keyboard box, prompts).

## Binding rules

`nxos::hle::Registry` (`src/os/src/hle.rs`), collected by `nxos::services::register`:

| Call | Binds |
|---|---|
| `force(name, f)` | every reference to `name` in every module (GOT, GLOB_DAT, ABS64; the SDK's own calls go through its PLT), also where a module defines it: the runtime's `HOST_OVERRIDE` rule |
| `hle(name, f)` | only an import no module defines (C library and system pieces) |
| `alias(name, target)` | every reference to the modules' own definition of `target` |
| (left over) | a logging stub answering 0 and recording an `UNIMPL Import` gap |

Host functions are `unsafe fn(*mut Cpu)` at synthetic guest addresses (`src/runtime/src/hostfn.rs`); they read AAPCS64
arguments from the guest registers and stack, may call guest code only through `boundary::call_guest`, and must give
their virtual core up if they block (`sched::without_core`).

## Gaps and how they are reported

Every unanswered request is logged once with a tag and counted; the summary of all gaps prints at exit and on demand
(`src/os/src/log.rs`, `report_gaps`):

| Line | Meaning |
|---|---|
| `UNIMPL Import <name>` | an import nothing provides was called; answered 0 |
| `UNIMPL Service <name>` | sm refused a name nobody registered (ResultNotRegistered 0xE15). A Switch 2 program instead gets a catch-all session (`ipc::Unknown`: every command succeeds, command 0 hands out an object); `NXOS_NS2_STRICT=1` refuses as for Switch 1 |
| `UNIMPL Command <svc> <n>` | a session had no handler: success with zeroed output (matches nxnative and Turbo) |
| `UNIMPL Svc <n>` | an SVC nobody handles: NotImplemented 0xF601 (`src/os/src/svc.rs`) |
| `UNIMPL Nvn <proc>` | an NVN proc graphics does not implement ([nvn-objects.md](nvn-objects.md#nvn-entry-points)) |

`NXOS_TRACE=svc,ipc,hle,nvn,fs` (or `all`) logs each call of those kinds; `NXRT_TRACE=svc,host,...` the runtime's
side. The `sdk: NintendoSDK <n>` line gives the SDK major read from the `sdk` module (19 when absent), which selects
command layouts in several services.

## Service connections and the boot

The process starts without rtld, in `nn::init::Start`'s order (`src/os/src/boot.rs`): `_init_libc0`,
`nnosInitialize`, `_init_libc1`, `nninitInitializeSdkModule` (each service init returns at once,
`src/os/src/services/sdkinit.rs`), the host arguments, `nninitStartup`, `_init_libc2`, every module's DT_INIT, then
`nnMain`. Host bindings are installed before the modules are relocated. Arguments: `<game>.exe <data dir>` (or
`NXOS_DATA` / `NXRS_DATA`); saves under `<data>\save` (or `NXOS_SAVE`).

Constant answers of offline services (local wireless, friends, telemetry, IR camera, SSL, parental controls, the
shop) are one row each in `src/os/src/services/simple_table.rs`, served by one host function that finds its row by
the name it was bound under (`simple.rs`). Services with a little state are `offline.rs`, `net.rs`, `system.rs`.

## File system and save data

`src/os/src/services/fs.rs` serves nn::fs directly over the packages:

| Mount | Backed by |
|---|---|
| `MountRom` | the RomFS read in place with the update applied ([packing.md](packing.md#romfs-markers)), with mods' `romfs\` layered over it |
| `MountRom(name, program index)` | `<data>\romfs_programs\<index>.img`, else the game's |
| `MountAddOnContent(index)` | the installed title's RomFS |
| `MountSaveData(uid)`, `MountDeviceSaveData` | `<save>\user<n>`, `<save>\device`, transactional |
| `MountTemporaryStorage`, `MountCacheStorage` | `<save>\temp`, `<save>\cache<n>`, written directly |
| `MountSystemData` | `<save>\system\<id>`, else "target not found" |

Save transactions are described in [game-state.md](game-state.md#saves). Handles a guest holds are the host's number
shifted left by 4 (games mask low bits). `fsp-srv` over IPC (`fsp.rs`) serves the SD card as `<data>\sdmc`, the
save-data info readers (what exists), the device operator and detection notifiers that never fire.
`NXOS_FS_TRACE=1` logs every file open with its result (`=miss` only the failures).

## Add-on content

The by-name takeovers and `aoc:u` (`system.rs`) list the titles installed in the data folder whose RomFS opens
(`fs::aoc_indices`); the list-changed event exists, purchases never happen.

## Input

`src/os/src/services/hid.rs` shows the host's controllers (`src/os/src/input/`) as npads: player 1 is Handheld in
handheld mode, else No1, and reads the keyboard and mouse, scripted input and its controller ORed; players 2-8 read
their own pad. Joy-Con pairs are shown in the style the game asked for; a full-key pad the game does not support is
shown as a JoyDual pair. State reads fill all 16 history entries with consecutive sampling numbers. Six-axis reports
live motion (`src/os/src/input/motion.rs`); vibration goes to the player's pad. Touch and gestures come from the mouse or the
control pipe (`gesture.rs`); rarer devices (Joy-Con 2 mouse, the SDK's IMU, Palma, the console's own sensor) are
`hid_ext.rs`. Controller paths, `button_layout` / `button_map`, rumble options and the controller-support prompt are
in [docs/settings.md](../settings.md) and the `src/os/src/input/*.rs` module headers.

## Software keyboard and applets

- `swkbd.rs`: ShowKeyboard and the InlineKeyboard (its methods taken over by SDK name). A text box opens over the game
  (`keyboard_box=1`), Enter = OK, Esc keeps the default; the default is the game's initial text, else `keyboard_text`.
  With no window (`NXOS_SWKBD=auto`, headless) the default is answered at once.
- `applet.rs` (appletOE / appletAE library applets an SDK starts itself): MiiEdit answers "cancelled", PlayerSelect
  answers the PC's player (`pc_user`, or the picker with `ask_user=1`), others their output size of zeros.
- `notice.rs`: the error applet shows a message box beside the game (`error_notice`); offline HTML pages open in the
  PC's browser (`offline_pages_in_browser`). Neither opens under `NXRS_NO_ACTIVATE` or headless.
- `mii.rs`: the six default Miis and a MiiModel stand-in whose parts draw nothing.

## Display, window and operation mode

- `oe.rs`: operation mode from `docked` (TV by default; `NXOS_OPMODE` overrides), changed live with the console's
  notification; focus messages follow the window (in front / behind) only with `pause_on_focus_loss=1`
  (`NXRS_KEEP_FOCUS=1` forces in focus); the language from `language` (`NXOS_LANG`); ExecuteProgram and
  RequestToRelaunchApplication end this process and start a successor (program `index` of the same application,
  or program 0 again), passing `NXRS_LAUNCH_PARAM` (a file), `NXRS_PREV_PROGRAM` or `NXRS_RELAUNCH=1`; the log
  shows an `exec:` line.
  `NXOS_GFX_SCALE` sets what GPU-scale queries answer.
- `vi.rs`: display, layer and native window are host objects; the layer crop is kept for the present; the frame
  number is the present count. BufferMaxResolution answers the largest of 720p / 1080p / 1440p / 2160p the host's
  display holds when docked (1080p handheld); `NXOS_DISPLAY_MAX=0..3` caps it. `vi_ipc.rs` serves raw vi /
  nvdrv clients that draw on the CPU into nvmap memory and queue it through the binder (shown through graphics'
  `present_rgba`).

## Accounts, network, shop

- `account.rs`: local profiles only; the player is `pc_user` (1..8) named `pc_user_name` (`NXOS_NICK`); more profiles
  exist when a game made them (`user<n>.registered` markers). Online accounts are unavailable.
- `net.rs`, `socket.rs`: no network (ENETDOWN), but event descriptors and loopback UDP work inside the process
  (network libraries' worker wake-ups, RakNet's start-up datagram). Endless waits return every 100 ms so a blocked
  thread stays visible.
- nifm's request goes on hold for one query, then unavailable; friends, shop and BCAT answer empty (`offline.rs`,
  `system.rs`).

## Settings, languages and fonts

`settings.rs` and `set` (`system.rs`): the language codes in nn::settings order, the region that follows from the
language, the PC's name as nickname; `set:sys` reports firmware 12.1.0 (20.1.0 for a Switch 2 program). Shared fonts
are system content, so `misc.rs` serves a stand-in TTF per slot from the host's fonts (Arial, Segoe UI, Tahoma;
`NXOS_FONT` names another).

## C library, time and random numbers

- `time.rs`: user and network clocks are the host's real time, the steady clock its monotonic clock, calendar
  conversions the host's time zone; the `time:*` services' shared page is kept so the SDK reads clocks without
  requests. `NXOS_FIXED_TIME=<POSIX seconds>` pins the user clock.
- The system counter runs at the title's rate on the guest clock (`src/os/src/clock.rs`), which recording may slow.
- `spl:`, `csrng`: random bytes from the OS (`system.rs`, host entropy in `os.rs`).
- `os.rs`: `game_env` getenv pass-through and `alloc_big_align` ([memory.md](memory.md#engine-allocators)).

## Modules and symbols

`ro.rs`: `nn::ro::LoadModule` maps an NRO the build translated (found by the image's SHA-256), relocates it against
the modules loaded then, binds imports of earlier modules that only it defines, and runs DT_INIT on the caller's
thread; UnloadModule reverses it. A module not built fails as on the console; an online SDK fails so the game plays
offline; a module the game decodes at run time is captured to `<data>\nro\<sha256>.nro` for the next build. The
control-pipe command `addr <mangled name>` gives a symbol's guest address and its takeover.

## Problems found on games

### Mii editor result read as a new Mii

- **Symptom:** Mario Kart 8 Deluxe crashes in `nn::mii::CreateId::IsValid` after the Mii editor applet.
- **Cause:** zeroed applet output reads as "a Mii was made at index 0" of an empty database.
- **Fix:** MiiEdit answers `result = 1` (cancelled), `index = -1` (`applet.rs`).

### Bluetooth interfaces missing at start

- **Symptom:** Pokémon Violet 4.0.0 aborts at start.
- **Cause:** it opens `btm:u` and `bt` for the Pokémon GO Plus + and aborts when sm refuses them.
- **Fix:** both are served offline with events that never fire (`net.rs`).

### Event descriptor sockets

- **Symptom:** Ocarina of Time's network library (NPLN) aborts at start.
- **Cause:** newer network libraries create an event descriptor for their worker's wake-up and abort without one.
- **Fix:** `bsd:u` and the by-name socket takeovers serve event descriptors and loopback UDP (`socket.rs`, EventFd 31).

### Unknown Switch 2 services

- **Symptom:** Mario Kart World calls through a null object after `nn::chat::Initialize`.
- **Cause:** sm refused `chat:u`, so command 0 never handed out the interface.
- **Fix:** a Switch 2 program gets the catch-all `Unknown` session (above).

### Name entry in scripted runs

- **Symptom:** a scripted run of a new game stops at name entry (PORT_PLAN M6: Pokémon Violet).
- **Cause:** the software keyboard's text box waits for the player.
- **Fix:** `keyboard_box=0` answers `keyboard_text` at once; headless runs (`NXOS_SWKBD=auto`) do so already.

## Known limits

- Online play of any kind is unavailable on purpose.
- `nvdrv` is answered with failures for the SDK's hardware movie decoder path ([movies.md](movies.md)); NVN itself is
  the graphics layer.
- nxnative's notes on SDK names that changed between versions are carried in the takeover tables themselves (several
  mangled names per function) rather than documented separately.
