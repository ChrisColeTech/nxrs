# Rendering

The Vulkan back end that replays the record layer's command lists on a discrete GPU (`src/graphics/src/vk/`), and
NVN's texture layouts and formats (`src/graphics/src/tex/`). nxnative replays on Metal; nxrs replays on Vulkan 1.3
only (`renderer=vulkan` is the only renderer, DECISIONS.md section 3), so nxnative's Metal and OpenGL material has no
counterpart here. The record layer is in [nvn-objects.md](nvn-objects.md), shaders and pipelines in
[shaders.md](shaders.md), pacing in [frame-rate.md](frame-rate.md).

| Module | Part |
|---|---|
| `device.rs` | instance, device choice, features, validation counters |
| `layout.rs`, `tables.rs` | the one pipeline layout; the bindless pool set |
| `mem.rs`, `lazy.rs`, `gmem.rs` | pool placement, upload, write-back, raw global memory |
| `image.rs`, `sparse.rs`, `scale.rs` | textures as images, sparse images, resolution scale |
| `exec.rs`, `defer.rs`, `counters.rs`, `xfb.rs`, `rt.rs` | the replay of one submission; deferred recording; counter reports; transform feedback; acceleration structures |
| `queue.rs` | submissions, timeline, completion and its CPU-visible effects, queue parking |
| `present.rs`, `upscale.rs`, `screen.rs`, `framegen.rs`, `lsfg.rs` | present, sharp upscale, refresh match, frame generation |
| `perf.rs`, `shot.rs`, `capture.rs`, `lost.rs` | counters and log lines, screenshots, capture / replay, device loss |
| `src/graphics/src/tex/` | block-linear layout and deswizzle (CPU and GPU), formats, ASTC on the GPU and its BC7 cache |

## Device

`vk::device`: Vulkan 1.3 is required (`nxrs-build doctor` checks it). `NXGFX_GPU=<part of a name>` picks a device,
else the discrete GPU with the most local memory. Extensions used when present: dynamic rendering, extended dynamic
state 1-3 and dynamic vertex input, VK_EXT_graphics_pipeline_library, VK_EXT_descriptor_buffer
(`NXGFX_DESCBUF=0` binds sets instead), VK_EXT_mesh_shader (`NXGFX_MESH=0`), VK_EXT_transform_feedback,
VK_KHR_acceleration_structure + ray query (`NXGFX_RT=0`), VK_EXT_robustness2 (`NXGFX_ROBUST2=0`; robust access on
NVIDIA by default, `NXGFX_ROBUST=0/1`), VK_EXT_memory_budget, VK_EXT_external_memory_host, VK_KHR_push_descriptor,
VK_NV_device_diagnostic_checkpoints and VK_EXT_device_fault. `NXGFX_VALIDATION=1` turns the validation layer on.
`driver_exclusive` (ini; auto = on for Intel) keeps pipeline compilation and drawing apart.

## Textures and formats

- **Images by serial.** A texture is found by its initialization serial (games copy NVNtexture objects); images stay
  in GENERAL layout for life (`vk::image`). Textures of the same shape over the same storage share one image
  (`NXGFX_ALIAS=0` gives each its own); when one image takes over another's storage, the old texture object forwards
  to it (`NXGFX_TAKEOVER_FORWARD=0`). Images are made at first use, not at registration (`NXGFX_LAZY_IMAGES=0`:
  MKW registered 7.2 GB of images, most never drawn in a race). Destruction waits until the GPU is past the last use
  (`NXGFX_DESTROY_DELAY`) and runs in queue order (`NXGFX_RELEASE_ORDER`).
- **Layout.** `tex::layout`: Tegra X1 block-linear (GOB 64 bytes x 8 rows; block height from the height in blocks
  x 1.5 / 8); packaged textures carry their 8 bytes of layout parameters. `tex::deswizzle` holds the X1 GOB order
  and the Switch 2's different order of 16-byte pieces.
- **Upload.** Block-linear textures of `MIN_BYTES` and more are deswizzled on the GPU (`tex::deswizzle_gpu`,
  `NXGFX_GPU_DESWIZZLE=0` for the CPU path, which stays the reference).
- **Formats.** `tex::format` maps NVN formats to Vulkan, with a swizzle for formats without a Vulkan layout of their
  own; a view's swizzle composes over the texture's (`NXGFX_SWZ_OVERRIDE=1` replaces instead).
