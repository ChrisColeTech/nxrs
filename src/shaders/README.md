# shaders

Every shader a game ships, found when the game is built and translated to SPIR-V for Vulkan before it ever runs.
The interface (calls, output files, shader key, descriptor model) is in [docs/INTERFACES.md](../../docs/INTERFACES.md#shaders-srcshaders).
Credits and licenses: the repository's [README](../../README.md#credits-and-licenses).

## How

1. **Open the game** with the translator's package API (`translator::package`: folder, NSP or NUP, the update applied in
   place; installed add-on content through `dlc`).
2. **Find every shader** in the ExeFS code modules (NSO segments decoded here: LZ4 or ZBIC) and every RomFS file, by
   content, never by name, in parallel (8 threads). One copy per key; vertex / pixel pairs from program variations.
3. **Translate** with `dmsh_xlate.exe` (nxnative's tool on Ryujinx's shader translator, a NativeAOT executable built
   from source by `tools/build_dmsh_xlate.ps1`), 4 processes over batches of 64 jobs; a batch whose process dies or
   hangs is split and run again, so a shader that crashes the translator costs only itself.
4. **Fix** what Vulkan needs and the translator leaves out (`fixes.rs`: storage image read / write without format
   capabilities; a size query's missing depth / layer component read as 1 on a texture typed 2D for want of a
   per-game type answer; a fetch / read with a computed `Offset`, which Vulkan allows on gathers only, gets the offset
   added to its coordinate, a constant one becomes `ConstOffset`), then **validate** each module with the Vulkan SDK's `spirv-val --target-env vulkan1.3`.
5. **Write** `spv/<key>.spv` + `spv/<key>.json` (bindings with their Vulkan descriptor types and what the game binds
   there), `manifest.tsv`, `pairs.tsv`, `problems.txt`. A rebuild reuses every output whose job and tools are unchanged.

