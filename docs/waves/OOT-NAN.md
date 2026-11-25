# OOT-NAN — Ocarina's forest is white (NaN in the HDR forward pass)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md` (dist lock / machine lock; never two copies of one title). Runs
**alone** among engine waves (an independent read-only audit of OOT-RGB9E5's `storemu.rs` / `image.rs` / `exec.rs`
change may still be running; if the lead then fixes something there it will tell you). Title: "The Legend of Zelda
Ocarina of Time" (scenario `oot`, GPU 1); regression on Mario Kart World at the end (scenario `mkw`, GPU 0).

## Brief (lead)

**Problem (recorded, `docs/waves/OOT-RGB9E5.md` report).** With the E5B9G9R9 storage fix in, the 630 red lines are
gone (2 left, both known), but from the Kokiri Forest load every frame is white behind correct subtitles
(`build/play/20261125-140847-oot`). Pass dumps (`build/play/20261125-141630-oot-dump2`, `-143329-oot-dump3`,
`-144102-oot-dump4`): the forest's forward pass (pixel shader `9f36b8ee91e736c6`) writes NaN into 42-57% of the
HDR target; the volumetric-fog draw `888c1068ac8f7043` then makes 100% NaN, shown white. The 640x360 inputs are clean.

**First suspect (from that report, not chased):** about a dozen textures of unrelated sizes all report their
storage at `0x13dc17000`, the start of the physical pool that OOT-PIPE made GPU-backed (`vk/mem.rs`
`placement_for`). If textures in a physical pool resolve to the pool's base instead of base + offset, they alias
each other and inputs become garbage (NaN). OOT-PIPE's final run shows the same lines, so it predates the storage
fix. Check this first; then bisect the forward pass's draws and inputs (its 1280x720 and 3D-volume inputs) and
Maxwell's `0 * Inf = 0` (FMUL/FFMA `.FMZ`) rule in the shader translation (`dmsh_xlate` output; nxrs patches SPIR-V
after it in `vk/spvfix.rs`).

**Goal / done.** The cause found and fixed as a general correction; the `oot` run's forest shots show the scene,
no device loss, red lines unchanged or fewer. Then, if time allows, push the `oot` route to **Link moving under
input** (59 FPS target; report FPS vs 59, 5% low).

**Tests:** a unit test for the correction (e.g. a physical-pool placement at a non-zero offset resolves to base +
offset); `cargo test -p graphics --lib --release`. **Regression:** `py scripts/play.py mkw --env NXGFX_GPU=0` after
relinking it: race FPS (last 29.6), red lines (known: 1), sheet read.

**Memory/descriptor/coherence changes: the lead runs an independent audit.** Never skip draws; don't weaken
robustness (no NaN scrubbing that hides a real cause); native NDVE stays off. **Exclusions:** `dmsh_xlate` itself,
other titles' routes, no commits, don't edit docs/32. Back up and restore anything you move (pipeline caches,
saves), and check the restore direction.

**Deadline:** 4 hours wall clock. Report per protocol §5 in this file under "## Report".

## Report (agent, protocol §5; 2026-11-25 ~14:50-17:50, the only engine wave on the machine)

