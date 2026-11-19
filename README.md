# RecompLauncherEngine

A Rust engine that runs Switch games as native programs: their ARM64 code is translated to C ahead of time, and the
game's data is read in place from its own packages.

| Part | What |
|---|---|
| [`src/translator`](src/translator/README.md) | package input (folder, NSP, NUP, XCI, NCA; updates and DLC in place) and the ARM64 → C translator, with its development CLI `nxrs-translate` |
| `src/runtime` | the host runtime |
| [`src/cli`](src/cli/README.md) | `nxrs`: the build (clang, chunk cache, `<name>_code.dll` + loader), updates, DLC, the launcher contract ([docs/LAUNCHER_CONTRACT.md](docs/LAUNCHER_CONTRACT.md)) |
| [`src/profiles`](src/profiles/README.md) | game profiles (nxnative's 187, as data) and their lookup by title and build id |
| [`src/shaders`](src/shaders/README.md) | every shader of a game found at build time (ExeFS and RomFS, the containers of many engines, zstd / Yaz0 / ZBIC / Oodle) and translated to SPIR-V for Vulkan, validated, with a manifest by shader key; CLI `nxshaders` |
| [`src/os`](src/os/README.md) | the OS services: the SDK's service clients replaced by name (file system over the packages in place, saves, accounts, applet / oe, time, settings, hid, audio renderer, vi, Mii, the offline stand-ins), the IPC framework and `sm`, the memory and IPC SVCs, virtual cores, the process boot, and the NVN bootstrap with a null device |

Build and test: `cargo build --release`, `cargo test` (seconds). The CLI is `target/release/nxrs`; it links games
against `target/release/nxrt.lib` (`cargo build --release -p runtime`).

## Credits and licenses

The engine builds on these projects. Their license texts are in [`LICENSES/`](LICENSES/).

| Source | Copyright and license | What it contributed |
|---|---|---|
| mkw-recomp | © 2026 Adrià, MIT ([text](LICENSES/mkw-recomp.txt)) | the translator, ported to Rust: NSO loading, module seeds and layout (`loader/`), the lifter, NEON/FP lowering, local flags and registers, jump tables (`xlate/`), function discovery, chunked emission and the patch table (`aot/`); the runtime headers the generated C includes (`src/translator/runtime/`, vendored; only their attribution line moved here) |
| totk-pc-recomp | © 2026 Valiantsin Dzerakh, MIT ([text](LICENSES/totk-pc-recomp.txt)) | the translator code mkw-recomp took from it (its `recomp/`) |
| LAS Recomp | © 2026 Peiikai, MIT ([text](LICENSES/las-recomp.txt)) | the original NSO loader, lifter, NEON/FP lowering, generator and `cpu.h` / `simd.h` / `simd_fp.h` that totk-pc-recomp and mkw-recomp built on |
| nxnative | © 2026 seedds, MIT ([text](LICENSES/nxnative.txt)) | the engine's design: this is a Rust port of nxnative, which runs ARM code natively; the translator fills that gap |
| the owner's code | ChrisColeTech | package input: NSP / NCA / ticket / key handling, BKTR updates, NUP, DLC install and the RomFS markers, ported from the owner's readers written for the Turbo fork |
| mkw-recomp (runtime) | © 2026 Adrià, MIT ([text](LICENSES/mkw-recomp.txt)) | the runtime's mechanisms, ported to Rust in `src/runtime/`: the flat PC → function dispatch table (`dispatch.rs`, `asm.rs`), host functions at synthetic guest addresses and the `HOST_OVERRIDE` binding rule (`hostfn.rs`, `loader.rs`), guest setjmp/longjmp owners (`sjlj.rs`, `asm.rs`), nn::os fibers on host fibers (`fiber.rs`), the Windows guest memory model (`mem.rs`: fixed reservations below 2^39, write watch, GetInfo ids, QueryMemory states), the `rt_*` services and the crash report's guest backtrace (`rt.rs`, `fault.rs`); `tools/mkw_gen.py` runs its unmodified `recomp/gen.py` |
| LAS Recomp (runtime) | © 2026 Peiikai, MIT ([text](LICENSES/las-recomp.txt)) | through mkw-recomp: the host-function registry and dispatch it started from, and the GetWriteWatch use in the memory layer |
| nxnative (runtime) | © 2026 seedds, MIT ([text](LICENSES/nxnative.txt)) | the minimal kernel (`kernel.rs`: threads, ArbitrateLock/Unlock, the process-wide-key condition variable, address arbitration, events, WaitSynchronization) ported from `dm/host/hle_thread.c`; import binding by symbol name with an UNIMPL report from `hle.c` / `loader.c` |
| mkw-recomp (build step) | © 2026 Adrià, MIT ([text](LICENSES/mkw-recomp.txt)) | the compile step in `src/cli/` (`cc.rs`), re-expressed from `tools/prepare/gamecode.py` and `tools/build/build.py`: the game-code flag set, objects reused under a content key of the compiler, headers, flags and C, the retry of a compiler crash, the link through a response file |
| nxnative (profiles) | © 2026 seedds, MIT ([text](LICENSES/nxnative.txt)) | the 187 game profiles of `dm/games` (`game.tsv`, the `patches.py` tables and stubs), `dm/tools/sdk_patches.py`, `engine_patches.py` and `dm/games/settings.tsv`, imported as data into `src/profiles/`; the profile checks and defaults of `game.py` |
| the owner's code (Turbo) | ChrisColeTech | the launcher-facing logic of the Turbo fork the `nxrs` contract keeps (`--update`, `--update-info`, the DLC options, their messages and exit codes), and the Pokémon Legends: Z-A 60 FPS patch (`src/profiles/games/0100F43008C44000/turbo.toml`) |
| nxnative (shaders) | © 2026 seedds, MIT ([text](LICENSES/nxnative.txt)) | the shader readers of `dm/gfx/tools/shader_split.py` and `cpk.py`, ported to Rust in `src/shaders/src/formats/` (BNSH, agl BAHS and its program variations, NDSR / NDVE, sead's primitive drawer, SHN!, MSHD, GLSLC outputs, Snowdrop, Heaps, CRI CPK / CRILAYLA, GFPAK, Trinity packs, Yaz0 / SARC / zstd dictionaries) with the shader key and checks (`nvn.rs`); `dmsh_build.py`'s translation jobs (`job.rs`: VertexA/B merge, c1, NVN2 compute layout and stage banks, task / mesh stage memory, transform feedback, tessellation mode); the `dmsh_xlate` tool (`dm/gfx/tools/dmsh_xlate`), built from its source with `src/shaders/tools/dmsh_xlate.patch` (the Vulkan target and a descriptor set per stage) |
| Ryujinx | © Ryujinx Team and Contributors, MIT ([text](LICENSES/ryujinx.txt)) | the shader translator in `dmsh_xlate` (`Ryujinx.Graphics.Shader`, `Ryujinx.Common`, `Spv.Generator` 1.3.2 with nxnative's changes), compiled into the tool, not linked into the engine |
| MacNativeGames | © 2026 MacNativeGames contributors, MIT ([text](LICENSES/macnativegames.txt)) | through nxnative: `dmsh_xlate`'s `VertexCounterLowering` |
| lztans | © 2026 patodois, MIT OR Apache-2.0 ([text](LICENSES/lztans.txt)) | the Oodle decoder (Kraken, Mermaid / Selkie, Leviathan; a crate dependency, written clean-room) for Game Freak's GFPAK entries and Trinity packs |
| the owner's code (shaders) | ChrisColeTech | the ZBIC decoder for Switch 2 NSO segments (`src/shaders/src/formats/zbic.rs`), ported from the owner's `zbic.py` for the Turbo fork, and its test vector |
| mkw-recomp (shaders) | © 2026 Adrià, MIT ([text](LICENSES/mkw-recomp.txt)) | not included: its `sm86` translator is run from its own checkout as the cross-check of `dmsh_xlate` (`src/shaders/tools/crosscheck_sm86.py`) |
| nxnative (os) | © 2026 seedds, MIT ([text](LICENSES/nxnative.txt)) | the OS services in `src/os/`, ported from `dm/host`: the HLE takeovers by name and their answers (`hle_fs.c`, `hle_oe.c`, `hle_account.c`, `hle_time.c`, `hle_hid.c`, `hle_mii.c`, `hle_audren.c`, `hle_audio.c`, `hle_sdk16.c`, `hle_misc.c`, `hle_offline.c`, `hle_net.c`, `hle_friends.c`, `hle_sf.c`, `hle_sdkinit.c`; the offline stand-in table in `services/simple_table.rs` is generated from them), the boot order of `main.c`, the memory SVCs of `hle_svc.c`, the NVN device properties and object semantics of `dm/gfx/nvn/hle_nvn.c`, `dm/gfx/hle_nv.c`, `dm/gfx/hle_vi.c`, and the texture layout of `nvn_tex.c` (itself deko3d's algorithm: © 2018-2020 fincs, zlib, [text](LICENSES/deko3d.txt)) |
| mkw-recomp (os) | © 2026 Adrià, MIT ([text](LICENSES/mkw-recomp.txt)) | the virtual-core design of `src/os/src/sched.rs` (one guest thread per application core, checkpoints at SVCs and spin waits, from its `kernel_cores.c`) |
