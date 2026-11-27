# WEB-W10c: Mario Kart 8 Deluxe past its title logo in Chrome (a pass sampling its own depth attachment)

Branch `web-w10c` (from `gameplay-harness` b0ba530). Title: Mario Kart 8 Deluxe (0100152000022000), web folder
`P:\Games\Nintendo\DUMPS\Mario Kart 8 Deluxe\nxrs-web\Mario Kart 8 Deluxe 1G1U1D`; regression: Anime Girls (beach).
Chrome on GPU 1 only (`--use-adapter-luid=0,113326`), one game Chrome at a time. Evidence `build/WEB/W10c`.

## Symptom (WEB-W10b, `build/WEB/W10b/mk8d/run1`)

The title logo at 60 FPS, then nothing changes; 14,940 WebGPU errors by 157 s, all one cause:

```
[Texture (1920x1080 Depth32Float)] usage (TextureBinding|RenderAttachment) includes writable usage and another usage
in the same synchronization scope.
```

A render pass samples the depth texture it has as its depth attachment. Vulkan allows that while the pass's draws do
not write depth (the attachment is only read); WebGPU allows it only when the attachment is declared
`depthReadOnly` (`stencilReadOnly` for the stencil aspect), which forbids load/store ops on that aspect and pipelines
that write it. The web replay always declared the attachment writable.

## Plan (native behaviour is the reference)

The replay (`src/graphics/src/web/submit.rs`) resolves a pass's draws one at a time and only then knows which draws
sample the attachment and which write it. So a pass is replayed as **segments**:

1. A segment starts at the pass begin (as recorded). It tracks, per aspect (depth, stencil), whether a draw of it
   sampled the attachment's image (a bound texture view of that image, its aspect, overlapping the attachment's mip
   and layers) and whether something wrote it (a clear load op, a draw whose pipeline writes the aspect, a
   `vkCmdClearAttachments` of it).
2. A draw that samples an aspect the segment has written, or writes one the segment has sampled, ends the segment
   before the draw: the segment's colour and depth/stencil stores are kept (`store`), and a continuation segment
   begins with every attachment loaded (no clears), the same views, resolves and formats. Vulkan sees one pass;
   WebGPU sees two that leave the same contents.
3. When a segment ends (split or pass end), each aspect it sampled and never wrote is marked read-only in its begin
   packet (worker mode 3: `depthReadOnly` / `stencilReadOnly`, no load/store ops). Depth and stencil separately.
4. A draw that samples and writes the same aspect at once (a real feedback loop, undefined in Vulkan without the
   feedback-loop extension) is left as is and named in a red line; evidence decides whether it needs more.

Worker (`src/web/gpu-worker.js` `PASS_BEGIN`): mode 3 = read-only aspect.

Tests: the segment rules and the packet edits are pure functions (`src/graphics/src/web/seg.rs`), compiled into a
host test (`cargo test --release -p graphics --lib web_seg`).

Then the next blocker on the way to the title menu and a race, as time allows (seen in W10b: `uniform bank c3 ... in
no memory pool` red lines at 26 s, a movie decoder event not delivered at 57 s). Anime Girls re-run once with the
current engine (web build rebuilt into its library web folder after a backup).

Expected files: `src/graphics/src/web/{submit,seg,mod}.rs`, `src/graphics/src/lib.rs` (test include),
`src/web/gpu-worker.js`, this document, `docs/35-WEB-TARGET-PLAN.md` (W10c row).

## Results

### Built

1. **Depth/stencil pass segments** (as planned): `src/graphics/src/web/seg.rs` (`Seg`, `modes`, `mark_read_only`,
   `keep_stores`, `continuation`); `submit.rs` `Pass.ds` / `begin_at` / `payload` / `seg`, `end_segment`, `ds_use`
   (draws: the pipeline's writesDepth / writesStencil as WebGPU defines them, and the bound views of the attachment's
   image found while the bind groups are resolved; `vkCmdClearAttachments` of depth/stencil counts as a write);
   `gpu-worker.js` `PASS_BEGIN` mode 3 = `depthReadOnly` / `stencilReadOnly`. MK8D's Depth32Float error is gone.
