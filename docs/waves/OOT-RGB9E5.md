# OOT-RGB9E5 — storage writes to E5B9G9R9 images (Ocarina's black/white forest)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md` (dist lock / machine lock conventions; never two copies of one title).
Runs **alone** (an independent read-only audit of OOT-PIPE's `vk/mem.rs` / `vk/lazy.rs` change may still be running;
if the lead then fixes something there it will tell you). Title: "The Legend of Zelda Ocarina of Time" (scenario
`oot`); regression check on one other title at the end (see below).

## Brief (lead)

**Problem (recorded, `docs/waves/OOT-PIPE.md` report, `build/play/20261125-131719-oot-final`).** From the Kokiri
Forest load (~340 s) every frame is plain black or white behind correct subtitles. 630 red lines "storage image ...
zero storage stand-in": the game writes 2560x1440 **E5B9G9R9** images as storage images, and the RTX 5090 (like most
GPUs) does not support that format for storage, so those writes go to a zero stand-in. Title, story, nightmare and
Link's house render correctly. The SPIR-V comes from the external `dmsh_xlate` translator; nxrs already patches
SPIR-V after it (`src/graphics/src/vk/spvfix.rs`, `xfb.rs`).

**Goal / done.** Storage access to formats the device cannot store (E5B9G9R9 first; design it general — a table of
emulated storage formats) works: bind an R32_UINT storage view of the same image memory (image created with
`MUTABLE_FORMAT` + the uint view format in its format list, or an alias), and rewrite the shader's `OpImageWrite` /
`OpImageRead` on such bindings to pack/unpack RGB9E5 in SPIR-V (exact shared-exponent encoding per the Vulkan spec
"Shared Exponent" section, with its clamping/rounding). Sampling the image keeps the native E5B9G9R9 view. Done = the
`oot` run's forest shots show the scene (not black/white), the 630 lines gone, no device loss; then push the `oot`
route on to **Link moving under input** if time allows (59 FPS target).

**Tests:** unit tests for the pack/unpack against reference values (zero, max, denormal-ish, negative clamp, NaN/Inf
handling per spec) and for the SPIR-V rewrite (`spirv-val` passes on the patched module); `cargo test -p graphics
--lib --release`. **Regression:** the patch only applies to bindings whose format needs emulation; show one other
title unchanged (e.g. `wonder` or `violet` one run, sheet read, red lines unchanged).

**This is a descriptor/format change: the lead runs an independent audit.** Never skip draws or dispatches; don't
weaken robustness; native NDVE stays off. **Exclusions:** `dmsh_xlate` itself, no commits, don't edit docs/32.

**Note:** OOT-PIPE lost the title's original `pipeline-cache.bin` (a cache; the live one is regenerated). Back up and
restore anything you move, and check the restore direction.

**Deadline:** 4 hours wall clock. Report per protocol §5 in this file under "## Report".

## Report (agent, protocol §5; 2026-11-25 ~13:10-15:30, the wave alone on the machine)

