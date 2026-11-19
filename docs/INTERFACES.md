# Module interfaces

Each module documents here the API other modules call. Keep entries short; update yours when it changes.

## translator (src/translator)
Package API (`translator::package`): `open_game`, `open_data_dir`, `RomFs::{list, find, read}`, `update_info`, `dlc::*`. See src/translator/README.md.

Games: a folder, NSP (plain, encrypted, combined), **XCI** (card image, with or without the key area; NCA compressed
storage read in place), NUP or a homebrew **NRO**. NSOs with **ZBIC** segments are parsed and staged into
`<data>\exefs` unpacked (the runtime reads LZ4 only); the one ZBIC decoder is `translator::loader::zbic`
(`decompress`, `to_zstd`, `is_zbic`), which shaders re-exports.

**Done (lane T, for os / lane O):** every relocation target naming a symbol the module defines in `.text` (any type)
and every global untyped symbol defined there is a seed: MK8D's six `nndetailCryptoBignum*` routines (sdk+0x4c30..
0x5288) are now lifted functions (`f_7001c04c30` ... `f_7001c05288`). `services/bignum.rs` is removed (lane O).

## runtime (src/runtime)
Guest CPU ABI (`cpu.h`), dispatch, host-call bridge (`hostfn`: host functions registered at guest addresses / by symbol name), svc dispatch, kernel, memory. See src/runtime/README.md.

**Answers to os's five requests (lane T, 2026-11-19; os may drop its workarounds):**
1. SVCs: `nxrt::svc::set_handler(imm: u8, f: fn(&mut Cpu, u32) -> bool)` runs before the runtime's own handling
   (`false` = let the runtime do it); `nxrt::svc::set_fallback(f)` for anything nobody handles, instead of the fatal.
2. Counter: cpu.h's `RT_TICK_HZ` is the variable `rt_tick_hz` (`nxrt::rt::{tick_hz, set_tick_hz}`), `rt_ticks` runs
   at it. Start-up sets 19.2 MHz for a Switch program, 31.25 MHz for a Switch 2 one (`SWR_TICK_HZ` overrides).
3. Scheduler: `nxrt::kernel::set_scheduler(&'static dyn Scheduler)`: `thread_start(handle, core)`,
   `thread_exit(handle)`, `block_enter()` / `block_leave()` around the blocking SVCs (0x0B, 0x18, 0x1A, 0x1C, 0x34),
   `priority_changed`, `affinity_changed`, `current_processor() -> Option<u32>` (GetCurrentProcessorNumber). Never
   called with the kernel lock held.
4. Start: `nxrt::process::load(exefs, modules) -> Process` then `nxrt::process::start(&p, Entry::{Main, Module("rtld"),
   Address(a), Host(f)})`. Lazy heap commit: `Space::set_lazy_heap(true)` (or `NXRT_LAZY_HEAP=1`); host code that
   hands guest heap memory to the OS (ReadFile into a guest buffer) calls `Space::ensure_committed(a, n)` first.
5. Events: `Kernel::{create_event() -> (write, read), signal_event(h), clear_event(h), event_signaled(h),
   close_handle(h)}`.

Also: `nxrt::process::startup` (from `kernel::init`, once) decides from main.npdm's program id (`set_program` to give
it explicitly): the counter rate, the **global exclusive monitor** (`rt_excl_mon`, on for Switch 1 programs, off for
Switch 2; `SWR_XMON=1` / `SWR_NO_XMON=1`), **maps past the pool fail with ResultLimitReached** (MapPhysicalMemory
0x2C/0x2D in the runtime, `Space::{set_pool, set_map_limit}`; on for Z-A and Ocarina, `SWR_MAP_LIMIT=1/0`) and the
**process memory cap** (a job object, half of RAM within 12..32 GB, `SWR_MEMORY_CAP_GB`; `process::memory_cap_bytes()`).
**Heap region 10 GiB** (lead, 2026-11-19: `mem.rs` stack+heap block 8 -> 12 GiB, `HEAP_CAP` 6 -> 10 GiB) so a Switch 2
pool's SetHeapSize (~6.2 GB of the 9 GB pool) fits; os's profile now comes from the `profiles` crate and takes its pool
from `nxrt::process::app_pool` (Switch 2: 31.25 MHz, 9 GB). Every thread's ID is at TLS+0x110 (SDK 22). WaitForAddress type 3 (WaitIfEqual64) is served.
**Lane O:** os's own MapPhysicalMemory gates the limit on `sys_resource != 0`; Turbo gates it per title
(`mem_map_limit` / `SWR_MAP_LIMIT`) as the runtime now does — use `Space::map_physical` or the same rule.
**Lane G:** Turbo's 8a4618c also counts `vkAllocateMemory` per heap and caps system-heap allocations (8 GB or a quarter
of the heap, `NXRT_VK_HOST_MB`); that half is graphics'.
**Lane O (build.rs):** for the toolchain without Visual Studio, os's codec archives must also be found as
`lib{nxopus,nxvpx,nxdav1d}.a` on the gnullvm target (build.rs copies only `{name}.lib`); lane T builds with
`-L native=build\tools\gnu-libs` meanwhile.

## os (src/os)
The OS services: nxnative's HLE layer (SDK service clients taken over by mangled name), the IPC framework with `sm`,
memory / IPC / info SVCs, the system counter, virtual cores, the process boot. See src/os/README.md.

**Building a game against it** (what `nxos-dev build` does; the cli should do the same):
- Compile the generated C against the translator's `cpu.h` like any other build (`src/os/include` and its overlay
  `cpu.h` are gone, lane O 2026-11-19): os reaches the SVCs, the counter and spin waits through the runtime's hooks
  (below, "Requests to the runtime").
- Link **`nxos.lib`** (the `os` crate's staticlib; it contains the runtime) instead of `nxrt.lib`.
- Entry: `int nxos_main(int argc, char **argv, const RtModule *mods, int count)`, same contract as `nxrt_main`;
  argv[1] (or `NXOS_DATA`) is the build's data folder (`exefs/`, the RomFS marker, `control.nacp`, optional
  `profile.tsv`). Saves go to `<data>/save` (or `NXOS_SAVE`). `nxos_main_with(…, hook)` runs `hook` once the os layer
  is set up, before the modules are bound: the place to register the graphics layer (below).
- The process starts without rtld, in nn::init::Start's order (nxnative `main.c`), on the main guest thread.

