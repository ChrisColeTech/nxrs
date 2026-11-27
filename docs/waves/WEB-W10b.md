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

### Built (as planned, items 1-4)

- `gpu-worker.js`: BC stand-ins (`createTexture`, `standinFormat`, `copyTex`, `texWritten` decode pass); format
  shadows (`BITS` table of 32-bit formats, `shadowOf`, `reinterp`, `viewWritten` after every pass end, copy, blit and
  clear; shadows and staging textures destroyed with their texture).
- `shaders/src/wgsl.rs`: `Wgsl.interp`, `Wgsl.variant`, `Wgsl.spirv`; `Variant { interp, packed }`, `convert_with`,
  `interp_fix`, `set_interp` (result struct copied with the fragment's interpolation), `unpack_inputs`
  (A2B10G10R10 bit fields; SNORM `max(v / 511, -1)`).
- `graphics/src/web/icd.rs`: `vs_variant` (one worker module per (module, variant)); pipelines take the fragment's
  interpolation at creation. `fmt.rs` `packed`; `submit.rs` feeds packed attributes as `uint32` with the variant.

### Tests

`cargo test --release -p shaders --lib wgsl`: `vertex_outputs_take_the_fragment_interpolation` (GLSL test modules in
`src/shaders/testdata/wgsl`, compiled with glslangValidator) and `packed_vertex_inputs_are_unpacked_from_a_u32`:
3 passed. The worker's passes have no unit test (WebGPU only in Chrome): verified in the game below.

### MK8D in Chrome (GPU 1), `build/WEB/W10b/mk8d/run1`

All four logged causes are gone: no BC size error, no view-format error (`texture 2398 (rgba8unorm 1280x720) viewed as
rgb10a2unorm: a shadow texture kept bit-equal`), no interpolation error, no vertex-format red line. 54.8-60 FPS. The
game still stays on its title logo (t60 = t140 byte-identical; A presses logged): **14,940 WebGPU errors by 157 s
from one new cause**, the next blocker:

```
[gpu] WebGPU: [Texture (unlabeled 1920x1080 px, TextureFormat::Depth32Float)] usage (TextureBinding|RenderAttachment)
includes writable usage and another usage in the same synchronization scope.
[gpu] WebGPU: [Invalid CommandBuffer] is invalid due to a previous error.
```

A pass samples its own depth attachment (Vulkan: depth writes off, the read-only depth layout). WebGPU allows that
only with `depthReadOnly: true` (and no depth load/store ops) on the attachment; the web pass begin does not know yet
whether a draw of the pass writes depth. Next step: the replay's pass (submit.rs `begin_pass`) marks the depth
attachment read-only when no pipeline of the pass writes depth and a bind group of the pass samples it (or splits the
pass at the first sampling draw). Also seen once: `uniform bank c3 ... is in no memory pool: zeros bound` red lines at
26 s, and `movie: decoder ... event 0 not delivered` at 57 s.

### Regressions

- Beach Bounce rebuilt with this branch's nxrs-build (backup in `build/WEB/W10b/bb/backup-before`), Chrome GPU 1,
  `build/WEB/W10b/bb/run1`: title, then the hospital story scene at 100 s, 59.3 FPS, 0 WebGPU errors, 0 red lines.
  The W10-MK8D heap split does not hurt it.
- Anime Girls: not re-run (deadline).

### Notes

MK8D's previous web files were backed up to `build/WEB/W10b/mk8d/backup-before` before the rebuild.
