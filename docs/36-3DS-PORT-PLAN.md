# 36 - 3DS port plan (.3ds/.cia in → native + wasm out)

**Status: plan, owner-directed 2026-11-27 (wave 3DS-PLAN).** A second console input turns a 3DS title into a
**native app** (the same `<name>.exe` + `<name>_code.dll` + `<name>_data` layout a Switch build produces today) and,
through the existing `--target web` lane, into a **browser build**. The donor is
`D:\Projects\3dsrecomp` (fearkov/3dsrecomp, MIT per its Cargo.toml): a static recompiler for 3DS titles whose front
half (rom, CRO, discovery, ARM/Thumb/VFP → C) is complete and game-proven and whose back half is deliberately
host-agnostic (docs/interface.md). nxrs builds that back half. The Switch targets stay the reference and must not
regress; no ARM64 code path changes.

```text
.3ds/.cia/.cxi ──► translator3ds (rom/cro/discover/codegen) ──► C (recomp.h v8 ABI) ──┐
os3ds / runtime3ds / gpu3ds / front ds (Rust) ────────────────────────────────────────┴─► cc.rs ──► <name>_code.dll + <name>.exe
                                                                                       (emcc lane: game.wasm + game.js)
3DS services (srv, apt, fs, hid, gsp, dsp, ldr::ro …) ──► os3ds ──► gpu3ds (PICA → Vulkan | command ring → WebGPU)
```

## 1. Executive summary

What exists on each side:

- **nxrs** (this repo): package input (folder/NSP/XCI/NUP/NRO, `translator/src/package`), an ARM64 → C translator
  (`translator/src/xlate`, `aot`, `loader`), the runtime's `Cpu`/dispatch/host-call bridge (`runtime`), the HOS
  service host (`os`), NVN on Vulkan (`graphics`), the web lane (`--target web`, docs/35, complete through W11),
  the launcher (contract v2), the harness. Every assumption in the translator and runtime is 64-bit ARM: the
  `Cpu` struct (`translator/runtime/cpu.h:7-32`) holds `x[32]` and 128-bit NEON `v[32]`; discovery seeds from
  `.eh_frame_hdr`, ELF symbols and `R_AARCH64_RELATIVE` relocations (`translator/src/loader/module.rs:170`).
- **3dsrecomp**: reads a decrypted `.3ds`/`.cia`/`.cxi` (`src/rom.rs`), parses CRO modules and the main exe's
  `static.crs` (`src/cro.rs`), finds functions in ARM and Thumb from entry, calls, exports, relocations, imports,
  pointers and a shape-judged gap scan (`src/discover.rs`), and emits C: one function per guest function, calls as
  C calls, svcs answered in place where the host allows, anything else handed to the host's interpreter
  (`src/codegen`). Its interface is `recomp.h` v8 (`abi/recomp.h`): a `Context`, read/write page tables, a `Host`
  callback struct (memory, `interpret`, `lookup`, `svc`), sorted `Entry` tables, CRO modules whose entries are
  offsets from a run-time base, and origins/stale tables so changed code falls back to the interpreter
  (docs/interface.md). It compiles with any C compiler (`src/compile.rs`: `-O2 -ffp-contract=off
  -fno-math-errno`), installs a library, and has a verify lane that checks recompiled functions against an
  interpreter (`src/verify.rs`).

**Recommended shape of the port:**

| Piece | Decision |
|---|---|
| 3DS code → C | Vendor 3dsrecomp's front half into a new crate `src/translator3ds` (rom, cro, image, discover, arm, thumb, codegen, natives), behaviour unchanged, the `recomp.h` v8 header kept verbatim (section 3) |
| Guest host | New crate `src/runtime3ds`: the run loop, dispatch over the Entry tables, page-table memory, guest threads on host threads, an ARM11/Thumb-1/VFP interpreter for the `interpret` callback and uncovered code, svc dispatch to os3ds |
| OS services | New crate `src/os3ds`: srv, apt, fs, hid, gsp, dsp, ldr::ro, cfg, ptm, y2r, soc, the 3DS SVC table (section 4) |
| GPU | New crate `src/gpu3ds`: GX command-list processing, render targets, PICA VS/GS and TEV combiners → GLSL → SPIR-V → the existing WGSL stage; a native Vulkan executor and a web ring executor after docs/35's split (section 5) |
| Window and input | `front` gains a `ds` module: two-screen layout, touch, present; the pads and keyboard come from the existing input layer (section 6) |
| Build pipeline | `src/cli` gains a 3DS branch in `pipeline.rs` and `cc.rs`; the data folder, launcher contract, web host files and `nxrs-build` surface stay as they are (section 7) |

**Reused unchanged:** the whole CLI compile/link/object-cache machinery (`cc.rs`), the data-folder staging
(`package::extract`, `romfs.source`, markers), the web host (`front/web` page, workers; `webhost`/`webpack`/
`webzip`), the WGSL shader stage and packs (`shaders`), the launcher (one new source check + an SMDH decoder),
`nxconfig`, the harness. **Rebuilt for the 3DS:** the translator, the guest CPU host, the OS services, the GPU
front, the two-screen presentation. **Not reusable** for the 3DS guest: `translator`'s xlate/aot/loader (ARM64),
`runtime`'s `Cpu`/kernel/mem/fiber (64-bit state, 2^39 identity memory), `os`'s HOS IPC and services, `graphics`'s
NVN record/replay. They keep serving the Switch targets untouched.

The port's one structural idea: **keep 3dsrecomp's ABI exactly** (Context, page tables, Host callbacks, Entry
tables, origins). Everything the nxrs back half does is a host implementing that interface — which is what
docs/interface.md designed it for, and what its reference host (`abi/` in Rust, `src/verify.rs`) demonstrates.

## 2. Input formats (.3ds/.cia)