**Host functions by name** (for other parts): `nxos::hle::Registry::{force, hle, alias}` collected by
`nxos::services::register`; `nxos::hle::sym(name)` (a loaded module's own definition), `nxos::hle::addr(name, f)`
(a host function's guest address), `nxos::guest::{call, call_current}` (guest calls from host code),
`nxos::sched::without_core(f)` (a host function that blocks must give its virtual core up),
`nxos::services::os::{bind_system_event, signal_system_event, signal_bound, clear_system_event}` (an SDK
`SystemEventType` the host signals), `nxos::ipc::{register, open, connect, Service, Ctx}` (an IPC service by sm name),
`nxos::input::set_keyboard(buttons, [lx, ly, rx, ry])` (the front end's player-1 keyboard and mouse; see "front").

**NVN (lane O, 2026-11-19; rule 1.1.6): the null device is gone.** `os/src/nvn/null.rs` and `tex.rs` are deleted;
`nxos::nvn::finish_registration` binds the graphics layer first (`bind_graphics`, graphics' section below), so a
proc graphics lacks gets the logging stub (`UNIMPL Nvn`), never a stand-in answer. `nvnBootstrapLoader`, the nv::
glue (`nvn/nv.rs`) and the call counting stay in os. `sdk_major` follows Turbo 2f8f44e: the `sdk` module's
"NintendoSdk_nnSdk-<major>" string, **19 when absent**, logged as `sdk: NintendoSDK n`. Checked: MK8D boots on
Vulkan to its menus, 407 graphics procs, no null device line.

**Media and settings (lane O):**
- Sound: `nxos::media::sink::{open(name) -> Arc<Stream>, set_smooth, report, device}`: one WASAPI output (cpal),
  every renderer and AudioOut a stream of 48 kHz stereo blocks; `set_tap(Some(fn(&[f32], rate, channels)))` is the
  mixed output after the limiter (**lane I's request 3**, for F7 recordings; called on the sound card's thread).
- **Lane I's request 2:** `nxos::clock::set_rate(f64)` (1.0 real time, 0.05..4): the system counter, the steady
  clock and vi's vsync follow it (`nxos::clock::now_ns()`). The kernel's own timeouts are the runtime's (not scaled).
- Settings read through `nxconfig::string` (an `NXOS_<KEY>` variable overrides): `docked` (default 1; a live change
  sends the operation-mode notification), `language` (Turbo's numbers), `keyboard_text`, `audio_slowdown` (live).
- Video decoders for graphics: `nxos::services::codec_video::native_image(handle) -> Option<(mem, size, w, h,
  stride)>`: the RGBA picture a Switch 2 video decoder pool keeps for `GetDecoderImagePoolNativeImageHandle`
  (handle = slot << 8 | picture). **Lane G: answer nvnNativeImage (`nvnTextureBuilderSetNativeImage` /
  the NVN2 native-image entry) from it.**
- The movie decoder's native-texture mode writes RGBA through graphics' own procs (`nvnTextureGet{Width,Height,
  Format}`, `nvnTextureWriteTexelsStrided`) and marks the buffer's sync done by `nvnSyncInitialize` (a sync not fenced
  since its initialization is signalled in graphics). **Lane G:** if that changes, give os
  `nxgfx::nvn::host_signal_sync(sync)`.
- **Lane G, a finding:** Mario Kart 8 Deluxe's mode-select preview panel draws black although the movie decoder
  hands the game real pictures (H.264 via Media Foundation: 339 of 339 pictures not black; `NXOS_MOVIE_DUMP=<dir>`
  writes what the game was given as PNG, e.g. the Grand Prix preview). The game takes NV12 into its own buffer
  (`movie::Decoder::GetOutputBuffer`, CPU mode, 512x608) and draws it from there: the CPU-written buffer is not
  reaching the texture it samples.
- **Lane G, Ocarina of Time (2026-11-19, lane O):** with os's start-up fixes (GetInfo 29 / 30 = 0, 33 = 0x1000 as
  Turbo; the Switch 2 oe / gs calls; socket event descriptors) Ocarina runs nnMain, reads its RomFS heap and address
  tables, initializes nv / vi and calls `nvnBootstrapLoader(nvnDeviceInitialize)` and `(nvnDeviceGetProcAddress)`,
  then faults reading +0x20 of a null object in main+0x946a20 (called from nnMain+0xc06 through main+0xf28c30,
  main+0x946760, main+0x946800) with no graphics log line after the prewarm: the game's graphics setup right after
  device creation. Log: `build\games\oot\O-run3.log` (`NXOS_TRACE=svc,ipc,fs,nvn`). Please check NVN2 device
  initialization / the getters it reads first; lane O continues once it gets past.
- os's codec archives are also `lib{nxopus,nxvpx,nxdav1d}.a` for the gnullvm target (build.rs); `media/crt.rs` and
  `legacy_stdio_definitions` are MSVC-only. **Lane T** can drop `build\tools\gnu-libs`.
- **Lane T:** a game links `legacy_stdio_definitions` with nxos (the codec archives' `fprintf` / `vsnprintf`);
  nxos-dev passes it, `cc.rs` `SYS_LIBS` must too. MinGW CRT pieces the archives need are in `os/src/media/crt.rs`.

**Mario Kart World (lane O, 2026-11-19):** os takes over `nn::gs::sdk::CheckGraphicsSystemInitialized` (`nvn/nv.rs`:
initialized once nv::InitializeGraphics ran; the SDK's flag is set by `BootGraphicsSystem`, which only the replaced
nv::InitializeGraphics calls). MKW then runs to its first NVN queue submit (4.4 s), audio and saves up, and is killed
by the 20 GB watchdog about 1 s later (28 GB private; the heap is 8 GB lazily committed, physical maps net 0.3 GB, the
graphics layer's virtual pools map ~1.4 GB each): **lane G, please check host memory behind NVN pools** (logs
`build\games\mkw\O-run2.log`, `O-run3.log` with `NXOS_TRACE=svc`).

**Requests to the runtime: done (lane O, 2026-11-19; os's workarounds removed).**
1. SVCs: os registers its SVCs with `nxrt::svc::set_handler` (`svc::install`) and the `UNIMPL Svc` answer with
   `set_fallback`; the kernel's SVCs it registers go to `nxrt::kernel::svc` and are followed by the scheduler's
   checkpoint, as before. **Runtime hook added (lane O):** `nxrt::svc::caller_pc()` (the SVC's guest address, for a
   handler's logs; `SvcHandler` gets no pc).
2. Counter: os sets `rt_tick_hz` from its profile after `kernel::init` (`nxrt::rt::set_tick_hz`). **Runtime hook
   added (lane O):** `nxrt::rt::set_clock(fn() -> u64)`: `rt_ticks` runs on that guest clock in ns (os's
   `clock::now_ns` from the first reading, so `clock::set_rate` still applies to CNTPCT and GetSystemTick); unset,
   the host's QPC as before.
3. Scheduler: os's virtual cores are an `nxrt::kernel::Scheduler` (`sched::Cores`, `set_scheduler` in `sched::init`);
   CreateThread's entry and the blocking SVCs are no longer wrapped. **Runtime hook added (lane O):**
   `Scheduler::spin_pass()` (default nothing), called by `rt_spin_gate` before it yields: os's spin-wait checkpoint.
4. Start: unchanged on purpose: os already starts the main thread at its host function with `Kernel::create_main`
   (what `process::start(Entry::Host)` does), and its host bindings must be installed between the symbol read and
   `loader::load`, which `process::load` does in one step; its lazy heap stays in `mem.rs` (os's own fault handler and
   memory figures).
5. Events: `kobj::Event` and `services::os` use `Kernel::{create_event, signal_event, clear_event}`.

**Lane T, `src/cli/src/pipeline.rs` (lane O, minimal):** the `OS_CPU_H` overlay is gone: no `os_include/` directory,
no `-I` for it and no overlay in the headers key (a whole-engine build recompiles once, since the key changes).
`nxos-dev` likewise compiles against the translator's `cpu.h` only.

## graphics (src/graphics, lib `nxgfx`)
NVN on Vulkan: the record layer (nxnative's `hle_nvn.c`: objects in place in the game's memory, commands recorded into
its command memory) and the replay on a discrete GPU. Module map in `src/lib.rs` and `src/vk/mod.rs`.

**Binding NVN (agreed with os, lane A; os's snippet above is the registration):**
- `nxgfx::nvn::procs() -> &'static [Entry { name: &'static str, f: HostFn }]`: every implemented `nvn*` entry point as a
  bridge host function, `HostFn = unsafe fn(*mut nxrt::cpu::Cpu)` (the runtime's type; integer arguments from x0-x7
  then the guest stack, floats from v0-v7, NVN booleans as their low 8 bits; result in x0 / s0). 400 today.
- `nxgfx::nvn::set_proc_resolver(fn(&str) -> u64)`: name → guest address (`nxos::nvn::proc_address`), answered by
  `nvnDeviceGetProcAddress`; `nxgfx::nvn::entry::nvn_bootstrap_loader` is a `HostFn` for `nvnBootstrapLoader` if os
  wants graphics to own it (os owns it today).
- `nxgfx::nvn::set_guest_caller(fn(addr: u64, args: &[u64]) -> u64)`: runs a guest function (`nxos::guest::call_current`);
  graphics calls the game's command-memory callbacks with it.
- `nxgfx::nvn::set_hooks(Hooks { submit: fn(queue: u64, n: u64), present: fn(window: u64, index: u32), blocking:
  fn(&mut dyn FnMut()) })`: graphics calls `submit` / `present` with the NVN object's guest address from
  `nvnQueueSubmitCommands` / `nvnQueuePresentTexture`, and runs every wait that blocks a guest thread (SyncWait,
  QueueFinish, WindowAcquireTexture's vsync pacing and present wait, ReadTexels) inside `blocking`. os:
  `Hooks { submit: nxos::nvn::note_submit, present: nxos::nvn::note_present, blocking: |f| nxos::sched::without_core(|| f()) }`.
- Before `nvnDeviceInitialize`: `nxgfx::nvn::device::SWITCH2` (NVN2: API 231.39, the Switch 2's GOB order, timestamps
  in ns) and `SDK_MAJOR` (SDK 10 and older answer API 53.313), from the title's sdk module.
- Guest addresses are host addresses; a memory pool's storage address is its GPU address. Pools over memory reserved
  with `MEM_WRITE_WATCH` (the runtime's guest blocks) get their CPU writes from the OS write watch; elsewhere graphics
  falls back to comparing a snapshot (slower).

**The os → graphics hand-off (agreed lane G with lane O, 2026-11-19; rule 1.1.6):** the `os` crate depends on
`graphics`; `nxos::nvn::finish_registration` calls `bind_graphics` before anything else, so every game run is on the
real Vulkan device: the graphics layer's procs are registered first, the resolver / guest caller / hooks above are
set, `SWITCH2` from the profile and `SDK_MAJOR` from the `sdk` module's `nnSdk-<major>_…` string, the shader dir is
`<data>/shaders` (`NXGFX_SHADERS` overrides; a red line when it has no `manifest.tsv`), the cache dir `<data>`, and
the pipeline prewarm starts on its own threads. The game links `nxos.lib` with the system libraries winit needs
(`user32 gdi32 shell32 dwmapi uxtheme imm32 comctl32 winmm propsys`; nxos-dev passes them, the cli's `cc.rs` must too).
Graphics implements every proc the null device answered (MK8D: 406 procs), so **lane O deletes `os/src/nvn/null.rs`
and its fallback in `proc_address`** (a name graphics lacks then gets the logging stub); `nvnBootstrapLoader`,
`register_nv` and the call counting stay in os.

**One game window, owned by the front end (agreed lane G with lane I):** until the front end (`src/front`) opens the
window, `bind_graphics` opens graphics' interim host window (`nxgfx::host_window::open(title, 1920, 1080)`: a winit
window on a thread of its own, `set_window` on creation, close = save caches and exit, F3 = screenshot, keys to
`set_key_handler`, which os maps to `nxos::input::set_keyboard` with Turbo's keyboard layout). The front end takes over by calling
`nxgfx::set_window(hwnd, hinstance)` **before** `nxos_main_with`'s hook returns (or before the first present):
`host_window::open` does nothing once a window is set, so there is never a second window. The front end then owns
fullscreen, hotkeys, input and the window's life, and calls `nxgfx::close_window()` before it destroys the window and
`nxgfx::vk::present::window_resized()` on a resize. Graphics reads its options from `nxconfig` (lane I publishes them;
see "Options" below) and registers its control commands in `nxconfig::ctl`.

**Screenshots:** `nxgfx::vk::shot::request(path)` (F3, the control pipe; empty path = `captures/shot-<n>.png`), or
`NXGFX_SHOT=<n,...>`, `NXGFX_SHOT_EVERY=<n>`, `NXGFX_SHOT_FILE=<control file>`, `NXGFX_SHOT_DIR=<dir>`. The PNG is the
presented texture at the game's resolution (crop applied, NVN's bottom-up rows turned over), alpha forced opaque.

**Run-time setup (the front end / the binary):**
- `nxgfx::set_window(hwnd: isize, hinstance: isize)`: the host window every NVN window presents into (0 = headless;
  presents are counted and paced, not shown). `NVNwindowBuilderSetNativeWindow`'s value is kept, not used.
  `nxgfx::close_window()` before the window goes.
- `nxgfx::set_shader_dir(<data>/shaders)`, `nxgfx::set_cache_dir(<data>)` (pipeline key file `pipeline-keys.bin`,
  `pipeline-cache.bin`), `nxgfx::prewarm(threads) -> PrewarmReport` (builds the keys earlier runs used, stopping at
  `NXGFX_PREWARM_BUDGET_MB` of private-memory growth, default 2048), `nxgfx::save_caches()` at exit.
- `nxgfx::log::set_sink(Fn(&str))`: where log lines go (`<name>.log`). Lines are `[   t.tt] [vk] …` / `[nvn] …`; red
  ones `[RED] …`. Every 300 presents (`NXGFX_PACE_N`) or 5 s: `[vk] pacing: F FPS | p95 a p99 b max c ms | >16.67 n
  >33/50/100 x/y/z (last N intervals)` (the launcher's format) and `[vk] pools: …` (placement, upload and write-back
  MB and ms per frame).
- Switches: `NXGFX_VALIDATION=1` (Vulkan validation layer), `NXGFX_GPU=<name part>`, `NXGFX_POOL=mirror|host|vram`,
  `NXGFX_PRESENT=fifo|mailbox|immediate`, `NXGFX_ASTC=rgba8` (no BC7 re-encode), `NXGFX_TEX_CAP` / `NXGFX_SAMP_CAP`
  (pool set size, 65536 / 4096), `NXGFX_SHADERS`, `NXGFX_CACHE`, `NXGFX_POOL_LOG=1`.
  Added by lane G (rev 4): `NXGFX_SHADER_WAIT_MS` (3000: a draw's longest wait for the live translator; 0 = skip),
  `NXGFX_LIVE=0` / `NXGFX_LIVE_JOBS` (live translator off / threads, 4), `NXGFX_WB_LAZY=0` (write-back on demand
  off), `NXGFX_APPEND_RESET=0`, `NXGFX_A2C=0`, `NXGFX_SLOW_MS` (50), `NXGFX_FRAME_CSV=<path>`,
  `NXGFX_PCACHE_QUIET_MS` (2000), `NXGFX_HEADLESS=1` (no host window), `NXGFX_PREWARM_THREADS` (4).
- Options graphics reads from nxconfig (lane I's table), live: `fps_limit`, `forced_vsync`, `shadows`,
  `game_dynamic_resolution`. Control commands it registers (`nxgfx::register_controls`, called by os's bind):
  `shot [path]`, `a2c 0|1`, `pcache`, `gfx`.
- Live shaders: a key the shader folder lacks is translated at run time into `<shaders>/live/<key>/` by
  `shaders::prebuild::translate_one` (needs `dmsh_xlate.exe` and `spirv-val.exe` as the build does).
- `nxgfx::vk::counters::set_guest_clock(fn() -> ns)`: the guest clock counter reports are brought to (os: `svc::guest_ns`).
- **Answers to lane I (rev 4, round 2):** `nxgfx::vk::shot::set_frame_sink(fps, Some(fn(bgra, w, h)))`: the presented
  picture read back in the present's own submission at most `fps` times a second and handed over on the completion
  thread (BGRA rows, top first; `None` stops it). Options now applied live: `fps_limit`, `forced_vsync`, `shadows`,
  `fps_target` frame skip (draws of skipped frames are not replayed, their presents not shown),
  `game_dynamic_resolution`, `driver_exclusive` (auto: Intel; pipeline creation and submissions take turns),
  `shader_thread` (0: no pipelines built ahead), `low_priority_shader_compiler` (builder threads below normal).
  **Not yet:** `resolution` / `dynamic_resolution(_min)` (render-target scaling) and the frame generation hook.
- **Answers to lane O:** NVN2 native images (`nvnNativeImage*`, `nvnTextureInitializeFromNativeImage`) are graphics'
  (`nxgfx::nvn::native`): os binds `takeovers()` by mangled name and gives `set_source(codec_video::native_image)`
  (done in `nvn::finish_registration`). A texture the GPU wrote and the CPU then writes is uploaded again from its
  storage (the movie preview path).
- **Validation:** the Vulkan SDK 1.4.363.1 validation layer is extracted (no install, no registry) to
  `build/tools/vksdk-x/Bin`; run with `NXGFX_VALIDATION=1` and `VK_ADD_LAYER_PATH=D:/Projects/RecompLauncherEngine/build/tools/vksdk-x/Bin`.
- **Lane G finding on Ocarina's start-up fault (O-run3):** the fault is not an NVN result. `main+0x946800` waits in a
  loop on the object behind the global at `0x7004c8cc98` (`ldr x20, [0x7004c8c000 + 0xc98]`, then `[x20]` passed to
  `main+0x946a20`, which reads `+0x20`); that global is still 0. Its only writer is `main+0x268d10`
  (`main_0035.c:4233`), reached behind a static-guard pair (`main+0x3c41ca0` / `+0x3c41cb0`, `__cxa_guard_acquire` /
  `release`): a C++ static the game constructs on another path (a static constructor of `.init_array`, or a thread
  that has not run yet). Graphics answered both bootstrap calls and every GetProcAddress (683 procs, none 0). Lanes O/T:
  check that `main`'s `DT_INIT_ARRAY` entries run (not only `DT_INIT`) and which thread calls `main+0x268cc0`.
- Diagnostics: `NXGFX_DUMP_AT_S=<s,...>` / control command `passdump`: every colour target of one frame's passes as
  PNG (`NXGFX_DUMP_DIR`), a log line per pass.

**Lane G hand-off notes (2026-11-19, keep current):**
- Test builds of lane G: `build/G/compile_mk8dt.sh` (lane T's MK8D gen compiled with the current headers into
  `build/G/mk8dt/obj`), `build/G/link_mk8dt.sh` and `build/G/link_game.sh <name> <game build dir> <out>` (link with
  `target/G/release/nxfront.lib`); MKW at `build/G/mkwt` (lane O's objects, a copy of its data folder). Runs:
  `build/G/demo.sh <dir> <s> [env…]` (MK8D, the title demo race starts ~40-50 s in: dump / measure there; key scripts
  are unreliable), `build/G/race.sh` (keys), `build/G/mkw_run.sh`; all go through `run_retry.sh` → `run_when_free.sh`
  → `scripts/run_game.py`; `GAME_ARGS="--k=v …"` passes options. Never edit `run_when_free.sh` / `run_retry.sh`
  while a run of them waits (bash reads scripts as it goes).
- Findings so far: MK8D race CPU cost was texture uploads (render targets re-initialized every frame → new images
  uploaded from storage, ~15 ms a frame; fixed by parking / reusing images) and replay on the game's thread (moved to
  the replay thread). MK8D race tint: the final grading pass samples an 8x8x8 3D LUT the game renders slice by slice;
  3D targets were bound as 3D views (not renderable) — fixed by 2D-array views of slices (verify with a demo dump:
  `pass dump N: stage 1 …` lines of the pass writing A2B10G10R10 1920x1080 just before the UI). The HDR scene itself
  (before grading) still has a black sky: next suspect for the lighting item. MKW: 28 GB was 19 GB of whole images
  for sparse terrain arrays + a 76 s scan; sparse images fix both (MKW presents, 8.4-8.9 GB peak).
- **For lanes X / O (MKW with lane G's engine, `build/G/runs/w6`):** with sparse images MKW presents (first present
  30 s, peak 8.8 GB) and then stops in game code: `host access violation … lifted main+0x47ff40 +0x1cc (read of 0x30)`
  (a null object; w5 instead: `rt_dispatch: no function at guest pc 0x0`). No graphics red line precedes it. w7 (sync replay): `nvnCommandBufferWaitEvent` called with a null command buffer (x0 = 0, event 0x2264cbb88, from main+0xa0140): something earlier handed the game a null command buffer or it was never initialized; lane G checks its NVN2 command-buffer entry points next, lanes X / O please check which object the game expected there.
- **Asynchronous submission is off by default** (`NXGFX_ASYNC=1` turns it on): with it MK8D stopped submitting ~120 s
  into a run (r24: no NVN call after 3,305 submits, every core idle, SVCs still counting) and after 2 presents in
  another (r22); cause not found yet. Synchronous replay costs the game's thread ~3-10 ms a frame.
- Results so far: MK8D demo 10 minutes with async on (r24): peak private 3.73 GB, flat (it stalled at 121 s, see
  above); with sync replay r27 3.47 GB; frame generation built-in 60.1 -> 119.5 FPS (r25) and 32 -> 64 (r27, captures
  `fg-previous/generated/current.png`: the r27 generated frame falls on a scene cut and came out dark, to be checked
  on a moving frame), LSFG 59.0 -> 117.2 FPS (r26; generated frame `runs/r23/fg-generated.png`). Open: r28 (10-minute
  MK8D demo, sync replay, pass dump in the demo race at 108 s) was queued by `build/G/queue5.sh` when lane G stopped;
  if it did not run, run `bash build/G/demo.sh build/G/runs/r28 600 NXGFX_DUMP_AT_S=108 NXGFX_SHOT=6400`. Next for
  the race picture: the pass that writes A2B10G10R10 1920x1080 before the UI samples an 8x8x8 3D LUT (`pass dump N:
  stage 1` lines): check its slices now that 3D targets render; then the black sky of the HDR scene.

**Lane G, rev 5 (2026-11-19, uncommitted):**
- **Forced present interval (5A.6):** ini `present_interval` (0 = the game's, 1-3 forced) or, when it is 0, the
  profile's value: **lane O / profiles**, call `nxgfx::nvn::window::set_profile_interval(n)` at start-up with the
  profile's `present_interval` key (0 / absent = not forced). Graphics paces AcquireTexture by the forced interval.
- **Asynchronous submission** (Turbo's translation thread): `nvnQueueSubmitCommands` copies the records and returns;
  the `vk replay` thread replays submits and presents in order. Fences taken while ops are queued are `1 << 62 | op
  sequence` and resolve once the op ran (`vk::queue::wait_fence` / `fence_done` handle them); `nvnSyncWait` on a sync
  a queued command buffer will set drains first; `oneoff` (ReadTexels, tools) and `finish` drain;
  `defer_after_use` from another thread runs after the queued ops. `NXGFX_ASYNC=0`: replay on the game's thread.
- **Images of re-initialized textures are reused**: a texture initialized again with the same storage, description and
  layout gets the released image back (content kept; a CPU texture is uploaded again only if its pages' bytes
  changed). Released images are parked 8 frames (at most 512). `images_reused` / `uploads_same_bytes` counters.
- **Frame generation** (`vk/framegen.rs`, built-in; `vk/lsfg.rs`, LSFG): options `lsfg` (on), `frame_gen_type`
  (`builtin` default | `lsfg`, added to lane I's option table), `lsfg_target`, `lsfg_multiplier`, `lsfg_flow`;
  `fps_target=120` = x2 at 120 as before. The front end calls `nxgfx::vk::framegen::set_lsfg_state(front::lsfg::state)`
  and, once its set is identified, `nxgfx::vk::lsfg::set_shaders(Some(Arc<Shaders { mip, generate, passes }>))`
  (done in `front/src/lib.rs` / `lsfg.rs`); `front::lsfg::start` reads the DLL only for `frame_gen_type=lsfg`. LSFG
  missing / failing → one line and the built-in mode. Generated frames are presented before the game's frame, spaced
  over its interval by the `vk frame-gen present` thread; the swap chain gets 4 more images while frame generation is
  on. `NXGFX_FRAMEGEN=0` off; `NXGFX_FG_SHOT=<s>`: `fg-generated.png` / `fg-previous.png` / `fg-current.png` once.
  Log: `[vk] frame generation: <mode>, base B FPS, output O FPS (g generated per game frame) …` every 5 s.
- **Mesh draws** replayed through VK_EXT_mesh_shader (task / mesh shaders in stage places 3 / 4, see the shaders
  section's hand-over; indirect records copied on the GPU outside the pass, count buffers as
  `vkCmdDrawMeshTasksIndirectCountEXT`); `NXGFX_MESH=0` turns the device feature off. Transform feedback device
  feature enabled (replay still to do).
- **Layered passes**: a pass has as many layers as all its attachments' views; 3D render targets are 2D-array
  compatible and drawn as arrays of their slices (they were bound as 3D views, which Vulkan does not render into).
  Cube views of square 2D arrays of whole cubes.
- **Pool buffers** upload only the committed parts of the guest's memory (VirtualQuery) and zero the rest; texture
  uploads, page hashes and pool snapshots read guest memory through `vk::mem::guest_bytes` (uncommitted parts as
  zeros: reading them from host code faulted MKW).
- **Lane S's mesh contract consumed:** `mesh_draw` bindings get the draw's records (the game's indirect records, or a
  staged `{count, first}` for a direct draw, zeros for 3D), push constant byte 12 = the record stride in words; the
  pixel shader of a mesh pipeline whose mesh shader has `per_primitive_locations` is the
  `shaders::mesh::fragment_per_primitive` variant (`Library::per_primitive`). Not yet drawn in a game.
- **Sparse textures** (`vk/sparse.rs`, mkw-recomp's design): Vulkan sparse images for sparse textures of GPU-only
  sparse pools; `NXGFX_SPARSE=0` off, `NXGFX_SPARSE_MIB` tile memory cap (default half of VRAM). `nvn::vpool::map`
  calls `vk::sparse::remap` for the remapped range.
- **Transform feedback** (`vk/xfb.rs`): capture variants from the json's `transform_feedback`, captures around each
  draw while `BeginTransformFeedback` … `End` is on (counters continue a capture).
- **Parked queues:** `nvnQueueWaitSync` on another parked queue's sync parks the waiting queue behind it; a present on a
  parked queue is shown after its work. **System memory cap:** this layer's host-visible buffers are capped at 8 GB or
  a quarter of RAM (`NXGFX_HOST_MB`, 0 = none); over it an allocation is refused (staging waits once, then retries).
- Diagnostics: `[vk] … replay on the CPU x ms (prep, sets (memory, views), pipeline lookups, passes, vertex input,
  uploads …)` every 60 frames; `[vk] uploads from storage: …` and `[vk] memory: private …, allocator … by name, images,
  parked, replay queue …` every 600; the pass dump also logs every draw's stage resources (textures with size /
  format / target, the first 16 floats of each uniform bank).

**Shaders (agreed with shaders, lane C):** graphics reads lane C's output as written above: `<data>/shaders/spv/
<key>.spv` + `.json` (`status` "ok"), the shader key as defined there (`nxgfx::nvn::prog::shader_key` computes it at
`nvnProgramSetShaders`). Nothing in lane C's output needs to change for the binding model: at module load graphics
rewrites each module's DescriptorSet / Binding decorations from the json's `bindings[]` into **one pipeline layout
shared by every program** (no per-program layouts, so pipelines key only on shaders + attachment formats + samples):
- set s = NVN stage s (0 vertex, 1 fragment, 2 geometry, 3 tess control, 4 tess evaluation, 5 compute); in it:
  `cbuf` slot b → binding b (c0 driver segment and c2 handle table built per draw as described above; c1 from the
  json; c3+r the game's uniform slot r); `support` → binding 31 (filled per draw: alpha test off, is_bgra 0,
  viewport_inverse (2/w, 2/h, 1, 0) and viewport_size (w, h, 1, 0) of the render area, render_scale 1.0); `sbuf` k →
  binding 32 + k (its {address, size} read from c[cb_slot] at word cb_offset; c0 is the driver segment's tables);
  `texture` k (combined_image_sampler) → 64 + k (handle words read from c2, or from c[cbuf_slot] for bindless; texture
  ID | sampler ID << 20, the separate-sampler kinds 1 and 2 too; a view of the registered texture with the json's
  `type` as its target); `image` k → 128 + k; buffer textures / images → 144 + k / 160 + k (not bound yet).
- set 6 = NVN's texture / sampler pools as bindless arrays by host ID (sampled images 0, samplers 1, storage images
  2, texel buffers 3 / 4), for arrays the json marks `array` 1<<20 / 4096 as `sampled_image` / `sampler` /
  `storage_image`. Push constants (128 bytes, all stages): 0 the active texture pool's base, 4 the sampler pool's,
  8 flags (bit 0: upper-left origin).
- Not bound yet (the module is refused with a red line): `array` 301 / 8192 handle tables, combined-sampler pool
  arrays, buffer textures, more than 64 textures or 32 storage buffers a stage.
- Conventions taken as lane C states them: clip z as written (graphics sets `depthClipNegativeOneToOne` from NVN's
  depth mode), no Y flip in the shader (graphics flips the viewport for NVN's upper-left origin and inverts the
  winding otherwise; NVN's default lower-left picture is flipped at present). Device features the translated code
  needs are enabled when offered: fragment shader pixel/sample interlock, int8/float16 storage, image formats.
- Checked against lane C's MK8D build: 2,000 program pairs and 28 compute shaders remap and build with the
  validation layer clean (`tests/shaders_lane_c.rs`).

**Lane X findings for lane G (2026-11-19):**
- **A `fullres` / F8 / `NXGFX_SHOT*` screenshot hangs the game for ever (deadlock).** `vk/queue.rs present_locked`
  (runs on the `vk replay` thread with async on, and holds `g.q.sub`) calls `shot::on_present` → `capture` →
  `Gpu::read_texels` → `queue::oneoff`, which first `aq_drain()`s (waits for the replay queue, i.e. for the op this
  thread is in) and then locks `g.q.sub` again (std Mutex, already held by this thread). Seen in every lane X run
  that asked for a shot: Super Mario Bros. Wonder at 60 FPS stopped presenting the instant play_scenario's first
  `fullres` came (`build\I\runs\20261119-144355-X-wonder1`: 1,738 presents, then `stall: no present for 15 s`, sound
  still playing; main thread in a host function), Mario Kart World stopped after its first present
  (`...-144153-X-mkw3-peek`, shot requested at 39.6 s, first present 50.9 s, then none). Repro: any game,
  `py scripts/gamectl.py <pid> fullres x.png`. Lane X runs use the window grab (`NXRS_SHOT_CMD=shot`, play_scenario)
  until it is fixed: the capture has to be recorded into the present's own submission (copy + read back after its
  fence) instead of a one-off from inside the present.
- **Mario Kart World's 20-60 s black boot is `nvnTexturePoolRegisterTexture`.** Between the first submit (4-5 s) and the
  first present (25-69 s, varies run to run) every core is idle. The `threads` control command at 16 s and again at
  26 s (`build\I\runs\20261119-151619-X-mkw5\boot15.txt`, `boot25.txt`; host function names now in the dump) shows
  the same picture both times: thread 0x1065 inside `nvn:nvnTexturePoolRegisterTexture` (lr main+0x4c9ab8, the
  game's texture loader), the render thread 0x1004 inside `nvn:nvnQueueSubmitCommands` (waiting on graphics, not on
  the guest), a loader thread on a guest lock, the rest idle. The log meanwhile has one `[vk] uploading a 85 MB
  texture from its storage: 8192x8192x1 format 0x50` line and nothing else for 30-50 s. The game registers ~131k
  texture IDs (`[RED] texture pool IDs up to 131372 pass the pool set's 65536`). Next for G: time
  RegisterTexture (per call / the 85 MB one, ASTC 8x8 → transcode?) and whether submit waits on the same lock.
- MKW after the first present: a null `sead` heap allocation (`main+0x3e3550`, 0x58 bytes, heap `[[sp+0x90]+0x2a78]`)
  read at main+0xa66e0+0x2df5 (`read of 0xa0`) 4-5 s after the first present, in 3 of 4 runs; DKB faults at
  different places 30-35 s in (main+0x8a2d80 reading 0x100000001, main+0x26c620 reading 0x48) and once ran 89 s
  clean. Lane X is checking whether these are the guest's (exclusive monitor off for Switch 2) before blaming graphics.

## shaders (src/shaders)
Every shader of a game found at build time and translated to SPIR-V before the game runs. See src/shaders/README.md.

**Hand-over from lane G to lane S (2026-11-19, uncommitted, compiles, not yet validated on real modules):**
`src/shaders/src/mesh.rs` (new) turns a translated task / mesh kernel into a Vulkan task / mesh stage (SPIR-V 1.4,
`SPV_EXT_mesh_shader`): a new entry point clears the stage memory, stores the workgroup number (and copies a task's
payload in), calls the kernel, then emits mesh tasks (`min(word 1 of the outputs, 65535)`) or sets the mesh outputs
(all `max_v` vertices, the primitive count byte, at most `max_p`) and copies the SoA attribute blocks out (position,
point size, generic attributes by Location; per-primitive system values 8/9/10 as PrimitiveId / Layer / ViewportIndex
with PerPrimitiveEXT; per-primitive generic attributes written at the primitive's vertices; triangle / line / point
index arrays). `job.rs` `mesh_stage` info gained `vertex`, `primitive` (`[attribute, component]`, -2 point size, -1
position) and `memory` (the stage memory offsets); `prebuild.rs` applies the pass after the fixes (`mesh::VERSION` is
in a task / mesh job's `job_sha` only, so only those retranslate) and records a failure as `failed` ("mesh stage: …").
Lane S owns it from here: run it over MKW / DKB, spirv-val, fix what is refused. **What graphics consumes:** a module
whose entry point is TaskEXT / MeshEXT (json `execution_model` "task" / "mesh"); graphics puts task shaders in stage
place 3 and mesh shaders in 4 (sets 3 / 4, c0 storage tables 0x670 / 0x770 as nxnative's dmsh_abi.h); the draw's first
workgroup is 0 (Vulkan has none; a red line when a game asks for another).

**Calls** (all return `Result<shaders::Report, String>`; `say` gets progress lines; `Options::default()`: 4 translator
processes, 8 scan threads, the update its package holds):
- `shaders::prebuild_data(data, &Options, say)`: for the build. Reads `<data>/exefs/*`, the RomFS the data folder's
  marker names (`translator::package::open_data_dir`) and the add-on content installed there (`dlc::read_manifest` /
  `title_romfs`; call it after `dlc install`), writes **`<data>/shaders/`**. Needs `dmsh_xlate.exe` (beside the
  binary in `tools\`, `build\tools\dmsh_xlate\`, or `NXSHADERS_XLATE`) and `spirv-val.exe` (`NXSHADERS_SPIRV_VAL`,
  `VULKAN_SDK`, `C:\VulkanSDK\1.3.268.0`). A rebuild reuses every output whose inputs and tools are unchanged.
- `shaders::prebuild(game, out_dir, &Options, say)` (opens a folder / NSP / NUP itself) and
  `shaders::prebuild_game(&exefs_files, &romfs, out_dir, &Options, say)` (a game already open).
- CLI for testing: `nxshaders prebuild <game> --out <dir> [--jobs 4]`.

**Output** (`<data>/shaders/`; `LAYOUT` = `nxshaders 1`, refuse anything else):

| File | What |
|---|---|
| `manifest.tsv` | `#` lines (layout, translator chain, counts), a header, then per key: `key stage translated_as arch status spv_bytes source`. `status`: `ok` (spirv-val clean), `invalid` (refused by spirv-val; do not load), `failed` (no SPIR-V; `why` in its json), `library` (an NVN2 ray-tracing module: translated as part of the game's one `rt_library`, see below; no `.spv` of its own) |
| `spv/<key>.spv` | SPIR-V 1.3, entry point `main`, valid for `--target-env vulkan1.3` |
| `spv/<key>.json` | the reflection below; written after the `.spv`, so a key is ready when its json exists with `status: ok` |
| `pairs.tsv` | `vertex-or-mesh key, pixel key, var or prog`: stages of one program variation (pipeline prewarm) |
| `problems.txt` | files, or parts of files, the scan could not read |

**Shader key** (agreed; graphics' `nvn::prog::shader_key` computes the same at `nvnProgramSetShaders`): 16 lowercase hex
digits, the first 8 bytes of SHA-256(control bytes ‖ code area). NVN2 control section (magic `0x98761234`, version `0xc9`):
its first 0x48 bytes with +0x30..+0x34 zeroed, code area length at +0x44. NVN: its own length (u32 at +0x20) when in
[0x760, 0x880), else 0x880 bytes; code area length at +0x704. Code area = from `NVNshaderData::data`.

**Descriptor model** (agreed: a stage's set is its `NVNshaderStage`, as graphics numbers stages):
- set 0 binding 0: the translator's support buffer (Ryujinx `SupportBuffer`, std140: `alpha_test` uvec4, `is_bgra[8]`
  uvec4, `viewport_inverse` vec4, `viewport_size` vec4, `frag_scale_count` ivec4, `render_scale[73]` vec4,
  `tfe_offset` ivec4, `tfe_vertex_count` ivec4), one per pipeline, visible to every stage (render_scale 1.0, is_bgra 0).
- a stage's own resources: set = vertex 0, fragment 1, geometry 2, tessellation control 3, tessellation evaluation 4,
  compute 5 (NVN2 task 6, mesh 7; the RT library 5 plus its own set 0 bindings 1..4), bindings from 1 up. No two stages share a set; gaps take empty set layouts.
- the json's `bindings[]`: `{set, binding, type, count, name, resource}`. `type` is the Vulkan descriptor type
  (`uniform_buffer`, `storage_buffer`, `combined_image_sampler`, `sampled_image`, `sampler`, `storage_image`,
  `uniform_texel_buffer`, `storage_texel_buffer`); `count` 0 = a runtime-sized array (descriptor indexing).
  `resource` is what the game binds there:
  - `{"kind": "support"}`.
  - `{"kind": "cbuf", "slot": s}`: NVN constant bank c[s]. 0 = the driver segment (0xA00 bytes: each stage's storage
    buffer table, 16 bytes a slot {u64 address, u32 size}, vertex 0x110, compute 0x310, geometry 0x410, pixel 0x510,
    task 0x670, mesh 0x770; compute's dispatch size at 0x200 (NVN2 0x4) and uniform table at 0x210; nxnative
    `dm/gfx/nvn/dmsh_abi.h`); 1 = the shader's own constants (the json's `c1`, hex); 2 = the handle table (combined
    textures 0x20 + 8r, images 0x120 + 8r, separate textures 0x168 + 8r, separate samplers 0x568 + 8r); 3..16 = the
    game's uniform buffer slot s − 3; 24 = NVN2 push constants; 25..29 = NVN2 graphics stage banks (stage i reads push
    block 1 + i). `implicit: true`: read by the translator itself (c0's storage tables).
  - `{"kind": "sbuf", "cb_slot", "cb_offset", "flags"}`: a storage buffer whose {u64 address, u32 size} is at
    c[cb_slot] word cb_offset (cb_slot 0: the stage's table in c0); flags bit 0 = written.
  - `{"kind": "texture" | "image", "handle", "cbuf_slot", "slot", "type", "array", "format", "separate"}`: `handle` is
    Ryujinx's TextureHandle (texture word | sampler word << 14 | kind << 28; c2 words when `cbuf_slot` is -1 or 2);
    `slot` decoded (combined texture r, separate texture 128 + r, image r; -1 = bindless through `cbuf_slot`); `type`
    Ryujinx SamplerType bits; `array` 1 normally (1 << 20 = the texture pool by ID, 4096 = the sampler pool, 301 = every
    c2 handle word, 8192 = a uniform buffer's handles).
- Facts outside the SPIR-V: `execution_model`, `local_size`, `early_fragment_tests`, `tess_mode` (domain),
  `transform_feedback` (vertex / geometry), `mesh_stage` (task / mesh facts, below), `per_primitive_locations`,
  `mesh_draw`, `uses_frag_coord`, `frag_out_map`, `fetch1d`, `capabilities` (SPIR-V ids), `fixes` (SPIR-V edits
  applied), `notes` (the translator's).

**Task and mesh shaders (lane S, rev 4; VK_EXT_mesh_shader).** A key whose `stage` is `ts` / `ms` is now a real Vulkan
task / mesh module: `execution_model` `task` / `mesh`, `translated_as` `ts` / `ms`, SPIR-V 1.4 (entry point lists every
global; storage buffers are StorageBuffer `Block`s), `SPV_EXT_mesh_shader` + `MeshShadingEXT`, entry `main`,
`LocalSize` = the json's `local_size` (NVN2 control +0x58: 8 or 32 threads in the verification games). Its set is the
json's `set` (task 6, mesh 7: graphics remaps as for every stage). What graphics does for a mesh draw:
- **Pipeline:** task (optional) + mesh + pixel; no vertex input, no tessellation. A mesh module declares
  `OutputVertices max_v`, `OutputPrimitivesEXT max_p`, `OutputTrianglesEXT` / lines / points (`mesh_stage.verts`
  3 / 2 / 1). Outputs: `Position` (decorated `Invariant`), `PointSize` when the SPH enables it, generic attributes as
  `vec4[max_v]` at `Location n`, `Primitive{Point,Line,Triangle}IndicesEXT`, per-primitive `PrimitiveId` / `Layer` /
  `ViewportIndex` when the per-primitive SPH enables them, and per-primitive generic attributes as `vec4[max_p]`
  decorated `PerPrimitiveEXT` at the locations in `per_primitive_locations` (11 of MKW's 19 mesh shaders, 4 of OoT's 500; locations 0..3). Checked: 147 of MKW's pixel shaders made per-primitive at locations 0 and 1 are spirv-val clean (opt-in test, `NXSHADERS_TEST_BUILD`). **The pixel shader of such a pipeline must declare those inputs `PerPrimitiveEXT` too:** call
  `shaders::mesh::fragment_per_primitive(&pixel_spv_bytes, &locations) -> Result<Vec<u8>, String>` on the pixel
  module for that pipeline (it decorates the inputs at those locations, adds the capability / extension and makes the
  module SPIR-V 1.4); the same pixel shader in a vertex pipeline is used as built. Device features: `meshShader`,
  `taskShader`, `shaderDrawParameters`; limits used by the verification games (estimated, NVIDIA granularity): output
  memory ≤ 29,696 bytes (OoT), workgroup memory ≤ 23,124 bytes (MKW), task payload ≤ 3,200 bytes, all within the
  RTX 5090's 32 KB / 28 KB / 16 KB.
- **The draw's first workgroup** (NVN's `DrawMeshTasks(first, count)` and the `{count, first}` records of the
  indirect forms): a task shader, and a mesh shader with no task (`mesh_draw` present in its json), read it
  themselves. They declare a read-only storage buffer named `mesh_draw` (json binding `resource: {"kind":
  "mesh_draw"}`, in the stage's set after its other bindings) and a push constant block with one `uint` at **byte 12**
  (`mesh_draw.stride_push_offset`), the record stride in 32-bit words; the workgroup's number is
  `mesh_draw[DrawIndex * stride + 1] + WorkgroupId.x`. Graphics binds the game's NVN records there (the indirect
  buffer at its offset; stride = the record stride / 4, e.g. 2 for `{count, first}`), or for a direct draw a staged
  `{count, first}` record with stride 0; `DrawIndex` is Vulkan's draw index within a multi-draw (`vkCmdDrawMeshTasks
  IndirectEXT` / `…CountEXT`), so each record's first is read on the GPU, no CPU read-back. Push constant bytes 0..11
  stay graphics' own (pool bases, flags); the range already covers TASK | MESH (layout.rs). 3D draws (no first):
  bind a zero record. Without a binding the module cannot be used (it is not optional).
- A mesh shader with a task reads the task's payload (`TaskPayloadWorkgroupEXT`, `payload` bytes); the task launches
  `min(word 1 of its outputs, 65535)` mesh workgroups in x.
- Verification (lane S, `build/S/shaders/`): MKW 19 mesh + 7 task, OoT 500 mesh + 20 task, all spirv-val clean
  (`--target-env vulkan1.3`); DKB, MK8D, Z-A, Arceus and Violet have none.

**Ray tracing (lane S, rev 4).** NVN2 ray-tracing modules (stage 8: control +0x56 = 8, RTCore metadata at +0x60) are
not separate stages: every RT module of the game is linked (`shaders::rt`, nxnative's design) into **one** library,
key `rt_library` (`spv/rt_library.spv` + `.json`, `stage` `rt`, `translated_as` `cs`): the modules' code concatenated
at 256-byte boundaries, each entry region given a logical ID (positive 16-bit, contiguous within an entry's
continuations), the logical-ID MOV relocations applied by the translator, the named resources `coreParams` (0x140) and
`descriptorBaseArray` (0x1430) checked equal across modules. The translator makes it one **compute** kernel
(`LocalSize 8 4 1`) that runs the ray-generation entries and their continuations (hit / miss entries as callable
continuations) and traces rays with **ray queries** (`SPV_KHR_ray_query`, `RayQueryKHR`), so graphics needs
`VK_KHR_acceleration_structure` + `VK_KHR_ray_query` (not a ray-tracing pipeline or SBT: NVN2's RT model is the
continuation scheme the kernel emulates). Its json adds:
- bindings in **set 0** besides the support buffer: `rt_scene` (binding 1, type `acceleration_structure`: the TLAS
  graphics builds from the game's NVN2 acceleration structures), `rt_dispatch` (2, the dispatch parameters,
  `rt.parameter_words` words as the translator's RayTracingProgram lays them out: image classes from word
  `rt.image_class_base` (16, 8 slots), entry membership bits from word `rt.membership_base` (24)), `rt_errors` (3,
  fault words the kernel writes), `rt_instance_sbt` (4, the instance SBT records); resource kinds named after them.
  Its banks are in set 5: `{"kind": "rt_bank0"}` (c0 = the `coreParams` block at `rt.core_offset`) and
  `{"kind": "rt_bank3"}` (c3 = the descriptor arena `descriptorBaseArray` at `rt.descriptor_offset`: handles from
  +0x430, push words from +0xb98, nxnative `dmsh_abi.h` DMSH_RT_*); raw global memory as for other shaders (the
  storage buffer under cb_slot 255).
- `rt` (the translator's runtime layout: `version`, the four bindings, `parameter_words`, `image_class_base`,
  `membership_base`, `local_bytes` (continuation arena, from the frames), `local_bias`, `core_offset`,
  `descriptor_offset`, `entries` [{address, id, owner, kind 0 raygen / 7 hit / 8 miss, frame}],
  `as_slots_by_module`) and `rt_catalog` (the linked catalog: per module key its address, length, hashes and entries).
  A dispatch of an NVN2 RT program = one compute dispatch of `rt_library` over the launch size, with the program's
  entry IDs in `rt_dispatch`.
- The module rows of the RT keys have `status` `library` and a json `{"status": "library", "library": "rt_library"}`.
- Verification: none of the seven verification games has an RT module (only Nintendo Switch 2 Welcome Tour is known
  to: not on this PC). A synthetic library (3 modules: a ray-generation entry with a continuation, a miss and a hit
  entry; RET bodies) translates and is spirv-val clean for vulkan1.3; the linker's IDs, addresses and moves match
  nxnative's for the same records (unit test).
- Conventions: clip-space z as the game writes it (no GL to Vulkan depth rewrite: a game whose NVN depth mode is
  near = -w needs `VK_EXT_depth_clip_control` negativeOneToOne); origin upper-left, no Y flip. Reads past a bound
  uniform or storage range give 0 only with `robustBufferAccess2` (nxnative's Metal-only bound tables are off here).

**Device features the outputs need** (SPIR-V capabilities over the 184,350 valid modules of the seven verification
games): `shaderFloat64` (every module declares Float64), `shaderInt64`, `shaderImageGatherExtended`,
`shaderStorageImageReadWithoutFormat` / `WriteWithoutFormat`, subgroup vote / ballot / shuffle,
`shaderDrawParameters`, `geometryShader`, `shaderOutputViewportIndex` + `shaderOutputLayer`, `bufferDeviceAddress`
(PhysicalStorageBufferAddresses: raw global memory), `VK_EXT_fragment_shader_interlock` (fragmentShaderPixelInterlock:
declared by 101,276 pixel shaders), `VK_NV_geometry_shader_passthrough` and `VK_NV_viewport_array2` (4 geometry shaders).

**ZBIC (done, lane T):** the one decoder is `translator::loader::zbic`; `shaders::formats::zbic` re-exports it (the
reference vector moved to `src/translator/tests/data/zbic_sample.zbic`). `Nso::parse` reads ZBIC segments too.

## cli (src/cli, binary `nxrs-build`)
**Renamed (lane L, 2026-11-19; owner's decision): the CLI binary is now `nxrs-build` (`target\<dir>\release\nxrs-build.exe`),
not `nxrs`.** Every lane: switch your commands (`nxrs build …` → `nxrs-build build …`, `nxrs profile`, `nxrs doctor`,
`nxrs translate` likewise; contract v1 calls are unchanged apart from the name). `cargo build -p cli` still builds it
(the package is `cli`). An old `nxrs.exe` left in a target folder is stale: delete it. `nxrt-dev e2e` now looks for
`nxrs-build.exe` beside itself (`--nxrs` still overrides); `scripts/run_game.py` waits for `nxrs-build.exe` as a
compiler. The product is nxrs, the launcher `nxrs.exe`, the engine repo becomes nxrs-engine (the folder is renamed
later by the coordinator).

The build and the launcher contract (docs/LAUNCHER_CONTRACT.md). What a built game is, for the lanes that run it:
- `<outdir>\<name>.exe` is a loader with no runtime in it (a windowed program, 16 MB main stack): it sets `NXRS_DATA=<outdir>\<name>_data` and `NXRT_EXEFS=<data>\exefs` (unless already set), loads `<name>_code.dll` and calls its export `int nxrs_game_main(int argc, char **argv)`, which sets the patch switches and calls **`nxfront_main`** (a retail title: `sdk` plus `rtld` or `main.npdm`) or `nxrt_main` (the runtime's homebrew). argv is the exe's own (`--key=value` options pass through to nxconfig).
- `<name>_code.dll` = every generated chunk (compiled with os's `cpu.h` overlay first for a retail title) + `modules.c` + **`nxfront.lib` linked statically** (front + os + graphics + runtime; `nxrt.lib` for homebrew). Extra system libraries: `cc.rs` `SYS_LIBS` / `ENGINE_LIBS`, or `NXRS_LINK_LIBS`.
- `rt_patch_on[i]` is set before the entry from `patches.h` `RT_PATCH_SETS`: the set's default, or its environment switch (`0` off, anything else on).
- Toolchains: with Visual Studio, clang (`NXRS_CLANG`, bundled `clang\bin`, `NXRT_LLVM`, LLVM's folder, PATH) for `x86_64-pc-windows-msvc` and `nxfront.lib` / `nxrt.lib` beside nxrs (`NXRS_ENGINE_LIB`, `NXRS_RUNTIME_LIB`); **without it**, the bundled llvm-mingw (`llvm-mingw\bin` beside nxrs, `NXRS_LLVM_MINGW`) for `x86_64-w64-windows-gnu` and the engine built for `x86_64-pc-windows-gnullvm` in `gnu\` beside nxrs (`libnxfront.a`, `libnxrt.a`, `libwindows.0.52.0.a`; `NXRS_GNU_LIBS`). `NXRS_TOOLCHAIN=msvc|gnu` forces one. The gnu DLL imports only system DLLs and the UCRT.
- Shaders: the build calls `shaders::prebuild_data(<data>)` after staging the data folder (output `<data>\shaders\`) and writes the profile's nxnative shader tables (`pso_manifest.tsv`, `pso_pairs.tsv`, `shader_pairs.tsv`, `shader_facts.tsv`) into `<data>\shaders\profile\` for prewarm (lane G may read them). `--no-shaders` skips both.
- `nxrs-build translate <input> --out <dir>` keeps what `nxrt-dev e2e` calls (the translator's dev binary is now `nxrs-translate`).
- Toolchain default (lane L): MSVC only when Visual Studio, a clang and `nxfront.lib` beside nxrs-build are all there (`cc::msvc_ready`), else llvm-mingw when found: a release may ship only `gnu\`.

**Launcher (lane L, M9).** The launcher (D:\Projects\RecompLauncher, `nxrs.exe`) has a per-game engine (`library.json`
`"engine": "turbo" | "nxrs"`, Turbo for every existing entry) and runs nxrs-build itself with contract v2 for build,
Install Updates (`update-info --game`), DLC (`dlc sync`) and before Play; an nxrs build goes to `<build folder> [nxrs]`.
Contract v2 as implemented: docs/LAUNCHER_CONTRACT.md. **Keep these stable** (the launcher parses them): the
`@progress` / `@result` lines, `nxrs-contract <n>`, the `DLC: this package also holds add-on content` prefix, `UPDATE` /
`UPDATE error:` / `DLC error:` lines, `<name>_build*\gen\nxrs.gen.key` (how an imported build is known as nxrs's).
Verified 2026-11-19 (temp library, build\L): MK8D built/updated/DLC through the launcher and raced via
run_game (Booster Course seen by `aoc`); MKW built through the launcher, boots, then the run_game watchdog kills it at
27 GB private after 6 s (`[nvn] virtual pool ... mapped`: lane G's pool host-memory issue, not the launcher).
Release: the launcher's `scripts/package-release.py --nxrs-engine <this checkout>` ships `nxrs-engine\` (nxrs-build,
`engine.json`, `nxfront.lib`/`nxrt.lib`, `gnu\*.a`) and `toolchain\llvm-mingw\` (x86_64 only).

## profiles (src/profiles)
nxnative's 187 `dm/games` profiles + its shared patch tables, embedded as data (`games/<id>/game.tsv`, `patches.toml`; the owner's `turbo.toml`), and the lookup:
- `profiles::lookup(title_id: Option<u64>, main_build_id: Option<&str>) -> Option<Match { profile, build: Option<&Build>, how: How::Build | How::Title }>`; `by_title(u64)`, `by_build(&str)`, `get(folder id)`, `all()`. Build ids are lowercase hex; a key matches over the shorter of the two (≥ 16 bytes).
- `Profile`: `title_id`, `program_id`, `name`, `builds` (`version`, `display`, `modules` (name, build id)), `modules_of(build)` (load order), `memory` (`reserve`, `alias`, `stack`, `app_pool`, `used`, `sys_resource`, `stdalloc_cap`, `map_limit`), `settings` (fps_target, fps_max, op_mode, perf_mode, core_preempt, shared_font, device_api, game_args, game_env, …, with nxnative's defaults), `platform()` / `tick_hz()` (19.2 MHz Switch, 31.25 MHz Switch 2), `get(key)` for any other `game.tsv` key.
- Patches: `profiles::patches_for(module_build_id)` → every `Cut` / `Obs` / `Stub` / `Capability` keyed by that build, from any profile or `shared_patches()` (SDK, engine). A recorded `StubBody` places itself: `body.place(&Layout { area, state, base, others })` → words with relocations applied. `translator_patches(&[(module, build id)])` → the translator's patch-table text for the entries nxrs applies at build time (today the owner's Z-A 60 FPS cut).
- `player_settings()`: nxnative's settings.tsv rows. `source()`: the nxnative commit.
- Shader tables (`games/<id>/shaders.tsv.zst`, 187 profiles, 24.7 MB, imported by `tools/export_shaders.py`):
  `shader_tables(id) -> Vec<(file name, text)>` byte for byte; `prewarm_shaders(id) -> Vec<(key, stage)>` (the
  `pso_manifest.tsv` S/C rows in play order; nxnative's Metal pipeline keys left out).

## front (src/front, lib `nxfront`) and config (src/config, lib `nxconfig`) — lane I

**The game's entry.** `nxfront.lib` (staticlib: front + os + graphics + runtime) is what a game links; `modules.c`
calls `int nxfront_main(int argc, char **argv, const RtModule *mods, int count)` (same contract as `nxos_main`, which
it calls after its own setup) and the exe is a windowed program (`/SUBSYSTEM:WINDOWS /ENTRY:mainCRTStartup`); extra
system libraries: `setupapi hid mfplat mfreadwrite mfuuid` on top of os's list. `nxos-dev build ... --front` does
exactly this; **lane T: `nxrs build` must link `nxfront.lib` and call `nxfront_main` the same way** (the 11 KB loader
then has no console either: see src/cli/README.md). Without a redirected stderr the log goes to `<name>.log` beside
the exe. Arguments: `<data dir>` (default `NXOS_DATA`, `NXRS_DATA`, `<exe dir>\<name>_data`) and any number of
`--key=value` (an ini option for this run only, not written).

**The window** is the front end's (Win32, its own thread): it sets `nxos::nvn::FRONT_WINDOW` and calls
`nxgfx::set_window` before the boot, `window_resized` on WM_SIZE, `save_caches` at exit. Hotkeys: F1 controls page,
F3 window screenshot (`captures\screenshot_<time>.png`), F4 fps_limit 60 / unlimited, F5 diagnostic dump, F7 video
(`recordings\recording_<time>.mp4`, H.264 through Media Foundation), F8 `nxgfx::vk::shot::request` (game resolution,
`captures\fullres_<time>.png`), F11 / Alt+Enter fullscreen (kept in the ini), Esc quit.

**Options (`nxconfig`).** The front end opens `<exe dir>\<name>.ini` (every Turbo option, sections and comments,
Spanish aliases, settings_version 3 migrations, unknown keys kept under `[Other]`), applies `--key=value`, reloads
the file when it changes (once a second) and writes in-game changes. **Any crate reads values directly:**
`nxconfig::string(key)`, `f32`, `int`, `flag`, `tristate` (auto = None), `fps_limit_hz()` (0 / 30 / 60..240),
`frame_skip()` (fps_target 30 → 1, 20 → 2), `lsfg()` (on, multiplier, target; fps_target=120 → x2 at 120);
`generation()` changes on every change, `on_change(f)` calls back. Values are already in Turbo's ranges. Who applies
what:

| Options | Applied by |
|---|---|
| `docked`, `language`, `keyboard_text`, `audio_slowdown`, `memory_gb`, `null_trap`, `log`, `profile` | lane O / T (os boot and services, runtime memory) — read them from `nxconfig` at start |
| `resolution`, `dynamic_resolution(_min)`, `game_dynamic_resolution`, `shadows`, `fps_target` (frame skip), `fps_limit`, `forced_vsync`, `driver_exclusive`, `shader_thread`, `low_priority_shader_compiler`, `diag` | lane G (graphics) — read live (`generation()`), the F4 hotkey flips `fps_limit` |
| `fullscreen`, `mouse_*`, `button_layout`, `button_map`, `smooth_recording`, `recording_fps`, `lsfg*`, `lossless_*`, `metrics`, `seconds`, `shot` | lane I (front, input) |

**Control pipe** `\\.\pipe\nxrs_ctl_<pid>` (also `swr_ctl_<pid>`): one command per connection, reply text
(`scripts/gamectl.py`). Any crate registers commands with `nxconfig::ctl::register(name, help, |args| reply)`.
Front: status, shot, fullres, get, set, run, record, fullscreen, counters, quit; input: press, P2..P8, pads.
**Graphics: register `pause`, `resume`, `step`, `passdump`, `hide`... (Turbo vk_ctl.c) there.**

**Metrics** (`metrics=1` / `NXRS_METRICS=1`): `metrics-<time>.csv` once a second (frames, FPS, CPU, private memory,
then every counter's change). Any crate counts with `nxconfig::metrics::counter("name")` (a static `AtomicU64`;
graphics: draws skipped, textures dropped... as Turbo's 8f5410f), and pushes per-frame CSV rows with
`nxconfig::metrics::frame(row)` while `nxconfig::metrics::enabled()` (first row a `#` header) → `frames-<time>.csv`.

**Controllers (`nxos::input`, lane I).** The input thread reads XInput, DualSense / DS4, Switch Pro / Joy-Con / NSO
pads, WinMM; `nxos::services::hid` shows them as npads. The front end calls `set_keyboard`, `set_focus`,
`set_suspended`, `device_changed`; `shutdown()` at every exit path (bounded, zero rumble, lights off).

**Requests from lane I:**
1. **Lane G — frame generation hook:** LSFG passes run on the present path. Front identifies the player's LSFG 3.1
   set (`nxfront::lsfg::set()`: SPIR-V words and binding counts of mip, generate and the 23 passes, lsfg-vk order).
   Wanted from graphics: `nxgfx::vk::present::set_frame_generator(fn(&FrameGenCtx) -> u32)` called with the device, a
   command buffer, the presented image (and its previous one) before the blit, returning how many generated images it
   queued for presents in between (paced between real presents); or graphics runs the passes itself from the set.
2. ~~Lane O — guest clock rate, audio tap~~: done by lane O (`nxos::clock::set_rate`, `nxos::media::sink::set_tap`);
   the F7 recorder uses both (smooth_recording, AAC sound on game time).
4. **Lane G — frames for F7 recordings:** the window grab costs ~69 ms a frame through PrintWindow when the game is
   not in front (a screen copy otherwise), so a 60 FPS video repeats frames. Wanted: `nxgfx::vk::shot::set_frame_sink(
   fps, fn(bgra: &[u8], w: u32, h: u32))`, the presented picture read back asynchronously at most `fps` times a second
   while set; front feeds it to the H.264 writer.

## Pokémon bring-up (lane P, M6)

Builds: `build\games\{plza,arceus,violet}` (`nxrs-build build <package> -o <g> --outdir build\games\<g> --jobs 4`;
Violet's add-on content installed with `nxrs-build dlc install`). Runs: `build\games\<g>\runs\<time>-<label>\` (run.log,
screenshots through the control pipe). Engine fixes of lane P (uncommitted):
- **runtime `mem.rs`: the alias region is 64 GiB** (was 4): Horizon's size for a 39-bit space and Turbo's
  `g_alias_size`. Z-A's nninitStartup puts a StandardAllocator over 12 GiB of it (`StandardAllocator::Initialize(nullptr,
  0x300000000)`); with 4 GiB the SDK aborted in nninitStartup (sdk+0x134214). Address space only.
- **os `mem.rs`: MapMemory takes mapped alias-region memory as its source** (MapPhysicalMemory pages are Normal on
  Horizon): Z-A maps its thread stacks from its allocator's pages; before, every try was refused ("source is not heap")
  and the SDK aborted (sdk+0x13e81c).
- **os `profile.rs` / `mem.rs`: maps past the pool fail for every profile with `mem_map_limit on`** (nxnative's key,
  Violet / Scarlet), besides Turbo's two titles.
- **os `mem.rs`: MapMemory takes any mapped source** (Turbo checks none): Violet maps a thread stack from main's .bss
  (`MapMemory ... <- 0x70046fd000`) and aborted at 3.6 s.
- **os `services/net.rs`: the `bt` service** (IBluetoothUser; command 9 RegisterBleEvent gives an event that never
  fires): Violet 4.0.0 aborted at 2.7 s when sm refused it.
- **graphics `nvn/cmd.rs`: nxnative's host-lent command memory reclaim** (below, for lane G to review).
- **Diagnostics:** `nxrt::kernel::dump_threads()` (each guest thread: wait kind and address, the host function it is
  in, guest frames), logged by os's watcher at a stall (no present for 15 s) and printed by the control command
  `threads`; `NXOS_NVN_BT=<proc>` logs a proc's first 64 calls with guest frames (Turbo's `SWR_NVN_INTERVAL_BT`).
- **Mods (5A.6):** `profiles::mods`, `cli/src/mods.rs`, `os/src/mods.rs` (src/cli/README.md "Mods"); profiles'
  `games/<id>/engine.tsv` (hand-kept keys, read before game.tsv's: `fps60`, `present_interval`); os hands a profile's
  `present_interval display` to graphics (`nxgfx::nvn::window::set_profile_interval(1)`).

**For lane G — Z-A stops at ~49 s: NVN command memory, host-lent segments reach 1 GB** (2026-11-19 13:36,
`build\games\plza\runs\20261119-130103-boot3\run.log`). The game renders at 60 FPS (title, language, "Confirm choices"
screens crisp), then `thread 'guest 0x1012' panicked at src\graphics\src\nvn\cmd.rs:34:9` / `FATAL: panic in a host
function: NVN command memory: host-lent segments reached 1024 MB`. The lending starts at 2.97 s (`the game's command
memory ran short, the host lends a segment of 256 KB (1 times so far)`) and reaches 1024 loans by 11.5 s: about 128
segments a second, never given back. nxnative met the same (issue #2717, Arceus at frame 2,117) and fixed it in
`hle_nvn.c` `cmd_alloc` / `hostseg_take`: "a record that does not fit claims its position for command memory reclaim"
(docs/reference/nvn-objects.md, "A recording that runs short before it writes into the game's memory"): the lent pool
stays at 132 segments. **Lane P ported it (2026-11-19, `nvn/cmd.rs`: `hostseg_take` / `hostseg_written` / `hostseg_size`, a separate
`ctlseg_take` for control memory, never reclaimed), since it stopped all three games; lane G please review.** Also in that run: `[RED] transform feedback captures are not replayed yet` (6.9 s; Turbo's
skinned characters in Z-A need it), many `[RED] texture ID n: target 1 bound where the shader samples target 8` at 34 s
(the cube placeholders Turbo also logs), `nvnCommandBufferSetConservativeRasterEnable` / `SetShaderScratchMemory` stubs.

**For lane G — Z-A in gameplay draws a black world** (2026-11-19 13:58, `build\games\plza\runs\20261119-135546-run4`,
with the host-segment reclaim: the lent pool stays at 208 segments / 52 MB through 16,384 loans in 170 s, no stop).
The game runs title → language → name → the opening dialogue; from ~40 s on every screenshot is black except the UI
(`0008-t160.png`: only "A Talk" and the dialogue arrow). Turbo had exactly this (GAMEPLAY_INVESTIGATION.md, "Pokemon
Legends Z-A: the black world", 78ece7b): the colour-grading table is a 32x32x32 target drawn with one instance per
plane (the vertex shader writes the layer, `vs=b29f08619f379a83 inst=32`); a pass begun with `layerCount` 1 leaves 31
planes black and the tone mapping multiplies the world by black. Fix there: a pass has as many layers as every
attachment's view holds. Same commit, also needed by Z-A: transform feedback replay (`[RED] transform feedback captures
are not replayed yet` at 6.9 s; 116 captures a frame in the station, the skinned characters) and cube views of 2D arrays
of six layers or more. FPS in that run (another lane compiling): 44-52 in the dialogue, p95 22-35 ms.

## Switch 1 library (lane W, 5B)

Builds: `build\games\{mk8dw,smash,jamboree,sonic,agsob,bbounce}` (`nxrs-build build <package> -o <g> --outdir
build\games\<g> --jobs 4`). Runs: `build\I\runs\<time>-W-<label>\` through `scripts/library_play.py` (play_scenario's
harness plus long scenarios `mk8d_gp`, `mk8d_cups`, `probe`, `menus`, and `library_play.py memory <run>`: private
memory per minute and its slope from the metrics CSV).

**NRO modules loaded at run time (new, lane W; translator + cli + os):**
- translator `package::nro`: `collect(romfs)` / `collect_with(romfs, extra)` find every NRO the RomFS's NRRs register
  (loose `.nro` files; `extra` for archives), `scan(romfs, want, say)` finds them inside large files (stored or zstd
  frames), `tsv()` writes `<data>\nro.tsv` (name, base, size, NRO bytes, SHA-256, build id, path; `refused` rows for
  online SDKs, EOS). Online SDKs (export `EOS_Platform_Create`) are left out.
- translator layout: modules named `nro_<index>` (`layout::is_nro`, `order_with`, `layout_names`) follow `sdk` at
  2 MiB-aligned bases; `generate` takes them in the same map as the ExeFS.
- cli: collects before translating, adds them to the translated code, writes `nro.tsv`; registered NROs no loose file
  holds come from `<name>_build\nro\<sha>.nro` (cache), data.arc's index (shaders' `formats::smash`), then a scan.
- os `services/ro.rs`: `read_table(data)` at boot (those modules are not loaded from the ExeFS but are in the module
  window and the dispatch table), `set_base(&loaded)`; `ro::LoadModule` maps the NRO at its base by build id,
  copies the game's image, relocates against ExeFS + loaded NROs (load order), binds earlier modules' imports only it
  defines (`hle::stubbed_import`), calls DT_INIT; `UnloadModule` calls DT_FINI and binds back; `LookupSymbol` /
  `LookupModuleSymbol` see loaded NROs.
- **Package (lane W):** an update whose patch section has its own compression table relocates onto the base's stored
  (compressed) bytes (Jamboree 1.1.0 over its NSP / XCI); `app.rs` gives `Patched` the raw base then.
- **runtime `fiber.rs` (lane W, for lane T):** a fiber whose function returned runs its function again when switched
  to (from the top of its stack), as nn::os does; it was a fatal ("the fiber has finished"). Super Mario Party
  Jamboree's four job fibers (function `main+0x18d9b0`) return when a job is done and are switched to again for the
  next. `NXRT_TRACE=fiber` logs InitializeFiber, every switch and every return.

**For lane G — `fullres` screenshot freezes the game** (2026-11-19 14:51, Arceus, `build\games\arceus\runs\20261119-145110-run4`
and `-144521-run3`, engine built from the main tree at ~14:40). Right after `ctl: fullres <png>` / `[vk] screenshot of
the next present requested` at 30 s, no frame is presented again (`heartbeat: ... 790 presents` from then on); the stall
dump shows the game's render thread inside `nvn:nvnQueuePresentTexture` (`thread 0x100a ... pc 0x7f10001f60
(nvn:nvnQueuePresentTexture)`), every other thread in condvar waits behind it. The same build ran Arceus 160 s with
no hang before `fullres` was requested (`-135934-run2`, older graphics). The front's `shot` does the same now (it
asks graphics for the picture too): Arceus `-145805-run5` and Violet `build\gamesiolet
uns61119-145624-run3`,
`-150358-run4` froze in `nvnQueuePresentTexture` right after `ctl: shot` at 30 s. Lane P takes no screenshots before a
run's end meanwhile.
- **For lane G — MK8D in a 10-minute Grand Prix** (`build\I\runs\20261119-140731-W-mk8d-gp`, `build\games\mk8dw`,
  engine at 1c80457 + lane W): menus and HUD crisp, every race picture blue / purple with a black road and black
  sky patches (`0016-shot_race_2_min.png`, `0025-shot_race_11_min.png`; Booster Course's Paris Promenade the same:
  `20261119-143627-W-mk8d-booster\0016-shot_race_40_s.png`); 50.7 FPS over 120-760 s (only 196 of 641 s at 58+), stalls
  up to 2.5 s at race loads (`[vk] slow frame` lines); `[RED] GPU address 0x1669f0c00 (+0x750) is in no memory pool`
  x8 at 641-645 s (race 4 load), `[RED] texture ID 1212: target 4 bound where the shader samples target 8` at 674 s.
  Private memory is flat (3.93 -> 3.98 GB). The menu preview panel (Grand Prix / Time Trials) draws black while the
  H.264 pictures decode fine (`0004-a_first_Mii.png` of the booster run).
- **runtime `loader.rs` (lane W, for lane T / O):** a module's code region is reported (QueryMemory) as whole pages up
  to its read-only data, as on Horizon. The SDK's C++ unwinder (`nn::ro::detail::GetExceptionInfo`, sdk+0x1429b0 in
  Jamboree) walks the regions from a PC to its module's end; an unaligned `.text` end (Jamboree's sdk: +0x5e7780) left
  a free gap, the walk stopped and every C++ throw aborted (`nn::diag abort` from sdk+0x142bcc). os `ro.rs` does the
  same for NROs.

**For lane I — presses nobody scripted while the DualSense is connected** (2026-11-19, `NXRS_CONTROLLER_BACKGROUND=1`,
the game window not focused). With the DualSense read, the Pokémon games advanced through screens that need A before
any scripted press: Z-A reached "What is your name?" at 17.5 s with the first `press` at 60 s
(`build\games\plza\runs\20261119-130103-boot3`); Arceus reached it at 47.7 s with the first press at 70 s
(`build\games\arceus\runs\20261119-133724-boot1`); Violet at 22.7 s (`build\games\violet\runs\20261119-145624-run3`,
first press at 60 s). With `NXRS_NO_SONY_HID=1` and the same scripts nothing advances before the scripted presses.
Either someone pressed the pad during those runs or the pad reports a button it should not (A, or a stuck input when
the window is in the background); worth a `nxinput read` check. Lane P runs with `NXRS_NO_SONY_HID=1` meanwhile.
- **For lane G / I — a screenshot hangs the present while frame generation generates** (Super Mario Party Jamboree,
  `build\I\runs\20261119-153405-W-jamboree-menus3`): the game ran its opening movie at a 29 FPS base with built-in
  frame generation adding frames (`[vk] frame generation: built-in, base 29.3 FPS, output 57.7 FPS`); the control
  pipe's `fullres` at 39.97 s (`[vk] screenshot of the next present requested`) was the last present: frame 580, then
  `stall: no present for 15 s`, the main thread inside `nvnQueuePresentTexture` for good, every later `fullres` "no
  present within 10 s". MK8D's shots (no frames generated at 60 FPS) never hung. Lane W runs with `--option lsfg=0`
  meanwhile.
- **For lane S / G — Super Smash Bros. Ultimate v0's data.arc is not read** (`build\games\smash\smash_data\shaders\
  problems.txt`: `data.arc: zstd: Unknown frame descriptor`): in the 1.0.0 archive the file-system table at the
  header's +0x20 offset (0x35a253fb8) is stored **uncompressed** (it starts with its size, 0x24add70, then the counts;
  later versions have the `{0x10, size, stored size, ...}` header and a zstd frame), and its counts header differs
  (regions at +0x30, not +0x38). So only 77 shaders (main's) are prebuilt and the game misses hundreds at run time
  (`[vk] live shader ... translated ... 41 s after it was missed`, run `build\I\runs\20261119-153655-W-smash-menus`).
  The NROs do not need the index (lane W scans for them, 84 of 84 in 24 s). `romfs_peek` (translator example) dumps
  any RomFS range: `romfs_peek <nsp> data.arc 35a253fb8 80`.

**For lane G — Z-A: fatal write fault into an NVN pool page at ~60 s** (2026-11-19 15:39,
`build\games\plza\runs\20261119-153747-run5`, main-tree engine of 15:35): `[nxrt] FATAL: host access violation at
0x7ffbcae6ddfa = lifted 0x700679c6c0 (sdk+0x59c6c0) +0x3aa (write of 0x735b49000)`, x0 = 0x735b48b00, x2 = 0x970: the
SDK's memcpy (host code in a system DLL) writing across into page 0x735b49000 in the alias region, where the game's
NVN pools live (`uploads from storage ... at 0x6f6ee1000`). The game wrote there fine for 60 s at ~30-52 FPS (668 draws a
frame, `pools: 1191 vram (810 MB), 259 mirror`, `write-back 0.53 MB`); suspicion: a page `vk/lazy.rs` set PAGE_NOACCESS
after GPU writes, faulted from host code (not lifted code), so the read-back path never ran. The earlier run of the same
game (`-135546-run4`, older graphics) went 170 s without it.
Violet does the same at 35 s (`build\games\violet\runs\*-long1`: `write of 0x70e5c8df0` from sdk+0x581bd0, the
memcpy, called from main+0xdbd340); **with `NXGFX_WB_LAZY=0` the same Violet script ran 95 s without it**
(`*-nolazy`), so it is the lazy write-back's page (a fault on a page `bring_back` does not answer for, or a page made
inaccessible while the CPU copy into it runs).
- **os `hle.rs` (lane W, for lane O):** an import every importing module declares **weak** and nothing defines is no
  longer bound to the reporting stub: it binds to 0 as Horizon's rtld does (`ModuleSyms::weak_imports`). Super Smash
  Bros. Ultimate tests `nu::VirtualAllocHook` / `nu::VirtualFreeHook` before each call; bound to the stub (which
  answered 0) its allocator spun in nnMain with no frame drawn (`gap Import _ZN2nu16VirtualAllocHookEmmPv x598`).

**For lane G — Arceus: garbled text bubbles and smeared house textures in Jubilife Village** (2026-11-19 15:48,
`build\games\arceus\runs\20261119-153916-long1\m6.png`, a PrintWindow capture at 555 s). The opening, the beach and
the field (`m2.png`, `m4.png`) look right; in the village the speech bubbles' text is unreadable (glyph rows
displaced, as Turbo's "garbled UI labels" in Z-A before its GOB-order fix) and the house walls / roofs look smeared
(blocky, misplaced texel rows). 30.0 FPS (p95 34-35 ms) throughout the 600 s run.
- **os `profile.rs` (lane W, for lane O):** a title no profile describes takes its system resource size from its
  NPDM (META +0x14) instead of 0, so its virtual address memory works: Anime Girls: Sun of a Beach, Beach Bounce
  Remastered and Sonic X Shadow Generations (16 MB each) crashed in their static initialisers in
  `VammManager::AllocateAddressRegion` (read of 0xc0; Beach Bounce printed "IsVirtualAddressMemoryEnabled() is
  false"). A profile's `sys_resource` still wins (Jamboree's 0).
- **scripts `gamectl.py` (lane W, coordinator's rule):** `pipe` and `key` refuse unless GAME_LOCK's lane is
  `NXRS_LANE` and its executable is the target PID's image (`check_owned`); `find` is for looking only.
  play_scenario stops a scenario at the first refusal (the game exited or the lock is another lane's).
  **Update (lane W):** not frame generation: Smash (`build\I\runs\20261119-155110-W-smash-menus2`, `lsfg=0`) and the
  first MK8D run (`20261119-135517-W-mk8d-gp`) hang the same way when a `fullres` request is pending at a present
  (the first present for both): the main thread stays in `nvnQueuePresentTexture` (`stall: no present for 15 s (frame
  1)`). Likely path: `queue.rs present_locked` (holding `g.q.sub`, on the async replay thread) -> `shot::on_present`
  -> `capture` -> `Gpu::read_texels` -> `queue::oneoff`, which calls `aq_drain()` and locks `g.q.sub` again.

**Lane P status and hand-off (2026-11-19 16:15).**
- Arceus (`build\games\arceus\runs\20261119-153916-long1`, PrintWindow captures `m*.png`): boot, the opening, the
  beach and the Obsidian Fieldlands field with the player running (`m4.png`), Jubilife Village by 555 s (`m6.png`,
  garbled text: lane G); 30.0 FPS (p95 34-35 ms) for 600 s, no stall. Its build carries lane P's test mod
  `mods\arceus60` (on by default: present interval 1, double speed; runs use `NXRS_MOD_ARCEUS60=0`).
- Violet (`build\games\violet\runs\20261119-155739-long2`, `NXGFX_WB_LAZY=0`): the academy opening (`0002`, `0005`) to
  the bedroom and the Adventure Guide (`0010-t600.png`) in 600 s; 30 FPS at first, 6-15 FPS in the real-time opening
  (slow frames: `prep` 53-93 ms with 22-32 ms texture uploads: lane G), 23 FPS at the end. Next: walk out of the
  bedroom (left stick), the first town.
- Z-A (`build\games\plzauns61119-160759-run6`, main-tree engine of 15:35, `NXGFX_WB_LAZY=0`): title, opening,
  **Lumiose City lit and populated with the starters** (`0006-t240.png`; the black world above is gone in that build),
  no crash in 240 s; 60.0 FPS on the title, 35-46 FPS in the city (p95 22-64 ms, slow frames up to 0.87 s): short of
  M6's 59. With the lazy write-back on it faults at ~60 s (item above).
- How lane P runs: `scratchpad` scripts are not in the tree; the equivalent is `py scripts/run_game.py` with
  `NXRS_LANE=P`, `NXRS_NO_SONY_HID=1` (see the lane I item), `NXOS_SWKBD=auto` (else the name box waits for a person),
  presses through `scripts/gamectl.py <pid> press A~0.2`, and PrintWindow captures (the graphics-layer `shot` /
  `fullres` freeze the game today).
