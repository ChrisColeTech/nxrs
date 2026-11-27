# MKW-RENDER — missing textures, texture pop-in and the character-select screen

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~09:45, worktree
`D:\Projects\RecompLauncherEngine-mkwrender`, branch `mkwrender` (base `gameplay-harness` `dd9baed`: MKW-HEAVY merged).
Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0. Deadline 4 h (~13:45).

## Starting point

The owner, playing MKW on the merged build, reports (1) missing textures, (2) severe texture pop-in, (3) the
character-select screen not displaying properly. Known from every sheet since MKW-SYNC: the roadside shrubs on the desert
slope render as near-black silhouettes. Earlier waves' scenarios took no shot on the character screen and only
`mkw-turn` took shots closer than ~40 s apart, so their "sheets correct" says nothing about these three bugs.

## Plan (written before code)

1. **Make the bugs visible** with the existing harness (no new harness options; lead's instruction):
   `scripts/scenarios.toml` — `mkw` shoots every menu screen (the A presses split, the 6 s rhythm kept, so its
   route and window do not move); new `mkw-pop`: the same menus, then a shot about every 0.5 s for 40 s of the race,
   run with `--faults` and `NXGFX_TEX_TRACE=1`. Route checks for the new shots once a run's sheets are read.
2. **Baseline** on the base build (`dd9baed`, the library's DLL): `mkw-pop --faults` and `mkw`; read every sheet; the
   report's "Near each screenshot" (stand-ins etc.) and the tex trace tie a picture fault to textures.
3. **Causes**, each traced to source: pop-in (a texture sampled before its contents land: stand-in, upload of storage
   the game has not finished writing, the upload skip / chunk tracking missing a write, the ASTC cache), the black
   shrubs (format, swizzle, alpha, sampler, missing upload), the character screen (what is wrong in its shot, then the
   pass that draws it).
4. **Fix at source**, behind a switch where a fix changes a default path (`docs/env_switches.tsv`);
   `NXGFX_USE_SYNC=verify` over a race shows no missed pages.
5. **Verify**: the new shots before and after side by side here; route checks; `mkw` FPS against the baseline;
   `cargo test --release` for touched crates.

Expected files: `scripts/scenarios.toml`, `scripts/checkpoints.json`, `src/graphics/src/vk/image.rs` and what the
causes lead to, `docs/env_switches.tsv`, this doc. What would disprove the pop-in hypothesis: the dense shots show the
late textures with no stand-in, no late upload and no CPU write after the upload (then it is level-of-detail streaming
the game does itself and its timing).

## 1. Making the bugs visible (scenarios)

The first `mkw-pop` run on the base build (`build/play/20261127-094847-rd-base-pop`) showed that the old menu timing no
longer holds: at today's ~50 FPS the opening ends well before 138 s, and the 16 A presses meant for the opening already
walk through Single Player, Grand Prix, the character select (~70-90 s), the kart select, the cup and 50cc; the race
starts at 135-166 s (it varies by run). So `mkw` and `mkw-pop` now shoot each of those 16 screens (`o1`..`o16`, 5.5 s
after each press, just before the next; the 6 s rhythm and the route are unchanged). `mkw-pop` then shoots every ~0.5 s
for 40 s of the race (`pop`, 80 shots, 238-280 s). Which `o` shot is the character select moves with the run's speed
(`o5` in the after runs); the route checks stay on the HUD shots (grid, race).

## 2. Missing textures and the character screen: released texture objects bound the zero stand-in

**Evidence (base build).** `rd-base-pop` logged **6,653 zero stand-ins**, all but one of one kind: `its slot names
texture serN, which has no image now (released, its image parked)`, 216-488 a second at 90-103 s (the character and
kart select) and 121-275 a second at 161-167 s (the race intro), then none logged. The engine logs a stand-in per texture
ID at most once per 600 frames and **at most 4,096 IDs**, so after ~167 s the race's stand-ins were still bound but no
longer logged. The tex trace of one (`ser35026`, character portrait): registered lazily at IDs 30438 and 30786 at
frame 0; **finalized** by the game at frame 201 (its registration kept, `lazy::finalized`); its first use at frame
2462 makes the image (`taken at ID 30786 for a bound use (finalized true)`), which is released after that submission
(`release_after_use`, parked); from frame 2463 on, every use of the slot finds no image and binds the stand-in. The
fault capture `rd-base-pop/faults/fault-08-p2464.png` is the character select at that moment: "Mario", "Sort" and the
page dots over the blurred course, **no portrait grid, no character model, an empty selection bracket**.

**Cause.** On the console a texture descriptor outlives its texture object: a slot registered with an object the game
finalized still samples the object's storage. `release_after_use` (and `tex_release`) keep the image parked and the pool
slot's descriptor until eviction, so pool-indexing shaders kept working; a **bound** use goes through
`exec::tex_view_uncached`, which looks the slot's serial up among the live images, finds none and binds the stand-in.
The registration (`lazy::take`) was used up by the first use, so nothing could make the image again.

**Fix (`NXGFX_RELEASED_SLOT_USE`, default on).** `Gpu::remake_released` (`image.rs`), called from `tex_view_uncached`
when the slot's object has no image: when the slot still names that very object and its release kept its description
(`Images::released`), the image is made again from it (`reg_tex_now`; `ensure_image` takes the parked image back and
uploads again only pages that changed) and released after this submission, as the first use was. The rule is the pure
function `released_slot_remake` (tested). Counted in the `uploads from storage` line.

**Result.** `rd-v1-pop` (`20261127-100011`): stand-ins 6,653 → **8** (the known `texture ID 0 ... nothing is registered
there` at boot and a few in the race window; see §5); ~2,500-3,000 remakes per stats line (~5 a frame) in the menus and
the race. Pictures: the character select (`rd-v1-pop/06-o5.png`) shows the full portrait grid (Mario, Luigi, Peach,
Pauline, Yoshi, Toad, Koopa, Baby Mario, Wario, Waluigi, Bowser, Baby Luigi), Mario's 3D model, the name banner's
tire-track backdrop and the selection bracket on Mario; the kart select (`07-o6.png`) its kart grid and Mario in the
Standard Kart; the race HUD's lap counter has its flag icon (`1/3`; base: `1 3` with a gap).

**Second step (v1 → v3).** `rd-v1-pop` still had 7 such stand-ins, each right after `made again ... image ok` followed by
`vk replay released` in the same frame: a make outside a submission's recording (record-time translation, MKW-RECORD)
hands `release_after_use` to `defer_after_use`, which with nothing being recorded runs it after the last fence
already submitted (often done): the image went before the draws that wanted it. (The same holds for the first use's
make in the old path.) Now a remade image is kept and released by a sweep at the present `remade::KEEP` (60) frames after
its make (`Gpu::sweep_remade`, `queue.rs` present path); a later use takes the parked image back. And each
`release_after_use` carries a generation (`release_gen`): a release queued after an earlier submission's use no longer
releases an image a later submission made again. `rd-v3-pop` (`20261127-104040`): **stand-ins 1 in the whole run** (the
known boot one, `texture ID 0 ... nothing is registered there`); remakes ~50 per stats line (5 textures, every 60
frames) instead of ~3,000; the gate's give-ups and re-translations **4,234-4,297 → 157-172** a run (parallel lanes no
longer meet missing images and give up).

| | base `rd-base-pop` | `rd-v1-pop` | `rd-v3-pop` |
|---|---|---|---|
| zero stand-ins (logged; capped at 4,096 IDs) | 6,653 | 8 | 1 |
| gate (give-ups + re-translations, run) | 4,234 | 4,296 | 157 |
| character select | `faults/fault-08-p2464.png`: no portraits, no model | `06-o5.png`: correct | `07-o6.png`: correct |
| race HUD lap counter | `1 3` | `1/3` with its flag icon | same |

## 3. Pop-in

What the 0.5 s shots and the logs show:

- **Stand-ins were the pop-in the engine caused.** A finalized texture's slot showed its texture for one frame (the
  first use's image) and the zero stand-in after it, and stand-ins are fault-capture triggers, not anything the old
  sheets showed. 6,653 logged in the base run (and, past the log's 4,096-ID cap, more in the race that were not
  logged): character portraits, kart icons, name banners, the race intro's and HUD's icons. Gone (§2).
- **The upload skip and chunk tracking miss no CPU write.** `20261127-102015-rd-verify` (`mkw`,
  `NXGFX_USE_SYNC=verify`, boot to lap 2, 341 s, route 3/3 on): **0 written pages missed in closed chunks** at every
  report.
- **The ASTC path is not late.** `astc::record` decodes (or takes the BC7 disk cache's blocks) and copies into the image
  in the same submission as the first use; nothing provisional is shown first. Big uploads (8192² levels, 20-250 ms,
  MKW-SPIKE) make a slow frame, not a late texture.
- **Sparse terrain arrays** (8192x8192x68 BC7 / BC6H, Vulkan sparse images, `sparse.rs`) are bound as the game maps
  pages; the game streams their tiles itself, so a coarse-to-fine change there is the game's level-of-detail streaming
  unless a tile it wrote is missing. Not separately checked (no tile readback tool).
- In the 78-80 race shots of `rd-v3-pop` (238-302 s) no texture appears late or changes from shot to shot beyond the
  camera's movement; the sand's streaks at 290-302 s are the game's motion blur at speed (they follow the kart's
  speed, the coins and HUD are sharp). Still shots 0.5 s apart cannot show a one-frame pop or a sub-0.5 s swap.

## 4. The black shrubs (not fixed)

Still near-black in every run of this wave (e.g. `rd-v3-pop/28-pop.png`, `29-pop.png`, `36-pop.png`, `59-pop.png`).
What the pass dump of `20261127-101211-rd-dump` (frame 13044, 271 s; files in the scratchpad, not kept) shows:

- MKW draws the course forward: a depth prepass (passes 68-102, colour mask 0, depth LESS), then the colour passes
  (129-160, depth test **EQUAL**, no depth write), each with its own vertex programs.
- The shrubs are drawn by a single colour draw (pass 146: vertex `def3f05b1930ac56`, pixel `cb0bfd018d46e25c`) and their
  pixels **are written**: (16-24, 16, 8) of 255 in the scene target where the sand around them is above 1.0. Brightened
  4x the crop shows the full leaf texture in brown: the albedo arrives, the lighting comes out ~0.
- Not shadow: the screen-space shadow mask (pass 125, target 0) is white (lit) over the shrubs.
- Tried and taken out: **NoContraction** on all pre-raster float arithmetic (the hypothesis: the EQUAL test failing
  between two programs' positions). Built, run (`20261127-103033-rd-v2-pop`): shrubs unchanged; the dump then showed the
  shrub pixels are written, so the depth test is not what darkens them. The change moved nothing and was removed
  (§4 rule). Ryujinx's output already marks the position `Invariant`.
- Not the 18 blacklisted draws a frame either (§5): with them compiled the shrubs are the same.
- Left: the pixel shader's inputs. Its textures: three sparse 8192x8192x68 arrays (BC7_SRGB ser33391, **BC6H_UFLOAT**
  ser33393, BC7_SRGB ser33395), small per-object textures (275x275 BC7, 165x165 RGBA8 viewed (R,0,1,1), 138x138
  SRGB), probe arrays (128x128x12 cube array from passes 44-66), a 256x256 RGBA16F LUT (pass 9). A BC6H lighting atlas
  whose tiles are zero, or a mask texture read as zero, would give exactly this.