**What nxrs's package input reads today.** `translator/src/package/mod.rs::detect` (mod.rs:134-158) opens a
folder, a `PFS0` NSP (plain, encrypted, combined 1G+1U(+1D)), a card image (XCI: `HEAD` at 0x100 or 0x1100), an
indexed NUP, or an NRO (magic at +0x10). `open_game` returns `Game { exefs: BTreeMap<String, Vec<u8>>, romfs:
Arc<RomFs>, control, addons, applied, marker }`; the translator's `generate` takes the ExeFS map of NSO files
(`aot/gen.rs:127`), lays them out at `FIRST_BASE 0x7000000000` (`loader/layout.rs:7`; the web lane re-bases at
`WEB_FIRST_BASE 0x1_D000_0000`), and discovery seeds from `.eh_frame_hdr`, symbols and relocations
(`loader/module.rs:170-194`). Encrypted NCA content needs prod.keys (`package/keys.rs`; NXRS_PROD_KEYS, the
launcher's `keys\prod.keys`); the crypto primitives are AES ECB/CTR/XTS (`package/crypto.rs`). A build stages the
data folder with `Game::write_data` (mod.rs:352): `exefs\` unpacked (ZBIC through `loader::nso::for_runtime`), the
RomFS extracted (or `romfs.bin`), `control.nacp` + icon, the `romfs.source` record.

**What a .3ds / .cia / .cxi is.** A cart image (`.3ds`) is an NCSD at 0x100 with a partition table at +0x120
(media units of 0x200); partition 0 is the game's NCCH. A `.cxi` is that NCCH alone. A `.cia` is a 0x2020-byte
header, then the certificate chain, the ticket, the title metadata (TMD, content records in big-endian), and the
content files, each starting on 64 bytes; the executable is content index 0. The NCCH header says where the ExeFS
(+0x1A0) and RomFS (+0x1B0) sit, gives the program id (+0x118) and the exheader (+0x200), whose code-set infos
name the text/rodata/data addresses and sizes and whose flag 0x0D bit 0 says `.code` is LZSS-compressed. All of
this is exactly what 3dsrecomp reads: `rom.rs::Title::load` (rom.rs:268-324) detects NCSD → first partition,
NCCH at 0, or a CIA header size at 0 and walks `cia_executable` (rom.rs:219-255) to content 0; `decompress`
(rom.rs:385) undoes the backwards LZSS; `read_romfs` (rom.rs:357) reads the IVFC level-3 tables and leaves file
data in the dump. Its `Title` keeps the file open and reads on demand — gigabyte dumps are not loaded whole.

**Decryption: what 3dsrecomp expects, plainly.** It expects a **decrypted dump**. An NCCH without the NoCrypto
flag (flags[7] & 0x04) whose crypto method byte (flags[3]) is non-zero is refused; a method-0 image whose ExeFS
table still reads as garbage (no `.code` name) is refused with "the dump is encrypted, it has to be decrypted
first"; a `.cia` whose content record says encrypted (flag bit 0) is refused the same way (rom.rs:289-297,
314-320, 247-249). 3DS encryption (NCCH key slots, ticket title keys for CIAs) is a different key set from the
Switch's prod.keys and is **not** implemented in the first waves; nxrs says the same refusal line until the owner
decides a decryption lane (section 10, D2). Decrypted dumps are the common case from GodMode9 and no-intro-style
sources.

**The precise changes.** The Switch package reader stays untouched; the 3DS formats get their own reader in the
new crate, and `nxrs-build` picks between them by magic:

- `translator3ds/src/rom.rs`: 3dsrecomp's rom.rs, reworked to read through the same seek/read abstraction the
  Switch package uses (`package::io::Blob`) so one open path serves both, with `Game3ds { program_id, exheader,
  code programs, romfs, smdh }`. `.3ds`, `.cia` and `.cxi` (and a bare `.app`/NCCH content file) detected by
  magic; encrypted inputs refused with 3dsrecomp's exact line.
- `translator3ds/src/cro.rs`, `image.rs`: the CRO/CRS parse and the exe segment split, as upstream.
- `translator3ds/src/title.rs`: the nxrs-side glue — `programs()` (exe first, then every `*.cro` in the RomFS,
  each with the seeds the relocations and anonymous imports give, exactly `lib.rs::programs` upstream), and a
  `RomFs` adapter so the RomFS reads through the **existing** `translator::package::romfs::RomFs` trait shape:
  `list/walk/find/read/image`. With that adapter, `extract::tree`, the `romfs.source` record, the DLC layout
  conventions and `webpack.rs`'s data manifest all keep working for 3DS builds without changes.
- Control data: the ExeFS `.icon` entry holds an SMDH (icons + titles). `write_data` gains a 3DS branch writing
  `control.smdh` instead of `control.nacp`; the launcher decodes SMDH for the card (section 7). `Game3ds::marker`
  names a new marker constant `romfs.ds` so `romfs.source`'s currency record names the `.3ds`/`.cia` package and
  any applied update (a v0 update CIA replaces content 0; version from the TMD's title version).
- `SourceKind` and `OpenOptions` stay Switch-shaped; the 3DS branch carries its own enum in translator3ds
  (`DsKind::{Cart, Cia, Cxi, App}`) and never touches `package/mod.rs`'s `detect`. The detection switch lives in
  `pipeline.rs` (one magic check before `package::open_game`) so the launcher's contract arguments do not change.

## 3. The translator question

The 3DS CPU is an ARM11 (ARMv6K): 32-bit ARM with conditional execution on almost every instruction, Thumb-1,
VFP2 (32 single registers `s0-s31`, no NEON), and no hardware divide (the C library's `__aeabi_*divmod` loops are
hot — 3dsrecomp overrides them on the host, `natives.rs`). This is a different ISA generation from the ARM64 the
nxrs lifter translates, and the lifter is ARM64 through and through: bit-field decoders for A64 encodings
(`xlate/lifter.rs`, `simd.rs`), function-local NZCV as one 64-bit word, the `Cpu` layout above whose offsets are
statically asserted (`cpu.h:34-39`), jump-table shapes specific to ADRP/ADD addressing (`jumptables.rs`). Its
output is kept byte-identical to mkw-recomp's (`xlate/mod.rs:4`). Three options:

**(a) Extend nxrs xlate with an ARM11 backend.** A second ISA inside the A64 lifter means new decoders for the
ARM condition-field-everywhere model and the Thumb-1 variable-width stream, a flags model for the four plus GE
bits, VFP lowering, new jump-table shapes (`add pc, pc, rm lsl 2` and `ldr pc, [pc, rm lsl 2]` tables), and a
`Cpu` ABI that cannot be the Switch one. This is writing a second translator that shares only scaffolding, in the
crate whose guarantee is "byte-identical to the ARM64 reference". Cost: months; risk to the Switch lane: real.
**Not recommended.**

**(b) Vendor 3dsrecomp's front half into the translator crate family — recommended.** The code is ~6.4k lines of
Rust (rom 612, cro 285, image 125, discover 917, arm 213 + thumb 124 decode, codegen 3,650, natives 257,
overrides 199), MIT (`Cargo.toml license = "MIT"`), self-contained, and proven on four titles at ~99.8% of
executed instructions coming from the library (README). It becomes `src/translator3ds`, headers stripped, credited
in the README per the repo's no-attribution-headers rule. Its generated C stays written against the verbatim
`recomp.h` v8 — we do **not** fork the ABI. What that means for the nxrs pipeline:

- The generated code's world is the `Context` (r[16], flags, thumb, ge, exclusive monitor, budget, tls) plus two
  page tables of 2^20 pointers each (`read_pages`/`write_pages`, 4 KiB pages of the 32-bit space; a null entry
  sends the access to the host's read/write callbacks) — docs/interface.md §Memory. Memory is *not* identity-mapped
  host memory as in the Switch lane; that is fine and, on the web, better (a 3DS title needs ~0.5 GiB of guest
  space, not a 16 GiB linear window; section 5 of docs/35's constraints do not apply here).
- Guest calls are C calls; an unknown target goes through `lookup`; an svc goes through `svc` (answered in place
  or handed back via `EXIT_SVC`); anything the codegen left out goes through `interpret` with the address and
  opcode (docs/interface.md §Host callbacks). The generated C is portable C99 plus GCC builtins
  (`__builtin_bswap32`, `__builtin_clz`, `__builtin_expect` — verified in `codegen/arm.rs:607`,
  `codegen/thumb.rs:293`, `abi/recomp.h:165`); `recomp.h`'s stack probe already has a portable
  `__builtin_frame_address(0)` arm (`recomp.h:636-656`), so **emcc compiles it as is** — the same class of result
  docs/35 W0 proved for the Switch C.
- Entry tables (`entries.c`): one `Entry {address, code}` per place execution can return to (basic blocks,
  after-call, after-svc), sorted for binary search; CRO modules carry entries as offsets from a run-time `base`
  the host writes when the title loads the module (docs/interface.md §Modules) — this is the 3DS counterpart of
  nxrs's fixed-base NRO mapping, and it is what `os3ds`'s `ldr::ro` service drives.
- Origins/stale: every function's code as hashed pieces; a changed byte (mod, update, another revision) marks the
  function stale and the host keeps it out of lookups so it runs interpreted instead (docs/interface.md §Origins).
  This replaces nothing in nxrs — it is strictly better than refusing to boot, and it is how a v0-update CIA is
  handled per function.

What `cc.rs` links against, compared. A Switch build links `<name>_code.dll` with `nxfront.lib` and exports
`nxrs_game_main(argc, argv, const RtModule*, int)` (`cc.rs:504-522`, `STUB_C` at cc.rs:1033 resolves that
symbol); `RtModule` carries ARM64 `GuestFn` tables built by `pipeline::modules_c` over `layout.json`. A 3DS build
does the same dance with 3DS parts: it links `nxfront3ds.lib` (os3ds + runtime3ds + front ds + gpu3ds) and
`pipeline::modules_c` gains a 3DS form that exports the same `nxrs_game_main` shape whose module table is the
Entry tables and module bases from `entries.c` (the linked-in variant of docs/port.md's program, which does
exactly this with Zakuro). The loader `.exe` (`cc.rs::STUB_C`) therefore does not change at all: it resolves
`nxrs_game_main` and calls it.

**(c) Keep 3dsrecomp as a separate front-end that emits a library the pipeline consumes.** `nxrs-build build
game.3ds` would shell the 3dsrecomp binary, then compile/link around its output. Rejected: it splits the build
into two programs' outputs (their chunk cache, progress lines and `@progress` machine lines would not merge), it
loses the generated-C-on-disk reuse contract (`gen_key`, pipeline.rs:146), and the library-installed-elsewhere
model (build.rs::install, `%APPDATA%\3dsrecomp`) contradicts nxrs's "a built game carries its data in one folder"
rule (LAUNCHER_CONTRACT.md). Vendoring (b) keeps one program, one cache, one folder. If upstream 3dsrecomp moves,
its diff ports hunk by hunk into `translator3ds`, as nxnative's did for nxrs (PORT_PLAN §1.1).

Extras carried over from the donor because they already solve nxrs problems: `natives.rs` (the armcc division
overrides, found by byte signature, emitted as `RECOMP_OVERRIDE` C), `overrides.rs` (handwritten function
replacements — the 3DS form of profile patches for a first pass), `build.rs`'s hints (the interpreter's miss list
fed back as `Source::Hint` seeds, per title and code hash — the donor's `hints.txt` mechanism maps onto nxrs's
`discovered.tsv` loop, `aot/hints.rs`, with a 3DS file format: one address per line, bit 0 set for Thumb) and
`verify.rs`'s differential idea (a 3DS oracle wave, section 8, W8).

## 4. Host runtime / OS services

What a 3DS title calls at run time (the well-known service surface; exact SVC numbers beyond the ones cited are
pinned against 3dbrew in 3DS-W2 — UNVERIFIED here, the three below come from docs/interface.md):

- **SVCs**: `svc 0x28` GetSystemTick (the hottest by far — donor runs it in place, docs/interface.md §Ticks),
  `svc 0x25` WaitSynchronizationN and `svc 0x14` ReleaseMutex (run in place on a lent mutex, §A mutex lent);
  plus thread/process/memory/sync management (ControlMemory, QueryMemory, CreateThread, SleepThread, events,
  mutexes, memory blocks, the clock). The nxrs runtime's svc mechanism (`runtime/src/svc.rs::set_handler`/
  `set_fallback`, `nxrt_svc_impl`) is the ARM64 path; `runtime3ds` gets its own svc dispatch off the `Host::svc`
  callback with the same shape (handler registry + gap fallback that answers with a logged refusal, mirroring
  `os`'s `UNIMPL` line discipline).
- **Services** (named handles through `srv:`): srv (service locator — nxrs's `sm` counterpart), apt (applet
  lifecycle: menu, home menu, sleep, screen swap, the applet state machine every game drives), fs (SDMC + RomFS +
  archives), hid (buttons, touch, circle pad, accelerometer), gsp::Gpu and gsp::Lcd (command lists, framebuffer
  swaps, LCD state incl. the 3D slider), dsp (audio: firmware load + the DSP's own programs), ldr::ro (CRO
  load/unload/link), cfg (region, language, model, blocks), ptm (battery, play time), y2r (YUV→RGB, some games),
  soc (sockets), mic/cam (inputs), plus small ones (boss, news, ndm, ir) most games only open.
- **CRO loading**: `ldr::ro` maps a module wherever the loader decides, applies its relocations, runs its crs
  init; `runtime3ds` then writes the load address into the library's module `base` variable (docs/interface.md
  §Modules) so generated code works wherever the game loaded it. This is the 3DS form of nxrs's fixed-base NRO
  translation; nothing else changes at run time.

What `src/os` + `src/runtime` provide for HOS games and what transfers: the *shapes* transfer, the code does not.
`os`'s service registry is keyed by ARM64 mangled names over CMIF IPC (`os/src/hle.rs`); 3DS services are plain
command dispatch per handle, simpler than CMIF. The pieces worth porting as designs: the boot order discipline
(`os/src/boot.rs` runs the SDK's init order), the gap ledger (`os/src/watch.rs`'s `gaps` summary — os3ds gets one
from day one), the virtual-core scheduler hooks (`os/src/sched.rs` over `nxrt::kernel::Scheduler`), the media
sink (`os/src/media/sink.rs` — one WASAPI stream; os3ds audio renders into it), the clock (`os/src/clock.rs`
`set_rate`), the input layer (`os/src/input/*`: pads, keyboard, layout, rumble — os3ds's hid reads the same
`set_keyboard`/pad state), and the web service arms (`os/src/web`, `webdata.rs` — os3ds's wasm side follows the
same pattern when 3DS-W7 lands).

The mapping (service → nxrs place → native / wasm feasibility):

| 3DS service | nxrs place | Native | Wasm |
|---|---|---|---|
| SVC table (kernel) | `runtime3ds/src/svc.rs` + `os3ds/src/kernel.rs` (threads, sync, memory, clock) | yes (host threads, one guest thread each like nxrt) | yes (Emscripten pthreads, docs/35 W2 pattern) |
| GetSystemTick / lent mutex | `runtime3ds/src/hostfast.rs` (the tick/mutex fields of Context, interface.md §Ticks/§A mutex lent) | yes | yes (budget = cycles; same rules) |
| srv | `os3ds/src/services/srv.rs` | yes | yes |
| apt (applets) | `os3ds/src/services/apt.rs`; dialogs on the host like nxrs's swkbd/error notices | yes | yes (page-level dialogs; front/web menu pattern) |
| fs | `os3ds/src/services/fs.rs` over `translator::package::romfs` (the build's extracted `romfs/`) + `<data>\save` | yes | yes (I/O worker + OPFS saves, docs/35 W3 pattern) |
| hid | `os3ds/src/services/hid.rs` reading the existing `os` input layer (pads, keyboard) + touch from `front::ds` | yes | yes (page input block, W3) |
| gsp::Gpu / gsp::Lcd | `os3ds/src/services/gsp.rs` → `gpu3ds` (section 5) | yes (Vulkan) | yes (ring → GPU worker) |
| dsp (audio) | `os3ds/src/services/dsp.rs` over an ARM7/DSP program interpreter into `os/src/media/sink` | yes (largest single unknown, D5) | same code (interpreter is portable) |
| ldr::ro | `os3ds/src/services/ldr_ro.rs` writing module bases into the library | yes | yes |
| cfg | `os3ds/src/services/cfg.rs` (region/language from `nxconfig`, New3DS model setting) | yes | yes |
| ptm (battery, play time) | `os3ds/src/services/ptm.rs` (host battery via the front end) | yes | yes (Battery API) |
| y2r | `os3ds/src/services/y2r.rs` (CPU or a small compute pass) | yes | yes |
| soc (sockets) | `os3ds/src/services/soc.rs` (native sockets; wasm: refused per title until the host proxies, like nxrs's web net) | yes | refuse per title first |
| mic / cam | refuse with a gap line (no owner game needs them yet) | defer | defer |
| boss / news / ndm / ir | answer the handshake, refuse the rest with gap lines | defer | defer |

Work per crate: `runtime3ds` owns the run loop (budget, exits, stack limit, the trail), the dispatch (binary
search over Entry tables, stale filtering), memory (a region table + page-table arrays over a 4 GiB-scale
reservation; lazy commit like `runtime/src/mem.rs`'s, but committed pages are handed to the tables as pointers and
GPU-visible pages stay null in `write_pages` so the host callbacks see writes — the explicit-write-watch trick the
ABI designs for), threads (one host thread per guest thread; the scheduler interface mirrors
`nxrt::kernel::Scheduler`), and the interpreter (`runtime3ds/src/interp`: ARM11 + Thumb-1 + VFP, single
instruction for `interpret`, block mode for uncovered code; new code — nxrs's ARM64 interpreter
`runtime/src/interp` does not transfer). `os3ds` owns the services above over those hooks, the boot sequence, and
the gap ledger.

## 5. Graphics

**PICA200 vs NVN.** NVN is a modern explicit API (pools, descriptor buffers, queues — `graphics` records its 523
entry points and replays on Vulkan). The 3DS GPU is driven by **command lists**: the CPU fills a memory buffer
with register writes and calls the GSP to process them (`gsp::Gpu` SubmitCommandList; SetBufferSwap presents a
framebuffer triple). The PICA pipeline: programmable vertex and geometry shader units (binary DVLE programs in
`shaders`/DVLB containers), a fixed-function fragment stage (texture-environment combiners — TEV — plus
depth/stencil/blend), texture units with 3DS formats (RGBA4/565/5551/8/8x3, ETC1, …) and tiled/swizzled layouts,
render-to-texture, memory fills, display transfers. (Hardware facts here are the well-established PICA surface —
register map, combiner model, DVLE layout — to be pinned against 3dbrew in 3DS-W4; UNVERIFIED in this doc.)

**What a 3DS GPU path needs, and where it goes.** `src/gpu3ds` implements both halves the way `graphics` splits
them (docs/35 W4: "draw decisions vs Vulkan calls"):

- *Record half* (CPU): parse command lists per submit into draws, state, uploads — the replay-shaped output nxrs
  already knows how to consume and the web ring already carries. Framebuffer swaps become presents; memory fills
  and display transfers become copies.
- *Shaders*: PICA VS/GS programs decode to GLSL, compiled by the glslang lane `shaders` already uses for its own
  compute shaders, to SPIR-V; TEV combiner graphs generate a fragment shader per configuration. On the web the
  existing SPIR-V → WGSL stage (`shaders/src/wgsl.rs`, with the W0(e)/W4 rewrites docs/35 established) converts
  them exactly as it converts Maxwell shaders today; shader packs (`shaders/src/pack.rs`) work unchanged.
- *Geometry shaders*: real on PICA, absent from WebGPU. docs/35 §4.4 already fixed the rule: "WebGPU gaps
  (geometry/tessellation/mesh shaders, transform feedback, sparse images) get explicit lowerings or a per-title
  refusal … never silently skipped draws". The PICA GS is mostly pass-through/expand work, so the native path uses
  Vulkan geometry shaders where present and the web path gets the standard lower-to-instanced-vertex trick or a
  per-title refusal.
- *Targets and textures*: render targets are guest memory regions (color + depth, with copies between), so the
  pool/write-back problem is smaller than NVN's but the same shape; textures decode/deswizzle CPU-side where a
  Vulkan format match does not exist (ETC1: a decode pass like the existing `astc_decode.comp`; the web has no
  ETC1 either, so decoded RGBA is the web form, consistent with docs/35 W10b's stand-in approach).
- *Native executor*: Vulkan following `graphics/src/vk`'s patterns (device, present, sync) but a much smaller
  executor — no descriptor buffers, no sparse, no mesh shaders.
- *Web executor*: the same command-ring → GPU worker design docs/35 W4-W10c built; `gpu3ds` writes the ring, the
  page's worker executes WebGPU. The page protocol (`WEB_HOST_PROTOCOL`, `layout.rs:19`) gains the 3DS present
  shapes.

**Present**: the GSP hands the LCD three framebuffers per screen; the front end presents both screens (section 6).
Format conversion and the top/bottom split happen in the present path as `graphics`'s present does today.

## 6. Dual screen + input + presentation

`src/front` owns the window (`front/src/window.rs`: Win32, fullscreen toggle, `poll_input`, `hwnd()` handed to
graphics as the present surface; Linux has `window_linux.rs`). For 3DS titles it gains a `ds` module:

- `front/src/ds/layout.rs`: screen arrangement as player settings in the game's ini (stacked default, side by
  side, single screen with a swap key, per-screen zoom); the two present viewports computed from the window size
  the way the single present letterboxes today. `nxgfx`'s window is set the same way (`nxgfx::set_window`), with
  the present surface reporting two rects.
- `front/src/ds/touch.rs`: pointer input mapped through the bottom screen's viewport into 3DS touch coordinates,
  fed to `os3ds`'s hid service alongside the pads; a touch on the top screen (where a layout shows it) maps as a
  second touch surface or is configurable. Mouse capture stays as it is.
- Stereo (the 3D slider): the slider is LCD state the game reads (`gsp::Lcd`) and only titles that enable
  stereoscopic rendering produce two eye views. **Recommendation: report slider 0 and present one view per
  screen; stereoscopic output is a later, opt-in wave.** It is in the owner-question list (D6) because it touches
  the GPU executor's target count.
- Input mapping: 3DS is one circle pad, d-pad, A/B/X/Y/L/R/Start/Select, touch. The existing pad/keyboard layer
  maps directly (face buttons already follow `button_layout`, DECISIONS §5); the circle pad is stick 0; a second
  stick may map to d-pad or touch-drag as a per-game setting later.

Web (`src/front/web`): the page presents one canvas today (docs/35 W2: "canvas fills the window letterboxed").
`ds` layout extends the page: the same canvas carries both viewports (the GPU worker's present already blits by
rect), or two canvases if the layout setting wants independent scaling — one canvas with rects first. `input.js`
gains pointer/touch forwarding into the input block's touch fields; the menu (`menu.js`) stays over both screens;
saves/audio/io workers are untouched.

## 7. Pipeline

`nxrs-build build game.3ds` and `... --target web` — the exact changes:

- **`src/cli/src/main.rs`**: doc-comment and help text name the new inputs (`.3ds .cia .cxi`); `doctor` gains a
  line for the 3DS toolchain facts (nothing extra beyond clang/emcc — no keys required for decrypted dumps);
  contract stays 2 (the launcher's arguments do not change; new inputs ride the existing `build <source>`).
- **`src/cli/src/pipeline.rs`**: `build()` detects the container family first (magic: NCSD/NCCH/CIA → 3DS).
  The 3DS branch: open via `translator3ds::title::open` → `Game3ds`; module lines print the exe/CRO programs
  instead of NSO modules; **no** Switch profile lookup, NRO collection, fpsfind, cheats or mods machinery (those
  stay ARM64-shaped; the 3DS equivalents — patches via overrides, hints via the interpreter loop — come from the
  donor's mechanisms); translate via `translator3ds::gen::generate` (same progress lines: `Analyzing the code...`,
  `C generated: n/m chunks`, `C code: … functions` so the launcher's parsers keep working); data folder via the
  same `write_data` flow (RomFs adapter) writing `control.smdh`; the gen key hashes the programs' bytes plus
  options as today; `units()` compiles the generated C with **3DS flags** (below); link with the 3DS engine
  library and the 3DS `modules_c`; the web lane then runs unchanged (`webpack`, `webshaders`, `webzip`,
  `webhost` — `build.json` gains nothing required; its `titleId` field carries the 3DS program id).
- **`src/cli/src/cc.rs`**: `find_engine3ds` (`NXRS_ENGINE3DS_LIB`, else `nxfront3ds.lib` beside nxrs-build,
  `gnu\libnxfront3ds.a` for llvm-mingw — mirroring `find_engine` cc.rs:526); a 3DS C flag set: the generated code
  is donor C, so its measured flags are the donor's (`-O2 -ffp-contract=off -fno-math-errno -w`,
  compile.rs:10) instead of the ARM64 set (`-O1 -march=x86-64-v3 -fno-vectorize -fno-slp-vectorize`, cc.rs:53);
  the emcc lane unchanged (the same flags ride `WEB_GEN_OPT` for 3DS units). Object cache, crash-retry, chunk
  reuse: unchanged. `STUB_C`/`STUB_C_LINUX`: unchanged (the entry symbol is still `nxrs_game_main`).
- **`src/cli/src/profile.rs`**: 3DS profiles are a new small table (`profiles/games3ds/<16-hex program
  id>/ds.toml`: tick rate, fps target, per-title overrides of the donor's `--overrides` file, memory sizes);
  `profile::find` stays Switch-only and a `find3ds` picks by program id; the `Profile:` line says "3DS" so logs
  stay legible.
- **`src/cli/src/stubgen.rs`, `cheats.rs`, `mods.rs`, `fpsfind.rs`, `shaderimport.rs`, `shaderxlate.rs`**: not
  called on the 3DS branch (guards + one log line each if invoked with a 3DS input).
- **Web lane specifics** (`webhost.rs`/`webpack.rs`/`websplit.rs`/`webzip.rs`): webpack's RomFS manifest works
  over the adapter as is; webzip gzip copies as is; `websplit`'s hot/cold split needs a 3DS hot-set recorder
  (a `NXRS_FNPROF`-style counter in `runtime3ds` writing `<data>\hotset.txt`) — later wave (3DS-W7), one module is
  the default as in WEB-W6b.
- **Runtime libs**: `runtime3ds`/`os3ds`/`gpu3ds` build as staticlibs like `runtime`/`os`/`graphics` do; `front`
  links them into `nxfront3ds.lib` the same release assembly `scripts/build` uses for `nxfront.lib`.
- **Profiles crate** (`src/profiles`): loads `games3ds` beside `games`; `source()`/`check()` count both.
- **Contract v2 / launcher**: `docs/LAUNCHER_CONTRACT.md` gains the 3DS input line and the `control.smdh` file
  (an engine-side doc change, listed in section 9). `src/launcher`:
  `services/sources.rs::valid_source` (sources.rs:18) accepts `.3ds` (NCSD at 0x100), `.cia` (0x2020 at 0),
  `.cxi`/`.app` (NCCH at 0x100 off the header or bare); `services/artwork_decode.rs` decodes SMDH icons
  (48×48 RGB565 + titles) beside its nacp/icon path; `constants/words_library.rs` gains the new source words; the
  Add game flow, Play, DLC sync (a 3DS title's DLC is update-style content CIAs — out of scope for the first
  waves; the launcher's DLC window shows the empty state), library layout and contract plumbing are untouched.

## 8. Wave plan

Each wave: independently verifiable, sized like the repo's (docs/31 §8: split when one part verifies alone), wave
doc written before code (`docs/waves/3DS-W<n>.md`, §3.4 of the protocol), tests in the changed crates run with
`cargo test --release -p <crate>`, evidence under `build/DS/`. The lead opens the waves from this plan and adds
the ledger rows to `docs/32-REMAINING-WORK-PLAN.md` (the ledger is lead-owned; agents do not edit it). Order and
exit criteria:

| Wave | Outcome | Exit criterion (measured, in the evidence folder) |
|---|---|---|
| **3DS-W0 Feasibility** | The donor's front half vendors clean and compiles | `translator3ds` builds; a synthetic `.cxi` fixture (the donor's own test shapes) translates to C; the C compiles with the bundled clang *and* emcc to a static lib + wasm object; `cargo test -p translator3ds` green; no Switch crate touched |
| **3DS-W1 Input** | `.3ds`/`.cia` accepted end to end (translate-only) | `nxrs-build build <game>.3ds --no-build` stages `exefs`-equivalent programs + `romfs/` + `control.smdh` + `romfs.source`; a real decrypted title's RomFS listing equals the donor's (`rom.rs` cross-check); launcher imports `.3ds`/`.cia` with SMDH card art (launcher `cargo test`) |
| **3DS-W2 Guest host** | Code runs: boot to the title's entry under a headless harness | `runtime3ds` runs the Entry tables (context loop, budget, exits, memory page tables, threads); an ARM11/Thumb interpreter covers uncovered code; svc dispatch answers the kernel table; a title's entry and first threads run headless with a svc/gap log; interpreter differential vs the donor's verify lane (or its own golden set) 0 mismatches on the tested classes |
| **3DS-W3 Services** | A title reaches its main menu with input | srv/apt/fs/hid/cfg/ptm/ldr::ro implemented per section 4's table; the title boots its menu, input moves it (harness scenario + sheets); `gaps` summary reported |
| **3DS-W4 Graphics (native)** | First pixels on Vulkan | gpu3ds processes command lists, VS/GS + TEV → SPIR-V, render targets + present of both screens in the `front::ds` layout; the title's menu drawn correctly (sheets read) |
| **3DS-W5 Audio + the rest** | Title into first gameplay, native | dsp audio audible through the media sink (underruns counted), y2r/soc per table; first controllable gameplay per the repo's bar (first controllable gameplay, short run) |
| **3DS-W6 Pipeline polish + profiles** | A second title builds and plays native | `profile.rs` 3DS table, per-title overrides, doctor line; second title to gameplay; contract unchanged (launcher untouched except W1's import) |
| **3DS-W7 Web** | The title plays in Chrome | os3ds/runtime3ds/gpu3ds `cfg` wasm arms; page two-screen layout + touch; PICA shaders through WGSL; command ring → GPU worker; the title's menu and gameplay in Chrome with data streaming (docs/35 W3 patterns); websplit hot-set optional, off by default |
| **3DS-W8 Validation + verify lane** | The 3DS library gets its own columns | `docs/34-LIBRARY-VALIDATION.md`-style results for the owner's 3DS titles; the donor's verify idea (recompiled vs interpreter differential) runs as a build-time check on translated functions; results and failures named |

Dependencies: W0 → W1 → W2 → W3 → W4/W5 (W4 can start at W3's menu evidence) → W6 → W7 → W8. W1 and W2 are the
natural split point if a wave must split (docs/31 §8).

## 9. The complete file list

New crates first, then updates. (Donor files keep their upstream names inside the new crates; headers stripped,
credits to the README.)

**New crate `src/translator3ds`** (workspace member; the 3DS front half):

| File | What |
|---|---|
| `src/translator3ds/Cargo.toml` | the crate (MIT dependency note in the README credits, no source headers) |
| `src/translator3ds/src/lib.rs` | crate root: `programs()`, `Mods` (code/exheader/romfs replacements), re-exports |
| `src/translator3ds/src/rom.rs` | NCSD/NCCH/ExeFS/RomFS/.cia reading + LZSS decompress (donor rom.rs over `package::io::Blob` reads; encrypted refusals verbatim) |
| `src/translator3ds/src/cro.rs` | CRO0/CRS parse, exports/imports/anonymous imports, relocation slots (donor cro.rs) |
| `src/translator3ds/src/image.rs` | exe segments from the exheader's code-set infos (donor image.rs) |
| `src/translator3ds/src/discover.rs` | function discovery: seeds, gap scan, huge-function split, tables (donor discover.rs) |
| `src/translator3ds/src/arm.rs` | ARM flow-only decode (donor arm.rs) |
| `src/translator3ds/src/thumb.rs` | Thumb-1 flow decode (donor thumb.rs) |
| `src/translator3ds/src/codegen/mod.rs` | C emission: functions, containers, entries/origins tables, file chunking (donor codegen/mod.rs) |
| `src/translator3ds/src/codegen/arm.rs` | ARM lowering (donor; svc-in-place, cp15 TLS, barriers) |
| `src/translator3ds/src/codegen/thumb.rs` | Thumb lowering (donor) |
| `src/translator3ds/src/codegen/vfp.rs` | VFP lowering (donor) |
| `src/translator3ds/src/codegen/hoist.rs` | shared page-table looks for reg+const accesses (donor) |
| `src/translator3ds/src/codegen/locals.rs` | register/flag local analysis (donor) |
| `src/translator3ds/src/abi.rs` | `recomp.h` v8 verbatim as a `include_str!` constant (donor abi/recomp.h) |
| `src/translator3ds/src/natives.rs` | the aeabi division overrides (donor natives.rs) |
| `src/translator3ds/src/overrides.rs` | handwritten overrides loading (donor overrides.rs) |
| `src/translator3ds/src/hints.rs` | `.hints` read/write, bundled-hints shape, code hash (donor build.rs hints half; the runtime's interpreter loop feeds the write side) |
| `src/translator3ds/src/gen.rs` | the nxrs-side `generate()`: `GenOptions3ds`, progress lines, files written under `<build>\gen`, gen-key inputs |
| `src/translator3ds/src/title.rs` | `Game3ds` + `open()`: kind detection, `DsKind`, SMDH control, the `RomFs` adapter for the data-folder staging, `romfs.ds` marker text |
| `src/translator3ds/tests/synthetic.rs` | fixture builder (donor rom.rs/cro.rs test shapes) + end-to-end translate tests |
| `src/translator3ds/tests/real_titles.rs` | opt-in tests over a real decrypted title (paths via env, like `translator/tests/real_packages.rs`) |

**New crate `src/runtime3ds`** (the guest host):

| File | What |
|---|---|
| `src/runtime3ds/Cargo.toml` | the crate |
| `src/runtime3ds/src/lib.rs` | crate root, `start()`/`Process3ds` |
| `src/runtime3ds/src/context.rs` | `Context` mirrors of `recomp.h` (Rust side), budget/exits/trail |
| `src/runtime3ds/src/dispatch.rs` | Entry-table binary search, module bases, stale filtering, `lookup` |
| `src/runtime3ds/src/run.rs` | the run loop: budget, stack limit, exits, the trail accounting |
| `src/runtime3ds/src/mem.rs` | regions + `read_pages`/`write_pages` page tables, lazy commit, GPU-visible null-write pages, the write callbacks |
| `src/runtime3ds/src/thread.rs` | host thread per guest thread, scheduler hooks (nxrt's `Scheduler` shape), priorities |
| `src/runtime3ds/src/svc.rs` | svc dispatch registry + fallback (gap lines) to os3ds handlers |
| `src/runtime3ds/src/hostfast.rs` | GetSystemTick in place, the lent mutex, per interface.md §Ticks/§A mutex lent |
| `src/runtime3ds/src/interp/mod.rs` | ARM11/Thumb-1/VFP interpreter: single-instruction `interpret` + block mode |
| `src/runtime3ds/src/interp/{arm.rs,thumb.rs,vfp.rs}` | the interpreter's three decoders |
| `src/runtime3ds/src/prof.rs` | the hot-set/miss recorder (`<data>\hotset.txt`, `.hints` writer) |
| `src/runtime3ds/tests/*.rs` | context/dispatch/mem unit tests; interpreter golden cases |

**New crate `src/os3ds`** (services; lib `nxos3ds`):

| File | What |
|---|---|
| `src/os3ds/Cargo.toml` | the crate (depends on runtime3ds, translator3ds, os's input/media halves by feature or extraction — see note) |
| `src/os3ds/src/lib.rs` | crate root, `nxos3ds_main` (the 3DS `nxos_main` shape), boot |
| `src/os3ds/src/boot.rs` | the 3DS boot order (srv/apt/gfx init) and process start |
| `src/os3ds/src/kernel.rs` | the kernel SVCs: threads, sync objects, memory blocks, clock (over runtime3ds hooks) |
| `src/os3ds/src/hle.rs` | the service registry + gap ledger (`gaps`) |
| `src/os3ds/src/services/srv.rs` | service locator |
| `src/os3ds/src/services/apt.rs` | applet lifecycle (menu, home, sleep, screen swap) |
| `src/os3ds/src/services/fs.rs` | FSUSER/FSREG over the build's `romfs/` + `<data>\save` + SDMC |
| `src/os3ds/src/services/hid.rs` | buttons/touch/circle pad over the existing input layer |
| `src/os3ds/src/services/gsp.rs` | gsp::Gpu + gsp::Lcd (submits to gpu3ds; LCD state incl. 3D slider) |
| `src/os3ds/src/services/dsp.rs` | DSP firmware + program execution into the media sink |
| `src/os3ds/src/services/ldr_ro.rs` | CRO load/unload; writes module bases into the library |
| `src/os3ds/src/services/cfg.rs` | region/language/model blocks |
| `src/os3ds/src/services/ptm.rs` | battery/play time |
| `src/os3ds/src/services/y2r.rs` | YUV→RGB |
| `src/os3ds/src/services/soc.rs` | sockets (native; wasm refuses per title) |
| `src/os3ds/src/services/misc.rs` | mic/cam/boss/news/ndm/ir refusals with gap lines |
| `src/os3ds/src/web.rs` | wasm arms (io/saves/audio/input bridging, docs/35 patterns) — added in 3DS-W7 |
| `src/os3ds/tests/*.rs` | service unit tests (apt state machine, fs paths, cfg blocks) |

**New crate `src/gpu3ds`** (PICA on Vulkan + web ring):

| File | What |
|---|---|
| `src/gpu3ds/Cargo.toml` | the crate (ash; glslang lane via shaders) |
| `src/gpu3ds/src/lib.rs` | crate root, `bind`/`procs` shape like `graphics`'s |
| `src/gpu3ds/src/gx.rs` | command-list parse → recorded frame (draws, state, fills, transfers) |
| `src/gpu3ds/src/targets.rs` | framebuffer/render-target bookkeeping, buffer swaps |
| `src/gpu3ds/src/shaders/{mod.rs,dvle.rs,tev.rs}` | DVLE VS/GS decode → GLSL; TEV combiners → fragment GLSL; glslang → SPIR-V |
| `src/gpu3ds/src/tex.rs` | 3DS formats/layouts, ETC1 decode pass |
| `src/gpu3ds/src/vk.rs` | the native Vulkan executor |
| `src/gpu3ds/src/web.rs` | the ring executor (docs/35 W4 shapes) — completed in 3DS-W7 |
| `src/gpu3ds/tests/*.rs` | gx parse fixtures, shader golden tests |

**`front` updates**:

| File | What |
|---|---|
| `src/front/src/ds/mod.rs` | the ds module root (entered when the built title is 3DS) |
| `src/front/src/ds/layout.rs` | screen arrangement settings + viewport computation |
| `src/front/src/ds/touch.rs` | pointer→touch mapping through the bottom viewport |
| `src/front/src/lib.rs` | module wiring |

**CLI updates** (`src/cli/src`):

| File | What |
|---|---|
| `main.rs` | help/doc text for `.3ds/.cia/.cxi`; doctor line |
| `pipeline.rs` | container-family detection, the 3DS build branch (translate/data/compile/link/web), 3DS `modules_c` |
| `cc.rs` | `find_engine3ds`, 3DS C flag set, 3DS web gen flags |
| `profile.rs` | `find3ds` over `games3ds`, the "3DS" profile line |

**Profiles + docs + launcher updates**:

| File | What |
|---|---|
| `src/profiles/src/lib.rs` | load `games3ds`, `check()` counts both |
| `src/profiles/games3ds/<program id>/ds.toml` | per-title data (added as titles are validated) |
| `Cargo.toml` (root) | workspace members: translator3ds, runtime3ds, os3ds, gpu3ds |
| `PORT_MAP.tsv` | a third tree (`DS = 3dsrecomp`) with donor rows; `scripts/port_map.py --check` covers it |
| `scripts/port_map.py` | the DS tree column/verification |
| `README.md` | Credits table: 3dsrecomp (MIT) |
| `docs/LAUNCHER_CONTRACT.md` | `.3ds/.cia` inputs, `control.smdh`, no-key note for decrypted dumps |
| `docs/INTERFACES.md` | translator3ds/runtime3ds/os3ds/gpu3ds module sections |
| `src/launcher/src/services/sources.rs` | `.3ds/.cia/.cxi/.app` validation (magic checks) |
| `src/launcher/src/services/artwork_decode.rs` | SMDH decode (icons + titles) |
| `src/launcher/src/constants/words_library.rs` | new source words |
| `src/launcher/src/services/sources_tests.rs` | the new accept/refuse cases |

Note on the `os` input/media reuse: `os3ds` reads pad/keyboard state and the audio sink without dragging HOS IPC
in — if `os`'s input and media modules do not split cleanly behind features, the small reusable parts (the pad
scan, `set_keyboard`, the sink interface) extract into a leaf crate (`src/input` / `src/mediasink`) and both `os`
and `os3ds` depend on it; that extraction is part of 3DS-W2's brief and is the only change to `os` this port
makes.

## 10. Risks and open questions

| # | Risk / question | Recommendation |
|---|---|---|
| R1 | The donor is WIP upstream; a vendored copy drifts | Vendor once at the current commit into `translator3ds`, note the pin in the wave doc; upstream diffs port hunk by hunk afterwards. Absorbing vs keeping a separate crate: **vendor into the engine workspace** (one build, one cache); not an owner decision by default, but noted (D4) |
| R2 | Encrypted dumps: 3DS key handling is a different key set (NCCH slots, CIA title keys) and a new policy surface | First waves accept decrypted dumps only and refuse with the donor's exact line (already implemented in the vendored rom.rs). A decryption lane later needs the owner's key policy (D2: same shape as prod.keys — `%LOCALAPPDATA%\nxrs\keys\` — or refuse forever) |
| R3 | Minimum game target: the port's first title decides W3-W5's order (which services the game actually touches first) | Recommend one of the donor's four proven titles (Pokémon Alpha Sapphire, Pokémon Y, Majora's Mask 3D, Persona Q — README) that the owner owns a decrypted dump of (D1: the owner names it; the waves' titles follow) |
| R4 | DSP audio is the largest single unknown: real titles load DSP firmware and run DSP-encoded audio; an honest path is a DSP program interpreter (the class of work Citra's teakra does) | Budget it as its own wave inside W5; until it lands, a title without audio must not pretend (the repo's real-I/O rule): the dsp service refuses audibly and the game decides. Do not ship a silent sink as a pass (D5: accept the interpreter's size, or defer audio-critical titles) |
| R5 | Geometry shaders on WebGPU: PICA GS has no WebGPU equivalent | Per docs/35 §4.4's rule: lower to instanced-vertex on the web or refuse per title with a named reason; never skip silently. Native Vulkan keeps real GS |
| R6 | Stereoscopy (the 3D slider): present one view per screen and report slider 0? | Recommend yes for the first waves; stereoscopic dual-view is a later opt-in wave (D6) |
| R7 | The ARM11 interpreter's quality decides how often unlowered code runs (the donor leaves uncommon instructions to the host) | The W2 differential check (donor's verify idea) is the gate; the hints loop turns misses into translated functions at the next build, as nxrs already does with `discovered.tsv` |
| R8 | Contract risk: the launcher's build-success check expects `<name>.exe` + `<name>_code.dll` + `<name>_data` | Unchanged by design (section 7); verify with a real launcher build in W1's evidence |
| R9 | `os` input/media extraction (the leaf-crate question in section 9) touches Switch code | Keep the extraction minimal and test-covered (`cargo test -p os`); it is the only `os` change this port makes (D7: approve the extraction if the feature-split proves ugly) |
| R10 | wasm memory model: page tables point into Emscripten linear memory; 4 GiB-scale reservations do not exist there | The page-table model is the mitigation (tables are arrays of offsets; uncommitted pages are null → callbacks). Proven or refuted in W0's emcc object + W7's run, like docs/35 W0 proved the Switch lane's |

Owner decisions this plan needs before or during the waves: **D1** the first 3DS title (recommendation: a donor-proven
title the owner owns decrypted); **D2** 3DS key policy (recommendation: decrypted dumps only for now, keys folder
convention later); **D3** whether DLC CIAs are ever in scope (recommendation: out of scope, launcher shows the
empty state); **D4** absorb the donor as a vendored crate (recommendation: yes, as section 3 argues); **D5** the
DSP interpreter's budget (recommendation: its own wave inside W5, teakra-class scope); **D6** stereoscopy
(recommendation: defer, slider 0); **D7** the `os` input/media leaf extraction (recommendation: allow it, minimal
and tested).

UNVERIFIED items in this doc, to pin in the waves: the full 3DS SVC numbering and service command surfaces
(3DS-W2/W3, against 3dbrew); the PICA200 register/combiner/DVLE specifics (3DS-W4); the 3DS guest clock rate for
GetSystemTick (3DS-W2, measured against a title); exact memory-map values for the region table (3DS-W2). The
donor-side statements (rom/cro/discover/codegen/abi/compile behaviour, MIT licence, the four tested titles, the
~99.8% coverage claim) are from its source and README as cloned at `D:\Projects\3dsrecomp` (2026-11-27).