- **ASTC.** Desktop GPUs do not sample ASTC: a compute shader decodes it to RGBA8 (`src/graphics/shaders/astc_decode.comp`, the
  same bytes as astcenc's LDR decode), and 256x256 and larger levels are re-encoded to BC7 (`bc7_encode.comp`, modes
  1, 3, 6, 7). Encoded levels go into an append-only disk cache in the game's data folder (`astc_bc7.pack` /
  `.idx`, `tex::astc_cache`; `NXGFX_ASTC_CACHE=0`, `NXGFX_ASTC_CACHE_MB` default 8192). `NXGFX_ASTC=rgba8` skips the
  BC7 step; `NXGFX_BC7_MODE6=1` is the old mode-6-only encoder.
- **Sparse textures** (`vk::sparse`): textures in a GPU-only sparse virtual pool become Vulkan sparse images; each
  mapped 64 KiB page binds the blocks holding the same texels to memory of that physical page, so pages mapped twice
  share texels. Without it MKW made 19 GB of whole terrain arrays in its first second. `NXGFX_SPARSE=0` (whole images),
  `NXGFX_SPARSE_MIB` (slot memory cap, default half the largest device-local heap).
- **Storage images** without NVN's IMAGE flag: `NXGFX_IMAGE_NOFLAG=1`, `--image_noflag=1` or the control command
  `image_noflag 1` (off by default).
  `NXGFX_IMG_FORMAT=0` keeps storage views in the texture's format instead of the shader's.

## Render targets, passes and clears

Rendering is dynamic rendering over the bound targets, begun at the first draw or clear after a target change and
ended by anything that cannot run inside it (copies, dispatches, barriers); a full barrier follows each of those
(`vk::exec`). A layered pass covers all layers of its targets.

Window origin: NVN's default lower-left origin equals Vulkan's framebuffer orientation once the picture is flipped
at present, so viewports are flipped only for an upper-left origin, and the winding is inverted whenever they are not.

Resolution scale (`vk::scale`, ini `resolution` 0.25..4, live; `dynamic_resolution`, `dynamic_resolution_min`):
compressible 2D targets more than one texel in each direction are made `scale` times larger; viewports, scissors and
clear rectangles follow; the translator's support buffer carries `render_scale` so `gl_FragCoord` and texel fetches
stay right; copies between scales are blits; live changes rebuild scaled images at the next submission.
`shadows=0` skips shadow-map passes (told by a depth-only size of their own).