**Outcome.** E5B9G9R9 storage now works. The 630 "zero storage stand-in" red lines are gone (2 red lines in the
whole run, both known kinds), and the game's lighting words reach the E5B9G9R9 images. There was no device loss. **The
forest is still not visible in the final picture.** It is white, as before. A pass dump shows the forest scene drawn
into the HDR target (pass 94: the Deku Tree's roots, the sky, the trees). But raw read-backs show **NaN**. The
forward pass that draws the forest writes NaN into 42-57% of the HDR target's pixels. The full-screen
volumetric-fog draw then makes all of them NaN, and the tone-mapped picture is white. That is a separate defect,
described under Unproven. Link moving under input was not reached.

1. **Built.**
   - **Diagnosis.** The E5B9G9R9 textures (NVN 0x40, IDs 294-304, 2560x1440) are written by compute shader
     `a335c1db6270c355`. Its SPIR-V packs RGB9E5 itself (`<<27`, `*262144`, ...). It then stores the word as the
     bits of a float in R of a formatless `OpImageWrite`, a formatted SUST with only R set. Ryujinx's `EmitSust`
     declares a formatted store `Unknown` when the translator has no GPU state. The game registers the image with
     an **R32_UINT view** (`[vk] texture ID 294 ...: storage of a E5B9G9R9_UFLOAT_PACK32 image asked as R32_UINT`).
     The image was made without STORAGE use, because its own format has none here, so the view could not be made.
     A first version that packed the float texel as RGB9E5 double-packed the game's word, and the forest went
     **black** (run deleted). The format the game asks for decides what the texel means.
   - **`src/graphics/src/vk/storemu.rs` (new): a table of emulated storage formats** (E5B9G9R9 -> R32_UINT view,
     conversion `Rgb9e5`). `emulation(image, requested, view)` gives `Rgb9e5` when the game asked for the image's
     own format (the texel is a colour). It gives `Bits32` when the game asked for `r32ui` (the texel's R holds the
     word). `rewrite(words, set, binds)` retypes each named formatless float storage-image variable to `r32ui`, and
     retypes its loads. Each `OpImageWrite` on it stores `uvec4(word)`: the RGB9E5 pack, or `bitcast(texel.x)`.
     Each `OpImageRead` reads `uvec4` and gives back floats: the unpack, `(r,g,b,1)`, or `(bits,0,0,1)`. The new
     types are deduplicated, and the variables are moved after the new pointer types. Any other use of the image
     (copies, calls, phis, texel pointers) means no variant. The pack follows the spec's shared-exponent formula
     exactly, in integers (Mesa's method). NaN and negative values clamp to 0, +Inf to 65408.
     `pack_rgb9e5` / `unpack_rgb9e5` are CPU references.
   - **`image.rs`.** An E5B9G9R9 texture with the IMAGE flag, or used as an image without it, gets STORAGE use
     through `EXTENDED_USAGE` (`Gpu::emulated_storage`, `NXGFX_STOREMU=0` off). Its storage views take the
     table's format when the requested one has no storage (`image_view_kind`, next to the sRGB UNORM twin). The
     render-scale re-creation keeps `EXTENDED_USAGE` for such images. `Image::format_of_view` is new. Sampled
     views keep E5B9G9R9.
   - **`shader.rs`.** `Library::with_storage_emulation` is variant kind 5, keyed on (binding, conversion) pairs. It
     says in vklog once per variant, or in red once per shader when no variant can be made.
   - **`exec.rs`.** `prepare_storage_bindings` now returns, per stage, the formatless (`fmt == 0`) storage images
     bound to an emulated view. `emulate_storage` swaps those stages for their variants: in draws after the
     dual-source/alpha variants, before the pipeline key; in dispatches before `compute_pipeline`, which now comes
     after the storage preparation. The unpatched pipeline is never made for such a dispatch. Nothing is skipped.
     A shader whose variant fails keeps its module (red line).
2. **Tests** (`cargo test -p graphics --lib --release`: 208 passed, 0 failed, run after the final source).
   - `rgb9e5_reference_values`: zero, 1.0, the maximum, clamping of 1e9/+Inf/70000, negatives, NaN as 0, the
     smallest step 2^-24, half of it rounding up, a float denormal, and `max_s` rounding to 512 moving the exponent.
     It also checks the unpack of known words, 20,000 inputs (random bits, NaN, Inf, denormals, half steps) against
     an independent f64 spec formula, and the round trip of representable values.
   - `writes_pack_and_validate` / `reads_unpack_and_validate` / `bits_mode_writes_and_reads_the_word`: a hand-built
     compute module (store, read, size query) is rewritten and passes `spirv-val --target-env vulkan1.1` (the
     Vulkan SDK's, found through `VULKAN_SDK` or `NXGFX_SPIRV_VAL`). The emitted code is run by a small evaluator.
     The packed word equals `pack_rgb9e5` for ~2,900 inputs, the read gives `unpack_rgb9e5` (+1.0 alpha), and
     `Bits32` stores and reads the exact word.
   - `only_the_named_plain_bindings_are_rewritten`: another binding or set, an empty list, and an image used by
     `OpCopyObject` give no variant, and the other binding keeps its float type.
   - `storemu_folder` (opt-in, `NXGFX_STOREMU_IN=<shaders/spv>`): every formatless float storage image of OoT's
     42,848 translated shaders is emulated. **317 variants pass spirv-val, 0 refused.**
   - These are new functions, so the tests could not fail before. The game runs are the before/after evidence.
3. **Verified** (OoT on GPU 1, MKW on GPU 0; one game at a time; engine rebuilt with `build.ps1 -KeepCache
   -SkipGnu` and OoT relinked under `build/locks/dist.lock` each time, released after).
   - **`build/play/20261125-140847-oot`** (`play.py oot`, Bits32 build, same code as the final apart from rustfmt
     and log wording): completed 139/139, normal exit, 441 s. **Red lines: 2** (before: 632): one known `a draw
     with no render target bound`, and one `pipeline-crashed.bin` skip (the old 11-23 entry OOT-PIPE named). No
     device loss. The vklog shows IDs 294/296/298/300/302/304 bound as `Bits32` and `a335c1db6270c355` packed.
     Window [100, 300] (the opening's cutscenes, which run at 30): **30.9 FPS mean, 5% low 29.5, min 28.7 (target
     59)**. Worst pacing 6.35 s at 341.6 s (the forest load, as before). **Sheet:** title, story tapestry and
     nightmare correct. 04-house is a white frame of the opening movie: the movie is ~5 s ahead of OOT-PIPE's
     run, as the earlier present count shows, and the shot comes before any emulation (first at 341 s). The forest
     load is black, then Deku Tree, Navi and end are **white** behind correct subtitles.
   - **Pass dump** `build/play/20261125-141630-oot-dump2` (`--seconds 395`, `NXGFX_DUMP_AT_S=380`; key passes in
     its `passdump/`: `strip.png`, `means.txt`): pass 94 (HDR B10G11R11 2560x1440) shows the forest scene.
     Pass 107 is a full-screen draw: pixel shader `888c1068ac8f7043` over three 128x72x96 froxel volumes
     (ser5553/5557 B10G11R11, ser5561 R16F, written by compute `5b2bb0924ecc403b` from `9ed48d1011f1b48b`'s
     scattering), blending into the same target. In the PNGs it turns the whole frame white, and every later pass
     and the final picture stay white (the raw read-back below shows it is NaN). The composite that samples the E5B9G9R9 lighting (`8df148d58b7cbe8e`) is near the
     frame's end.
   - **Raw read-backs** (`NXGFX_DUMP_RAW=1`, decoded by a scratch script).
     `build/play/20261125-143329-oot-dump3` (2560x1440 passes; `hdr-stats.txt`, `nan-mask-pass91.png`): the HDR
     target ser28968 is 0 through pass 39. Pass 71 is finite (median 3.1, max 17.2, no NaN). **Pass 91, the first
     forward pass over it (mostly pixel shader `9f36b8ee91e736c6`, BC7 materials, depth ser28969), leaves 42% of
     the sampled pixels NaN.** The NaN mask is the background geometry (foliage/terrain), not the roots. It rises to
     57-69% by pass 104. **Pass 105 (the fog draw `888c1068ac8f7043`) makes 100% NaN**, which reads as white. That
     is the "saturation" the PNGs showed, since NaN converts to white.
     `build/play/20261125-144102-oot-dump4` (`nan-640x360.txt`): the 640x360 inputs (AO/GI-like R32F, R16G16F,
     RGBA32F, B10G11R11) have **no NaN** up to the forward pass. Pass 119's 640x360 copy is all NaN, downstream.
     The E5B9G9R9 lighting can't hold NaN: its decode is always finite, and its composite runs at the frame's end.
   - **Regression, Mario Kart World** (lead's request; `build/play/20261125-142730-mkw`, `play.py mkw --env
     NXGFX_GPU=0`, relinked on the final build): completed 43/43, normal exit. Race window [250, 325]: **29.6 FPS
     mean, 5% low 27.0, min 24.0** (last: 29.1). **1 red line** (the known `pipeline-crashed.bin` skip). **Sheet:**
     boot drive, title train, Mario Bros. Circuit menus and grid, race and later race all rendered correctly. MKW
     has no E5B9G9R9 storage, so it does not touch the new path. The build also carries the lead's `mem.rs` sparse
     translation and OOT-PIPE's physical-pool change, and shows no regression from them.
   - **Blacklist experiment:** with `pipeline-crashed.bin` moved aside, the skipped pipeline compiled without a
     compiler crash (no new entry, normal exit), and the forest stayed black (that run was in Rgb9e5 mode, deleted).
     The file is restored byte-identical (SHA-256 46B49CEF...). The lead may drop that entry, which has now been
     shown safe twice.
4. **Unproven / next.**
   - **The forest picture: NaN from the forward pass.** Pass 91's draws (`9f36b8ee91e736c6` and six others)
     write NaN over the background geometry, and the fog draw spreads it everywhere. Next step: find the input or
     operation that makes it. The 640x360 inputs are clean; still unchecked are its 1280x720 inputs, the 3D volumes
     (ser5485 64x64x64, ser5481 512x512x64 0x3d, the froxels written by compute `9ed48d1011f1b48b` /
     `5b2bb0924ecc403b`) and the cube ser5264. Bisect the pass's draws, then check the shader for an FMZ
     (0 x Inf = 0 on Maxwell) or rsqrt(0)/normalize(0) the translation leaves as NaN. Then route on to Link moving.
   - **Observation for the lead (not chased):** a dozen textures of unrelated sizes (depth 256x144 and 2560x1440,
     RGBA8 1920x1080, ...) all report their storage at **0x13dc17000**, the start of the physical pool OOT-PIPE made
     GPU-backed (`CPU write kept out of GPU-written texture ... at 0x13dc17000`). The same lines are in OOT-PIPE's
     final run, so it is not from this wave. If those are virtual-pool textures resolved to the physical pool's
     *base* rather than base + offset, they alias each other.
   - An emulated image read by a formatless `OpImageRead` and a `Rgb9e5` store are tested but not met in OoT (all
     its uses so far are `Bits32` stores).
   - A formatless float shader bound to a game-registered `r32ui` view of an image whose own format *can* be stored
     still writes floats into a uint view (undefined in Vulkan). `storemu` only covers images of table formats. A
     wider `Bits32` (any formatless store through an integer view) would be a separate, general change.
5. **Slips.** A first in-game version double-packed (fixed the same session). I ran `rustfmt` on my new file only.
   Three superseded OoT runs and the first pass dump were deleted. The raw read-backs were deleted after decoding
   (their stats and the NaN mask are kept in the run folders). Diagnosis scripts lived in the scratchpad and wrote
   no repository files. Everything was relinked under the dist lock (OoT and MKW on the final build). Saves: harness
   copies only. Time: ~13:10-15:30, inside the 4 h. One slip: I changed the report header's time with a `sed`
   one-liner instead of the editor tool (a single word in this file, nothing else).
6. **Questions.** None.