2. **Next blocker found and fixed: movie decoder events refused on the web** (`src/os/src/services/movie.rs`
   `ev_ok`). An event entry had to be at or above 4 GiB; the web's compact map puts MK8D's heap at 0x20000000, so
   `RegisterClientEvents` recorded `0x0 0x0 0x0 0x0` and Start's event was never delivered (`event 0 not delivered`
   in W10b). The check is now "inside the guest's modules or ASLR region" (native: the same addresses as before; the
   native run registers 0x134ba03d0.., the web now 0x24ba03d0..). The decoder now gets its input buffers; it still
   decodes nothing on the web (H.264 through WebCodecs is not wired, docs/35, red line kept).
3. **Next blocker found and fixed: a pass sampling other mips of its colour attachment** (W10c run1/run2: `Texture
   (480x272 RG11B10Ufloat) usage (TextureBinding|RenderAttachment)`, 16,292 errors by 156 s, frames stuck on the
   logo). The diagnostic named it: MK8D's bloom chain renders mip k (1..4) while sampling a `TYPE_2D` view of mips
   0..5 of the same image (Vulkan allows reading the other mips; WebGPU refuses a bound view whose range includes an
   attached mip). `seg::unattached_mips` + `submit.rs` `unattached_view`: the sampled view is bound as a view of the
   same image without the attached mips (base kept, so the shader's mip numbering is unchanged: mips 0..k; when the
   base itself is attached, the mips above it), cached by (view, base, count), destroyed with its view;
   `obj::View.swizzle` kept for it. A view of only the attached mip (a real feedback loop) gets a red line.

### Tests

`cargo test --release -p graphics --lib web_seg` (host build of `web/seg.rs`, included from `lib.rs` for tests):
5 passed — `a_segment_that_only_samples_depth_declares_it_read_only`, `depth_and_stencil_are_decided_separately`,
`writing_then_sampling_splits_and_the_continuation_loads_everything`, `sampling_then_writing_splits`,
`a_downsample_chain_binds_the_mips_below_the_one_it_renders`. They are new (the code did not exist before). The movie
fix has no unit test (needs the os memory map); verified in the game.

### MK8D in Chrome (GPU 1), `build/WEB/W10c/mk8d`

- `run1` (items 1-2): 0 Depth32Float errors; the movie decoder gets its events and input; still on the logo (the
  RG11B10 errors). The Chrome GPU process went away at 108 s (no crash report, no TDR in the System log; cause not
  found), so later shots are missing.
- `run2` (diagnostic build): the colour-attachment line above; logo only (t60 = t90, t120 = t150).
- **`run3` (items 1-3): 0 WebGPU errors in 200 s; the title logo, then the title menu (Single Player / Multiplayer /
  Online Play / Wireless Play, Mario/Peach/Bowser art) from ~41 s (`t60`..`t180`), 59.5-59.7 FPS.** A presses every 4
  s are delivered (`buttons 0x0001`, focus 1) but the menu stays on Single Player through 180 s: the race is not
  reached. Red lines: only the movie decoder's WebCodecs one.

### Regression: Anime Girls (rebuilt with this branch, backup in `build/WEB/W10c/ag/backup-before`)

`build/WEB/W10c/ag/run1`: beach gameplay (Kayla on the pier, a lizard, HUD, 5 kills) at t130; 0 WebGPU errors;
51.9-53.3 FPS (W8e's beach ~52-55). Red lines `rgba8unorm ... reads depth`, `uniform binding of 4352 bytes`,
`A4B4G4R4_UNORM_PACK16` were already in W8e's runs.

### Unproven / next

- MK8D's menu does not act on A (Single Player stays highlighted): not diagnosed (deadline). Next: compare the native
  `mk8d` scenario's input timing (presses of 3 s cadence; held vs 80 ms taps) and log the game's npad reads on the
  web around a press.
- H.264 movie decoding on the web (WebCodecs): not wired; the title's movie surfaces stay empty.
- The `uniform bank c3 ... in no memory pool` red lines (W10b, 26 s) did not appear in run3.
- run1's Chrome GPU process exit at 108 s: not reproduced in run2/run3.

MK8D's previous web files: `build/WEB/W10c/mk8d/backup-before`. Build logs: `build/WEB/W10c/mk8d/build{1..4}.log`.
Driver: `build/WEB/W10c/tools/drive.mjs` (W8e's plus `--shots`, `--xfrom`, `--xevery`).