| Module | What |
|---|---|
| `nvn.rs` | control section + code area, the shader key, stages, the consistency checks |
| `formats/nvnfmt.rs` | BNSH (with NVN2 ray-tracing / task / mesh stages), agl BAHS and its program variations, NDSR, NDVE, sead's primitive drawer, Shin'en SHN!, MercurySteam MSHD |
| `formats/glslc.rs` | GLSLC compiler outputs (NVN and NVN2; one stage, or several as one program) |
| `formats/archive.rs` | Yaz0, SARC, zstd (with the game's `Pack/ZsDic.pack.zs` dictionaries), gzip, size-prefixed zstd / LZ4 frames, Oodle (`lztans`), CRI CPK + CRILAYLA |
| `formats/gamefreak.rs` | GFPAK (stored, zlib, LZ4, Oodle Kraken), Trinity file systems (`data.trpfs`: FlatBuffers packs, Oodle) |
| `formats/engines.rs` | Snowdrop BERG records (zlib / zstd, nested zstd chunks), Heaps PAK shader cache (base64 GLSLC outputs), Hedgehog Engine 2 PACx 403 (Sonic Frontiers) and 405 (Sonic X Shadow Generations) archives and Needle (HNSHV002) containers, PlatinumGames DAT archives and shader objects (VSO / PSO / ..., the binding-table form), Criterion Bundle 2 (zlib resources with GLSLC outputs), RAGE effects (rgxd), YEBIS .tfxo files (loose, and zlib streams in code modules) |
| `formats/unity.rs` | UnityFS bundles (stored / LZ4 / LZMA blocks) and serialized files: Shader objects' LZ4 blobs found by the shape of their fields (no type tree needed), 2019.3+ and older program code, stored program code (compute kernels), GLSLC outputs in TextAssets |
| `formats/unreal.rs` | Unreal paks (versions 4, 8, 11; AES-encrypted indexes and entries with the key found in main: two 16-byte literals its code stores, or a 32-byte window; stored / Zlib / LZ4 / Oodle blocks), IoStore containers (.utoc / .ucas: shader libraries, cooked packages' export bundles), Unreal Engine 3 packages and global shader caches, shader code archives (version 2; LZ4 or Oodle shaders), shaders inline in cooked packages and global shader caches (LSLG records stored, zlib, LZ4; Oodle shader map tables; Switch 2 NVN2 records) |
| `formats/raw.rs` | control sections loose in main: code areas by data records (relocations), Diablo II records, adrp + add pairs, or the headed area ending at it (PhyreEngine) |
| `formats/capcom.rs` | RE Engine PAKs (KPKA, block tables) with SDF shader definitions and OCIO pipelines; MT Framework ARC archives and SPK shader packages |
| `formats/koei.rs` | KTGL RDB files (IDRK / PDRK entries, G2S / _DOK GLSLC outputs), LINKDATA archives by their index, Koei Tecmo's and Gust's chunked zlib |
| `formats/smash.rs` | Super Smash Bros. Ultimate's data.arc: the file-system table, every current file (every region's) |
| `formats/fromsoft.rs` | FromSoftware dvdbnd archives (.bdt by their .bhd5), DCX, BND3 entries with GLSLC outputs |
| `formats/zbic.rs`, `formats/nso.rs` | ZBIC (Switch 2 NSO segments; the decoder is `translator::loader::zbic`, shared with the build); NSO images |
| `scan.rs` | the whole game in parallel work items (files, Trinity pack groups, CPK entry ranges, Unreal pak file groups, IoStore chunks, RE Engine PAK entries, RDB files, LINKDATA / dvdbnd / data.arc entries, code modules) |
| `job.rs` | translator jobs: VertexA/B merge, c1, NVN2 compute c0 layout and stage banks, task / mesh stage memory, facts |
| `xlate.rs`, `fixes.rs`, `reflect.rs`, `prebuild.rs` | the translator and spirv-val driven in parallel, SPIR-V edits, binding reflection, the build step; `prebuild::translate_one` is the live translator the graphics layer calls for a shader the build did not find |
| `bin/nxshaders.rs` | the CLI: `prebuild`, `prebuild-data`, `stage`, `export`, `coverage` |
| `tools/` | `build_dmsh_xlate.ps1` + `dmsh_xlate.patch` (the Vulkan target, a descriptor set per stage); `crosscheck_sm86.py` |

`cargo test -p shaders`: unit tests of every format on small synthetic inputs, well
under a second.

## Verification (2026-11-19, the owner's games, read in place; 4 translator processes)

| Game | Found | Translated | Failed | spirv-val clean | Scan / translate / validate / total (s) | Keys the game used (Turbo's cache): found, ok |
|---|---|---|---|---|---|---|
| Mario Kart 8 Deluxe (NSP 1G+1U+1D) | 29,651 | 29,651 | 0 | 29,651 | 8 / 27 / 93 / 148 | 2,515 / 2,515, 2,515 ok |
| Mario Kart World (NUP) | 11,609 | 11,609 | 0 | 11,609 | 27 / 26 / 46 / 112 | 1,200 / 1,200, 1,200 ok |
| Donkey Kong Bananza (NUP) | 22,854 | 22,854 | 0 | 22,854 | 23 / 49 / 94 / 185 | 828 / 828, 828 ok |
| Ocarina of Time (NUP) | 42,848 | 42,848 | 0 | 42,848 | 28 / 182 / 322 / 604 | 871 / 871, 871 ok |
| Pokémon Legends: Z-A (NUP, Trinity + Oodle) | 28,603 | 28,603 | 0 | 28,603 | 9 / 55 / 106 / 205 | 279 / 279, 279 ok |
| Pokémon Legends: Arceus (NSP 1G+1U, GFPAK + Oodle) | 12,851 | 12,851 | 0 | 12,851 | 5 / 14 / 42 / 75 | 275 / 275, 275 ok |
| Pokémon Violet (NSP 1G+1U+1D, Trinity + Oodle) | 35,937 | 35,937 | 0 | 35,937 | 22 / 48 / 155 / 274 | 290 / 290, 290 ok |

- The three failures of the first build are fixed (fixes 3): MKW's 078997bf631992c5 is a compute program whose code
  area starts with a 0x80-byte Ampere SPH (word 0 0x020c0481, "instruction 0x481" does not exist), taken off by
  `job::is_ampere_sph`; Ocarina's d17eefaff4f0bc23 and Violet's abd67260d710b0a0 fetch with a computed offset
  (`fixes::dynamic_offsets`). MKW, Violet and Ocarina re-run with 0 failed and 0 invalid (`build/G/shaders/`).
- The key sets equal Turbo's prebuild scans for MKW, MK8D, DKB and OoT (Turbo could not read the Pokémon games), and
  nxnative's documented counts for Arceus (12,851) and MK8D (29,650 + one sdk shader).
- A rebuild with nothing changed reuses every output (Violet: 18 s).
- Cross-check, mkw-recomp's sm86 against dmsh_xlate on 500 MKW programs spread over the key range (318 pixel, 154
  vertex, 28 compute): both valid on all 500; interfaces (stage inputs and outputs, constant banks, storage buffers,
  textures, images, workgroup size) equal on 413. The 87 others: 59 vertex shaders where sm86 keeps a load of c16 whose
  value is never read (Ryujinx removes it), 28 compute shaders where the two address NVN2's per-shader c0 layout
  differently (dmsh_xlate maps it onto NVN's fixed tables, sm86 keeps raw c0 offsets) with the same resource counts.

The commands:

    nxshaders prebuild <game> --out build/shaders/<id> --jobs 4
    nxshaders coverage build/shaders/<id> "<build folder>/<name>_nvn"
    nxshaders export <game> --out build/shaders/crosscheck/sample --sample 500 --arch sm86
    uv run --no-project --python 3.14 src/shaders/tools/crosscheck_sm86.py --mkw <mkw-recomp> --spec build/tools/sm86/instructions.json --sample build/shaders/crosscheck/sample --build build/shaders/mkw --work build/shaders/crosscheck/work
