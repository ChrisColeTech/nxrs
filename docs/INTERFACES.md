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
  (pool set size, 262144 / 4096), `NXGFX_SHADERS`, `NXGFX_CACHE`, `NXGFX_POOL_LOG=1`.
  Added by lane G (rev 4): `NXGFX_SHADER_WAIT_MS` (3000: a draw's longest wait for the live translator; 0 = skip),
  `NXGFX_LIVE=0` / `NXGFX_LIVE_JOBS` (live translator off / threads, 4), `NXGFX_WB_LAZY=0` (write-back on demand
  off), `NXGFX_APPEND_RESET=0`, `NXGFX_A2C=0`, `NXGFX_SLOW_MS` (50), `NXGFX_FRAME_CSV=<path>`,
  `NXGFX_PCACHE_QUIET_MS` (2000), `NXGFX_HEADLESS=1` (no host window), `NXGFX_PREWARM_THREADS` (4).
  Added by lane G3 (rev 7): `NXGFX_DESCBUF=0` (classic descriptor sets instead of descriptor buffers), `NXGFX_GPL=0`
  (whole pipelines only, no linking from parts), `NXGFX_PROFILE=1` (the fine timers inside `sets`), `NXGFX_ASYNC=0`
  (replay on the game's thread; on by default now), `NXGFX_ALIAS=0` (no image shared between texture objects over the
  same storage), `NXGFX_IMG_FORMAT=0` (storage views in the texture's format, not the shader's declared one),
  `NXGFX_PIPE_LOG=1` (each pipeline compile logged as it starts and ends), `NXGFX_PIPE_NOOPT=1` (whole pipelines with
  DISABLE_OPTIMIZATION).
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
- **(Superseded by rev 7: asynchronous submission is on by default, see lane G3.)** Asynchronous submission was off by default: with it MK8D stopped submitting ~120 s
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

**Lane G, rev 6 (2026-11-19, uncommitted; lane P's five items):**
- **Lazy write-back (`vk/lazy.rs`) is off by default** (`NXGFX_WB_LAZY=1` turns it on). The fault it caused is fixed
  as well: a page was taken out of the lazy set before it was readable again, so a second thread touching the same
  page in that window got a fatal fault (Violet ~35 s, Z-A ~60 s). Now the page stays marked as being read back, other
  threads' faults wait for it and retry, and a fault on a page that is already back simply retries.
- **Screenshots no longer freeze the game** (`shot`, `fullres`, F8, `NXGFX_SHOT*`): `queue::present` takes the
  screenshot before it locks the submission state (the capture's own submission locked it again: deadlock, the
  game stopped in `nvnQueuePresentTexture`). Shots requested while a queue is parked go to the next present.
  Checked: `gamectl fullres` twice on lane G's Violet (`build/G/runs/v2/fullres1.png`, `fullres2.png`); the game
  played on at the same rate. (Lanes W / X / P saw the freeze on builds without this change.)
- **Review of lane P's host-lent command memory reclaim (`nvn/cmd.rs`): OK.** It matches nxnative's design.
  Parked and queued submissions copy their records, so a reclaimed segment is never replayed late. What is left: a
  segment with no guest range (an overflow by a command buffer that is not recording, or at the very end of the
  game's memory) is never reclaimed (red line once). Control memory is never reclaimed, with a panic at 256 MB; it
  can reach that in a long session if a game keeps giving none.
- **Built-in frame generation quality** (`fg_interp.comp`): the "no motion" vector is tried first, matches are judged
  over a small patch rather than one pixel, and where no vector fits, the nearer real frame is shown unwarped instead
  of a warped guess. Lane P's Arceus captures (`m6.png`: torn speech-bubble text, smeared house texels) were taken
  with frame generation on (base 30 -> 60): a generated frame, most likely. Lane P: compare a capture with
  `--option lsfg=0`.
- **Write watch across allocations** (`vk/mem.rs`): a mirrored pool spanning several allocations had no write watch,
  so all of it was compared with a snapshot at every submission (`prep` 40-50 ms a frame in Violet with 0 uploads).
  When the whole range fails, the watch is now asked per allocation (the whole range is still tried first: a region
  walk on every submission cost MK8D 1.9 s a frame in `r29`). The pools line now ends with `N mirrors without write
  watch (MB)`. The texture check after CPU writes takes each texture once, not once per dirty page under it, and
  looks only at the dirty ranges over it. `prep` in the replay line is split into `pools` and `texture hashes`.
  Results: Violet `build/G/runs/v5` holds its 30 FPS cap from 20 s to 150 s, `prep` 3 ms a frame (was 40-65 ms,
  `v2` / `v3`: 11-25 FPS). MK8D `r30`: races at 54-60 FPS from 81 s to 281 s (were 41-47, and 22-26 in `r28`).
- **Descriptor work** (`vk/exec.rs`): a stage's c0 / c2 driver banks are rebuilt and staged only when a bind for that
  stage changed them (Z-A: ~1,700 draws a frame, `sets` 7.9 ms a frame).
- **Z-A (item 5) is still 35-46 FPS in Lumiose** (`build/G/runs/z1`, `z2`; picture right, `z2/lumiose.png`). Upload
  stalls are gone; what is left is the replay's descriptor work: ~1,640 draws and ~2,170 new descriptor sets a frame,
  `sets` 10-12 ms of 15-19 ms of replay CPU a frame. Caching the staged constant blocks (support buffer, c1, push
  banks, c0 / c2) did not move it; the cost is the set allocations and writes. Next: push descriptors or descriptor
  buffers for the stage sets, or the replay on its own thread (async, off since MK8D's stalls).
- MK8D with all of rev 6 (`r31`, `r32`): races and battle at 58-60 FPS, race start included; the race picture is
  right in lane G's build (`r28/shot-006400.png`, `r32/shot-005000.png`): lane W's tint and black road came from a
  build without these changes or with frame generation; worth one more look on the main build once it is rebuilt.
- **Texture uploads decode on several threads** (`vk/image.rs decode_many`): with 4 or more textures (4 MB or more)
  due in one submission, their storage is read and de-tiled on up to 8 threads before recording (Z-A's streaming
  bursts: 200-370 ms frames).
- **Guest memory reads (`vk::mem::guest_bytes`) use ReadProcessMemory on the own process** instead of VirtualQuery:
  VirtualQuery scans a large uniform reservation page by page to size its region, ~2.5 ms a call in Violet (its
  texture checks: 60 ms a frame). Pages with no storage still read as zeros.
- **ASTC decoding allocates no buffers per texture** (`Sub::gpu_scratch`, 64 MB device-local chunks kept with the
  submission; the BC7 copy reads the blocks in place). Image initialization and uploads of one submission share two
  barriers instead of two each.
- **MKW boot stall (lane X):** the first submission's set-up took 11.4-11.7 s (29,331 images, 10 s of it two buffer
  allocations per ASTC level) and the first frame came at 19-38 s; now 2.0 s and 9.6 s (`build/G/runs/w11`). A
  submission's set-up over 1 s is logged with its parts (`a submission's set-up took …`, `textures at a submission: …`).
- **For lanes X / O, MKW's crash after its first frame:** with a PDB (`w11`), the fault is
  `nvnCommandBufferWaitEvent` called with a **null command buffer** from main+0xa0040 (`cmd.rs` reads `o.cur` at
  offset 8: "read of 0x8"); the other runs' read of 0xa0 at main+0xa66e0 comes from the same caller tree. Graphics
  only sees the null; the object that should hold the command buffer is the null heap allocation X found. Not
  changed by the graphics fixes above. A graphics cause would be sizes the game budgets its heap from
  (`nvnTextureBuilderGetStorageSize`, pool and command-memory sizes on NVN2); not checked yet.
- **Compressible textures (render targets) are not uploaded from storage** on creation. MK8D's dynamic resolution makes
  new targets of 1880x1057 .. 1920x1080 every frame, and each was uploaded: 20-50 ms a frame in races (`r28`).

**Lane G3, rev 7 (2026-11-19, uncommitted).** Runs in `build/G/runs/` (`z3`..`z7`, `a1`, `m2`, `w12`, `x1`, `x2`,
`v6`..`v8`) and `build/I/runs/20261119-183639-G3-mk8d-movie`; lane G3's run scripts are `build/G/g3/run.sh <game> <run>
<s> [NAME=VALUE ...]` (its own links in `build/G/<game>t`, relinked when `target/G` is newer, saves backed up) and
`build/G/g3/press_run.py` (`PRESS=0`, `CTL="t:command;..."` control commands at given seconds).
- **Descriptor work (Z-A, item 1).** Measured first (fine timers, now opt-in with `NXGFX_PROFILE=1`, they cost 1-2 ms a
  frame themselves): of `sets` 9-12 ms a frame, allocating and writing the ~2,150 sets was 1.5-1.8 ms, rebuilding the
  c0 / c2 driver banks 2.5 ms, texture views 1.5-2 ms, memory lookups 1 ms, set keys 0.5 ms. `tests/desc_bench.rs`
  (ignored tests): 2,170 classic sets of 15 uniform buffers 1.8 ms; the same descriptors by `vkGetDescriptorEXT` 0.12 ms.
  Push descriptors do not fit the layout (32 descriptors at most in the one push set a layout may have; a stage set has
  176 bindings and a draw binds two to five sets). **Descriptor buffers** (VK_EXT_descriptor_buffer) are on when the
  device has them (`NXGFX_DESCBUF=0`: classic sets): the stage sets are written into a per-submission ring (4 MB
  chunks, one draw's sets in one chunk, no key / hash), the pool set is a mapped buffer written in place at
  registration (texel buffers by address), every pipeline of the shared layout has the descriptor-buffer flag, buffers
  a shader reads have device addresses (`Buf::addr`, pools' in `mem::buffer_address`). Also: c0 and c2 have generations
  of their own (a uniform-buffer bind, the most common, changes neither for a graphics stage), c2 is rebuilt only when
  its handles changed, a stage's texture / image descriptors are kept while its handle bank, pools and submission stand
  (`TexPart`), the pool lookup has a per-thread cache (`mem::resolve`, invalidated by `POOL_GEN`, overlapping pools
  respected), neighbouring pools share one `GetWriteWatch` call, vertex input and vertex buffers are not set again when
  unchanged. **Result (`z5`, sync, descriptor buffers on):** Lumiose 42.5-48 FPS, median 46.6 (was 34-38, `z3` /
  `z4`); replay CPU 11 ms a frame (was 15-17), `sets` 5.7 ms (was 9-12), `prep` 3.2 (pools 2.5), vertex input 1.4;
  the game waited 2.5 ms a frame in SyncWait.
- **Z-A and the replay thread.** The game waits in SyncWait for each frame's work (`the game waited X ms per frame
  (sync, acquire, finish)` at the end of the replay line), so the replay's CPU time adds to its own whichever thread runs
  it; with the replay on its own thread its next frame's CPU work still overlaps part of it: `z6` (async, rev 7)
  49.5-52.4 FPS against `z5` (sync) 42.5-48; it waited 16.3 ms a frame. 59 FPS needs the replay (~10.5 ms) and the
  game (~12 ms) to overlap more or the replay to shrink further.
- **Asynchronous replay (item 2): on by default now** (`NXGFX_ASYNC=0`: on the game's thread). The bug found: with ops
  queued, a fence the game took (`nvnQueueFenceSync`) named the latest op of any queue, and resolved to the timeline
  value after it even when that op's queue had parked at an event wait: a SyncWait passed before the parked work ran
  (GPU test `event_wait_parks_a_queue_until_another_queue_signals` failed with async on). Now a fence names its own
  queue's last op (Turbo's `last_seq`), and an op whose queue is parked afterwards resolves through a receipt of that
  queue (when its parked work has run). Also from Turbo: at most 64 ops and 2 presents queued (the game's thread waits
  in the OS's blocking hook), a SyncWait on a sync no queued command buffer has fenced yet waits only with a timeout
  and only until it is fenced or the queue runs dry. Runs: MK8D's demo 300 s at 60 FPS (`a1`, before the receipt fix;
  r22 / r24's stops did not come back), Z-A `z6` above, and with the receipt fix Z-A `z7` 49-52.4 FPS for 58 s (then its window was closed from outside, as in `v9`: lane G3's runs now set `NXRS_CLOSE_QUIT=0`).
- **MK8D's menu preview (item 3) fixed.** `nvnTextureWriteTexels` (the movie decoder's frames) writes the texture's
  storage, swizzled (Turbo's `write_texels`), and every image over that storage uploads from it again; it wrote only the
  one object's image, and the menu samples its own texture object over the same memory. Picture:
  `build/I/runs/20261119-183639-G3-mk8d-movie/0004-a_first_Mii.png` (the Grand Prix panel plays the race movie). GPU test
  `write_texels_reach_every_texture_over_the_storage`.
- **Wonder's long frames (item 4).** Lane X's run: 63 frames over 50 ms in 240 s, 6.2 s of pipeline creation inside them
  (single frames of 0.3-2.1 s at new places), texture streaming 40-60 ms, and the 2.8-3.0 s frames at 200 / 263 / 322 s
  were its own pass dumps (`NXGFX_DUMP_AT_S`). Pipelines are now linked from parts (VK_EXT_graphics_pipeline_library,
  Turbo's `NXRT_VK_GPL`, here on by default: `NXGFX_GPL=0` off): a new combination of known shaders links in about a
  millisecond, the optimized whole pipeline is built on two background threads and replaces it; and a program's shader
  parts are made ahead on two background threads when the game sets it (`nvnProgramSetShaders`, for the depth formats
  and sample counts seen lately; within the prewarm's memory budget). `x1` (parts, nothing made ahead): 22 slow frames,
  pacing windows under 58 FPS 3 of 44 (lane X: 17 of 44); `x2` (1,740 parts made ahead): 17 slow frames, mean 59.2 FPS,
  windows under 58 3 of 44. What is left is the first entry into a level (26-29 s: 0.3, 2.5 and 0.26 s frames of
  shaders never seen before, compiled at their first draw); a second session prewarms them from the key file.
- **Lazy write-back (item 5).** `v6` (lazy on): no fault in 150 s, but 10-13 FPS: an eager write-back landing on a page
  held on the device brought it back first, which made the completion thread wait for all GPU work submitted since
  (48 ms of write-back a frame). Such pages now stay on the device (its buffer has the newest bytes, this write-back's
  among them). The pools line shows `lazy write-back: N pages on the device, … brought back …` when it is on. `v7`
  (lazy off): 30 FPS, Violet's cap. `v8` (lazy on): clean for 150 s (no fault), but 18-23 FPS: 29 ms of write-back a
  frame with no page brought back: the pages already held were protected again at every completion (one VirtualProtect
  a page, ~415 a frame). Now only newly held pages are protected, in runs of neighbours (page by page when a run spans
  two allocations). `v9` (that fix, lazy on): no fault in 58 s (the window was then closed from outside, as lanes W /
  X2 saw), but still 20 FPS and 28 ms of write-back a frame with 466 pages held and none brought back: the cost is in
  the completion path's lazy bookkeeping or the landing beside it, not found yet. **(Rev 8: on by default, see below.)** Lazy write-back stayed off by
  default: clean in Violet now (`v8` 150 s, `v9`), slower than copying back (30 FPS in `v7`).
- **MKW null command buffer (item 6; lane X2's root cause: the render-buffer pool fills).** Graphics now answers NVN2's
  device integers as Turbo does (`DEVINFO2`, 0xAA items, and the constants of the driver compiled into the game's sdk:
  `nvn::device::set_sdk_image`, called by os with the sdk module's image; MKW's sdk gives API 231.39, GPU code 201.20);
  MKW still faults the same way (`w12`). The storage sizes lane X2 measured are the reference rule's: Turbo's
  `nvn_tex.c`, nxnative's and mkw-recomp's `runtime/gfx/nvn/tex/nvn_tex.c` (MIT) all pick the block height from
  height x 1.5 / 8 GOBs and align to 512 bytes, as `tex/layout.rs` does (2560x1440 RGBA8 = 0xf00000: 1440 rows in
  128-row blocks = 1536; the 3072x3072 one is the shadow map, 3 layers of D16 = 0x3600000); the only NVN goldens anyone
  has are the X1 SDK's (mkw-recomp `probes/nvn_tex_probe.c`), which this layout passes. 2560x1440 is the console's own
  docked scene size (mkw-recomp: "the 3D scene still renders at the console's docked 2560x1440"; MKW 1.8.0 takes it
  from its display object, not from what the host reports). **The same pool exhaustion is mkw-recomp's issue #255 /
  #210 / #305 on MKW 1.8.0** (ours is 1.8.0: its allocator init is `main+0xeb1870`, mkw-recomp's `rb_alloc_init` for
  1.8.0), and mkw-recomp fixes it in the game, not in sizes: `recomp/patches-mkw-180.txt` (lines 188-195) gives the
  render-buffer allocator's pool table two slots (`main+0xeb199c` `mov w1, #8 -> #16`, `main+0xeb19b0` `mov w9, #1 ->
  #2`) and routes `bl main+0xeb1870` at `main+0xd65720` and `bl main+0xb72c0` at `main+0xb6f90` to host functions, and `runtime/core/boot/patches.c`
  (`host_shadow_pool`, `host_shadow_rb`, `shadow_second_pool`, `second_pool_bytes`) fills the second slot, when the
  first shadow map is made, with a pool from memory of the runtime's own under a `pead::ExpHeap` of the game's class
  (1.8.0 `expheap_create` `main+0xbeb4e0`, `rb_create` `main+0xb72c0`, `rb_pool_init` `main+0xeb1e60`): 128 MiB for
  1.8.0's photo mode at 1440 lines plus one more 3072 shadow map. **For lanes X / O (the game-patch layer): port that
  patch (MIT); graphics has nothing to change there.**
- **5A.4 rows:** raw global memory (`vk/gmem.rs`: the 8,192-entry memory pool table with 2 MiB hints and the write log
  under constant slot 255, pools copied back after a shader wrote them through a loaded pointer, at most 64 MB each);
  resolution scale and the host's dynamic resolution (`vk/scale.rs`, the module's notes; GPU test
  `resolution_scale_renders_at_twice_the_size`); `nvnCommandBufferResolveDepthBuffer` (the pass ends, a barrier: the
  depth target's writes visible to later reads; Turbo leaves it a stub); per-title texture rewrites are every title's
  here (CPU writes to a texture's storage reload it for all games, Turbo's two titles included). **Ray tracing: not
  done.** NVN2's ray-tracing entry points are known by name from MKW's sdk (`CommandBufferDispatchRays` / `…Indirect`,
  `CommandBufferBuild` / `Copy` / `Compact` / `Relocate` / `Bind…AccelerationStructure(s)(Deferred)`, `Device…
  AccelerationStructure…`, `ProgramBuilder…RayTracing…`); graphics has the memory-requirement query and the queue
  builder's sizes, not the rest. Turbo has no ray-tracing path either (its prebuild skips `rt` keys). What is left: the
  argument layouts of the builds, binds and dispatch (from the sdk's own code, or a game that uses them: none of the
  owner's do), VK_KHR_acceleration_structure + VK_KHR_ray_query on the device, BLAS / TLAS builds from the game's
  geometry, and a dispatch as one compute dispatch of lane S's `rt_library` with its set-0 resources (`rt_scene`,
  `rt_dispatch`, `rt_errors`, `rt_instance_sbt`) and banks (`rt_bank0`, `rt_bank3`).
- **Texture pool set: 262,144 IDs** by default (`NXGFX_TEX_CAP`; MKW registers up to ~131,400).
- **Review of lane X2's write-back landing (`vk/queue.rs` "write-back base", `vk/mem.rs land_changed`): correct.**
  The base is copied in `pre` after the submission's uploads, so it is the device as the GPU work found it; only bytes
  the work changed land, the rest keeps what the CPU wrote meanwhile (Turbo's `wr_land`), pages the GPU left alone are
  not touched, and the write watch / snapshot are kept consistent around the landing. Costs: every write-back is read
  back twice (the base and the result) and compared byte by byte; fine at MK8D's 34 small write-backs a frame. Pages
  held on the device by lazy write-back no longer reach it (`lazy::outside`, above): bringing them back there made the
  completion thread wait for all later GPU work (`v6`: 48 ms of write-back a frame in Violet).
- **Lane W2's picture bugs (rev 7):**
  - **Beach Bounce's black final copy: fixed.** The game draws its scene into one texture object (ser36) and samples
    it through another over the same storage (ser37, other flags): two images, the second one never drawn (pass dump
    `build/G/runs/b1`: pass 44 samples ser37 while passes 0-43 draw ser36). A texture object over the storage of a live
    image of the same shape (size, levels, layers, samples, bytes per texel) now views that image (`image.rs`
    `alias_image`, `Image::alias_of`; the owner's memory passes to an alias when the owner is finalized; resolution
    scale follows the owner; `NXGFX_ALIAS=0` off). `b2`: the hospital scene presents.
  - **Shadow Generations' false colours: fixed.** Its last pass is a compute shader that loads the RGBA8 window target
    as `r32ui` (a raw surface load, SULD.B) and stores it formatted; graphics bound an RGBA8 storage view to the
    `r32ui` binding. A storage image's declared integer format (json `format`) now picks the view format when it is
    the same texel size (`Res::Img::fmt`, `exec.rs tex_view`; `NXGFX_IMG_FORMAT=0` off). `s8`: the title screen is
    right (`s6`, before: yellow / grey / navy).
  - **MK8D's race picture: not fixed, cause narrowed.** Same build, different runs: `build/I/runs/…-225612-G3-mk8d-race`
    clean; `…-232203` / `…-232536` (lsfg 0) with the road flat cyan and banded light; `…-232813` / `…-233028`
    (`NXGFX_ASYNC=0`) cyan patches only where kart and item shadows fall; `…-233249` / `…-233504` (`NXGFX_GPL=0`) one
    clean, one cyan. It is not run-to-run: the cyan / banded shots are all at one place of the lap (the turn past the
    dark wall), the clean ones earlier; the picture there is wrong whatever async, GPL or descriptor buffers. A pass dump
    at a clean spot (`build/G/runs/k1`, 82 passes) shows what to look at next, at the bad spot: passes 37 / 38 (point
    lights, additive) bind **the same image ser157177 as colour targets 0 and 1** and some draws write both (output map
    0xff): two Vulkan attachments over one image written in one subpass is undefined (the game may mean two views, or a
    second texture over the same storage); and 0x15cf60600 holds four render targets in one frame (A2B10G10R10 normals,
    two RGBA8, a 960x540 one): the transient pool the game reuses within the frame, which the aliasing above now keeps
    as one image where the shapes match. Not fixed in this pass.
  - **Donkey Kong Bananza's compiler crash: isolated, not fixed.** With `NXGFX_PIPE_LOG=1` (new: every pipeline the
    driver compiles, as it starts and ends) the last one started in `build/G/runs/d3` is vertex `d87ccbede95b9f2c` +
    pixel `6655320127e098d6`. It crashes outside the game in a second: `NXGFX_GPL=0 NXGFX_TEST_SHADERS=<dkb>_data/
    shaders NXGFX_TEST_PAIR="d87ccbede95b9f2c 6655320127e098d6" cargo test -p graphics --features gpu-tests --test
    shaders_lane_c one_program_pair -- --ignored` (new ignored test). Both modules pass `spirv-val` (all 22,854 of DKB's
    do), the validation layer says nothing, `DISABLE_OPTIMIZATION` does not help, and each module builds with other
    partners (pixel `6655…` with vertex `0885…` / `1899…`, vertex `d87c…` with another pixel shader): only the pair
    crashes, so it is the driver's cross-stage step on this interface (10 vec4 varyings, location 5 flat, a
    gl_PerVertex with `float[70]` after ClipDistance[2]: the same in the partners that build). Next: cut the pair down
    (outputs the pixel shader does not read, the `float[70]` member) in the offline test until it builds; then lane S
    emits that form.
  - **Review of W2's two graphics changes: both right.** `tex_slots` (a texture handle's separate sampler bank,
    Ryujinx's packing `texture slot | (sampler slot + 1) << 16`) and `ShaderCache::refused` (a draw waits for the live
    translator only for a shader not yet loaded; one that failed to build is not waited for: `None` in `modules` is
    stored only after a build error). Its doc comment had displaced `get_ex`'s: put back.

**Lane G3, rev 8 (2026-11-20, uncommitted).** Runs in `build/G/runs/` and `build/I/runs/*-G3-mk8d-race`.
- **MK8D's cyan road (M5): narrowed again, not fixed.** New scenario `mk8d-lap` (play_scenario: the mk8d steps, then a
  shot every 5 s for 70 s) and pass-dump controls (`NXGFX_DUMP_PNG=0`: log lines only; `=<w>x<h>`: pictures of
  targets of that size only; target lines now show each target's layers and view). Findings:
  - The two suspects are cleared. Colour targets 0 and 1 of the point-light passes are **layers 0 and 1 of one
    2-layer array** (view `L0+1` / `L1+1`), not one image twice; the 6-target probe passes are a cube's 6 faces.
    The address holding four targets a frame holds them one after another (normals, then two RGBA8, then a half-size
    one); same-shape ones share an image now (rev 7), as the memory does on the console.
  - It is not the resolution either: the game's own dynamic resolution drops to 1440x810 when its frame is slow (the
    dumps themselves made it slow), and the dumped 1440 frames are cyan, but with `game_dynamic_resolution=0` at
    1920x1080 the same turn is just as cyan (`…-002736`). (Tried and reverted: leaving the GPU's idle time between
    submissions out of the timestamp reports; the game still dropped to 1440 under the dumps' slowdown.)
  - What is wrong is **shadowed ground**: the cyan is wherever a shadow falls (the kart's own shadow is a cyan blob
    in every frame; the "bad turn" is the stretch under the grandstand roof). It is already cyan in the G-buffer
    pass's colour target. The shadowing reads a **2048x2048 R32G32_SFLOAT texture (a variance shadow map, view
    swizzle R G 0 1) that no pass draws** (it is uploaded from its storage, `uploads from storage: … 2048x2048 fmt
    0x16`) and a 2-layer D16 cascade array that passes 65-67 draw. Next: read the R32G32 texture back (its content,
    and whether the game fills it from the GPU through a copy we miss or from the CPU), and the light pass's use of
    it.
