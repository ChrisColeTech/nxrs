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
