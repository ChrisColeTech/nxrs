# WEB-W10b: Mario Kart 8 Deluxe past its title logo in Chrome (WebGPU validation errors)

Branch `web-w10b` (from `gameplay-harness` 1162e39). Title: Mario Kart 8 Deluxe (0100152000022000), web folder
`P:\Games\Nintendo\DUMPS\Mario Kart 8 Deluxe\nxrs-web\Mario Kart 8 Deluxe 1G1U1D`; regressions Anime Girls (beach) and
Beach Bounce (title menu). Chrome on GPU 1 only (`--use-adapter-luid=0,113326`), one game Chrome at a time.

## Symptom (WEB-W10-MK8D, `build/WEB/W10/mk8d/run-title`)

The title logo at 60 FPS, then nothing changes: 16,281 WebGPU validation errors by 157 s, each invalidating a whole
command buffer. Four causes logged:

1. BC textures whose size is not a multiple of 4 (`bc4-r-unorm 80x45`, `bc5-rg-unorm 74x74`, `bc7 170x105`): Vulkan
   allows them, WebGPU refuses the texture, and every view, bind group and command buffer using it after.
2. `rgb10a2unorm` views of an `rgba8unorm` 1280x720 texture (a MUTABLE_FORMAT image viewed in another 32-bit format):
   WebGPU only allows the srgb twin as a view format.
3. A render pipeline whose vertex output 0 is `perspective` and fragment input 0 `linear`.
4. `A2B10G10R10_SNORM_PACK32` vertex attributes (no WebGPU vertex format): the draw was refused.

## Plan (native behaviour is the reference)

1. **BC stand-in** (`src/web/gpu-worker.js`): a BC texture whose base size is not whole blocks is realised as a texture
   of its decoded format at the logical size (`rgba8unorm[-srgb]` for BC1/2/3/7, `r16float`/`rg16float` for BC4/5,
   `rgba16float` for BC6H) plus a staging BC texture rounded up to whole blocks. Copies go into the staging texture
   (their block extents are valid there); each copied region is then decoded into the stand-in by a small render pass
   that `textureLoad`s the staging texture (the GPU's own BC decoder). Views and sampling use the stand-in, so sampling
   coordinates, `textureDimensions` and mip sizes stay the logical ones (Vulkan's).
2. **Format aliases** (`gpu-worker.js`): a view of a size-compatible format WebGPU cannot view (rgb10a2unorm of
   rgba8unorm, ...) gets a shadow texture of the view's format; the bits are carried between the texture and its
   shadows by a reinterpreting render pass after every write (pass end with one of them attached, copies, clears,
   blits), so each sees what the other wrote, as Vulkan's shared memory does.
3. **Interpolation** (`src/shaders/src/wgsl.rs`, `src/graphics/src/web/icd.rs`): Vulkan takes the fragment stage's
   interpolation; WebGPU requires both stages to match. At pipeline creation a vertex module whose outputs differ from
   the fragment inputs is converted again with the fragment's interpolation and sampling on those outputs.
4. **Packed vertex formats** (`wgsl.rs`, `src/graphics/src/web/fmt.rs`, `submit.rs`): A2B10G10R10 SNORM / UINT /
   SINT / USCALED / SSCALED attributes are fed as `uint32` and unpacked in a variant of the vertex module (bitfield
   extracts, normalisation with the -1 clamp of Vulkan's SNORM rule).

Expected files: `src/web/gpu-worker.js`, `src/shaders/src/wgsl.rs`, `src/graphics/src/web/{icd,submit,fmt,obj}.rs`,
this document, `docs/35-WEB-TARGET-PLAN.md` (W10 row).

## Results

(filled in as the work lands)