- **Donkey Kong Bananza survives its compiler crash.** The NVIDIA compiler faults on vertex `d87ccbede95b9f2c` +
  pixel `6655320127e098d6` whole **and** linked from parts (pipeline libraries do not avoid it; the offline test with
  `NXGFX_VALIDATION=0 NXGFX_GPL=1` crashes too). Bisecting by zeroing the vertex shader's outputs: zeroing any one of
  locations 0, 4, 5, 6, 7 or 9 makes it build; 1, 2, 3, 8 or the position do not; flat / centroid decorations do not
  matter. So it is the compiler on the two programs together, not one bad construct found yet. Graphics now records
  a pipeline whose compile was under way when the process died (`<data>/pipeline-compiling.bin`, written around each
  compile, whole or linked) into `<data>/pipeline-crashed.bin` at the next start and never compiles it again: its
  draws are skipped with a red line. DKB: `d7` crashes once (as before), `d8` runs 150 s past that point (it is at
  13-17 FPS there and draws the world purple: separate problems, `shot-002500.png`). New ignored test
  `one_program_pair_builds` + `NXGFX_PIPE_LOG=1` find such a pair; `build/G/dkbpair` holds this one.
- **Lazy write-back: fixed, on by default** (`NXGFX_WB_LAZY=0` off). The 28 ms a frame was `lazy::touch`: every
  write-back landing, texture upload and pool sync "touched" every page of its range, each a lock and a VirtualQuery
  (64 ms a frame of it in `v11`, timers added to the pools line). It now visits only the held pages in the range.
  Violet `v12`: 150 s clean at its 30 FPS cap, write-back 0.5 ms a frame (1.3 ms copied back eagerly, `v7`). Z-A
  (`NXRS_EXCLUSIVE=1`): `z8` lazy on 53.5-58 FPS, `z9` lazy off 47-48.5: the write-backs it no longer copies were most of Z-A's gap
  (the game waited 15.6 ms a frame against 18.0). MK8D's demo with it on: 60 FPS, nothing held (`m5`).
- **Not reached this round:** Wonder's first-entry stutter, ray tracing / the rest of 5A.4.

**Lane G3, rev 9 (2026-11-20, uncommitted).**
- **DKB dying at 2.8 s in every session after the first (lane X2): fixed.** The vertex shader `d87ccbede95b9f2c`
  crashes the compiler **on its own as a pipeline-library pre-rasterization part** (offline: with any pixel shader,
  `NXGFX_GPL=1`); the shader parts made ahead at ProgramSetShaders compile it at start-up, outside the rev 8 guard.
  The crash list now covers library parts too: a part under way when the process died is recorded (a marker key
  per part: pre-rasterization shaders / pixel shader) and not made again, its programs are compiled whole instead
  (which works for this vertex shader with every pixel shader but `6655320127e098d6`). `build/G/runs/d9` dies at
  5.5 s (the part), `d10` / `d11` run 90 s without a fault. Still open: the pair itself (its draws stay skipped).
  Bisecting so far: zeroing any one of the vertex shader's outputs 0 / 4 / 5 / 6 / 7 / 9 makes the part compile,
  1 / 2 / 3 / 8 / position do not; Invariant, flat / centroid, location order (all ten reversed in both stages),
  SPIR-V 1.0 (refused: the translator declares GroupNonUniform* capabilities in every module, unused here) and
  `spirv-opt -O` (101 KB -> 35 KB, still crashes) change nothing. `spirv-reduce` is shrinking the optimized module
  with `build/G/g3/interesting.bat` (the part still crashes) as the test: it only got to 31.6 KB (almost any removal
  makes the crash go away).
- **DKB's crashing pair: fixed (rev 9), in graphics at module load.** Writing the vertex shader's Location outputs
  once at the end instead of where the translated code writes them (inside branches) makes NVIDIA's compiler build
  both the library part and the whole pair (offline `one_program_pair_builds`; the exact transform tried first by hand
  with spirv-dis / spirv-as). `vk/spvfix.rs` `late_outputs` does this for every vertex shader when its module is made
  (private copies, stored to the outputs before each return; a module using an output any other way is left as it
  is); `NXGFX_LATE_OUTPUTS=0` turns it off. Lane C's pipeline test on 300 pairs each of MK8D, DKB and Z-A builds them
  all with the same validation errors as without it (10 / 12 / 20, all `maxCombinedClipAndCullDistances`: the
  translator's gl_PerVertex declares more clip + cull distances than the device allows; lane S). DKB `d12` / `d13`
  with the crash lists cleared: the pair compiles, no fault, nothing skipped. The crash list stays as a safety net.
  Lane S could emit this form itself (then graphics' pass finds nothing to do).
- **Z-A:** `z10` (all of the above, `NXRS_EXCLUSIVE=1`): 58.5-60 FPS in most windows after the first 15 s (was
  53.5-58 in `z8`); `z11` with `NXGFX_LATE_OUTPUTS=0`: about the same (58-60, one 49-54 stretch), so the gain is not the
  late outputs: it came with this link's newer os layer (other lanes' scheduler work) on top of lazy write-back.
  Z-A is at its 59-60 in most windows now; the dips that remain are not attributed.
- **Wonder's first-entry stutter: not worked on.** As in rev 7 (`x2`): the 0.3 / 2.5 / 0.26 s frames at the first
  entry into a level are shaders compiled at their first draw; a second session prewarms them from the key file.
  Next: make the shader parts ahead for a level's programs when they are registered (ProgramSetShaders already does
  for the parts; the whole pipelines it links are not made until the draw).
- **MK8D shadow map: open.** See above (the 2048x2048 RG32F texture is zero on the GPU and in memory, nothing writes
  its address). Next: find the shader that turns shadowed ground cyan (the screen-space shadow pass samples it with
  the depth and the D16 cascade array) and read what it expects there; check whether lane W2's Smash / Anime Girls
  banding goes through the same pass.
- **DKB at 13-17 FPS after the opening:** 4,072 draws, 2,573 write-backs and 9 submissions a frame, 1,275 mirror
  pools (1.9 GB); the pool sync (write watch over the mirrors, every submission) takes 33.8 ms a frame and the
  completion waits 8.4 ms a frame for the pool table. Not changed yet. The purple world: not looked at yet.
