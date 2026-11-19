# profiles

Game profiles: what the engine knows about each game, as data, and the lookup by title id and module build id.
The data is nxnative's `dm/games` (187 profiles; its `dm/games` folder has 192 entries: the 187 profile folders and
five files), imported by `tools/export.py`, plus the owner's patches. Everything is embedded at compile time; the
engine reads no files for it. API: [docs/INTERFACES.md](../../docs/INTERFACES.md#profiles-srcprofiles).

| Path | What |
|---|---|
| `games/<title id>/game.tsv` | nxnative's profile, byte for byte: identity, `build` rows (version, display version, module build ids), modules, memory layout, per-game settings |
| `games/<title id>/patches.toml` | its `patches.py` as data (27 games) |
| `games/<title id>/turbo.toml` | the owner's patches from Turbo, kept by hand (Pokémon Legends: Z-A: base frame rate 60, build `b1f12fd9…`) |
| `shared/sdk_patches.toml`, `engine_patches.toml` | nxnative's patches of Nintendo SDK modules and of middleware, for any game |
| `shared/settings.tsv` | nxnative's player-facing settings list |
| `shared/source.txt` | the nxnative commit the files came from |
| `tools/export.py` | the importer (developer side): `py -I src/profiles/tools/export.py <nxnative>`; `--check` compares |

## Patch tables

nxnative's `patches.py` files are code. Their data tables (`CUTS`, `OBS`, `CAPABILITIES`, `BINDING`, the fingerprints
of `MATCH_*` rules) are copied; their stubs (`FRAME_STEP`, `STUBS` and the stubs of `MATCH_*` rules), functions that
assemble ARM words into the rewriter's patch area, are recorded: the exporter runs each against a recording context at
two layouts, and every word that differs is a pc-relative instruction whose target is named by the one symbol whose
move explains it (`area`, `state`, `base`, `other:<label>`, or a fixed `module` offset). A third layout checks the
record: relocated there, it must equal what the stub assembles there. 10 of the 14 stubs record; 4 read the module's own
code (`ctx.address`, an anchored rule's `find`) and are kept by name with `evaluated = false`.

nxnative's cuts are data for the lanes that port its rewriter (they were made for code running natively under Apple's
kernel); nxrs applies only entries marked `apply = "build"` (today the owner's Z-A cut) through the translator's patch
table, as a switchable set (`NXRS_PATCH_ZA_60FPS=0` turns it off at run time; `NXRS_NO_PROFILE_PATCHES=1` at build
time leaves it out).

## Verification

`cargo test -p profiles` (0.05 s): 187/187 profiles parse and pass `game.py header`'s checks; the owner's seven games
resolve (by the main build ids of the owner's own packages: MK8D 4.0.1, DKB v458752, OoT v131072, Z-A 2.0.2 and Arceus
1.1.1 by registered build; MKW, whose NUP holds the 1.8.0 program, and Violet v786432 by title, their profiles
registering other builds); every patch table round-trips TOML → Rust → TOML → Rust unchanged; every recorded stub
placed at its recording layout gives back its words; the Z-A cut becomes the translator's `site` line.
`tools/export.py --check` re-exports from nxnative and compares (218 files, 0 differences).

## Not done

- The shader tables of each profile (`shader_map.tsv`, `shader_pairs.tsv`, `pso_manifest.tsv`, …, 6.5 M lines) are not
  imported: the shader lane reads them from nxnative.
- Stubs that read the module's code are not recorded; placing a `match` rule needs the fingerprint search over the
  module (nxnative `nso_match.py`), which is the translator lane's port.
- `profile_guess.py` (a provisional profile for a game without one) is not ported.
