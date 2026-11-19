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

## shaders (src/shaders)
Every shader of a game found at build time and translated to SPIR-V before the game runs. See src/shaders/README.md.

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
| `manifest.tsv` | `#` lines (layout, translator chain, counts), a header, then per key: `key stage translated_as arch status spv_bytes source`. `status`: `ok` (spirv-val clean), `invalid` (refused by spirv-val; do not load), `failed` (no SPIR-V; `why` in its json), `skipped` (ray-tracing programs) |
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
  compute 5 (NVN2 task 6, mesh 7), bindings from 1 up. No two stages share a set; gaps take empty set layouts.
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
  `transform_feedback` (vertex / geometry), `mesh_stage` (task and mesh shaders are translated as compute with their
  stage memory in shared memory; not yet Vulkan task / mesh stages), `uses_frag_coord`, `frag_out_map`, `fetch1d`,
  `capabilities` (SPIR-V ids), `fixes` (SPIR-V edits applied), `notes` (the translator's).
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

## cli (src/cli, binary `nxrs`)
The build and the launcher contract (docs/LAUNCHER_CONTRACT.md). What a built game is, for the lanes that run it:
- `<outdir>\<name>.exe` is a loader with no runtime in it (a windowed program, 16 MB main stack): it sets `NXRS_DATA=<outdir>\<name>_data` and `NXRT_EXEFS=<data>\exefs` (unless already set), loads `<name>_code.dll` and calls its export `int nxrs_game_main(int argc, char **argv)`, which sets the patch switches and calls **`nxfront_main`** (a retail title: `sdk` plus `rtld` or `main.npdm`) or `nxrt_main` (the runtime's homebrew). argv is the exe's own (`--key=value` options pass through to nxconfig).
- `<name>_code.dll` = every generated chunk (compiled with os's `cpu.h` overlay first for a retail title) + `modules.c` + **`nxfront.lib` linked statically** (front + os + graphics + runtime; `nxrt.lib` for homebrew). Extra system libraries: `cc.rs` `SYS_LIBS` / `ENGINE_LIBS`, or `NXRS_LINK_LIBS`.
- `rt_patch_on[i]` is set before the entry from `patches.h` `RT_PATCH_SETS`: the set's default, or its environment switch (`0` off, anything else on).
- Toolchains: with Visual Studio, clang (`NXRS_CLANG`, bundled `clang\bin`, `NXRT_LLVM`, LLVM's folder, PATH) for `x86_64-pc-windows-msvc` and `nxfront.lib` / `nxrt.lib` beside nxrs (`NXRS_ENGINE_LIB`, `NXRS_RUNTIME_LIB`); **without it**, the bundled llvm-mingw (`llvm-mingw\bin` beside nxrs, `NXRS_LLVM_MINGW`) for `x86_64-w64-windows-gnu` and the engine built for `x86_64-pc-windows-gnullvm` in `gnu\` beside nxrs (`libnxfront.a`, `libnxrt.a`, `libwindows.0.52.0.a`; `NXRS_GNU_LIBS`). `NXRS_TOOLCHAIN=msvc|gnu` forces one. The gnu DLL imports only system DLLs and the UCRT.
- Shaders: the build calls `shaders::prebuild_data(<data>)` after staging the data folder (output `<data>\shaders\`) and writes the profile's nxnative shader tables (`pso_manifest.tsv`, `pso_pairs.tsv`, `shader_pairs.tsv`, `shader_facts.tsv`) into `<data>\shaders\profile\` for prewarm (lane G may read them). `--no-shaders` skips both.
- `nxrs translate <input> --out <dir>` keeps what `nxrt-dev e2e` calls (the translator's dev binary is now `nxrs-translate`).

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
