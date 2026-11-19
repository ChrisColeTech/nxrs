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
   per-game type answer), then **validate** each module with the Vulkan SDK's `spirv-val --target-env vulkan1.3`.
5. **Write** `spv/<key>.spv` + `spv/<key>.json` (bindings with their Vulkan descriptor types and what the game binds
   there), `manifest.tsv`, `pairs.tsv`, `problems.txt`. A rebuild reuses every output whose job and tools are unchanged.

| Module | What |
|---|---|
| `nvn.rs` | control section + code area, the shader key, stages, the consistency checks |
| `formats/nvnfmt.rs` | BNSH (with NVN2 ray-tracing / task / mesh stages), agl BAHS and its program variations, NDSR, NDVE, sead's primitive drawer, Shin'en SHN!, MercurySteam MSHD |
| `formats/glslc.rs` | GLSLC compiler outputs (NVN and NVN2; one stage, or several as one program) |
| `formats/archive.rs` | Yaz0, SARC, zstd (with the game's `Pack/ZsDic.pack.zs` dictionaries), gzip, size-prefixed zstd / LZ4 frames, Oodle (`lztans`), CRI CPK + CRILAYLA |
| `formats/gamefreak.rs` | GFPAK (stored, zlib, LZ4, Oodle Kraken), Trinity file systems (`data.trpfs`: FlatBuffers packs, Oodle) |
| `formats/engines.rs` | Snowdrop BERG records (zlib / zstd, nested zstd chunks), Heaps PAK shader cache (base64 GLSLC outputs), Hedgehog Engine 2 PACx 403 archives and Needle (HNSHV002) containers |
| `formats/zbic.rs`, `formats/nso.rs` | ZBIC (Switch 2 NSO segments) to standard zstd; NSO images |
| `scan.rs` | the whole game in parallel work items (files, Trinity pack groups, CPK entry ranges, code modules) |
| `job.rs` | translator jobs: VertexA/B merge, c1, NVN2 compute c0 layout and stage banks, task / mesh stage memory, facts |
| `xlate.rs`, `fixes.rs`, `reflect.rs`, `prebuild.rs` | the translator and spirv-val driven in parallel, SPIR-V edits, binding reflection, the build step |
| `bin/nxshaders.rs` | the CLI: `prebuild`, `prebuild-data`, `stage`, `export`, `coverage` |
| `tools/` | `build_dmsh_xlate.ps1` + `dmsh_xlate.patch` (the Vulkan target, a descriptor set per stage); `crosscheck_sm86.py` |

`cargo test -p shaders`: unit tests of every format on small synthetic inputs (and the ZBIC reference vector), well
under a second.

## Verification (2026-11-19, the owner's games, read in place; 4 translator processes)

| Game | Found | Translated | Failed | spirv-val clean | Scan / translate / validate / total (s) | Keys the game used (Turbo's cache): found, ok |
|---|---|---|---|---|---|---|
| Mario Kart 8 Deluxe (NSP 1G+1U+1D) | 29,651 | 29,651 | 0 | 29,651 | 8 / 27 / 93 / 148 | 2,515 / 2,515, 2,515 ok |
| Mario Kart World (NUP) | 11,609 | 11,608 | 1 (Ampere instruction not implemented) | 11,608 | 27 / 22 / 45 / 103 | 1,200 / 1,200, 1,200 ok |
| Donkey Kong Bananza (NUP) | 22,854 | 22,854 | 0 | 22,854 | 23 / 49 / 94 / 185 | 828 / 828, 828 ok |
| Ocarina of Time (NUP) | 42,848 | 42,848 | 0 | 42,847 (1: Offset on a non-gather sample) | 24 / 157 / 212 / 441 | 871 / 871, 870 ok |
| Pokémon Legends: Z-A (NUP, Trinity + Oodle) | 28,603 | 28,603 | 0 | 28,603 | 9 / 55 / 106 / 205 | 279 / 279, 279 ok |
| Pokémon Legends: Arceus (NSP 1G+1U, GFPAK + Oodle) | 12,851 | 12,851 | 0 | 12,851 | 5 / 14 / 42 / 75 | 275 / 275, 275 ok |
| Pokémon Violet (NSP 1G+1U+1D, Trinity + Oodle) | 35,937 | 35,937 | 0 | 35,936 (1: as Ocarina's) | 15 / 40 / 119 / 209 | 290 / 290, 290 ok |

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
