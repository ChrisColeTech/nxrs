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
| M0 | Foundations: workspace, profiles, package listings | Partial | 202d449, lane Q (uncommitted) | **Lane Q 2026-11-20:** profiles: the "5 missing" of 192 were nxnative's 5 loose files in `dm/games` (187 folders + 5 files at e4b0d9b); nxnative has since grown to 207, all imported with `tools/export.py` + `export_shaders.py` from 7ebb76e (20 new folders, 1 new `patches.toml`, `settings.tsv`; existing files byte-identical; `export.py --check` 240 files 0 differ, `export_shaders.py --check` 0 differences); `cargo test -p profiles` 14/14 + `tests/port_map.rs`. **CI:** `.github/workflows/ci.yml` (windows-latest: workspace build + every test needing no GPU / game data, cargo cache, glslang 15.1.0 built and cached; gnullvm job with llvm-mingw 20261104; oracle job on request); same commands run locally green except lane O3's two uncommitted os tests (below in INTERFACES Q). **PORT_MAP.tsv** written: 6,171 rows (nxnative 7ebb76e 3,873, Turbo 11e966f 2,090, mkw-recomp donor files 208), `scripts/port_map.py --check` regenerates and compares (0 problems), statuses corrected after the audit. Open: `nxrs --update-info` byte-identity and the 7 package listings are not re-checked by lane Q |
| M1 | Translator | Done | c7d34a3, adc736f, lane T (5A.1) | MKW C 242,014 / 245,653 functions identical to mkw-recomp, every diff explained; ZBIC NSOs, XCI as the game's package and the 6 SDK seeds done in 5A.1 (audit 2026-11-20: XCI as the game's package is unit-tested only, never built and run: Partial in 5A.1) |
| T-lane | dynarmic differential tests | Done (default run) | lane Q (uncommitted) | `src/oracle` (`nx-oracle`, own workspace: dynarmic-sys-mythrax 0.3.7 built with Boost 1.86 headers): 71 instruction classes (data processing, branches, system registers, every load / store form incl. exclusives, LSE, CAS and SIMD structures, scalar FP, Advanced SIMD, crypto), lifted C compiled with the game flags vs dynarmic (accurate settings), 2,000 cases per class: **140,520 cases in 11 s, 0 mismatches** after one translator fix (STGP was lowered as a 32-bit STP: `lifter.rs`, test `stgp_is_not_a_32_bit_stp`); `--mutate` self-test catches every word of every class. By design, not filed: `RET Xn` (n != 30) lowered as a return; PACGA (key-dependent). FPSR cumulative flags differ (counted apart). CI: `ci.yml` oracle job (`oracle: true`, `oracle_cases` 1000000 for the nightly size). Not compared: FP with FPCR other than 0 (FZ, rounding modes) |
| M2 | Execution and the bridge (runtime) | Done | c7d34a3, 6fae87d | homebrew e2e 80/80, threads 200/200. Since: two threads-on-one-core races closed (d38c1ab, 684f86c), paused fibers' stacks freed (9182594) |
| M3 | OS + HLE boot | Done | 3845d1c | The null GPU it used is deleted; every run is on the real Vulkan device (5A.2) |
| G-lane | Graphics: NVN on Vulkan | Partial | 34f540b … f6fc757 | Async replay, lazy write-back, descriptor buffers, pipeline libraries, sparse images, mesh draws, transform feedback, raw global memory, resolution scale, frame generation (built-in + LSFG), shared aliased images, late vertex outputs (DKB's driver crash). Details: 5A.4 and INTERFACES lanes G / G3. **Lane G4 (2026-11-20, uncommitted):** storage images without the IMAGE flag, integer outputs, alpha test, dual-source, RTX 50 ballot fix, sharp upscale, linear render-target readback, clip / cull trim and varying linking (lane C's test clean), graphics ini options logged, GPU tests for buffer textures and append counters (INTERFACES lane G4). **Still open:** recording on several threads, ray tracing, VRAM eviction, capture / replay; MK8D's cyan / black shadowed ground (narrowed, not fixed), Jamboree's white layer (not a blend state: the game draws it opaque), DKB 13-17 FPS (change made, unmeasured: DKB aborts at DT_INIT on the current tree) and purple world, Wonder's first-entry stutter (first-run profile prewarm added, unmeasured), Smash device-lost (not reproduced alone in 360 s). **Round 3:** async replay's two missing waits fixed (command-buffer FenceSync, waits on a parked queue), indirect plain draws clamped, buffer-texture descriptors refreshed when their pool goes, DKB pool sync 32 -> 10-12 ms (15-25 FPS) and a stage-set cache (sets 27 -> 15 ms, 60% reused), conditional rendering ported from Turbo (addr + 16, timestamps' counter 0, pending reports, clears obey it: MK8D's black world), replaced pipelines destroyed; Wonder's first-run frame not fixed (a bigger part budget made it worse); Smash's invalid write being narrowed with pass / freed-resource reports. **Lane G5 (2026-11-20, uncommitted):** capture / replay reproduces a frame offline (every GPU-drawn image whole, registrations kept past finalize; G4's "white" replays were transparent black); **MK8D's async black stretches: fixed** (nvnTextureFinalize released the scene target ser6754 on the game's thread while that frame's work, still queued, read it through pool slots: tone mapping bound the zero stand-in; the release now runs in queue order; async lap `-G5-mk8d-lap-order`: 0 black shots of 14 against 3-6 before, 59.7 FPS), plus serial forwarding for taken-over / reused images (GPU test); texture lifetime log for MKW's ser28824 fault (`NXGFX_WATCH_ADDR`; one MKW run on G5's tree: no fault in 45 s); **handle-table texture arrays** rewritten onto the pools (Jamboree: 1,413 refused shaders now build, lane C test 0 validation errors); DKB vertex-input skip (replay CPU 25.8 -> 21.0 ms at the same frame, FPS unchanged: DKB's own present interval 3); Anime Girls' banding located in Unity's LUT pass (not fixed); Jamboree still white (game state); MK8D kart blob narrowed to the lighting pass (not fixed) |
| S-lane | Build-time shaders | Done (shader side) | 6c11b77, de2a969 | 184k shaders over 7 games, 0 failed; task / mesh and ray-query modules. Lane G4: clip / cull distances trimmed to the device and varyings a pixel shader reads that its vertex shader never writes declared (`shaders::link`, applied by graphics): lane C's pipeline test 0 validation errors on MK8D, DKB, Z-A (300 pairs each), Smash, Jamboree, Violet (600 each) |
| **GAPS** | **Turbo + lane parity (section 5A)** | In progress | | (audit 2026-11-20: docs/AUDIT.md cross-checks nxnative and Turbo feature by feature; its corrections are marked in the rows, its gaps added as Not started rows in 5A.1-5A.5) Not Done: 5A.1 1 row, 5A.2 6 rows, 5A.3 3 rows, 5A.4 10 rows, 5A.6 2 rows. Assigned: lane O3 (5A.1-5A.3, profile wiring of 5A.6), lane G4 (5A.4, graphics ini options) |
| M4 | First pixels: MK8D title screen on Vulkan | Done | | MK8D title, Mii select, main menu at 59.4 FPS. Shots: `build/G/m4/` |
| M5 | MK8D at 60 | Done (Playable, lane G5 2026-11-20) | 6bdccef | **Playable by the 5B bar:** `build/I/runs/20261120-135316-G5-mk8d-m5` (current HEAD 6bdccef, exclusive, async replay, from boot through the menus into a Mushroom Cup race, 200 s): picture right in all 16 race shots (black fraction 0.002-0.14, the 0.14 a night shot with star power; no black stretches, no banding), **59.65 FPS** over 100-190 s (76 of 84 s at 58+, p99 31.6 ms), exit 0. Earlier: Grand Prix 11 min at 59.1 FPS, sound clean, memory flat (`20261119-212504-W2-mk8d-gp4`). **Known issue:** the karts' shadow blob under them is cyan instead of dark (lighting pass where the shadow mask is 0; INTERFACES lane G5) |
| M6 | Pokémon trio | In progress | | Arceus 21.7 min of story at 30.0 FPS (stops at a story point that needs a hand play); Violet 20 min at 30.0 FPS in the house, a new game waits at name entry (software keyboard: lane O3); Z-A to the Lumiose station platform at 58.5-60 FPS. None at its 5B target yet |
| M7 | MKW + DKB | In progress | | MKW (lane K): mkw-recomp's 1.8.0 game layer ported (149-entry patch table as the profile's recomp.txt: 97 spin waits, 43 sites in 9 switchable sets; host side os gamefix: zstd on the host, camera stand-in, render-buffer pool, seed, output / aspect / vert+ / display, frame rate, race scene, RomFS resolution overlay); the 15.5 s crash fixed (core mask 0x3f from the NPDM instead of 0x7); the 31-40 s stall fixed (the window acquire sync satisfied at once, as mkw-recomp's); with mesh draws off (`NXGFX_MESH=0`) MKW reaches a Grand Prix race (HUD right, 3D mostly black, 8-11 FPS, memory watchdog at 20 GB after 383 s); GPU faults: the mesh ones come from nxrs's indirect / 3D mesh draws (skipped, 603 s with no device loss), the cube-map compute write fixed with robustImageAccess; the 71 s stall fixed (lane K2: not the particle loop but a lazy write-back deadlock, a thread waiting for a page it was bringing back itself; the oracle's new --words / --fpcr mode found no float mismatch under FPCR.FZ), the 28-34 s present gaps fixed (the write watch walked its range once per 4,096 written pages); MKW now reaches the course flyover lit and right (with G4's conditional-rendering change) at 10-20 FPS; GPU faults left in *direct* mesh draws of one task program (fea1d6ca04512de1) even with indirect / 3D skipped (INTERFACES MKW lane K2); 10 minutes in a race and 60 FPS not reached. Lane K3: the early "128x128x5 pass" fault is a direct mesh draw (the pass begins inside the draw; ser28824 alive throughout) reading ~5.3 MB past some base into a freed range that no binding of its own covers; candidate robustBufferAccess2, not run (3 driver resets, MKW runs stopped); menu text = glyph uploads lost to page hashes taken after the upload read (fix built, not verified); race 7-9 FPS, pool sync 65-79 ms a frame (INTERFACES MKW lane K3). DKB: start-up and driver crashes fixed, opening plays at 13-17 FPS (lane G4) |
| M8 | Ocarina of Time | Deferred: next after 5B | | Owner 2026-11-19: after the other games are validated; deferred is not excluded. Builds (ZBIC done); stops at a static-init fault (main+0x946a20). The rows that need it (unrouted voices, its movies, its graphics needs) stay Partial until it runs |
| M9 | Launcher contract v2 | Partial | launcher c599914, lane Q (uncommitted, both repos) | **MKW through the launcher (lane Q, 2026-11-20):** built by the launcher itself (a scratch build of it, temporary library `build\Q\lib`, `nxrs --select-title "Mario Kart World" --build`, contract v2 `build ... --machine`) into `build\Q\games-head\Mario Kart World [nxrs]` with the engine at a9d15ba: `@result ok` in 522 s (translate 21 s, shaders 127 s, compile 373 s); `run_game.py` 60 s (NXRS_LANE=Q): boots, presents 120 frames, then the known stop at ~14.5 s (host access violation, lane O3's). The same build from the working tree at 02:40 stopped at 1 s (`rt_dispatch` at patch page +0xff0: lanes O3 / X2's uncommitted move of the MKW patch to `recomp.txt` and `gamefix.rs`, half landed at that time). **Launcher (lane Q):** `doctor` in Settings > Engine and `nxrs --nxrs-doctor`; Game > Copy saves from the Turbo / nxrs build (Turbo `save\user` <-> nxrs `save\user1`, backup first, offered once on Play); `.xci` accepted (nxrs only); release.yml bundles the engine behind `nxrs_engine` / `NXRS_ENGINE_IN_RELEASE`, off (owner decision: token and publishing). Launcher tests 32/32 + UI 10/10. Open: owner decision on OpenGL for nxrs games; the keys fallback path is lane O3's |
| M10 | Linux | Done (build / package criterion) | lane L (uncommitted; its graphics part went in with 792f907), lane Q (uncommitted) | **Owner 2026-11-20: as Eglantine — CI builds and ships Linux packages; no hardware test gate. Built and tested in CI and WSL (llvmpipe); not run on a real Linux GPU.** Lane Q: `ci.yml` `linux` job packages the x86-64 engine bundle (`scripts/package_engine.py`: `nxrs-engine/` with nxrs-build, engine.json, libnxfront.a, libnxrt.a, tools/dmsh_xlate (built from nxnative e4b0d9b, the Windows tool's `--source-sha` e11dc43d4ddc5b58), tools/spirv-val) as the artifact `nxrs-engine-linux-x64`; the launcher's Linux release carries it (release.yml, package-release.py --nxrs-engine). WSL: bundle packaged, unpacked elsewhere: contract 2, doctor green but the display; a game build with the unpacked bundle not done (WSL's disk failed, INTERFACES Q). **Lane L 2026-11-20:** the whole engine builds and runs on x86-64 Linux behind `cfg(target_os = "linux")`, Windows untouched (INTERFACES "Linux (lane L)": every Windows piece and its Linux equivalent). Runtime: mmap guest memory, userfaultfd + PAGEMAP_SCAN write watch, one signal handler with fault hooks, System V assembly boundary, own fiber switch, SIGPROF samples, cgroup / watchdog memory cap. Graphics: Xlib / XCB / Wayland surfaces. os: hidraw, SDL2 pads, BlueZ Switch 2 pads, FFmpeg H.264 / AAC, system opus / vpx / dav1d, ALSA through cpal, zenity / kdialog dialogs. Front: winit window, Unix-socket control channel, ffmpeg F7. cli: clang + ld.lld, `<name>` + `<name>_code.so`; Linux `dmsh_xlate` (same source hash). CI: `linux` job (ubuntu-latest). **Verified in WSL (Ubuntu 24.04):** `cargo test` 349 green; homebrew e2e 8/8, 200/200 repeated; `nxrs-build doctor`; Beach Bounce built (189 s) and run under Xvfb on llvmpipe (the only Vulkan device WSL has here): title at ~56 FPS, A presses through the control socket into the story, F7 MP4 (H.264 + AAC); Anime Girls built with its 53,109 shaders by the Linux dmsh_xlate (all valid, sampled SPIR-V byte-identical to Windows) and its 3D title drawn on llvmpipe (with the profile prewarm off: llvmpipe crashes in a pipeline-library part, noted for lane G4). **Not verified:** a real GPU (no dzn / GPU driver in this WSL), real sound (no sound server), real controllers / Bluetooth, Wayland, the cgroup cap (WSL refuses it; the watchdog ran). Exit criterion (M5 on Linux: MK8D at 60) needs a Linux PC with a GPU |
| 5B | The library: gameplay per game | In progress | | Bar (owner 2026-11-20): past the intro videos into first controllable gameplay, short run, no story progression, no hand plays. The "Gameplay target" column's longer targets are superseded by that bar. By it: Wonder, Arceus, Violet, Z-A, Sonic Generations, Shadow Generations, Beach Bounce, Anime Girls, Smash reach gameplay (picture / FPS / crash issues listed per row); Jamboree (white layer), MKW, DKB not yet; MK8D reached it (lane G5, M5 row) |

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
| `patch_nso.py` | 1,856 | `nx-aot::patch`, `nx-loader` | PORT the cut, frame-step and observation-gate machinery as image patches. DROP the Apple rewrites (x18, tpidr, svc/cntpct gates, sp alignment, exclusive ordering): the lifter handles them. **Keep the Trinity rendezvous fix** (`trinity_completer_sites`): the original code stores a flag, then loads a state word, with no barrier, and x86 also reorders a store before a later load. Plain lifted accesses are not atomics, so `nx-aot` detects the same site and emits a `SEQ_CST` fence there. This is verified on Violet and Z-A in M6 (lane O3, 2026-11-20: the Trinity fence is in: `cli::profile::trinity_sites` finds nxnative's exact completer shape in every module and the table gets a two-word site, the store then `dmb ish` (lifted as a SEQ_CST fence), set `trinity`, `NXRS_PATCH_TRINITY=0` off; Violet's main: one completer at main+0x2d0010c, `cli profile::trinity_tests`; takes effect at its next build. FRAME_STEP / STUBS: see 5A.1; observation gates not yet) |
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
| XCI (and its secure partition) as the game's own package, not only as update/DLC. The README's "refused, as in Turbo" is wrong | `nca.py` `open_ncas`, `dlc.members` | Partial (audit 2026-11-20: was Done; committed in 7360d85, but never built and run as a game, which this section requires, and the launcher's `validSource` refuses `.xci`) lane T: XCI with or without key area; NCA compressed storage added (the owner's only XCI, Super Mario Party Jamboree, needs it): ExeFS + 1,539 RomFS files read, all 139 NROs match the game's NRR hashes. Also homebrew `.nro` input. Not built and run as a game |
| ZBIC-compressed NSO segments (Switch 2: Ocarina, and any other); **one** decoder shared by translator and shaders (shaders has its own today) | `zbic.py`, `nxloader.py` (3eac6c2, b196d14) | Done (lane T; audit 2026-11-20: committed): `translator::loader::zbic`, shaders re-exports it. OoT translates (297,043 functions), builds with shaders (42,848) and runs: main staged unpacked, loaded, DT_INIT runs; then an nn::diag abort in sdk init after GetInfo 29/30/33 (os) |
| The 6 untyped SDK crypto symbols seeded (os `bignum.rs` workaround then removed) | — | Done (audit 2026-11-20: committed; `bignum.rs` is gone, so the lane O note is stale) lane T: seeded; MK8D's six are lifted functions. Removing `bignum.rs` is left to lane O (it is editing `services/mod.rs`; INTERFACES.md) |
| `nxrs build` runs the shader prebuild; profiles' shader tables imported | lane C `shaders::prebuild` | Done (lane T; audit 2026-11-20: committed): MK8D 29,651 shaders at build, the game ran on them at ~60 FPS; 187 profiles' tables embedded (24.7 MB), written to `<data>\shaders\profile\`; nxrs now links `nxfront.lib` |
| `--key=value` applies any ini option for one run | e4b388d | Done by lane I (`nxconfig`); lane T: argv passes through, the runtime skips `--k=v` when finding its folder, and applies `memory_gb`, `null_trap`, `log`, `profile` |
| Settings versions and their migrations (`settings_version` 3: `game_dynamic_resolution` auto) | 01f9119, 81a1d95 | Done by lane I (`nxconfig`); MK8D's ini rewritten at version 3 in lane T's run |
| Per-game memory cap (job object), allocations counted | 8a4618c | Done (lane T; audit 2026-11-20: committed): `process memory cap: 32768 MB` in MK8D and OoT runs, raised to the pool + 8 GB for memory_gb. Counting vkAllocateMemory is lane G's |
| Every `switchrecomp.py` option has its nxrs equivalent or a launcher-agreed replacement (`--chunk`, `--keep-c`, `--no-shaders`, `--dlc-stub`, …) | `switchrecomp.py` | Partial (audit 2026-11-20: was Done; a flat folder looks for `romfs/` in its parent, a folder without a RomFS fails, a folder's `control.nacp` / icon is not copied, `SWR_CFLAGS` is absent, a shader-prebuild failure fails the build where Turbo exits 0; gap rows below. Lane J: execution hints (`discovered.tsv`) and `--interp-only` are back, see the hints row) lane T: `--chunk`, `--native`, `.nro` input added; the rest as listed in src/cli/README.md |
| `doctor`: Vulkan device version, free disk | — | Done (lane T; audit 2026-11-20: committed): devices with API/driver version (1.3 needed), free disk, toolchain, Visual Studio, engine libraries |
| Toolchain for machines without Visual Studio (runtime for `x86_64-pc-windows-gnullvm`, llvm-mingw bundled) | a1c5d81 (Turbo bundles its compiler) | Done (lane T; audit 2026-11-20: committed): `NXRS_TOOLCHAIN=gnu` / no Visual Studio → bundled llvm-mingw 20261104 + the engine built for gnullvm (`gnu\libnxfront.a`); MK8D built so imports only system DLLs and the UCRT and ran 60 s at 60 FPS; homebrew tests pass. Release packaging (bundling llvm-mingw + gnu\) is M9 |
| Exclusive monitor on for Switch 1 programs only; LDAR/LDAPR/STLR/LDXR as atomic accesses — confirm the translator's equivalents | b89e9e5, efa9b2a, e440705 | Done (lane T; audit 2026-11-20: committed): global monitor (`rt_excl_mon`) on for MK8D, off for OoT (run logs); LDXR/LDAR/LDAPR acquire, STLR now seq-cst |
| Runtime: the 5 requests os filed (SVC hook, tick rate incl. Switch 2 tick, scheduler hooks, start entry, kernel events), os workarounds removed | `docs/INTERFACES.md`, 78ece7b | Done (lane O3, 2026-11-20): os uses all five: SVC registry and fallback (`svc::install`), tick rate and guest clock (`set_tick_hz`, `set_clock`), the scheduler (`sched::Cores`), kernel events (`kobj`), and now the start entry too: `nxrt::process::start_main(space, npdm, Entry::Host(boot_main), setup)` (new; `start` uses it) replaces os's own `create_main` call, os's counter rate and virtual cores set in its `setup` before the main thread exists. os 86 / runtime 26 tests pass; MK8D 133 s through boot, menus and into a Grand Prix race, exit 0, 0 gaps (`build/I/runs/20261120-021410-O3-mk8d-start`). The SVC fallback answering success is fixed (row "Unknown SVCs answer" below) |
| Map past the application pool fails with ResultLimitReached; SDK 22 thread ID; WaitIfEqual64 | b316a9f, 78ece7b, 362b172 | Done in the runtime (lane T; audit 2026-11-20: committed): map limit on for Z-A/OoT (OoT log), TLS+0x110, WaitForAddress type 3; os answers memory SVCs itself (INTERFACES.md note to lane O) |
| DLC whose data is missing at start is not offered to the game (MK8D hangs otherwise); `os/src/services/fs.rs` `aoc_indices` offers every manifest title | 2cdeabd, `runtime/dlc.c` `dlc_has_data` | Done (lane O3, 2026-11-20): `fs::aoc_indices` offers only titles whose RomFS opens (stubs still listed), a `[RED] dlc: ... not offered` line for the others, decided once; test `fs::tests::dlc_with_missing_data_is_not_offered` |
| Folder input as Turbo: a flat folder (`main` inside), the RomFS optional, `control.nacp` and the icon copied from the folder or its `control/` | `switchrecomp.py:497-501`, `nxloader.py:431-433` | Done (lane O3, 2026-11-20): `translator::package::folder::{find_romfs, control}`: the RomFS is `romfs/` or `RomFS/` beside `exefs/` or inside a flat folder, and optional (an empty RomFS, no marker); `control.nacp` and the icon from the folder or its `control/`; `folder::tests::detects_exefs_layouts` covers both layouts |
| Saves of Turbo builds found by nxrs builds: Turbo's `<data>\save\user` against nxrs's `save\user<N>` (migration at boot or the 6.3 copy); keys fallback `%LOCALAPPDATA%\nxrs` (launcher d538b31) instead of `RecompLauncher\RecompLauncher` | `runtime/svc_fs.c:511`; launcher d538b31 | Done (lane O3, 2026-11-20): user 1's save, the first time it opens and is absent, is copied from Turbo's `save\user` (or older `save\system`), the original kept (`fs::adopt_turbo_save`, test `turbo_save_is_adopted`); keys default `%LOCALAPPDATA%\nxrs\keys\prod.keys` first, then the old RecompLauncher path (`translator::package::keys::keys_candidates`, test `keys_found_in_the_launchers_data_folder`; cli / nxrs help text updated) |
| Engine CI (tests and native builds) and `scripts/port_map.py --check` run as a test; the launcher's release.yml passes `--nxrs-engine` | a9a4add `.github/workflows/engine-ci.yml`, 7f23568 | Not started (audit 2026-11-20) |
| Shader prebuild beside the compile and never fatal (a failure is a warning: the live translator covers it); `@progress data` and real shader counts | 6d04b3b `start_shader_prebuild` | Not started (audit 2026-11-20) |
| `SWR_CFLAGS`; reserved Windows names (CON, NUL, COM1, trailing dot / space) in ExeFS output; `--jobs` default the CPU count | `switchrecomp.py:440,768` | Not started (audit 2026-11-20) |
| Execution hints (`.hints`) and the interpreter fallback (`--interp-only`), or the owner's decision to drop them; `discovered.tsv` (section 3.1) does not exist | 40df1f9 `program_hints`, `runtime/main.c:38-50` | Done (lane J, 2026-11-20, uncommitted; owner decision 2 = Turbo parity, reimplemented): **interpreter** `runtime::interp` (Rust, no MZ Labs code): the lifter's decode and its own bit helpers (`xlate/util.rs`, shared by `#[path]`), the semantics of the C it emits (`simd.h` / `simd_fp.h` on their exact path; tables read from simd.h); entered by `rt_dispatch` for an aligned address inside a module with no lifted function, back in lifted code at the first known start, calls through the `nxrt_call_guest` barrier, SVCs through `nxrt_svc_impl`, an `rt_sj_owner` frame per entry (longjmp / landing-pad resumes find it by exact return sites and `.eh_frame_hdr` function). **Oracle** `nx-oracle --interp` (all 71 classes, dynarmic): 0 mismatches at FPCR 0, FZ, DN, RP, RM, RZ, DN+FZ+RZ and DN+FZ+RP (416,568 cases each) and 2.77 M cases at FPCR 0. **Homebrew e2e** (`nxrt-dev e2e`, 8 programs: threads, fibers, setjmp / longjmp from a host callback, C++ exceptions, atomics, TLS, float printf): all pass with `NXRS_INTERP_ONLY=1` (everything interpreted), as `nxrs-build --interp-only` builds, and mixed (`nxrs-translate --limit 1/3/12`: 24 of 24). **Hints**: every interpreted entry appended to `<data>\discovered.tsv` (module, offset, first 4 words as the version stamp); `nxrs-build` adds them as function starts when the words still match (`translator::aot::hints`; "Execution hints: N function(s) … translated now; M left out (another version)"), the generation key includes them. `--interp-only` (v1 and v2, hidden) builds no function. Not run in a game: no library game is known to miss code today |
| Host sampling profiler (`fnprof_hit` is empty, `g_fnprof` never set) | `tools/host_prof.py`, `SWR_HOST_PROF` | Not started (audit 2026-11-20) |
| Unknown SVCs answer NOTIMPL / NOTFOUND (today: success); real 0x30 / 0x31 / 0x37 resource limits, 0x6F GetSystemInfo, 0x80 SetGpuMemoryAttribute; 0x02 / 0x03 state seen by QueryMemory | `runtime/kernel.c:1013,1116-1130`, `mem.c` `mem_set_state` / `mem_set_attr` | Done (lane O3, 2026-11-20): the fallback answers NotImplemented (0xF601) with its gap line; 0x2E / 0x2F NotFound; 0x30 / 0x37 resource limits (pool, 800 threads, 800 events, 256 transfer memories, 64), 0x31 current values (memory in use, live threads: new `Kernel::thread_count`), 0x6F GetSystemInfo (Turbo's pool sizes and process id range), 0x80 SetGpuMemoryAttribute and 0x02 / 0x03 recorded per page run and reported by QueryMemory, with Horizon's address / size / permission / combination checks (`os::mem::Memory::{set_permission, set_attribute}`). Tests: `svc::tests::unserved_svcs_answer_as_turbo`, `mem::tests::permissions_and_attributes_are_recorded`; MK8D `build/I/runs/20261120-025044-O3-mk8d-mouse` boots and plays with them |
| Stuck-mutex healer (each frame) | `runtime/kernel.c:410` `kernel_heal_mutexes` | Not started (audit 2026-11-20) |
| nxnative patch machinery applied: MATCH rules by fingerprint (`nso_match`: Arceus's capture-demo crash fix, OoT's 3 movie cuts), FRAME_STEP / STUBS sets (MKW, OoT), observation gates, the Trinity rendezvous fence (Violet, Z-A) | nxnative `dm/tools/patch_nso.py:889-917,1474`, `nso_match.py` | Partial (lane O3, 2026-11-20): **MATCH cuts applied at build time**: `translator::aot::nso_match` reimplements nso_match.py (unwind-table functions, the normalised body digest, callee references, the JSON fingerprint) and reproduces nxnative's fingerprints exactly (`nso_match::tests::arceus_capture_demo_function_matches_nxnative`: Arceus 1.1.1's capture-demo function at main+0x16e33ec). `nxrs-build` places every `[[match]] kind = "cuts"` rule of the selected profile on modules without cuts of their own, and the SDK / engine-wide ones, as sites of set `match` (`NXRS_PATCH_MATCH=0` off; rules whose binding purpose is `address space`, nxnative's Apple limits, stay out): Arceus's capture-demo crash fix (5 cuts) and Ocarina's 3 movie rules (4 cuts at main+0x3b5c710..0x3b5cec0) match the owner's builds (`cli profile::match_tests`). They take effect at each game's next build (not rebuilt here). **FRAME_STEP / STUBS (lane O3, 2026-11-20):** `cli::profile::stubs` turns each evaluated `[[stub]]` of a build and each `[[match]]` kind `frame_step` / `stubs` (functions and `others` placed by fingerprint, `base` / `other:<label>` resolved, asserted sites checked) into table lines: `area` / `areloc` (the stub's code and relocations) and sites whose word is `@b:area:<off>` (sets `fstep` `NXRS_PATCH_FSTEP`, `stubs` `NXRS_PATCH_STUBS`). The translator (`aot::patches`) lifts each module's area as a code module of its own, `<module>_stubs`, after the last module (its first page the state page), the sites branch there when the set is on and discovery makes the return points function starts (`patches::tests::stub_area`, `emit::tests::stub_site_branches_to_the_area_and_back`). os maps it, writes the state page at each present and runs the step controller (`os::fstep`, nxnative's nvn_fstep reimplemented without the phone / Mac clock probes; the hold through `set_profile_interval` when it changes): `fstep::tests::controller`. `nxrs-build` writes `<data>rame_step.txt` (step_max, render scale, capabilities). On the owner's builds: Ocarina's frame ratio + dynamic resolution (79 words, 2 sites) and Mario Kart World's frame interval (20 words, 1 site) place (`cli profile::stub_tests`); both profiles say `fps_target 60`, so the frame step is on at their next build (not rebuilt here, no run). The 4 bodies not evaluated at export are ported as generators (cli `stubgen`; HW Age of Calamity's reproduces nxnative's recorded build entry). **Left:** a run on a rebuilt OoT / MKW. Observation gates: not built on purpose (every nxnative gate is an Apple address-space workaround, no profile records one; INTERFACES O3) |
| Profile game.tsv fields used at run time: `fps_target` / `fps_max` / `fps_hold` (43 profiles), `op_mode`, `perf_mode`, `core_preempt`, `alloc_big_align`, `game_env`, `launch_param`, `programs`, `tex_dyn_slots`, `shader_key_skip`, `draw_barriers`, `stdalloc_cap` | nxnative `dm/games/game.py` | Not started (audit 2026-11-20) |
| The title's version in the boot log; OoT's full-resolution built-in patch shown equivalent to the profile's | 13a2693 `svc_fs.c`; `nxloader.py:56` | Not started (audit 2026-11-20) |
| NROs decoded at run time (Mario Tennis Fever's boot stub decodes its NRO from memory): a capture boot during the build writes the image of a `LoadModule` with no built copy and the build translates it, instead of the load failing (`os/src/services/ro.rs:274`) | nxnative `dm/host/hle_ro.c:57-85` (`DM_NRO_CAPTURE`), `scripts/build_ipa_from_nsp.py:1636-1660` | Partial (lane J, 2026-11-20, uncommitted): os `ro` keeps the image of a LoadModule with no built copy as `<data>\nro\<sha256>.nro` (`ro::capture_to`, unit-tested); `NXRS_NRO_CAPTURE=<dir>` writes there and ends the run (the capture boot); `nxrs-build` reads captures with its NRO cache, and when the NRRs register NROs no file holds it boots the built game once (`NXRS_NRO_CAPTURE`, 120 s, the child killed by its handle; `NXRS_NO_CAPTURE_BOOT=1` skips) and builds again with what it caught. Left: a real capture boot (Mario Tennis Fever is not in the owner's library list) |
| The nxrs engine in the Linux launcher bundle (`nxrs-engine/` was Windows-only, launcher `docs/RELEASES.md:25`) | Eglantine `.github/workflows/build-linux.yml`, `release.yml:95-107` (AppImage + tar.gz per architecture); launcher `scripts/package-release.py` | Done (lane Q, uncommitted; owner 2026-11-20: as Eglantine, no real-GPU gate): engine CI packages `nxrs-engine-linux-x64.tar.gz`; the launcher's release.yml builds the engine and its dmsh_xlate on Linux and `package-release.py --nxrs-engine` stages it (Linux file list, tools/, no llvm-mingw), on by default |
| Linux arm64 (as Eglantine ships) | Eglantine `build-linux.yml` (`ubuntu-24.04-arm`); mkw-recomp's AArch64 Linux runtime (the reference for the port) | Not started: needs an AArch64 port of the runtime's assembly boundary (guest calls, `rt_dispatch` / `rt_sj_owner` / host entry) and fiber switch, today x86-64 System V only; the generated C, the os and graphics crates are portable |

### 5A.2 OS services, audio, video (lane O)

| Item | Turbo source | Status |
|---|---|---|
| Audio to a real output device (WASAPI); `audio_slowdown`; no underruns | `runtime/audio.c`, `audio_queue.h`, `audio_limit.h` | Done: MK8D on WASAPI 68.5 s, 67.6 s not silent, 0 underruns, limiter + smooth slowdown (`os/src/media/sink.rs`, `services/audren.rs`) |
| hwopus decoding through libopus (Turbo decoded silence before c406ae6) | c406ae6, `runtime/opus/` | Done (lane O3, 2026-11-20): `services::codec::tests::hwopus_ipc_decodes_real_opus_as_libopus`: an Ogg Opus file made by ffmpeg + libopus (`os/testdata/media/tone.opus`, 440 / 660 Hz stereo, 2 s, 96 kb/s) demuxed and every packet sent through the `hwopus` IPC service (GetWorkBufferSize, OpenHardwareOpusDecoder + DecodeInterleavedOld, OpenHardwareOpusDecoderForMultiStream + DecodeInterleaved with reset): the PCM equals libopus decoding the same packets sample for sample, and is the tone (SNR > 20 dB after the pre-skip). Still no built game decodes through it (MKW imports it, stops before a race) |
| Renderer mix fixes: a voice reaching no mix stays audible; mixes kept between updates; splitters | 8a265ab, a22b102 | Done (lane O3, 2026-11-20): the SDK 1.x path now has a host mixer (`services/audren_ipc.rs`, row below), and Turbo 8a265ab / a22b102 are in it, one unit test each: `mixes_are_kept_between_updates` (changed-mixes-only sections behind the 0x20-byte header, an empty update keeps every mix), `splitters_route_voices_and_mixes` (a voice and a mix through a splitter to several mixes with each destination's volumes; splitters kept when the next update sends none), `a_voice_reaching_no_mix_stays_audible` (no mix named and a splitter with no destination in use: the final mix's front pair). The in-process SDK renderer (SDK 2+) mixes with the SDK's own code, so nothing differs there |
| Video: VP9 (Switch 2 GSV, libvpx), AV1 (dav1d, nn::ndve), H.264 (menu previews), the Switch 2 video decoder's fences and back-pressure; Ocarina's movies on screen | f9c8469, 6c4578c, 5093a7d, 0ae160a, 9ce9b8e, `runtime/vp9`, `av1`, `oh264` | Done (lane O3, 2026-11-20) for the decoders and the Switch 2 protocol: real clips made with ffmpeg (`os/testdata/media`: VP9 and AV1 in IVF, H.264 x264 with B-frames, 192x112, 72 frames) through the service paths, every picture's NV12 equal (FNV) to ffmpeg's decode, end of stream flagged: `codec_video::tests::{vp9,av1,h264}_clip_decodes_as_ffmpeg` (the SDK client's protocol: image pools, Register, Submit until 3603 busy (back-pressure seen in each), Acquire, the sync handle waited on with TryWaitNativeSync (the fence), GetOutputBufferProperties, ReadDecoderOutputImageNv12, Release; H.264 pictures come in display order with their packets' times) and `movie::tests::movie_{vp9,h264}_clip_decodes_as_ffmpeg` (Acquire / Send input, Acquire / Release output; with all 16 output slots held, packets stay queued, no input buffer is free and no output comes). Not done: Ocarina's movies on screen (Ocarina deferred, M8) |
| AudioIn opens (silent capture device is not acceptable: a real device or the service's own "no device") | — | Partial (lane O3, 2026-11-20): `services::audio::tests::audio_in_on_the_real_capture_device_and_without_one` drives the AudioIn takeovers (ListAudioIns, OpenAudioIn, Start, two 10 ms buffers appended and released full, Stop, Close) on the real capture device, then the "no device" answer (nothing listed, the open fails). On this PC there is no capture endpoint (the DualSense's headset microphone is disconnected), so only the no-device half ran (against the PC's real absence and with the device hidden). Left: run the test once with a microphone plugged in |
| Services Turbo answers that os may not: `btm:u` interface (Violet 4.0.0), MiiEdit applet cancel, bsd event descriptors, time shared page entries 0x38, the game's SDK version read for the NVN API | 13a2693, 9ae9441, 362b172, 1f12977, 2f8f44e | Done (lane O3, 2026-11-20), one test each against Turbo's replies: time shared page `time::tests::shared_page_layout_as_turbo` (1f12977: ContinuousAdjustmentTimePoint at 0xD0, 0x38-byte entries, the current one's clock source id at +0x28 equal to the context's, multiplier / shift 0); MiiEdit over the applet IPC path (new `services/applet.rs`: appletOE / appletAE, the library applet creator, accessor, IStorage; `applet::tests::mii_edit_applet_over_ipc_answers_cancelled`: 0x100 bytes, result 1, index -1, as 9ae9441; PlayerSelect gives the console user); `net::tests::btm_u_hands_out_its_interface` (13a2693: IBtmUserCore, its four Acquire...Event commands answer true with an event); `net::tests::bsd_event_descriptors` (362b172: EventFd, Write, Poll POLLIN / POLLOUT, Read, EAGAIN 35, Close, SDK 22's registration 36); SDK version as before |
| Software keyboard (`keyboard_text`, the swkbd font) | `runtime/swkbd_font.h`, `svc_ns2.c` | Done (lane O3, 2026-11-20): the Win32 text box itself `swkbd::tests::win32_text_box_answers_what_was_typed` (opt-in, `--ignored`: it opens a window on the desktop; passed at 02:30, failed at 03:15 with the box not opening while the owner used the PC; a real window and message loop: typing replaces the selected default, Backspace, Enter answers, Esc keeps the default, the length limit); `NXOS_SWKBD=auto` answers the default at once. Pokémon Violet on a new save (save moved aside, restored after, backup `build/O3/saves/violet-0238`): language, look, then "What is your name?" -> 'Player' (auto) at 107-115 s and the opening plays on (Clavell, the Academy, the Paldea montage) (`build/I/runs/20261120-023853-O3-violet-new`, `20261120-024401-O3-violet-new2`). **Mom not reached:** the opening runs past the 5-minute run limit and the game writes no save before the house, so whether Mom appears needs one run of about 8 minutes (owner's call) or the owner's hand play Violet, the approved 8-min new-save run (`build/I/runs/20261120-032706-O3-violet-new3`, save restored and checked identical): name entry auto-answered at 107 s, the opening plays, the game's first save commit at 326 s, then the bedroom at the desk with "Get Up" (the left stick, not A) to the end at 527 s; no Mom in the room by then. Not settled: the next run should keep that new save (the 326 s commit) and move the left stick to get up (A cannot), then look for Mom. |
| Docked / TV mode by default; `docked`, `language` options | d5b36a1 | Done: from `nxconfig`, live docked change notifies the game; MK8D `oe: docked, TV mode, language en-US` |
| Delete the null NVN device; os hands NVN to graphics (with lane G) | rule 1.1.6 | Done: `nvn/null.rs`, `tex.rs` deleted; MK8D runs to its menus on Vulkan (407 graphics procs) |
| Focus changes to the game (message 15, `oe` FOCUS is never written) and applet focus enter / leave around host dialogs; RequestToRelaunchApplication restarts program 0 (today it ends the run) | nxnative `hle_oe.c` `dm_focus_set`, `dm_applet_enter` | Partial (lane O3, 2026-11-20): nxnative's focus design: the window's state (1 in front, 2 behind, 3 minimised) from the front end, 2 while a host applet is open over it (swkbd box, controller applet, MiiEdit, applets over IPC), one message 15 per change when the game's focus handling mode asks (not under 0 / 3); and FocusStateChanged queued at boot as Turbo does (Jamboree sets mode 2). `NXRS_KEEP_FOCUS=1` keeps a background run in focus; control `focus 1|2|3`. Test `oe::tests::focus_changes_notify_the_game`; MK8D `build/I/runs/20261120-030302-O3-mk8d-ctl` (in the background: focus 2, message 15 popped, the game played on). **Note for every lane: a game window behind other windows is now out of focus for the game.** Left: RequestToRelaunchApplication Owner correction (2026-11-20): nxrs is a PC app, so focus is a player setting: ini `pause_on_focus_loss` (default 0, written with its comment, read live). At 0 the game is in focus from boot and never told it lost it; at 1 the behaviour above (window in the background or minimised, or a host prompt open). `NXRS_KEEP_FOCUS=1` stays the harness override. Test `oe::tests::focus_changes_notify_the_game` covers both values and a live switch. Also options now: `controller_prompt` (default 1), `home_quit` (default 1), `keyboard_box` (default 1: the text box; 0 answers `keyboard_text` at once). |
| InlineKeyboard's 10 methods not taken over (SetMovedCursorCallback, SetWindowMode, alpha / scale / translate, touch, hardware keyboard, GetImage, GetWindowSize): GetImage reads applet shared memory | nxnative `hle_swkbd.c` | Not started (audit 2026-11-20) |
| Shared fonts per slot: CJK and the Nintendo button-icon extension font (one stand-in TTF today) | `runtime/main.c:676-707`; nxnative `make_shared_fonts.py`, `make_ext_font.py` | Done (lane O3, 2026-11-20): six slots (Standard, Chinese Simplified / Extended / Traditional, Korean, Nintendo Extended): each the player's `<data>\fonts\<Name>.bfttf` (decrypted as Turbo) or `.ttf`, else one system TTF (Arial, Segoe UI, Tahoma; `NXOS_FONT`); GetSharedFont / the pl getters / SDK 21's built-in fonts answer by slot (`misc.rs`, test `bfttf_decrypts`). The console's CJK and button-icon fonts are firmware: only the player's files give them |
| psm (battery / charger), set:sys, set 7 / 8 / 9 / 11, fatal:u, spl:, mm:u | `runtime/svc_core.c` (psm_disp, setsys_disp, :1048, :1069, :1240), `plat.c:196` | Not started (audit 2026-11-20) |
| bcat mounts an empty cache (today it fails 0x2A2); aoc 4-13 / 50 / 100 / 101; fsp 18 / 60-62 / 400 / 500 (SD card, BCAT save, save-data info, DeviceOperator) | `runtime/svc_core.c` bcat / aoc_disp, `svc_fs.c` | Not started (audit 2026-11-20) |
| vi `ListFrameHistory`, IHOSBinderDriver, indirect layer 2450 / 2451; a catch-all for unknown Switch 2 services | nxnative `dm/gfx/hle_vi.c`; `runtime/svc_vi.c`, `svc_ns2.c` | Not started (audit 2026-11-20) |
| Missing SDK takeovers: the H.264 init overload, `movie::Extractor::SetDataSource`, ssl GetBuiltInCertificates, PlayReport::Send (HLE_FORCE_DIRECT), ngc China, album share URL, Palma x20 | nxnative `hle_codec_video.c`, `hle_movie.c`, `hle_net.c`, `hle_offline.c`, `hle_hid.c` | Not started (audit 2026-11-20) |
| IPC audren / aud / audout / audin for SDK 1.x | `runtime/svc_core.c:740-898`; nxnative `hle_audren_sv.c` | Partial (lane O3, 2026-11-20): `audren:u` served (`services/audren_ipc.rs`, Turbo's audio.c / audren_update design reimplemented): GetWorkBufferSize (the service budget), OpenAudioRenderer, IAudioRenderer (rate, samples, mix buffers, state, RequestUpdate(Auto), Start / Stop, QuerySystemEvent signalled as blocks play, rendering time limit), IAudioDevice; a host mixer (PCM 8-32 / float / DSP ADPCM, pitch, biquads, wave buffers, mixes, splitters, buffer-mixer / biquad / aux effects, 5.1 sinks) paced 8 blocks ahead of the sound card. `audren_ipc::tests::audren_u_serves_an_sdk1_client` + the mixer tests. Left: aud / audout / audin over IPC; no SDK 1.x game in the library to run it |
| Audio RMS / peak line every 5 s; output device choice, mute, WAV dump | `runtime/audio.c:149-160` (9ce9b8e); nxnative DM_AUDIO_* | Not started (audit 2026-11-20) |
| Movie / NDVE / NDSR network run by the host (OoT's movies, agl anti-aliasing) | 9ce9b8e, `nvn/vk/vk_replay.c:1918-1930`; nxnative `nvn_ndve.inc` | Not started (audit 2026-11-20) |
| Engine frame lock (Unity / UE / NieR / fb paths, the fps_target hold), with the ini's frame-skip `fps_target` renamed so the profile key keeps its meaning | nxnative `hle_fpslock.c` | Partial (lane O3, 2026-11-20): `os/src/fpslock.rs`: nxnative's fixed rows per main build (RE Engine, NLG, Criterion x5, FIFA; frame rate / vsyncs / ms / ticks / seconds / mode / double kinds with its value rules), NieR:Automata 1.0.3's interval mode, present interval and resolution floor, and Frostbite's SimulationTimeSettings found by name in any build; written at every present to the profile's `fps_target` (`NXOS_FPS_TARGET` overrides). Tests `fpslock::tests::{field_values, frostbite_by_name}`. None of the owner's games has a row, so no run shows it. The generic target frame rate a game steps by (nxnative fps_target.py) is found on main as loaded and written each present (`fpslock::target_field_find`, test `target_field_by_its_readers`, lane O3 2026-11-20). Unity, Unreal (console variables, fixed frame rate) and Monolith finders ported (cli `fpsfind` at build time -> `fps_lock.txt` -> os `fpslock`; synthetic tests; Anime Girls' Unity 2021.3.28f1 main has no reviewed registration form, so nothing is written there). **Left:** a run on a game whose engine one of them finds and the ini / profile `fps_target` name clash |
| Cheats: Atmosphère `.txt`, the dmnt VM, master codes, `.enabled`, `.meta.tsv`, the apply thread and a panel; code cheats as build-time image patches (risk R2) | nxnative `hle_cheats.c`, `front_mac.m` | Partial (lane O3, 2026-11-20): `os/src/cheats.rs`: the dmnt VM (opcodes 0-A, C0-C4 with the reference's word order, conditionals and else, loops, register / static / saved registers, key conditions and edges, float arithmetic), the file (`<data>\cheats\<build16>.txt` or the Atmosphère tree; `NXOS_CHEATS` another folder), master codes, `<build>.enabled` toggles (written when they change), `NXOS_CHEATS_ON`, one pass per present, control commands `cheats [reload]` / `cheat on|off <name>`; a cheat whose region is not set up yet waits, a failed access is logged once and retried, an unsupported opcode turns that cheat off with the reason. Tests `cheats::tests::{parse_entries, vm_opcodes}`. Code cheats write the bytes but not the translated code (said once). **Left:** `.meta.tsv`, the front's cheat panel (lane Q / front), code cheats as build-time image patches (risk R2), opcodes FF0-FFF (pause / log) |
| MiiEdit answers "cancelled" as Turbo (today `ok`); ShowUserCreator | 9ae9441; nxnative `hle_account.c` | Done (lane O3, 2026-11-20): over the applet IPC path (`services/applet.rs`, test `mii_edit_applet_over_ipc_answers_cancelled`, Turbo's exact 0x100 bytes); by name (`mii.rs` AppendMii) as before. ShowUserCreator: not started |

### 5A.3 Input, window, front end, harness (lane I)

| Item | Turbo source | Status |
|---|---|---|
| Real controllers in `hid`: XInput, DualSense / DS4 (lights, rumble, motion), Switch Pro, Joy-Con (1 and 2), WinMM fallback; hot-plug; several players | `controller_sony*.h`, `svc_hid.c`, `hid_layout.h`, 88401aa, a0581fe; nxnative `pad_*.h`, `switch2_proto.h`, `motion_fusion.h` | Done: all paths written and unit-tested (XInput; DualSense / DS4; Switch Pro, Joy-Con 1, NSO pads; Joy-Con 2 / Pro Controller 2 over BLE; WinMM; hot-plug; 8 players). DualSense (USB, fw 0630) on hardware: player 1, input and motion streaming, output reports byte-identical to Turbo's, buttons and rumble (low, high, both) confirmed by the owner by hand 2026-11-19 (`nxinput rumble`); MK8D's own rumble reaches the pad (1,138 vibration calls, motor bytes up to 147/255). Pad kinds without hardware here (Xbox, DS4, Switch Pro, Joy-Con 1 / 2) are fixture-tested only |
| `button_layout` (switch / xbox / positional) and `button_map` | 08130c4, 10e1ef4, ac1bd36, `pad_layout.h` | Done: the owner's `pad_layout.h` ported with tests, applied to every pad path; the owner's presses on the DualSense map as expected (`nxinput read`, 2026-11-19); the game read the mapped buttons in MK8D |
| Keyboard and mouse as a controller: `mouse_right_stick`, `mouse_sensitivity`, `mouse_invert` | `backend_win32.c` | Done (lane O3, 2026-11-20): with `--mouse_right_stick=1`, real mouse raw input (WM_INPUT) reaching the right stick, logged by the front end (`input: mouse raw motion -4 -18 over 17.1 ms -> right stick -32767 32767 (sensitivity 100%; ...)`), in MK8D (`build/I/runs/20261120-025044-O3-mk8d-mouse`); `mouse_invert` / `mouse_sensitivity` changed live in the same run (`run mouse_invert 3`, `run mouse_sensitivity 3.00`); keyboard as before |
| Game window: fullscreen / windowed, F11, title bar state, Esc quit, F1 help; windowed loader exe (no console) | `backend_win32.c` | Done: MK8D windowed, F11 / Alt+Enter fullscreen and back (kept in the ini), title states, F1 page, Esc quit; loader is a GUI-subsystem exe |
| Hotkeys: F3 screenshot (`captures/`), F4 fps limit, F7 recording (`smooth_recording`, `recording_fps`) | `backend_win32.c`, `rec_win32.c`, `rec_flow.h` | Partial (audit 2026-11-20: was Done; F3 and F7 take window grabs (`front/capture.rs`), not the game's frame, and F7's audio is taken after slowdown and limiter; F5's dump has no thread states; F8 / `shot` through graphics are right) F3 (PNG of the game), F4 (`fps_limit` + title), F7 (H.264 + AAC MP4; `smooth_recording` slowed the clock to 0.30 and kept every frame distinct, A/V lengths 1.97 / 2.07 s), F8 (graphics' shot) verified in MK8D |
| The ini: every Turbo option read and honoured (`renderer`, `resolution`, `dynamic_resolution(_min)`, `game_dynamic_resolution`, `shadows`, `fps_target` incl. frame skip 30/20 and 120 = LSFG x2, `fps_limit`, `forced_vsync`, `memory_gb`, `driver_exclusive`, `shader_thread`, `low_priority_shader_compiler`, `lsfg*`, `lossless_scaling`, `lossless_path`, `log`, `diag`, `profile`, `metrics`, `null_trap`) and written with comments when missing | `runtime/main.c` option table | Partial: every option read, normalised, published (`nxconfig`), written with comments, migrated, reloaded live, `--key=value` (all verified in MK8D); applied today: front / input (`fullscreen`, `mouse_*`, `button_*`, `smooth_recording`, `recording_fps`, `lsfg*`, `lossless_*`, `metrics`, `diag`), lane O (`docked`, `language`, `keyboard_text`, `audio_slowdown`), lane G (`fps_limit`, `forced_vsync`, `shadows`, `game_dynamic_resolution`); still unread by their lanes: `resolution`, `dynamic_resolution(_min)`, `fps_target` frame skip, `driver_exclusive`, `shader_thread`, `low_priority_shader_compiler` (G), `memory_gb`, `null_trap`, `log`, `profile` (T / O) (audit 2026-11-20: stale: every option listed is read now; left: `lsfg_resolution` only logged, `diag` writes gaps and pads only, `renderer` silently forced to vulkan, the hidden keys `script` / `heap_max_mb` / `loop_limit` / `show_game_rt` / `shader_process` and `-v` absent). Lane O3 2026-11-20: `memory_gb`, `null_trap`, `log`, `profile` honoured as Turbo's main.c (runtime `process::startup`), one unit test each: `process::option_tests::{memory_gb_sets_the_application_pool, null_trap_page_zero_never_maps, log_two_traces_everything, profile_starts_the_profiler}` Lane G4 (2026-11-20): `fps_target` frame skip, `driver_exclusive`, `shader_thread`, `low_priority_shader_compiler` were already read by graphics; now `shader_thread=0` also stops the shader-part and optimizer threads, `low_priority_shader_compiler` covers the optimizer threads too, and the device line logs all four (`[vk] graphics options: driver_exclusive auto (off), shader_thread 1, low_priority_shader_compiler 1, fps_target 60 (frame skip 0)`, plus a line when the frame skip changes). `renderer=opengl` (or any non-Vulkan value) now says once that Vulkan is used (config `options.rs`, a small change outside graphics) |
| Frame generation through the owner's Lossless.dll (LSFG) | `runtime/lsfg.c` | Partial (owner decision 2026-11-19: Eglantine's design is the default): **built-in frame generation** on the presented frames in graphics (`vk/framegen.rs`, own shaders `fg_luma` / `fg_flow` / `fg_interp`: luma pyramid, coarse-to-fine block matching, warp from both neighbours with occlusion fallback; pacer with target / multiplier / credit / hitch hold; generated frames presented before the game's frame, spaced by their own thread). **LSFG mode** (`frame_gen_type=lsfg`): the player's Lossless.dll passes (front's set handed to graphics) run as Vulkan compute with push descriptors (`vk/lsfg.rs`, lsfg-vk's pass graph), falling back to the built-in mode when the DLL is missing or fails. Options: `lsfg`, `frame_gen_type` (new), `lsfg_target`, `lsfg_multiplier`, `lsfg_flow`, `fps_target=120`. MK8D: built-in base 60.1 → output 119.5 FPS (`build/G/runs/r25`), LSFG (the owner's Lossless.dll from Steam) base 59.0 → output 117.2 FPS (`r26`), a generated LSFG frame `build/G/runs/r23/fg-generated.png` beside its game frame `fg-current.png`; both runs overlapped other lanes' builds, so their slow stretches are not frame generation's |
| Control pipe (scripted actions from the harness) | 0b0d902, d54f04e | Partial (audit 2026-11-20: was Done; the basic commands work, but Turbo's `vk_ctl.c` debugging commands (flight, pause / step, hide / show, gputime, rawdump, passview, watch, drawstats, texdump, wpage, wtrace, bp, rdoc, inpool, texat, switches) are absent and `set` / `get` mean ini values, not graphics switches) `\.\pipe\nxrs_ctl_<pid>` + registry; status / press / pads / shot / quit verified in MK8D |
| Metrics, dropped-work counters, input record / replay | 8f5410f | Partial (audit 2026-11-20: was Done; record / replay is Done, but `nxconfig::metrics::counter` / `add` / `frame` have no caller in any crate, so the counter columns and `frames-*.csv` are always empty; Turbo's guest CPU, busiest-thread and wait-by-kind columns are absent) metrics CSV per second (verified), counter registry for every crate, frames CSV sink; NXRS_INPUT_RECORD / REPLAY verified in MK8D (replayed states read by the game) (lane O3 2026-11-20: counter producers and the frames CSV are wired, see the row "Metrics: counter producers") |
| Harness: guard (GAME_LOCK, memory watchdog), scenarios with frame-time statistics, per-run ini, the pacing line at least every 5 s, PresentMon | dec5a09, 4f0f731, `tools/play_scenario.py` | Done: run_game.py guard as a module (waits for builds: cargo / rustc / clang / nxshaders / nxrs), play_scenario.py (Turbo's scenarios, per-run ini, stats), gamectl.py; PresentMon 2.6.0 (Intel-signed, build/tools/presentmon) records the game unelevated: MK8D scenario 1,147 presents, 45.7 FPS, p50 19.0 / p95 31.7 / p99 87.5 ms over 100-140 s; pacing gaps ≤ 5.07 s |
| Touch screen: mouse = touch with the short-press latch, scripted touch, gestures (`hid.rs` `touch_fill` is always idle) | nxnative `hle_touch.c` | Partial (lane O3, 2026-11-20): the touch screen is fed: the mouse's left button over the window (not while it is the right stick), scaled to 1280x720, a press shorter than 60 ms latched (nxnative's short-press latch), Start / End attributes and a finger id per press (`input::{set_touch, touch_now}`, `hid.rs touch_fill`); control pipe `touch x y [s]` and `drag x0 y0 x1 y1 [s]`. Test `input::touch_tests::touch_press_latch_and_release`; MK8D `build/I/runs/20261120-030302-O3-mk8d-ctl` logs the first press. Left: gestures (still idle); a game that reads touch run by hand Gestures (lane O3, later): the gesture recognizer behind GetGestureStates (`services/gesture.rs`, nxnative hle_touch.c / yuzu's rules for one finger: Touch, Press after 0.5 s, Pan, Tap / double tap, Swipe over 400 px/s with its direction, Complete, Idle; 8 px slop), test `tap_press_pan_swipe`. Pinch / rotate need two fingers (the host has one). |
| Controller-support prompt that waits for players to join (today it answers at once); Home held 2 s -> quit prompt (`input::home_held` has no caller); user-select dialog (today user 1) | nxnative `ctrl_prompt.h`, `home_quit.h`, `user_select_mac.h` | Partial (lane O3, 2026-11-20): nxnative's state machines reimplemented and tested (`os::ui::{CtrlPrompt, HomeQuit}`, tests `controller_prompt`, `home_quit`) with a small topmost prompt window over the game (`os::ui::show`). The controller applet waits on the prompt when fewer players than the minimum are in (`hid.rs ctrl_prompt`: a controller pressing a button joins, A / Return done with the minimum in, B / Escape cancels, the maximum or no waiting controller finishes; the game reads no pads meanwhile, `input::set_prompt`); `NXOS_CTRL_PROMPT=0` answers at once (scripted runs). Home held 2 s opens "Quit the game?" (front `window.rs home_quit`, A / Return quits; `NXOS_HOME_QUIT=0` off). Not run in a game yet (no run with pads). Left: the user-select dialog (user 1 today) |
| Rumble on / gain / cutoff, press to join, pad merge, keyboard player; vibration-device init sends an idle vibration | nxnative `docs/settings.md` DM_RUMBLE*, DM_PAD_*, `hle_hid.c` | Not started (audit 2026-11-20) |
| F3 / F7 from the game's frame (not a window grab), F7 audio before slowdown, the recording stall watchdog | `backend_win32.c` `g_shot_req`, `rec_flow.h`, `rec_audio_time.h` | Partial (lane O3, 2026-11-20): F3 writes the game's presented picture (graphics' shot at the next present) to `captures\screenshot_*.png`; F7 records graphics' frame sink (the presented picture read back at the present, `set_frame_sink`) instead of a window grab: MK8D `build/I/runs/20261120-030302-O3-mk8d-ctl` `record start/stop` saved 1920x1080, 4.5 s video + 4.5 s sound, 244 game frames at 1.4 ms. Left: F7 audio before slowdown, the recording stall watchdog |
| F5 / `diag`: thread states (`nxrt::kernel::dump_threads`) and render targets; threads.txt every 15 s, a stall dump at 6 s, the driver-crash fatal after 8 s frozen | `backend_win32.c:461`, `runtime/main.c:512-524` | Partial (lane O3, 2026-11-20): F5 writes every guest thread's state (`nxrt::kernel::dump_threads`) to the log and `captures\threads.txt` beside the gaps and pads. Left: render targets (graphics), threads.txt every 15 s, the 6 s stall dump (os dumps at 15 s), the driver-crash fatal |
| Metrics: counter producers (waits by kind, dropped work), guest CPU per thread, the busiest threads, xmon; frames CSV fed when `metrics=1` | `runtime/kernel.c:925-985` (8f5410f) | Partial (lane O3, 2026-11-20): producers wired: waits by kind with their time (`wait_sleep`, `wait_sync`, `wait_mutex`, `wait_condvar`, `wait_address` and `_us`, runtime `kernel::svc`), `audio_underruns`, `audio_slowed_frames`, `ipc_requests`, `gap_answers` (os, the `metric!` macro); `frames-*.csv` fed by os at every present (`#present,guest_s,frame_ms,submits,nvn_calls,threads`). Seen in MK8D `build/I/runs/20261120-025044-O3-mk8d-mouse` (metrics and frames CSVs, `counters`). Left: guest CPU per thread / busiest threads, xmon counters, graphics' own per-frame columns (lane G4) Per-thread guest CPU (lane O3, later): every guest thread's CPU time each second (`nxrt::profile::thread_cpu`, GetThreadTimes) as `cpu_t<handle>_ms` and `guest_cpu_ms` in metrics, and a `cpu: busiest guest threads over 10 s` log line (os `watch.rs`). |
| Control-pipe debugging commands: Turbo's `vk_ctl.c` set and nxnative's memdump, cvarf / cvari, drag, env, addr, texlist / texraw, bufsnap, atrace | `nvn/vk/vk_ctl.c:93-159`; nxnative DM_CTL | Partial (lane O3, 2026-11-20): os / front commands added: `addr <name>`, `memdump <addr> <len> <file>`, `env <NAME>`, `speed [rate]` (the guest clock), `focus 1|2|3`, `touch`, `drag` (MK8D `build/I/runs/20261120-030302-O3-mk8d-ctl`: all answered); `peek`, `threads`, `pads`, `press`, `counters` were there. Left: the graphics-side set (pause / resume / step, hide / show, flight, drawstats, texdump, inpool, texat, gputime, rawdump, passview, watch, rdoc: lane G4), `wpage` / `wtrace` / `bp`, nxnative cvars and texlist Write watches (lane O3, later): `wpage <page>`, `wtrace <from> <to>`, `wclear` (`os/src/wwatch.rs`: guard pages, each write logged with the lifted guest function that made it, the page guarded again after a single step, 600 lines; test `writes_are_seen_and_the_page_rearmed`). `bp` (breakpoints) is not done: guest code is translated, a breakpoint needs a host address per guest pc. |
| `lsfg_resolution` used; hidden keys `script`, `heap_max_mb`, `loop_limit`, `show_game_rt`, `shader_process`; `-v`; `renderer=opengl` / `eden` refused or decided by the owner instead of silently becoming vulkan | `runtime/main.c:203-214,449`, `backend_win32.c:67`, `gpu_gl.c:3434` | Not started (audit 2026-11-20) |
| NvOptimus / AmdPowerXpress exports; display kept awake while focused; screen refresh match; released pad input on focus loss | `runtime/entry.c` (6aace78); nxnative `front_mac.m` DM_SCREEN_MATCH | Not started (audit 2026-11-20) |
| Graceful exit order (audio pause, graphics drain) and the console control handler; minidump on a crash; probe breakpoints | nxnative `hle_lifecycle.c`, `crash.c`, `probe.c` | Not started (audit 2026-11-20) |
| Nintendo pads (Pro Controller, Joy-Con 1 and 2, Pro Controller 2) read by their printed labels whatever `button_layout` says: today `xbox` swaps them too (`os/src/input/mod.rs:877`, `input/layout.rs`) | nxnative `dm/host/pad_apple.h:1-19` | Done (lane J, 2026-11-20, uncommitted): `layout::for_nintendo` (the switch table plus `button_map`) for every `Kind::Nin` / `Kind::S2` pad, `button_layout` for the others; the default stays `switch` (owner). Test `input::layout::tests::nintendo_pads_by_label`; not run with a pad |
| Per-game defaults for player options from the profile (frame generation, console mode, frame rate), shown as the defaults, with "reset to tested defaults"; the ini wins (`profiles::player_settings()` has no caller, AUDIT.md:346) | nxnative `docs/settings.md:14-20,221-223`, `dm/games/settings.tsv`, `settings.py`; Eglantine `src/common/game_presets.h:46-135` | Dropped (owner 2026-11-20: settings are global) |
| `mute_on_focus_loss` (default 0) beside `pause_on_focus_loss`; nothing presented while the window is minimised (the game not told unless `pause_on_focus_loss=1`) | Eglantine `src/qt_common/config/uisettings.h:118-131`; nxnative `dm/host/front_mac.m:1702` | Done (lane J, 2026-11-20, uncommitted): the ini option (Controls, global, live) mutes the sound card's output while the window is behind others or minimised (`oe::set_focus` -> `media::sink::set_muted`, a 10 ms fade; the streams and a recording go on, the game is not told). Test `media::sink::tests::mute_fades_to_silence_and_back`; the launcher lane adds the checkbox. Not presenting while minimised was lane G4's already (`vk/present.rs`) |
| Profile picker as an option (`ask_user`, default 0) when the game opens the user selector and more than one local profile exists (see the controller-prompt row); prompt texts (controller prompt, Home quit, picker) in the game's language | nxnative `dm/host/hle_account.c:63-90`, `user_select_mac.h`, `docs/env_switches.tsv:32,95` (`DM_CTRL_PROMPT_TEXT`, `DM_HOME_QUIT_TEXT`) | Done (lane J, 2026-11-20, uncommitted): `ask_user` (Controls, global) opens `ui::pick_user` from `ShowUserSelector` and the PlayerSelect applet when more than one profile is registered (D-pad / arrows, digits, A / Return, B / Esc; cancelled = the selector's ResultCancelledByUser / UiReturnArg 2); `ui_text`: the three prompts in the `language` option's language (14 tables), any line overridable by `NXRS_CTRL_PROMPT_TEXT` / `NXRS_HOME_QUIT_TEXT` / `NXRS_USER_PROMPT_TEXT`. Tests `ui::tests::user_picker`, `ui_text::tests::languages_and_overrides`. Not run in a game; the prompt window takes no keyboard focus (pads answer, as for the other prompts) |

### 5A.4 Graphics (lane G)

| Item | Turbo source | Status |
|---|---|---|
| Wire os → graphics; MK8D title screen and menu (= M4) | — | Done (`build/G/m4/mk8d-title.png`, `mk8d-main-menu.png`) |
| Virtual / sparse pools | `vk_sparse.inc` | Partial (lane G, uncommitted): virtual pools as before, and **Vulkan sparse images** for sparse textures of GPU-only sparse pools (`vk/sparse.rs`, mkw-recomp's `vk_sparse.c` ported: 64 KiB slots per physical page, aliased, binds before each submission on a sparse timeline, levels outside the tiles bound once). MKW: 31 sparse images (three 8192x8192x68 terrain arrays, 1.4-5.8 GB of address space each) with 1,663 MB of tiles bound; MKW used to be killed at 28 GB private 1 s after its first submit (it made 19 GB of whole images), now peaks at 8.4-8.9 GB and presents (`build/G/runs/w3`..`w7`; it then stops in game code: INTERFACES lane G notes). Pages of the address map skip sparse-pool textures (MKW's first frame took 76 s scanning them) |
| Buffer textures, query counters with the GPU's own time, append counters as indirect-draw arguments | 8097152, 7b5a28c | Partial: all three implemented (`vk/counters.rs`: timestamps stamped on the GPU and brought to the guest clock, SAMPLES_PASSED from occlusion queries; texel buffer views per draw and in the pool set; append resets + size-0 storage buffers) — MK8D runs with SAMPLES_PASSED counted (no red), GPU time logged; buffer textures and append counters not exercised by MK8D, DKB not runnable yet (audit 2026-11-20: also open: no idle-gap exclusion in the GPU times (283c6b6), timestamp records write the time into the counter field where Turbo writes 0, render conditions read addr+8 where Turbo compares addr and addr+16; gap row below) **(lane G4, uncommitted) Done for buffer textures and append counters:** GPU tests `buffer_texture_read_by_texel_fetch` (a usamplerBuffer over pool memory, 256 texels) and `append_counter_as_indirect_draw_arguments` (a vertex shader counts into a 64-byte record, DrawElementsIndirect draws the full quad from it; with `NXGFX_APPEND_RESET=0` the test fails, so it exercises the rule). Still no owner game seen using either |
| Transform feedback, layered passes, raw global memory through loaded pointers | 78ece7b | Partial (lane G, uncommitted): **layered passes** done (a pass has as many layers as its attachments' views; 3D targets are 2D-array compatible and drawn as arrays of slices, they were bound as 3D views); **transform feedback** replayed: the owner's capture variant (`vk/xfb.rs`, from Turbo 78ece7b: XfbBuffer / Stride / Offset on the captured outputs, narrower ones split out) bound with the feedback buffers around each draw, counters carry a capture over several draws (unit-tested; no running game captures yet: Z-A / Violet not built); **raw global memory** done (lane G3, uncommitted: `vk/gmem.rs`, Turbo 78ece7b / nxnative: the 8,192-entry memory pool table with its 2 MiB hints and the write log under constant slot 255, pool buffers with device addresses, a pool a shader wrote through a loaded pointer copied back to guest memory by the next submission, at most 64 MB each); MKW's two such shaders (c9a56f796ea4a4cd, ce770ef0597876ca) are not reached before its fault, DKB's not run yet |
| Recording on several threads; parked submissions at GPU event waits without red log lines | 362b172, 1316538 | Partial (lane G4, 2026-11-20: the deferred recorder `vk/defer.rs`, `NXGFX_DEFER=1`, measured again on MK8D 140 s exclusive: 8.2-9.7 ms of replay CPU a frame against 5.8-6.4 off, same FPS: **stays off**; lane G, uncommitted): **asynchronous submission** (Turbo's translation thread): submits and presents replayed on a `vk replay` thread, fences as op sequence numbers, SyncWait / ReadTexels / Finish drain (`vk/queue.rs`); MK8D runs on it (r19, r20). **Parked submissions without red lines:** a queue waiting on another parked queue's sync parks behind it (`nvnQueueWaitSync` receipts), a present on a parked queue is shown when its work has run, parked queues are retried after every replayed op (both red lines gone). Lane G3 (rev 7): Turbo's bounds (64 ops, 2 presents queued; the game's thread waits in the OS's blocking hook) and its SyncWait on a sync a queued command buffer fences (with a timeout only, until it is fenced or the queue runs dry); MK8D's demo 300 s at 60 FPS with it on, no stall (`build/G/runs/a1`; r22 / r24's stops did not come back); fences name their own queue's last op and resolve through a receipt while that queue is parked (a SyncWait passed before parked work ran: GPU test `event_wait_parks_a_queue_until_another_queue_signals` with async on); **on by default since rev 7** (`NXGFX_ASYNC=0` off): Z-A Lumiose 49.5-52.4 FPS against 42.5-48 sync (`build/G/runs/z6` / `z5`). Recording one submission on several threads: not done **(lane G4, uncommitted) Recording one submission on several threads: not done.** Turbo `vk_defer.c` (off by default in Turbo) records the vkCmd* calls of a submission on a recorder thread through a ring of copied commands. Here every vkCmd call in `vk/exec.rs`, `queue.rs`, `image.rs`, `present.rs` goes straight to ash on the replay thread, so it needs a recorder layer wrapping all of them (~70 command kinds). Not started for lack of time in this round; the async replay thread already takes the replay off the game thread **(lane G4, 2026-11-20 round 2) Recording on a thread of its own: implemented behind `NXGFX_DEFER=1` (off by default, as in Turbo), `vk/defer.rs`, Turbo `vk_defer.c`'s design:** the device and its loaders are loaded through a shadow `vkGetDeviceProcAddr` handing out shims for every vkCmd* this layer uses; on the submission's main buffer the replay thread's state / bind / draw / dispatch calls are queued (scalar ones as words, array ones with copies) for a recorder thread, the rarer ones (barriers, passes, copies, clears, queries, AS builds) first wait for it; the buffer is ended after it catches up. GPU tests: the 19 of `gpu` with `NXGFX_DEFER=1` and `tests/defer.rs` (200 draws, 8,401 commands queued, picture right, validation clean). In a game: MK8D ran 113 s at 60 FPS with it on (`build/I/runs/20261120-031555-G4-mk8d-defer`, killed by a session restart). **Measured (MK8D lap, `NXRS_EXCLUSIVE=1`, same build, frames 60-170 of the replay lines):** on (`20261120-033013-G4-mk8d-defer2`) replay 10.8 ms a frame, the game waited 3.8 ms, 57.6 FPS; off (`-mk8d-nodefer2`) 5.75 ms, 0.2 ms, 59.4 FPS. Slower: the waits for the recorder at every barrier / pass / copy (the replay puts a full barrier after most copies and passes) cost more than the overlap gains, and every queued command takes a lock. **Stays off by default.** Next, if wanted: batch without the lock (a single-producer ring as Turbo's) and defer the barriers and pass begins too (deep copies) |
| Write-back on demand, joined ranges (discrete-GPU model) | 6e86483, ad91c4d | Done (audit 2026-11-20: was Partial; on by default and evidenced on Violet `v12` and Z-A `z8` / `z9`; only DKB, the case it is for, is unrun) implemented (`vk/lazy.rs`: pages with 3 frames of GPU writes go PAGE_NOACCESS instead of being copied back, a fault reads the page back on a copy of its own; adjacent write-backs joined in `mem::note_gpu_write`); MK8D runs with it on, its write-backs do not reach the streak; DKB (the case it is for) not runnable. **Off by default since rev 6** (`NXGFX_WB_LAZY=1`): it crashed Violet (~35 s) and Z-A (~60 s). The race behind that (a second thread faulting on a page while it was being read back) is fixed. Lane G3 (rev 7): a write-back landing on a page held on the device leaves it there (bringing it back made the completion thread wait for all later GPU work), held pages are not protected again at every frame; Violet with it on is clean (`build/G/runs/v8` 150 s, `v9`) but 18-23 FPS against 30 with it off (`v7`): the 28 ms a frame left were `lazy::touch` visiting every page of each range it was given (rev 8: only the held pages): Violet 150 s clean at its 30 FPS cap with 0.5 ms of write-back a frame (`build/G/runs/v12`), Z-A 53.5-58 FPS against 47-48.5 off (`z8` / `z9`, exclusive runs). **On by default since rev 8** (`NXGFX_WB_LAZY=0` off) |
| A draw whose shader is missing waits for a live translation instead of being skipped | 336dcb7 | Partial (audit 2026-11-20: was Done; Turbo's cap on a frame's total shader waits (283c6b6, 1.5 s) is absent, only a 3 s cap per program set) (`vk/live.rs` + `shaders::prebuild::translate_one`, 4 translator threads, waited-for keys first): MK8D with an empty shader folder translated 1,821 shaders live (0.1 s each) and drew its title (`build/G/runs/r9`). That run lost the device at 17 s, not diagnosed |
| Storage use for images without the IMAGE flag (off by default), integer output variants, storage textures | caa07ad, 3d77358 | Done (lane G4, uncommitted): `vk/image.rs` `want_image` / `upgrade_storage` (Turbo caa07ad): a texture registered or bound as an image without the IMAGE flag marks its storage; images over it get storage use (an sRGB one through its UNORM twin, `EXTENDED_USAGE`), an image made before is made again with its content copied and the pool descriptors rewritten. **On by default** as Turbo now has it (`NXGFX_IMAGE_NOFLAG=0` / ini `image_noflag=0` / control `image_noflag 0` off). GPU test `image_stores_to_a_texture_without_the_image_flag` (RGBA8 and RGBA8_SRGB, image registered after the texture: 4096/4096 texels). **Integer output variants** (`shaders::link::int_outputs`, Turbo `vkspv_int_outputs` incl. 3d77358's late integer types): a pixel shader writing an integer target gets its float outputs retyped (pipe.rs `link_io`); GPU test `float_output_bits_reach_an_integer_target` (R32UI) passes, though this driver also passes the bits without the variant. "Storage textures stay storage by default" (Turbo `to_gpu` off) needs nothing here: a CPU texture stays read from storage until the GPU writes it |
| Push constants as driver bank 24; memory pool table 8192; queues / texture pools / mesh draws as Ocarina needs | 9ce9b8e, 4bb959d, 9f19968 | Partial: push constants as bank 24 (and NVN2's stage banks 25-29) are replayed (`vk/exec.rs`); the memory pool table has 8,192 entries (lane G3, `vk/gmem.rs`); Ocarina's queue / texture pool / mesh needs not checked (Ocarina does not reach its game scenes) |
| Ray tracing: DispatchRays and acceleration structures (NVN2), on lane S's ray-query library | — | **Blocked: the NVN2 argument layouts need a game that traces rays** (none of the owner's does; the driver is not in the games). Partial (lane G4, 2026-11-20): **the Vulkan side done** (`vk/rt.rs`): VK_KHR_acceleration_structure + ray_query enabled when offered (`NXGFX_RT=0` off), pool buffers usable as build input, bottom levels built from NVN2's own build description (`nvn::query::AsBuild`: triangles, boxes) and top levels from its instance records (bottom levels named by guest address, patched to device addresses), kept by the guest address built at (`rt::lookup` for the dispatch). GPU test `ray_query_traces_a_triangle`: a triangle from guest memory, a top level over it, 1,024 rays from a ray-query kernel: 528 hit, 0 wrong, validation clean. **Not done:** the NVN2 command entry points (BuildAccelerationStructure, DispatchRays and the rest) and running lane S's `rt_library` against `rt::lookup`: their argument layouts are still unknown (no symbolized implementation in the owner's games' sdk, none in Turbo or nxnative). No owner game uses ray tracing. Round 3: MKW's and DKB's `main` only load the names into pointers (proc loader at MKW `main+0xdd4f84`, DKB `main+0x5de2ec`) and never call them; the implementation is the system driver's, in neither `main` nor `sdk`: the layouts need a game that traces rays (INTERFACES lane G4 round 3) |
| Live alpha-to-coverage switch; per-title texture rewrites; present ticket race fix | 0b0d902, 362b172, a22b102 | Done (audit 2026-11-20: was Partial; all three addressed; the ticket race stays impossible with the replay thread on, as tickets resolve through receipts) `a2c 0|1` control command (and `NXGFX_A2C=0`); the ticket race cannot happen here (a present's ticket is written by the presenting thread, there is no translation thread); **per-title texture rewrites**: Done for every title (lane G3): Turbo reads a texture the CPU rewrites after creating it again only for two titles (`dm_game_tex_rewrite`); here CPU writes to a texture's storage reload it in all games (write watch and page hashes, `image_begin_submit`), and `nvnTextureWriteTexels` writes the storage (Turbo's `write_texels`) so every texture object over it reloads (MK8D's menu previews) |
| Resolution scale, dynamic resolution (host and the game's own), shadows option, vsync, fps limit | `render_scale.h`, 81a1d95 | Done (audit 2026-11-20: was Partial; every item implemented and run, only the host's dynamic resolution has no game run yet) `fps_limit` (0 / 30 / 60..240), `forced_vsync`, `shadows=0` (depth-only passes of their own size skipped) and the game's own dynamic resolution (`game_dynamic_resolution`: GPU timestamps, or a 256x slower clock when 0) read live from nxconfig. **Resolution scale and the host's dynamic resolution** (lane G3, uncommitted; Turbo has them in its GL renderer only, done here on Vulkan: `vk/scale.rs`): compressible 2D render targets at `resolution` x their size (0.25-4, live: scaled images are made again with their content blitted, the pool set rewritten), passes' viewports / scissors / clears scaled, the translator's render scales in the support buffer (texel fetches, image loads, texture sizes, gl_FragCoord), blits between scales, buffer <-> texture copies and texel reads through a bounce image, presents and screenshots cropped at the scale; `dynamic_resolution` steps 0.1 down to `dynamic_resolution_min` when the GPU is busy over 90% (Turbo's rule). GPU test `resolution_scale_renders_at_twice_the_size` (validation clean); in a game: MK8D at `--resolution=2` (`build/G/runs/m3`): the scene renders at 2x (3840x2160 targets, the picture sharper), 57-60 FPS in its demo |
| Screenshot (`vk_shot`), perf counters and the pacing line (`vk_perf`), render sizes in the log, pipeline cache on disk | `vk_shot.c`, `vk_perf.c`, `vk_pcache.c` | Done: `vk/shot.rs` (F3, `shot` command, `NXGFX_SHOT*`), `vk/perf.rs` (GPU time and replay lines every 60 frames, slow frames, render sizes every 600, `NXGFX_FRAME_CSV`), pacing line every ≤ 5 s, `vk/pcache.rs` (saved 2 s after the last new pipeline, atomic replace, header checked). MK8D `build/G/runs/r10`. Rev 6: a shot is taken before the present locks the submission state (the `shot` / `fullres` freeze); MK8D `r28` wrote a shot at present 6400 and played on |
| Alpha test applied in the shader (today recorded and logged red; hit in Smash) | `nvn/vk/vk_shader.c:747` `vkspv_alpha_test`; nxnative `dmsh_alpha.py` | Done (lane G4, uncommitted): `shaders::link::alpha_test` (Turbo `vkspv_alpha_test`): the pixel shader variant compares colour 0's alpha with the reference by NVN's function before each return, function and reference in push constants at byte 16 (appended to the module's own push block when it has one); `exec.rs` picks the variant per draw, the red line is gone; `NXGFX_ALPHA_TEST=0` / control `alpha_test 0` off. GPU test `alpha_test_discards_in_the_shader` (GREATER 0.5: the 0.3 half discarded, the 0.7 half drawn). Smash `build/I/runs/20261120-025107-G4-smash-lost`: no alpha-test red line |
| Dual-source blending (the SPIR-V edit; only the blend factors are mapped) — a lead for Jamboree's white layer; integer output variants | `vk_shader.c:611` `vkspv_dual_source`, `:625`, 3d77358 | Done (lane G4, uncommitted): `shaders::link::dual_source` (output 1 as Location 0 Index 1) picked per draw when target 0 blends with a SRC1 factor; a second source the shader does not give counts as zero (ZERO / ONE, nvn_mtl.m blend_zero_src1). GPU test `dual_source_blending_reads_output_1`. **Not Jamboree's cause:** its white quad blends SRC_ALPHA / ONE_MINUS_SRC_ALPHA (pass dump of `build/I/runs/20261120-022711-G4-jamboree-blend2`); MK8D uses no SRC1 factor. Integer output variants: see the storage row |
| Subgroup ballot made per lane (RTX 50 hang), on by default in Turbo | `vk_shader.c:199` `vkspv_ballot_per_lane` | Done (lane G4, uncommitted): `shaders::link::ballot_per_lane` (Turbo `vkspv_ballot_per_lane`) applied at module load, on by default (`NXGFX_BALLOT_FIX=0` off), a line per rewritten shader. GPU test `ballot_counter_made_per_lane_counts_right` on this RTX 5090 (the warp-aggregated counter after an early return: 170 of 170 counted) |
| Prewarm from the profile's pipeline manifest on a first run (the build writes `pso_manifest` / pairs / `shader_facts` and nothing reads them; `profiles::prewarm_shaders` has no caller) | 6d04b3b; nxnative `pso_offline.py` | Partial (lane G4, uncommitted): `pipe::prewarm_profile`, called after the key-file prewarm: on a first run (fewer than 64 pipeline keys) the shader parts of every program in `<shaders>/profile/shader_pairs.tsv` are made ahead on the shader-part threads, the pairs of `pso_manifest.tsv`'s seeded shaders first (`NXGFX_PROFILE_PREWARM=0` off; the prebuild memory budget applies). Not run on a first run of a game yet (Wonder's own manifest is empty; its pairs table has 14,603 pairs). `shader_facts.tsv` is still read by nothing (5A.5 row) |
| Linear render targets read back to the CPU (on in Turbo; MKW's crowds) | `vk_replay.c:2340` | Done (lane G4, uncommitted): `exec.rs linear_rt_readback` (Turbo `rt_readback`, on; `NXGFX_RT_READBACK=0` off): at the end of a pass each LINEAR / LINEAR_RENDER_TARGET colour target in a CPU-accessible pool is copied into its storage (with its stride), written back at the fence. GPU test `linear_render_target_reaches_guest_memory` (fails with the switch off). MKW, the game it is for, does not reach those draws yet |
| VRAM budget eviction and freeing superseded render targets (Turbo's default) | `vk_tex.c:570-600`, `vk_dev.c` `vk_budget` | Done (lane G4, 2026-11-20): `image.rs vram_budget` every 30 frames (VK_EXT_memory_budget): over 90% of the device-local budget (`NXGFX_VRAM_EVICT_PCT`, `NXGFX_VRAM_EVICT=0` off) every parked image (render targets and textures kept for a re-initialization) is freed, with a line. GPU test `vram_budget_frees_parked_images`. No game has reached the budget on this 32 GB card; superseded render targets are already taken over or parked, so the parked set is what there is to free |
| Sharp (contrast-adaptive) present upscale (Turbo's default; nxrs blits linearly) | `vk_present.c:230,333` | Done (lane G4, uncommitted): `vk/upscale.rs` + `shaders/upscale.comp` (Turbo present.frag mode 2 as a compute pass into an RGBA16F image of the letterboxed size, then a 1:1 copy into the swap chain), when the window is larger than the picture; `NXGFX_UPSCALE=standard` / ini `upscale=standard` linear blit, `NXGFX_SHARPNESS` 0..1. The window test (640x360 picture into the 2880x1620 window, `NXGFX_FRAMEGEN=0`) is validation clean; Smash ran on it (log line, 3 images = 92 MB). With frame generation on, its frames are still blitted linearly |
| Render-condition semantics, timestamp counter field 0, idle-gap exclusion in counter times, the per-frame shader-wait cap | `vk_replay.c:107,2235-2390`, `vk_submit.c:94`, 283c6b6 | Partial (lane G5 row check 2026-11-20): render-condition semantics (addr / addr + 16, pending reports, clears obey it) and the timestamp counter field 0 Done in 48b009b (lane G4, GPU test `conditional_clear_runs_once_after_a_timestamp_report`); idle-gap exclusion in counter times and the per-frame shader-wait cap not done |
| NVN procs: SetConservativeRasterEnable (a call jumps to null today), SetShaderScratchMemory, MemoryPoolBuilderSetNativeHandleNVX, SyncInitializeFromNvRmSyncNVX; RegisterFastClear* return 1 | `hle_nvn.c`, nxnative `hle_nvn.c:3278,3822` | Partial (lane G4, uncommitted): `nops.rs`: RegisterFastClearColor / Colori / Colorui / Depth return 1; SetConservativeRasterEnable (one red line: not applied) and SetShaderScratchMemory as procs. The two NVX natives are not done |
| Device-lost diagnostics: flight recorder, Aftermath, checkpoints, a report instead of a panic; RenderDoc; memory report; pipeline statistics | `vk_flight.c`, `vk_aftermath.c`, `vk_dev.c:184,285`, `vk_submit.c:85,273`, `vk_pipe.c:415` | Partial (lane G4, 2026-11-20): **checkpoints and the fault report done** (`vk/lost.rs`, Turbo `vk_lost`): VK_NV_device_diagnostic_checkpoints before every dispatch and indirect draw (`NXGFX_CHECKPOINTS=all`: every draw, `0` off) with what each was (shader keys, sizes, indirect address), VK_EXT_device_fault's addresses; on VK_ERROR_DEVICE_LOST (submit, the completion wait, present, acquire, sparse binds) the report is logged once and the process ends with exit code 71 (no retry, no panic). First catch: lane K's MKW reset was a task / mesh draw reading 0xca19c6000 (INTERFACES MKW section). **GPU-hang fixes:** robust buffer access on by default on NVIDIA (Turbo / Ryujinx; `NXGFX_ROBUST=0` off); every shader loop bounded (`shaders::link::loop_guard`, 65,536 back-edges a call, `NXGFX_LOOP_GUARD`; GPU test `loops_are_bounded`); indirect dispatch and mesh-draw counts held to the device's limits (`clamped_indirect`, GPU test `indirect_dispatch_through_the_count_clamp`). DKB with these ran 167 s past the point where it had lost the device (`-G4-dkb-hang4`). Flight recorder, Aftermath, RenderDoc, memory report: not done |
| Capture and replay without the game (for the colour bands, the cyan ground, the white layer) | nxnative `nvn_capture.c/.h/.inc` | Partial (lane G4, 2026-11-20 round 4: `vk/capture.rs` + bin `nxgfx-replay`, mkw-recomp's gfx/cap design: guest memory, pools, registrations, submissions and presents over N frames, replayed at the same addresses to PNGs; GPU-only content and sparse mappings not captured; first capture queued, not replayed yet. Before: nxnative's `nvn_capture` records the NVN streams and the memory they touch for an offline replay, a port of its own) |
| Frame step and its present tick (OoT, MKW) | nxnative `nvn_fstep.h`, `hle_nvn.c` | Not started (audit 2026-11-20) |
| Pool windows over maxBufferSize, sparse pool granules on first use, per-frame upload budget | `vk_mem.c:1178,1192`, `vk_dev.c:271` | Not started (audit 2026-11-20) |
| ASTC: BC7 modes beyond 6 (banding) and the disk cache | `vk_astc.cpp:180-330` | Not started (audit 2026-11-20) |
| GPU deswizzle, built-in counter kernels, real-time thread class (PORT_MAP marks these Done; nothing exists) | nxnative `nvn_deswizzle_gpu.*`, `nvn_builtin.h`, `nvn_rt.h` | Not started (audit 2026-11-20) |
| Frame generation, frame-level skip: no generated frame when the two real frames are identical or when most of the frame disagrees (a scene cut, a menu or dialogue box opening), then two warm frames before generating again (the per-pixel fallback alone still garbled Arceus's village text) | Eglantine `src/video_core/renderer_vulkan/present/fsr3_frame_gen.h:91-93` (duplicate frames), `frame_gen.cpp:26-27,242-244` (warm-up); the scene-cut check is new (neither reference has one) | Done in 768bb46 (lane G4: `framegen.rs`, `fg_stats.comp`; duplicate, cut and two warm-up frames; GPU test `frame_generation_skips_duplicates_and_scene_cuts`) |

### 5A.5 Shaders (lane G)

| Item | Turbo source | Status |
|---|---|---|
| The 3 failures: MKW's unimplemented Ampere instruction; Ocarina's and Violet's offset on a non-gather sample | — | Done: MKW's was a compute program starting with a 0x80-byte SPH (`job::is_ampere_sph`), the other two fetch with a computed offset (`fixes::dynamic_offsets`). Re-run: MKW 11,609/11,609, Violet 35,937/35,937, OoT 42,848/42,848 valid, 0 failed (`build/G/shaders/*.log`) |
| Real task / mesh shaders on Vulkan (`VK_EXT_mesh_shader`) instead of compute (MKW, DKB) | 9f19968 (Turbo's mesh draws) | Done in shaders (lane S, uncommitted): task / mesh keys are real `TaskEXT` / `MeshEXT` modules around the translated kernel (`mesh.rs`, nxnative's stage-memory layout): SPIR-V 1.4, outputs from the SPH maps, per-primitive attributes `PerPrimitiveEXT` (+ `mesh::fragment_per_primitive` for the pixel shader), invariant position, the draw's first workgroup read on the GPU from the `mesh_draw` records (DrawIndex, stride in push byte 12). MKW 19 mesh + 7 task, OoT 500 + 20, all spirv-val clean (DKB has none: 0 task / mesh keys). Contract for lane G in INTERFACES.md (shaders). Graphics' replay (lane G code: vk/exec.rs mesh_draw) must bind `mesh_draw` + push the stride and patch pixel shaders per INTERFACES (audit 2026-11-20: stale, the replay binds it: `vk/exec.rs` mesh_draw; only game evidence is missing); not yet seen in a game: MKW does not reach its mesh draws yet |
| Ray-tracing stages translated (NVN2) | — | Done in shaders (lane S, uncommitted): every stage-8 module linked into the game's one `rt_library` (`rt.rs`: RTCore metadata read and checked, logical IDs, MOV relocations, resource layout; nxnative's design), translated as one compute kernel tracing with ray queries (VK_KHR_ray_query + acceleration structures, not a ray-tracing pipeline: NVN2's continuation model is emulated in the kernel); reflection with the RT bindings and catalog (INTERFACES.md). A synthetic 3-module library translates and is spirv-val clean; no owner game has RT modules (Welcome Tour is the known one), so not seen in a game |
| Sonic X Shadow Generations' archive format; Unity, Unreal and the other engine readers nxnative has | nxnative `shader_split.py` | Done: PACx 405 (Sonic X Shadow Generations: 6,064 found, 6,064 valid, `build/G/shaders/sonic`), Unity (UnityFS LZ4/LZMA, serialized files), Unreal (paks incl. encrypted indexes, IoStore, `.ushaderbytecode`, inline, UE3), PlatinumGames DAT, Criterion BND2, RE Engine KPKA/SDF/OCIO, MT Framework ARC/SPK, KTGL/Koei/Gust, Smash data.arc, FromSoftware BHD5/BDT/DCX/BND3, RAGE rgxd, YEBIS tfxo, loose control sections in main. **Added (lane S, uncommitted):** MonoGame MGFX 8 / 5 in XNB (LZX, LZ4) and main, Monolith (ARH1/2, XBC1, cmp-zlib, HCPS, SLCT), Retro (RFRM / PACK, SNVN, LZSS in main), SUMO (Ninty_NX, NVNsh, MuxFDMR), CIVBSF, SHAD + CASC, Diablo III packages, Camelot CRA (index from NXSHADERS_NRO_DIR) + NARC, PKZ, RPF6 (key from main), Frostbite, BEA, NLG (rev 1 + patch, rev 4), IPK, Double Eleven packs, LSPK + BSHD, KIT, VBF + Phyre, Viper (zip / chained LZ4, VIPR), PDAT; unit tests on synthetic inputs. On the owner's games: Super Mario Party Jamboree (XCI v0, Bezel Engine 2.9 archives) 25,693 found (25,447 from .bea), all valid, every one of the 971 keys nxnative's run of the game used found; no other owner game uses these formats. Regression (build/S/shaders): MK8D 29,651, MKW 11,609, DKB 22,854, OoT 42,848, Z-A 28,603, Arceus 12,851, Violet 35,937 found, all spirv-val clean, 0 failed, 0 scan problems |
| Shader translation facts (`shader_facts.tsv`, manual facts) read by the translator; TT Games `ttdat` reader; c1_guard, cb_extent, shader_regressions, tex_census, ampere_orin tests | nxnative `dm/gfx/tools` | Not started (audit 2026-11-20) |
| nxnative's reference docs (13), `env_switches.tsv`, `settings.md`; tools check_thread_cuts, gpuprof, fps_target, monolith_fps, docs tools, fuzz corpus, bug_report, run_saves; jump_solver cases as translator tests | nxnative `docs/`, `dm/tools`, `scripts/` | Not started (audit 2026-11-20) |

---

### 5A.6 Frame rate: 60 FPS options (owner, 2026-11-19)

There is no universal 60 FPS patch: each game caps its rate in its own code, and a game that steps its world per
frame (not by measured time) runs at double speed when unlocked. Three layers, from universal to best quality:

| Item | Lane | What | Status |
|---|---|---|---|
| Frame generation, built-in (Eglantine's design: optical flow + interpolation compute passes + pacer, no game motion vectors, no paid software) as the default; LSFG with the player's own Lossless.dll as an option, falling back to the built-in mode when the DLL is missing | G | Universal: every game shows 60 (or more) while running at its own rate; input still responds at the game's rate | In progress |
| Forced present interval: a per-game profile switch (and ini option) that overrides the interval the game asks the window for (NVN `nvnWindowSetPresentInterval` / swap interval) so the game presents every vsync | G (switch in the present path), profiles (per-game flag) | Off by default; turned on per game only after a run shows the game keeps its normal speed at 60 (it steps by measured time). The profile records the result for each tested game, including "unsafe: double speed" | Done (lane O3, 2026-11-20, the wiring): the profile's `present_interval` (engine.tsv `display`, or 1..3; also the data folder's profile.tsv) is read into os's profile and handed to `nxgfx::nvn::window::set_profile_interval` at the graphics hand-off (`os::nvn::apply_present_interval`); `profile::tests::present_interval_reaches_graphics` (display -> forced 1, the ini's `present_interval` wins, off by default); MK8D with `present_interval display` in its profile.tsv logs `present interval forced to 1 by the game's profile` (`build/I/runs/20261120-025044-O3-mk8d-mouse`). Which games are safe: the next row |
| Mod and patch files: load community 60 FPS (and other) mods per game and version: exefs code patches as IPS / IPS32 / `.pchtxt`, keyed by the module's build ID, plus RomFS layering from `mods/<mod>/` (and the `mod.json` form the translator README names). Code patches are applied at build time the way the Z-A profile patch is (both versions lifted, a switch per patch set), so a mod can be turned on or off without a rebuild | P (implements it in the translator / cli / profiles; first users are the Pokémon games) | Per game and version; the best result (true 60) where a mod exists. Mods are the player's files: never shipped, never downloaded by the engine | Done (lane P; audit 2026-11-20: committed in 7360d85 / cb4b426): `profiles::mods` (IPS, IPS32, `.pchtxt`, `mod.json`, the Atmosphère / Ryujinx / yuzu folder layouts; unit-tested), the build (`cli/src/mods.rs`: `<build>\mods\<mod>\`, each changed `.text` word a switchable site of set `mod_<folder>`, data bytes and the RomFS folder into `<data>\mods.tsv`, conflicts with the profile's cuts left out with a line), the run (`os/src/mods.rs`: `NXRS_MOD_<NAME>=0/1` or a `disabled` file, data bytes written at load, RomFS layered in fs; modules.c checks the marker). Checked on Arceus with a test `.pchtxt` lane P wrote (main+0x2aa278c, the present interval): on = 60 FPS, `NXRS_MOD_ARCEUS60=0` = the game's 30, same build |
| Which games are safe at 60: test the forced interval and available patches on the 30 FPS games (Arceus, Violet; Z-A already has its profile patch) and record per game: safe / double speed / needs a patch | P | Results go into the profiles and the 5B table | Partial (lane P): results in `profiles/games/<id>/engine.tsv` (`fps60`; a safe game also gets `present_interval display`, which os now hands to graphics' `set_profile_interval`). **Arceus: double speed** (forced interval and the test patch both 59-60 FPS, the opening's dialogue ran ~1.9x faster: 18 s vs 33.5 s from the load to the name prompt with the same presses). **Violet: needs a patch** (it sets interval 2 every frame; forced to 1 it ran 33-40 FPS and its opening slowed). **Z-A: its profile patch** (60.0 FPS on title and menus). Not done: a 5B table entry per game |

### 5A.7 Owner decisions (researched 2026-11-20)

Ten open design questions, researched in Eglantine (`cf6cfe9caa`), nxnative (7ebb76e), Turbo and nxrs; the evidence
(file and line references) and the reasoning are in [DECISIONS.md](DECISIONS.md). Each waits for the owner; gaps found on the way are
Not started rows in 5A.1, 5A.3 and 5A.4 (marked "decisions research 2026-11-20").

| # | Question | Recommendation | Status |
|---|---|---|---|
| 1 | Frame generation default | Off by default (`lsfg=0`, one-time ini migration); on per game only where the profile records a clean run; a frame-level skip for duplicates and scene cuts. Eglantine ships it off and per-game switchable; nxnative has none; Turbo needs the player's DLL, so it is off in practice | Decided (owner 2026-11-20): **one global setting, off by default**; no per-game enabling (settings are global); duplicate / scene-cut skip kept |
| 2 | Interpreter fallback and execution hints | Turbo parity, reimplemented (Turbo's interpreter is MZ Labs' AGPL `codegen.py` / `a64.py`): an interpreter generated from nxrs's lifter for misses inside modules, `discovered.tsv` fed to the next build, a "rebuild" notice; plus nxnative's NRO capture boot | Decided (owner 2026-11-20): as recommended; all player settings global |
| 3 | Renderer choice | Vulkan only, said honestly: the launcher shows no OpenGL choice for nxrs games; a missing Vulkan device reported by `doctor`. Eglantine offers OpenGL but pins Vulkan for its Switch 2 games; nxnative is Metal only | Decided (owner 2026-11-20): as recommended; all player settings global |
| 4 | Docked vs handheld | Keep it the console mode the game is told (operation + performance mode), TV by default, offered as "sharper / lighter"; resolution stays separate; the profile's `op_mode` as the per-game default | Decided (owner 2026-11-20): **one global setting, TV by default**, offered as "sharper / lighter"; no per-game default from profiles (settings are global) |
| 5 | Controller layout default | `xbox` (as printed) for new inis, `switch` positions as the setting; Nintendo pads always by label. Both references and Turbo default to positions, so this departs from Turbo | Decided (owner 2026-11-20): **Switch positions stay the default** (as Turbo, Eglantine, nxnative); Nintendo pads still read by label (5A.3 gap row) |
| 6 | Console-style prompts | Keep `controller_prompt=1` (only when the game asks), `home_quit=1`, `error_notice=1`; profile picker as an option, off | Decided (owner 2026-11-20): as recommended; all player settings global |
| 7 | Build optimisation and build options | One measured flag set, no compiler options for players; the launcher hides (not greys) Optimization for nxrs games; player build options are content only | Decided (owner 2026-11-20): as recommended; all player settings global |
| 8 | Focus / background | Keep `pause_on_focus_loss=0`; add `mute_on_focus_loss`; release held pads; skip presenting while minimised | Decided (owner 2026-11-20): as recommended; all player settings global |
| 9 | Distribution | nxrs engine as binaries in the public launcher's releases (Windows now, Linux after M10), no engine source; licence check against PORT_MAP first | Decided (owner 2026-11-20): **the engine repo is public** (MIT, LICENSE added; licence check done: all third-party permissive, no GPL/AGPL code); launcher releases may bundle it without a token |
| 10 | Linux | Experimental until one library game runs on a real GPU with sound (Steam Deck / RADV and NVIDIA); CI build + tests stay | Decided (owner 2026-11-20): **as Eglantine** — CI builds and ships Linux packages, no hardware test gate |

## 5B. The LIBRARY phase: every game in the owner's library, played past the title

Owner, 2026-11-20 (supersedes the 2026-11-19 wording): the goal is to **finish development**; the library check is
that each game **gets past its intro videos into its first controllable gameplay**, not to play the game's story. A
game counts as **Playable** when a short run (a few minutes) shows: boot, intro videos and title passed, the first
controllable scene reached (scripted input through the control pipe moves the player / kart / character), no crash,
the picture correct (screenshots checked), memory under the watchdog, sound playing, and FPS from the pacing line at
the game's target. **No story progression** (no "reach the first town / the Fieldlands / a race 10 minutes"), no
scripted walkthroughs, no owner hand plays. Agents spend their time on engine development; a game run exists only to
reproduce or verify a specific engine fix. Sources are the owner's library entries (read only).

| Game | Package | Lane | Gameplay target | Status |
|---|---|---|---|---|
| Mario Kart 8 Deluxe | dump folder (+ update, Booster Course DLC) | W | Grand Prix race, 10 min, 60 FPS (= M5) | **Playable (M5, lane G5 2026-11-20): `build/I/runs/20261120-135316-G5-mk8d-m5`, boot into a race at 59.65 FPS, picture right; known issue: cyan kart shadow blob.** Before:  Run `build/I/runs/20261119-212504-W2-mk8d-gp4` (`library_play.py mk8d_gp`, save as before the first Grand Prix): 50cc Mushroom Cup, all 4 races and result screens, 11 min of racing, **59.1 FPS over 120-760 s (577 of 641 s at 58+)**, longest frame 1.8 s at a race load, sound 0 underruns in 782 s, private memory 4.05 -> 4.15 GB (+11 MB/min), quit cleanly. The blue tint and black road are gone, but every race picture is **posterized** (hard colour bands, darker karts: `0020-shot_race_6_min.png`; lane G, INTERFACES "Lane W2 findings"), so the picture check fails. With 8 compilers running beside it the same Grand Prix ran 48.7 FPS (`-gp3`). The Grand Prix preview movie panel is no longer black (it plays, `-gp3/0004-a_first_Mii.png`). Battle (Balloon Battle) also runs (`20261119-174606-W2-mk8d-gp`) |
| Super Smash Bros. Ultimate | NSP v0 | W | a stage match vs CPU, 10 min | **10 min of matches (W2); memory not flat.** The 141 s first frame was 3,567 shaders translated live one by one: the 1.0.0 data.arc table is now read (`formats/smash.rs files_v1`), 27,517 shaders prebuilt, first present at 3.5 s. Run `build/I/runs/20261119-235023-W2-smash-match5` (`library_play.py smash_match`, `smash_fight.txt` re-picks at the character select): 3+ back-to-back matches vs CPU on different stages, 58.2 FPS over 150-840 s (569 of 676 s at 58+, hitches when a new stage or fighter's pipelines are built), no crash, sound 15 underruns in 784 s (no output device that hour: real-time clock). **Open:** private memory 5.6 -> 6.9 GB (+108 MB/min) outside graphics' allocator (flat at ~680 MB); colour bands as MK8D (lane G) |
| Sonic X Shadow Generations | NSP v0 | W | a playable act, 10 min | **Both games in gameplay for 10+ min (W2); sound check and the owner's hand test left.** Two programs: 0 = title menu + Shadow Generations, 1 = Sonic Generations (oe::ExecuteProgram, `nxrs-build --program 1 -o sonic_p1`, the harness follows the successor). Sonic Generations: `build/I/runs/20261119-204133-W2-sonic-gens` (10 min, 31.5 FPS, picture right, memory flat 5.15 GB); its sound was stretched the whole run, fixed (sink releases on a wall clock: `-sonic-gens5` 16.6 s slowed in 303 s, 1 underrun); `spindash.txt` survives the badniks but stops at a ledge ~1:30 in (a recorded route is next). Shadow Generations: **18 min of its first stage** (`20261120-011106-W2-shadow-watch`: title, Down + A, save slot 1, the ARK stage with `boost_run.txt`), 30.0 FPS (its target), memory flat 6.9-7.1 GB, picture right since lane G3's fix, no crash; sound unchecked (no output device on the PC); the script stalls at a robot fight, so the owner's hand test is next |
| Super Mario Party Jamboree | NSP / XCI (+ update) | W | a board turn and a minigame | **Hub reached under a white layer (W2 -> lane G).** 1.1.0 update over the NSP, 139 NROs. Opening movie, title at 60 FPS. After L+R the game is in its hub (Party Plaza drawn right, its music playing), but the last pass draws a full-screen white quad that the game itself sets to (1, 1, 1, 1) (`peek` of its colour bank, `build/I/runs/20261120-013909-W2-jamboree-fade`): a layer that should be invisible, most likely a blend state graphics does not apply (INTERFACES lane G). Not a service, network, save or executor wait (ruled out). Private memory 15.2 GB (11.6 GB textures), flat |
| Anime Girls: Sun of a Beach | NSP v0 | W | gameplay, 10 min | **Playable but for the picture check (W2).** Unity: SetThreadActivity / GetThreadContext3 added (runtime, as Turbo). Every frame was black because every texture handle read 0: Unity's shaders pack two constant banks in `cbuf_slot` (0x30002) and graphics read it as one slot (fixed, `tex_slots`, lane W2). Run `build/I/runs/20261119-220414-W2-agsob-play` (`library_play.py agsob`, `shooter.txt`: walk, turn, fire, jump): 12 min on the beach level, the scripted shots kill enemies (5 -> 8 kills, level 1 -> 2: `0009`, `0014`), **59.9 FPS over 120-840 s (708 of 721 s at 58+, p99 18.8 ms)**, private memory 4.01 -> 4.10 GB (+6 MB/min), sound 10 underruns all in the first minute (loading), 0 in play, quit cleanly. Open: the picture has the same hard colour bands as MK8D (lane G); then the owner's hand test |
| Beach Bounce Remastered | NSP v0 | W | gameplay, 10 min | **11 min of the story (W2); sound to re-check with a device.** Ren'Py on MNX. The window shows the scenes since lane G3's fix. Run `build/I/runs/20261120-002042-W2-bbounce-vn` (`library_play.py vn`, A every 4 s): 11 min of dialogue scenes, 59.6 FPS (641 of 661 s at 58+), memory 1.80 -> 1.94 GB, no crash. AudioOut #1 (two 5 ms buffers refilled on release) underran ~38 a second and played slowed: the sink now releases on a wall clock (Turbo's design): 582 underruns and 5.2 s slowed in 719 s, measured with no output device on the PC; a run with a device is owed |
| Pokémon Legends: Arceus | NSP 1G+1U | P | Obsidian Fieldlands, 10 min (= M6) | In progress (lane X2): with frame generation off the village is drawn right; driven by hand through the story (Jubilife, the Wallflower, Laventon / Cyllene, night, the Survey Corps quarters) for 21.7 min with no crash, 30.0 FPS every minute, sound 1,257 s of 1,265 s, 0 underruns, memory flat (6.1-6.2 GB, +4 MB/min) (`build\I\runs\20261119-222616-X-x2-arceus2-field`). The Fieldlands come after the next story day; not reached. **Progress lost between sessions is the game's:** the 21.7 min run wrote nothing (save and its work tree unchanged since 15:49, no fs errors); this part of the story has no menu and no Save (X / Plus do nothing at the Wallflower step), so the game keeps only its autosave point. The engine's save path works (Violet's in-game Save commits, below). **Stuck point recorded for a hand play (save backed up): INTERFACES lane X2** |
| Pokémon Violet | NSP 1G+1U+1D | P | first town, 30 min (= M6) | In progress (lane X2): 20 min of in-house play at 30.0 FPS, sound, flat memory (`build\I\runs\20261119-191702-X-x2-violet1-town`); the front door still answers "You're not quite ready to go to school yet" after the bag, the desk and the stairs (`20261119-222307-X-x2-violet3-town`); the step that unlocks it (likely talking to Mom in the living room) is not found yet. In-game Save works: menu > Save commits the save (`fs: commit save0`, files rewritten; `20261120-004322-X-x2-violet5-obj`). The menu has only Options / Save at this point (no Rotom Phone, no objective text); Mom is not in the house. **Stuck point recorded for a hand play (save backed up): INTERFACES lane X2** |
| Pokémon Legends: Z-A | NUP (Switch 2 Edition) | P | Lumiose at 60 with the profile patch (= M6) | In progress (lane X2): boot to the Lumiose station platform with no crash in every run; walking the platform (`build\I\runs\20261119-224857-X-x2-plza3-gate`, 321 s) held 60.0 FPS median / p10 59.1 (the earlier 600 s run `20261119-194703-X-x2-plza2-lumiose`: 46.7 median); the ticket gate into the city was not found by hand (no map yet at that point). **Stuck point recorded for a hand play (save backed up): INTERFACES lane X2** |
| Mario Kart World | NUP | X | a race, 10 min, 60 FPS (= M7) | In progress (lane X2): first frame at 9.6-12.8 s; the render-buffer allocator (agl DynamicTextureAllocator, fixed 285 MB pool) runs out at 15.5 s. mkw-recomp's 1.8.0 game patch is ported (profile `0400C3F00006E000/turbo.toml` set `mkw_rbpool`, os `gamefix.rs`, runtime host aliases; diffed against its patches.c): with the second pool made at the allocator's init (sized for 2160 lines, 0x24b00000) the misses land in pool 1, and MKW now stops at the same 15.5 s mark on a double free of one render buffer (main+0xa14e8, scene event 0xf; `x2-mkw42-rb`), docked and handheld. Open: why that event frees twice without allocating (INTERFACES lane X2). No race yet. Traced further: phase 0 of view 0 (which allocates the buffer) is sent only while bit 4 of the view's entry word is clear (main+0xa26b0); phase 0xf frees it; here the bit stays set after the first frame and four render contexts each send 0xf (`x2-mkw50-ev`). Next: the writer of that bit |
| Donkey Kong Bananza | NUP v458752 | X | opening + mining tunnel, 10 min (= M7) | In progress (lane X2): **the crash after the save commit is fixed** (os `sched.rs`: a core is taken only from a thread inside a host function, never from one running guest code; it was two threads running guest code on one virtual core, INTERFACES lane X2) and `nv::InitCheck` no longer fails when the game gives nv no pool. The game now starts: the new-save mode choice (it has no music, which was the "silent title"), then the opening with sound. About 10 s into the opening the NVIDIA shader compiler crashes the process (`nvgpucomp64.dll`, filed for lane G: `build\I\runs\20261119-215750-X-x2-dkb6-foreign`). After lane G3's crashed-pipeline guard (4652dc2): session 1 ran 59 s then died in the driver (recorded); every later session dies at 2.8 s in nvgpucomp64.dll+0x3c1c150 on a compile the guard does not catch (`x2-dkb-g3fix2`..`5`; INTERFACES lane X2, for G3) |
| Super Mario Bros. Wonder (Switch 2 Edition) | NUP | X | a course, 10 min | Playable (lane X2): 12.7 min with no crash (`build\I\runs\20261119-184515-X-x2-wonder1-10min`: pass dump at 600 s correct, 679 s of sound, 0 underruns, memory flat after minute 5); exclusive FPS run on the new scheduler (`build\I\runs\20261120-003340-X-x2-wonder-sched2`, NXRS_EXCLUSIVE=1, 229 s, course play): median 60.0, p10 59.9, per minute 58.9 / 59.7 / 59.4 after the load, memory 4.23-4.28 GB, 1 underrun, no crash |
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
- A one-time save copy from A is offered, and happens only after confirmation. Turbo keeps saves in `<data>\save\user` (all accounts), `device`, `cache` (`svc_fs.c`); nxrs in `save\user<N>`, `device`, `cache<N>` (`fs.rs`). Done in the launcher (lane Q): Game > Copy saves, both directions, backup first (docs/LAUNCHER.md).
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