- **MK8D's shadow texture (item 1): read back, all zeros.** New `NXGFX_DUMP_TEX=<w>x<h>` (with a pass dump): a
  sampled texture of that size is read back (`tex-<frame>-ser<n>-gpu.bin`, level 0 as the GPU has it) along with its
  guest storage (`-mem.bin`). The 2048x2048 R32G32_SFLOAT texture the shadow pass samples is zero in both, at 120 s
  and 130 s. `NXGFX_WATCH_ADDR=<hex>` (new: images made / released over an address, and GPU writes into it) shows
  only plain texture objects (flags 0, never drawn) made over its storage at the course load, and **no GPU write
  into its 32 MB at all**; nothing else is drawn there in the dumped frames. So either the game really leaves it
  zero (and the cyan comes from elsewhere in the shadow pass), or it is filled through memory we do not see as this
  address (another pool mapping the same pages). Not settled.

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
- **(lane X2, 22:05) Donkey Kong Bananza: the NVIDIA shader compiler crashes the process ~10 s into the game's opening**
  (0xc0000005, no FATAL: the fault is in `nvgpucomp64.dll+0x3c1c150` on a driver compile thread, `nvoglv64.dll+0x7b725`),
  logged with the new `NXRT_FOREIGN_FAULTS=1` (runtime `fault.rs`: a fault in another module is logged with module +
  offset frames before Windows ends the process). Same with `NXGFX_DESCBUF=0`. It comes 4 s after the first
  `[vk] raw global memory` / `append counters` lines, so a pipeline made for those draws is the suspect (main tree with
  lane G3's uncommitted work, 21:57). Runs: `build\I\runs\20261119-215750-X-x2-dkb6-foreign`,
  `20261119-220045-X-x2-dkb7-nodescbuf`, `20261119-214352-X-x2-dkb4-opening`.
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
  clean. `SWR_XMON=1` and `NXGFX_WB_LAZY=0` do not change either; `NXGFX_POOL=host` does not change MKW (DKB ran 120 s
  clean with it, but that run also never committed its save, so it proves nothing yet). Details: lane X section.

**Lane W2 findings for lane G (2026-11-19, engine built from the main tree at 120a8a0 + lane W2's files):**
- **MK8D races are posterized** (`build\I\runs\20261119-175850-W2-mk8d-gp3`, `build\games\mk8dw`, `lsfg=0`): the blue
  tint and black road are gone, but every race picture has hard colour bands (cel-like steps in the sky, grass and
  water, `0015-shot_race_1_min.png` Mario Kart Stadium, `0020`/`0021` Water Park, `0025` Sweet Sweet Canyon with a dark
  teal road) and the karts / characters look darker than lane G's `build\G\runs\r28\shot-006400.png`, which is smooth.
  The menus are clean (`0005`, `0006`). The Grand Prix preview movie panel now plays (`0004-a_first_Mii.png`, the
  panel shows the race clip, with the same banding), so the black panel is gone. The battle run on the same build
  (`20261119-174606-W2-mk8d-gp`, `0007-a_menu3.png`, Balloon Battle in Urchin Underpass) shows the banding too.
- **MK8D FPS in the Grand Prix**: with two game builds compiling beside it (8 clang processes) 48.7 FPS (`-gp3`);
  without them (`build\I\runs\20261119-212504-W2-mk8d-gp4`) **59.1 FPS over 120-760 s, 577 of 641 s at 58+**, the
  longest frame 1.8 s at a race load, sound 0 underruns in 782 s, private memory 4.05 -> 4.15 GB. The posterized race
  picture is the same in that run (`0020-shot_race_6_min.png`): the one thing between MK8D and M5.
- **Beach Bounce Remastered presents black** (`build\I\runs\20261119-182618-W2-bbounce-menus`, pass dump at 86 s in
  `build\games\bbounce\captures\pass-004785-*`): the game (Ren'Py on its own MNX engine) draws its scene correctly into
  a 1920x1080 RGBA8 target (passes 0-47, one quad each: `pass-004785-020-...png` is the hospital scene with Grandma
  Umi's dialogue), then the last pass (48: vertex `ae00dca5e2d1ea98`, pixel `317416f793e1251a`, one texture id260 of
  1920x1080, **with a 1920x1080 depth attachment, format 0x36**) blits it to the window texture and writes all zeros
  (centre pixel 00000000; every `fullres` is black). Likely the depth (test against an uncleared / wrong-cleared depth)
  or the sampled texture id260 not being the scene target. 60 FPS meanwhile.
- **Sonic X Shadow Generations, program 0 (title menu, Shadow Generations) shows false colours**: yellow / grey /
  navy "solarized" pictures at 1280x720 (`build\I\runs\20261119-191241-W2-sonic-menus2\0001-shot_boot.png`,
  `20261119-200906-W2-sonic-shadow\0012-a_intro_7.png`): the title and story scenes are recognisable but every colour
  is wrong (wrapping bands, as if a float or 10-bit target were read as UNORM8 or the other way). Program 1 (Sonic
  Generations, `sonic_p1.exe`) at 1920x1080 is right (`20261119-204133-W2-sonic-gens\0013-shot_play_1_min.png`).
- **Super Mario Party Jamboree: the white screen after the title is most likely a blend state (lane G).** The opening
  movie and title are posterized like MK8D; after L+R the hub (Party Plaza) is drawn right (pass 96) and the last
  pass (108: vertex `ce7eacf81c6e4ffc`, pixel `7f4356c21967c962`, one draw over 1920x1080 R8G8B8A8_SRGB with a depth
  attachment) covers it with plain white. That quad's colour bank c3 at guest 0x1d98ad300 reads (1, 1, 1, 1) **in
  guest memory too** (`peek 1d98ad300 40` while the screen is white, run `build\I\runs\20261120-013909-W2-jamboree-fade`), and
  its texture is id2709, an 8x8 BC4 with swizzle (ONE, ONE, ONE, R): the game asks for an opaque white quad on purpose.
  A screen-wide white quad the game keeps drawing during play only leaves the picture unchanged if it is blended
  multiplicatively (DST_COLOR / ZERO, or min) or masked; so this draw's blend (or colour write mask / depth test) is
  probably not what the game set. Meanwhile the game runs: the hub's music plays (renderer 0 not silent), the job
  threads idle. Lane W2 checked: no service, nifm, save or executor wait explains it (lane W2 section).
- **Fixed by lane W2 in graphics (small):** texture slot pairs (`tex_slots`, Anime Girls drew every texture as id 0)
  and no 3 s wait for a refused shader (Jamboree's 70 s stop); see the lane W2 section.

**Lane G4 (2026-11-20, uncommitted).** Engine `target/G4`; game links `build/G4/<game>t` (`build/G4/link.sh <game>`: the
game's objects, read only, a data folder of its own with the shaders through a junction to the game's, saves backed
up); runs `build/I/runs/*-G4-*`. GPU tests: `cargo test -p graphics --features gpu-tests --test gpu -- --test-threads=1`
(17 now, all validation clean).
- **shaders `link.rs` (new, used by graphics at module load and pipeline creation; built modules on disk unchanged):**
  `trim_cull` (Turbo `vkspv_trim_cull`: gl_PerVertex's unused CullDistance member dropped when clip + cull pass
  `maxCombinedClipAndCullDistances`; a red line names a shader whose cull distances are used), `io_locations` /
  `missing` / `add_outputs` (a pixel shader reading a varying its vertex / geometry / tessellation shader never
  writes: the earlier stage gets that output, holding (0, 0, 0, 1)), `int_outputs` / `output_kinds` (integer colour
  targets), `dual_source`, `alpha_test` (`ALPHA_PUSH_OFFSET` 16: function u32, reference f32), `ballot_per_lane`.
  **For lane S:** these could be applied at translation instead; graphics would then find nothing to do.
- **Lane C's pipeline test:** the errors left were not clip / cull ones on this layer version but
  `VUID-RuntimeSpirv-OpEntryPoint-08743` (pixel inputs no vertex output declares: 42 of Z-A's first 300 pairs). With
  `pipe.rs link_io` (vertex variant per pair, `NXGFX_LINK_IO=0` off) and the trim: 0 validation errors on MK8D, DKB,
  Z-A (300 pairs), Smash, Jamboree, Violet (600 pairs), prewarm included.
- **Variants:** `shader.rs Library::variant` (cached per module and tag; the key's last bytes differ, so pipeline parts
  and caches key on the variant): `with_outputs`, `with_int_outputs`, `fragment_variant(dual, alpha)`. Draw time
  (`exec.rs draw_setup_ex`): dual-source when target 0 blends with SRC1 factors (else those factors count as zero),
  alpha test when NVN's alpha function is not ALWAYS (`NXGFX_ALPHA_TEST=0`, control `alpha_test 0|1`). Pipeline time
  (`pipe.rs link_io`): integer outputs from the key's colour formats, missing varyings.
- **Storage use without the IMAGE flag:** on by default (`NXGFX_IMAGE_NOFLAG=0`, ini `image_noflag`, control
  `image_noflag 0|1`); `image.rs want_image`, `upgrade_storage` (at the next submission's start, or at once when a draw
  binds the image).
- **Present:** `vk/upscale.rs` sharp upscale (`NXGFX_UPSCALE=standard`, `NXGFX_SHARPNESS`); frame generation's frames
  are still blitted linearly.
- **Linear render targets** read back at pass end (`NXGFX_RT_READBACK=0`). **NVN:** RegisterFastClear* return 1,
  SetConservativeRasterEnable / SetShaderScratchMemory are procs now. **Pass dump** lines show target 0's blend
  equation, logic op, alpha to coverage and alpha function; `NXGFX_DUMP_TEX` reads back compressed textures too (as
  blocks).
- **Write watch per allocation** (`mem.rs begin_submit`): watched pools of one allocation share one GetWriteWatch a
  submission even with gaps between them (DKB: ~1,300 mirrors, 9 submissions a frame). **Not measured:** DKB built from
  the current tree aborts in DT_INIT at 0.9 s (`nn::diag` Result 0xe801 from sdk+0x159154, before graphics:
  `build/I/runs/20261120-025832-G4-dkb-pools`); **for lane O3**, probably the os work in the tree.
- **First-run profile prewarm** (`pipe.rs prewarm_profile`, `NXGFX_PROFILE_PREWARM=0`): the shader parts of the
  profile's pairs made ahead when the key file is nearly empty (Wonder's first entry). Not yet run on a first run.
- **MK8D shadowed ground (M5): narrowed, not fixed.** Pass dump of `20261120-024043-G4-mk8d-lap1` at 150 s: pass 28
  (shader `6aa6c36ed6065b7d`) writes an RGBA8 shadow mask at 0x15cf60600 (R = a constant from c3, G / B = the
  shadow terms from the depth, the 2048x2048 RG32F VSM and the 1024x1024x2 D16 cascades); passes 29 / 30 multiply its
  R by the karts' blob shadows (colour mask R only, ZERO / ONE_MINUS_SRC_ALPHA); pass 31 (the forward / G-buffer pass)
  samples it through texture ser352412 whose **texture swizzle and view swizzle are both (G, B, A, R)**. Composed (as
  Turbo does) that is (B, A, R, G); a view replacing the texture's would give (G, B, A, R). Tried
  (`NXGFX_SWZ_OVERRIDE=1`, run `-mk8d-lap2-swz`): the stretch past the grandstand is still flat cyan with black shadow
  (`0019-shot_lap.png`); in `-lap1` the same stretch came out black (`0022`, `0027`), the rest of the lap right
  (`0016`). The VSM is still all zero (G3 rev 9). Next: decompile pass 31's pixel shaders (`044c744ceb0df7c2` and the
  others sampling id7160) to see which mask channels they read, and find who should fill the VSM.
- **Jamboree's white layer: not a blend state.** Pass 108's quad (`20261120-022711-G4-jamboree-blend2`): blend
  SRC_ALPHA / ONE_MINUS_SRC_ALPHA, logic op COPY, no alpha to coverage, alpha function ALWAYS, colour mask all; its
  colour = texture x c3[0] + c3[1] with c3 = (1, 1, 1, 1) / 0, and its 8x8 BC4 texture (swizzle 1, 1, 1, R) is block
  `ff 00 00…` (every texel 1.0) on the GPU and in guest memory (`build/G4/jdump/tex-003663-ser3027-*.bin`). So the
  game draws an opaque white fade on purpose: its fade state never advances. Back to lanes W / X (game state); not a
  graphics state bug found.
- **Smash device lost: not reproduced alone.** `20261120-025107-G4-smash-lost` (`NXRS_EXCLUSIVE=1`, 360 s of matches):
  no device lost, 52.5 FPS (window 150-350 s). W2's losses came with other lanes' games on the GPU.
- **Not done:** recording one submission on several threads (needs a recorder over every vkCmd call, Turbo
  `vk_defer.c`), ray tracing (PORT_PLAN 5A.4 row), VRAM eviction, capture / replay, DKB's purple world, Wonder's
  stutter measured.

**Lane G4, round 2 (2026-11-20, after the 03:51 crash; uncommitted).**
- **GPU hangs (top priority).** `vk/lost.rs`: VK_NV_device_diagnostic_checkpoints before every dispatch and indirect
  draw (`NXGFX_CHECKPOINTS=all`: every draw; `0` off), what each was (shader keys, sizes, indirect address); VK_EXT_device_fault.
  On VK_ERROR_DEVICE_LOST (submit, the completion wait, present, acquire, sparse binds) the report is logged once,
  then the process ends with **exit code 71** (no retry, no panic). The device line says what is on: `if the GPU
  stops responding: checkpoints …, the driver's fault report …, loops bounded at …`.
  - **MKW:** lane K's runs `-K-mkw13-demote` / `-K-mkw14-robust` were reported: a mesh draw (task fea1d6ca04512de1,
    mesh ddc034e8e009ebfc, pixel 3cc3501255bb2d74), the driver's fault an **invalid read** (0xca19c6000,
    0xcbf689000). The task shader reads storage buffers through descriptors: **robust buffer access is now on by
    default on NVIDIA** (Turbo, Ryujinx; `NXGFX_ROBUST=0` off). Details in the MKW (lane K) section.
  - **DKB** (`-G4-dkb-pools2`, 03:44 reset): lost right after a new pipeline (pipeline lookups 446 ms) at 116 s, the
    frame rate falling 60 → 20 → 1 FPS before it; no checkpoints then. **With the loop bound and robust buffer access** (`-G4-dkb-hang4`, `NXGFX_CHECKPOINTS=all`): no device loss in 167 s, through the opening (55-60 FPS) into play (5-12 FPS) past the 116 s where `-pools2` was lost; the GPU at 11 ms a frame, the replay's CPU 45 ms (pool sync 32.5, sets 18): DKB's frame rate is the pool sync (item 10), not the GPU.
  - **Every shader loop is bounded** (`shaders::link::loop_guard`, applied at module load: a counter per function,
    the back-edge goes to the loop's merge after 65,536; `NXGFX_LOOP_GUARD=<n>`, 0 off). 47 of DKB's first 300
    modules have loops. Lane C's test clean on MK8D / DKB / Z-A / Smash with it; GPU test `loops_are_bounded`.
  - **Indirect counts held to the device's limits** (`exec.rs clamped_indirect`, `shaders/clamp_counts.comp`, a one-
    invocation kernel on a staged copy, `NXGFX_CLAMP_INDIRECT=0` off): indirect dispatches (compute limits) and indirect
    mesh draws (task limits, MKW's resets were mesh draws); direct mesh draws are held too. GPU test
    `indirect_dispatch_through_the_count_clamp`. (Round 3: indirect plain draws are held too, before the pass.) Before that, indirect plain draws were not (they sit inside a pass, where no kernel
    can run; the checkpoint names them and their address). Flight recorder / Aftermath: not done.
- **Pool sync, cold pools** (`mem.rs begin_submit`, `NXGFX_POOL_COLD=0` off): a watched pool with no CPU write for 60
  frames is asked once a frame instead of at every submission (DKB: 1,259 mirrors, 9 submissions a frame, 32 ms of
  pool sync a frame); a write to a cold pool reaches the GPU at the next frame's first submission.
- **Recording on a thread of its own** (`vk/defer.rs`, `NXGFX_DEFER=1`, off): barriers, passes, copies, blits, clears
  and queries are queued with copies now (they waited for the recorder before), and the main buffers have a command
  pool of their own (the replay thread records the upload buffer meanwhile; one pool's buffers may not be recorded
  at once: the validation layer's threading check found it). MK8D measured before this change: on 10.8 ms of replay a
  frame against 5.75 off, 57.6 against 59.4 FPS: off by default. Not measured again yet.
- **Ray tracing** (`vk/rt.rs`): acceleration structures from NVN2's own build descriptions, keyed by guest address;
  GPU test `ray_query_traces_a_triangle`. NVN2's command entry points still wait for their argument layouts.
- **VRAM eviction** (`image.rs vram_budget`, VK_EXT_memory_budget, every 30 frames, over 90%: the parked images go);
  GPU test `vram_budget_frees_parked_images`. **Sampler pool set** 16,384 (Smash registers sampler IDs up to 6,400:
  past the old 4,096 they had no descriptor). **NVN2 device item 0x6** answers 32.
- **Measured:** Wonder's first run with the profile prewarm (`-G4-wonder-first`, key file emptied): 3,955 shader parts
  made ahead, 58.5 FPS over 20-190 s, 36 slow frames, the worst 2.49 s at 83 s (1,706 ms of pipeline lookups, 61
  pipelines made: the first entry into a level still compiles there); G3's `x2` second session had 17. DKB
  (`-G4-dkb-pools2`): 60 FPS through the opening (39-80 s), then the device loss above; its pool sync 12-33 ms a frame
  (was 33.8).
- **MK8D shadowed ground:** with `NXGFX_ASYNC=0` (`-G4-mk8d-sync2`) the stretch past the grandstand is right (no cyan
  road, no black), so the cyan / black there comes from asynchronous replay (a race or the replay reading memory the
  game has rewritten by then); the karts' own shadow stays a cyan blob either way (the swizzle question above).
  `NXGFX_ASYNC_PRESENTS=1` (one present queued at most) is queued to tell lag from a race. **Queued when G4 stopped (the
  machine was held by other lanes' exclusive runs):** `build/G4/chain3.sh` (MK8D lap with `NXGFX_ASYNC_PRESENTS=1` and
  `NXGFX_CHECKPOINTS=all` -> `build/G4/c3_mk8d.out`, then Smash 360 s -> `c3_smash.out`) and `chain4.sh` (DKB with
  cold-pool scanning, exclusive -> `c4_dkb.out`); each starts once, waiting while the machine is held. Note: the 03:51
  crash came during G4's earlier MK8D run with `NXGFX_ASYNC_PRESENTS=1` (`-G4-mk8d-aq1`: device lost at 130 s, no
  checkpoints then).

**Lane G4, round 3 (2026-11-20 05:00-, after a57eecd; uncommitted).** `NXGFX_ASYNC_PRESENTS` stays at its default
(chain3's `=1` MK8D step was stopped before it ran; chain3 / chain4 replaced by `build/G4/chain5..9.sh`).
- **Asynchronous replay: two missing waits fixed** (`vk/queue.rs`; GPU tests fail without each fix, pass with it):
  - *A command buffer's FenceSync read last frame's fence.* A sync fenced inside a command buffer (and reused every
    frame) kept its old, signalled fence until the replay thread reached the record, so `nvnSyncWait` passed while
    this frame's work was still queued and the game rewrote memory that work reads. Now `submit` fences every such sync
    to the submission when it is queued (the replay still sets the exact value). Test
    `reused_command_buffer_sync_covers_the_new_submission`. Log line once: `command-buffer fence syncs: fenced when submitted`.
  - *A queue waiting on a parked queue's sync ran ahead.* `nvnQueueWaitSync` on an async fence returned at once even
    when that fence's submit had found its queue parked at an event wait (its fence a receipt): the waiting queue's
    work ran before the parked work. The wait now goes to the replay thread, which resolves it in queue order. And a
    wait queued on a queue that is itself parked with work is no longer dropped: `Parked.later` keeps it at its stream
    offset. Test `queue_waits_for_another_queues_parked_work`. Log line once: `a queue waits on another queue's parked work`.
- **Indirect plain draws held** (`exec.rs clamped_draw_record`, `draw_limits`): DrawArrays/ElementsIndirect records
  (and MultiDraw…IndirectCount's, up to 4,096) are copied and clamped before the pass (the pass ends there; the draw's
  set-up begins it again): 16M vertices / indices, 256K instances, 256M of the two together (`clamp_counts.comp`
  lowers instances; `NXGFX_DRAW_LIMIT` = that product in millions; `NXGFX_CLAMP_INDIRECT=0` off). GPU test
  `indirect_draw_with_garbage_counts_is_held` (3 x 4 billion instances: drawn in < 1 s, green).
- **Smash device loss (`-G4-smash-hang2`, 135.9 s into a match, exit 71, a driver reset at 05:14):** the fault report
  says *invalid write at 0x400000000*, no checkpoint reached (default level: dispatches / indirect draws only), during
  a run of new pipelines. Fixed one way stale device addresses can be written through: **buffer textures' pool-set
  descriptors name their pool buffer's device address**; when that pool was finalized (or a new one took its address)
  they kept the freed address. `mem::pool_add` / `pool_del` now queue the buffer textures over the range again
  (`Tables::requeue_buf_texs`), written at the next submission from the new pool, or over a 64 KB zero buffer when
  none holds them (`image.rs texel_dummy`). **Not Smash's cause:** with it and `NXGFX_CHECKPOINTS=all` the device
  was lost twice more at the same draw (`-G4-smash-fault` 131.7 s, invalid write at 0x300000000; `-G4-smash-fault2`
  203.9 s, at 0x400002000): **an indexed draw of 3 indices (a full-screen triangle), vertex shader `2df54d1acbb46153`,
  pixel shader `f9252217720fbfc5`** (one texture, three uniform banks, one colour output, no stores: the write is the
  colour target's). The images freed in the 15 s before (`lost.rs note_gone`, listed on loss) are ordinary released
  textures. The loss report now also names the pass's targets (texture serial and address, view, image, size, alias /
  sparse): `chain12` -> `c12_smash.out`.
- **DKB, cold pools measured** (`-G4-dkb-cold`, exclusive): pool sync 10-12 ms a frame (was 32.5), play at 15-25 FPS
  (was 5-12), no device loss in 170 s. What is left is descriptor sets: **27 ms a frame for 15,400 sets** (7,060 draws,
  4,280 write-backs, 9 submissions).
- **DKB descriptor sets: a set cache** (`exec.rs set_key`, `QState.sets`; `NXGFX_SET_CACHE=0` off): a stage's set is
  reused (its descriptor-buffer offset bound again, nothing built or written) when the module, the stage's bank
  generations (c0 storage buffers, c2 handles), its uniform bindings, the push-constant generation, the pass size,
  the pool bases, the dispatch size / base instance, the submission and the descriptor-buffer chunk are the same as
  for the stage's last set. Not cached: bindless handles (from a uniform or storage buffer), buffer textures, the
  resolution scale, pass dumps. `-G4-dkb-prof` (`NXGFX_PROFILE=1`): **7,820 of 13,070 sets a frame reused, sets 27 ->
  15 ms** (memory lookups 7.5, descriptor writes 4.2, banks 2.1); the replay 40.7 ms a frame against a 16.8 ms GPU,
  15-23 FPS (the fine timers cost some). Left on the replay: pool sync 11-13 ms, sets 15, vertex input 5. GPU tests
  all pass with it. Counter `sets_reused` (the per-frame line says "(N reused)").
- **Wonder's first-run level-entry frame: not fixed.** `vk shader parts` stopped at 22 s ("private memory grew 2048
  MB") with 3,955 of 14,603 programs made, the budget counting the game's own loading too. Tried: each job's own growth
  counted, the budget a twelfth of RAM (6 GB here) and 4 part threads (`-G4-wonder-first2`): 19,441 parts by 102 s,
  but the level entry still took 2.3 s (965 ms of pipeline lookups) and the run had many multi-second frames (up to
  11.9 s, most with the replay idle): worse. Kept: the per-job growth count with the 2 GB default and 2 threads.
  Making ahead everything a profile names does not cover a first level; what is left is whole pipelines (the parts
  alone still left 965 ms of lookups) or a second run (keys known).
- **Ray tracing, NVN2 argument layouts: not obtainable from the owner's games.** In MKW's and DKB's `main` the
  ray-tracing names (`nvnCommandBufferDispatchRays` …) appear only in the game's proc loader (name -> GetProcAddress
  -> a global pointer: MKW `main+0xdd4f84`, pointer via GOT `0x36e0208`; DKB `main+0x5de2ec`, GOT `0x245a680`); no code
  in either game loads that pointer again (no call site), and the implementation is the system's NVN driver, not in
  `main` or `sdk` (sdk exports only `nvnBootstrapLoader`, no `nvnCommandBuffer*`). So there is no caller to read the
  arguments from and no callee to disassemble; a game that traces rays is needed.
- **MK8D lap with both async fixes (`-G4-mk8d-asyncfix`, 59.7 FPS, exit 0):** the command-buffer sync fix is exercised
  (MK8D fences syncs inside its command buffers), but the stretch past the grandstand still comes out **black with only
  the lights' glow** (`0016`, `0019`; `0024` later in the lap is right). So that was not the race.
- **Conditional rendering ported from Turbo (`counters.rs rc_*`, exec `RENDER_COND`, clears): very likely the black
  world.** SetRenderEnableConditional compares the counters at addr and **addr + 16** (we compared addr + 8, the first
  report's timestamp); a timestamp report's counter field is **0** (we wrote the time); a timestamp report replayed but
  not landed yet counts as 0 (it lands at completion: how late depends on the async lag, which is why only async
  showed it); an address a samples-passed report writes is unknown at replay and keeps drawing; and **colour / depth
  clears obey the condition** (they ignored it). agl wraps its clears this way (clear once, then a TIMESTAMP_TOP report
  on the record); Turbo's note on the same bug: "the second clear ran and wiped the lit scene ... came out black".
  GPU test `conditional_clear_runs_once_after_a_timestamp_report`. MK8D lap queued (`chain11` -> `c11_mk8d.out`).
- **Pipelines replaced by their optimized build are destroyed** (`pipe.rs retire`, once the work recorded with them
  has run; `NXGFX_RETIRE_KEEP=1` keeps them): they were kept for the session (part of MKW's growth in play).
- **MK8D kart blob:** pass 31's pixel shader `044c744ceb0df7c2` reads the screen-space shadow mask (`fp_t_tcb_26`,
  sampled at the projected position) as `.x` only. With its texture and view swizzles both (G, B, A, R), `.x` is B
  composed (Turbo's rule) or G with the view replacing; neither is R, where passes 29 / 30 put the karts' blobs. The
  blob reaches the picture through another shader; not traced further this round.
- **Capture / replay: not started** (no nxnative capture source in reach; lower than the hangs and the races).

**Lane G4, round 4 (2026-11-20 06:15-, uncommitted).**
- **MK8D after the conditional-rendering port (`-G4-mk8d-rcond`, 59.7 FPS):** still black stretches in the lap (shots
  0013-0017 and 0020 at 0.55-0.78 black). Not new: `-G4-mk8d-nodefer2` (03:39) had them too (lap shots at 0.61 /
  0.77), `-G4-mk8d-sync2` (async off) none. So it stays an async-replay effect and neither the command-buffer syncs
  nor conditional rendering was it. Candidate being checked: occlusion results (samples passed) that land later in
  async mode, read by the CPU for culling (a one-time log line now says whether MK8D uses them; `c14_mk8d.out`).
  Not tried: `NXGFX_ASYNC_PRESENTS=1` (one present ahead), held off as asked.
- **Smash: the faulting pass is known** (`-G4-smash-fault3`, 166.96 s): the full-screen draw renders into **ser6166 at
  0x471074000, a 1920x1080 R8G8B8A8_SRGB image (image 0x224befa00d0, one level, not an alias, not sparse)** that is
  live in the image table, and the GPU's write faults at 0x400001000. The memory under a live image is invalid, so
  something freed or moved its allocation. **VK_EXT_device_address_binding_report** is now used with
  `NXGFX_CHECKPOINTS=all` (or `NXGFX_ADDR_REPORT=1`): every address range bound / unbound is kept and a loss names
  the object the faulting address belongs to, or the one last unbound there (`lost.rs on_binding`, `addr_owner`).
  `c16_smash.out` will say which.
- **Lazy images** (`image.rs reg_tex` / `materialize` / `materialize_all`, `NXGFX_LAZY_IMAGES=0` off): see the MKW (lane
  K) section. GPU test `vram_budget_frees_parked_images` makes its image through first use now (`Nvn::tex_base`).
- **DKB's 15-17 s frames** (`-G4-dkb-prof`: four of 6-17 s, the main thread in `nvnSyncWait` all the time, the replay
  idle): the command-buffer sync fix wrote the submission's fence *after* queuing it, so when the replay thread had
  already reached the record (and set the exact value, before an event wait parked the rest) the later write made the
  wait cover the parked rest too. Now written before the op is visible to the replay thread (`aq_push_with`).
  `-G4-dkb-cold` (before that fix) had 4-5 s frames already. `c15_dkb.out` measures.
- **Capture / replay** (`vk/capture.rs`, bin `nxgfx-replay`; mkw-recomp `runtime/gfx/cap` design): `NXGFX_CAPTURE=<file>`
  keeps pools, texture / sampler registrations and descriptor pools from the boot; at present `NXGFX_CAPTURE_AT`
  (600) it writes the guest's allocations (4 GiB .. 0x70_0000_0000) and their committed bytes, then each submission's
  flattened records, texture release and present for `NXGFX_CAPTURE_FRAMES` (2) presents, each preceded by the pages
  that changed (4 KiB hashes on 8 threads); one zstd stream. `nxgfx-replay <file> --shaders <data>/shaders --out <dir>`
  maps the allocations at their own addresses, restores state and memory, replays and writes `frame-<n>.png` per
  present. Not captured: what only the GPU held at the start (earlier render targets, device-local pools the GPU
  wrote) and sparse mappings. First capture queued on MK8D (`build/G4/mk8d.cap`, `c14_mk8d.out`); not replayed yet.
- **Wonder's level-entry frame:** not worked on this round.
- **Pool sync measured offline** (GetWriteWatch, this machine): ~2.2 us a call + ~0.95 us a MB scanned (committed or
  not); 3,500 small calls 8.7 ms; on 2-8 threads slower (16-23 ms: the kernel serializes them). So a scan of every
  mirror costs ~10 ms a submission for MKW's ~3,500 pools however it is split; fewer scans is what helps.
  **Consecutive queued submits of one queue now replay as one submission** (`queue.rs aq_main`, up to 8 / 64 MB;
  `NXGFX_AQ_MERGE=0` off): one pool sync and one vkQueueSubmit for all of them; each keeps its own fence (resolved to
  the merged submission's). The memory line ends "N submits replayed with the one before". GPU tests pass.
  `NXGFX_PROFILE=1` logs the write-watch calls / time per submission every 600 submissions.
- **Pipeline parts without RETAIN_LINK_TIME_OPTIMIZATION_INFO** (`pipe.rs gpl_part`; `NXGFX_PART_LTO=1` keeps it):
  nothing links parts with link-time optimization (the optimized pipeline is built whole from the modules), so the
  retained information only made each part bigger and slower (MKW's prewarm: ~7 MB a pipeline). Wonder first run
  queued to measure (`chain20` -> `c20_wonder.out`).
- **Smash with the address binding report (`-G4-smash-fault4`, 132.6 s, 08:26 reset):** the same draw again (the
  full-screen triangle into ser6168, a live 1920x1080 R8G8B8A8_SRGB image), *invalid write at 0x300002000*, and that
  address was **never bound to anything this layer created** (no image, buffer or memory of ours was there, then or
  before; the report sees bindings: GPU test `address_binding_report_sees_bindings`). So the write lands in memory
  the driver keeps for itself while writing our target, not in a freed allocation of ours. Leading suspect: the
  **alias images** (`image.rs alias_image`: a second VkImage of another format bound over the same memory instead of a
  view of one image); with NVIDIA's colour compression, two images over one memory can disagree about its
  compression state. Queued `chain22`: Smash with `NXGFX_ALIAS=0` (alias images off) and the full report, which now
  also gives the target image's bound address and how many alias images lie over its memory (`c22_smash.out`).
  MKW's indirect-dispatch fault (0x300001000) is the same kind of address.
- **Queue (07:40):** `chain19` = Smash with the address report (`c16_smash.out`), MK8D occlusion log + capture
  (`c14_mk8d.out`, `build/G4/mk8d.cap`), DKB profile (`c18_dkb.out`); then `chain20` Wonder first run. run_game's
  cooldown after 5 resets in the hour was 50 minutes at 07:37.

**Lane G4, round 5 (2026-11-20 08:50-, uncommitted).**
- **Smash's device loss: likely found.** With alias images off (`-G4-smash-noalias`, 134.1 s) the same draw still
  faulted (*invalid write at 0x300001000*, again in no memory of ours: 126 memory ranges bound, 2,019 unbound kept by
  the report), so not the aliases. What the faulting address does fit: one to two pages past a memory block, right
  after a full-screen draw into a 1920x1080 target. **The scissor was never held to the pass's render area**: a
  game scissor larger than the target (or one starting outside it) went to Vulkan as it was, fragments outside the
  render area are undefined, and NVIDIA writes them past the end of the target's image. Now `clamp_rect` holds the
  scissor to the render area (`NXGFX_SCISSOR_CLAMP=0` off; logged once: "a scissor past its pass's targets"). GPU test
  `scissor_past_the_target_is_held_to_it`. MKW's indirect-dispatch fault (0x300001000) may be another out-of-range
  write; not checked. **Verification queued** (`chain23` -> `c23_smash.out`: 240 s with `NXGFX_CHECKPOINTS=all`; the
  three runs before it all lost the device at 131-204 s).
  **Verified: not the cause.** `-G4-smash-scissor` (09:26 reset): the clamp did trigger (a 200x200 scissor at
  1741,898 in 1920x1080, from 4.7 s), and the device was still lost at 193.3 s in the same draw (ser6170), *invalid
  write at 0x300000000*. The clamp stays (it is Vulkan-correct). That run's report kept only the last 8,192 unbound
  ranges (it was full), so now every unbound range is kept, and the loss names the target's memory (allocation, its
  address and offset) and the nearest bound range below the fault. Next run queued (`chain25` -> `c25_smash.out`).
  `-G4-smash-noalias` (2,019 unbound, not full) still says the address was never ours.
- **MK8D (`-G4-mk8d-occl`, 58.0 FPS):** MK8D **does use samples-passed (occlusion) reports**, from the race start
  (60.7 s). In async replay their results land up to two presents later than in sync mode; if the game reads them
  without waiting (culling on last frame's result), objects are culled: the black stretches (lap shots 0.62 / 0.81 /
  0.70 black). Not changed yet: the natural test is `NXGFX_ASYNC_PRESENTS=1` (one present ahead), held off as asked;
  waiting for a go-ahead.
- **DKB (`-G4-dkb-poolprof`, 17.8 FPS with the fine timers):** **no multi-second frames any more** (the sync written
  before the op is visible fixed the 15-17 s nvnSyncWait). Pool sync 9.9 ms a frame: 0.87 ms a submission x 9, of it
  ~58 write-watch calls over ~255 hot pools (~0.7 GB scanned a submission). Submits are never merged here (the game
  waits for each one: no backlog). Sets 11.3 ms (lookups 5.3, writes 4.0), vertex input 4.7, replay 32.9 ms a frame.
- **Wonder first run (`-G4-wonder-first3`, 120 s):** the parts stopped at 4.3 s (3,100 made, the per-job growth count
  took the game's boot loading for theirs: the process was at 2.4 GB), and the level entry made 61 pipelines in 2.0 s.
  Now the parts stop at **6 GB of the process's private memory** (`NXGFX_PARTS_PRIVATE_MB`), and a part made on the
  replay's thread is logged with its key (`NXGFX_PIPE_LOG=1`) to see why the ones made ahead did not cover a level.
  Rerun queued (`chain24` -> `c24_wonder.out`, 120 s).
- **Frame generation off by default, one global setting** (owner, DECISIONS.md 1 and 8; config crate, outside
  graphics): `lsfg` default 0, `settings_version` 4 moves an ini's old default `lsfg=1` to 0 once (a 1 written after
  stays); config test `file_round_trip_with_migration_and_aliases`. **Whole-frame skip** (`framegen.rs`, new
  `shaders/fg_stats.comp`, `fg_interp.comp`): after the luma pyramids, one workgroup counts the coarsest level's
  changed and greatly changed pixels: nothing changed is a duplicate, more than half changed by over 0.2 of luma a
  scene cut, and the two frames after a cut only warm up; in all three the in-between frames show the previous frame
  as it is (Eglantine's duplicate skip and warm-up, reimplemented; the cut test is new). GPU test
  `frame_generation_skips_duplicates_and_scene_cuts` (duplicate, cut, warm-up x2, interpolate). **Nothing presented
  while minimised** (`present.rs record_present`: IsIconic on Windows, the front end's state on Linux via
  `set_minimised`; the game goes on); the window test now ends with 30 frames as if minimised (none presented, 30
  skipped, pacing still 59.5 FPS). (A first version of that test freed its program object before the
  submission that drew with it: a test bug, not the engine's.)
- **Capture / replay, first try:** MK8D's capture (`build/G4/mk8d.cap`, at present 6500: 83 regions, 1.6 GB of guest
  memory, 1,657 pools, 5,805 textures, 132 samplers, 2.4 s; 281 MB on disk) replays until its first pool, which is
  in the game's module data (0x70013465f0): the capture left the modules out. Now it takes everything below the
  kernel region (0x7F_0000_0000); `nxgfx-replay` reserves regions and commits pages as they come, says which regions
  it could not place, and `NXGFX_REPLAY_TRACE=1` lists the records. New capture queued (`chain23` -> `mk8d2.cap`).

**Lane G4, round 6 (2026-11-20 09:50-, uncommitted).**
- **Wonder's level-entry frame: fixed on a first run.** `-G4-wonder-first4` (120 s, keys and cache set aside):
  38,587 shader parts made ahead (stopping at 6 GB of private memory, not reached: 5.9 GB at 114 s), **no frame over
  a second; the level entry is 198 ms with 55 pipelines in 78 ms** (it was 2.0-2.5 s with 58-61 pipelines). 135
  parts were still made at their draw (logged with `NXGFX_PIPE_LOG=1`), cheap.
- **MK8D occlusion results** (`queue.rs aq_wait_room`, `OCCL_FRAME`, `NXGFX_OCCL_SYNC=0` off; unit test
  `occlusion_results_keep_the_replay_caught_up`): while a game has used samples-passed reports in the last 120
  presents, each present waits until every earlier op has been replayed, so the results come back as without async
  replay. Not `NXGFX_ASYNC_PRESENTS`. `-G4-mk8d-occlsync` (57.7 FPS): **1 black lap shot of 14 (0.69, 0016, past the
  grandstand) against 3-5 before**: better, not all.
- **Uploads held back over in-flight write-backs** (`mem.rs INFLIGHT`, `split_inflight`, `NXGFX_WB_GUARD=0` off; unit
  test `dirty_ranges_split_around_inflight_writebacks`): a CPU-dirty page over a range a submission in flight
  writes on the GPU (and lands back at completion) was uploaded with the guest's older bytes, overwriting the GPU's
  result before it came back; async replay widened that window. Those pieces now wait in the pool's pending list
  until the write-back has landed. GPU tests pass. MK8D lap queued to see the grandstand stretch (`chain28` ->
  `c28_mk8d.out`).
- **Capture / replay works end to end.** `build/G4/mk8d2.cap` (MK8D at present 6500, with module memory) replays:
  2 submissions, 2 presents, `frame-0/1.png` in `build/G4/replay2`. The regions are reserved before the device is
  made (the driver's own mappings sat where the guest's were). The HUD is right; the 3D scene is white, as expected
  from what a capture leaves out (the GPU-only state of earlier frames: exposure and other history targets).
- **Smash: no device loss with K2's lazy-page fix.** `-G4-smash-fault5` (K2's e2496aa: a pool's lazily held pages are
  brought back before its buffer goes; with `NXGFX_CHECKPOINTS=all` and the full address report): 240 s, the match
  played at 60 FPS (`0013-shot_fight_90_s.png`), **no device loss**, where the four runs before it lost the device
  at 131-204 s in the same draw. The "invalid write in no memory of ours" was that freed buffer.
- **MK8D with the write-back upload guard as well (`-G4-mk8d-wbguard`, 59.7 FPS): not fixed.** Lap shots black at
  0.62 / 0.76 / 0.82 (+ two at 0.24-0.26) of 14: no better than before (`-occlsync` had 1, the runs before 3-5; the
  lap differs run to run). Both changes stay (each is right on its own), but the black stretches are still an async
  effect without a known cause: `NXGFX_ASYNC=0` (`-G4-mk8d-sync2`) is still the only clean lap.
- **K2's e2496aa reviewed** (MKW (lane K2) section).

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
asks graphics for the picture too): Arceus `-145805-run5` and Violet `build\games\violet
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
- Z-A (`build\games\plza
uns61119-160759-run6`, main-tree engine of 15:35, `NXGFX_WB_LAZY=0`): title, opening,
  **Lumiose City lit and populated with the starters** (`0006-t240.png`; the black world above is gone in that build),
  no crash in 240 s; 60.0 FPS on the title, 35-46 FPS in the city (p95 22-64 ms, slow frames up to 0.87 s): short of
  M6's 59. With the lazy write-back on it faults at ~60 s (item above).
- How lane P runs: `scratchpad` scripts are not in the tree; the equivalent is `py scripts/run_game.py` with
  `NXRS_LANE=P`, `NXRS_NO_SONY_HID=1` (see the lane I item), `NXOS_SWKBD=auto` (else the name box waits for a person),
  presses through `scripts/gamectl.py <pid> press A~0.2`, and PrintWindow captures (the graphics-layer `shot` /
  `fullres` freeze the game today).
- **Esc from outside (lane W, for the coordinator):** lane W's runs get `[front] Esc: quit` (a WM_KEYDOWN VK_ESCAPE
  on the game window) that no lane W step sent: Jamboree `20261119-153405-W-jamboree-menus3` at 167 s,
  `20261119-161203-W-jamboree-m5` at 51.5 s, Smash `20261119-155110-W-smash-menus2` at 267 s, Anime Girls
  `20261119-161*-W-agsob-m5` at 35.7 s (before any step). Lane W's scenarios send only `press`, `shot` / `fullres` and
  `quit` through the pipe of the PID run_game started; they never post keys.
  Front (lane W, for lane I): `NXRS_ESC_QUIT=0` makes Esc log `Esc ignored` instead of quitting, and every Esc line
  now says whether the window was in the foreground (a real key press) or not (a message posted from outside).
  Lane W passes it in scripted runs; `NXRS_SHOT=shot|none` picks play_scenario's screenshot command.
- **Lane W hand-off (2026-11-19 16:35).** Engine for lane W games: `build\W-src` (a worktree at HEAD plus lane W's
  uncommitted files; scratchpad scripts `wt.sh` / `wsync.py` / `relink.sh` move it to HEAD, rebuild `target\W` and
  relink every lane W game). Blockers now: (1) **lane G**: any screenshot (`fullres` or the front's `shot`, both go
  through `shot::on_present`) can hang `nvnQueuePresentTexture` (Jamboree, Smash, Anime Girls: every run); runs
  without shots (`NXRS_SHOT=none`, scenario `presses`) are the way until that is fixed. (2) **someone closes lane W's
  game windows**: Esc in the foreground, then "window: closed by the user or the system" 30-60 s in (bbounce m6,
  agsob m6) — likely a person at the machine; lane W stopped launching. Next steps per game: MK8D waits for lane G
  (picture, 60 FPS); Jamboree / Anime Girls / Beach Bounce: a `presses` run to reach menus, then gameplay scenarios;
  Sonic: re-run (its last run predates the system-resource fix); Smash: find why nnMain runs 140 s before its first
  present (main thread busy, ~180 SVCs/s: `NXOS_TRACE=fs,svc`), then the menus.

**Lane W2 (2026-11-19 evening): Switch 1 library on the main tree.** Engine: `target\W2` (`cargo build --release -p
cli -p front`, CARGO_TARGET_DIR=target/W2) from the main tree (120a8a0, then f0bd36f, plus the other lanes' uncommitted
files of the moment); every lane W game relinked from it. Runs go through `scripts/library_play.py` with
`NXRS_LANE=W2 NXRS_ESC_QUIT=0 NXRS_NO_ACTIVATE=1 NXRS_CLOSE_QUIT=0`; runs are under `build\I\runs\*-W2-*`.
- **shaders `formats/smash.rs`: Smash 1.0.0's data.arc.** The 1.0.0 file-system table is stored uncompressed (it
  starts with its size, not 0x10) in the older layout: a 0x34-byte header (size, directories, directory offsets, file
  infos, file data, -, child hashes, -, second directory offsets, second file data, 0x10, 0x10, +0x30 u8 regions),
  the stream header and tables as later, the regions (12 each), directory infos (0x34, hashes inside), directory
  offsets (0x1c), child hashes (8), file infos (0x28: path, extension, parent, name hashes, file data index, flags),
  file data (16). Every directory offset names its range of file data (offsets relative to it): 533,796 files.
  `files_v1` reads it (unit test `table_v1`). The build now prebuilds 27,517 shaders (was 77).
- **Smash's 141 s to the first frame was the shaders**: the old run missed 3,567 shaders and the main thread waited
  for each live translation (0.1 s each, `live shader ... translated ... 107.7 s after it was missed`). With the
  index read, the first present comes at 3.5 s and no shader is missed (`20261119-190947-W2-smash-trace`). The fs
  and SVC trace showed nothing slow (the 38 MB table read in 61 ms). `NXOS_TRACE=fsread` (new, os `fs.rs`) logs
  every file read with its time.
- **runtime `kernel.rs`: SetThreadActivity (0x32) and GetThreadContext3 (0x33)**, as Turbo: the first accepts (the
  thread keeps running), the second copies the thread's `Cpu` (x0-x28, fp, lr, sp, pc, pstate, v0-v31, fpcr, fpsr,
  tpidr; 0x320 bytes). Unity's garbage collector (Anime Girls) called them 120 / 56 times a minute as gaps.
- **os `simple.rs`: nn::ssl Initialize / Finalize keep a count**; Finalize fails once it is back at zero.
  Sonic X Shadow Generations finalizes in a loop until it fails (`while (nn::ssl::Finalize().IsSuccess())`, main
  +0x122eeb0); the stand-in's constant success hung it at 11 s with no present.
- **Multi-program applications: oe::ExecuteProgram (os `oe.rs`, translator `app.rs`, cli `--program N`,
  `scripts/run_game.py`, `play_scenario.py`).** `nxrs-build build <nsp> --program N -o <name>_p<N> --outdir <same
  folder>` builds program N (title id + N; `NXRS_PROGRAM_INDEX`). ExecuteProgram(i, param, size) starts
  `<name>_p<i>.exe` (program 0: `<name>.exe`) beside the running one with the same arguments, `NXRS_LAUNCH_PARAM`
  (a file with the parameter) and `NXRS_PREV_PROGRAM`, logs `exec: program a -> program b (...): <exe> started as
  process N` and exits; `TryPopLaunchParameter` returns the parameter once, `GetPreviousProgramIndex` the caller.
  run_game follows the successor: it watches process N, rewrites GAME_LOCK to its executable (so gamectl's
  `check_owned` accepts it) and `run_game.current(pid)` gives play_scenario the live PID. Sonic X Shadow
  Generations: program 0 is the title menu and Shadow Generations, program 1 Sonic Generations
  (`build\games\sonic\sonic_p1.exe`, 100,478 functions, 4,893 shaders).
- **graphics `vk/exec.rs` (lane W2, small, for lane G): texture slot pairs.** A texture's `cbuf_slot` may pack two
  banks (Ryujinx: texture slot | (sampler slot + 1) << 16; Unity's NVN shaders use 0x30002). Graphics read the whole
  value as one slot and got 0 for every handle: every Anime Girls texture was id 0 / sampler 0 (black frames).
  `tex_slots` splits it for the texture word, the sampler word (kind 1) and the set cache test.
- **graphics `vk/exec.rs` + `shader.rs` (lane W2, small, for lane G): no 3 s wait for a refused shader.** A draw
  whose missing shader comes from the live translator but cannot be built (`texture array of 8192 (handle tables) is
  not bound yet`) waited the full 3 s each time: Jamboree stopped presenting for 70 s after its opening movie (23
  such shaders). The wait now ends when the module is refused (`Library::refused`).
- **front `window.rs`: `NXRS_NO_ACTIVATE=1`** shows the game window without taking the focus (scripted runs: the
  window no longer pops in front of whoever works at the PC); **`NXRS_CLOSE_QUIT=0`** ignores one window close and
  quits on a second within 10 s (a person who insists still closes it). Both logged. Closes still came: Jamboree
  `20261119-202422-W2-jamboree-play` was closed twice at 308-311 s with the window in the foreground (a person).
- **scripts (lane W2):** `library_play.py` scenarios `sonic_shadow`, `sonic_gens`, `smash_match`, `agsob`, `jamboree`,
  `boot` (a trace run); replays in `scripts/inputs`: `run_right.txt` (platformer: right + jump), `hold_a.txt` (A held 3
  s), `smash_fight.txt` (character pick from the top-left corner, then moves and attacks in 20 s blocks, no B),
  `shooter.txt` (walk, turn, ZR, jump). The replay's ly is positive up (Smash's first pick moved the hand up).
- **Lane W2, second round (2026-11-20 00:55).**
  - **os `media/sink.rs`: AudioOut buffers go back on a wall clock (Turbo's audout clock).** A block was released
    when the device's playback reached its end. Two consequences: a producer that keeps two 5 ms buffers in flight
    and refills on release (Beach Bounce, AudioOut #1) saw its queue drain at every device period (10 ms+ callbacks):
    ~38 underruns a second; and once the smooth slowdown slowed playback down, the releases slowed too, the producer
    slowed with them and the stream stayed slowed (Sonic Generations: "slowed down" for the whole 723 s run). Now a
    "nxos audio clock" thread (every 2 ms) releases each block once 48 kHz of wall time from the stream's first
    frame has passed its end (the clock waits, not bursts, when the producer is late); the samples were copied at
    append and the device plays them from the queue as before. Tests: `releases_on_the_wall_clock`,
    `the_clock_waits_for_a_late_producer`. Results (the PC had **no output device** from ~23:30 on: these runs were
    consumed by the sink's real-time clock, so a run with a device is still owed): Sonic Generations 1 underrun,
    16.6 s slowed in 303 s (was the whole run; `20261120-003954-W2-sonic-gens5`); Beach Bounce 582 underruns, 5.2 s
    slowed in 719 s (was ~38 a second and the whole run slowed; `20261120-002042-W2-bbounce-vn`; what is left
    follows its own frame hitches, up to 65 ms).
  - **Smash: 10+ min of back-to-back matches** (`20261119-235023-W2-smash-match5`, `smash_fight.txt`: B, then the pick
    from the top-left corner every 20 s, so the character select after each match is passed): 3+ matches vs CPU
    (Pikachu, Link, Donkey Kong) on different stages, 58.2 FPS over 150-840 s (569 of 676 s at 58+; p99 213 ms from
    pipeline builds when a new stage or fighter appears), no crash, quit cleanly, sound 15 underruns in 784 s.
    **Open: private memory 5.6 -> 6.9 GB (+108 MB/min)** while graphics' allocator stays at ~680 MB: the growth is
    outside the vk allocator (driver pipeline objects for each new stage / fighter, or the game's heap commits);
    a longer run tells whether it levels off.
  - **Sonic Generations:** `spindash.txt` (run + full jump, then a spin dash, 3.8 s cycles) survives the badniks (14
    rings kept, no death in 5 min: `-sonic-gens5`, `-sonic-gens4`), but Classic Sonic stops at a ledge behind a
    totem pole ~1:30 into Green Hill and the script does not get him up it; finishing the act needs a recorded route
    (`NXRS_INPUT_RECORD` from a person's play, then replayed). 31.5 FPS, picture right, memory flat.
  - **Shadow Generations: gameplay reached; the intro never looped.** The flow is title -> Shadow -> Down (the cursor
    starts on the "Shadow's Story" header) -> A (New Game) -> **a save-slot chooser** -> A (slot 1) -> a short intro ->
    the first stage (Space Colony ARK). The earlier "loop" was the skip script's A presses walking back through the
    chooser and the story menu, not the movie decoder: the game feeds every VP9 packet with time 0 and no EOS flag and
    Stops the decoder itself, which os answers correctly. Run `20261120-011106-W2-shadow-watch` (`shadow_watch`, then
    `boost_run.txt`: stick forward, ZR boost, B, A): **18 min of the stage**, 30.0 FPS (the game's target; p95 37 ms),
    private memory flat (6.86 -> 7.07 GB), picture right, quit cleanly; Shadow reaches a robot fight (`B -> Y` prompt)
    and the script stops there. No output device on the PC: no sound evidence.
  - **Jamboree white screen: handed to lane G (blend state of the last pass, lane G findings above).** Ruled out
    first: the nifm network check (it finishes), a lost wake-up in the nn::async executor (forced 50 ms wake-ups
    changed nothing), failed file opens, and the game logic itself (the hub's BGM plays; the fade quad's colour is
    (1, 1, 1, 1) in guest memory, so the game does not wait with the screen faded: it draws a white layer that should
    not show).
  - **Open findings at the stop (2026-11-20 02:05):**
    - **Smash memory: the cause found and fixed, not yet re-measured.** The growth was runtime `fiber.rs`: a fiber
      finalized while suspended kept its host fiber (stack committed, 256 MB reserved) so it would not be freed under
      live frames; Smash finalizes suspended job fibers all the time (11,471 in 14 min of `-smash-match5`, ~100 MB a
      minute). Now the host stack is freed (no lock is held across a switch and nothing can resume a finalized fiber;
      the fiber's `Arc` and `Cpu` are leaked, a few KB). The guest side is flat: physical maps 1.57 -> 1.79 GB in the
      first 4 min, then flat (new log line each minute: `memory: guest heap N MB committed, physical maps N MB`, os
      `watch.rs` + runtime `mem.rs Space::committed`). **The 20-min check could not be done: every Smash run since
      01:40 hit `VK_ERROR_DEVICE_LOST` 150-310 s in** (`20261120-014239-W2-smash-long`, `-014751-W2-smash-long2`,
      `-015904-W2-smash-long3-noasync` with `NXGFX_ASYNC=0` too), at new-pipeline hitches (pipeline lookups 440 ms);
      other lanes' games ran on the GPU at the same time. For lane G: reproduce Smash alone; the 23:50 run on the
      build before lane G3's e503854 played 14 min without it.
    - **Shadow Generations has no video end-of-stream problem** (the "loop" was the script walking back through
      menus; see above); its open items are only the sound check and the owner's hand test.
    - **Jamboree fade: handed to lane G** (the white quad the game sets to (1, 1, 1, 1) should not cover the hub:
      blend / write mask of pass 108; lane G findings).
    - **Sound re-check owed:** since ~23:30 the PC has no default output device (`[RED] audio: no output device` in
      every run log), so the sink's wall-clock release (Sonic's stretched sound, Beach Bounce's ~38 underruns a
      second) has no evidence with a real device yet. Before counting a run as sound evidence, check its log for
      `audio: output on <device>`. Beach Bounce's remaining underruns (~0.8 a second on the clock fallback) still need
      comparing against its 65 ms frame hitches on a run with a device.
    - Sonic Generations: a recorded human route (`NXRS_INPUT_RECORD`) is requested to finish Green Hill; the scripted
      spin dash stops at a ledge ~1:30 in.
    - MK8D, Anime Girls: re-check after lane G's posterization fix.
  - **New tools:** control command `peek <address> [bytes]` (os `watch.rs`: guest memory as hex words, readable pages
    only; how the lock owner above was read); `library_play.py` scenarios `sonic_gens2` (spin dash), `shadow_skip`,
    `vn` (A every 4 s for 11 min, shots each minute); replays `spindash.txt`, `run_jump.txt`, `skip_hold.txt`.
  - **scripts `run_game.py` (lane W2):** a run that handed over to a successor (ExecuteProgram) lost its claim to
    another lane's stale-claim sweep (it read the claim just before the hand-over rewrote it with the new pid); the
    watch now writes the claim again whenever it is missing while the game runs.
  - **Beach Bounce:** the window shows the scenes now (lane G3's shared images), 11 min of the story at 59.6 FPS,
    memory 1.80 -> 1.94 GB, no crash (`20261120-002042-W2-bbounce-vn`); the run hit the scenario's hard timeout
    (780 s, now 900 s) instead of quitting.
  - MK8D and Anime Girls: waiting for lane G's posterization fix; Smash's last match (`-smash-match5/0024`, Donkey
    Kong) shows the same bands.

## Switch 2 library (lane X, 5B)

Builds (engine from `target\X`): `build\games\{mkw,dkb,wonder}` (`nxrs-build build <NUP> -o <g> --outdir
build\games\<g> --jobs 4 --keys <launcher keys>`). Runs: `build\I\runs\<time>-X-<label>\` through
`build\X\xplay.py <exe> <label> <seconds> "<steps>" [--env K=V]` (run_game.py's guard; steps `wait`, `press BTN S`,
`hold`, `replay FILE S` (`build\X\inputs\`, `scripts\inputs\`), `threads NAME` (control pipe dump), `loop N k`;
writes `summary.json`: FPS / private MB per minute, the memory slope after 60 s, underruns). `build\X\xq2.sh <out>
<xplay args>` waits for GAME_LOCK. **Pictures:** `NXGFX_DUMP_AT_S=<s,...>` + `NXGFX_DUMP_DIR` (graphics' pass dump,
recorded in the submission: safe); the last 1920x1080 / window-size R8G8B8A8 pass of a dumped frame is the picture.
Not `fullres`/F8 (deadlocks, lane G section) and never a screen grab (it reads whatever covers the window).

**Fixed / added (lane X, uncommitted):**
- os `services/vi.rs`: the Switch 2 SDKs' vi calls nxnative takes over (HDR status / paper white / metadata set and
  clear, VRR support, extended swap interval, buffer max resolution, CreateLayer with LayerCreationSettings,
  InitializeWithVrrLfcEnabled, indirect image requirement). Mario Kart World called SetLayerNextHdrContentGamutMetadata
  through the SDK's null vi2 table (`rt_dispatch: no function at guest pc 0x0` 4 s after the first present).
- os `services/hid.rs`: GetTouchScreenState(s)<N> for every N = 1..16 (only 16 was taken over; each writes its own
  size). Donkey Kong Bananza aborted in GetTouchScreenState<1> (Result 0x52ca) at its first present.
- os `nvn/nv.rs`: `nn::gs::sdk::QueryApplicationGpuTimeSpan` (the game is the GPU's only client); nv::MemoryInfo is
  0x40 bytes from SDK 11. `nvn/mod.rs`: the SDK major skips a variant name (`nnSdk-Generic_21_4_0` was read as "no
  version": MKW is SDK 21, DKB 20). `services/misc.rs`: `pl::IsRunningOnOunce` from the profile's platform.
- runtime: `NXRT_FATAL_PEEK=<chain>[;...]` (crash report: `sp+0x90>0x2a78>0` = follow pointers, dump 0x100 bytes);
  the thread dump (`threads`, stalls) names the host function a thread is in.

**Open (hand-off):**
- MKW: first present 25-69 s after start (graphics' RegisterTexture, lane G section), then 4-5 s later a null object
  in the game's render thread (`main+0xa66e0`, or a host NVN proc reading 0x8 from `main+0xa0040`); the object at
  `[sp+0x90]` has its `+0x2a78` heap pointer 0 (`build\I\runs\20261119-155719-X-mkw8-peek`). Not changed by
  `SWR_XMON=1`, `NXGFX_WB_LAZY=0` or `NXGFX_POOL=host`. Next: which NVN query feeds that object (texture / pool
  sizes for NVN2) and a PDB for the code DLL so host frames get names.
- DKB: title at 54-60 FPS; 20-35 s in, after the game writes `save:/config.sav` and commits, it faults on bad
  pointers in its own data (`0x100000001`, `0x100000062`, null+0xa0, a branch to code bytes) at a different place
  each run; not changed by `SWR_XMON=1` or `NXGFX_WB_LAZY=0`; one 120 s run with `NXGFX_POOL=host` and one without
  the commit did not crash. Sound: 0.9 s not silent in 2 minutes of title (check the title music).
- Wonder: the NUP is the Switch 1 game 1.2.1. 12.5 minutes from boot through the title and file select into the
  first course with scripted input (`build\I\runs\20261119-164407-X-wonder4-10min`: 30 x A, then
  `build\X\inputs\wonder_run.txt` (run right, jump) ten times), exit 0, pictures right (pass dumps at 60..700 s),
  674 s of sound, 0 underruns. Left for Playable: FPS (pacing median 59.6, per minute 54-59.6, p10 52.7: single long
  frames, not yet attributed) and memory in the first 5 minutes (3.7 → 4.8 GB, then flat for 7 minutes).

## Switch 2 and Pokémon gameplay (lane X2, 5B / M6)

Engine: `target\X` (main tree at 120a8a0 + the working tree), games relinked with a PDB by `build\X\relink.sh <game>`
(`build\games\{mkw,dkb,wonder,arceus,violet,plza}`). Runs: `build\I\runs\<time>-X-x2-<label>\` through
`build\X\xplay.py` / `build\X\xq2.sh` (lane `X2`; `NXRS_ESC_QUIT=0`, `NXRS_NO_SONY_HID=1` by default; `shot NAME` is
graphics' own `fullres` now; new step `interactive S`: lines appended to `<run>\cmds.txt` are sent to the PID the run
started: `press BTN`, `hold BTN S`, `shot NAME`, `replay FILE`, `raw <pipe command>`, `end`). Saves backed up in
`build\X\saves\`.

**Debug instrumentation of lifted code (lane X2 tool, never in a game's own build):** `build\X\inst\inst.py <game>
<dir> <addr>:<mode>...` copies the chunk of each lifted function, compiles it with the game flags and writes a
`link.rsp`; `build\X\ilink.sh <game> <dir>` links it with `target\X` and a PDB; run it with `NXRS_DATA` /
`NXRT_EXEFS` set to the game's data (read only). Modes: `null` (log calls returning 0 with x0..x7 and lr), `all`,
`allN` (also dump 8 qwords at xN and at the result + 0x40), `line` (line tables: `llvm-symbolizer` maps a crash's host
address to the C line, i.e. the guest instruction), `crumbs:<reg>` (per-thread ring of the last 64 labels with the
register's value, dumped when the function dereferences that register while it is 0; `INST_DUMPS=<label>,<reg>,<lo>,<hi>`
dumps memory at a label). `build\X\dis\` has the flat images of MKW / DKB `main` for `llvm-objdump`.

**Mario Kart World's crash, root cause (os/runtime side done; for lane G3):** the null is not an OS allocation. Over
three instrumented runs no sead heap allocation (`main+0x3e3550`), GPU memory block (`main+0x5d6840` / `+0x5d72f0`) or
free-list allocation (`main+0xcff680`) returned 0. The crash path (`x2-mkw9-crumbs`, labels of `main+0xa66e0`): the
render thread asks **agl::utl::DynamicTextureAllocator** (the object at `[sp+0x90]`, initialised by `main+0xeb1870`
from `main+0xd65530`) for a render target; its fixed pool is the 285 MB virtual pool aliased at 1.1 s
(`0x11d00000`, ctx+0x2a70), full; it may grow only with a heap (ctx+0x2a78) and a chunk size (ctx+0x2a80, 32 MB), and
the game's init struct passes heap 0 (`x2-mkw15-initparams`: struct+0x10 = 0, from `[obj+0x5a0]`, zeroed in its
constructor `main+0xde5908` and never set), so `main+0xa7f14` takes the out-of-memory path (`main+0xa7ffc`: seven
`main+0x25e0e60` purges, then x22 = 0) and `[x22+0xa0]` faults (`main+0xa7234`), or the null command buffer of the
next event reaches `nvnCommandBufferWaitEvent`. The requests before it (`x2-mkw13-allocs`, every call of `main+0xa66e0`
with its descriptor and result): 45 distinct targets, **319 MB of storage by our `nvnTextureBuilderGetStorageSize`**
against the 285 MB pool, e.g. 2560x1440 RGBA8 = 0xf00000 (1536 rows: 16-GOB blocks), 2560x1440 8-byte = 0x1e00000,
3072x3072 = 0x3600000, 1280x720 = 0x3c0000. Handheld (`--option docked=0`, `x2-mkw14-handheld`) and
`NXOS_GFX_SCALE=1` (2048x1152 targets, `x2-mkw12-scale1`) crash the same way. So either our storage sizes / alignments
for NVN2 render targets are larger than the driver's (the pool fits on the console), or the game creates more targets
than on the console (something it reads decides their count / size), or it frees them later (they are released by a
GPU-progress check). Not the scheduler (`NXOS_STEAL_ANY_MS=0`, `x2-mkw3-nosteal`), not the write-back (below).
**For lane G3:** the sizes above against the real NVN2 driver; the entries' reuse (ids `7d05`.. come back, offsets are
reused) suggests frees do happen.

**Write-back lands only what the GPU changed (graphics, lane X2, Turbo parity; lane G please review):**
`vk/queue.rs submit_sub` copies each write-back range as the device had it before the submission's work (into a
"write-back base" buffer, recorded in `pre` after the uploads), and `vk/mem.rs land_changed` writes into guest memory
only the bytes where the device's result differs from that base, page by page (Turbo `vk_mem.c wr_land`, "a storage
buffer is written in part"). Landing the whole range put stale device bytes over what the CPU wrote meanwhile
(a read-write storage buffer over a pool the CPU also writes; a size-0 one runs to the end of its pool, up to 1 MB).
The pools line now counts them: `(N CPU-written bytes inside write-backs kept so far)`; Super Mario Bros. Wonder kept
1,936 in its first 4 minutes (`x2-wonder1-10min`), DKB 0. It is not the MKW / DKB crash.

**os `sched.rs`:** `NXOS_STEAL_ANY_MS=<ms>` (debugging; default 20, 0 = never): how long a waiter waits before it
takes a core from an equal or better holder (Horizon never does). The stats line counts those steals separately
(`N steals (M from an equal or better holder)`) and the first 16 are logged with both threads. **os `hid.rs`:**
`KeyboardState` is 0x30 bytes (nxnative, yuzu); 0x38 wrote 8 bytes past the caller's (no Switch 2 game here imports
it).

**Lane X2 status (2026-11-19 19:45):**
- Mario Kart World: crashes 4-5 s after its first frame (root cause above); blocked on lane G3's sizes.
- Donkey Kong Bananza: crashes within 0.2 s of the second A on the title (the first A commits the save), at a new
  place each run: `main+0x1943c0` write of 0 (`x2-dkb1-wbfix`), `main+0x454cb4` read of -1 (`x2-dkb2-lines`),
  `main+0x372de4` (`x2-dkb-nosteal`, no equal-priority steals); with `NXOS_FREE_CORES=1` it dies at 11 s
  (`x2-dkb-free`), so it is corruption that depends on how threads interleave. Ruled out: the write-back (0 CPU bytes
  kept), equal-priority steals, the HID state sizes it imports (touch, Npad, six-axis, Jcms checked against
  nxnative). The address arbiter and condvar SVCs match Turbo's `kernel.c`. Next: crumbs on the faulting functions'
  pointers at the moment of the second press; Turbo runs this game with every thread free, so compare its loader
  and file reads around the save load. Title music: the BGM stream is never opened (0.9 s of renderer sound in 60 s).
- Super Mario Bros. Wonder: 12.7 min, no crash, picture right, sound, memory flat after minute 5; FPS median 59.8 /
  p10 54.7 (another lane's build ran during minutes 2-5). Not marked Playable.
- Arceus: Jubilife Village right with `lsfg=0` (the garbled village was frame generation); 30.0 FPS, 620 s.
- Violet: in-house gameplay 20 min, 30.0 FPS, sound, flat memory; the story step that opens the front door was not
  found (the game answers "You're not quite ready to go to school yet").
- Z-A: 600 s to the Lumiose station platform, no crash, sound, memory flat, 46.7 FPS median (p10 43.5): FPS is
  lane G3's (`x2-plza2-lumiose`).
- **Windows closed from outside (for the coordinator):** Z-A `x2-plza1-lumiose` ended at 157.7 s with
  `window: closed by the user or the system` while only control-pipe presses were being sent (as lane W saw).

**Donkey Kong Bananza's random crash: root cause and fix (lane X2, os `sched.rs` + runtime `kernel.rs` / `boundary.rs`).**
The crash after the save commit was two guest threads running guest code on one virtual core at once. A waiter
that waited 2 ms for a core held by a worse thread (20 ms for any thread) took it, and the holder "ran on until its
next checkpoint": on Horizon the holder is paused, here both ran, and the game's per-core / same-core data (loaders
decompressing in parallel when the game starts) was corrupted at a different place each run. Experiments
(`build\I\runs\20261119-2*-X-x2-dkb-*`): one core with no steals (`NXOS_CORES=1 NXOS_STEAL_ANY_MS=0
NXOS_STEAL_WORSE_MS=0`, with or without `SWR_XMON=1`) and three cores with no steals both ran clean past the point;
the exclusive monitor alone (`SWR_XMON=1`) and every-thread-free (`NXOS_FREE_CORES=1`) still crashed. **Fix:** the
runtime tells the scheduler when a guest thread enters and leaves a host function (`Scheduler::host_enter` /
`host_leave`, around `nxrt_host_invoke`); a core is taken only from a holder that is inside a host function (no guest
code runs there), checked Dekker-style against its leaving (SeqCst on both sides); a holder whose core was taken waits
for a core in `host_leave` before it returns to guest code (Horizon: it was preempted). A holder in guest code keeps
its core until its next checkpoint (SVC or marked spin), or 5 s (`STEAL_GUEST`; 200 ms still broke DKB at 38 s, `x2-dkb3-opening`, logged: `took core N from thread
.. which ran guest code with no checkpoint`). The stats line counts those (`... from a holder in guest code`).
New debugging switches: `NXOS_CORES=<n>`, `NXOS_STEAL_WORSE_MS=<ms>` (0 = never). With the fix DKB ran past the
start of the game into its opening (`x2-dkb-hoststeal2`), with sound (20,291 of 35,852 renderer frames not silent).
**os `nvn/nv.rs`:** `nv::InitCheck` tests that nv::InitializeGraphics ran (nxnative's `inited`), not the pool's
address: Switch 2 games pass no pool (DKB: 0 bytes at 0x0) and DKB calls InitCheck from main+0xe19d04 when its
opening movie starts; it was a fatal. The "nearly silent title" was the new-save mode selection (Normal / Assist),
which has no music; the music plays once the game starts.

**Lane X2 (2026-11-20 00:45): MKW 1.8.0 render-buffer pool patch ported; MKW still stops at 15.5 s.**
- Profile `src/profiles/games/0400C3F00006E000/turbo.toml` (set `mkw_rbpool`, `NXRS_PATCH_MKW_RBPOOL`, build
  d00e4f399e411d06): mkw-recomp's four sites (pool table 2 slots; `bl` at main+0xd65720 / main+0xb6f90 to the patch
  page + 0xf90 / 0xfc0). nxrs applies them at build ("patch applied" x4 in `build\games\mkw.buildX2.log`).
- Runtime `hostfn::alias` / `aliased` + `dispatch.rs`: a guest address outside the host window (the patch page)
  runs a host function. Os `gamefix.rs`: the hook diffed line by line against mkw-recomp's `patches.c`
  (`shadow_second_pool`): same calls, same arguments (ExpHeap::create main+0xbeb4e0 (region, size + 2 MiB, name, 1, 0),
  operator new main+0x3e3550 (0x58, heap, 8), pool init main+0xeb1e60 (pool, 0, 1, 7, count, size, 0x104 or 4, heap)).
  The pool's +0x10 field (8 vs 0x939) is not a flag: it is the GPU memory pool's serial number (`main+0x25b1290`
  `ldadd` on a global counter, stored at the pool object's +0x10c), so 0x939 is just the 2361st pool. The "empty
  regions" were a fresh pool: region +0x30 / +0x38 count bytes in use.
- Fixes since: (1) **runtime `boundary.rs call_guest`** now leaves the "inside a host function" state for the guest
  code it runs (`Scheduler::host_suspend`, os `sched.rs`) and re-enters after: a host function calling guest code
  (gamefix, HLE callbacks, the graphics layer's command-memory callbacks) was stealable for the whole call, so two
  threads could run guest code on one virtual core (the DKB race class). (2) The second pool is sized by mkw-recomp's
  rule for the scene's lines (`NXRS_MKW_SCENE_LINES`, default 2160 docked / 1440 handheld: 0x24b00000 docked) and made
  right after the allocator's init (`NXRS_MKW_POOL_MODE=map` for mkw-recomp's first-shadow-map timing; `grow` gives
  the allocator a growth heap at ctx+0x2a78 instead, the game's own path `main+0xa7f14`: it adds one 32 MiB pool and
  then fills both slots). Pool 0 fills before the first shadow map here, so the map timing never helped.
- Result: with the early pool, allocations that miss pool 0 now land in pool 1 (`x2-mkw35-free`: three pool-0 misses,
  no pool-1 miss, no out-of-memory path), and MKW stops at the same 15.5 s mark (docked and handheld, `x2-mkw46`) on a
  **double free** instead: `main+0xa14e8` (scene event 0xf in `main+0xa13b0`, called by `main+0xa07b0`) frees the
  render buffer at `[x21+0x98]` twice with no allocation between (`x2-mkw42-rb`: the same buffer freed twice in a
  row, its block +0xc0 cleared by the first free; pool index 0), and `main+0x47ff40` reads the null block. Other runs
  fault at main+0xcbfb0 / main+0x26bd20 on a render buffer without its texture, the same family. The sizes of the
  pool are not the cause (a 0x4f040000 second pool stops the same way). Next: why `main+0xa07b0` with event 0xf runs
  twice without its allocating branch (`main+0xa0bfc`: three flag tests at main+0xa0c94..0xa0cc4) and what
  `[[0x36cd6b8]+0x424]` (the test that skips the free at main+0xa14a0) reads; it is the same 15.5 s mark with the
  patch off, where the out-of-memory comes first.
- Display: MKW 1.8.0 makes its 3840x2160 swap chain at 0.8 s before any display query; `NXOS_DISPLAY_MAX=0..3` (new,
  os `vi.rs`) caps QueryDisplayBufferMaxResolution for tests (MKW never calls it).
- **The double free, traced (2026-11-20 01:10, `x2-mkw47-ev`, `x2-mkw49-view`, `x2-mkw50-ev`):** `main+0xa13b0` is a per-view render-phase handler: phase 0 allocates the view's render buffer (`main+0xa163c` -> slot+0x98), phase 0xf frees it (`main+0xa14e8`); the event 0xf branch in `main+0xa07b0` (flag tests at main+0xa0c94..0xa0cc4) only picks arguments for a lighting call, it allocates nothing. The phases come from `main+0xb5080` through `main+0xa0040`: phase 0xf is sent every time (main+0xb5928), phase 0 only when `main+0xa26b0` returns 1, i.e. when bit 4 of the view's entry word (table at `[[obj+0x1e38]+0x18]+0x10`, 0x4468-byte entries) is clear. Traced: one phase 0, then 0xc / 0xf from four render contexts (`x21` = 0x1130aa798, 0x11b2fc798, 0x115b1c798, 0x11d2ab798) on the same view (index 0), two 0xf -> the second frees the freed buffer. So on the console that bit is clear every frame for view 0 and here it stays set after the first frame. Next: find the writer of bit 4 (likely a "skip this view" state tied to GPU progress, an NVN query / sync result or the present cadence) and compare its input with Turbo / mkw-recomp. Not the scheduler steals (`NXOS_STEAL_ANY_MS=0` same), not game dynamic resolution (`game_dynamic_resolution=0` same), not the camera service (ours answers 0x7c4 as mkw-recomp's stand-in); `NXOS_CORES=1` dies earlier (main+0x308520 at 5 s).

**Lane X2 (2026-11-20 00:50), other games.** Super Mario Bros. Wonder, exclusive 229 s on the new scheduler (`x2-wonder-sched2`): median 60.0 / p10 59.9, memory flat, no crash; with the `call_guest` change relinked (`x2-wonder-suspend`, 150 s) 59.3-60.0 per minute, no crash. Arceus "lost progress": the game's, not ours (the 21.7 min run wrote nothing to the save or its work tree; at that story step the game has no menu Save, only its autosave points). Violet: in-game Save commits (`fs: commit save0`, `x2-violet5-obj`); still no way out of the house (menu has Options / Save only, Mom not found). Saves backed up in `build\X\saves\` before each run. **Concurrent runs in one lane share its claim file:** an exclusive run of mine waited while my own non-exclusive runs rewrote and removed `GAME_LOCK.X2`, and run_game's give-up path then failed on `os.remove` (FileNotFoundError). Exclusive runs now use their own lane name (`X2W`).

**For lane G3, DKB after 4652dc2 (crashed-pipeline guard), lane X2 2026-11-20 01:30:** session 1 (`x2-dkb-g3fix1`) ran 59 s into the title / opening and died in the driver (exit 0xC0000005); the guard recorded that pipeline ("1 pipeline(s) were compiling when the last session ended"). Every later session (`x2-dkb-g3fix2` .. `5`, with `NXRT_FOREIGN_FAULTS=1`) dies at **2.8 s** in `nvgpucomp64.dll+0x3c1c150` (frame 0 `nvgpucomp64.dll+0x312335d`, read of 0) with the guard still at 1 pipeline: this compile is not caught by the guard (a different path from the one it wraps: pipeline-library parts made ahead at ProgramSetShaders, or a compute pipeline?), so DKB no longer reaches its title. Save backed up in `build\X\saves\dkb-0122\`.

**Pokémon: where each one is stuck (for the owner to play past by hand; saves backed up):**
- Arceus: Jubilife Village, objective "Head to the Wallflower" (the game's autosave at 2026-11-19 15:49; no menu / Save yet at this step). Save: `build\X\saves\arceus-0033\`. A hand play to the first night (Survey Corps quarters) and on into the Obsidian Fieldlands, then an in-game Save once the menu has it, gets M6's place.
- Violet: the house at the start, in uniform; the menu has Options / Save only (no Rotom Phone), Mom is in no room we found, and the step down to the entrance answers "You're not quite ready to go to school yet." Save (in-game Save of 00:45): `build\X\saves\violet-stuck-house\`.
- Z-A: Lumiose station platform after "Lumiose City! We made it!" (60 FPS there); the platform ends at a fence and walking right toward the station building leaves the camera fixed with the player out of view (`x2-plza4-gate`). Save: `build\X\saves\plza-0118\`.

**Lane X2 2026-11-20 01:45, follow-up on the stuck points and MKW:**
- New debug switch, os `services/fs.rs`: `NXOS_FS_TRACE=1` logs every file open (and failed entry-type checks), `=miss` only the failures.
- Violet (`x2-violet7-fs`): no failed load behind the missing Mom. Opens: `arc/data.trpfs` + `.trpfd` (the game reads its field, NPC and script data out of that one archive, so file-level opens cannot show them), the wwise banks, `event/trevt/bin/event.trevt`, `aoc1:/keyfile.dat`, `save0:/main`. Misses are all fallbacks: `config/trinity_config/data.trowc`, the localized `English(US)/*.bnk` (the root banks then open), `appli/data/*_eng.arc` existence checks, `save0:/poke_trade`. No ro / NRO use, no script-module load, no red line besides 20 shaders not in the library at boot. Not proven either way: Mom may be spawned and not drawn (a missing shader), or the save is past her scene. Next check: walk the living room for a "Talk" prompt.
- Z-A (`x2-plza5-fs` .. `x2-plza8-gates`): **not a stall.** The game keeps running (60 FPS, threads in their normal waits, `t0`/`t1`/`t2` thread dumps), the menu opens and shows the objective **"Head through the ticket gates to leave the train station."**, and walking back left brings the player into view: walking right / past the platform end leaves the camera where it was (the draw count drops 935 -> 220 because the view no longer holds the crowd). No file misses but `world/scene/startup/startup_0.trscn` (existence check). Whether the camera should follow there is open; the ticket gates are in the station hall to the right of the arrival point.
- MKW: the view release `main+0x47fca0` (clears bit 4 at `main+0x47fd90`) is called only from `main+0x47ed60`, which is called only from `main+0x47f470`, the render stage with code 9 (dispatched by `main+0x76f60` at `main+0x77808`, and by tail branches at `main+0x47e6b0` / `main+0x26280e0`). Instrumented (`x2-mkw52-rel`, `x2-mkw53-prep`): **neither `main+0x47ed60` nor `main+0x47f470` runs at all before the crash**, while the stage that sends the phase events (`main+0xb5080`) does. So the stage list never contains stage 9 for that view, or `main+0x76f60` takes another branch; `x2-mkw54-stage` logs the stage codes seen at `main+0x771a4` (0..0xa, 9 once) but the code-9 path at `main+0x777c0` is not reached. Next: why `main+0x76f60` skips its code-9 entry (the comparison chain before `main+0x777c0`), then what fills that stage list.

**Lane X2 2026-11-20 01:55:**
- Violet: no "Talk" prompt anywhere in the living room / by the TV / the side door (`x2-violet8-talk`), and the pet bed there is empty too: Mom (and the pet) are not in the room, not merely undrawn. The 19 "not in the shader library" keys are **stale prewarm keys**: they come from `violet_data\pipeline-keys.bin` (written by an earlier session), they are not in this game's library (`manifest.tsv`: 35,937 shaders, 0 failed), and no draw asked for them (`skipped: no shader 0` in every replay line). Not lane S's prebuild, not a translation failure, and not the reason Mom is missing.
- Z-A (`x2-plza9-hall`): the player cannot step right off the platform onto the hall's stone floor at the arrival point (she turns, does not move: `0001-h1`); walking back (down) she leaves the picture while the camera stays where it is (`0003-h3`, `0004-h4`). The camera follows only forward movement in these runs. Not reached: the ticket gates.
- MKW: the stage dispatcher `main+0x76f60` reads the stage code `[x20]` and the byte `[obj+0x901]` at `main+0x77150`: with bit 3 set only code 2 runs (`main+0x771d4`); otherwise codes <=3 go to `main+0x77694`, 4..6 to `main+0x77770`, 7 and 9 to `main+0x777c0`. Bit 3 is written at `main+0x4cd24c` from `[x0+0x79] != 0` (and `+0x910` gets the same byte); the path that clears `+0x79` (`main+0x4cd210`) runs only while `[x0+0x2dc] & 0x3f == 0`. Bit 1 is set by `main+0xbbdcb0`. Instrumented (`x2-mkw55-skip`): only ~15 dispatches before the crash (the scene runs 2-3 frames), the bit-3 path taken 4 times, codes <=6 otherwise, code 7/9 never. Next: what sets `[x0+0x2dc]` low bits / `[x0+0x79]` (likely a pending-load or not-ready state of the scene) and whether the stage list holds a 9 at all in the first frames.

**Lane X2 final (2026-11-20 02:10, loop stopped by the owner): open findings.**
- Violet new game (`x2-violet9-new`, save moved aside and restored afterwards; original back in place, backup also in `build\X\saves\violet-before-newgame\`): language choice and "Choose a look" work; after picking a look the screen stays the plain menu gradient for 2+ minutes while the game presents at ~30 FPS, no file errors. Most likely the name entry: os `swkbd.rs` shows a host text box and waits for PC typing, which a scripted run cannot answer. The rerun with `NXOS_SWKBD=auto` (answer the default at once) was stopped before it started. **Next:** rerun the new game with `NXOS_SWKBD=auto`; if Mom appears in the opening, the old save is the odd one; if not, it is ours.
- Z-A floor / FPCR: **not a physics bug.** The player was blocked by an NPC (the boy in the yellow hoodie and his Helioptile stand at the hall arch: moving diagonally shows "A Talk" / "Interact Helioptile", `x2-plza10-move`); diagonal movement works and the camera follows it. FPCR handling matches Turbo by design: MSR FPCR keeps FZ/DN/AHP/RMode (`c->fpcr = v & 0x7c00000`), MXCSR mirrors FZ as FTZ+DAZ, directed rounding (RMode != 0) takes the slow paths of `runtime/simd.h` (Turbo instead sets MXCSR RC; same results); Z-A writes FPCR in 9 places; FRSQRTE/FRECPE are emulated (`rsqrte_s` 1161 / `recpe_s` 677 uses). Open: the camera stays put when she walks into the arch area behind the benches (`x2-plza11-gates`): the picture shows the platform with the player out of view. The ticket gates are past that arch (objective marker to the right of the bench pillar). Save: `build\X\saves\plza-0118\`.
- MKW scene-ready flag: stage dispatch `main+0x76f60` skips stages 7/9 while `[obj+0x901]` bit 3 is set; bit 3 comes from `[x0+0x79]` in `main+0x4cd130` (called from `main+0x4cc580`), which clears `+0x79` only while `[x0+0x2dc] & 0x3f == 0`. `+0x2dc` has ~120 writers in main (many different classes use the offset), so the writer for this object needs a dynamic watch: a crumbs dump of `[x0+0x2dc]` / `[x0+0x79]` at `main+0x4cd204` per frame, then a write watch on that address. Expected answer: a resource-load or async-task count (fs async read, a thread signal, or a GPU fence the game polls) that never drains here.
- Tools left in `build\X\` (lane X2 only): `xplay.py`, `icmd.sh`, `inst\inst.py` / `crumbs.py` (crumbs now guard their helper definitions when two functions of one chunk are instrumented), `relink.sh`, `ilink.sh`, inputs.

## Lane O3 (os / runtime / front, 5A.1-5A.3, 2026-11-20)

- **runtime `process::start_main(space, npdm, entry, setup)`**: the main thread for a host that loads the modules
  itself (os): the kernel and start-up first, then `setup(&MainThread { priority, core, stack_size })` (os sets its
  counter rate and virtual cores there, before any guest thread exists), then the thread at `Entry::Address` /
  `Entry::Host`. `process::start` uses it. os no longer calls `create_main` itself.
- **runtime `Kernel::thread_count()`** (live guest threads), `rt::trace_spec(env, log)`, `profile::on()`.
- **SVCs (os `svc.rs`)**: an SVC nobody serves answers **NotImplemented 0xF601** (was success) with its gap line;
  0x2E / 0x2F NotFound; 0x30 / 0x31 / 0x37 resource limits, 0x6F GetSystemInfo as Turbo; 0x02 / 0x03 / 0x80
  recorded per page run (`mem::Memory::{set_permission, set_attribute}`) and reported by QueryMemory.
- **Metrics producers**: runtime `kernel::svc` adds `wait_{sleep,sync,mutex,condvar,address}` and `_us`; os adds
  `audio_underruns`, `audio_slowed_frames`, `ipc_requests`, `gap_answers` through `nxos::metric!("name", n)` (a
  per-site cached `nxconfig::metrics` counter: use it for new counters); os pushes one `frames-*.csv` row per
  present (`#present,guest_s,frame_ms,submits,nvn_calls,threads`). **Lane G4:** graphics may push its own per-frame
  rows with `nxconfig::metrics::frame` (they share the file; start them with a `#` header line).
- **New services**: `audren:u` (host mixer for SDK 1.x, `services/audren_ipc.rs`, with Turbo 8a265ab / a22b102),
  `appletOE` / `appletAE` (library applets over IPC: MiiEdit cancelled, PlayerSelect the console user, others
  closed with zero output; the proxy's other interfaces answer with gap lines; `services/applet.rs`).
- **Profile `present_interval`**: os's `profile::Profile::present_interval` (engine.tsv `display` or 1..3, or the data
  folder's `profile.tsv`) goes to `nxgfx::nvn::window::set_profile_interval` (`os::nvn::apply_present_interval`).
- **front**: `input: mouse raw motion ... -> right stick ...` lines (the first 24) when `mouse_right_stick=1`.
- **Test data**: `src/os/testdata/media/` (ffmpeg-made Opus / VP9 / AV1 / H.264 clips and ffmpeg's per-frame NV12
  FNV hashes); os tests that need kernel objects call `crate::mem::tests::test_kernel()`.
- **Tools (lane O3 only)**: `build/O3/relink.sh <game> <dir>` (a game's objects + `target/O3` nxfront into a copy),
  `build/O3/oplay.py` (scripted run through `run_game.run`: wait / press / hold / shot / replay / mouse / ctl;
  `O3_ARGS` adds `--key=value`). Copies in `build/O3/{mk8d,violet}`; saves backed up in `build/O3/saves`.
- **Patch table (translator `aot::patches`), new lines**: `area <set> <module> <words>`, `areloc <module> <index>
  adrp|b|bl state|area|module <addend>`, and a site word `@<op>:<target>:<addend>[:<word>]`. A module with an area
  gets a lifted code module `<module>_stubs` (in layout.json, so in modules.c and the dispatch table) after the
  last module (2 MiB aligned; the layout packs modules, so there is no room right after main); its first page is the frame-step state page. os skips `*_stubs` when loading the ExeFS and maps them RW.
  `Module::code(name, base, code, words, entries)` and `Module::extra_seeds` (function starts the table adds).
- **Profile patches placed by `nxrs-build`** (cli `profile::{stubs, trinity_sites}`): sets `fstep`
  (`NXRS_PATCH_FSTEP`), `stubs` (`NXRS_PATCH_STUBS`), `trinity` (`NXRS_PATCH_TRINITY`); `<data>\frame_step.txt`.
  **For lane K:** MKW's profile says `fps_target 60`, so its next build gets nxnative's frame-interval stub (main,
  one site) and the host's frame step (hold 1 at 60); `NXRS_PATCH_FSTEP=0` or `NXOS_FPS_TARGET=0` leaves the game's
  own interval. **Lane O3 builds a separate MKW copy** (`build\O3\mkw-fs`, from `build\games\mkw\mkw_data` as a
  folder input, engine `target\O3`) only to prove the frame-step stubs module translates, compiles and links (done
  04:35: `main_stubs` 1 function at 0x7005601000, main's site in set 10 `fstep` dispatches there, `frame_step.txt`
  step_max 2, `build\O3\mkw-fs.build.log`, linked); K's
  `build\games\mkw` and its saves are not touched, and O3 does not run it while K owns MKW. **For lane G4:** the
  frame step sets the hold through `nvn::window::set_profile_interval` (only when
  it changes, as that logs); a quiet setter would be better.
- **os `fstep`** (state page writer and step controller, at each present), **os `cheats`** (dmnt VM, per present;
  control commands `cheats`, `cheat on|off <name>`), **os `fpslock::main()`** (main's facts once booted).
- **Options, PC defaults** (owner, 2026-11-20: nxrs is a native PC app, not an emulator): `pause_on_focus_loss` 0,
  `controller_prompt` 1, `home_quit` 1, `keyboard_box` 1, and the PC's player: `pc_user` 1 (1..8: the profile
  opened at launch, the last-opened user, the account picker's and PlayerSelect's answer, always registered) with
  `pc_user_name` "Player" (`NXOS_NICK` overrides); test `account::tests::pc_user_option`.
- **Library applets, the call per applet** (owner rule: a host UI only where a PC player needs one):
  Controller (0x0C): host prompt, done (`controller_prompt`). Software keyboard (0x11): host text box, done
  (`keyboard_box`). PlayerSelect (0x10): no UI, the `pc_user` option answers. Error (0x0E): **built**, a message
  box beside the game on its own thread (`error_notice` 1; nn::err's ShowError Result / ErrorCode /
  ApplicationErrorArg message / the SDK's version errors; os `services::notice`). OfflineWeb (0x17): **built**,
  the game's page opens in the PC's browser (`offline_pages_in_browser` 1): `nxrs-build` now stages the Manual
  NCA's `html-document/` tree in the data folder (translator `package::app::manual_files`), nn::web's offline page
  start resolves the document path there. Neither opens under `NXRS_NO_ACTIVATE` or headless (logged). Tests
  `notice::tests::{error_texts, offline_page_files}`. Auth (0x0A, parental PIN): no UI,
  a PC has no parental lock. Cabinet (0x0B, amiibo): no UI until there is an amiibo-file option. DataErase (0x0D),
  Set (0x16), PhotoViewer (0x15): console settings / album, closed. NetConnect (0x0F), Web (0x13), Shop (0x14),
  LoginShare (0x18), WifiWebAuth (0x19), MyPage (0x1A): online, closed. MiiEdit (0x12): no UI, cancelled (the game
  keeps its default Mii).
- **Observation gates: not built, on purpose.** Every gate nxnative places (OBS_UE_MALLOC, OBS_REGION_ASK / GOT /
  FREE, OBS_STDALLOC_INIT, OBS_UE_REGION; `dm.h`, `hle_svc.c`) is a workaround for the iPhone's / Mac's address space
  ("not the game", its own comments): the `address space` purpose nxrs leaves out (512 GB guest space). No profile
  records an `[[obs]]` entry. A gate mechanism with no caller would be a fake feature; when a game needs one, the
  translator's site + host function call is the place.
- **Frame lock finders:** the generic target frame rate (nxnative `fps_target.py`) is in os `fpslock`
  (`target_field_find`, on main as loaded). Unity (internal-call table or code registration: targetFrameRate,
  QualitySettings.vSyncCount's walk), Unreal (t.MaxFPS / rhi / nvn.SyncInterval / Project.FrameRateLimit
  TAutoConsoleVariables; UEngine::FixedFrameRate on GEngine with its bUseFixedFrameRate bit) and Monolith (its
  reviewed wait / duration / setter fingerprints) run at build time in cli `fpsfind` and go to `<data>ps_lock.txt`,
  which os `fpslock` writes each present (the profile's `ue_fps_cvar`; Monolith's 0 = 2 vsyncs; Unreal's fixed rate
  only while the flag is set). Synthetic tests per finder (`fpsfind::tests`, `fpslock::tests::engine_places_from_the_build`).
  **Anime Girls: Sun of a Beach** (Unity 2021.3.28f1): no places, as nxnative's finder would say: its IL2CPP side
  resolves the call by name at run time (main+0x1827400) and the engine registers it in neither reviewed form.
- **Stubs that read the module at export: ported** (cli `stubgen`: nxnative's ctx here / emit / site / put /
  address, run at build time on the game's main): Hyrule Warriors: Age of Calamity's frame step (matched rule;
  the port run at 1.3.0's addresses gives exactly the build entry nxnative recorded:
  `hwaoc_matches_the_recorded_build_entry`), Ori and the Will of the Wisps' constant-buffer table lock, Xenoblade X
  DE's camera step, 0100A00019DE0000's render size (with its anchored find: the one caller of nn::vi::CreateLayer
  through its PLT stub). Synthetic-module tests for the last three (no recorded output exists for them).
- **Violet new save, stick replay (2026-11-20 04:37, `build/I/runs/20261120-043726-O3-violet-new5`)**: control-pipe stick
  presses (`ctl press LDown~1.5` ...) get the player up from the desk and walking the bedroom (shots 0005-0010);
  first save commit at 317 s; no Mom in the bedroom, and the replay did not reach the stairs, so "Mom missing" is
  still open (next: walk to the room's exit). Original save restored and diffed; the new save is in
  `build/O3/saves/violet-newgame-run5`. Three earlier attempts (04:03, 04:13, 04:24) were blocked by GPU-reset
  cooldowns from lane K's MKW runs (mkw13 / mkw14 / mkw15, started seconds before each).
- **Violet, walk to the stairs (04:51, `build/I/runs/20261120-045122-O3-violet-new6`)**: the player walks to the
  stairwell's brick railing and inspects the Skwovet plush (shots 0004-0013): input and game logic work. The stair
  opening is at the railing's north end, by the green mat under the window; this replay walked along its south
  side, so "Mom missing" is still not settled (next: from the desk, up toward the window, then left down the
  stairs). Save restored and diffed; new save in `build/O3/saves/violet-newgame-run6`.
- **Findings**: Violet new game passes name entry with `NXOS_SWKBD=auto` (the old run's gradient was the opening's
  wait, not the keyboard: no swkbd call had happened); the opening runs past 5 minutes, so "Mom missing" is still
  open. AudioIn: this PC has no capture endpoint now (DualSense mic disconnected). The owner was at the PC during
  the mouse run (raw motion logged at 4 s, the window closed at 41 s): mouse runs move the real cursor, do them
  only when the owner agrees.

## MKW (lane K, M7)

Engine `target\K` (`cargo build --release -p cli -p front`), game rebuilt with `nxrs-build build <NUP> -o mkw --outdir
build\games\mkw` (log `build\games\mkw.buildK.log`), relinked by `build\K\relink.sh mkw`. Runs:
`build\I\runs\*-K-mkw*` through `scripts\play_scenario.py` (`NXRS_LANE=K NXRS_ESC_QUIT=0 NXRS_CLOSE_QUIT=0
NXRS_KEEP_FOCUS=1 NXRS_NO_ACTIVATE=1`). Saves backed up in `build\K\mkw-save-backup-*`.

**What fixed the 15.5 s crash: the core mask.** MKW's NPDM gives its threads cores 0-5 (ACI0 ThreadInfo; mkw-recomp's
kernel answers GetInfo CoreMask 0x3f); nxrs gave every program 0x7 (3 virtual cores, nxnative's Switch 1 value). With
3 cores the game's render job system ran one view's phase events on four render contexts (lane X2's trace: two phase
0xf, the double free; stage 9 never reached). Fix (os, O3's files, small and separate): `profile.rs` reads a Switch 2
program's core mask from its NPDM (`npdm_core_mask`; Switch 1 programs keep 0x7), `sched.rs` `MAX_CORES` 4 -> 8.
`K-mkw5-cores6`: "core mask 0x3f", "6 virtual cores", past 15.5 s to the course-map loading screen with no fault.
**For O3:** please keep both; DKB (also Switch 2) gets its NPDM's cores too now.

**Also fixed in O3's in-progress `mem.rs` (one token, lane K):** `set_attribute` rejected attribute 0x10
(PermissionLocked, the RELRO lock every SDK module's DT_INIT sets): MKW aborted at main's DT_INIT (Result 0xE801,
main+0xf42348 -> sdk+0x1793a0). Allowed for svcSetMemoryAttribute now (mkw-recomp memory.c `ATTR_PERMISSION_LOCKED`).

**Patch layer.** mkw-recomp's `recomp/patches-mkw-180.txt` is the profile's `games/0400C3F00006E000/recomp.txt`
(the translator's own table format, which is that file's; new profile file kind `RecompTable`, keyed by `build main
d00e4f39…`, appended by `profiles::translator_patches`; MKW's `turbo.toml` is gone, its four shadow sites are in the
table). 149 entries: 97 spin waits + 43 sites in 9 sets + 9 set lines; every original word checked at build ("patch
applied: Mario Kart World: 9 code patch sets, 43 sites, 97 spin waits"). Host side: os `gamefix.rs` +
`gamefix/mkw.rs`, `mkw_zstd.rs`, `mkw_view.rs`, `mkw_romfs.rs`. os `Cargo.toml` gets `zstd-sys` (experimental
bindings) and `zstd`. `movie.rs` gets `playing()`; `mods.rs` gets `add_overlay` (a game fix's RomFS layer).

| mkw-recomp entry | What it does | nxrs equivalent | Status |
|---|---|---|---|
| 97 `spin` lines (main 81, subsdk0 2, subsdk1 10, sdk 4) | back edge of a spin wait: a scheduling checkpoint | recomp.txt `spin` -> translator `rt_spin_gate` | Ported, built |
| `lan` set, 1 site (main+0xbdfc2c) | title menu opens with LAN Play chosen | `NXRS_PATCH_MKW_LAN` (default 0) | Ported (no LAN service yet: off) |
| `zstd` set, 3 sites (main+0xe1cf4, 0xf0c34, 0xdec5c) | ZSTD_decompressMultiFrame and the loads' stream on host libzstd (`ov_zstd.c`) | `NXRS_PATCH_MKW_ZSTD` (1), `gamefix/mkw_zstd.rs` (page 0xff0 / 0xfe0), `NXRS_MKW_ZSTD_CHECK=1` | Ported, runs (`K-mkw3`: 184 MB of stream in 158 ms, 0 guest fallbacks) |
| `output` set, 2 sites (main+0xdce850/4) | GraphicsSystem's output size from the patch page | `NXRS_PATCH_MKW_OUTPUT` (0) + `NXRS_MKW_RESOLUTION` / `NXRS_MKW_ASPECT`, page 0xfd0 (committed only then) | Ported, not run |
| `camera` set, 2 sites (main+0xcdc344, 0xcdc34c) | camera service not opened; its event a real one that never fires; stand-in service (0x7c4 slot 0xd8, else 0x5c4) at main+0x378ecb8 | `NXRS_PATCH_MKW_CAMERA` (1), page 0xf50 | Ported, runs ("no camera: the camera service is a stand-in") |
| `aspect` set, 8 sites | Hor+ cameras (0xf80), 2D projection (0xf70, 0xf40), vfx2 screen effects (0xf20), pane masks / prompts (0xdf0) | `NXRS_PATCH_MKW_ASPECT` (0) + `NXRS_MKW_ASPECT=21:9`, `gamefix/mkw_view.rs` | Ported, not run (`window` shape not supported: logged) |
| `vertplus` set, 1 site (main+0x894a4) | race FOV for a taller output (0xf30) | `NXRS_PATCH_MKW_VERTPLUS` (0) | Ported, not run |
| `display` set, 3 sites | display size fitted to the output's shape (0xf10, 0xf00) | `NXRS_PATCH_MKW_DISPLAY` (0) | Ported, not run |
| `shadow` set, 4 sites | render-buffer allocator's second pool at the first shadow map; `MKW_SHADOW_SIZE` | `NXRS_PATCH_MKW_SHADOW` (1, was `NXRS_PATCH_MKW_RBPOOL`), `NXRS_MKW_SHADOW_SIZE`, `NXRS_MKW_RB_EXTRA_MIB`; mkw-recomp's timing and size rule (0xb600000 at the console's lines) | Ported, runs (`NXRS_MKW_POOL_MODE=early` keeps X2's experiment) |
| `seed` set, 18 sites | fixed random seeds (`seed.c`, 0xfb0) | `NXRS_PATCH_MKW_SEED` (0) + `NXRS_MKW_SEED` | Ported, not run (the clock / GetInfo 11 parts of seed.c: not ported, they live in time / svc) |
| game.c frame update (vtable main+0x367aac0, 0xfa0) + fps_auto.c | `MKW_FPS` auto / 60 / 30 / 20 / 15, game speed log | `NXRS_MKW_FPS` (unset = the game's own choice, as mkw-recomp's auto at full speed) | Ported, not run |
| race_scene.c (14 node vtables, 0xe00..) | in-a-race flag for 20 / 15 fps | installed with `NXRS_MKW_FPS` auto / 20 / 15 | Ported, not run |
| `tools/romfs/resolution.py`, `romfs_res.c` | For3D / Screen lines (and LODDistanceRatio) in general_format's GamePerformanceGraphicsConfigGParam | `NXRS_MKW_RESOLUTION=<lines>`, `NXRS_MKW_DETAIL_DISTANCE`, `gamefix/mkw_romfs.rs` -> `<data>\cache\resolution-<L>\` layered by `mods::add_overlay` | Ported, compiles; not run yet |

**SDK answers diffed (mkw-recomp's 321 replaced SDK names against nxrs's os / graphics):** 245 have a named nxrs
counterpart; the other 76 (hid Npad / six-axis / vibration, touch, VP9 decoder internals, nv allocators, a few
applet / os observers) are answered below the SDK by nxrs's services, not by name. Differences found:
- **GetInfo CoreMask 0x7 vs 0x3f** (the NPDM's): fixed above. This was the crash.
- **oe::GetCurrentGraphicsScale**: nxrs 0 (nxnative: the whole GPU), mkw-recomp 10 (its own placeholder). 10 did not
  change the crash (`K-mkw4-scale10`); left at 0.
- **nvnSyncWait on a sync never fenced**: nxrs ALREADY_SIGNALED, mkw-recomp TIMEOUT_EXPIRED. For lane G4 (not changed).
- **nvnDeviceGetInteger** (`build\K\devinfo_diff.py`): equal to mkw-recomp's 1.8.0 table but item 0x6 (table 16 vs
  32; nxrs prefers the sdk switch's own constant, so only if that switch has none). For G4 to confirm.
- oe performance mode (1 docked), operation mode, focus (1, messages after a window change), display version, time,
  account, fs synchronous: same.

## Infrastructure, tests, launcher (lane Q, 2026-11-20)

**Profiles.** `src/profiles` now holds nxnative 7ebb76e's 207 profiles (20 new, `shared/source.txt`); nothing in
the 187 existing folders changed. Re-import: `py -I src/profiles/tools/export.py <nxnative>` and
`uv run --no-project --python 3.14 src/profiles/tools/export_shaders.py <nxnative>` (a sparse clone of upstream is at
`build\Q\nxnative-up`), then `--check`.

**PORT_MAP.tsv** (repository root): one row per file of nxnative (`NX`), Turbo (`TB`) and the mkw-recomp donor files
(`MR`): lines, destination crate/module, action, status, note. `scripts/port_map.py` writes it from ordered rules
(edit a rule, not the TSV) and `--check` compares with the trees (defaults: `build\Q\nxnative-up`, else
`D:\Projects\nxnative`; `D:\Projects\Switch2RECOMP-main`; `D:\Projects\mkw-recomp-main`; `--format-only` in CI);
`cargo test -p profiles --test port_map` checks the format. **Every lane: when a row's work lands, change its rule's
status in `scripts/port_map.py` and regenerate.**

**CI** (`.github/workflows/ci.yml`, not pushed by lane Q): `cargo build --workspace --locked`, `cargo test --workspace
--locked --lib --bins --tests` (doc tests are not run: graphics' docs have a non-compiling block), the PORT_MAP format
check; a gnullvm job (`-p front -p runtime`, llvm-mingw 20261104); the oracle on request. **Test rules for every lane:**
a test that needs a GPU goes behind graphics' `gpu-tests` feature; one that needs a game, keys or a dump is `#[ignore]`
or returns early unless an environment variable names its data; a test never hangs (CI has no GPU, no games).
**Lane O3:** two uncommitted os tests break that today: `services::time::tests::shared_page_layout_as_turbo` fails and
`services::net::tests::bsd_event_descriptors` hangs (killed after minutes); fix or gate them before committing.

**Oracle** (`src/oracle`, binary `nx-oracle`; not a workspace member). Build: Visual Studio's environment, CMake and
Ninja, `BOOST_ROOT` = Boost headers (1.86 at `build\Q\boost\boost_1_86_0`); lane Q's script `build\Q\oracle-build.bat`.
Run: `nx-oracle [--cases 2000] [--states 8] [--class a,b] [--out DIR] [--mutate]` (llvm-mingw from
`NXRS_LLVM_MINGW` or `build\tools\llvm-mingw`). Each class: random words of its encoding group that `Lifter::lift`
lowers and dynarmic runs, `Lifter`'s raw C compiled into a DLL with the game flags, run from random register / vector /
flag / memory states on both sides at the same addresses (data window at 0x10_0000_0000, code at 0x20_0000_0000),
compared: X0-X30, SP, next PC, NZCV, V0-V31, TPIDR_EL0, memory. Output `<out>\mismatches.tsv`. **Lane T / anyone
touching `src/translator/src/xlate`: run it** (11 s, 0 mismatches expected). Found and fixed: STGP lowered as STP W.
By design: `RET Xn` (n != 30) is a return to the caller; PACGA's value. Not modelled: FPSR cumulative flags (soft
count only); FPCR other than 0. The lifter lowers many unallocated encodings dynarmic rejects (harmless: never in
compiled code).

**Launcher (D:\Projects\RecompLauncher, uncommitted).** `doctor` shown in Settings > Engine (`nxrsDoctorArguments`,
`parseNxrsDoctor`: keep the `ok   ` / `FAIL ` line prefixes and exit code 0 / 1 of `nxrs-build doctor` stable);
`nxrs --nxrs-doctor`. Save copy between Turbo and nxrs builds (Turbo `save\user` <-> nxrs `save\user1`, `device`,
`cache` <-> `cache0`; backup `save-backup-<time>`), `nxrs --select-title <g> --copy-saves`. `.xci` sources (nxrs only).
release.yml: engine bundle behind `nxrs_engine` / `NXRS_ENGINE_IN_RELEASE` (off).

- **Audit rows (lane O3, later the same night):** focus (`oe::{set_focus, applet_enter, applet_leave, focus}`;
  FocusStateChanged queued at boot as Turbo; **a window behind others is now OutOfFocus for the game**, so
  background scripted runs may want `NXRS_KEEP_FOCUS=1`), touch (`input::{set_touch, touch_now}`, control `touch`
  / `drag`), control `addr` / `memdump` / `env` / `speed` / `focus`, F3 / F7 from graphics' shot and frame sink, F5
  thread states, DLC with missing data not offered, Turbo `save\user` adopted as user 1, keys under
  `%LOCALAPPDATA%\nxrs\keys` first. SetMemoryAttribute records any bits (Turbo); only SetGpuMemoryAttribute refuses
  bits other than 0x20 (the DKB 0xE801 abort at 0.9 s: DKB and Wonder start clean, `*-O3-dkb-svc`, `*-O3-wonder-svc`).
- **Jamboree (for the coordinator / lane G4):** with the boot FocusStateChanged and `NXRS_KEEP_FOCUS=1`
  (`build/I/runs/20261120-030909-O3-jamboree-focus`, current tree incl. G4's uncommitted graphics), the game ran
  60 FPS through the L+R / A steps, then at 103 s (entering the hub) presents stopped: frame 4731 took 2.5 s and the
  main thread sits in `nvnQueueFinish` (thread dump in the run's actions.jsonl). Whether the focus message changed the
  fade is not known yet: the follow-up run was closed from outside at 19 s (the owner is at the PC and closes game
  windows), so lane O3 stopped running games. Next: one run of the same steps with `NXGFX_ASYNC=0` / the last
  committed graphics, to tell the GPU hang from the fade.

**Where MKW stands now (lane K, 2026-11-20 03:20).** No crash. `K-mkw5-cores6`: the course-map loading screen
(picture right, 2560x1440 from the game's own capture, `0001-shot_boot.png`). `K-mkw6-race` (scenario `mkw`, 230 s)
and `K-mkw7-sync` (`NXGFX_ASYNC=0`): presents stop at 31-40 s (frames 168-216, the next scene after the map) and
never resume; every guest thread waits, the main thread sits in **`nvnSyncWait` called from the frame update
(main+0x69d038, frames main+0x69cd74 / 0xe5bfd4 / 0x69a6a8)**; the same with async and sync replay; no fault, no GPU
error line, no "wait expired" red (so its waits are short and repeated). The race is not reached; FPS and the
10-minute run are not measured yet. **For lane G4 (most likely theirs):** a queue parked on a `WAIT_EVENT` whose
event is never released (the retry runs only on `nvnEventSignal` and on submissions: an event the guest writes with
a plain store, or one a parked queue's own later record signals, parks for good), so the fence the frame update
waits for never completes. A log line when a queue parks / is still parked at the os stall dump would settle it.
Earlier (`K-mkw5`) the window losing focus paused the game (FocusHandlingMode 2, as on a console): scripted MKW runs
need `NXRS_KEEP_FOCUS=1`.

**Lane G4 for lane K (2026-11-20):** G4's parked-queue poll and never-fenced SyncWait change were taken out of the tree
again (lane K's AcquireTexture sync fix in `nvn/window.rs` covers the stall). Kept from G4: **device item 0x6** answers
32 on the Switch 2 (mkw-recomp's 1.8.0 table), and the write-watch grouping per allocation joins a gap only while the
pools cover most of the span (G4's one MKW run, `build/I/runs/20261120-032722-G4-mkw-park`, overlapped a lane K run and
was killed at ~56 s; its pool sync was up to 47 ms in slow frames before that bound). The mkw-recomp 1.8.0 rendering
fixes (#423 / #485 / #441 / #291 / #468 / #22 / #395 / #384) are not checked by G4: file the ones a K run shows.

**MKW's GPU resets (lane G4, 2026-11-20 04:10): found by the new device-lost report.** Lane K's run
`build/I/runs/20261120-040222-K-mkw13-demote` (exit code 71, G4's `vk/lost.rs`) logged: the GPU started and had not
finished checkpoint #7576, **a mesh draw with task shader fea1d6ca04512de1, mesh shader ddc034e8e009ebfc, pixel shader
3cc3501255bb2d74**; the driver's fault report: **invalid read at 0xca19c6000** (instruction pointer 0x200b68340). The
task shader reads two storage buffers (s0 / s1 from its c0 table) and the mesh-draw records: a read past a bound range
faults the GPU when robust buffer access is off, which it was. **Fix (G4): robust buffer access is on by default on
NVIDIA** (Turbo and Ryujinx do the same; `NXGFX_ROBUST=0` off). Still to find: why the address is outside the bound
range (a storage buffer whose guest address is in no pool gets the zero buffer, 64 KB; or the task shader's record
index). Not run by G4 (K owns MKW runs). K's `20261120-041317-K-mkw14-robust` (a build from before the default changed: its log has no `if the GPU stops responding` line) lost the device the same way: checkpoint #5256, the same three shaders, invalid read at 0xcbf689000. If a build with robust buffer access still resets there, the read is not through a bounded descriptor (a pointer the task shader computes): then the task shader's SPIR-V is the next look (G4). **K's `-K-mkw15-robust-sets` (04:24):** another mesh program this time (task 6098504f5d12ea03, mesh 1de24686294997d8, pixel 1e3e40ea043238db), checkpoints #124-#190 in flight (all mesh draws of that program), the fault an **invalid read at 0x0**. A read at address 0 is not a bounded descriptor read: a descriptor whose address is 0 or a pointer the shader built. G4's next step (needs a run, K's or G4's): `NXGFX_CHECKPOINTS=all` and the pass dump of the frame before, to see which resource of that program is unbound; meanwhile `NXGFX_MESH=0` (mesh draws off) tells whether MKW runs on without the resets.

**Not ported (and why):** mkw-recomp's button-icon sheet redraw (#19, its own UI textures), Discord presence, its
launcher / setup / settings menu, LAN sockets (no LAN layer in nxrs; the `lan` set is there, off). The 1.8.0
rendering fixes of its CHANGELOG are graphics / shader work, filed for **lane G4**: #423 / #485 push constants reach
every stage and bank c24 (the upscale pass and SMAA; 1.8.0's docked picture was cropped and blurred without it), #441
compute writes of the water height map reaching guest memory at the fence, #291 pitch textures of any size written
back at a fence wait, #468 released textures overwritten by a later image dropped, #22 constant-bank reads through
a storage view, #395 integer abs written as `x * ((x >> 31) | 1)`, photo mode's compute push (#384). Not checked
against nxrs's graphics in this lane.

**Lane O3, continued (2026-11-20, after a2b663b):**
- **MATCH cuts** (`translator::aot::nso_match`, cli `profile::match_sites`): a build now applies nxnative's
  function-matched cuts (set `match`, `NXRS_PATCH_MATCH=0` off; `address space` rules left out). Arceus's
  capture-demo crash fix and Ocarina's movie cuts match the owner's builds; **they take effect at the games' next
  build** (the patch table changes the translation key, so that build translates again).
- **Prompts**: `os::ui` (state machines + a topmost prompt window over the game). The controller applet now waits for
  players to join when it wants more than it sees: **scripted runs that might meet a controller applet should set
  `NXOS_CTRL_PROMPT=0`**. Home held 2 s asks to quit (`NXOS_HOME_QUIT=0` off). `input::set_prompt` blocks the game's
  pads while a prompt is open.
- **Fonts**: shared font slots from `<data>\fonts\<Name>.bfttf|.ttf`, else a system TTF.
- **Folder input**: flat folders and folders without a RomFS build (translator `package::folder`).
- **Metrics**: per-thread guest CPU (`cpu_t<handle>_ms`, `guest_cpu_ms`), `cpu: busiest guest threads` lines;
  runtime `profile::thread_cpu()` (the profiler's thread list is kept always now).
- **Gestures**: GetGestureStates answers from the touch screen (`services/gesture.rs`).
- **Control pipe**: `wpage`, `wtrace`, `wclear` (write watches, `os/src/wwatch.rs`).
- **Jamboree (current tree, `build/I/runs/20261120-032440-O3-jamboree-focus3`, NXRS_KEEP_FOCUS=1)**: with the boot
  focus message the game still shows the white fade over the hub at 60 FPS (no hang this time). So the focus
  message is not what the fade waits on. The fade stays a game-state question (lane W / X): next candidates are the
  account / friends / online-play answers around the hub (`friends::TryPopFriendInvitationNotificationInfo` and
  `nifm::IsNetworkAvailable` are the only stand-ins on the path).
- **Violet new save (`build/I/runs/20261120-032706-O3-violet-new3`)**: past name entry and the opening to the
  bedroom ("Get Up" with the left stick) at about 7 min; Mom not seen yet. Keep that run's new save next time.
- **Not done** (lane O3): FRAME_STEP / STUBS, observation gates and the Trinity fence; the engine frame lock;
  the cheats engine; `bp`; the user-select dialog; shader facts read by the shaders crate (G4's crate; graphics'
  prewarm reader exists in `vk/pipe.rs prewarm_profile`); AudioIn's real-device half (needs a microphone).

**Lane O3 (2026-11-20, after 44ae629): PC-app settings and the frame lock.**
- New ini options (config `options.rs`, written with comments, read live): `pause_on_focus_loss` (0: the game is
  always in focus; 1: told it lost the focus when its window is in the background or a host prompt is open),
  `controller_prompt` (1), `home_quit` (1), `keyboard_box` (1). Lane Q: the launcher toggles. `NXRS_KEEP_FOCUS=1`,
  `NXOS_CTRL_PROMPT=0`, `NXOS_HOME_QUIT=0`, `NXOS_SWKBD=auto` stay the harness overrides.
- Engine frame lock: `os/src/fpslock.rs` (`fpslock::tick` at every present; `NXOS_FPS_TARGET` overrides the
  profile's `fps_target`).
- Jamboree: the two stand-ins on its hub path answer as Turbo / nxnative do: `nifm::IsNetworkAvailable` false (Turbo's
  IRequest state 1 "free" and GetResult 110-300: no network), `friends::TryPopFriendInvitationNotificationInfo`
  0 = none (nxnative's own stub; Turbo's friend service has no invitation either). Neither explains the fade.

**Lane K 2026-11-20 (after the 03:51 crash): MKW's stall and GPU hang, graphics side (small changes in src/graphics,
lane K's; G4 please review).**
- **The 31-40 s stall was the window acquire sync** (`nvn/window.rs`): nxrs fenced it with every queue's work so far;
  MKW's frame update waits on it with an infinite timeout (main+0x69d034, the sync at [[frame+0x150]+0x78]) while
  queues 2 and 19 hand work to each other through GPU events (0x2264c7100 / 0x140; a temporary park log showed it,
  removed), so a frame took ~13 s. The texture is the game's once its last present is done (`present_wait`, just
  before): the sync is satisfied at once (fenced 2, fence 0), as mkw-recomp's (`K-mkw9-acq`, `K-mkw10-acq`: frames
  kept coming, the title scene drew, 1,082-1,763 draws a frame).
- **Then the device was lost ~40-45 s in** (`K-mkw9`, `K-mkw10`, `K-mkw12`: these are the TDRs of 03:28 / 03:30 /
  03:51; `K-mkw6` / `K-mkw7` at 03:05 / 03:13 too). mkw-recomp's CHANGELOG names three NVIDIA "instruction pointer
  invalid" device losses on 1.8.0's title screen, all shader shapes: (1) integer abs (#395): 1,451 of MKW's 11,609
  translated shaders use GLSL.std.450 SAbs; (2) fragment shaders that discard midway (#22 / #231): 2,186 use OpKill;
  (3) constant-bank reads indexed by a vertex attribute (#22). `vk/spvfix.rs` now has `sabs_product` (SAbs as
  `x * ((x >> 31) | 1)`, `NXGFX_SABS_FIX=0` off) and `kill_demote` (OpKill as OpDemoteToHelperInvocation, then the
  selection's merge block or a return; the capability and SPV_EXT_demote_to_helper_invocation added; only with the
  device's shaderDemoteToHelperInvocation, `NXGFX_DEMOTE_FIX=0` off), both called in `vk/shader.rs` build next to
  the ballot fix. Unit-tested, and all 3,637 MKW modules they change pass spirv-val (vulkan1.3; opt-in test
  `spvfix_folder`). `K-mkw12` had the SAbs rewrite alone and still lost the device; the demote rewrite is the next
  run. (3) is not done: it needs the translator's bank reads (G4 / shaders).
- **For G4's device-lost diagnostics:** MKW's loss comes ~40 s in, on the title scene after the course map; the last
  draw per queue there is what I need. I run MKW one exclusive run at a time and stop at the first GPU RESET.
- **`K-mkw13-demote` (SAbs + demote rewrites; 04:03 GPU RESET): G4's device-lost report names the work.** At 36 s:
  "the GPU started #7576 a mesh draw with shaders [1:3cc3501255bb2d74 3:fea1d6ca04512de1 4:ddc034e8e009ebfc]",
  everything before it finished; driver fault report: instruction pointer fault, **invalid read at 0xca19c6000**.
  So it is a memory read out of range in a mesh draw, not a compiler shape. The task shader (fea1d6ca04512de1) reads
  two storage buffers bound by its constant bank (`cp_s0`, `cp_s1`, unbounded runtime arrays) and the mesh-draw
  parameters (`mesh_draw`); it uses no raw pointers. The mesh shader reads a constant bank and the payload only.
  Suspects: a storage buffer whose address resolves to no pool gets the 64 KiB zero buffer (`exec.rs` `Res::Sbuf`
  `None` arm) and the shader indexes past it, or a bound range shorter than what the shader reads. Diagnosis switch
  `NXGFX_ROBUST=1` (`vk/device.rs`, default off: core robustBufferAccess) for the next run. The SAbs / demote rewrites
  stay (valid, and mkw-recomp's fixes), but were not this fault.
- **`K-mkw14-robust` (`NXGFX_ROBUST=1`; 04:13 GPU RESET):** the same mesh draw (#5256, same three shaders) faulted
  at 20 s, invalid read at 0xcbf689000, and no storage buffer fell back to the zero buffer (new red line in `exec.rs`
  `Res::Sbuf`, never printed). Inconclusive: with descriptor buffers nxrs writes descriptors at
  `storage_buffer_descriptor_size`, not the robust sizes robustBufferAccess needs (`exec.rs` 1822), so robustness may
  not have applied. What the three shaders read, from their SPIR-V: the task shader (fea1d6ca04512de1) storage
  buffers `cp_s0` / `cp_s1` and `mesh_draw[mesh_push[0] * DrawIndex + 1]`, constant banks, its payload (`uint[352]`,
  both loops bounded); it emits `min(shared[357], 65535)` mesh groups. The mesh shader (ddc034e8e009ebfc) one constant
  bank, the payload, shared memory, outputs bounded (32 vertices, at most 16 primitives); the fragment shader
  (3cc3501255bb2d74) two constant banks only. No raw pointers, no textures. Next: `K-mkw15` with
  `NXGFX_DESCBUF=0 NXGFX_ROBUST=1` (classic sets, where robustness applies).
- **`K-mkw15-robust-sets` (`NXGFX_DESCBUF=0 NXGFX_ROBUST=1`; GPU RESET at 18.5 s):** another mesh draw (#190, shaders
  [1:1e3e40ea043238db 3:6098504f5d12ea03 4:1de24686294997d8]) faulted, invalid read at **0x0**. Every MKW device loss
  so far is in a mesh draw (task + mesh shaders), at a different draw each run. Mesh draws bind nxrs's own draw records
  (`exec.rs` `mesh_draw`: `qs.mesh_rec`, read by the task shader at `mesh_draw[mesh_push[0] * DrawIndex + 1]`) and,
  for indirect draws, a staged {x, 1, 1} argument copy; which read faults is not known yet. Next: `NXGFX_MESH=0`
  (mesh draws skipped) to confirm the hang is confined to the mesh path.
- **Not run yet (lane K, 04:35):** `K-mkw16-nomesh` (`NXGFX_MESH=0`: mesh draws skipped, to confirm the hang is
  confined to the mesh path and let the menus / race go on meanwhile) did not start: G4's exclusive DKB run took the
  machine first. Command, for whoever runs MKW next: `py scripts/play_scenario.py mkw-race --exe
  build\games\mkw\mkw.exe --label K-mkw16-nomesh --option lsfg=0 --env NXRS_KEYS=12:A*400/1.5 --env NXGFX_MESH=0`
  (with `NXRS_LANE=K NXRS_ESC_QUIT=0 NXRS_CLOSE_QUIT=0 NXRS_KEEP_FOCUS=1 NXRS_NO_ACTIVATE=1`). Lane K's graphics
  changes, all small: `nvn/window.rs` (acquire sync satisfied at once: the stall fix), `vk/spvfix.rs` + `vk/shader.rs`
  (`sabs_product`, `kill_demote`, their switches `NXGFX_SABS_FIX` / `NXGFX_DEMOTE_FIX`), `vk/device.rs`
  (`NXGFX_ROBUST`, diagnosis, off), `vk/exec.rs` (a red line when a storage buffer falls back to the zero buffer).
- **MKW mesh path against mkw-recomp (lane K, 04:40, no run needed):**
  - mkw-recomp draws task/mesh natively (`VK_EXT_mesh_shader`) on NVIDIA by default since 01/11/2026 ("40 s of race
    without a hang"); compute passes only on AMD / RADV (`vk_mesh_cs.c`). It records **only direct**
    `nvnCommandBufferDrawMeshTasks(first, count)` -> `vkCmdDrawMeshTasksEXT(count, 1, 1)`, **ignores `first`** (logged
    when non-zero, "drawn from group 0"), and **skips every indirect, 3D and multi mesh draw** (`nvn2.c`
    `mesh_skipped`). nxrs draws all of them and passes `first` through its own `mesh_draw` record (task shader:
    `shared[0] = mesh_draw[mesh_push[0] * DrawIndex + 1] + workgroup`), a lane S design mkw-recomp does not have.
  - Checked and consistent in nxrs: NVN2 stage 6 / 7 -> host 3 / 4 for binds (`cmd.rs` `st_host`); task / mesh take
    the tessellation stages' descriptor sets with TASK_EXT / MESH_EXT visibility (`layout.rs`); push constants reach
    both stages (stride at byte 12, the task shader reads it there); `support_buffer` and every bank / storage buffer
    are remapped into the module's own stage set (`shader.rs` `remap`); the storage tables in c0 (task 0x670, mesh
    0x770) match the reflections (task `cp_s0` / `cp_s1` at words 412 / 416 = 0x670 / 0x680, mesh 476 / 480 =
    0x770 / 0x780); payload and output indices bounded; group counts within NVIDIA's limits.
  - So the two differences left against the reference: the shader translation itself (Ryujinx via dmsh_xlate against
    mkw-recomp's sm86 translator) and the `first` / indirect handling. Next test: mkw-recomp's rule (draw direct mesh
    draws from group 0, skip indirect / 3D / multi), behind a switch, to see whether the device loss goes.
- **`K-mkw16-nomesh` (`NXGFX_MESH=0`, 383 s, no GPU reset): the hang is confined to the mesh path, and MKW reaches a
  race.** With A every 1.5 s (mkw-recomp's press-race cadence) it went through the title, menus and course select
  into a Grand Prix race: `0016-shot_race.png` shows the race HUD (5 coins, lap 1/3, 24th, the minimap with the
  racers), the 3D scene nearly all black (the mesh draws carry much of it). The title scene before it is overexposed
  with a black rectangle at the top left (`0003`). 8-11 FPS throughout. The run ended on the memory watchdog at 20.0 GB
  private (graphics allocator 12.2 GB in blocks, 38,789 textures = 7.3 GB; guest heap 3.1 GB): memory grows without
  the mesh draws too.
- **`K-mkw17-meshref` (`NXGFX_MESH_REF=1`: mkw-recomp's mesh rule, direct mesh draws from group 0, indirect / 3D
  skipped; GPU RESET at 05:00):** no mesh draw faulted (65 s, where every earlier run's mesh fault came at 18-45 s), so
  nxrs's indirect / 3D mesh draws or its `first` record are the likely mesh-fault cause. Instead, at 65 s:
  "the GPU started #93513 a dispatch of 2x2x6 groups ... 5:881a4d611e825d6c", **invalid write at 0x1000**. That
  compute shader (256 threads) writes six storage images (handles 72..84, format Unknown, type 259; one per cube
  face: a cube-map / mip pass): a storage-image descriptor whose image is not bound or already released. The
  mesh-off run (`K-mkw16`) did not hit it in 383 s.
- **Next (lane K):** (1) narrow the mesh fault between "indirect / 3D draws" and "`first` through the record" (two
  switches on `NXGFX_MESH_REF`); (2) the cube-map compute write: log the storage images bound to 881a4d611e825d6c (handle,
  registered texture, view size) when it is dispatched; mkw-recomp's #468 (a released texture a later image was written
  over) is the reference to diff against; (3) the 20 GB memory growth (`K-mkw16`, watchdog at 383 s).
- **`K-mkw18-skip` (`NXGFX_MESH_REF=skip`: indirect / 3D mesh draws skipped, `first` kept; `NXGFX_CHECKPOINTS=all`;
  `NXGFX_LOG_SHADER=881a4d611e825d6c`):** no device loss in 68 s (the mesh faults came at 18-45 s with everything
  drawn); stopped by the memory watchdog (20.9 GB at 68 s). New switch modes: `NXGFX_MESH_REF=skip` / `first0` / `1`.
  The cube pass 881a4d611e825d6c binds valid images: a 128x128 cube (t8, ser37540) and a 128x128x12 cube array (t9),
  one storage view per level 0..6, every view made. It is a mip chain: each dispatch runs 2x2x6 groups of 256 for
  every level down to 2x2, so most stores fall outside the smaller levels. Hardware ignores such stores; Vulkan
  leaves them undefined without `robustImageAccess`, which nxrs did not enable (mkw-recomp enables every feature the
  driver offers). **Fix (`vk/device.rs`, small, lane K):** `robustImageAccess` on with G4's buffer robustness (same
  `NXGFX_ROBUST` switch, default on for NVIDIA). Verifying in `K-mkw19`.
- **G4 for K2 (2026-11-20 06:45): lazy images in the tree** (`image.rs reg_tex` / `materialize`, `NXGFX_LAZY_IMAGES=0`
  off): a texture registered for sampling gets its image at its first use (a draw's handle, DrawTexture), not at
  RegisterTexture; a shader that indexes the texture pool itself makes all waiting ones first. MKW's shaders take
  every handle from c2 (one from a uniform bank), none index the pool. The memory line now ends "N registered without
  one yet". **One G4 MKW run queued** to measure it (`build/G4/chain13.sh`: `mkw-race` 330 s, `NXGFX_MESH_REF=skip`,
  `build/G4/mkwt`, label `G4-mkw-lazy`) after G4's Smash run; it waits on the machine lock like any run. Replaced
  pipelines are now destroyed too (`pipe.rs retire`).
  **Measured (`-G4-mkw-lazy`, title demo, 185 s):** private memory **10.6 GB at 49 s, 12.1 GB at 102 s, 12.0 GB at
  153 s** (`K-mkw16` at 93 s: 16.7 GB, then 18.5 at 162 s); textures **2,850-3,065 images = 3.0-3.3 GB** (was 36,958 =
  6.6 GB) with ~36,000 registrations never drawn; allocator 6.3 GB in blocks (was 10.6), sparse tiles still 1.7 GB.
  The title's pictures look right (`0008-shot_race.png`). The run then lost the device at 185.8 s (07:11 reset) in an
  **indirect dispatch** (compute `532f4107bf53157e`, indirect at 0x2a8108004): *invalid write at 0x300001000*, the same
  0x3 / 0x4_0000_xxxx write fault as Smash's (lane G4 round 4): being chased with the address binding report.
- **Memory (for G4, numbers from `K-mkw16` / `K-mkw18`):** private memory jumps by 6-8 GB within 10 s when the title
  scene first loads, to 16-17 GB, then grows ~0.6 GB a minute to the 20 GB watchdog (mkw-recomp: 10.6 GB private in a
  race on Windows). At 93 s: graphics allocator 10.6 GB in blocks (textures 36,958 = 6.6 GB, sparse tiles 1.7 GB,
  scratch 0.5 GB, pools 0.3 GB), guest heap 3.1 GB, physical maps 1.0 GB. The pipeline prewarm alone added 2.4 GB for
  334 pipelines (~7 MB each: NVIDIA keeps compiled pipelines in process memory), and pipelines made during play add
  the same way. Not a released-texture build-up like mkw-recomp's #468: nxrs drops parked images after 8 frames.
- **`K-mkw19-skip-robimg` (`NXGFX_MESH_REF=skip`, robustImageAccess on, `NXGFX_PREWARM_BUDGET_MB=512`; 05:27):**
  **no device loss in 373+ s.** So (1) the mesh faults come from nxrs's indirect / 3D mesh draws (skipped here,
  `first` kept and drawn), not from `first`; (2) the cube-map pass ran without faulting with image robustness on.
  New stop: presents end at 71 s (frame 683) and never resume. The main thread waits on an address (0x15b54b4d4, in
  worker 0x1016's job record) from the frame update (main+0x69c9ac -> 0x45e5dc / 0x45ebf0); every other worker waits
  on its own job word; worker 0x1016 runs guest code (in main+0x17bf530, a float-math function using pi, called from
  main+0x17bdc34 / 0x17bb13c / 0x17bc0d4 / 0x17b5270 / 0x17b4f30 / 0x17d6028 / 0x15081c / 0x147d2c / 0x465aa4) and
  never finishes its job. One dump only, so whether it loops there is not known yet. Not seen in `K-mkw16` (mesh
  draws off, 383 s).
- **The 71 s stall, lane K (05:45, `K-mkw20-dumps`, same settings as `K-mkw19`):** reproduced at the same place
  (presents stop ~70 s; main waits on a job word from the frame update). The busy worker sits under the same caller
  in both runs: `K-mkw19` worker 0x1016 in main+0x17bf530, `K-mkw20` worker 0x1012 in main+0x17c0c00, both returning to
  main+0x17bdd78. That caller is a spawn loop (main+0x17bdb80..0x17bdcbc, an LCG-random spawn function main+0x17bdd00
  per item, particles by its look): it runs `w20 = (w10 - fcvtzs(s8 * ucvtf(s0) * 0.01 * ucvtf(w10))) * w20` times.
  The lifted FCVTZS saturates as ARM does (`simd_fp.h` `toint`: NaN 0, out of range INT_MAX / INT_MIN), so a C-cast
  wrap is not it. MKW sets FPCR.FZ (bit 24) at start (main+0xca8518, 0x185069c). Not settled yet: whether the loop
  never ends or runs very long (the repeated pipe dumps kept only their first line; `build\K\dumps.py` now keeps
  them whole); then the oracle with FPCR = 0x1000000 over the loop's float instructions (`ucvtf s0, s0`, `fmul`,
  `fcvtzs`, the spawn function's) needs the oracle's word-list / FPCR options, which it does not have yet.
- **Descriptor-buffer robust sizes (lane K, `vk/device.rs` `desc_size`, `vk/exec.rs`, `vk/tables.rs`):** with
  robustBufferAccess on (G4's default), buffer and texel-buffer descriptors are now written at the robust sizes, as
  VK_EXT_descriptor_buffer requires; the plain sizes were used before.


**Engine bundle (lane Q, 2026-11-20; Linux as Eglantine, owner).** `scripts/package_engine.py --platform linux|windows
--target <release dir> [--gnu <dir>] --xlate <dmsh_xlate> --spirv-val <spirv-val> [--out dist]` writes
`nxrs-engine-<platform>-x64.tar.gz|zip` + `.sha256`: `nxrs-engine/` = nxrs-build(.exe), engine.json (contract, engine,
version, platform; on Linux `requires`: the system packages), the engine libraries (Linux `libnxfront.a` /
`libnxrt.a`; Windows `nxfront.lib` / `nxrt.lib` + `gnu/`), `tools/dmsh_xlate(.exe)`, `tools/spirv-val(.exe)`,
`LICENSES/`. The launcher's package-release.py makes the same `nxrs-engine/` folder. **shaders (lane G4, a lookup only):**
`Tools::find` now also takes `spirv-val` from `tools/` beside nxrs-build (as it already did `dmsh_xlate`), so a bundle
needs no Vulkan SDK. ci.yml `linux` job: release build, `dmsh_xlate` built from nxnative e4b0d9b (`NXNATIVE_XLATE`;
checked out with `core.autocrlf true`, which gives the Windows tool's `--source-sha` e11dc43d4ddc5b58: an LF checkout
gives fe842711268600f0, a different shader-cache identity for byte-identical SPIR-V), packaged, unpacked elsewhere and
checked (`contract`, `doctor`, `dmsh_xlate --source-sha`), uploaded as the artifact `nxrs-engine-linux-x64`. x86-64 only.
Verified in WSL (Ubuntu 24.04): engine release build 2 m 52 s, dmsh_xlate built the CI way in 23 s (e11dc43d4ddc5b58),
bundle packaged (sha256 7f0399c9…), unpacked: `nxrs-contract 2`, doctor every check ok but the display (none in WSL),
and the launcher's `stage_nxrs(..., 'linux')` + `check_nxrs` on those real files (6 files, executable bits kept).
**Not verified:** a game build with the unpacked bundle (Beach Bounce) — WSL's own disk went read-only, then the
distribution stopped starting ("I/O error", `Wsl/Service/CreateInstance/E_FAIL`) before it began; the owner's WSL needs
a look (lane Q did not try to repair it).

## MKW (lane K2, M7, 2026-11-20)

Lane K2 takes over from lane K. Engine `target\K2`; game relinked by `build\K2\relink.sh mkw` (target\K2's
`nxfront.lib`); one start per test through `build\K2\run.sh <label> <seconds> [--env ...]` (play_scenario `mkw-race`,
A every 1.5 s, `NXRS_LANE=K2`, saves backed up to `build\K2\mkw-save-backup-*` first, `build\K2\stallwatch.py`
alongside). Runs `build\I\runs\*-K2-mkw*`.

**The "70 s particle-loop stall" was a lazy write-back deadlock, not a float bug.**
- `K2-mkw4` / `K2-mkw5` reproduced it (~64 s, worker 0x1015 / 0x1012 under main+0x17bdd78). New dump switch
  (runtime `kernel.rs` `dump_threads`): `NXRS_DUMP_FRAMES=<ret>[,<ret>]` prints the 16 words at the frame record of a
  frame returning there. At main+0x17bdc34 (the spawn function's frame): loop index w23 = 0, count w20 = 4, the same
  in two dumps 7 s apart. So the spawn loop never looped: one call never came back. Lane K's count formula reading was
  right but the count is 4.
- The worker's host stack (cdb, non-invasive, `build\K2\stallwatch.py`: `~*kc` of every thread when no frame arrives
  for 10 s; `cdb-*.txt` in the run folder) showed it: guest store in f_70017bdd00 -> access violation -> graphics
  `vk::lazy::veh` -> `bring_back(p)` -> its `memmove` into the page faulted again -> `veh` -> `bring_back(p)` -> waits
  in a yield loop for `returning` to drop p, which only the outer call on the same thread can do. The thread waited
  for itself for ever; main waited for its job.
- **Fix (graphics `vk/lazy.rs`, small):** a thread never waits for a page it is bringing back itself (a thread-local
  marks it; a second fault on it goes on to the other handlers with a red line), and `protect` now says whether
  VirtualProtect worked: a page that can no longer be made accessible (its memory changed while it was held on the
  device) is dropped with a red line instead of copied into. **For G4:** the red lines `lazy write-back: page ...`
  will name the case when it happens again; `K2-mkw6` passed the stall point (course flyover at 63.8 s) without
  either line, so the exact cause of the second fault is not caught yet.
- **The oracle checked the float side anyway** (`src/oracle`: new `--words HEX,..` (exactly these words, no other
  class) and `--fpcr HEX` (every state's FPCR, the host MXCSR set from it as the runtime does: FTZ|DAZ = FPCR.FZ);
  build `build\K2\oracle-build.bat` into `target\K2o`). The loop's 11 float words and all 1,765 FP / SIMD words of
  main+0x17ac000..0x17c4000 (the spawn code and its callees: fmul, fmla, fmadd, ucvtf #32, fcvtzs, frecpe / frsqrte,
  fcsel, ...): 0 mismatches against dynarmic at FPCR 0 and at 0x1000000 (706,000 cases each). mkw-recomp's FZ handling
  is the same as nxrs's (cpu.h `rt_fpcr_sync` identical: FTZ|DAZ = FPCR.FZ on MSR FPCR, thread / fiber entry,
  host->guest calls). Tool: `build\K2\nsotext.py <nso> <out>` (the NSO's decompressed .text, for llvm-objdump).

**28-34 s present gaps: the write watch walked its range once per 4,096 written pages (graphics `vk/mem.rs`
`watch::take_one`, small).** `K2-mkw1` / `K2-mkw2`: frames of 9-34 s (presents stop, main in nvnSyncWait, every
worker idle); cdb: the replay thread inside `mem::begin_submit` -> `GetWriteWatch`. A full 4,096-page buffer asked
again from the range's start, so a pool with N written pages cost N / 4,096 walks of the whole range. It now asks
again from the page after the last one returned. After it: no frame over 5 s in `K2-mkw4` / `-5` / `-6` (was 8.9 /
11.3 / 27.6 s in `K2-mkw2`). A `write watch: ... took N ms` line logs any call over 100 ms (none since).

**Set cache and mesh draws (graphics `vk/exec.rs`, small).** G4's per-stage set cache (48b009b) keyed a task shader's
set without its draw records (`Res::MeshDraw`, each mesh draw stages its own `{count, first}`): a later mesh draw of
the same program reused the earlier draw's `first`. `SetKey` now has the mesh records. Indirect mesh draws copy the
game's records into staging with the counts (an aligned binding of the values the counts came from) instead of
binding the game's memory. `NXGFX_MESH_REF=skipind` / `skip3d` (only indirect / only 3D skipped) added for the next
narrowing; the mesh checkpoint names the draw (direct / indirect, 1D / 3D, counts, first).

**Mesh faults are not only indirect / 3D.** With `NXGFX_MESH_REF=skip` (indirect / 3D skipped) the GPU still faulted
twice in direct draws of the same program as `K-mkw13`: `K2-mkw3` at 21 s (#7129, invalid read 0xce3266000) and
`K2-mkw6` at 72.7 s (#184544, "direct, 1D 6x1x1, first 0", invalid read 0xe0145e000), shaders [1:3cc3501255bb2d74
3:fea1d6ca04512de1 4:ddc034e8e009ebfc], robust buffer + image access on, no buffer destroyed in the report. Lane K's
`K-mkw19` / `-20` (373 + 150 s, no fault) ran before the set cache and the robust descriptor sizes (a7ebd79 / 48b009b).
`K2-mkw7-nocache` (`NXGFX_SET_CACHE=0`, skip): GPU reset at 14.6 s in a direct mesh draw of another, mesh-only
program (#2715, "direct, 1D 1x1x1, first 0", shaders [1:3cafb02cdf0d87fb 4:11ed01f0618a65a3], invalid read at
0x6cad56000). So neither the set cache nor the task shader nor `first` is the cause: direct mesh draws of several
programs read an unmapped page. The checkpoint covers the whole draw, the pixel stage included; `K2-mkw3`'s report
listed ~48 textures handed to be destroyed 2 s before its fault. Next: the mesh / pixel shaders' texture and storage
bindings at the faulting draw (`NXGFX_CHECKPOINTS=all` + `NXGFX_LOG_SHADER=<key>`), and whether a texture destroyed
in that window is still in the bindless table (mkw-recomp #468 is the reference). Three GPU resets in this lane's
session (06:24, 06:58, 07:22), all in mesh draws.

**Picture.** Since the 06:45 build the course-select map, the menus and the course flyover draw lit and right
(`K2-mkw6` `0002`, `0003`: Mario Bros. Circuit at 63.8 s), where `K2-mkw2` / `-5` (06:06 / 06:41) still had the black
world map and black-silhouette characters on an overexposed backdrop. Not isolated: that span holds G4's conditional
rendering change (48b009b, "MK8D black world") and lane K2's lazy / write-watch / set-key changes; the conditional
rendering change is the likely one. Still wrong: menu text misses glyphs ("L s roo C" for Mushroom Cup, course names
cut), and the title sky was not seen again.

**FPS: 8-20 in menus and the flyover** (`K2-mkw6` pacing 10-20). The replay's `prep / pools` is 50-60 ms a frame
(the per-submission write-watch pool sync over ~3,500 mirror pools, 3.3 GB): the largest single cost, for G4.

**G4's review of K2's e2496aa (10:00):** `mem.rs release` bringing a pool's lazily held pages back before its buffer
goes: right, and the likely explanation of "never bound" faults (G4's Smash runs now carry it). It runs on the game's
thread inside nvnMemoryPoolFinalize, so a finalize may now wait for a page readback: fine, and only when pages are
held. GPU texture write-back (`tex_writeback` / `tex_writeback_land`): right in shape (copied at the submission's
end after the pass, landed at completion only if the texture is still the one written and its storage untouched).
Two notes: the copy is the image's bytes, so it is only right where the upload is a straight copy (true for every
format it lets through: depth, ASTC, BC7 and expanded 3-component formats are excluded); and landing drops silently
when the image table is busy, so a busy frame keeps the old behaviour (fine, said here so it is not mistaken for a
miss). The 262,144-entry checkpoint ring is ~8 MB even at the default level (each slot a String): fine.

**G4's review of K2's changes (07:30):** `lazy.rs` (no self-wait on the page a thread brings back; `protect` returns
whether it worked, a page that cannot be made accessible is dropped): right. `mem.rs` write watch resuming after the
last returned page when the buffer fills (with `reset` the returned pages are not returned again): right, and the
peek path still stops at a full buffer as before. `exec.rs` mesh record in `SetKey`: right (each mesh draw stages its
own {count, first}; a reused set read the earlier draw's). G4 also sees the same *invalid write at 0x3 / 0x4_0000_xxxx*
class in Smash (a full-screen draw into a live 1920x1080 image) and in G4's MKW run (an indirect dispatch, 185.8 s):
VK_EXT_device_address_binding_report now names the faulting address's object (`NXGFX_ADDR_REPORT=1`, on with
`NXGFX_CHECKPOINTS=all`), which may help the mesh-draw faults too. Pool sync: G4 is on it (`NXGFX_PROFILE=1` logs the
write-watch calls and their time every 600 submissions).

**Lane K2, round 2 (07:25-08:10): the device losses.**
- **Destroyed textures are not it.** `NXGFX_DESTROY_DELAY=<presents>` (graphics `vk/image.rs`, diagnosis) holds a
  released image's and its views' destruction that many presents past its last use. `K2-mkw8-delay-cold` (64
  presents): the same direct mesh draw faulted at 14.4 s (#2270, shaders [1:3cc3501255bb2d74 3:fea1d6ca04512de1
  4:ddc034e8e009ebfc], invalid read 0x37b756000). That draw's three shaders sample no texture at all (task: storage
  buffers c0 table s0 / s1, banks c0 / c3 / c4, the draw records; mesh: c3 + support; pixel: c3 + support). mkw-recomp
  #468 is the opposite case (released copies kept too long: a leak); nxrs drops parked images after 8 frames.
- **The translated mesh shaders read primitive indices from shared memory that is never cleared.** Ryujinx's mesh
  shaders (via dmsh_xlate) declare 32 vertices / 16 primitives, set the primitive count from shared word 580 and take
  each primitive's three indices from shared memory, 8 bits each (0..255); the task shader emits `min(shared[357],
  65535)` mesh groups. Vulkan shared memory starts undefined; mkw-recomp's translator zeroes its index array before a
  mesh shader runs (head.py `mib[k] = 0`) and bounds every payload / index store. Two passes (graphics
  `vk/spvfix.rs`, called in `vk/shader.rs`; 19 mesh + 7 task modules of MKW rewritten, all pass spirv-val vulkan1.3,
  unit-tested): `mesh_indices` ANDs every stored primitive index with OutputVertices - 1 (`NXGFX_MESH_INDEX_FIX=0`
  off); `zero_workgroup` gives task / mesh shaders' Workgroup variables a null initializer, with the device's
  shaderZeroInitializeWorkgroupMemory now enabled (`vk/device.rs`; `NXGFX_ZERO_SHARED=0` off). `K2-mkw9-meshfix`
  (skip mode, `NXGFX_CHECKPOINTS=all NXGFX_ADDR_REPORT=1`, destruction delayed): no mesh-draw fault in 55 s
  (24,577 mesh draws; the two runs before it faulted in mesh draws at 14.4 / 14.6 s). Then a device loss **outside
  the mesh path**: "in the pass #632904 a pass 128x128 into [c0 ser28824 at 0x26b7b3000 ... 128x128x5
  R16G16B16A16_SFLOAT]", invalid read at 0xe93d56000, "in no memory bound or unbound since the start" (G4's address
  report; its unbound ring keeps 8,192, so an old unbind may have left it). Every MKW fault address so far
  (0x37b756000, 0x6cad56000, 0xca19c6000, 0xcbf689000, 0xce3266000, 0xe0145e000, 0xe93d56000) is a page in the range of
  the game's own GPU addresses (its pools sit at 0x1_xxxx_xxxx-0x2_xxxx_xxxx and up): worth checking whether a guest
  GPU address reaches the device as a device address somewhere (only c9a56f796ea4a4cd / ce770ef0597876ca use the raw
  global-memory table, and none of the faulting programs). One fault in 55 s is not proof the mesh faults are gone.
- **Menu text (offline):** two candidates fixed or instrumented, neither proven yet: (1) a buffer -> texture copy whose
  source lies in a cold pool now takes the CPU's writes to that range first (`mem::sync_range`, called from exec's
  COPY_B2T; `NXGFX_COPY_SYNC=0` off; the first 4 logged); (2) a page the CPU wrote since its pool was last asked is no
  longer held on the device by the lazy write-back (`apply_writebacks`, a write-watch peek; `NXGFX_LAZY_CPU_CHECK=0`
  off): bringing such a page back put the buffer's copy over the CPU's bytes. Diagnosis line: `CPU write kept out of
  GPU-written texture` (a CPU write under half of a texture the GPU wrote is not uploaded: the rule in `image.rs`); not
  seen in `K2-mkw9` before its fault at 55 s.
- **Guest GPU addresses -> device addresses, every path read (08:10):** vertex / index / indirect / uniform / storage
  binds, texel buffers, copies, mesh records and the clamp kernel all go through `mem::resolve` (a pool range ->
  (VkBuffer, offset), the pool's own buffer) or a staging buffer of the submission; descriptor-buffer addresses come
  from the buffer's own device address (`addr_of` / `buffer_address`, the zero buffer when none); query / report
  targets and conditional rendering are read and written by the CPU at completion; only c9a56f796ea4a4cd /
  ce770ef0597876ca use the raw global-memory table (delta per pool). No path hands a guest address to the device as
  is. **What can hold a dead device reference: the texture pool's slots.** A slot keeps the descriptor of the image
  it was registered with after the game finalizes that texture (on the console that descriptor still names memory
  that is there); nxrs parks the image 8 frames, then destroys it and its views, and the slot's descriptor then names
  freed memory (with descriptor buffers, a VA that is no longer mapped). A shader that indexes the pool (bindless) or a
  draw whose plain binding reads that slot then reads unmapped memory. `NXGFX_DESTROY_DELAY=64` only moved that
  later. **Fix (graphics `vk/image.rs` `scrub_slots`, `vk/exec.rs` `dummy_view_ready`):** when a parked or evicted
  image (or an alias's views) goes, every pool slot naming its texture first gets the zero stand-in's view (the
  console would show the old bytes; nxrs shows zeros, no fault). `NXGFX_SCRUB_SLOTS=0` off. Not run yet.
- **For G4 (Smash):** if the slot scrub holds for MKW, Smash's faults (an invalid write into a live 1920x1080 target)
  may share the cause through a **storage-image** slot: `write_image` writes the pool's B_IMG descriptor too, so a
  finalized texture's storage slot keeps naming its image's view after the image goes; a compute pass or a shader
  storing through that slot then writes freed (or, once reused, someone else's live) memory. `scrub_slots` rewrites
  both the sampled and the storage descriptor with the stand-in's views; a Smash run with `NXGFX_SCRUB_SLOTS=0` /
  unset against the same scenario would tell.
- Runs `K2-mkw7` (07:22), `K2-mkw8` (07:32), `K2-mkw9` (08:03) each reset the display driver; next MKW run waits for
  G4's Smash address-report run.

**`K2-mkw10-scrub` (09:18, the one run: `NXGFX_MESH_REF=skip NXGFX_CHECKPOINTS=all NXGFX_ADDR_REPORT=1`, slot scrub,
mesh shared-memory fixes, lazy CPU-write check; 240 s to its hard timeout): no GPU fault, no stall.** The first MKW
run since the set cache (48b009b) to go its whole length without a device loss: 139,265 direct mesh draws, past the
55 s point where `K2-mkw9` lost the 128x128x5 pass, through the course map (63 s) into the race (85 s on). The scrub
fired ("texture pool: 1 slot(s) of an image that goes now name the zero stand-in", from 11.9 s). One clean run: the
pass fault and the mesh faults are gone as far as it shows; not yet run with indirect / 3D mesh draws on.
- **Picture:** the course map lit and right (`0003`); in the race (`0004`-`0011`) the 3D view is flat sky blue with a
  black-silhouette character portrait and a faint HUD: with indirect / 3D mesh draws skipped most of the course is not
  drawn, so the next run is `NXGFX_MESH_REF` unset (all mesh draws), the step that serves the bar.
- **Menu text still drops letters** ("zlario ros Cir it" for Mario Bros. Circuit): the cold-pool copy sync never fired
  (no buffer -> texture copy from a cold pool) and the lazy CPU-write check did not change it. Not the glyphs: the
  `CPU write kept out of GPU-written texture` lines named five RGBA16F textures of 11x1516 / 13x1211 at 0x2434b0000..
  0x2437ad000 (4-8 KB of 151-189 KB written by the CPU, not uploaded: the half-covered rule in `image.rs`). Those are
  CPU-updated tables the GPU wrote once; their rows stay stale (a candidate for the black portraits). mkw-recomp writes
  a GPU-written texture back to its storage at the fence and merges later CPU writes (`vk_tex_alias.c`
  `gpu_written_locked`); nxrs has no texture write-back, so the fix is either that or uploading the CPU-written rows
  only. The glyph path itself is not identified yet.
- 8 "GPU address ... is in no memory pool" lines at 9 s (1 MB reads at 0x13ba32000.. / 0x2a9204fd0, skipped binds).
- FPS: 13-19 in menus, 8 in the race (prep / pool sync, G4).

**`K2-mkw11-meshall` (09:46, all mesh draws on, `NXGFX_TEX_WB=0`, both diagnostics): GPU reset at 105 s in the race.**
Through the menus and course map with indirect / 3D mesh draws drawn, no mesh fault; then "invalid read at 0xdfc3b6000
... inside BUFFER 0x176e98e0550 (bound at 71.10 s: 0xdfc250000 +0x800000), UNBOUND at 71.56 s": a GPU read 34 s after
an 8 MB buffer was freed; the checkpoint was "no longer kept" (the ring held 4,096; with `all` the GPU runs more than
that behind the replay in a race: now 262,144, `vk/lost.rs` `N`). **Cause found by reading (lane K2, graphics
`vk/mem.rs` `release`):** pages a pool's buffer held on the device (lazy write-back) stayed in the lazy table when the
pool went (nvnMemoryPoolFinalize, or a new pool over the same base): they named the freed buffer, and the next touch of
those pages (the CPU's, or a new pool's buffer over the same memory, `ensure_buffer` brings its range back first) had
the lazy reader copy from it on the GPU. The reader's submission has no checkpoints, which is why the report named only
the last draw's. **Fix:** `release` brings the pool's held pages back (`lazy::touch`) while its buffer still exists.
The same freed-buffer read may be `K2-mkw9`'s "never bound" address (its unbind ring keeps 8,192). Not run yet.
Picture with all mesh draws on (`K2-mkw11` `0004`, 85 s, the race's opening flyover behind a wipe): the course terrain
is drawn and lit (sand, grass, hills, sky) where the skip-mode runs showed flat sky; the road and many patches of
ground are black (a material or lighting input still missing).

**For G4 (texture code, lane K2's change, contained): GPU-written textures go back to their storage.**
`image.rs` `tex_writeback` / `tex_writeback_land`, `queue.rs` `Sub::tex_wb`, `exec.rs` `target_image`, `mem.rs`
`cpu_written`. A texture written as a render target or copy destination (`target_image`) is listed for its
submission; at the end of that submission small ones (single level and layer, uncompressed 2D, at most 512 KB, image
the storage's size, scale 1, at most 64 a submission) are copied to a host buffer, and at completion written into
their guest storage through the swizzle (`write_storage`), the storage's write watch reset, `gpu_owned` cleared and the
page hashes dropped. A later CPU write to part of the texture then uploads the whole texture from storage that holds
both (mkw-recomp's `gpu_written_locked` / `wb_at_fence`). Dropped (old behaviour kept) when a later GPU write to the
texture came (`gpu_seq`), the CPU wrote the storage meanwhile (write-watch peek), a page is held on the device (lazy),
or the image table is busy (`try_lock` on the completion thread: never waits). `NXGFX_TEX_WB=0` off. Not run yet.

**Left (lane K2; bar per plan 5B 2afe332: a race start with the right picture, no GPU fault, at the FPS target):**
the 128x128x5 pass fault (slot scrub to verify) and whether direct mesh draws are clean; then all mesh draws on
(`NXGFX_MESH_REF` unset); menu text; FPS (pool sync, G4); the frame-rate / resolution hooks where they serve the FPS
target. No 10-minute race, no side-by-side play.

## Linux (lane L, M10, 2026-11-20)

The whole engine builds and runs on x86-64 Linux: `nxrs-build` (translator, shaders, clang + ld.lld), the runtime and
`libnxfront.a` a game links. Every Linux piece sits behind `cfg(target_os = "linux")`; the Windows code paths are
unchanged (wrapped in `cfg(windows)`, nothing restructured). The windows / windows-sys crates are now
`[target.'cfg(windows)'.dependencies]`; Linux adds `libc`, `zbus` (os, BlueZ), `winit` + `raw-window-handle` (front),
`cc` + `pkg-config` (os build).

**Windows piece -> Linux equivalent** (file: what replaced it):

| Windows | Linux | Where |
|---|---|---|
| VirtualAlloc reservations at fixed addresses, commit / decommit, VirtualProtect, VirtualQuery | `mmap` PROT_NONE + MAP_NORESERVE + MAP_FIXED_NOREPLACE; commit = `mprotect` RW, decommit = `MADV_DONTNEED` + PROT_NONE; a byte-per-page state map over the 39-bit space and a reservation table stand for VirtualQuery (`query`, `committed_runs`, `allocation_of`, `writable`); `readable` / `read_nofault` through `process_vm_readv` (ReadProcessMemory) | runtime `linux.rs`, `mem.rs`; graphics `vk/mem.rs` (`committed`, `guest_bytes`), `nvn/vpool.rs` |
| GetWriteWatch / ResetWriteWatch (MEM_WRITE_WATCH) | userfaultfd in asynchronous write-protect mode (UFFD_FEATURE_WP_ASYNC, Linux 6.7) over the watched reservations + PAGEMAP_SCAN (mkw-recomp's `linux_mem.c` design): kernel writes (a `read()` into guest memory) are seen too; no watch -> `None`, callers compare snapshots as before | runtime `linux.rs` `written_pages` / `watched`; graphics `vk/mem.rs` `watch` |
| Vectored exception handlers (crash reporter, heap first touch, lazy write-back pages, debug write watches) | one SIGSEGV / SIGBUS / SIGILL / SIGFPE / SIGTRAP handler (SA_ONSTACK, alt stacks) with a hook list run newest first (`nxrt::linux::add_fault_hook`, AddVectoredExceptionHandler(1, ..)'s order); unclaimed faults chain to the previous handler (Rust's stack-overflow report) or the default action; `Fault` gives rip / rsp / the write bit (REG_ERR) / the trap flag | runtime `linux.rs`, `fault.rs`; os `mem.rs`, `wwatch.rs` (read-only page + single step instead of PAGE_GUARD); graphics `vk/lazy.rs` |
| Crash report stack walk (RtlVirtualUnwind) | glibc `backtrace()` through the signal frame (.eh_frame), modules by `dladdr` | runtime `fault.rs` |
| Windows x64 ABI assembly boundary (rt_dispatch, rt_sj_owner, nxrt_call_guest, rt_svc, host entry; SEH unwind info) | the same shims for System V (rdi / rsi / rdx / rcx, same frame layout so `SjSlot` is one type, CFI instead of SEH) | runtime `asm.rs`, `boundary.rs` (test asm) |
| Fibers (ConvertThreadToFiberEx / CreateFiberEx / SwitchToFiber / DeleteFiber) | the same four calls over `nxrt_ctx_switch` (asm: callee-saved registers, MXCSR, x87 control word) and mmap'd stacks with a guard page | runtime `fiber.rs` (`host` module), `asm.rs` |
| SuspendThread + GetThreadContext (profiler samples), GetThreadTimes | SIGPROF to the thread, the pc copied from its signal context; `pthread_getcpuclockid` | runtime `linux.rs`, `profile.rs` |
| QueryPerformanceCounter | CLOCK_MONOTONIC | runtime `rt.rs` |
| Job object memory cap | a cgroup v2 of the process's own with `memory.max` (where the user's tree delegates the memory controller), else a watchdog thread on resident + swapped memory that stops the game with the reason in the log | runtime `linux.rs` `memory_cap`, `process.rs` |
| SetProcessAffinityMask (NXOS_HOST_CPUS) | `sched_setaffinity` on every thread | os `boot.rs` |
| SetThreadPriority (shader threads), CreateWaitableTimerEx (frame pacing) | per-thread nice (`setpriority`), `clock_nanosleep` TIMER_ABSTIME | graphics `vk/pipe.rs`, `vk/present.rs` |
| GlobalMemoryStatusEx, GetProcessMemoryInfo, GetProcessTimes | `sysconf`, `/proc/self/status` (VmRSS + VmSwap), CLOCK_PROCESS_CPUTIME_ID | graphics `vk/mod.rs`, `vk/perf.rs`, `vk/pipe.rs`; front `metrics.rs` |
| Named pipe control channel `\\.\pipe\nxrs_ctl_<pid>` (+ `swr_ctl_<pid>`) | Unix domain socket `$XDG_RUNTIME_DIR/nxrs_ctl_<pid>` (+ `swr_ctl_<pid>`; /tmp without XDG_RUNTIME_DIR), same one-line protocol, removed at exit | front `ctl.rs` |
| WASAPI output and capture (cpal) | cpal's ALSA backend (PulseAudio / PipeWire through ALSA's default device); a capture device counts only when it opens (ALSA always names "default") | os `media/sink.rs`, `media/mic.rs` |
| MMCSS | none needed (no equivalent call; real-time scheduling would need privileges): not ported | - |
| XInput pads | SDL2 game controllers loaded at run time (libSDL2-2.0.so.0), as Turbo's Linux backend; SDL's own PS4 / PS5 / Switch HID drivers off so the native HID path keeps those pads | os `input/xinput.rs` |
| WinMM joysticks (fallback) | SDL2 joysticks | os `input/winmm.rs` |
| Raw HID (SetupAPI + HidD_*: DualSense / DS4, Switch Pro, Joy-Con, NSO pads) | hidraw: `/sys/class/hidraw` (uevent HID_ID / HID_NAME, bus 5 = Bluetooth), usages and report lengths from the report descriptor (report-id byte counted as HidP_GetCaps does), non-blocking read, 30 ms bounded write, HIDIOCGFEATURE | os `input/hidio.rs` |
| Switch 2 controllers over Bluetooth LE (WinRT) | BlueZ over D-Bus (zbus): discovery, Nintendo manufacturer data, connect, GATT by UUID, AcquireNotify / AcquireWrite with StartNotify / WriteValue fallbacks | os `input/switch2_ble.rs` |
| Win32 game window (+ raw mouse, ClipCursor, borderless fullscreen) | winit window on its own thread (X11 or native Wayland): F1-F11 / Esc / Alt+Enter, keys by position as virtual-key codes, raw mouse motion, cursor grab, borderless fullscreen, close / focus handling, NXRS_NO_ACTIVATE | front `window_linux.rs` |
| Vulkan Win32 surface | VK_KHR_xlib_surface / xcb_surface / wayland_surface from raw-window-handle (`nxgfx::set_window_raw`, `set_window_unix`) | graphics `vk/present.rs`, `vk/device.rs` (instance extensions, filtered by what the loader has), `host_window.rs` |
| Media Foundation H.264 and AAC decoders | FFmpeg libavcodec through a small C shim compiled against the system headers (`media/ffdec.c`, `media/ff.rs`, same `mf::{H264, Aac}` API); libopus / libvpx / libdav1d from the system (pkg-config) instead of the MinGW archives | os `media/`, `build.rs` |
| Media Foundation sink writer (F7 recording) | the `ffmpeg` program: BGRA frames on its stdin (libx264, else libopenh264 / NVENC / VA-API), the sound as 48 kHz s16 beside it, muxed with AAC when the recording stops | front `rec_linux.rs` |
| PrintWindow capture (control `shot`, `shot=`) | the presented picture read back by graphics at the next present (Wayland lets no program read another window) | front `capture.rs` |
| MessageBox, the swkbd text box, the prompt window, F1 page (GDI) | the desktop's dialogs (zenity, else kdialog) as Turbo's `plat_message`: message box, `--entry` text box, `--question` prompt (closed by PID when the pads answer), `--forms` F1 page with the four options; ShellExecute -> `xdg-open` | os `desk.rs`, `services/notice.rs`, `services/swkbd.rs`, `ui.rs`; front `help.rs` |
| Lossless Scaling app launch / registry Steam path | Steam's Linux library folders for Lossless.dll (in-engine LSFG); the Windows app itself is not started (it captures Windows windows only) | front `lsfg.rs` |
| GetTimeZoneInformation, GetLocalTime | `localtime_r` (`tm_gmtoff`) | os `services/time.rs`; front `capture.rs` |
| GetSystemMetrics (screen size) | the window system's primary monitor through winit (`services::vi::HOST_SCREEN`) | os `services/vi.rs`, front `window_linux.rs` |
| TerminateProcess at quit | `_exit` | front `lib.rs` |
| GUI-subsystem log redirect | stderr at /dev/null or closed -> `<name>.log` beside the game | front `lib.rs` |
| ExecuteProgram (`<name>_p<N>.exe`) | the same `std::process` launch (fork / exec), no extension | os `services/oe.rs` |
| seek_read / seek_write | `pread` / `pwrite` | os `services/fs.rs` (`posio`) |
| Keys under %LOCALAPPDATA% | `$XDG_DATA_HOME` (~/.local/share)`/nxrs/keys/prod.keys`, the launcher's layout, then `~/.switch/prod.keys`; SWR_PROD_KEYS first as before | translator `package/keys.rs` |
| clang for x86_64-pc-windows-msvc / llvm-mingw, `<name>.exe` + `<name>_code.dll` | clang for x86_64-unknown-linux-gnu + ld.lld: `<name>` (loader: dlopen of `<name>_code.so`, NXRS_DATA / NXRT_EXEFS, a 16 MB main stack by setrlimit + re-exec) and `<name>_code.so` (-fPIC objects, `-Bsymbolic` so the generated code's runtime calls bind without a PLT, `--no-undefined`; system libraries: `-lopus -lvpx -ldav1d -lavcodec -lavutil -lasound -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc`); `nxrs_game_main` exported with default visibility; the mods' disabled markers read through /proc/self/exe | cli `cc.rs`, `pipeline.rs`, `mods.rs` |
| dmsh_xlate.exe, spirv-val.exe (shader build tools) | `build/tools/dmsh_xlate-linux/dmsh_xlate` (NativeAOT linux-x64, `src/shaders/tools/build_dmsh_xlate.sh`: same source, same `--source-sha` e11dc43d4ddc5b58 as the Windows tool), `spirv-val` from PATH (spirv-tools) | shaders `xlate.rs` |
| nxrt-dev (homebrew e2e) toolchain | PATH clang / ld.lld, `libnxrt.a` | runtime `bin/nxrt_dev.rs` |

**Using it.** Build the engine with `cargo build --release -p cli -p front -p runtime` (needs clang, lld, pkg-config,
libasound2-dev, libopus-dev, libvpx-dev, libdav1d-dev, libavcodec-dev, libavutil-dev, a glslangValidator); put
`libnxfront.a` / `libnxrt.a` beside `nxrs-build` (the release folder has them) and run `nxrs-build <package> -o <name>
--outdir <dir>`. `nxrs-build doctor` checks the Linux pieces (clang, ld.lld, the engine libraries, libvulkan, ALSA, the
codec / FFmpeg libraries, a display, SDL2, zenity / kdialog, ffmpeg for F7, Vulkan devices, disk, keys). `--target
linux` is accepted on Linux, refused on Windows (and `--target windows` the other way). A game needs at run time:
libvulkan + a Vulkan 1.3 driver, ALSA, libopus, libvpx, libdav1d, libavcodec, libavutil; optional SDL2 (pads), zenity or
kdialog (dialogs), ffmpeg (F7); hidraw access for native HID pads (the usual udev rules, e.g. Steam's).

**Verified (WSL 2, Ubuntu 24.04, kernel 6.18, 2026-11-20).** WSL here has no GPU driver for Vulkan (Mesa without
dzn; no WSLg session), so the only Vulkan device is **llvmpipe** (Mesa's CPU rasterizer), and the game window ran on an
X server of its own (Xvfb :91). What that proves and what it does not is listed apart.
- `cargo build --workspace` and `cargo test --workspace --lib --bins --tests`: **349 passed**, 0 failed (runtime 30 incl.
  the write watch, fault hooks, thread samples and the longjmp boundary; os 114 incl. 27 input tests and the FFmpeg /
  opus / vpx / dav1d decoders bit-exact; shaders 65; translator 65). Windows: `cargo check --workspace --tests` and the
  touched crates' tests green after every change.
- Homebrew e2e (`nxrt-dev e2e`: guest NROs translated and linked against `libnxrt.a`): **8/8** (hello, threads, atomics,
  TLS, fiber, setjmp, C++ exceptions, float printf); each re-run 25 times: **200/200**.
- `nxrs-build doctor`: every check green but the display (none in a plain WSL shell) and the keys (none under ~).
- **Beach Bounce Remastered** (01004060145C0000) built from the dump in place (SWR_PROD_KEYS, read-only): 73,066
  functions, 397 MB of C, compile 179 s, link 0.8 s, total 189 s; a rebuild relinks in 3.5 s (every object reused).
  `<name>` 9 KB loader + `<name>_code.so` 294 MB. Run 60 s (`~/nxrs-games/runs/bbounce-r1`): window 1440x810 (Xlib),
  first present at 2.0 s, **3,343 frames in 60 s** on the title menu (picture right), the 16 shaders Unity makes at
  run time translated live (as on Windows), audio: no output device (WSL has no sound server) so the clock fallback ran.
  Run 75 s (`-r2`): control socket `/tmp/nxrs_ctl_<pid>`: `status`, `pads` ("no controllers"), six `press A~0.2` took
  the game from the title into the story (screenshot through `shot`). Run 45 s (`-r3`): `record start` / `stop`: an MP4
  with H.264 1920x1080 60 FPS (libx264) and AAC 48 kHz stereo, 9.0 s.
- **Anime Girls: Sun of a Beach** (0100C6801E0FA000) built (1,471 s: shaders 744 s, compile 708 s): **53,109 shaders
  found, translated by the Linux dmsh_xlate and all 53,109 valid** under spirv-val, as the Windows build; the same
  106,218 files, and every 10th SPIR-V module (5,311) **byte-identical** to the Windows build's (the json sidecars
  differ only by the tool binary's hash). Run 60 s with `NXGFX_PROFILE_PREWARM=0`: the 3D title screen right, ~1.5 FPS
  (llvmpipe renders on the CPU).
- **For lane G4 (llvmpipe only, not seen on NVIDIA):** with the first-run profile prewarm on, Anime Girls dies at
  ~1.4 s inside llvmpipe's `vkCreateGraphicsPipelines` (libvulkan_lvp.so+0xfcef5, read of 0x8) for a pipeline-library
  part from `vk/pipe.rs` `gpl_part` <- `shader_parts` (the fragment-shader part, pipe.rs:580) <- `prebuild`. Either a
  Mesa GPL bug or a create info NVIDIA tolerates (worth a run with the validation layer on a Linux GPU); the in-flight
  part guard did not stop the next session from crashing on another part. `NXRT_FOREIGN_FAULTS=1` logged it.
  - **G4 (round 4): likely an invalid create info of ours.** pipe.rs:580 is the *pre-rasterization* part. A program
    made ahead (`prebuild` / first-run profile prewarm) whose vertex shader is not translated yet kept only its pixel
    shader, and its pre-rasterization library was created with **no shader stage** (invalid; NVIDIA tolerated it).
    `shader_parts` now makes no parts without a vertex / mesh stage (the program is made whole at its draw). Not run on
    llvmpipe here.

**Not verified (no hardware for it in WSL):** a real GPU driver (NVIDIA / AMD / Intel Vulkan on Linux; the GPU-heavy
paths ran only on llvmpipe), real sound (ALSA / PulseAudio / PipeWire output and capture), real controllers (hidraw
pads, SDL2 pads, BlueZ Switch 2 pads: built and unit-tested; SDL2 loads, BlueZ answers "not running"), a native
Wayland session (only X11 through Xvfb ran), the cgroup memory cap (WSL refuses the cgroup; the watchdog path ran),
the F1 / swkbd / prompt dialogs on a desktop (zenity present, not driven), and gameplay at a frame-rate goal. The M10
exit criterion (M5 on Linux: MK8D at 60) needs a Linux PC with a GPU.

**Left:** run on a Linux PC with a GPU (MK8D at 60, then the library), sound and controllers there; a Linux release
package (the launcher's Linux CI builds Turbo games; shipping `nxrs-build` + `libnxfront.a` + `libnxrt.a` + the Linux
`dmsh_xlate` with it is the launcher's part); MMCSS has no counterpart (not ported).

## Lane J (interpreter, execution hints, NRO capture, pads, prompts; 2026-11-20)

**Interpreter** (`src/runtime/src/interp`, `nxrt::interp`). Code the build did not translate runs instruction by
instruction on the same `Cpu`, guest memory, SVC entry (`svc::nxrt_svc_impl`) and host-call boundary as lifted code.
- *When:* `dispatch::nxrt_dispatch_resolve` finds no lifted function, host function or live setjmp owner at an aligned
  address inside a module (`interp::take`); misaligned and out-of-module addresses stay fatal. `NXRS_INTERP_ONLY=1`
  interprets every dispatch of a normal build; a build without functions (`nxrs-build --interp-only`, hidden, v1 and
  v2) does so by itself (`Table::build` accepts modules with no functions then, and says so in the log).
- *One meaning with the translator:* `exec.rs` follows `xlate/lifter.rs`'s decode (its bit helpers are that crate's
  `xlate/util.rs`, included by `#[path]` as `interp::xutil`), `simd.rs` follows `xlate/simd.rs`; their semantics are
  the C's (`runtime/simd.h`, `simd_fp.h`, on the exact path; `fp.rs`, `lanes.rs`; the estimate / CRC / AES tables are
  read from simd.h). An encoding the lifter refuses ends the run (`FATAL: interp: <word> at <pc> ...`). **Anyone
  changing `xlate/lifter.rs`, `xlate/simd.rs` or `runtime/simd*.h`: change the interpreter with it and run
  `nx-oracle --interp`** (lane J's build: `build\J\oracle-build.bat`, target `target\Jo`; 0 mismatches expected, also with
  `--fpcr 1000000`, `2000000`, `400000`, `800000`, `c00000`).
- *Control flow:* BL / BLR call through `asm::nxrt_call_guest` (a barrier), RET returns to whoever entered the
  interpreter (the lifted code's host-stack model; a context restore's RET is a jump, as `aot::emit` lowers it), a jump
  to a lifted start, a host function or out of the modules leaves through `rt_dispatch` (asm `nxrt_interp_entry` status
  1, no Rust frame left behind). Each entry runs under `rt_sj_owner` (asm `nxrt_interp_owner`); `sjlj::jump_target`
  asks `interp::owns` for such slots: never for a call (`interp::is_call`: the BL / BLR before x30 names the target,
  or x30 is `RET_HOST`), else the frame's own call return sites (setjmp) or an address in the same function
  (`interp::funcs`, the module's `.eh_frame_hdr` through MOD0) as its active call (landing pads). Register branches in
  interpreted code ask the same search, so a longjmp / unwinder resume from interpreted code reaches the frame below.
- *Stats:* `interp::stats()` = (entries, instructions); the first 32 entries are logged (`NXRT_TRACE=interp`: all).

**Execution hints** (`discovered.tsv`). `interp::set_data_dir` (os `boot`, runtime `process::run`) names
`<data>\discovered.tsv`; every interpreted entry is appended once: `module \t 0xoffset \t w0 w1 w2 w3` (the first four
words as a version stamp). `nxrs-build` reads it (`translator::aot::hints`, `GenOptions::hints`): an entry becomes a
function start when its module still has those words there, else it is left out ("another version of the program");
the generation key includes the hints. **Launcher (lane Q):** a run that wrote entries can say "N new functions
found; rebuild for full speed" (count the non-`#` lines; the build itself is incremental).

**NRO capture.** os `services::ro`: a LoadModule with no built copy keeps the image as `<data>\nro\<sha256>.nro`
(`ro::capture_to`); `NXRS_NRO_CAPTURE=<dir>` writes there and ends the run (exit 0). `nxrs-build`: captured images
join its NRO cache (`<name>_build\nro`), and a build whose NRRs register NROs no file holds boots the built game once
with `NXRS_NRO_CAPTURE` (up to 120 s, `NXRS_NO_ACTIVATE=1 NXRS_KEEP_FOCUS=1`; `NXRS_NO_CAPTURE_BOOT=1` skips) and
builds again with what it caught.

**Translator `--limit`** (nxrs-translate, a diagnostic): with a limit, only the emitted functions are called directly
and the rest through `rt_dispatch`, so a partial build links and its missing functions are interpreted (the e2e mixed
runs use it).

**Player options (all global; owner 2026-11-20: no per-game player settings).**
- Nintendo pads (`Kind::Nin`, `Kind::S2`) read by their printed labels whatever `button_layout` says
  (`input::layout::for_nintendo`: the switch table plus `button_map`); the default layout stays `switch`.
- `mute_on_focus_loss` (Controls, default 0, live): `oe::set_focus` -> `media::sink::set_muted` (device output faded
  out over 10 ms while the window is behind others or minimised; streams, recording and the game go on).
- `ask_user` (Controls, default 0): with more than one registered profile, `ShowUserSelector` and the PlayerSelect
  applet open `ui::pick_user` (`account::choose_user`; cancelled = ResultCancelledByUser 0x27c / UiReturnArg result 2).
- Prompt texts in the game's language: `ui_text::texts(Prompt, min, joined)` by the `language` option (14 tables),
  for the controller prompt, Home quit (front `window.rs` / `window_linux.rs`) and the picker; a front end may override
  lines with `NXRS_CTRL_PROMPT_TEXT`, `NXRS_HOME_QUIT_TEXT`, `NXRS_USER_PROMPT_TEXT` (title, hint; one per line).
  **Launcher (lane Q):** checkboxes for `mute_on_focus_loss` and `ask_user` beside `pause_on_focus_loss`.

**Tests.** `cargo test -p runtime --lib interp` (incl. `hand_off_between_lifted_and_interpreted_code`, a child process:
lifted -> interpreted -> lifted calls, an SVC, a host function, a lifted tail call, discovered.tsv written once and
read back); `nxrt-dev e2e` with `NXRS_INTERP_ONLY=1`; `cargo test -p translator --lib hints`; os `input::layout`,
`media::sink::tests::mute_fades_to_silence_and_back`, `ui::tests::user_picker`, `ui_text`, `ro::capture_tests`.