**Outcome.** Found and fixed. **The forest now renders**: the Deku Tree, the roots, moss and volumetric light shafts,
Navi, Kokiri village, Link's house, and Navi waking Link. There was no device loss, and the red lines are unchanged
(2, both known). The cause was not the physical pool and not FMZ. **NVN2 compute shaders read the dispatch's group
counts at c0 + 0x4, and nxrs wrote them only at c0 + 0x200** (NVN1's place), so those words were 0. The game's sky
irradiance compute `21d9f83e6027c565` divides by its sample count `(c0[1] << 5) * (c0[2] << 5)`. With 0 that is
`1 / 0 = Inf`, and every one of its spherical-harmonics sums came out as +Inf, -Inf or NaN. The forward pass reads
those sums (storage buffer s1, words 12-26) for distant and foliage pixels and wrote NaN there. The fog draw then
spread the NaN over the whole frame, which showed as white. See Verified for how far the `oot` route now goes.

1. **Built.**
   - **Diagnosis** (steps, kept apart from the correction):
     - The 0x13dc17000 suspect is not a placement bug. `nvnTextureInitialize` computes `pool.mem + off`, and the
       alias pool's base is `ph.mem + poff - voff`. The dump shows the forest's render targets at distinct addresses
       across the 305 MB transient heap (0x13eb17200, 0x13fa17400, ...). The textures at the pool start are the
       game's own transient aliasing at offset 0 (several depth targets of different passes).
     - **NaN trap** (run `build/play/20261125-145643-oot-nantrap`, flags in its `nan-flags.txt`). The forward pixel
       shader `9f36b8ee91e736c6.spv` was swapped in place (backup, restored; see Slips) for a scratch-instrumented copy
       that writes, instead of its colour, bit flags for which of its inputs held NaN or Inf: interpolants,
       FragCoord, each texture group, the bindless 2D and 2D-array samples, storage buffers s0-s3, uniform banks, and
       the final output. Read-back of the forward pass: 78,765 sampled pixels flagged **`s1, final`** and nothing else.
       Textures, interpolants and uniforms were all clean. s1 is read at fixed words 12-14, 16-18, 20-22, 24-26 and 35.
     - **s1's contents** (run `-151304-oot-sbuf`, with a new dump-only log of storage buffers, see below). s1 is c0
       word 332 at 0x25a757f00 (+0xc0). Words 12-14 are +Inf, 16-18 NaN, 20-22 -Inf and 24-26 NaN, and the shader
       reads exactly these words. It is written by computes `21d9f83e6027c565` (pass 4) and `1bba09ec4bc8ff67`
       (words 32-35 only).
     - **Its inputs are clean** (run `-152125-oot-sky`). `21d9` sums samples of the 256x256 RGBA16F sky (ser5291,
       drawn in pass 2 from the 3D scattering LUT). The raw read-back of that target and the LUT's first slice have
       no NaN or Inf (max 104.5). Finite inputs cannot sum to Inf, so the compute's own math made it. Its SPIR-V does
       Fibonacci-sphere sampling: N = `(c0[0].y << 5) * (c0[0].z << 5)`, z = 1 - (2i + 1) / N, then atomic float
       adds. nxnative's `dm/gfx/nvn/dmsh_abi.h`: `DMSH_C0_DISPATCH 0x200` and `DMSH_C0_DISPATCH_NVN2 4` ("NVN2 compute
       keeps them here, below its per-shader layout at 0x10"; nvn_mtl.m writes both, the NVN2 copy for NVN2 SDKs
       only). `docs/INTERFACES.md` already says "compute's dispatch size at 0x200 (NVN2 0x4)", but `c0_bytes` wrote
       only 0x200.
   - **Correction** (`src/graphics/src/vk/exec.rs`): `c0_bytes` now writes the dispatch size at 0x200 and, for a
     Switch 2 game (`nvn::device::SWITCH2`, as nxnative gates on its NVN2 SDK), also at `C0_DISPATCH_NVN2` = 0x4
     (`put_dispatch`). `reads_draw_words` counts a compute storage record over 0x4..0x10 as reading the dispatch
     words, so a descriptor set keyed without them cannot be reused across dispatch sizes. (A set that binds c0
     itself already counted.) This is general: every NVN2 compute shader that reads `gl_NumWorkGroups` got 0 before
     (MKW is NVN2 too).
   - **Diagnostic kept** (exec.rs, pass dumps only): each draw's storage buffers are logged in the dump lines as
     `s<slot>:<off>@<addr>+<size> w<written> [first 40 words as floats]`, like the c-banks already are.
   - **Folded in from the lead's audit of OOT-RGB9E5** (the lead handed me these files):
     - **M1** (image.rs `reg_tex_now` and the re-backing loop near `texture_slots()`): an image whose format has
       storage only through an emulated view gets **no pool storage view** (None, as before OOT-RGB9E5), because a
       shader indexing the pool's storage slots gets no packing variant. No OoT shader indexes a storage-image pool
       (checked over all 42,848 reflections), so this was not the NaN.
     - **M2** (storemu.rs `emulation`): the requested format decides. A table format requested packs into its twin
       view (any image), and the game's `r32ui` view of an emulated image is `Bits32`.
     - **M3** (image.rs `emulated_storage` + `emulated_storage_creatable`): `vkGetPhysicalDeviceImageFormatProperties`
       is asked once per format for 2D + STORAGE with `EXTENDED_USAGE | MUTABLE_FORMAT`. If refused, there is no
       emulation (red once) and the image is made without storage as before. `sparse::create` already falls back to
       a whole image when its sparse query finds no blocks; I did not change it.
     - **L3** (storemu.rs): global OpUndef / OpConstant / OpConstantComposite / OpConstantNull results now have their
       types recorded, so a constant texel no longer refuses the rewrite.
     - **L5** (exec.rs `prepare_storage_bindings`): formatted bindings skip the emulated-view lookup.
     - **L7**: `NXGFX_STOREMU` and `NXGFX_STOREMU_IN` rows added to `docs/env_switches.tsv` by hand (the generator
       would have rewritten other waves' rows; its `--check` now reports only the 16 old "no meaning" rows).
     - **L1** (note_storage_write before a pipeline that may fail): not done.
   - `scripts/scenarios.toml` `[oot]`: the route goes on past the Deku Tree into Link's house (see Verified). Its
     comment is updated below with what is known now.
2. **Tests** (`cargo test -p graphics --lib --release`: **209 passed, 0 failed**, 6 ignored; run after the final
   source; `py -m unittest scripts.test_play`: OK).
   - `vk::exec::set_patch_tests::nvn2_compute_reads_the_dispatch_size_at_0x4`: with NVN2, words 1-3 hold the
     dispatch and 0x200 still does; word 0 and 0x10 are untouched. Without NVN2, words 1-3 stay 0. A compute set that
     binds c0 reads the dispatch words. The helper is new, so the test could not fail before; the before/after
     evidence is the game runs.
   - `storemu` `writes_pack_and_validate` gained the M2 case (an `R32_UINT` image asked as E5B9G9R9 through its uint
     view packs `Rgb9e5`). It would have failed before, when `emulation` keyed on the image format and gave None.
3. **Verified** (OoT on GPU 1, MKW on GPU 0, one game at a time; engine rebuilt with `build.ps1 -KeepCache -SkipGnu`
   and the titles relinked under `build/locks/dist.lock`, released after each).
   - **`build/play/20261125-153150-oot-nan-fix`** (`play.py oot`, the brief's 470 s route on the fixed build): 139/139
     steps, normal exit. Window [100, 300] (the opening's cutscenes, which the game runs at 30): **31.1 FPS mean, 5%
     low 29.5, min 28.7 (target 59)**. **Red lines: 2** (the known `no render target bound` and the old
     `pipeline-crashed.bin` skip), the same as before. No device loss. **Sheet:** title, story, nightmare correct;
     04-house is the opening's white movie frame (as in OOT-RGB9E5's run); 05-forest_load black (the load). **06
     Deku Tree, 07 Navi on the moss, 08 the Deku Tree's roots and moss with light shafts all render**, with the
     subtitles. Before (`-140847-oot`): white from 06 on.
   - **The extended route** (the current `[oot]` entry, explored over five runs; superseded ones deleted).
     **`build/play/20261125-170343-oot-route6`** (1235 s, 454/454 steps, normal exit, 2 red lines, no device loss): after
     the Deku Tree talk ("Understood. I'll go get him!"), Navi flies over the Kokiri village (houses, fences, stairs)
     and into Link's house at ~640 s. She wakes him ("Link! Hey, Link, wake up!", the "Navi, Forest Fairy" card at
     ~1020 s). He dresses (hat, boots, belt), and at ~1232 s the camera is behind him, standing in his house. The final
     run, **`build/play/20261125-172447-oot-route7`** (1287 s, 491/491, normal exit, 2 red lines, window [100, 300] **31.1
     FPS mean, 5% low 29.5, min 28.7**), was slower through the dialogue: it is still on "So, come on. Let's go
     together!" and the boots at 1284 s. Every shot of both runs renders: interiors, characters, Navi's glow and the
     light shafts. One earlier exploration run had a 1.7 FPS minimum in the window: frames of 100-775 ms at 256-258 s
     in the opening, with GPU ~10 ms and replay under 1 ms, so the game side was slow. Not seen in the other runs;
     not chased.
   - **Regression, Mario Kart World** (`build/play/20261125-154021-mkw-nan`, relinked on the final engine): 43/43,
     normal exit, race window [250, 325] **29.2 FPS mean, 5% low 25.5, min 25.0** (last 29.6), **1 red line** (the
     known `pipeline-crashed.bin` skip). **Sheet:** boot drive, title train, menus, grid, race and later race correct.
     MKW is an NVN2 game, so its compute shaders now see their dispatch size too; nothing visible changed.
   - Diagnosis runs kept: `-145643-oot-nantrap` (flags), `-151304-oot-sbuf` (s1 values and writers),
     `-152125-oot-sky` (sky target clean).
4. **Unproven / next.**
   - **Link moving under input is not reached yet** (no FPS vs 59 for gameplay). The waking and dressing cutscene
     runs to ~1230-1300 s with A every 3 s, and its pace varies by a minute between runs. Next step: shorten the
     route (A every ~1.5 s through the house dialogue), then put the walk and the window right after "Let's go
     together!".
   - **The forest looks very misty and bright** (strong haze over the village). It may be the game's morning mist; I
     had no reference picture to compare. If it is too strong, the next suspects are other NVN2 compute inputs from
     c0: **indirect dispatches** still get the CPU-side `d.x/y/z` (probably 0) in c0 at both 0x200 and 0x4. nxnative
     copies the indirect record into c0 on the GPU (`nvn_mtl.m` 16483). That is a separate change, also for NVN1.
   - Forest performance: frames of 70-380 ms (25-35 ms of pool preparation and ~56 MB uploaded per frame), and a
     ~20 s stretch without presents at the transition into Link's house (~617-637 s). The window measured is still
     the 30 FPS opening, not gameplay.
   - The NaN trap's scratch tool (`nantrap.py`, scratchpad) worked well. A small checked-in diagnostic (an env-keyed
     variant that ORs NaN flags into the colour) would make the next NaN hunt one run.
