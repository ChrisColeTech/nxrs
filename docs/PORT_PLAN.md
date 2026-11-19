# Rust port of nxnative to Windows x86-64: the plan (revision 4)

Date: 2026-11-19 (revision 4). The engine lives at `D:\Projects\RecompLauncherEngine`; this file is kept at
`docs/PORT_PLAN.md` there (the copy under Switch2RECOMP-main's `build/evidence/audit` is the original, superseded).

Working name: **nxrs** (rename freely). The plan's `nx-*` crate names map onto the repository's one-crate-per-part
layout: `translator` (nx-xlate, nx-aot, nx-package, nx-loader's build half), `runtime` (nx-cpu, nx-mem, nx-kernel's
core), `os` (nx-kernel, nx-hle, nx-fs, nx-audio, nx-video, nx-input, nx-front's services side), `graphics` (nx-nvn,
nx-vk, nx-tex), `shaders` (nx-shader), `profiles`, `cli` (nx-cli, nx-cc, nx-run).

## Progress

Status: **Done** = exit criterion met and committed; **Partial** = committed, criterion not fully met (what is missing
is in the Notes and in section 5A); **In progress** = an agent is working on it now; **Not started**.
Agents update their own rows when they finish.

| Phase | What | Status | Commit(s) | Notes |
|---|---|---|---|---|
| M0 | Foundations: workspace, profiles, package listings | Partial | 202d449 | 187 of 192 profiles imported (nxnative's 187 + the Z-A 60 FPS patch); no CI (no remote yet); `PORT_MAP.tsv` not written |
| M1 | Translator | Partial | c7d34a3, adc736f | MKW C 242,014 / 245,653 functions identical to mkw-recomp, every diff explained; MK8D cold build 70 s. **Missing:** ZBIC NSOs (Ocarina, Switch 2 games), XCI as the game's own package, 6 untyped SDK seeds |
| T-lane | dynarmic differential tests | Not started | | parallel, not gating |
| M2 | Execution and the bridge (runtime) | Done | c7d34a3, 6fae87d | homebrew e2e 80/80, threads 200/200; 5 os requests open (`docs/INTERFACES.md`) |
| M3 | OS + HLE boot | Done | 3845d1c | MK8D to first submit 0.64 s, 5 min no crash, peak 1.7 GB, **on the null GPU** (to be removed, rule 1.1.6) |
| G-lane | Graphics: NVN on Vulkan | Partial | 34f540b | Wired to os (M4). ~620 entry points. Rev 5 (lane G, uncommitted): replay on its own thread (async submission), re-initialized textures keep their images (MK8D's per-frame uploads gone), layered passes and 3D targets as slice arrays, mesh draws (VK_EXT_mesh_shader, lane S's modules), transform feedback captures, Vulkan sparse images (MKW no longer killed at 28 GB), built-in and LSFG frame generation, parked queues without red lines, system-memory cap and a memory line. DLSS evaluation answered by a filtered upscale of the input rectangle (no network). Rev 6: lazy write-back off by default (its fault race fixed), screenshot deadlock fixed, write watch per allocation (Violet's 40-50 ms snapshot compares), c0 / c2 banks cached per stage binding generation, multi-threaded texture de-tiling, no storage uploads for render targets, steadier built-in frame generation. MK8D 10-minute demo `r28`: private memory flat at 5.9 GB from 200 s to 520 s. **Still open:** ray dispatches, depth resolve, resolution scale, the async-replay stall (off by default). Gaps in 5A.4 |
| S-lane | Build-time shaders | Done (shader side) | 6c11b77, de2a969 | 184k shaders over 7 games, 0 failed / 0 invalid; real task/mesh modules (MKW 7+19, OoT 20+500); ray tracing as a ray-query library (no owner game uses it); all nxnative readers incl. Jamboree BEA (25,693). Graphics still has to replay mesh draws and RT (lane G)
| **GAPS** | **Turbo + lane parity (section 5A)** | In progress | | everything Turbo does today, ported; real I/O only |
| M4 | First pixels: MK8D title screen on Vulkan | Done | | os → graphics wired (os links graphics, no null device in the path); title screen, Mii select and main menu on the RTX 5090 at 59.4 FPS; title FLIP 0.017 vs Turbo's capture (menu 0.138: Turbo's capture has banding ours lacks); `[vk] pools:` logs placement and write-back ms/frame. Shots: `build/G/m4/` |
| M5 | MK8D at 60 | Not started | | needs audio, input, video, window from GAPS |
| M6 | Pokémon trio | In progress | | lane P: all three build (`build\games\{plza,arceus,violet}`). **Arceus** 30.0 FPS from boot through the opening, the Obsidian Fieldlands beach and field to Jubilife Village in 600 s (village textures and text garbled: lane G). **Violet** past the start-up aborts, through its academy opening to the bedroom and the first tutorial in 600 s (`NXGFX_WB_LAZY=0`), 30 FPS early, 6-15 FPS in the opening, 23 at the end (texture-upload-bound slow frames: lane G). **Z-A** 60 FPS on title / menus with its patch, Lumiose City lit at 35-46 FPS (no crash in 240 s with `NXGFX_WB_LAZY=0`; with the lazy write-back a pool-page write fault at ~60 s: lane G). Fixes: 64 GiB alias region, MapMemory sources, `bt`, `mem_map_limit`, host-lent command memory reclaim; stall dumps. Details and lane G / I items: INTERFACES.md, lane P |
| M7 | MKW + DKB | Not started | | needs ZBIC, real mesh shaders |
| M8 | Ocarina of Time | Deferred | | owner 2026-11-19: after the other games are validated. ZBIC done; builds; stops at a static-init fault after SDK start-up |
| M9 | Launcher contract v2 | Done for MK8D; MKW builds and starts, then hits the 20 GB watchdog at 6 s (NVN pool host memory, lane G) (uncommitted, lane L) | launcher c599914 | CLI renamed nxrs-build; launcher per-game engine, v2 build/update/DLC/Play; MK8D built, updated (none/4.0.1), Booster Course installed and seen in game, raced via run_game; MKW built through the launcher (Switch 2, 481 s); release packaging with llvm-mingw passes the relocated smoke test |
| M10 | Linux | Not started | | |

## What changed

**Revision 4 (owner, 2026-11-19).**

- **Progress table** above; milestones carry a status.
- **Section 5A, the GAPS phase:** every feature the Turbo engine has today (the owner's fork commits, and Turbo's
  runtime options, hotkeys, services and Vulkan behaviour) that the new engine lacks, plus what lanes A, B and C left
  unfinished. The functionality is up for grabs: the owner's fork code is his (MIT), MZ Labs' and nxnative's
  approaches may be reimplemented (section 8), so agents **port it, they do not leave it out or refuse it**.
- **Real I/O only (rule 1.1.6):** no null GPU, no fake controllers, no silent audio sink in a game run. Games run on
  the real Vulkan device, read real pads, play real sound. Fakes hide real bugs.
- **No feature is dropped by default.** "Turbo refuses it" or "not needed for the first game" is not a reason; only the
  owner drops a feature.

**Revision 3 additions:**

- **No source headers (owner, 2026-11-19).** Attribution is only in the README's Credits and licenses section; source files carry no copyright, license or "ported from" headers (section 8).

**Revision 3** corrects revision 2, which had turned the plan into an MKW port built on mkw-recomp's
runtime. That is not the project.

- **nxnative is the base and the primary source again.** Its host, HLE, NVN layer, game profiles and reference docs are
  ported wholesale, module by module, into Rust crates.
- **mkw-recomp (MIT) is a parts donor, and only for what nxnative lacks on Windows x86-64:**
  - the translator: a lifter to C, compiled with clang, ported to Rust as `nx-xlate`/`nx-aot` in days;
  - the Windows gaps that native ARM execution let nxnative skip: the guest memory model, the call bridge from
    translated code into host HLE, TLS/x18/tpidr, `svc` dispatch, exclusive monitors, guest `setjmp`/`longjmp` and
    fibers;
  - Vulkan know-how and code for rewriting nxnative's Metal back end piece by piece. The structure stays nxnative's.
- **What we do not do with mkw-recomp:** we do not run its runtime, we do not generalise it, and MKW is not the first
  milestone.
- **Switch 2 shaders:** nxnative's Ryujinx-based path stays primary. mkw-recomp's SM86 translator is a cross-check and
  fallback (section 3.4).
- **New milestone order:**
  1. the translator (days, with dynarmic differential tests in parallel);
  2. the OS and HLE boot from nxnative;
  3. first pixels on Vulkan;
  4. the games, starting with **Mario Kart 8 Deluxe**: the owner game nxnative supports best and the easiest to bring
     up.
- **Estimate:** about 650–1,050 sessions over 7–9 months. Revision 1 said 900–1,450 sessions over 9–12 months.

**Revision 2** added mkw-recomp as a source. Its findings on how mkw-recomp works are kept in section 3.

**Revision 1** was the from-scratch Cranelift translator. That translator is dropped in favour of the mkw-recomp port.

---

## 0. Summary for the owner

**What we are building.** nxnative plays Switch and Switch 2 games on Apple chips by running the game's ARM code
directly on the ARM CPU. Your PC has an Intel CPU, so two things must be added before nxnative's work can run on it:

1. **A translator** that turns the game's ARM code into x86 code ahead of time. We do not write one from scratch.
   mkw-recomp (MIT) already has a proven one (ARM → C → clang) that runs Mario Kart World at 60 on your PC. We port
   that translator to Rust.
2. **The few low-level pieces an ARM Mac gets for free:**
   - how game memory is laid out on Windows;
   - how translated code calls the engine's own functions;
   - thread registers, system calls and atomic operations.

   mkw-recomp has these for Windows too, and we take them.

Everything else comes from nxnative, ported into Rust: its stand-in for the Switch's OS and services, its graphics
layer, its knowledge of 192 games, and its 13 reference documents. nxnative's graphics layer targets Apple's Metal, so
it is rewritten onto Vulkan for the RTX 5090. Wherever mkw-recomp shows how a piece is done on Vulkan and Windows, it is
the model for that piece.

```
 BUILD (nxrs build — the launcher's "Build")                    RUN (<name>.exe)
 ┌────────────────────────────────────────────────┐            ┌───────────────────────────────────────────────┐
 │ NSP/NUP/update/DLC in place  (nx-package)       │            │ Translated game code (C → clang)              │
 │ Profile from nxnative's 192   (nx-profiles)     │ _code.dll ►│   └ calls into the host via the bridge (nx-cpu)│
 │ ARM64 ─► nx-xlate/nx-aot (mkw-recomp's lifter   │            │ nxnative's host, in Rust: kernel/threads,     │
 │   ported to Rust) ─► C ─► bundled clang          │ _data/ ──► │   memory (Windows model from mkw-recomp),     │
 │ Shaders: nxnative shader_split + dmsh_xlate      │            │   services, fs, hid, audio, movies, cheats    │
 │   (Ryujinx) ─► SPIR-V for Vulkan                 │            │ nxnative's NVN layer: record (ported) +       │
 │ Pipeline manifest (nxnative pso_manifest.tsv)   │            │   replay rewritten Metal → Vulkan (nx-vk)     │
 └────────────────────────────────────────────────┘            └───────────────────────────────────────────────┘
```

---

## 1. Architecture and crates

### 1.1 Principles

1. **nxnative's module structure is the crate structure.** `dm/host` becomes `nx-kernel`, `nx-hle`, `nx-fs`,
   `nx-audio`, `nx-video`, `nx-input` and `nx-front`. `dm/gfx` becomes `nx-nvn` (the record layer, ported as-is) and
   `nx-vk` (nvn_mtl.m's replay, rewritten for Vulkan, keeping its function names and order where possible). `dm/tools`
   and `dm/games` become `nx-loader`, `nx-profiles` and `nx-shader`. Keeping the structure lets nxnative's upstream
   changes be ported hunk by hunk.
2. **Translator and Windows-gap crates come from mkw-recomp:** `nx-xlate`, `nx-aot`, `nx-cc`, `nx-genrt`, `nx-cpu`,
   and the mechanism inside `nx-mem`. They stand in for what nxnative gets from running natively: `patch_nso.py`'s
   Apple rewrites, `entry.S`, `thread.c`, `nvn_thunks.S`, `hle_bigstack.c` and the gate stubs.
3. **Identity guest memory.** The game's addresses are host addresses, below 2^39. nxnative assumes this
   ("the game's memory is the host's"), and mkw-recomp does the same on Windows. So nxnative's host code, which
   dereferences guest pointers, ports mechanically.
4. **Build-time work where nxnative does it:** shaders split and translated offline, and a pipeline manifest prewarmed
   at load. On a discrete NVIDIA GPU, ASTC is decoded and recompressed to BC7 on the GPU at run time (mkw-recomp's
   measured approach), with an optional disk cache.
5. **The launcher contract is kept** (section 6).
6. **Real I/O only.** A game run uses the real Vulkan device and a real window, real controllers (XInput, DualSense /
   DS4, Switch Pro, Joy-Con, keyboard, mouse), a real audio sink and real decoders. No null GPU, no stand-in
   controller, no silent sink, no "black picture" decoders: a fake hides the bugs the run is meant to find. Unit tests
   may use small synthetic inputs, but no milestone or GAPS item is passed on a fake.
7. **Turbo parity.** Anything the Turbo engine does today is ported (section 5A). The owner's fork code may be copied;
   MZ Labs' and nxnative's designs are reimplemented. Leaving a Turbo feature out needs the owner's say-so.

### 1.2 Crate list

| Crate | Source | Purpose | Interface sketch | Key external crates |
|---|---|---|---|---|
| `nx-common` | nxnative `dm.h`, env switches; Turbo log format | Logging (`[  t] [vk] pacing:`), switches, hashing | `log!`, `Switch::get("DM_…")` | `parking_lot` |
| `nx-xlate` | **mkw-recomp `recomp/lifter.py`, `simd_c.py`, `flags.py`, `regs.py`, `jumptables.py`** | ARM64 → C per function: decoder, integer/FP/NEON/crypto lowering, function-local flags and registers, jump tables | `lift_fn(&Module, start) -> CFunction` | — (decodes by bit fields, as the Python does); `yaxpeax-arm` 0.5 as a test cross-check |
| `nx-aot` | **mkw-recomp `recomp/gen.py`, `nso.py`, `module.py`, `layout.py`, `patches.py`**; nxnative `jump_solver.py`, `nro_collect.py`, `nso_match.py` | Function discovery, image patches (nxnative profile cuts), spin-gate sites, chunked C emission, function tables | `Aot::new(modules, profile).discover().emit(jobs)` | `rayon`, `lz4_flex` |
| `nx-cc` | **mkw-recomp `tools/prepare/gamecode.py`, `fastcc.py`** | Bundled clang (llvm-mingw): `-O1` flag set, chunk cache, link probe, `<name>_code.dll` | `compile(gen, jobs) -> Dll` | — |
| `nx-genrt` | **mkw-recomp `runtime/cpu/cpu.h`, `simd.h`, `simd_fp.h`, `fma.c`** | The C headers the generated code includes. Stays C; mirrored in Rust as `#[repr(C)] GuestCtx` | (C) | — |
| `nx-cpu` | **mkw-recomp `runtime/cpu/dispatch.c`, `hostfn.c`, `sjlj.c`**, `kernel_fiber.c` (fiber switch) | Run time: PC → function table, the **host-call bridge** (host functions at guest addresses), guest `setjmp`/`longjmp`, fiber context switch, `rt_svc`/`rt_brk`/`rt_udf` entry, fault → guest-PC report, dispatch-miss log | `rt_dispatch`, `register_host(addr, f)`, `call_guest(ctx, addr, args)` | `windows-sys` |
| `nx-oracle` (dev) | dynarmic | Differential tests of lifted C vs dynarmic per instruction class. Parallel lane, not gating | `diff(class, n)` | `dynarmic-sys-mythrax` 0.3.7 |
| `nx-mem` | nxnative `hle_svc.c` (memory SVCs, layout rules, `memory.md`); **mkw-recomp `core/memory/memory.c`, `memory_early.c`, `platform/win32_mem.c`** (Windows mechanism) | Guest space reservation below 2^39, 4 KB commit, `MEM_WRITE_WATCH`, read traps, aliases, QueryMemory | `Space::reserve`, `map/unmap/protect`, `written_pages(range)` | `windows-sys` |
| `nx-loader` | nxnative `loader.c`, `main.c`, `nso.py`, `nacp.py`, `patch_nso.py` (cut and frame-step parts) | Module layout, relocation, imports bound to host-call addresses, `HLE_FORCE` takeovers and aliases, frame-step state page, NROs | `load(&Package, &Profile) -> Process` | — |
| `nx-package` | Turbo owner code (`nca.py`, `nsp.py`, `nup.py`, `dlc.py`, `zbic.py`, `runtime/nsp.c`/`nup.c`/`dlc.c`); nxnative `romfs.py`, `pack_nup.py` | NSP/NSZ/XCI/NCA/BKTR/NUP/DLC/updates/ZBIC, keys, RomFS in place | `Package::open`, `RomFs::open(base, update)` | `aes`, `ctr`, `xts-mode`, `sha2`, `zstd` |
| `nx-profiles` | nxnative `dm/games/**`, `sdk_patches.py`, `profile_guess.py`, `settings.*` | 192 profiles as data; `patches.toml` from the 27 `patches.py`; per-game runtime facts (memory layout, cores, tick rate) | `Profile::for_build(title, build_ids)` | `serde`, `toml` |
| `nx-kernel` | nxnative `hle_thread.c`, `hle_svc.c` (non-memory), `hle_os.c`, `hle_guestthread.c`; Windows thread mechanics from mkw-recomp `platform/win32_sync.c`, `kernel_preempt.c` (reference) | Threads (one host thread per guest thread), cores, priorities, preemption, sync SVCs, events, handles, guest clock | `svc(ctx, n)`, `create_thread`, `wait_sync`, `arbitrate_lock` | `windows-sys` |
| `nx-hle` (+ `nx-hle-macros`) | nxnative `hle.h`, `hle.c`, all service `hle_*.c`, `hle_sf.c` | HLE registry by mangled name (`#[hle]`, `#[hle_force]`, `hle_alias!`); the CMIF/sf framework; the services | `#[hle("_ZN2nn…")] fn f(ctx, a: u32, p: GuestPtr<T>) -> Result` | `linkme`, `syn`, `quote` |
| `nx-fs` | nxnative `hle_fs.c`, `account_users.h` | Mounts, RomFS in place, transactional saves | `mount`, `SaveBackend` | — |
| `nx-nvn` | nxnative `gfx/nvn/hle_nvn.c`, `nvn_obj.h`, `names.txt`, `nvn.h`, `nvn_tex.c`, `nvn_fstep.h`, `nvn_pace.h`, `nvn_capture.*` | NVN interface layer: bootstrap takeover, 523 entry points, objects in guest memory, command records | `trait Replay`, `entry_table()` | — |
| `nx-vk` | nxnative `nvn_mtl.m` + `.inc` + `nvn_owner.m` + `nvn_queue`, `nvn_dirty` (structure); **mkw-recomp `gfx/vk/*`** (donor for Vulkan pieces) | NVN replay on Vulkan: device, pools (discrete-GPU model), textures, pipelines, descriptors, recording, queues, present, capture | `VkReplay::new(window) -> impl Replay` | `ash` 0.38, VMA (MIT) or `gpu-allocator` 0.28 |
| `nx-shader` | nxnative `shader_split.py`, `dmsh_build.py`, `dmsh_reflect.py`, `dmsh_xlate`; mkw-recomp `vk_spirv_patch.c` (binding rewrites); mkw-recomp `sm86/` (cross-check only) | Build-time shaders → SPIR-V for Vulkan, shader map, pipeline manifest | `build_shaders(romfs, profile)` | `rspirv` 0.13, `spirv-tools` 0.13 |
| `nx-tex` | nxnative `nvn_tex.c`, `nvn_deswizzle.c`; **mkw-recomp `shaders/texture/astc_decode.comp`, `bc7_encode.comp`, `bc1_encode.comp`, `vk_astc.c`** | Block-linear layout and deswizzle; NVN → `VkFormat`; ASTC → BC7 on the GPU | `deswizzle`, `nvn_to_vk`, `AstcTranscoder` | — |
| `nx-audio` | nxnative `hle_audio.c`, `hle_audren.c`, `hle_audren_sv.c` | AudioOut, the in-process renderer, the sink | `AudioOut`, `Sink` | `cpal` 0.18 |
| `nx-video` | nxnative `hle_movie*.c`, `hle_codec*.c` | VP9 (libvpx), AV1 (dav1d), H.264 (Media Foundation), Vorbis | `VideoDecoder` | `dav1d` 0.12, own `vpx-sys`, `windows` 0.62, `lewton` |
| `nx-input` | nxnative `dm_input.h`, `pad_*.h`, `switch2_proto.h`, `motion_fusion.h`; Turbo `controller_sony*`, `pad_layout.h`; mkw-recomp `input/switch2_win.c` (Windows BLE) | XInput, DualSense/DS4, Switch Pro, Joy-Con 1/2, keyboard, motion, rumble | `Pads::poll()` | `hidapi` 2.6, `windows`; optional `sdl3` 0.22 |
| `nx-front` | nxnative `front.h`, `front_mac.m` responsibilities, `home_quit.h`, `ctrl_prompt.h`; Turbo window/INI logic | Window, present surface, INI, overlay (cheats panel, prompts) | `Front::run` | `winit` 0.30, `egui` |
| `nx-cli` | nxnative `build_ipa_from_nsp.py` pipeline; Turbo `switchrecomp.py` logic | `nxrs-build.exe`: build, update-info, DLC; launcher contracts v1/v2 | — | `clap` 4.6 |
| `nx-run` | nxnative `main.c` | `<name>.exe`; loads `<name>_code.dll` and `_data` | — | — |
| `nx-harness` | nxnative `regress.py`, `bench.py`; Turbo `guard.py`, `play_scenario.py`, `pace.py`/`trend.py` | Guard, scenarios, pacing, FLIP diff, capture replay compare | — | `image`; FLIP (BSD-3); PresentMon (MIT, external) |

**Generated-code ABI (from mkw-recomp, fixed from day 1):**

- The `Cpu` struct at mkw-recomp's offsets: x[32], sp 256, pc 264, nzcv 272, tpidr, tpidrro, exclusives, fpcr/fpsr,
  v[32] at 512.
- One `void f_<addr>(Cpu *)` per guest function.
- Host services `rt_*` (`rt_dispatch`, `rt_svc`, `rt_spin_gate`, `rt_brk`, `rt_udf`, `rt_ticks`) exported by the
  runtime `nxrt.dll`: a Rust `cdylib`, loaded by the small `nx-run.exe`.
- `<name>_code.dll` links against `nxrt.dll`'s import library and exports one table, `nx_game_tables()` (functions,
  modules, hashes, patch sets).

---

## 2. Exactly what is taken from mkw-recomp, and why

nxnative never needed any of these on Apple silicon: the CPU ran the game's code, and macOS supplied the rest.

| Gap on Windows x86-64 | nxnative today (ARM-native) | Taken from mkw-recomp | Into | Why this and not our own |
|---|---|---|---|---|
| **CPU translation** | None. `patch_nso.py` rewrites only what Apple's kernel rejects | `recomp/lifter.py` (817), `simd_c.py` (1,168), `flags.py` (146), `regs.py` (111), `jumptables.py` (1,345), `gen.py` (911), `nso.py`, `module.py`, `layout.py`, `patches.py` (630): about **5.2k lines of Python, ported to Rust** | `nx-xlate`, `nx-aot` | Runs MKW at 60 on this PC. Covers PAC/LSE/LDAPR (Switch 2) and LDXP/STXP. Fixes the clang loop-hoisting trap (compiler barrier on call-free back edges). Writing a translator from scratch was revision 1's weeks of work |
| **Compiling the C** | — | `tools/prepare/gamecode.py`, `fastcc.py`: the `-O1 -fno-vectorize -fno-slp-vectorize` flag set (measured: fewer instructions than zig's defaults), chunked parallel compile, link probe | `nx-cc` | Measured settings; about 6.5 min total setup on a 6-thread PC (Turbo's gcc: about 35 min) |
| **The C support library of generated code** | — | `runtime/cpu/cpu.h`, `simd.h`, `simd_fp.h`, `fma.c` (MXCSR mirroring FPCR.FZ, slow paths for the rounding modes, correctly rounded `fma`) | `nx-genrt` (stays C) | The lifter's output is written against them |
| **Indirect calls** | The CPU jumps natively | `runtime/cpu/dispatch.c`: an O(1) PC → function table, read-only after init, plus a host-function hash | `nx-cpu` | Lock-free and measured |
| **Calling bridge: translated code → host HLE** | ARM thunks: `entry.S` stub table, `nvn_thunks.S` (16,360 generated lines), the `hle_bigstack.c` big-stack entry, the AAPCS narrow-int workarounds in `hle.h` | `runtime/cpu/hostfn.c` (`HOST_OVERRIDE`: a host function placed at a guest address the import resolves to; `HOST_FUNCTION`: an address for a host function without a symbol, e.g. vtables and NVN procs) | `nx-cpu` + `nx-hle-macros` | The host function receives `Cpu*` and reads x0–x7/v0–v7 itself. Narrow ints and variadics stop being a problem, and the big-stack trick is unneeded (translated code already runs on host stacks). nxnative's `HLE`/`HLE_FORCE`/`HLE_FORCE_ALIAS` semantics are kept on top |
| **TLS, x18, tpidr** | `patch_nso.py` rewrites them to thread slots via `tpidrro_el0` | Fields in `Cpu` (`tpidr`, `tpidrro`); x18 is an ordinary register | `nx-genrt`, `nx-kernel` | Free in a translator |
| **`svc` and `cntpct` gates** | `patch_nso.py` → gate stubs → `dm_gate` on a gate stack | `rt_svc(c, imm, pc)` called from the lifted C; `rt_ticks()` for the counter | `nx-cpu` → `nx-kernel::svc` (nxnative's handlers) | Direct call, no stack switching. The tick rate is a profile value: 19.2 MHz on Switch 1, 31.25 MHz on Switch 2 (mkw-recomp verified the Switch 2 rate in the SDK) |
| **Exclusive monitors, LSE, barriers** | Native, plus `ldaxr`/`stlxr` ordering rewrites | The lifter's lowering: `excl_addr`/`excl_val` + `__atomic_compare_exchange_n` up to 128-bit, `__atomic_*` for LSE, `SEQ_CST` fences for `DMB`/`DSB` | `nx-xlate` | x86-correct by construction (C11 atomics) |
| **Guest `setjmp`/`longjmp` and fibers** | Native | `runtime/cpu/sjlj.c` (a setjmp caller runs under a host setjmp point), `core/kernel/kernel_fiber.c` (nn::os fibers by name, with their own host stacks and MXCSR) | `nx-cpu`, `nx-kernel` | One C function per guest function cannot follow a guest `longjmp` into the middle of a function without this |
| **Spin waits** | `patch_nso.py` spin gates (nxnative found these first) | `rt_spin_gate` in lifted loops + `tools/cpu/spin_waits.py` (itself taken from nxnative) + the compiler barrier | `nx-xlate`, `nx-aot` | Same idea, already adapted to lifted C |
| **Guest memory on x86 Windows** | Mac reservation (`hle_svc.c` `mem_init`), 16 KB pages | `platform/win32_mem.c` (`VirtualAlloc` at fixed addresses, `MEM_WRITE_WATCH`), `core/memory/memory.c` (4 KiB commit, alias region, the GetInfo ids, the QueryMemory states), `memory_early.c` (read traps for GPU-written pages) | `nx-mem` | Windows-proven mechanism. **The layout values stay nxnative's per game** (`game.tsv` `mem_reserve`, `mem_alias`, `mem_stack`, `app_pool`) |
| **GPU memory on a discrete GPU** | Zero-copy `MTLBuffer` (GPU address = CPU address; unified memory only) | `gfx/vk/mem/*`: pools as buffers backed by imported host memory or physical chunks; **VRAM mirror** of buffer-bound pools by CPU write generations; **shadow copies** of CPU-visible pools on their own thread; write-back at fences; byte-merge; lazy bases; upload batching | `nx-vk::mem` (rewrite of nvn_mtl.m's pool section) | The design-inherent cost the audit flagged; mkw-recomp reaches 60 with it on this GPU |
| **Vulkan recording, pipelines, textures** | Metal (`nvn_mtl.m`) | Pieces as the reference or source: `vk/pipe` (graphics pipeline library, key file + background builds, pairs prewarm, `vk_spirv_patch.c` binding rewrites, alpha test as a SPIR-V rewrite); `vk/exec` (descriptor memos/caches, parallel recording chunks, dynamic state only when it changes); `vk/tex` (images, sparse textures, eviction); `vk/device` (sync with timeline semaphores, latency, present, FSR1); `gfx/cap` (capture/replay design); `shaders/texture/*` (GPU ASTC decode + BC7 encode) | `nx-vk`, `nx-tex` | The structure stays nxnative's (record format, owner, queue semantics); these are the proven Vulkan answers for each part |
| **Windows Switch 2 controllers over BLE** | CoreBluetooth | `input/switch2_win.c` (WinRT BLE; `switch2_proto.h` is nxnative's own) | `nx-input` | Windows transport for nxnative's protocol code |

**Not taken:** mkw-recomp's runtime as a whole (`core/boot`, `sdk/ov`, `audio`, `ui`, `plugin`, `hacks`, `presence`)
and its NVN front end (`gfx/nvn/api`). Those are covered by nxnative's equivalents. MKW-specific tables are not taken
either; MKW uses nxnative's profile `0400C3F00006E000`.

---

## 3. Design details for agents

### 3.1 Translator (`nx-xlate`, `nx-aot`, `nx-cc`)

**Function discovery** (mkw-recomp `gen.py`). Seeds:

- `.eh_frame_hdr` FDEs;
- exported FUNC symbols;
- RELATIVE relocation targets;
- `BL` targets;
- ADR/ADRP+ADD addresses, except jump-table anchors.

A function is `[start, next start)`. A direct branch out of a function makes a new start, until no new start appears.

nxnative adds:

- `rewriter.md`'s data-in-text rules;
- the 26 documented jump-table shapes, as tests of `jumptables.rs`;
- NROs loaded at run time, found in the RomFS at build time (`nro_collect.py` port);
- `discovered.tsv` (functions the dispatcher missed in earlier runs, re-added at the next build).

**Emission.** One C function per guest function, in chunks sized by estimated C. Flags are function-local and the 6
hottest registers are locals. Spin gates and compiler barriers go on call-free back edges.

**Profile patches.** nxnative's cuts and frame-step stubs (27 `patches.py`, plus `sdk_patches.py` by build ID) are
exported once, on the developer side, to `patches.toml`: ARM words plus relocation tokens. `nx-aot` writes them into
the image before lifting, as mkw-recomp's `image_patches`, and `nx-loader` writes the same bytes into the loaded
`.text`, so code that reads itself agrees.

- The patch area is an extra code segment, lifted like any code.
- Original-byte assertions stay.
- Runtime-switchable sets (mkw-recomp `patches.py`: original and replacement both lifted, chosen by `rt_patch_on[]`)
  are used for profile options such as frame step on/off.

**Compile.** Bundled llvm-mingw clang with mkw-recomp's flag set. Chunks are cached by content hash and retried once on
failure (this machine's compile instability). The output is `<name>_code.dll`.

**Verification:**

1. **Port fidelity:** for MKW 1.0.0 (the dump mkw-recomp supports) the Rust port's C must be byte-identical to
   `recomp/gen.py`'s. This proves the port, not the game.
2. **Coverage:** every distinct `.text` word of the seven games' modules lifts. Unsupported words become `rt_udf`; the
   count is reported and must be < 0.01%.
3. **Semantics:** the dynarmic differential lane, in parallel and not gating.

### 3.2 The host-call bridge

nxnative registers HLE functions by Switch mangled name. In nxrs, each registration (`#[hle]`, `#[hle_force]`,
`#[hle_force_direct]`, `hle_alias!`) produces a Rust wrapper `extern "C" fn(c: *mut Cpu)` that:

1. reads x0–x7 (narrow ints truncated as nxnative's comments require) and v0–v7;
2. reads stack arguments from guest `sp`;
3. calls the typed body;
4. writes w0/x0/v0.

The loader binds every import of that name to a guest address in a host-call range, and `nx-cpu` maps that address to
the wrapper. Through that address:

- a direct `BL` in lifted C calls the wrapper directly;
- an indirect call reaches it through `rt_dispatch`;
- NVN's 523 procs (`nvnBootstrapLoader`/`nvnDeviceGetProcAddress`) return such addresses, so the 16k-line thunk file
  is not needed. Per-proc call counts stay in Rust.

**Host → guest calls** (thread entry, callbacks, NRO init, nn::jit plugins) use `call_guest(ctx, addr, args)`, which
syncs MXCSR (`rt_fpcr_sync`) and dispatches.

### 3.3 Memory (`nx-mem`)

- **Reservation:** mkw-recomp's scheme. `VirtualAlloc(MEM_RESERVE | MEM_WRITE_WATCH)` at fixed addresses below 2^39:
  the stack+heap block from 4 GiB up, and the alias region 4 GiB-aligned. Sizes come from the nxnative profile.
- **Commit:** 4 KiB; decommitted pages return zeroed.
- **QueryMemory:** answered from our own region table (nxnative `memory.md`: it must report each module's real layout).
- **GPU visibility:**
  - `GetWriteWatch` on pool ranges replaces nxnative's write-protect dirty tracking (`nvn_dirty.c`);
  - read traps (mkw-recomp `memory_early.c`) make GPU-written bytes visible before the CPU reads them.
- **Aliases:** `MapMemory`, transfer memory and the nn::jit dual view use section views at 64 KiB granularity, with a
  logged copy fallback below that (risk R4).

### 3.4 Shaders: which Switch 2 path

| | nxnative: Ryujinx 1.3.2 + nxnative changes (`dmsh_xlate`, C#) | mkw-recomp: `sm86/` (Python) |
|---|---|---|
| Covers | Maxwell (Switch 1) **and** Ampere/SM86 (Switch 2). 16 Switch 2 games pass Mac gameplay testing, including MKW, DKB and OoT; `shaders.md` holds 3,082 lines of per-game fixes | SM86 only; validated on MKW (11,604 stages); "three of six" known bugs still open |
| Output | SPIR-V. Ryujinx's translator was written for Vulkan; nxnative then goes to MSL, a stage we drop | GLSL → SPIR-V for Vulkan |
| Dependencies | .NET at CI time only (NativeAOT exe) | NVIDIA `nvdisasm`/`ptxas` downloaded on the user's PC to build the instruction spec |

**Decision: nxnative's `dmsh_xlate` is primary for both generations.**

- It is one translator and one binding ABI to adapt for seven games, not two.
- It has the broadest proven game coverage, including all three of the owner's Switch 2 games.
- It needs no NVIDIA tool download at the user's end.

mkw-recomp's `sm86` is kept as a **cross-check** (build-time differential on the same SM86 programs) and a **fallback**
for any SM86 program `dmsh_xlate` rejects. Its binding rewrite stage (`vk_spirv_patch.c`) is used for both paths.

### 3.5 Textures

nxnative's deswizzle and layout math (`nvn_tex.c`, from deko3d under zlib) plus mkw-recomp's GPU ASTC decode and GPU
BC7 encode:

- textures of 256² and larger become BC7;
- smaller ones stay RGBA8;
- sparse textures become sparse images.

An optional on-disk BC7 cache comes later. mkw-recomp measured its offline encoder as slower than the GPU path.

---

## 4. File-by-file mapping

Actions:

- **PORT**: to Rust, same logic.
- **REWRITE**: Metal→Vulkan or Apple→Win32, same responsibility.
- **DONOR**: mkw-recomp code ported or used as the reference for that piece.
- **DATA**: imported as data.
- **TOOL**: external build-time tool.
- **DROP**: not needed, with the reason.
- **DEFER**: no owner game needs it yet; it keeps its row.

`nxrs/PORT_MAP.tsv` holds one row per file of both trees, and a seconds-fast test checks it is complete.

### 4.1 Totals

| Area | Lines | Destination | Mostly |
|---|---|---|---|
| nxnative `dm/host` (non-test) | 28,882 | `nx-kernel`, `nx-hle`, `nx-fs`, `nx-audio`, `nx-video`, `nx-input`, `nx-front`, `nx-loader` | PORT; Apple parts REWRITE; ARM entry code DROP (replaced by the mkw-recomp bridge) |
| nxnative `dm/host/tests` | 15,992 | Crate tests | PORT logic tests; fast by default |
| nxnative `dm/gfx/nvn` | 50,183 (incl. `nvn_thunks.S` 16,360 generated, `nvn_mtl.m` 18,947) | `nx-nvn` (PORT), `nx-vk` (REWRITE with mkw-recomp DONOR) | |
| nxnative `dm/gfx` gl/vk/metal/msl/tex/root | 7,305 | `nx-vk`, `nx-tex`, `nx-hle` | Mixed |
| nxnative `dm/gfx/tools` | 17,259 | `nx-shader`, `nx-profiles` | PORT; MSL parts DROP; `dmsh_xlate` TOOL |
| nxnative `dm/tools` | 10,912 Python + 613 | `nx-aot`, `nx-loader`, `nx-package`, `nx-profiles`, `nx-harness` | PORT; Apple rewrites DROP |
| nxnative `dm/games` | 192 profiles, about 6.5 M lines of TSV | `nx-profiles` | DATA |
| nxnative `scripts`, `docs/reference` | 8,224 / 12,731 | `nx-cli`, `nx-harness` / `nxrs/docs` | PORT / DATA |
| mkw-recomp donor files (sections 2 and 4.7) | about 5.2k Python + about 3k C ported; about 30k C of `gfx/vk` used as reference/source | `nx-xlate`, `nx-aot`, `nx-cc`, `nx-genrt`, `nx-cpu`, `nx-mem`, `nx-vk`, `nx-tex` | DONOR |

### 4.2 nxnative `dm/host`

| File | Lines | → Crate | Action | Note |
|---|---|---|---|---|
| `Makefile`, `.gitignore` | 111 | — | DROP | Cargo |
| `dm.h` | 236 | `nx-common`, `nx-hle` | PORT | |
| `registration.h` | 9 | `nx-hle` | REWRITE | `linkme` slices |
| `hle.h` | 140 | `nx-hle`, `nx-hle-macros` | PORT | Macros generate `Cpu*` wrappers (3.2); the big-stack part dropped |
| `hle.c` | 133 | `nx-hle::registry`, `nx-loader` | PORT | Unimplemented imports bind to a logging host function |
| `entry.S` | 173 | — | DROP → DONOR | Replaced by `rt_svc`/`rt_dispatch`/`hostfn` (mkw-recomp) |
| `thread.c` | 75 | — | DROP → DONOR | Slots are `Cpu` fields |
| `hle_bigstack.c` | 136 | — | DROP | Host code runs on host stacks |
| `hle_svc.c` | 1,099 | `nx-kernel::svc`, `nx-mem` | PORT | Mac VM → `nx-mem` (DONOR mechanism) |
| `hle_thread.c` | 1,446 | `nx-kernel` | PORT + REWRITE | pthread/Mach → Win32 threads, `WaitOnAddress`; mkw-recomp `win32_sync.c` and `kernel_preempt.c` as the Windows reference |
| `hle_guestthread.c` | 63 | `nx-kernel` | PORT | Uses `call_guest` |
| `hle_os.c` | 184 | `nx-hle::os`, `nx-kernel::time` | PORT | Guest clock at the profile's tick rate |
| `hle_table.c` | 16 | `nx-hle::libc` | REWRITE | Per-function Rust shims reading guest args (memcpy, strlen, …) |
| `hle_time.c` | 217 | `nx-hle::time` | PORT | |
| `hle_sf.c` | 176 | `nx-hle::sf` | PORT | |
| `hle_sdkinit.c`, `hle_sdk16.c` | 422 | `nx-hle` | PORT | |
| `hle_oe.c` | 489 | `nx-hle::oe` | PORT | |
| `hle_lifecycle.c` | 49 | `nx-hle::lifecycle` | PORT | Console control handler |
| `hle_account.c`, `hle_account_users.c`, `account_users.h` | 291 | `nx-hle::account` | PORT | |
| `user_select_mac.h` | 26 | `nx-front` | REWRITE | egui |
| `hle_fs.c` | 1,506 | `nx-fs` | PORT | RomFS in place via `nx-package` |
| `romfs_image.h`, `romfs_identity.h` | 181 | — | DROP | iOS RomFS image; we read in place |
| `save_sync.c/.h` | 842 | — | DROP | iCloud |
| `hle_hid.c`, `dm_input.h`, `hle_touch.c`, `hle_haptics.c/.h` | 2,223 | `nx-hle::hid`, `nx-input` | PORT | |
| `haptics_apple.h/.m`, `motion_apple.h`, `motion_pro_mac.h`, `pad_hid_mac.h`, `pad_jc1_mac.h`, `pad_switch2_apple.h/.m` | about 2,800 | `nx-input` | REWRITE | hidapi; WinRT BLE from mkw-recomp `switch2_win.c` (DONOR) |
| `motion_fusion.h`, `motion_pro.h`, `pad_apple.h`, `pad_slots.h`, `pad_hid.h`, `pad_pro.h`, `pad_jc1.h`, `switch2_proto.h` | about 1,400 | `nx-input` | PORT | Pure parsers |
| `ctrl_prompt.h`, `home_quit.h`, `home_quit_mac.h`, `front.h` | 396 | `nx-front` | PORT / REWRITE | |
| `front_mac.m` | 1,816 | `nx-front` | REWRITE | winit + egui; the cheats panel |
| `memstat.m` | 210 | `nx-harness` | REWRITE | `GetProcessMemoryInfo` |
| `crash.c`, `probe.c` | 721 | `nx-cpu::fault`, `probe` | REWRITE | VEH + host PC → guest function map; minidump |
| `loader.c` | 910 | `nx-loader` | PORT | Image patches; imports → host-call addresses |
| `main.c` | 183 | `nx-run` | PORT | `nn::init::Start` order |
| `hle_audio.c`, `hle_audren.c`, `hle_audren_sv.c` | 2,078 | `nx-audio` | PORT (sink REWRITE → cpal) | |
| `hle_codec.c` | 304 | `nx-hle::codec` | PORT | |
| `hle_codec_video.c`, `hle_movie.c`, `hle_movie_av1.c/.h`, `hle_movie_yuv.h`, `hle_nvdrv.c` | 2,132 | `nx-video` | PORT | |
| `hle_movie_avc.c/.h`, `hle_movie_vt.h` | 508 | `nx-video::avc` | REWRITE | Media Foundation (MK8D menu previews) |
| `hle_cheats.c` | 2,103 | `nx-hle::cheats` | PORT | Code writes → recompile is not possible at run time: code cheats need a rebuild with them as image patches (risk R2); data cheats work live |
| `hle_fpslock.c` | 388 | `nx-hle::fpslock` | PORT | |
| `hle_jit.c` | 386 | `nx-hle::jit` | DEFER | Needs run-time translation (no owner game) |
| `hle_ro.c` | 127 | `nx-hle::ro` | PORT | NROs translated at build time |
| `hle_mii.c`, `hle_swkbd.c`, `hle_misc.c`, `hle_friends.c`, `hle_offline.c`, `hle_net.c/.h` | 1,551 | `nx-hle` | PORT | |
| `hle_web.c`, `web_apple.m` | 712 | `nx-hle::web` | DEFER | WebView2 later |
| `hle_oodle.c`, `hle_glslc.c`, `hle_uecvar.c`, `hle_uemalloc.c` | 458 | `nx-hle` | DEFER | Arceus's GLSLC is checked in M6 |
| `hle_count.c`, `hle_fdstat.c` | 94 | `nx-hle::diag` | PORT | |
| `tests/*` (76) | 15,992 | Crate tests | PORT logic tests; DROP `*_mac.m`/`*_apple.m` | |

### 4.3 nxnative `dm/gfx`

| File / dir | Lines | → Crate | Action | mkw-recomp donor for the Vulkan side |
|---|---|---|---|---|
| `README.md`, `nvn_metal_mapping.md` | 296 | `nx-vk/docs` | REWRITE | — |
| `hle_nv.c`, `hle_vi.c` | 215 | `nx-hle` | PORT | — |
| `nvn/hle_nvn.c` | 3,947 | `nx-nvn` | PORT | NVN2 proc/devinfo facts: `gfx/nvn/procs/nvn2.c`, `nvn2_devinfo*.inc` (reference) |
| `nvn/include/nvn.h`, `names.txt`, `nvn_protos.h`, `nvn_count.h` | 3,259 | `nx-nvn` | PORT (generator in `build.rs`) | — |
| `nvn/nvn_thunks.S`, `nvn_entry.S` | 16,451 | — | DROP | `hostfn.c` bridge (3.2) |
| `nvn/nvn_obj.h` | 383 | `nx-nvn::obj` | PORT | — |
| `nvn/nvn_mtl.h`, `nvn_mtl.m` | 19,018 | `nx-vk` (`device`, `pool`, `tex`, `pipe`, `draw`, `queue`, `present`) | REWRITE, keeping its function names and order | `vk/device/*`, `vk/exec/*`, `vk/pipe/*`, `vk/tex/*` |
| `nvn/nvn_rec.h` | 514 | `nx-vk::rec` | REWRITE | `vk/exec/vk_exec_par.c`, `vk/device/vk_rec*.c` (parallel and reused recording) |
| `nvn/nvn_owner.h`, `nvn_owner.m` | 850 | `nx-vk::owner` | PORT (logic) | — |
| `nvn/nvn_queue.h`, `.inc` | 331 | `nx-vk::queue` | REWRITE | `vk/device/vk_sync.c`, `vk_sync_ts.c` (timeline semaphores) |
| `nvn/nvn_dirty.c/.h` | 583 | `nx-vk::dirty` | REWRITE | `vk/mem/vk_mem_gen.c` (write generations), `core/memory/memory.c` (write-watch) |
| Pool handling inside `nvn_mtl.m` | — | `nx-vk::mem` | REWRITE | **`vk/mem/*`**: back, mirror, shadow, wback, merge, lazy, upload, usnap |
| `nvn/nvn_tex.c/.h` | 274 | `nx-tex` | PORT | — |
| `nvn/nvn_hmap.h` | 78 | — | DROP | `HashMap` |
| `nvn/nvn_pace.h`, `nvn_link_drain.h`, `nvn_link_latch.h`, `nvn_fstep.h` | 406 | `nx-vk::present` | PORT | `vk/device/vk_latency.c`, `nvn/host/gfx_present.c` (present on Windows) |
| `nvn/nvn_timestamp.h` | 11 | `nx-nvn` | PORT | — |
| `nvn/nvn_rt.h` | 41 | `nx-kernel` | REWRITE | MMCSS / CPU Sets |
| `nvn/nvn_pso_ds.h`, `nvn_pso_skey.h` | 100 | `nx-vk::pipe` | PORT | `vk/pipe/vk_pipe_keys.c`, `vk_pipe_gpl.c`, `vk_pipe_pairs.c` (key file, GPL, prewarm) |
| `nvn/nvn_refl.h/.m`, `dmsh_abi.h`, `dmsh_chain.h` | 573 | `nx-shader` | PORT | `vk/pipe/vk_pipe_layout.c`, `vk_spirv_patch.c` (bindings) |
| `nvn/msl_image.h`, `host_msl.h/.m` | 80 | — | DROP | — |
| `nvn/nvn_builtin.h` | 22 | `nx-vk::builtin` | REWRITE (GLSL) | — |
| `nvn/nvn_border.h`, `nvn_depth_clear.h`, `nvn_wline.inc` | 128 | — | DROP | Vulkan has custom border colours and wide lines |
| `nvn/nvn_primitive.h` | 29 | `nx-vk` | PORT (quads) | — |
| `nvn/nvn_tess.inc`, `nvn_geometry.h/.inc`, `nvn_feedback.h/.inc` | 2,016 | `nx-vk` | REWRITE (smaller: native Vulkan tessellation, geometry shaders, transform feedback) | `vk/exec/vk_mesh_cs.c` (mesh-as-compute fallback) |
| `nvn/nvn_accel.*`, `nvn_raytrace*` | 710 | `nx-vk::rt` | DEFER | — |
| `nvn/nvn_ndve.inc` | 255 | `nx-vk::ndve` | REWRITE (compute) | — |
| `nvn/nvn_capture.c/.h/.inc` | 869 | `nx-vk::capture`, `nx-harness` | PORT | `gfx/cap/*` (replay without the game) |
| `msl/*.metal` (17) | 632 | `nx-vk/shaders` | REWRITE (GLSL) | `runtime/shaders/*` (FSR1, HDR, …) |
| `metal/` | 442 | — | DROP | — |
| `gl/`, `vk/` (GL and Vulkan-API games) | 5,334 | — | DEFER | — |
| `tex/nvn_deswizzle.c/.h` | 158 | `nx-tex` | PORT | `gfx/nvn/tex/tex_par.c` (parallel deswizzle) |
| `tex/nvn_deswizzle_gpu.*` | 110 | `nx-vk::tex` | REWRITE | — |
| `tex/nvn_fmt_mtl.c/.h` | 98 | `nx-tex::fmt` | REWRITE | `vk/tex/vk_formats.c` |
| ASTC (Apple native) | — | `nx-tex`, `nx-vk::tex` | NEW from DONOR | `shaders/texture/astc_decode.comp`, `bc7_encode.comp`, `bc1_encode.comp`, `vk/tex/vk_astc.c`, `vk/tex/vk_sparse.c` |
| `tests/` | 49,656 | `nx-vk/tests` | PORT selected; DROP Metal probes | — |

### 4.4 nxnative `dm/gfx/tools`

| Files | Lines | → | Action |
|---|---|---|---|
| `shader_split.py` | 3,902 | `nx-shader::split` | PORT |
| `dmsh_build.py`, `dmsh_reflect.py`, `dmsh_entry.py`, `dmsh_side.py`, `dmsh_vin.py`, `dmsh_abi.py`, `dmsh_facts.py`, `dmsh_fetch.py`, `dmsh_dryrun.py` | 2,659 | `nx-shader` | PORT (MSL stages removed) |
| `dmsh_xlate/` + `dm/third_party/ryujinx` | 365 + 75,901 C# | `tools/dmsh_xlate` | TOOL (NativeAOT, built in CI) |
| `dmsh_rewrite.py`, `dmsh_alpha.py`, `dmsh_atomic.py`, `dmsh_logic.py`, `dmsh_wide.py`, `dmsh_ampere.py` | 1,249 | `nx-shader::fixes` | PORT those still needed on Vulkan; DROP Metal-gap fixes |
| `dmsh_geometry.py`, `dmsh_mesh.py`, `dmsh_meshstage.py`, `dmsh_tess.py` | 1,239 | `nx-shader` | REWRITE (smaller; Vulkan has these stages) |
| `dmsh_rt*.py` | 876 | — | DEFER |
| `dmsh_chain.py`, `dmsh_chain.lock`, `tool_lock.py`, `own_compilers.py`, `kit.py` | 364 | `nx-shader::chain` | PORT |
| `dmsh_pull.*`, `dmsh_mtlcache.py`, `dmsh_pch.py`, `host_msl.py`, `spirv_cross_bin/`, `tbdr_*.py` | about 740 | — | DROP (Metal) |
| `pso_offline.py` | 908 | `nx-shader::pso` | REWRITE (Vulkan key file + pairs, DONOR `vk_pipe_keys.c`/`vk_pipe_pairs.c`, `tools/shaders/pipeline_pairs.py`) |
| `gen_protos.py` | 72 | `nx-nvn/build.rs` | PORT |
| `ampere_orin/*` | 930 | `nx-shader/tests` | PORT |
| `vk_shaders.py`, `gl_shaders.py`, `gl_aliases.py`, `idtech4_programs.py`, `mvgl.py` | 824 | — | DEFER |
| `c1_guard.py`, `cb_extent.py`, `shader_regressions.py`, `tex_census.py`, `passgpu_top.py` | 699 | `nx-harness` | PORT |
| `casc.py`, `cpk.py`, `cra.py`, `d3cpk.py`, `frostbite.py`, `lspk.py`, `rpf6.py`, `smash_arc.py`, `uepak.py`, `unityfs.py`, `vbf.py` | about 2,600 | `nx-shader::containers` | DEFER |

### 4.5 nxnative `dm/tools`

| Files | Lines | → | Action |
|---|---|---|---|
| `patch_nso.py` | 1,856 | `nx-aot::patch`, `nx-loader` | PORT the cut, frame-step and observation-gate machinery as image patches. DROP the Apple rewrites (x18, tpidr, svc/cntpct gates, sp alignment, exclusive ordering): the lifter handles them. **Keep the Trinity rendezvous fix** (`trinity_completer_sites`): the original code stores a flag, then loads a state word, with no barrier, and x86 also reorders a store before a later load. Plain lifted accesses are not atomics, so `nx-aot` detects the same site and emits a `SEQ_CST` fence there. This is verified on Violet and Z-A in M6 |
| `sdk_patches.py`, `nso_match.py` | 624 | `nx-profiles` | DATA / PORT |
| `jump_solver.py` | 704 | `nx-aot` tests | REF: mkw-recomp's `jumptables` is the implementation; nxnative's cases are its tests |
| `nso_asm.py` | 33 | `nx-profiles` exporter | PORT (encoders) |
| `nso.py`, `nso_code.py`, `nso_fnmap.py`, `nso_syms.py`, `nso2elf.py`, `sdis.py` | 581 | `nx-loader`, dev tools | PORT |
| `insn_census.py`, `loader_census.py`, `coverage_report.py` | 329 | `nx-aot` | PORT (with mkw-recomp `lift_coverage.py`) |
| `nro_collect.py` | 164 | `nx-aot` | PORT |
| `jit_capture.py` | 283 | — | DROP (iOS) |
| `romfs.py`, `nacp.py` | 378 | `nx-package` | PORT |
| `romfs_image.py`, `tree_tar.py`, `vol_space.py` | 403 | — | DROP (iOS packaging) |
| `nx_extract/` | 574 + binary | `nx-package` | REWRITE (Turbo's in-place code) |
| `shader_cache.py`, `shader_lib.py` | 788 | `nx-shader` | PORT |
| `make_shared_fonts.py`, `make_ext_font.py` | 206 | `nx-hle::pl` | PORT |
| `ue_*.py`, `unity_*.py`, `il2cpp_sym.py`, `snowdrop_pools.py` | 1,240 | `nx-profiles::engines` | DEFER |
| `fps_target.py`, `monolith_fps.py` | 316 | `nx-profiles` | PORT |
| `check_thread_cuts.py`, `audio_channel_check.py`, `gpuprof.py`, `triage_report.py`, `usage.py`, `proc_group.py`, `run_saves.py` | 1,307 | `nx-harness` | PORT as needed |
| `docs_patches.py`, `docs_symptoms.py`, `engine_patches.py`, `env_switches.py`, `issue_areas.py`, `pr_areas.py`, `impact_map.tsv` | 966 | `nx-harness::docs` | PORT |
| `lint_*.py` | 705 | — | DROP (clippy) |
| `nvn_native_sizes.py`, `nvn_slots_got.py` | 351 | `nx-nvn` tests | PORT |
| `ghidra/`, `native_tools.sh` | 98 | — | DROP |
| `fuzz/` | 657 + corpus | `cargo-fuzz` | PORT the corpus |
| `tests/` | 25,379 | Crate tests | Re-expressed |

### 4.6 nxnative `dm/games`, `scripts`, platform dirs, docs, third_party

| Item | → | Action |
|---|---|---|
| `dm/games/<id>/` × 192 (README, `game.tsv`, `shader_*.tsv`, `pso_*.tsv`, `bf_want.tsv`, …) | `nx-profiles/games/` | DATA |
| `patches.py` × 27 | `patches.toml` | DATA via a dev-side exporter |
| `game.py`, `settings.py`, `settings.tsv`, `profile_guess.py` | `nx-profiles` | PORT |
| `scripts/build_ipa_from_nsp.py` | `nx-cli build` | PORT the pipeline; DROP IPA |
| `scripts/pack_nup.py`, `pack_dump.py` | `nx-package` | PORT |
| `scripts/regress.py`, `bench.py`, `bug_report.py` | `nx-harness` | PORT |
| `scripts/cheat_anchor.py` | — | DEFER |
| `scripts/clean_metal_cache.py`, `push_romfs.sh` | — | DROP |
| `dm/mac`, `dm/ios`, `dm/sanitizers` | — | DROP |
| `docs/reference/*.md` (13 files) | `nxrs/docs/reference` | DATA + extend in the same symptom/cause/fix format |
| `docs/*.md`, `env_switches.tsv`, `agents/` | `nxrs/docs` | REWRITE / DATA |
| `third_party/dav1d`, `libvpx`, `stb_vorbis`, `flip`, `vulkan-headers`, `macnativegames` | `nx-video`, `nx-harness` | `dav1d` crate; own `vpx-sys`; `lewton`; FLIP via `cc`; `ash`; notices kept |
| `third_party/ryujinx` | `tools/dmsh_xlate` | TOOL |

### 4.7 mkw-recomp donor files (only these)

| mkw-recomp file | Lines | → | How |
|---|---|---|---|
| `recomp/lifter.py`, `simd_c.py`, `flags.py`, `regs.py`, `jumptables.py` | 3,587 | `nx-xlate` | PORT (byte-identical C) |
| `recomp/gen.py`, `nso.py`, `module.py`, `layout.py`, `patches.py` | 1,541 | `nx-aot` | PORT. `layout.py`'s base becomes per-profile |
| `recomp/versions.py`, `patches-mkw*.txt` | — | — | Not taken (profiles come from nxnative). `patches-mkw.txt`'s spin list is a cross-check for nxnative's MKW profile |
| `tools/prepare/gamecode.py`, `fastcc.py` | — | `nx-cc` | PORT |
| `tools/cpu/spin_waits.py`, `lift_coverage.py` | — | `nx-aot` | PORT |
| `runtime/cpu/cpu.h`, `simd.h`, `simd_fp.h`, `fma.c` | 1,863 | `nx-genrt` | Used as C (header comments removed; credited in the README) |
| `runtime/cpu/dispatch.c/.h`, `hostfn.c`, `sjlj.c` | 705 | `nx-cpu` | PORT |
| `runtime/core/kernel/kernel_fiber.c` | 169 | `nx-cpu`/`nx-kernel` | PORT (fiber switch only) |
| `runtime/core/kernel/kernel_preempt.c`, `platform/win32_sync.c`, `win32_sys.c` | about 1,500 | `nx-kernel` | REF for Windows thread mechanics under nxnative's scheduler |
| `runtime/core/memory/memory.c`, `memory_early.c`, `platform/win32_mem.c` | 1,505 | `nx-mem` | PORT the mechanism; layout from nxnative profiles |
| `runtime/gfx/vk/mem/*` | 6,151 | `nx-vk::mem` | DONOR (source for the discrete-GPU pool model) |
| `runtime/gfx/vk/pipe/*` | 4,265 | `nx-vk::pipe` | DONOR (GPL, key file, prewarm, SPIR-V binding patch) |
| `runtime/gfx/vk/exec/*` | 9,328 | `nx-vk::rec`/`draw` | REF + selected code (descriptor memo, parallel chunks, dynamic-state diffing) under nxnative's record format |
| `runtime/gfx/vk/tex/*`, `vk/device/*` | 12,514 | `nx-vk` | REF + selected code (sparse, ASTC, sync, latency, FSR1) |
| `runtime/gfx/cap/*` | 1,383 | `nx-vk::capture` | REF (nxnative's capture format stays) |
| `runtime/shaders/texture/*`, `fsr/*`, `ffx/*` | about 4,200 | `nx-vk/shaders` | Used as GLSL |
| `runtime/input/switch2_win.c` | 669 | `nx-input` | PORT (WinRT BLE transport) |
| `sm86/` | 5,729 | `tools/sm86` | Cross-check/fallback only (3.4), run with bundled Python; no port planned |

---

## 5. Milestones

**Measurement rules:**

- The guard holds: one game, no compiles.
- 10 minutes; the time-weighted mean of `[vk] pacing:` windows at t ≥ 60 s; PresentMon as a second ruler.
- "Warm" = the second run after a build.
- "Max" = the largest window maximum.
- Run folders are hashed.

**Lanes:**

- **T** (translator, CPU bridge, oracle)
- **O** (OS, HLE, package, loader)
- **G** (graphics, shaders, harness)

**First game: Mario Kart 8 Deluxe.** Among the owner's games it is the one nxnative supports best and the easiest to
bring up:

- 60 FPS on the iPhone with 6 ms of GPU time;
- a 1,518-row pipeline manifest (OoT: 25,170);
- NVN1, no mesh shaders, SDK 4.4.0;
- Switch 1's 3 cores and 19.2 MHz clock;
- Turbo got it from a boot crash to 60 FPS in one round.

The Switch 1 games follow, then the Switch 2 games.

| # | Milestone | Exit criterion (all must hold) | Week |
|---|---|---|---|
| **M0** | Foundations | Workspace and CI (`cargo test` < 60 s); `PORT_MAP.tsv` complete for both trees; `nxrs --update-info` byte-identical to Turbo's; the seven games' package listings equal Turbo's; 192/192 profiles parse; 27/27 `patches.py` export | 1–2 |
| **M1** | **Translator** (days) | (a) `nx-aot` C for MKW 1.0.0 byte-identical to mkw-recomp `gen.py` (port fidelity). (b) Every module of the seven games lifts: `rt_udf` words < 0.01%, reported per class. (c) All compile with the bundled clang into `<name>_code.dll`; MK8D cold build ≤ 5 min on the 285K; one changed chunk rebuilds in ≤ 30 s | 1–2 |
| T-lane | dynarmic differential (parallel, not gating) | Every lifter class used by the seven games: 2,000 cases per class in the default run (seconds), 1 M nightly; mismatches filed | 1–6 |
| **M2** | Execution and the bridge | libnx homebrew NROs (hello, threads + mutex + condvar, C++ exceptions, `setjmp`/`longjmp`, float `printf`, atomics, TLS, fiber switch) produce their expected output through `nx-cpu` + the bridge + minimal `nx-kernel` | 2–4 |
| **M3** | **OS + HLE boot (nxnative)** | MK8D runs from `nninitStartup` to its first NVN queue submit; no `UNIMPL` line beyond those nxnative's log shows for the same build; 5 min without a crash with the null `Replay` | 3–7 |
| **M4** | **First pixels on Vulkan** | MK8D title screen and main menu on Vulkan, matching a reference capture (FLIP mean < 0.05, plus the owner's visual check). Pool placement logged with write-back ms/frame | 6–10 |
| **M5** | **MK8D at 60** | GP race, 10 min warm: mean ≥ 59.0, no frame > 100 ms, ≤ 3 frames > 33 ms; cold run no frame > 1 s; no audio underruns; menu H.264 previews play; Booster Course DLC track loads; save survives a relaunch | 9–13 |
| **M6** | Pokémon trio | **Arceus**: Obsidian Fieldlands 10 min warm, mean ≥ 29.5, no frame > 100 ms. **Violet**: past "Choose a look" into the first town, 30 min without a crash, mean ≥ 29.5, no frame > 150 ms. **Z-A** (the Switch 2 Edition program; else the Switch 1 profile first): Lumiose 10 min warm, mean ≥ 59.0 with the 60 FPS profile patch, no frame > 100 ms | 12–19 |
| **M7** | Switch 2: MKW + DKB (NVN2, Ampere shaders via `dmsh_xlate`, mesh shaders) | **MKW**: race 10 min warm, mean ≥ 59.0, no frame > 100 ms. **DKB**: opening and mining-tunnel areas 10 min warm, mean ≥ 59.0, ≤ 5 frames > 50 ms, no frame > 100 ms | 15–24 |
| **M8** | OoT | Kokiri Forest from a save, 10 min warm: mean ≥ 59.0, no frame > 100 ms; 30 min without a crash; private memory ≤ 16 GB; brightness matches the reference (FLIP < 0.05) | 20–30 |
| **M9** | Launcher contract v2 | Build, play, update and DLC through nxrs for MK8D and one Switch 2 game, with a per-game engine choice | 6–12 |
| **M10** | Linux | M5 on Linux | after M8 |

**Probabilities** (honest):

| Milestone | Probability |
|---|---|
| M1 by week 2 | 85% |
| M3 by week 7 | 70% |
| M5 by week 13 | 60% |
| M6 by week 19 | 55% |
| M7 by week 24 | 50% |
| M8 by week 30 | 40% |

---

## 5A. The GAPS phase: Turbo parity and unfinished lane work

Runs now, in parallel with M4. Sources: the owner's fork commits (`git log upstream/main..turbo` in
`D:\Projects\Switch2RECOMP-main`, 104 commits), Turbo's runtime (`runtime/*.c`, `runtime/nvn/vk/*`, the option table in
`runtime/main.c`, hotkeys in `runtime/backend_win32.c`) and `switchrecomp.py`'s command line, plus what lanes A, B and
C reported as not done. For each item the agent **reads Turbo's implementation first** and ports it (the owner's files
copied and translated to Rust, MZ Labs' reimplemented), or nxnative's where nxnative has the better one. Each item is
done only when it works in a real game run (rule 1.1.6), and the agent marks it Done here with the commit and the evidence.

Lanes: **T** = translator, runtime, cli, package. **O** = os services, audio, video. **I** = input, window, front,
harness. **G** = graphics, shaders, and M4.

### 5A.1 Package and build (lane T)

| Item | Turbo source | Status |
|---|---|---|
| XCI (and its secure partition) as the game's own package, not only as update/DLC. The README's "refused, as in Turbo" is wrong | `nca.py` `open_ncas`, `dlc.members` | Done (uncommitted, lane T): XCI with or without key area; NCA compressed storage added (the owner's only XCI, Super Mario Party Jamboree, needs it): ExeFS + 1,539 RomFS files read, all 139 NROs match the game's NRR hashes. Also homebrew `.nro` input. Not built and run as a game |
| ZBIC-compressed NSO segments (Switch 2: Ocarina, and any other); **one** decoder shared by translator and shaders (shaders has its own today) | `zbic.py`, `nxloader.py` (3eac6c2, b196d14) | Done (uncommitted, lane T): `translator::loader::zbic`, shaders re-exports it. OoT translates (297,043 functions), builds with shaders (42,848) and runs: main staged unpacked, loaded, DT_INIT runs; then an nn::diag abort in sdk init after GetInfo 29/30/33 (os) |
| The 6 untyped SDK crypto symbols seeded (os `bignum.rs` workaround then removed) | — | Done (uncommitted, lane T): seeded; MK8D's six are lifted functions. Removing `bignum.rs` is left to lane O (it is editing `services/mod.rs`; INTERFACES.md) |
| `nxrs build` runs the shader prebuild; profiles' shader tables imported | lane C `shaders::prebuild` | Done (uncommitted, lane T): MK8D 29,651 shaders at build, the game ran on them at ~60 FPS; 187 profiles' tables embedded (24.7 MB), written to `<data>\shaders\profile\`; nxrs now links `nxfront.lib` |
| `--key=value` applies any ini option for one run | e4b388d | Done by lane I (`nxconfig`); lane T: argv passes through, the runtime skips `--k=v` when finding its folder, and applies `memory_gb`, `null_trap`, `log`, `profile` |
| Settings versions and their migrations (`settings_version` 3: `game_dynamic_resolution` auto) | 01f9119, 81a1d95 | Done by lane I (`nxconfig`); MK8D's ini rewritten at version 3 in lane T's run |
| Per-game memory cap (job object), allocations counted | 8a4618c | Done (uncommitted, lane T): `process memory cap: 32768 MB` in MK8D and OoT runs, raised to the pool + 8 GB for memory_gb. Counting vkAllocateMemory is lane G's |
| Every `switchrecomp.py` option has its nxrs equivalent or a launcher-agreed replacement (`--chunk`, `--keep-c`, `--no-shaders`, `--dlc-stub`, …) | `switchrecomp.py` | Done (uncommitted, lane T): `--chunk`, `--native`, `.nro` input added; the rest as listed in src/cli/README.md |
| `doctor`: Vulkan device version, free disk | — | Done (uncommitted, lane T): devices with API/driver version (1.3 needed), free disk, toolchain, Visual Studio, engine libraries |
| Toolchain for machines without Visual Studio (runtime for `x86_64-pc-windows-gnullvm`, llvm-mingw bundled) | a1c5d81 (Turbo bundles its compiler) | Done (uncommitted, lane T): `NXRS_TOOLCHAIN=gnu` / no Visual Studio → bundled llvm-mingw 20261104 + the engine built for gnullvm (`gnu\libnxfront.a`); MK8D built so imports only system DLLs and the UCRT and ran 60 s at 60 FPS; homebrew tests pass. Release packaging (bundling llvm-mingw + gnu\) is M9 |
| Exclusive monitor on for Switch 1 programs only; LDAR/LDAPR/STLR/LDXR as atomic accesses — confirm the translator's equivalents | b89e9e5, efa9b2a, e440705 | Done (uncommitted, lane T): global monitor (`rt_excl_mon`) on for MK8D, off for OoT (run logs); LDXR/LDAR/LDAPR acquire, STLR now seq-cst |
| Runtime: the 5 requests os filed (SVC hook, tick rate incl. Switch 2 tick, scheduler hooks, start entry, kernel events), os workarounds removed | `docs/INTERFACES.md`, 78ece7b | Partial (uncommitted, lane T): all five in the runtime (INTERFACES.md), tick 19.2 / 31.25 MHz in the runs; removing os's workarounds is lane O's |
| Map past the application pool fails with ResultLimitReached; SDK 22 thread ID; WaitIfEqual64 | b316a9f, 78ece7b, 362b172 | Done in the runtime (uncommitted, lane T): map limit on for Z-A/OoT (OoT log), TLS+0x110, WaitForAddress type 3; os answers memory SVCs itself (INTERFACES.md note to lane O) |

### 5A.2 OS services, audio, video (lane O)

| Item | Turbo source | Status |
|---|---|---|
| Audio to a real output device (WASAPI); `audio_slowdown`; no underruns | `runtime/audio.c`, `audio_queue.h`, `audio_limit.h` | Done: MK8D on WASAPI 68.5 s, 67.6 s not silent, 0 underruns, limiter + smooth slowdown (`os/src/media/sink.rs`, `services/audren.rs`) |
| hwopus decoding through libopus (Turbo decoded silence before c406ae6) | c406ae6, `runtime/opus/` | Partial: libopus 1.5.2 linked, classes / C interface / SDK 21 manager / `hwopus` IPC (`services/codec.rs`), unit-tested; no built game uses it yet (Ocarina needs ZBIC) |
| Renderer mix fixes: a voice reaching no mix stays audible; mixes kept between updates; splitters | 8a265ab, a22b102 | Partial: the SDK's own renderer mixes in-process (no host mixer, so mixes / splitters are the SDK's), MK8D audible; Ocarina's unrouted voices unverified (no build); audren:u for SDK 1.x not served |
| Video: VP9 (Switch 2 GSV, libvpx), AV1 (dav1d, nn::ndve), H.264 (menu previews), the Switch 2 video decoder's fences and back-pressure; Ocarina's movies on screen | f9c8469, 6c4578c, 5093a7d, 0ae160a, 9ce9b8e, `runtime/vp9`, `av1`, `oh264` | Partial: MK8D menu previews H.264 (Media Foundation) 926/926 pictures decoded, not black, handed to the game; panel still draws black (lane G, INTERFACES); VP9 / AV1 / Switch 2 decoders ported (`services/movie.rs`, `codec_video.rs`), not run (no Switch 2 build) |
| AudioIn opens (silent capture device is not acceptable: a real device or the service's own "no device") | — | Partial: real capture device through WASAPI, "no device" when none (`services/audio.rs`, `media/mic.rs`); no built game opens it |
| Services Turbo answers that os may not: `btm:u` interface (Violet 4.0.0), MiiEdit applet cancel, bsd event descriptors, time shared page entries 0x38, the game's SDK version read for the NVN API | 13a2693, 9ae9441, 362b172, 1f12977, 2f8f44e | Partial: SDK version done (MK8D `sdk: NintendoSDK 4`, boots on Vulkan); btm:u and bsd event descriptors served (`services/net.rs`), not exercised; MiiEdit by name (AppendMii) as nxnative, applet IPC path absent; time shared page not served |
| Software keyboard (`keyboard_text`, the swkbd font) | `runtime/swkbd_font.h`, `svc_ns2.c` | Partial: ShowKeyboard + InlineKeyboard with a Win32 text box in the Hind font, `keyboard_text` default (`services/swkbd.rs`); not exercised by a built game |
| Docked / TV mode by default; `docked`, `language` options | d5b36a1 | Done: from `nxconfig`, live docked change notifies the game; MK8D `oe: docked, TV mode, language en-US` |
| Delete the null NVN device; os hands NVN to graphics (with lane G) | rule 1.1.6 | Done: `nvn/null.rs`, `tex.rs` deleted; MK8D runs to its menus on Vulkan (407 graphics procs) |

### 5A.3 Input, window, front end, harness (lane I)

| Item | Turbo source | Status |
|---|---|---|
| Real controllers in `hid`: XInput, DualSense / DS4 (lights, rumble, motion), Switch Pro, Joy-Con (1 and 2), WinMM fallback; hot-plug; several players | `controller_sony*.h`, `svc_hid.c`, `hid_layout.h`, 88401aa, a0581fe; nxnative `pad_*.h`, `switch2_proto.h`, `motion_fusion.h` | Done: all paths written and unit-tested (XInput; DualSense / DS4; Switch Pro, Joy-Con 1, NSO pads; Joy-Con 2 / Pro Controller 2 over BLE; WinMM; hot-plug; 8 players). DualSense (USB, fw 0630) on hardware: player 1, input and motion streaming, output reports byte-identical to Turbo's, buttons and rumble (low, high, both) confirmed by the owner by hand 2026-11-19 (`nxinput rumble`); MK8D's own rumble reaches the pad (1,138 vibration calls, motor bytes up to 147/255). Pad kinds without hardware here (Xbox, DS4, Switch Pro, Joy-Con 1 / 2) are fixture-tested only |
| `button_layout` (switch / xbox / positional) and `button_map` | 08130c4, 10e1ef4, ac1bd36, `pad_layout.h` | Done: the owner's `pad_layout.h` ported with tests, applied to every pad path; the owner's presses on the DualSense map as expected (`nxinput read`, 2026-11-19); the game read the mapped buttons in MK8D |
| Keyboard and mouse as a controller: `mouse_right_stick`, `mouse_sensitivity`, `mouse_invert` | `backend_win32.c` | Partial: keyboard verified in MK8D (the game read A from X, the stick from W); the mouse stick (raw input, sensitivity curve, inversion) is written and unit-tested, not exercised in a game |
| Game window: fullscreen / windowed, F11, title bar state, Esc quit, F1 help; windowed loader exe (no console) | `backend_win32.c` | Done: MK8D windowed, F11 / Alt+Enter fullscreen and back (kept in the ini), title states, F1 page, Esc quit; loader is a GUI-subsystem exe |
| Hotkeys: F3 screenshot (`captures/`), F4 fps limit, F7 recording (`smooth_recording`, `recording_fps`) | `backend_win32.c`, `rec_win32.c`, `rec_flow.h` | Done: F3 (PNG of the game), F4 (`fps_limit` + title), F7 (H.264 + AAC MP4; `smooth_recording` slowed the clock to 0.30 and kept every frame distinct, A/V lengths 1.97 / 2.07 s), F8 (graphics' shot) verified in MK8D |
| The ini: every Turbo option read and honoured (`renderer`, `resolution`, `dynamic_resolution(_min)`, `game_dynamic_resolution`, `shadows`, `fps_target` incl. frame skip 30/20 and 120 = LSFG x2, `fps_limit`, `forced_vsync`, `memory_gb`, `driver_exclusive`, `shader_thread`, `low_priority_shader_compiler`, `lsfg*`, `lossless_scaling`, `lossless_path`, `log`, `diag`, `profile`, `metrics`, `null_trap`) and written with comments when missing | `runtime/main.c` option table | Partial: every option read, normalised, published (`nxconfig`), written with comments, migrated, reloaded live, `--key=value` (all verified in MK8D); applied today: front / input (`fullscreen`, `mouse_*`, `button_*`, `smooth_recording`, `recording_fps`, `lsfg*`, `lossless_*`, `metrics`, `diag`), lane O (`docked`, `language`, `keyboard_text`, `audio_slowdown`), lane G (`fps_limit`, `forced_vsync`, `shadows`, `game_dynamic_resolution`); still unread by their lanes: `resolution`, `dynamic_resolution(_min)`, `fps_target` frame skip, `driver_exclusive`, `shader_thread`, `low_priority_shader_compiler` (G), `memory_gb`, `null_trap`, `log`, `profile` (T / O) |
| Frame generation through the owner's Lossless.dll (LSFG) | `runtime/lsfg.c` | Partial (owner decision 2026-11-19: Eglantine's design is the default): **built-in frame generation** on the presented frames in graphics (`vk/framegen.rs`, own shaders `fg_luma` / `fg_flow` / `fg_interp`: luma pyramid, coarse-to-fine block matching, warp from both neighbours with occlusion fallback; pacer with target / multiplier / credit / hitch hold; generated frames presented before the game's frame, spaced by their own thread). **LSFG mode** (`frame_gen_type=lsfg`): the player's Lossless.dll passes (front's set handed to graphics) run as Vulkan compute with push descriptors (`vk/lsfg.rs`, lsfg-vk's pass graph), falling back to the built-in mode when the DLL is missing or fails. Options: `lsfg`, `frame_gen_type` (new), `lsfg_target`, `lsfg_multiplier`, `lsfg_flow`, `fps_target=120`. MK8D: built-in base 60.1 → output 119.5 FPS (`build/G/runs/r25`), LSFG (the owner's Lossless.dll from Steam) base 59.0 → output 117.2 FPS (`r26`), a generated LSFG frame `build/G/runs/r23/fg-generated.png` beside its game frame `fg-current.png`; both runs overlapped other lanes' builds, so their slow stretches are not frame generation's |
| Control pipe (scripted actions from the harness) | 0b0d902, d54f04e | Done: `\\.\pipe\nxrs_ctl_<pid>` + registry; status / press / pads / shot / quit verified in MK8D |
| Metrics, dropped-work counters, input record / replay | 8f5410f | Done: metrics CSV per second (verified), counter registry for every crate, frames CSV sink; NXRS_INPUT_RECORD / REPLAY verified in MK8D (replayed states read by the game) |
| Harness: guard (GAME_LOCK, memory watchdog), scenarios with frame-time statistics, per-run ini, the pacing line at least every 5 s, PresentMon | dec5a09, 4f0f731, `tools/play_scenario.py` | Done: run_game.py guard as a module (waits for builds: cargo / rustc / clang / nxshaders / nxrs), play_scenario.py (Turbo's scenarios, per-run ini, stats), gamectl.py; PresentMon 2.6.0 (Intel-signed, build/tools/presentmon) records the game unelevated: MK8D scenario 1,147 presents, 45.7 FPS, p50 19.0 / p95 31.7 / p99 87.5 ms over 100-140 s; pacing gaps ≤ 5.07 s |

### 5A.4 Graphics (lane G)

| Item | Turbo source | Status |
|---|---|---|
| Wire os → graphics; MK8D title screen and menu (= M4) | — | Done (`build/G/m4/mk8d-title.png`, `mk8d-main-menu.png`) |
| Virtual / sparse pools | `vk_sparse.inc` | Partial (lane G, uncommitted): virtual pools as before, and **Vulkan sparse images** for sparse textures of GPU-only sparse pools (`vk/sparse.rs`, mkw-recomp's `vk_sparse.c` ported: 64 KiB slots per physical page, aliased, binds before each submission on a sparse timeline, levels outside the tiles bound once). MKW: 31 sparse images (three 8192x8192x68 terrain arrays, 1.4-5.8 GB of address space each) with 1,663 MB of tiles bound; MKW used to be killed at 28 GB private 1 s after its first submit (it made 19 GB of whole images), now peaks at 8.4-8.9 GB and presents (`build/G/runs/w3`..`w7`; it then stops in game code: INTERFACES lane G notes). Pages of the address map skip sparse-pool textures (MKW's first frame took 76 s scanning them) |
| Buffer textures, query counters with the GPU's own time, append counters as indirect-draw arguments | 8097152, 7b5a28c | Partial: all three implemented (`vk/counters.rs`: timestamps stamped on the GPU and brought to the guest clock, SAMPLES_PASSED from occlusion queries; texel buffer views per draw and in the pool set; append resets + size-0 storage buffers) — MK8D runs with SAMPLES_PASSED counted (no red), GPU time logged; buffer textures and append counters not exercised by MK8D, DKB not runnable yet |
| Transform feedback, layered passes, raw global memory through loaded pointers | 78ece7b | Partial (lane G, uncommitted): **layered passes** done (a pass has as many layers as its attachments' views; 3D targets are 2D-array compatible and drawn as arrays of slices, they were bound as 3D views); **transform feedback** replayed: the owner's capture variant (`vk/xfb.rs`, from Turbo 78ece7b: XfbBuffer / Stride / Offset on the captured outputs, narrower ones split out) bound with the feedback buffers around each draw, counters carry a capture over several draws (unit-tested; no running game captures yet: Z-A / Violet not built); raw global memory not yet |
| Recording on several threads; parked submissions at GPU event waits without red log lines | 362b172, 1316538 | Partial (lane G, uncommitted): **asynchronous submission** (Turbo's translation thread): submits and presents replayed on a `vk replay` thread, fences as op sequence numbers, SyncWait / ReadTexels / Finish drain (`vk/queue.rs`); MK8D runs on it (r19, r20). **Parked submissions without red lines:** a queue waiting on another parked queue's sync parks behind it (`nvnQueueWaitSync` receipts), a present on a parked queue is shown when its work has run, parked queues are retried after every replayed op (both red lines gone). Recording one submission on several threads: not done |
| Write-back on demand, joined ranges (discrete-GPU model) | 6e86483, ad91c4d | Partial: implemented (`vk/lazy.rs`: pages with 3 frames of GPU writes go PAGE_NOACCESS instead of being copied back, a fault reads the page back on a copy of its own; adjacent write-backs joined in `mem::note_gpu_write`); MK8D runs with it on, its write-backs do not reach the streak; DKB (the case it is for) not runnable. **Off by default since rev 6** (`NXGFX_WB_LAZY=1`): it crashed Violet (~35 s) and Z-A (~60 s). The race behind that (a second thread faulting on a page while it was being read back) is fixed; turn it back on once a Pokémon run is clean with it |
| A draw whose shader is missing waits for a live translation instead of being skipped | 336dcb7 | Done (`vk/live.rs` + `shaders::prebuild::translate_one`, 4 translator threads, waited-for keys first): MK8D with an empty shader folder translated 1,821 shaders live (0.1 s each) and drew its title (`build/G/runs/r9`). That run lost the device at 17 s, not diagnosed |
| Storage use for images without the IMAGE flag (off by default), integer output variants, storage textures | caa07ad, 3d77358 | Not started |
| Push constants as driver bank 24; memory pool table 8192; queues / texture pools / mesh draws as Ocarina needs | 9ce9b8e, 4bb959d, 9f19968 | Not started |
| Live alpha-to-coverage switch; per-title texture rewrites; present ticket race fix | 0b0d902, 362b172, a22b102 | Partial: `a2c 0|1` control command (and `NXGFX_A2C=0`); the ticket race cannot happen here (a present's ticket is written by the presenting thread, there is no translation thread); per-title texture rewrites not done |
| Resolution scale, dynamic resolution (host and the game's own), shadows option, vsync, fps limit | `render_scale.h`, 81a1d95 | Partial: `fps_limit` (0 / 30 / 60..240), `forced_vsync`, `shadows=0` (depth-only passes of their own size skipped) and the game's own dynamic resolution (`game_dynamic_resolution`: GPU timestamps, or a 256x slower clock when 0) read live from nxconfig; not yet tried with changed ini values. **Missing:** `resolution` scale and the host's dynamic resolution (Turbo has them only in its GL renderer) |
| Screenshot (`vk_shot`), perf counters and the pacing line (`vk_perf`), render sizes in the log, pipeline cache on disk | `vk_shot.c`, `vk_perf.c`, `vk_pcache.c` | Done: `vk/shot.rs` (F3, `shot` command, `NXGFX_SHOT*`), `vk/perf.rs` (GPU time and replay lines every 60 frames, slow frames, render sizes every 600, `NXGFX_FRAME_CSV`), pacing line every ≤ 5 s, `vk/pcache.rs` (saved 2 s after the last new pipeline, atomic replace, header checked). MK8D `build/G/runs/r10`. Rev 6: a shot is taken before the present locks the submission state (the `shot` / `fullres` freeze); MK8D `r28` wrote a shot at present 6400 and played on |

### 5A.5 Shaders (lane G)

| Item | Turbo source | Status |
|---|---|---|
| The 3 failures: MKW's unimplemented Ampere instruction; Ocarina's and Violet's offset on a non-gather sample | — | Done: MKW's was a compute program starting with a 0x80-byte SPH (`job::is_ampere_sph`), the other two fetch with a computed offset (`fixes::dynamic_offsets`). Re-run: MKW 11,609/11,609, Violet 35,937/35,937, OoT 42,848/42,848 valid, 0 failed (`build/G/shaders/*.log`) |
| Real task / mesh shaders on Vulkan (`VK_EXT_mesh_shader`) instead of compute (MKW, DKB) | 9f19968 (Turbo's mesh draws) | Done in shaders (lane S, uncommitted): task / mesh keys are real `TaskEXT` / `MeshEXT` modules around the translated kernel (`mesh.rs`, nxnative's stage-memory layout): SPIR-V 1.4, outputs from the SPH maps, per-primitive attributes `PerPrimitiveEXT` (+ `mesh::fragment_per_primitive` for the pixel shader), invariant position, the draw's first workgroup read on the GPU from the `mesh_draw` records (DrawIndex, stride in push byte 12). MKW 19 mesh + 7 task, OoT 500 + 20, all spirv-val clean (DKB has none: 0 task / mesh keys). Contract for lane G in INTERFACES.md (shaders). Graphics' replay (lane G code: vk/exec.rs mesh_draw) must bind `mesh_draw` + push the stride and patch pixel shaders per INTERFACES; not yet seen in a game: MKW does not reach its mesh draws yet |
| Ray-tracing stages translated (NVN2) | — | Done in shaders (lane S, uncommitted): every stage-8 module linked into the game's one `rt_library` (`rt.rs`: RTCore metadata read and checked, logical IDs, MOV relocations, resource layout; nxnative's design), translated as one compute kernel tracing with ray queries (VK_KHR_ray_query + acceleration structures, not a ray-tracing pipeline: NVN2's continuation model is emulated in the kernel); reflection with the RT bindings and catalog (INTERFACES.md). A synthetic 3-module library translates and is spirv-val clean; no owner game has RT modules (Welcome Tour is the known one), so not seen in a game |
| Sonic X Shadow Generations' archive format; Unity, Unreal and the other engine readers nxnative has | nxnative `shader_split.py` | Done: PACx 405 (Sonic X Shadow Generations: 6,064 found, 6,064 valid, `build/G/shaders/sonic`), Unity (UnityFS LZ4/LZMA, serialized files), Unreal (paks incl. encrypted indexes, IoStore, `.ushaderbytecode`, inline, UE3), PlatinumGames DAT, Criterion BND2, RE Engine KPKA/SDF/OCIO, MT Framework ARC/SPK, KTGL/Koei/Gust, Smash data.arc, FromSoftware BHD5/BDT/DCX/BND3, RAGE rgxd, YEBIS tfxo, loose control sections in main. **Added (lane S, uncommitted):** MonoGame MGFX 8 / 5 in XNB (LZX, LZ4) and main, Monolith (ARH1/2, XBC1, cmp-zlib, HCPS, SLCT), Retro (RFRM / PACK, SNVN, LZSS in main), SUMO (Ninty_NX, NVNsh, MuxFDMR), CIVBSF, SHAD + CASC, Diablo III packages, Camelot CRA (index from NXSHADERS_NRO_DIR) + NARC, PKZ, RPF6 (key from main), Frostbite, BEA, NLG (rev 1 + patch, rev 4), IPK, Double Eleven packs, LSPK + BSHD, KIT, VBF + Phyre, Viper (zip / chained LZ4, VIPR), PDAT; unit tests on synthetic inputs. On the owner's games: Super Mario Party Jamboree (XCI v0, Bezel Engine 2.9 archives) 25,693 found (25,447 from .bea), all valid, every one of the 971 keys nxnative's run of the game used found; no other owner game uses these formats. Regression (build/S/shaders): MK8D 29,651, MKW 11,609, DKB 22,854, OoT 42,848, Z-A 28,603, Arceus 12,851, Violet 35,937 found, all spirv-val clean, 0 failed, 0 scan problems |

---

### 5A.6 Frame rate: 60 FPS options (owner, 2026-11-19)

There is no universal 60 FPS patch: each game caps its rate in its own code, and a game that steps its world per
frame (not by measured time) runs at double speed when unlocked. Three layers, from universal to best quality:

| Item | Lane | What | Status |
|---|---|---|---|
| Frame generation, built-in (Eglantine's design: optical flow + interpolation compute passes + pacer, no game motion vectors, no paid software) as the default; LSFG with the player's own Lossless.dll as an option, falling back to the built-in mode when the DLL is missing | G | Universal: every game shows 60 (or more) while running at its own rate; input still responds at the game's rate | In progress |
| Forced present interval: a per-game profile switch (and ini option) that overrides the interval the game asks the window for (NVN `nvnWindowSetPresentInterval` / swap interval) so the game presents every vsync | G (switch in the present path), profiles (per-game flag) | Off by default; turned on per game only after a run shows the game keeps its normal speed at 60 (it steps by measured time). The profile records the result for each tested game, including "unsafe: double speed" | Partial (lane G, uncommitted): the switch is in graphics' pacing (`nvn/window.rs` forced_interval): ini `present_interval` (0 off, 1-3 forced; added to the option table) over the profile's value, which os / profiles hand over with `nxgfx::nvn::window::set_profile_interval(n)` (profile key `present_interval`; not wired yet: lane O / profiles). Not yet run on a 30 FPS game (lane P) |
| Mod and patch files: load community 60 FPS (and other) mods per game and version: exefs code patches as IPS / IPS32 / `.pchtxt`, keyed by the module's build ID, plus RomFS layering from `mods/<mod>/` (and the `mod.json` form the translator README names). Code patches are applied at build time the way the Z-A profile patch is (both versions lifted, a switch per patch set), so a mod can be turned on or off without a rebuild | P (implements it in the translator / cli / profiles; first users are the Pokémon games) | Per game and version; the best result (true 60) where a mod exists. Mods are the player's files: never shipped, never downloaded by the engine | Done (lane P, uncommitted): `profiles::mods` (IPS, IPS32, `.pchtxt`, `mod.json`, the Atmosphère / Ryujinx / yuzu folder layouts; unit-tested), the build (`cli/src/mods.rs`: `<build>\mods\<mod>\`, each changed `.text` word a switchable site of set `mod_<folder>`, data bytes and the RomFS folder into `<data>\mods.tsv`, conflicts with the profile's cuts left out with a line), the run (`os/src/mods.rs`: `NXRS_MOD_<NAME>=0/1` or a `disabled` file, data bytes written at load, RomFS layered in fs; modules.c checks the marker). Checked on Arceus with a test `.pchtxt` lane P wrote (main+0x2aa278c, the present interval): on = 60 FPS, `NXRS_MOD_ARCEUS60=0` = the game's 30, same build |
| Which games are safe at 60: test the forced interval and available patches on the 30 FPS games (Arceus, Violet; Z-A already has its profile patch) and record per game: safe / double speed / needs a patch | P | Results go into the profiles and the 5B table | Partial (lane P): results in `profiles/games/<id>/engine.tsv` (`fps60`; a safe game also gets `present_interval display`, which os now hands to graphics' `set_profile_interval`). **Arceus: double speed** (forced interval and the test patch both 59-60 FPS, the opening's dialogue ran ~1.9x faster: 18 s vs 33.5 s from the load to the name prompt with the same presses). **Violet: needs a patch** (it sets interval 2 every frame; forced to 1 it ran 33-40 FPS and its opening slowed). **Z-A: its profile patch** (60.0 FPS on title and menus). Not done: a 5B table entry per game |

## 5B. The LIBRARY phase: every game in the owner's library, played past the title

Owner, 2026-11-19: get the rest of the library working on the nxrs engine and validate **gameplay**, not just the
title screen and movies. A game counts as **Playable** only when a run shows: controllable gameplay reached (scripted
input through the control pipe moves the player / kart / character), 10 minutes of that gameplay without a crash, the
picture correct (screenshots checked; FLIP against a Turbo capture where one exists), memory flat (no growth trend
over the 10 minutes, under the watchdog), sound playing with no underruns, and FPS from the pacing line at the game's
target. Then the owner plays it by hand. Sources are the owner's library entries (read only).

| Game | Package | Lane | Gameplay target | Status |
|---|---|---|---|---|
| Mario Kart 8 Deluxe | dump folder (+ update, Booster Course DLC) | W | Grand Prix race, 10 min, 60 FPS (= M5) | **Gameplay W, picture / FPS G.** 1G+1U+1D NSP, `build/games/mk8dw`. Scripted 50cc Mushroom Cup Grand Prix (`library_play.py mk8d_gp`), run `build/I/runs/20261119-140731-W-mk8d-gp`: 11 min racing through all 4 races and their result screens, no crash, sound 0 underruns over 783 s, private memory 3.93 -> 3.98 GB (+6 MB/min, flat), save committed (user + device). Open: picture blue / purple tint with black road (lane G), 50.7 FPS avg (196 of 641 s at 58+; stalls up to 2.5 s, lane G); Booster Course: `mk8d_booster` run `20261119-143627-W-mk8d-booster` races Paris Promenade (Golden Dash Cup) from the save the GP run wrote (relaunch: the save loads, the menus remember the last kart); menus crisp; menu previews decode (179 H.264 pictures, not black) but the panel draws black (lane G, 5A) |
| Super Smash Bros. Ultimate | NSP v0 | W | a stage match vs CPU, 10 min | **In progress (W).** Builds (`build/games/smash`, 2042 units, 45 min) with its 84 fighter NROs found by a RomFS scan (24 s) and loaded by `ro::LoadModule`; `nu::VirtualAllocHook` now weak-0 as on Horizon. Boots to its first present at 141 s (main thread busy before), loads 2 NROs, then hung in `nvnQueuePresentTexture` on a pending `fullres` (lane G, INTERFACES). v0 data.arc index unread, 77 shaders prebuilt (lane S) |
| Sonic X Shadow Generations | NSP v0 | W | a playable act, 10 min | **In progress (W).** Builds (`build/games/sonic`, 6 min); crashed in its static initialisers (VammManager, no system resource) until os took the NPDM's 16 MB system resource for unprofiled titles |
| Super Mario Party Jamboree | NSP / XCI (+ update) | W | a board turn and a minigame | **Package fixed (W):** 1.1.0 update opens over the NSP and the XCI (its patch relocates onto the base's compressed bytes; `real_update_over_compressed_base` checks every NRO against the NRR). Game loads 139 NROs at run time: built at fixed bases, loaded by `ro::LoadModule` (new, os `services/ro.rs`). Built (`build/games/jamboree`, NSP + 1.1.0). Fixed on the way: fibers that return and are switched to again (runtime), the code region's page end that broke every C++ throw (runtime loader). Reaches the opening movie (H.264 at 1920x1080 into a texture, ~29 FPS); a pending `fullres` hung the present (lane G) |
| Anime Girls: Sun of a Beach | NSP v0 | W | gameplay, 10 min | **In progress (W).** Builds (`build/games/agsob`, 53,109 shaders); same static-initialiser crash, fixed as Sonic's; now boots and renders at 60.0 FPS (p99 18.5 ms) for 1,891 frames until a pending screenshot hung the present (lane G), run `20261119-162902-W-agsob-m6` |
| Beach Bounce Remastered | NSP v0 | W | gameplay, 10 min | **In progress (W).** Builds (`build/games/bbounce`); stopped at "IsVirtualAddressMemoryEnabled() is false", fixed as Sonic's; now presents from 1.4 s and runs at 58.6 FPS (`20261119-161338-W-bbounce-m5`); every run so far ended by an Esc / window close from outside within 30-45 s (INTERFACES lane W) |
| Pokémon Legends: Arceus | NSP 1G+1U | P | Obsidian Fieldlands, 10 min (= M6) | In progress |
| Pokémon Violet | NSP 1G+1U+1D | P | first town, 30 min (= M6) | In progress |
| Pokémon Legends: Z-A | NUP (Switch 2 Edition) | P | Lumiose at 60 with the profile patch (= M6) | In progress |
| Mario Kart World | NUP | X | a race, 10 min, 60 FPS (= M7) | In progress (lane X): builds (`build\games\mkw`, 1.8.0 / SDK 21), peaks 8.9 GB (sparse images, no memory kill); the vi HDR / gs calls that crashed it are taken over; boots to its first present after a 20-60 s stall in graphics' RegisterTexture, then a null game-heap allocation crashes it 4-5 s later (INTERFACES lane X / G) |
| Donkey Kong Bananza | NUP v458752 | X | opening + mining tunnel, 10 min (= M7) | In progress (lane X): builds (`build\games\dkb`), title at ~50-60 FPS after the hid touch fix; starting the game crashes on bad game data pointers 20-35 s in (being narrowed) |
| Super Mario Bros. Wonder (Switch 2 Edition) | NUP | X | a course, 10 min | Nearly Playable (lane X): the NUP holds the Switch 1 build 1.2.1; title, file select and the first course driven by scripted input, 12.5 min with no crash (`build\I\runs\20261119-164407-X-wonder4-10min`: pass-dump pictures correct, 674 s of sound, 0 underruns, quit cleanly); private memory 3.7 GB → 4.8 GB in the first 5 min, then flat (4.67-4.87 GB) for 7 min; **FPS short of target**: pacing median 59.6, per-minute 54-59.6, p10 52.7 (single long frames). Not yet driven past a course's goal |
| The Legend of Zelda: Ocarina of Time | NUP v131072 | — | (= M8) | Deferred by the owner |

Not separate targets: "Mario Kart World v1.8.0" (the same NUP as Mario Kart World) and "Pokemon Legends Arceus base"
(a test package of the update work).

Lanes: **W** = Switch 1 library, **X** = Switch 2 library, **P** = Pokémon, **G** = graphics (the blocker most games
share: memory growth in NVN pools, the MK8D road / tint, mesh draws). Game-specific OS / loader / service gaps are
fixed by the game's lane; graphics findings go to lane G through `docs/INTERFACES.md`.

## 6. Launcher integration

### 6.1 Contract v1 (drop-in)

`nxrs-build.exe` accepts every argument the launcher sends:

- `input`, `-o`, `--outdir`, `--jobs`
- `--update`/`--no-update`, `--update-info`
- `--install-dlc`/`--sync-dlc`/`--remove-dlc`/`--list-dlc`/`--datadir`
- `-q`

`--target`, `--backend`, `--cc`, `-O`, `--runtime`, `--native` and `--chunk` are accepted and logged as ignored.

**Progress lines** match `parseLine`: `Analyzing …`, `C generated: n/m` (`nx-aot`), `compiled n/m` (`nx-cc`),
`Linking …`, `RomFS: …`. These lines are kept verbatim:

- `UPDATE\t<path>\t<title id>\t<version>\t<display>` and `UPDATE error: <reason>`;
- `DLC error: <reason>`;
- `DLC: this package also holds add-on content <id>; install it with --install-dlc (the launcher does)`.

**Output layout** (Turbo's), in `<dump>\SwitchRecomp Builds\<name>\`:

| File | Contents |
|---|---|
| `<name>.exe` | `nx-run`, with `nxrt.dll` beside it |
| `<name>_code.dll` | The compiled game C (the launcher's `moduleSuffix`) |
| `<name>_data\` | `control.nacp`, icon, `romfs.nsp`/`romfs.nup` marker, `dlc.manifest` + `dlc\`, shader library (SPIR-V), pipeline manifest and cache, `discovered.tsv`, saves |
| `<name>.ini` | Turbo's keys and comments: `docked`, `fullscreen`, `button_layout`, `renderer` (`vulkan`), `resolution`, `fps_target`, `fps_limit`, `language`, `dynamic_resolution`, `low_priority_shader_compiler`, `forced_vsync`, `profile`, `settings_version`, + the Spanish aliases |
| `<name>.log` | `[  t.ttt] [vk] pacing: F FPS \| p95 a p99 b max c ms \| >16.67 n >33/50/100 x/y/z (last N intervals)`, Turbo's format (`pace.py` regex) |

**Launcher prerequisites:** a `switchrecomp.py` shim that execs `nxrs-build.exe`, the bundled clang as "compiler", and
`nx-run.exe` as "runtime" satisfy `Toolchain::problem()`. No launcher change is needed.

### 6.2 Contract v2

- `nxrs-build.exe --contract` prints `nxrs-contract 2`.
- Subcommands: `build`, `update-info`, `dlc install|sync|remove|list`, `doctor`.
- `doctor` checks AVX2, Vulkan 1.3, the driver and free disk space.
- Machine-readable `@progress\t<stage>\t<done>\t<total>` and `@result` lines.
- `engine.json` beside the engine; a per-game `engine` field (`turbo`|`nxrs`) in `library.json`.

### 6.3 A/B and the transition

- B builds go into `<dump>\SwitchRecomp Builds\<name> [nxrs]\`.
- A one-time save copy from A is offered, and happens only after confirmation. *Task: confirm Turbo's save location.*
- The launcher's default engine flips per game as its milestone passes, starting with MK8D at M5.

---

## 7. What carries over from Turbo (MIT-reusable only)

Owner-authored code or logic only (audit §5 Group 1). Nothing from Group 2 (the AGPL recompiler, OS emulation and
`runtime/nvn`).

| Turbo piece | → | How |
|---|---|---|
| `nca.py`, `nsp.py`, `runtime/nsp.c` (in-place NSP/NSZ, combined NSP, BKTR, AES-CTR/XTS) | `nx-package`, `nx-fs` | Port |
| `nup.py`, `runtime/nup.c` | `nx-package::nup` | Port; merged with nxnative `pack_nup.py` knowledge |
| `dlc.py`, `runtime/dlc.c` | `nx-package::dlc`, `nx-hle::aoc` | Port; the `dlc.manifest` format is unchanged |
| `zbic.py` | `nx-package::zbic` | Port |
| `switchrecomp.py` +234 logic | `nx-cli` | Re-express |
| `nxloader.py` +112 (Z-A 60 FPS patch for `B1F12FD9…`) | `nx-profiles` (Z-A Switch 2 program) | Re-express as `patches.toml` |
| `svc_fs.c` +159, `svc_core.c`/`svc_ns2.c` +492 (not the Eglantine-derived decoder protocol), `kernel.c` +252, `svc_hid.c` +116, `audio.c` +150 | `nx-fs`, `nx-hle`, `nx-kernel`, `nx-audio` | Re-express, after checking nxnative's equivalents |
| `backend_win32.c` +303 (window mode, memory cap, control pipe) | `nx-front`, `nx-harness` | Re-express |
| `controller_sony.h`, `controller_sony_win32.h`, `pad_layout.h` | `nx-input` | Port |
| `av1video.inc`, `build_dav1d.py`, `build_opus.py` | `nx-video` build | Port the build glue |
| `vk_perf.c/.h` | `nx-vk::diag` | Port; same log lines |
| `play_scenario.py`, `guard.py`, `pace.py`/`trend.py`, `host_prof.py`, `mkw_*.py`, `plza_probe.py` | `nx-harness` | Port |
| Fork-added NVN-on-Vulkan logic (lazy write-back, queue parking, pool descriptor set, transform feedback, counter times, mesh-indirect, NDVE/NDSR) | `nx-vk` | Logic only. mkw-recomp's MIT code covers most of it and is preferred |
| 17 fork docs | `nxrs/docs/turbo-history/` | History |

---

## 8. Licensing

**Default: MIT.** NOTICE and `THIRD_PARTY.txt` are written into every build (`cargo about` plus the vendored C
notices).

**Attribution lives in one place: the README.** (Owner's rule, 2026-11-19.) Source files carry **no** copyright,
license or "ported from" header comments: no per-file attribution blocks, no SPDX lines, no copied header templates.
The repository README has a single Credits and licenses section that names every source in the table below with its
copyright line and license, and states which parts of the engine each one contributed. The full MIT/BSD/Apache license
texts that those licenses require to accompany copies sit with it (README, or a LICENSES folder the README points to)
and are shipped with every build. Vendored third-party C (for example the generated-code headers from mkw-recomp)
also has its header comments removed when vendored; its credit is the README entry. Code comments explain the code,
never its provenance.

| Source | Licence | Copyright / origin | Used for |
|---|---|---|---|
| **nxnative** (seedds) | MIT | — | **The base**: host, HLE, NVN layer, profiles, docs, shader tools |
| **mkw-recomp** (adriadam10) | MIT | © 2026 Adrià | Translator, CPU bridge, Windows memory mechanism, Vulkan pieces, GPU ASTC (section 2) |
| **totk-pc-recomp** (valentderah) | MIT | © 2026 Valiantsin Dzerakh (`LICENSE.totk-pc-recomp`) | The lineage of the borrowed recompiler and runtime parts; includes the wdiasjunior Linux port (PR #60) |
| LAS Recomp | MIT | © 2026 Peiikai | Origin of `lifter.py`, `gen.py`, `simd_c.py`, `cpu.h`, `memory.c` (as cited in those files) |
| MacNativeGames | MIT | © 2026 MacNativeGames contributors | Cited in `memory.c`; also in nxnative (`nvn_mtl.m` index widening, clear quads; TotK counter lowering in `dmsh_xlate`) |
| Ryujinx 1.3.2 (via `dmsh_xlate`) | MIT | Ryujinx team | Shader translation (build tool) |
| deko3d layout algorithm (via `nvn_tex.c`) | zlib | — | Texture layout |
| AMD FidelityFX FSR1 | MIT | AMD | Scaling shaders |
| mkw-recomp `sm86` + DocumentSASS, nvsass-disassembler | MIT | — | SM86 cross-check tool only |
| NVIDIA `nvdisasm`/`ptxas` | CUDA EULA (redistributables) | NVIDIA | Only if the `sm86` cross-check runs: downloaded on the machine, never shipped |
| Vulkan Memory Allocator | MIT | AMD | Optional allocator |
| llvm-mingw (clang, lld) | Apache-2.0 WITH LLVM-exception | — | **Bundled compiler** for `_code.dll` |
| dav1d / libvpx / Opus / zstd | BSD-2 / BSD-3 / BSD-3 / BSD | — | Video, audio, decompression |
| dynarmic, `dynarmic-sys-mythrax` 0.3.7 (+ fmt MIT, Zydis MIT, Boost BSL) | 0BSD | — | Dev-only oracle |
| Rust crates: `ash` 0.38, `winit` 0.30, `cpal` 0.18, `hidapi` 2.6, `windows` 0.62, `dav1d` 0.12, `aes` 0.9, `ctr` 0.10, `xts-mode` 0.6, `sha2` 0.11, `zstd` 0.14, `lz4_flex` 0.14, `rspirv` 0.13, `spirv-tools` 0.13, `clap` 4.6, `serde`, `toml` 1.2, `rayon` 1.12, `linkme`, `egui`, `lewton`, `yaxpeax-arm` 0.5 (0BSD) | MIT/Apache-2.0 | — | Versions from crates.io, 2026-11-19 |
| NVIDIA FLIP, PresentMon | BSD-3, MIT | — | Harness |

**Never copied:**

- Turbo's MZ Labs-derived code (AGPL).
- Eglantine, eden-nx2, MKW-THWOMP, smo-recomp and Hookshot (GPL).
- NX2navi's later additions (all rights reserved).
- Unicorn (GPL-2): mkw-recomp's `lift_diff.py` uses it; we use dynarmic instead.

**The GPL option (Eglantine).** Shipping as GPL-3 would allow Eden/Eglantine's video_core, its C++ Maxwell/Ampere
shader recompiler and its NX2 services. The costs are full-source releases and no way back to MIT. **Recommendation:
stay MIT.** mkw-recomp shows the MIT path reaches 60 FPS on this hardware, and nxnative and Ryujinx (MIT) cover the
shaders.

---

## 9. Effort and calendar

**Unit:** one agent session of 1–2 hours that ends with a verified deliverable. **Staffing:** 3 lanes (T, O, G), one
agent each. Throughput assumption: 4–6 verified sessions per lane per working day. The owner reviews daily.

| Phase | Sessions | Lane | Weeks |
|---|---|---|---|
| P0 Foundations (workspace, CI, PORT_MAP, package from Turbo, profiles, CLI v1 skeleton, harness core) | 25–35 | O, G | 1–2 |
| **P1 Translator port** (`nx-xlate`, `nx-aot`, `nx-cc`: 5.2k Python → Rust, byte-identical) | **12–20** | T | **1–2 (days)** |
| P1b dynarmic oracle (parallel) | 10–20 | T | 1–6 |
| P2 CPU bridge and Windows gaps (`nx-cpu`, `nx-genrt`, `nx-mem` mechanism, fault map, homebrew M2) | 20–35 | T, O | 2–4 |
| P3 nxnative host port (kernel, svc, loader, HLE registry + services, fs; about 22k lines of C → Rust) | 90–130 | O | 3–14 |
| P4 `nx-nvn` record layer | 25–35 | G | 4–8 |
| P5 `nx-vk`: `nvn_mtl.m` rewritten onto Vulkan, with mkw-recomp donor code for pools, pipelines, recording, textures and sync | 120–180 | G (+T after M2) | 6–22 |
| P6 Shaders (`shader_split` port, `dmsh_xlate` CI, binding patch, manifest/prewarm) + textures (GPU ASTC) | 45–70 | G | 4–12 |
| P7 Audio, video, input, front end | 45–70 | O | 8–18 |
| P8 Per-game bring-up and performance (MK8D 30–60; Pokémon trio 50–90; MKW + DKB 60–110; OoT 50–100) | 190–360 | all | 9–30 |
| P9 Launcher v2 | 15–25 | O | 6–12 |
| Upstream sync (nxnative weekly, mkw-recomp donor files monthly) | 50–70 | rotating | ongoing |
| **Total to M8** | **about 650–1,050 sessions** (≈ 1,000–1,600 agent-hours) | 3 lanes | **7–9 months** |
| P10 Linux | 30–50 | 1 | +4–6 weeks |

**Against revision 1:**

- The translator falls from 115–180 sessions to 12–20, plus 10–20 for oracle tests in parallel.
- The Vulkan rewrite falls from 180–260 sessions to 120–180, because mkw-recomp's MIT Vulkan code covers the hardest
  parts (discrete-GPU pools, pipelines, recording).
- The nxnative host port and per-game bring-up do not shrink. They are most of the remaining work.

**MK8D at 60 at about month 2.5–3. All seven games at about month 7–9.**

---

## 10. Risks and mitigations

| # | Risk | Likelihood / impact | Mitigation |
|---|---|---|---|
| **R1** | **nxnative's NVN layer assumes unified memory** (GPU address = CPU address, zero-copy pools); on the RTX 5090 every pool needs a placement and sync policy | High / high | Port mkw-recomp's `vk/mem` model (import, VRAM mirror, shadows, write-back, write-watch) into `nx-vk::mem` first in P5. M4 logs write-back ms/frame |
| **R2** | **No run-time translation.** A function the build missed crashes, and code-writing cheats or nn::jit cannot apply live | Medium / high | Dispatch misses are logged with the PC and added at the next build (`discovered.tsv`); nxnative's jump-table cases are tests; code cheats are compiled in as image patches at build time. Later option: dynarmic as a run-time fallback (0BSD) |
| **R3** | **Bridge semantics.** nxnative's HLE assumes direct C calls on guest stacks, `HLE_FORCE` takeovers, host threads entering guest code and fibers; the lifted-C world changes how each happens | Medium / high | 3.2's wrapper generator with typed signatures from nxnative's prototypes; M2's homebrew suite (threads, exceptions, `setjmp`, fibers) before any game; nxnative's `t_*` unit tests ported |
| **R4** | **Shader binding ABI.** `dmsh_xlate`'s SPIR-V was post-processed for Metal via spirv-cross | Medium / medium | Use Ryujinx's Vulkan resource layout, rewritten by mkw-recomp's `vk_spirv_patch` approach; MK8D shaders validated (`spirv-val` + pipeline creation) in week 2; `sm86` as a cross-check and fallback for Switch 2 |
| **R5** | **NVIDIA driver memory and pipeline counts** (Turbo: 61.6 GB in `nvoglv64.dll`; OoT: 25,170 manifest pipelines) | Medium / high | Graphics pipeline library + key file + background prewarm with a memory budget (mkw-recomp `vk_pipe_*`); shared layouts; OoT's first-boot private bytes measured in M8's first session |
| **R6** | **Upstream drift** (nxnative fast; mkw-recomp daily) | High / medium | Pinned commits; `PORT_MAP.tsv`-driven weekly nxnative sync; donor files tracked separately and re-checked monthly |
| **R7** | **Windows guest memory**: fixed reservations below 2^39 collide with ASLR; 64 KiB view granularity for aliases | Medium / medium | Reserve first in `nx-run` (as mkw-recomp's `main.c` does); hole search fallback; copy fallback for small aliases, logged |
| **R8** | **Hybrid CPU (285K)** and thread placement | Medium / medium | Per-thread CPU Sets for frame threads; in-run A/B |
| **R9** | **Compile instability** on this machine under parallel load | Medium / low | `nx-cc` chunk cache + one retry; a jobs cap |
| **R10** | **Measurement contamination** (Turbo's lesson) | High / high | Guard on every run, PresentMon + pacing, hashed run folders, in-run A/B |

---

## 11. The first two weeks, as 1–2 hour tasks

Paths:

- `NX` = `D:\Projects\nxnative`
- `MR` = `D:\Projects\mkw-recomp-main`
- `T` = `D:\Projects\Switch2RECOMP-main`
- `R` = the new repo `D:\Projects\nxrs`, created in O1 with the owner's go-ahead

Rules for every task:

- Upstream repos are read-only.
- Default `cargo test` < 60 s.
- One game at a time, under the guard.
- Use `py` for Python.
- A task is done when its check passes.

### Week 1

| ID | Lane | Task | Deliverable | Verify |
|---|---|---|---|---|
| O1 | O | Create `R`: workspace with the section 1.2 crates, MIT LICENSE, README Credits and licenses section (section 8), CI on `windows-latest`, `xtask` fetching pinned llvm-mingw | Skeleton | `cargo test` < 60 s; CI green |
| O2 | O | `PORT_MAP.tsv` generator over `NX` + the `MR` donor files, seeded from section 4 | Tool + TSV | Every file has a row; line counts match `wc -l` |
| O3 | O | `nx-package`: keys, NCA (XTS header, AES-CTR), PFS0/NSP (Turbo `nca.py`/`nsp.py`) | Module | MK8D, Arceus and Violet NSP listings and SHA-256 of `main` equal Turbo's |
| O4 | O | `nx-package`: NUP, BKTR update view | Module | MKW, DKB and OoT NUP listings equal Turbo's; 10 MK8D update-patched files hash-equal |
| O5 | O | `nx-cli` v1 skeleton + `--update-info` | `nxrs.exe` | Byte-identical to Turbo's output on the owner's update files |
| T1 | T | `nx-aot`: NSO/module/layout (port of `MR/recomp/nso.py`, `module.py`, `layout.py`; base from the profile) | Module | Segments, symbols, relocations and seeds equal Python's on MKW and MK8D `main` (JSON diff) |
| T2 | T | `nx-xlate` part 1: integer, branch, load/store, system, exclusives, LSE (port of `lifter.py`) | Module | Word → C text equal to Python's for every distinct word of MKW `main` |
| T3 | T | `nx-xlate` part 2: `simd_c.py` | Module | The same equality for every FP/SIMD/crypto word |
| T4 | T | `nx-xlate` part 3: `flags.py`, `regs.py` | Module | Per-function C equal on 1,000 sampled MKW functions |
| T5 | T | `nx-xlate` part 4: `jumptables.py` + nxnative's 26 `rewriter.md` cases as tests | Module | Same tables for every `br` in MKW; nxnative cases pass (expected targets from `NX/dm/tools/tests/test_jump_solver.py`) |
| G1 | G | `nx-common` log line + `[vk] pacing:` formatter; `nx-harness guard` (Turbo `guard.py` extended to compilers) | Modules | Lines parse with `T/build/evidence/audit/pace.py`; guard unit test |
| G2 | G | `nx-profiles`: `game.tsv` + data TSVs for all 192 | Module | 192/192 parse; MK8D `pso_manifest` = 1,518 rows, OoT = 25,170 |
| G3 | G | Dev-side `export_patches.py`: the 27 `patches.py` + `sdk_patches.py` → `patches.toml` | Script + TOMLs | 27/27 export |
| G4 | G | `dmsh_xlate` NativeAOT build in CI from pinned `NX` | `dmsh_xlate.exe` | Translates 20 MK8D shaders (extracted with `NX`'s `shader_split.py`); `spirv-val` passes |

### Week 2

| ID | Lane | Task | Deliverable | Verify |
|---|---|---|---|---|
| T6 | T | `nx-aot::gen`: discovery fixed point, chunking, tables, manifest (port of `gen.py`) | `nxrs dev gen` | **M1(a)**: MKW 1.0.0 chunks and tables byte-identical to `py MR/recomp/gen.py`; time ≤ Python's |
| T7 | T | `nx-cc`: port `gamecode.py`/`fastcc.py` (flags, chunk cache, link probe, `<name>_code.dll` against `nxrt.lib`) | Module | MK8D's C compiles and links; one changed chunk rebuilds alone |
| T8 | T | Lift + compile all seven games; coverage report (port of `lift_coverage.py` + nxnative `insn_census.py`) | Report | **M1(b,c)**: `rt_udf` < 0.01%; MK8D cold build ≤ 5 min |
| T9 | T | `nx-cpu`: dispatch table, host-call registry, `sjlj`, `rt_svc` stub, fault handler mapping host PC → guest function; `nx-genrt` headers vendored | Crate | Unit tests; a hand-assembled NRO calling a host function returns its value |
| T10 | T | `nx-oracle` on `dynarmic-sys-mythrax` (timebox 2 sessions; WSL fallback): one lifted instruction compiled into a test DLL vs dynarmic | Harness | 0 diffs on the integer classes over 2,000 cases; a wrong lowering is caught |
| O6 | O | `nx-mem` on Windows (port of `MR` `win32_mem.c` + `memory.c` mechanism) with nxnative's MK8D `game.tsv` layout | Crate | Reservation below 2^39 succeeds 100/100 launches; write-watch reports exactly the written pages |
| O7 | O | `nx-loader`: port nxnative `loader.c` (relocations, imports → host-call addresses, profile image patches) | Crate | MK8D modules load; unresolved import names equal `NX/dm/tools/nso_syms.py`'s minus the SDK-defined ones; the OoT/Z-A patched words equal `patch_nso.py`'s `.changed.tsv` at cut sites |
| O8 | O | `nx-hle` registry + `#[hle]` macro generating `Cpu*` wrappers (narrow ints, stack arguments, v-regs) + 5 libc shims | Crates | Unit tests per signature shape (u8 bool, 9 args, float args, struct return) |
| O9 | O | Minimal `nx-kernel` from nxnative `hle_thread.c`: CreateThread, StartThread, ExitThread, SleepThread, WaitSynchronization, ArbitrateLock/Unlock, SignalProcessWideKey | Crate | libnx `hello` NRO prints its line (M2 begins) |
| G5 | G | `nx-shader::split` part 1 (SARC, Yaz0, zstd, BNSH/BFSHA: MK8D's path) | Module | MK8D shader list equals `shader_split.py`'s (keys and hashes) |
| G6 | G | `nx-tex`: deswizzle (nxnative) + NVN → `VkFormat` table; GPU ASTC decode + BC7 encode compute shaders from `MR/runtime/shaders/texture` compiled and run on 50 MKW blocks | Module + test | Deswizzle vectors from `NX` `t_tex.c` pass; ASTC GPU output equals the CPU decoder (`MR` `tex_astc.c` or astcenc) bit-exactly |
| G7 | G | Contract v1 dry run: `nxrs build <MK8D NSP>` writes `<name>.exe` (stub), `<name>_code.dll` (T7), `<name>_data\`, `<name>.ini` with Turbo's keys | CLI path | Files satisfy `library.cpp` `builtFiles()` (a Rust test of its conditions); the launcher GUI is not driven |

**End of week 2 checkpoint** (owner, 30 min):

- M0 and M1 recorded;
- the bridge (T9/O8) demonstrated;
- go/no-go on the oracle;
- MK8D confirmed as the first boot target.

---

## 12. Glossary for the owner

| Term | Meaning |
|---|---|
| **Translator / lifter** | Reads the game's ARM code and writes the same logic as C, one C function per game function. The bundled clang compiler turns that into x86 code |
| **Host / HLE** | nxnative's stand-in for the Switch's OS and services, now in Rust |
| **Bridge** | How translated game code calls the engine's own functions, and back |
| **NVN** | The Switch's graphics interface. nxnative records it; nxrs replays it on Vulkan |
| **Donor** | mkw-recomp: we take specific parts from it, not the whole program |
| **dynarmic** | yuzu's ARM translator, used only as a referee in tests |
| **Warm run** | The second run after a build, with shaders and pipelines prepared |
