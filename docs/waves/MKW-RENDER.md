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
