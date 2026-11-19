# Module interfaces

Each module documents here the API other modules call. Keep entries short; update yours when it changes.

## translator (src/translator)
Package API (`translator::package`): `open_game`, `open_data_dir`, `RomFs::{list, find, read}`, `update_info`, `dlc::*`. See src/translator/README.md.

**Request from os (lane A):** seed every relocation target that names a symbol the module defines in `.text`, of any
symbol type. MK8D's `sdk` exports six hand-written routines with untyped (`STT_NOTYPE`) symbols
(`nndetailCryptoBignum{MulAdd,Mul,Add,Sub}Words`, `nndetailCryptoBignum_{mul,sqr}_comba8`, sdk+0x4c30..0x5288),
reached only through the PLT: they are not function starts, so the first call stops in `rt_dispatch` ("no function at
guest pc sdk+0x4c30"). os takes them over by name meanwhile (`services/bignum.rs`).

## runtime (src/runtime)
Guest CPU ABI (`cpu.h`), dispatch, host-call bridge (`hostfn`: host functions registered at guest addresses / by symbol name), svc dispatch, kernel, memory. See src/runtime/README.md.

## os (src/os)
The OS services: nxnative's HLE layer (SDK service clients taken over by mangled name), the IPC framework with `sm`,
memory / IPC / info SVCs, the system counter, virtual cores, the process boot. See src/os/README.md.

**Building a game against it** (what `nxos-dev build` does; the cli should do the same):
- Compile the generated C with `-I src/os/include` **first**, then the translator's `include/` and `src/runtime/include`.
  The overlay `cpu.h` (`#include_next`s the translator's) routes `rt_svc` → `nxos_svc`, `rt_ticks` → `nxos_ticks`,
  `rt_spin_gate` → `nxos_spin_gate`, and makes `RT_TICK_HZ` the variable `nxos_tick_hz` (19.2 MHz Switch 1, 31.25 MHz
  Switch 2, from the title's profile).
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
`nxos::services::hid::set_input(buttons, lx, ly, rx, ry)` (the front end's player-1 input).

**NVN registration (agreed with graphics, lane B):** os owns `nvnBootstrapLoader` and hands out guest addresses for
NVN procs (`nxos::nvn::proc_address(name)`), from, in order: the graphics layer's procs, os's null device, a
logging stub per name. The binary that links both registers graphics from the `nxos_main_with` hook:
```rust
for e in nxgfx::nvn::procs() { nxos::nvn::register(e.name, e.f); }   // graphics' procs win over the null device
nxgfx::nvn::set_proc_resolver(nxos::nvn::proc_address);               // graphics' nvnDeviceGetProcAddress
nxgfx::nvn::set_guest_caller(nxos::guest::call_current);              // command-memory callbacks into the game
```
Graphics calls `nxos::nvn::note_submit(queue, n)` and `nxos::nvn::note_present(window, index)` from its
`nvnQueueSubmitCommands` / `nvnQueuePresentTexture` (the run's log marks and vi's frame number), and gives its core up
(`nxos::sched::without_core`) wherever it blocks a guest thread (vsync waits, fence waits).

**Requests to the runtime** (worked around in os meanwhile):
1. A hook for SVCs the runtime does not handle, or a registry (`svc::set_handler(imm, f)`), instead of a fatal; os
   routes `rt_svc` through `nxos_svc` with the overlay header today.
2. `rt_ticks` / `RT_TICK_HZ` at a rate set at start (19.2 MHz for Switch 1); os overrides both in the overlay.
3. Kernel hooks for a scheduler: thread start and exit, blocking-wait enter and leave, priority and core mask changes
   (os's virtual cores wrap CreateThread's entry and the blocking SVCs today), and `GetCurrentProcessorNumber` from it.
4. `process.rs`: an entry other than the main module's first instruction (an SDK title starts in rtld; os starts the
   main thread at its own host function and runs nn::init's steps), and lazy heap commit in `mem::Space`.
5. A public way to create kernel events and signal them from host code (os calls `kernel::svc` with a scratch `Cpu`).

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

**For the translator crate:** `loader::nso::Nso::parse` cannot read ZBIC-compressed segments (Switch 2; Ocarina of
Time 2.0's `main`). This crate decodes NSO segments itself (LZ4 and ZBIC, `shaders::formats::nso::image`).

## cli (src/cli, binary `nxrs`)
The build and the launcher contract (docs/LAUNCHER_CONTRACT.md). What a built game is, for the lanes that run it:
- `<outdir>\<name>.exe` is a loader with no runtime in it: it sets `NXRS_DATA=<outdir>\<name>_data` and `NXRT_EXEFS=<data>\exefs` (unless already set), loads `<name>_code.dll` and calls its export `int nxrs_game_main(int argc, char **argv)`, which sets the patch switches and calls `nxrt_main(argc, argv, mods, n)`. argv is the exe's own.
- `<name>_code.dll` = every generated chunk + `modules.c` + **`nxrt.lib` linked statically** (no `nxrt.dll`): the runtime and the code it runs are one image. A crate the runtime needs is linked by being in `nxrt.lib`; extra system libraries: `NXRS_LINK_LIBS` (space-separated `-l…`), or tell lane D to add them to `cc.rs` `SYS_LIBS`.
- `rt_patch_on[i]` is set before `nxrt_main` from `patches.h` `RT_PATCH_SETS`: the set's default, or its environment switch (`0` off, anything else on).
- nxrs finds `nxrt.lib` beside itself (or `NXRS_RUNTIME_LIB`) and clang via `NXRS_CLANG`, the bundled `clang\bin`, `NXRT_LLVM`, `C:\Program Files\LLVM\bin`, PATH.
- Shaders: the build calls `shaders::prebuild` once lane C publishes it here (its signature: what it reads from `<data>` and the game's `RomFs`, what it writes under `<data>`); until then the build logs the skip.
- `nxrs translate <input> --out <dir>` keeps what `nxrt-dev e2e` calls (the translator's dev binary is now `nxrs-translate`).

## profiles (src/profiles)
nxnative's 187 `dm/games` profiles + its shared patch tables, embedded as data (`games/<id>/game.tsv`, `patches.toml`; the owner's `turbo.toml`), and the lookup:
- `profiles::lookup(title_id: Option<u64>, main_build_id: Option<&str>) -> Option<Match { profile, build: Option<&Build>, how: How::Build | How::Title }>`; `by_title(u64)`, `by_build(&str)`, `get(folder id)`, `all()`. Build ids are lowercase hex; a key matches over the shorter of the two (≥ 16 bytes).
- `Profile`: `title_id`, `program_id`, `name`, `builds` (`version`, `display`, `modules` (name, build id)), `modules_of(build)` (load order), `memory` (`reserve`, `alias`, `stack`, `app_pool`, `used`, `sys_resource`, `stdalloc_cap`, `map_limit`), `settings` (fps_target, fps_max, op_mode, perf_mode, core_preempt, shared_font, device_api, game_args, game_env, …, with nxnative's defaults), `platform()` / `tick_hz()` (19.2 MHz Switch, 31.25 MHz Switch 2), `get(key)` for any other `game.tsv` key.
- Patches: `profiles::patches_for(module_build_id)` → every `Cut` / `Obs` / `Stub` / `Capability` keyed by that build, from any profile or `shared_patches()` (SDK, engine). A recorded `StubBody` places itself: `body.place(&Layout { area, state, base, others })` → words with relocations applied. `translator_patches(&[(module, build id)])` → the translator's patch-table text for the entries nxrs applies at build time (today the owner's Z-A 60 FPS cut).
- `player_settings()`: nxnative's settings.tsv rows. `source()`: the nxnative commit.