5. **Slips.**
   - **Swapped game file, restored:** `...\The Legend of Zelda Ocarina of Time_data\shaders\spv\9f36b8ee91e736c6.spv`
     was backed up to the scratchpad first, replaced by the trap for one run, and copied back from the backup.
     SHA-256 **CFC33D8BC004CB6395675751FDF457E809E57631E3570734D15ADF6F7AF1DEFC** before, after the restore, and
     again before the final runs. Nothing else in the game's folders was moved.
   - The first relink after my first build failed ("_code.dll is in use", the trap run's process was still exiting),
     and `nxrs.exe` returned 0 anyway, so the next diagnosis run (`-150513-oot-sbuf`, deleted) used the old build.
     I relinked again and checked the build log's `@result ok` and the DLL time after every later relink.
   - The trap was a scratch Python rewrite of the game's SPIR-V *text* (in the scratchpad, assembled with spirv-as).
     No repository file was written by a script. Superseded runs (`-150513-oot-sbuf`, route explorations 1-5) and
     the 2.5 GB of raw dumps were deleted; the flag summary is kept in the trap run.
   - `scripts/scenarios.toml` `[oot]` is now a 1360 s route (it was 470 s). Its first 470 s are unchanged, so the
     brief's forest check is the same; the lead may want it shorter once gameplay is reached.
   - The time ran to ~17:50, inside the 4 h.
   - Saves: harness copies only (no `--real-save`).
6. **Questions.** None.