Linear render targets the CPU reads are copied into their storage at the end of a pass and reach guest memory at the
fence (`NXGFX_RT_READBACK=0` off; MKW's crowds). Copying other GPU-written textures back is `NXGFX_TEX_WB=1`, off
because it washed MKW's race out.

## Blend, depth and stencil

All per-draw state is dynamic (extended dynamic state 1-3), so pipelines are keyed only by shaders, attachment
formats and sample count. Specifics:

- **Alpha test**: applied in the pixel shader (`shaders::link::alpha_test`; `NXGFX_ALPHA_TEST=0` off). Smash hits it.
- **Alpha to coverage**: as the game sets it; control command `a2c 0` or `NXGFX_A2C=0` turns it off live.
- **Dual-source blending**: output 1 becomes Location 0 Index 1 (`shaders::link::dual_source`).
- Depth bounds, sample mask, point size and tessellation levels are recorded commands (`src/graphics/src/nvn/cmd2.rs`).

## Vertex input and draws

- Dynamic vertex input; while nothing of it changes between draws it is not set again (`NXGFX_VIN_SKIP=0` sets it
  every draw).
- Indirect draws: plain records are clamped to sane limits before the draw reads them (16M vertices or indices, 256K
  instances; `NXGFX_CLAMP_INDIRECT=0`); `MultiDraw*IndirectCount` use Vulkan's count draws.
- Append counters used as indirect arguments (Donkey Kong Bananza's terrain) are reset where the frame begins
  (`NXGFX_APPEND_RESET=0`).
- Mesh draws (NVN2 `DrawMeshTasks*`) use real task / mesh stages ([shaders.md](shaders.md#nvn2-stages)); primitive
  indices are held below the vertex count (`NXGFX_MESH_INDEX_FIX=0`).
- Transform feedback uses a capture variant of the last vertex stage (`vk::xfb`).
- A scissor is clamped to its target (`NXGFX_SCISSOR_CLAMP=0`) after an out-of-range scissor lost the device.
- `NXGFX_DRAW_LIMIT` sets the clamp's vertex x instance product limit, in millions (default 256M); a garbage record
  once drew for seconds.

## Uniforms, copies and ordering

- Constant banks: c0 the driver segment, c1 the shader's constants, c2 the handle table, c3 + r the game's uniform
  slot r, c24-c29 NVN2's push and stage banks; uniform buffers are read from guest memory as bound. Equal descriptor
  sets are reused (`NXGFX_SET_CACHE=0` writes every draw's again).
- Storage buffers bound with size 0 get at most `NXGFX_SBUF0_MB` (1 MB); read-only storage buffers extend to the
  end of their memory (`NXGFX_SBUF_TO_END=0`: the size the game gave).
- Pool buffers (`vk::mem`): a pool larger than the device's largest buffer (`maxBufferSize`; `NXGFX_MAX_BUFFER_MB` to try
  it) gets several buffers side by side, and a range is bound from the one holding it (a range across two is red-lined);
  pools of 8 MiB and up are sparse-residency buffers backed in 2 MiB granules when a range in them is first used
  (`NXGFX_POOL_SPARSE=0` backs them whole); CPU writes uploaded at a submission's start take from a 64 MB per-frame
  budget (`NXGFX_UPLOAD_MB`), and past it wait in their pool until a draw, dispatch or copy reads them or a later
  submission has room.
- Raw global memory (`vk::gmem`): a shader access through a pointer it loaded goes through a memory pool table at
  constant slot 255 and a write log; pools stored to are copied back at completion. Donkey Kong Bananza's terrain and
  Mario Kart World depend on it.
- Ordering: every NVN submission is one Vulkan submission of two command buffers (uploads, then the replay); a
  completion thread makes write-backs, event values and counter reports land before the fence counts as reached
  (`vk::queue`). Submissions may merge (`NXGFX_AQ_MERGE=0`); presents queued at most `NXGFX_ASYNC_PRESENTS` (2).
- Deferred recording (`vk::defer`, `NXGFX_DEFER=1`, off by default): a recorder thread makes the Vulkan commands while
  the replay translates the next draws.

## Conditional rendering and queries

- Render conditions read the game's report at the address and address + 16; draws and clears obey them; pending
  reports are waited for (PORT_PLAN 5A.4, "Render-condition semantics"). This fixed MK8D's black world.
- Timestamps are stamped on the GPU where recorded and brought to the guest clock by the smallest difference seen
  (`vk::counters`); NVN2 reports nanoseconds, NVN 614.4 MHz ticks. With `game_dynamic_resolution=0` the reported clock
  runs 256 times slower, so the game sees its GPU work take no time and stays at its largest size.
  Idle GPU time between a frame's submissions is left out of report differences (`NXGFX_REP_GAPS=2`, default);
  `NXGFX_REP_GAPS=1` leaves out only gaps inside a submission (nxnative's rule), which kept Donkey Kong Bananza, with
  8 submissions a frame, at its smallest size.
- SAMPLES_PASSED: occlusion queries around each pass after ResetCounter; ReportCounter writes the sum of completed
  queries in submission order. `NXGFX_OCCL_SYNC` makes this frame's results available to it (MK8D read them two
  presents late).

## Present

`vk::present`: a blit of the game's texture into the acquired swap chain image, letterboxed to its aspect and flipped
for NVN's origin, in queue order; MAILBOX when offered (`NXGFX_PRESENT=fifo|mailbox|immediate`). When the window is
larger than the picture a contrast-adaptive sharpening pass upscales it (`vk::upscale`; `NXGFX_UPSCALE=standard` for the
linear blit, `NXGFX_SHARPNESS` 0..1, default 0.5). Presents and screenshots crop scaled pictures by
the same factor.

## Log lines and diagnostics

| Line / switch | What |
|---|---|
| `[vk] pacing: F FPS \| p95 a p99 b max c ms \| ...` | the pacing contract line, every `NXGFX_PACE_N` presents (300); launcher and harness parse it |
| `[vk] GPU time up to frame N: ...`, `[vk] replay up to frame N, per frame: ... (k skipped: no shader, no pipeline, no target)` | every 60 frames (`vk::perf`) |
| `[vk] render sizes ...` | every 600 frames |
| `[vk] slow frame ...` | a frame longer than `NXGFX_SLOW_MS` (50), with where its time went |
| `[RED] ...` | something unimplemented or wrong (`src/graphics/src/log.rs`) |
| `NXGFX_FRAME_CSV=<path>` | a row per frame from a bounded ring (a slow disk loses rows, never frames) |
| `NXGFX_DUMP_AT_S=<s>,...` / control `passdump` | colour targets of one frame's passes as PNG into `NXGFX_DUMP_DIR` (`captures`), a log line per pass; `NXGFX_DUMP_PNG=0`, `NXGFX_DUMP_TEX=<w>x<h>` |
| `NXGFX_SHOT=<n>,...`, `NXGFX_SHOT_EVERY`, `NXGFX_SHOT_FILE`, `NXGFX_SHOT_DIR`; F3 / F8 | screenshots of the presented picture (`vk::shot`) |
| `NXGFX_LOG_SHADER=<key>`, `NXGFX_SKIP_PS=<key>,...`, `NXGFX_SAMP_LINEAR=1` | per-shader binding log, skip draws by pixel shader, force linear filtering |
| `NXGFX_WATCH_ADDR=<hex>` | images made / released over an address |
| `NXGFX_CAPTURE=<file>` (+ `_AT`, `_FRAMES`) | capture memory and submissions; replay with `nxgfx-replay <file> --shaders <data>/shaders` (`src/graphics/src/bin/nxgfx-replay.rs`); `NXGFX_REPLAY_DUMP`, `NXGFX_REPLAY_TRACE` |
| `NXGFX_CHECKPOINTS` (default dispatches and indirect draws; `all`; `0`), `NXGFX_ADDR_REPORT=1` | device-loss reports (`vk::lost`) |

A capture misses what only the GPU held when it started (earlier render targets), so its first frame can differ.

### Device loss

On `VK_ERROR_DEVICE_LOST` (a hang the driver reset, TDR, or a fault) `vk::lost::on_lost` logs once the last
checkpoints each queue started and finished, the commands between them with their shader keys and indirect
arguments, and the driver's fault report (faulting addresses), then ends the process: nothing can run on a lost
device.

## Problems found on games

### Asynchronous replay black stretches

- **Symptom:** Mario Kart 8 Deluxe shows black stretches with the asynchronous replay.
- **Cause:** a scene target was released on the game's thread while queued work still read it.
- **Fix:** releases run in queue order ([nvn-objects.md](nvn-objects.md#a-texture-released-while-queued-work-reads-it)).

### Black world from ignored render conditions

- **Symptom:** Mario Kart 8 Deluxe's world draws black.
- **Fix:** conditional rendering as Turbo does it: the address and address + 16, counter 0 of timestamps, pending
  reports, clears obeying the condition (PORT_PLAN G-lane, round 3).

### Glyph uploads lost

- **Symptom:** Mario Kart World's menu text is missing.
- **Cause:** glyph uploads lost to page hashes taken after the upload read (PORT_PLAN M7, lane K3).
- **Fix:** built, not yet verified in a run.

### Cyan kart shadow (open)

- **Symptom:** the karts' shadow blob in Mario Kart 8 Deluxe is cyan instead of dark.
- **Cause:** narrowed to the lighting pass where the shadow mask is 0; not fixed (PORT_PLAN M5).

### White layer (open)

- **Symptom:** Super Mario Party Jamboree shows a white layer over the picture.
- **Cause:** not a blend state: the game draws it opaque; still open (PORT_PLAN G-lane).

### Mesh-draw faults (open)

- **Symptom:** the device is lost in Mario Kart World.
- **Cause:** GPU faults left in direct mesh draws of task program fea1d6ca04512de1 (PORT_PLAN M7, lanes K2 / K3).

### Banding (open)

- **Symptom:** colour banding in Anime Girls: Sun of a Beach.
- **Cause:** located in Unity's LUT pass, not fixed (PORT_PLAN G-lane, G5).

## Known limits

Tracked in PORT_PLAN 5A.4: recording on several threads (Partial); ray tracing command entry points (Blocked); RenderDoc,
Aftermath and pipeline statistics in the device-loss tooling (Partial).
