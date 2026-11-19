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

## graphics (src/graphics)
_(lane B)_

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
