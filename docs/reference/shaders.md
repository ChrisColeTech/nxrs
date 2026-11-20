# Shaders

Every shader of a game is found and translated to SPIR-V when the game is built, before it ever runs; a shader the
build missed is translated while the game runs. Build side: `src/shaders/` (CLI `nxshaders`); run side:
`src/graphics/src/vk/shader.rs`, `layout.rs`, `pipe.rs`, `live.rs`, `pcache.rs`, `spvfix.rs`, `xfb.rs`, `gmem.rs`.
The translator is `dmsh_xlate`: nxnative's tool over Ryujinx's shader translator, built as a native executable with
`src/shaders/tools/dmsh_xlate.patch` (Vulkan target, a descriptor set per stage) and run as separate processes.

| Module | What |
|---|---|
| `src/shaders/src/nvn.rs` | control section + code area, the shader key, stages, checks |
| `src/shaders/src/formats/` | container readers, by content, never by name |
| `src/shaders/src/scan.rs` | a whole game: ExeFS modules + RomFS (update applied) + DLC, in parallel, one copy per key |
| `src/shaders/src/job.rs` | a shader to a translator job (Maxwell or Ampere code, c1, facts) |
| `src/shaders/src/xlate.rs` | `dmsh_xlate` processes over batches, then `spirv-val` |
| `src/shaders/src/fixes.rs`, `link.rs`, `mesh.rs`, `rt.rs` | SPIR-V edits; NVN2 task / mesh and ray-tracing stages |
| `src/shaders/src/facts.rs`, `census.rs`, `reflect.rs` | per-game answers; LOD-clamp census; binding reflection |
| `src/shaders/src/prebuild.rs` | the whole build step and its manifest |

## What a shader is

Whatever file holds it, an NVN shader is a control section and a code area (`nvn.rs`):

- **NVN (Switch 1):** control magic `0x98761234`, 0x880 bytes (+0x20 its own length when 0x760..0x880, +0x700 the c1
  segment's start in the code area, +0x704 the code length). Graphics code starts with a 0x30-byte header
  (`0x12345678`) and a 0x50-byte SPH; compute may have a `0x12345679` header or none.
- **NVN2 (Switch 2, Ampere):** version `0xc9` at +4, own length at +0x30, code length at +0x44, stage byte at +0x56
  (5 compute, 6 task, 7 mesh, 8 ray tracing); code starts with a 0x80-byte SPH.
- **Key:** the first 16 hex digits of SHA-256(control bytes + code area). Control bytes: NVN2's first 0x48 with
  +0x30..+0x34 zeroed; else the section's own length when 0x760..0x880; else 0x880. The runtime computes the same key
  at `nvnProgramSetShaders` (`src/graphics/src/nvn/prog.rs`), since NVN lets the caller free the control section
  afterwards.

## Containers by engine

`src/shaders/src/formats/mod.rs` decompresses loose files by magic (zstd with the game's dictionaries, gzip, Yaz0, size-prefixed
frames, LZ4, Oodle via the `lztans` crate), opens archives member by member, and reads every container inside:

| Reader | Engines and games |
|---|---|
| `nvnfmt.rs` | BNSH, agl BAHS (sharcb), NVN2 packs (NDSR, NDVE, sead's primitive drawer), Shin'en SHN!, MercurySteam MSHD |
| `glslc.rs`, `raw.rs`, `nso.rs` | GLSLC outputs (magic `0x19866891`) in files or code; control sections loose in main (Bink 2, Diablo II, PhyreEngine); NSO segments (LZ4 / ZBIC) |
| `unity.rs`, `unreal.rs` | Unity bundles and serialized files; Unreal paks (v4 / 8 / 11), `.ushaderbytecode`, inline shaders |
| `engines.rs`, `studios.rs` | Snowdrop, Heaps, Hedgehog PACx 403 / Needle; Sumo, Firaxis, PhyreEngine, Viper, Larian, Bezel |
| `gamefreak.rs`, `monolith.rs`, `retro.rs`, `nlg.rs`, `koei.rs`, `capcom.rs`, `fromsoft.rs`, `ttgames.rs`, `frostbite.rs`, `smash.rs`, `monogame.rs` | GFPAK / Trinity; Xenoblade ARH; Retro RFRM; Next Level Games; KTGL; RE Engine / MT Framework; DSR dvdbnd; TT Games CC4; Frostbite cas; Smash `data.arc`; MonoGame XNB |
| `packs.rs`, `archive.rs` | PKZ, RPF6, CRA, CASC, PDAT, zip, LSPK, KIT, SRYK, IPK; Yaz0, SARC, CPK / CRILAYLA |

PORT_PLAN S-lane: 184k shaders over 7 games, 0 failed.

## Translation

`prebuild::prebuild_data` (the build's call, `nxshaders prebuild-data <data>`):

1. Scan the ExeFS, RomFS and installed DLC (`scan.rs`), keeping one copy per key and vertex / pixel pairs.
2. Make a job per shader (`job.rs`): Maxwell instructions after the 0x30 header, Ampere after the cut to the code
   length; the c1 segment (indirect jump tables); facts the code cannot give (compute thread groups, NVN2 compute's c0
   layout and stage banks, task / mesh stage memory, early fragment tests, transform feedback, tessellation mode).
3. Apply per-game facts (`facts.rs`): the profile's `shader_facts.tsv` and hand-written `shader_facts_manual.tsv`,
   copied into `<data>\shaders\profile\` by the build: texture and image dimensions, image formats, attribute and
   output component types, thread groups, shared memory. A changed fact retranslates the shader (it is in the job hash).
4. Run `dmsh_xlate <jobs.json> <out>` as several processes (`xlate.rs`); a batch whose process dies or hangs is split
   and rerun, so one crashing shader costs only itself. Refusals print `[RED] <key>: <reason>`.
5. Edit (`fixes.rs`): storage-image without-format capabilities, size-query components for textures assumed 2D, and
   variable texel-fetch offsets folded into coordinates.
6. Validate with `spirv-val` for Vulkan 1.3; write the manifest.

Tools are found at `NXSHADERS_XLATE` / `NXSHADERS_SPIRV_VAL`, else beside the binary (`tools\`), else the repository's
build folder. A rebuild reuses every output whose inputs and tools are unchanged. The build never fails on shaders:
a failure is a warning and the live translator covers the game ([packing.md](packing.md#the-build)).

Output (`<data>\shaders\`, layout `nxshaders 1`):

| File | What |
|---|---|
| `manifest.tsv` | per key: stage, translated as, arch, status (`ok`, `invalid`, `failed`, `library`), size, source |
| `spv/<key>.spv` | SPIR-V 1.3, entry `main` |
| `spv/<key>.json` | reflection: bindings, execution model, transform feedback, the fixes applied, `status`; written after the `.spv` |

### NVN2 stages

- **Task / mesh** (`mesh.rs`): the translator emits a compute kernel over one workgroup array; this pass wraps it in a
  real TaskEXT / MeshEXT entry point (SPIR-V 1.4): clear stage memory, store the workgroup number (from the draw's
  `{count, first}` record), run the kernel, then launch mesh workgroups or emit vertices and primitives. Positions are
  invariant (Ocarina of Time draws foliage twice and tests depth equality). Graphics binds task shaders to set 3 and
  mesh shaders to set 4.
- **Ray tracing** (`rt.rs`): every stage-8 module is linked into one library, translated as one compute kernel that
  traces with ray queries. Graphics' acceleration structures exist (`src/graphics/src/vk/rt.rs`); the command entry
  points do not (PORT_PLAN 5A.4, Blocked).

## Bindings and handles

At module load graphics rewrites each module's DescriptorSet / Binding decorations into one shared layout
(`vk::layout`), so programs never need layouts of their own:

- set s (0-5) = NVN stage s; binding b < 31 = constant bank c[b] (c0 driver segment, c1 constants, c2 handle table,
  c3 + r the game's uniform slot r, c24-c29 NVN2 push and stage banks); 31 the translator's support buffer; 32 + k
  storage buffers; 64 + k textures; 128 + k images; 144 + k / 160 + k buffer textures / buffer images.
- set 6 = the pool set: textures, samplers, images and texel buffers indexed by host ID, for shaders that index NVN's
  pools directly. Table sizes: `NXGFX_TEX_CAP` (262144), `NXGFX_SAMP_CAP` (16384).
- push constants (128 bytes): the active texture pool's base, the sampler pool's base, flags (bit 0: flipped viewport).
- constant slot 255 is reserved for raw global memory (`vk::gmem`).

Handle-table texture arrays are rewritten onto the pools (`NXGFX_HANDLE_TABLES=0` refuses such shaders as before);
Jamboree's 1,413 refused shaders build with it (PORT_PLAN G5).

## Edits at load and link

Applied by graphics when a module meets the device and its partner stage; files on disk stay as translated
(`src/shaders/src/link.rs`, `src/graphics/src/vk/spvfix.rs`, `src/graphics/src/vk/shader.rs`):

| Edit | Switch | Why |
|---|---|---|
| `trim_cull` | (always) | ClipDistance[8] + CullDistance[1] exceed devices with 8 combined |
| `io_locations` / `add_outputs` | `NXGFX_LINK_IO=0` | a pixel shader reads a varying its vertex shader never writes |
| `alpha_test` | `NXGFX_ALPHA_TEST=0` | NVN's alpha test in the pixel shader |
| `dual_source` | | dual-source blend output as Location 0 Index 1 |
| `ballot_per_lane` | `NXGFX_BALLOT_FIX=0` | NVN's warp-aggregated atomic add hangs RTX 50 GPUs |
| `late_outputs` | `NXGFX_LATE_OUTPUTS=0` | vertex outputs stored once at the end: NVIDIA's compiler faulted on DKB's d87ccbede95b9f2c |
| `spvfix::kill_demote` | `NXGFX_DEMOTE_FIX=0` | a discard turned into demote-to-helper: NVIDIA lost the device on a pass ending pixels midway (mkw-recomp #22 / #231) |
| loop guard | `NXGFX_LOOP_GUARD=<n>` | bounds back-edges a shader invocation may take (0 unbounded) |
| mesh index fix, zero shared | `NXGFX_MESH_INDEX_FIX=0`, `NXGFX_ZERO_SHARED=0` | MKW's mesh-draw device losses; zeroed task / mesh shared memory |
| `spvfix::sabs_product` | `NXGFX_SABS_FIX=0` | integer abs as a product, which NVIDIA's compiler does not turn into a GPU fault (mkw-recomp #395) |
| transform feedback variant | | `vk::xfb`: captured outputs split to their component counts, Xfb decorations |

`NXGFX_SPVFIX_IN` / `NXGFX_SPVFIX_OUT` run the `spvfix` rewrites over a folder for offline checks.

## Shaders compiled at run time

- **Live translator** (`vk::live`): a shader whose key is not in `<data>\shaders` is translated on a thread of its own
  into `<data>\shaders\live\<key>\` with the same translator, fixes and validation (`NXGFX_LIVE=0` off,
  `NXGFX_LIVE_JOBS` 4). A draw whose shaders are pending waits once per set of programs, at most
  `NXGFX_SHADER_WAIT_MS` (3000; 0 skips at once), capped per frame by `NXGFX_SHADER_WAIT_FRAME_MS` (1500).
- **Games with their own GLSL compiler**: the compile is the game's; `NXOS_GLSLC_CAPTURE=<dir>` saves each output as
  `<FNV-1a>.glslc` for offline translation (`src/os/src/services/nvdrv.rs`).
- `NXGFX_SHADERS=<dir>` names another translated folder.

## Pipelines and caches

`vk::pipe`: all per-draw state is dynamic, so a graphics pipeline's key is its shaders, attachment formats and sample
count; the pipeline count stays near the program count (NVIDIA's driver grew to 61 GB when one game's pipelines
multiplied per state).

- **Pipeline libraries** (VK_EXT_graphics_pipeline_library): vertex input, pre-rasterization, fragment shader and
  fragment output parts compiled once each; a new combination is only linked (about 1 ms against 50-250 ms), and the
  optimized whole pipeline replaces it from a background thread. `NXGFX_GPL=0`: whole pipelines only;
  `NXGFX_PART_LTO=1` retained parts; `NXGFX_PARTS_PRIVATE_MB` (6 GB) stops making parts ahead.
- **Key file and prewarm:** new keys are appended to a key file in `NXGFX_CACHE` (default the data folder); the next
  run builds them on background threads (`NXGFX_PREWARM_THREADS` 4) up to `NXGFX_PREWARM_BUDGET_MB` (2048) private
  memory or `NXGFX_PREWARM_MAX` (50,000) pipelines.
- **First run:** with fewer than `NXGFX_PROFILE_PREWARM_MIN` (64) keys, the profile's pipeline manifest seeds the
  prewarm (`pipe::prewarm_profile`, `NXGFX_PROFILE_PREWARM=0` off).
- **VkPipelineCache on disk** (`vk::pcache`): saved a few seconds after new pipelines stop (`NXGFX_PCACHE_QUIET_MS`,
  2000) through a temporary file and an atomic replace; used only when its header names this device and driver.
- `shader_thread=1` (ini) builds pipelines on their own threads; `low_priority_shader_compiler=1` at low priority;
  `driver_exclusive` keeps compiling and drawing apart (Intel).
- Diagnostics: `NXGFX_PIPE_LOG=1` (each driver compile logged at start and end, finds the one a compiler crash was
  in), `NXGFX_PIPE_NOOPT=1`, `NXGFX_RETIRE_KEEP=1`. The test `src/graphics/tests/shaders_lane_c.rs` builds vertex /
  pixel pairs of a game's folder (`NXGFX_TEST_SHADERS`, `NXGFX_TEST_PAIRS`, `NXGFX_TEST_PAIR` for one pair).

## Tools

```text
nxshaders prebuild <game> --out <dir> [--jobs 4] [--scan-jobs 8] [--scan-only] [--limit N]
nxshaders stage <game> --data <dir>          a data folder as the build stages it
nxshaders prebuild-data <data> [--jobs 4]    the build's call
nxshaders tex-census <export dir>            LOD-clamp / residency uses the translator drops (census.rs)
nxshaders coverage <out dir> <used>...       which keys a game used were found and translated
```

## Problems found on games

### Pipelines made at every new place

- **Symptom:** Super Mario Bros. Wonder's frames take 60-2,000 ms at every new place.
- **Cause:** whole pipelines compiled at first draw.
- **Fix:** pipeline libraries (above); the first-entry stutter of a first run is not fully fixed (PORT_PLAN G-lane).

### Draws skipped while their shader is translated

- **Symptom:** Mario Kart 8 Deluxe's race stays black after the course loads.
- **Cause:** the course's lighting is drawn once at load; a skipped draw is not made up by later frames.
- **Fix:** draws wait for the live translator (above).

### LOD clamp dropped

- **Symptom:** ASTC's error colour on streamed sparse textures (nxnative's Densha de GO!! case).
- **Cause:** the translator drops Maxwell `.LC` and residency predicates, so levels mapped but not filled are sampled.
- **Fix:** none in the translator; `nxshaders tex-census` lists the games and shaders that have them.

## Known limits

- nxnative's run-time GL shader capture for Metal and its Metal pipeline caches have no counterpart.
- mkw-recomp's `sm86` translator is only a cross-check (`src/shaders/tools/crosscheck_sm86.py`), not a fallback.
- PORT_PLAN 5A.5 ("Shader translation facts ... tests") is Partial: some of nxnative's shader regression tests are not
  ported.
