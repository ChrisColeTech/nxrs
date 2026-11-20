# Translator

nxnative rewrites a game's NSO in place and runs the ARM code natively. nxrs runs on x86-64, so its counterpart is a
static translator: every guest function is lifted to C ahead of time, compiled with clang and linked into
`<name>_code.dll`. The translator lives in `src/translator/` (`loader/`, `xlate/`, `aot/`); the runtime it calls into
is `src/runtime/`. This page keeps the rewriter page's questions (where code is found, how patches are placed, jump
tables, exclusives, system registers, relocations) and answers them for the translator.

| Part | Files |
|---|---|
| NSO loading, seeds, guest layout | `src/translator/src/loader/nso.rs`, `module.rs`, `layout.rs`, `zbic.rs` |
| One instruction to C | `src/translator/src/xlate/lifter.rs`, `simd.rs`, `util.rs` |
| Function-local flags and registers | `src/translator/src/xlate/flags.rs`, `regs.rs` |
| Computed branches | `src/translator/src/xlate/jumptables.rs` |
| Discovery, emission, chunks | `src/translator/src/aot/discover.rs`, `emit.rs`, `gen.rs` |
| Patch table | `src/translator/src/aot/patches.rs`, `nso_match.rs`, `hints.rs` |
| Headers the C includes | `src/translator/runtime/cpu.h`, `simd.h`, `simd_fp.h` |

The output text is kept byte-identical to mkw-recomp's generator so the two can be diffed (`src/translator/src/xlate/mod.rs`). PORT_PLAN
M1 records Mario Kart World's C as 242,014 of 245,653 functions identical, every difference explained.

## Function discovery

`aot::discover::discover` starts from the module's seeds (`loader::module::Module::seeds`): `.eh_frame_hdr` entries,
exported FUNC symbols, `R_AARCH64_RELATIVE` targets in `.text`, every relocation naming a symbol the module defines in
`.text`, and every global untyped symbol there. The untyped ones matter: MK8D's sdk exports its hand-written
`nndetailCryptoBignum*` routines untyped and reaches them only through the PLT. The module's first instruction is
always a start. Discovery then follows `BL` targets and branch targets out of known functions.

Code the build missed is not fatal any more. `rt_dispatch` sends an aligned in-module address with no lifted function
to the interpreter (`src/runtime/src/interp/mod.rs`), which runs it on the same `Cpu` and returns to lifted code at the
first known function start. Every entry address is appended to `<data>\discovered.tsv` (module, offset, four
instruction words as a stamp), and the next build adds them as starts (`aot::hints`); an entry whose words no longer
match (another game version) is left out. `nxrs-build --interp-only` or `NXRS_INTERP_ONLY=1` runs everything in the
interpreter, to tell a translator fault from a host one.

## What a function becomes

`aot::emit` writes one `void f_<addr>(Cpu *c)` per guest function:

- NZCV lives in a local `Flags F` and is written to `c->nzcv` only when control leaves other than by call or return
  (`src/translator/src/xlate/flags.rs`).
- The most used general registers live in C locals, synced with `Cpu` at calls, SVCs, traps, returns and tail exits
  (`src/translator/src/xlate/regs.rs`).
- `BL` is a C call; `RET` is a C return. Indirect branches and calls outside the chunk go through `rt_dispatch`, a
  flat table of one `u32` per guest instruction (`src/runtime/src/dispatch.rs`).
- FP / SIMD / crypto / CRC32 are lowered to calls of the inline helpers in `simd.h` with constant fields
  (`src/translator/src/xlate/simd.rs`); ARMv8.0-A + Crypto + CRC32 only, later encodings return `Unsupported` and become an `rt_udf` trap.
- A loop with no call on its back edge gets a compiler barrier; a back edge the patch table marks as a spin wait gets
  `rt_spin_gate`, which yields and lets the virtual-core scheduler run its checkpoint ([threads.md](threads.md)).

Functions are partitioned into chunks by estimated C size (`discover::partition`) and emitted in parallel
(`aot::gen::generate`); one chunk is one compile unit and one cached object ([packing.md](packing.md)).

## Jump tables

`src/translator/src/xlate/jumptables.rs` resolves a computed `br` statically: it walks back over the block and its predecessors
(`explore`), evaluates each path symbolically (`run`), and enumerates the values the path conditions allow (`solve`).
A whole function is resolved at once and iterated to a fixpoint, with the work capped (`MAX_STEPS`). A resolved table
becomes a C `switch`; an unresolved `br` falls back to `rt_dispatch`. The Rust port is result-identical with the Python
it came from on every `br` of MKW's modules (`src/translator/tests/jumptables_dump.rs`).

## Exclusives and atomics

The monitor is per thread in `Cpu` (`excl_addr`, `excl_val`, `excl_val_hi`, `excl_seq`). For Switch 1 programs a
global bucket table, `rt_excl_mon` in `cpu.h`, adds a sequence per 16-byte bucket so a store from another thread
breaks a pending `STXR`; Switch 2 programs run without it. `SWR_XMON=1` / `SWR_NO_XMON=1` force it
(`src/runtime/src/process.rs`). LDAR / LDAPR / STLR / LDXR are lowered as atomic accesses. x86 orders a later load
before an earlier store, which matters where a game relies on a load-acquire to order a plain store: Game Freak's
Trinity rendezvous completer (Violet, Z-A) gets a `dmb ish` after its release-flag store
(`cli::profile::trinity_sites`, set `trinity`, `NXRS_PATCH_TRINITY=0` off).

### Trinity rendezvous hang

- **Symptom:** Pokémon Scarlet / Violet or Legends: Z-A stops for good while its main thread waits.
- **Cause:** the rendezvous completer sets its release flag with a plain store and then reads the state with a
  load-acquire; on x86 the store can pass, so the main thread resets the state, reads the flag as 0 and parks.
- **Fix:** the build adds a `dmb ish` after the store (set `trinity`). Two matching functions in one module place
  none and say so in red.

## Thread and system registers

`TPIDR_EL0` and `TPIDRRO_EL0` are `Cpu` fields; the kernel points `tpidrro` at each thread's 0x200-byte TLS region in
the runtime's kernel region, and the thread id is also stored at TLS+0x110 (SDK 22). `CNTPCT` / `CNTVCT` /
`CNTFRQ` read the runtime's counter (`rt_ticks`, `rt_tick_hz`): 19.2 MHz for Switch 1 titles, 31.25 MHz for
Switch 2, `SWR_TICK_HZ` overrides. FPCR.FZ is mirrored into the host MXCSR (FTZ|DAZ) while guest code runs.

## Patch table

`aot::patches` reads the patch-table text the cli composes from the profile, the player's mods and code cheats:

```text
set    <name> <env> <default>                     a switchable set (NXRS_PATCH_<NAME>, NXRS_MOD_<NAME>, NXRS_CHEAT_<hash>)
site   <set> <module> <offset> <orig> <repl>     both words are lifted; the set's byte in rt_patch_on picks one at run time
span   <set> <module> <offset> <word>...         a span skipped whole
spin   <module> <head> <back-edge>               a spin-wait back edge (rt_spin_gate)
area   <set> <module> <word>...                  stub code, appended to <module>_stubs
areloc <module> <index> <op> <target> <addend>   relocation of an area word (adrp / b / bl to state, area or module)
```

Because a site keeps both the original and the patched word, every patch can be flipped while the game runs:
`rt_patch_on` is handed to the runtime at start (`nxrt_patch_table`, `src/runtime/src/patchset.rs`). A module with an
area gets a code module of its own, `<module>_stubs`, after the last module; its first page is the frame-step state
page ([frame-rate.md](frame-rate.md)).

Where the entries come from (`src/cli/src/profile.rs`, `src/profiles/src/patches.rs`):

| Source | Becomes |
|---|---|
| `cut` rows keyed by a module build id | sites; original bytes asserted first |
| `[[match]] kind = "cuts"` rules (function fingerprint, `aot::nso_match`) | sites of set `match` (`NXRS_PATCH_MATCH=0` off) on builds no row covers: Arceus's capture-demo crash fix, OoT's movie cuts |
| `[[stub]]` / `[[match]] kind = frame_step / stubs` | `area` + sites `@b:area:<off>`, sets `fstep` (`NXRS_PATCH_FSTEP`) and `stubs` (`NXRS_PATCH_STUBS`); stubs that read the module are assembled by `src/cli/src/stubgen.rs` |
| `src/profiles/games/<id>/recomp.txt` (MKW 1.8.0, mkw-recomp's table) | sets `NXRS_PATCH_MKW_*`, `bl`s to a patch page handled by `src/os/src/gamefix/` |
| `turbo.toml` (owner patches, e.g. Z-A 60 FPS) | cuts |
| player mods, code cheats | a set each ([cheats.md](cheats.md)) |
| `NXRS_PROBE=<module>:<offset>,...` | probe lines: `rt_probe` logs registers at that instruction |

`NXRS_NO_PROFILE_PATCHES=1` builds without the profile table; `NXRS_PATCH_PROFILE=0` turns off at run time the
profile patches that have no switch of their own. `nxrs-build profile <title id> [<build id>]` prints what applies.
nxnative's observation gates are parsed but not built: every gate in its profiles is an Apple address-space
workaround (PORT_PLAN 5A.1, "nxnative patch machinery applied").

### Patches across updates

`aot::nso_match` reimplements nxnative's `nso_match.py`: functions are the unwind table's FDEs; a body digest is the
SHA-256 of the instruction words with moving immediates cleared (outside branches, ADR / ADRP, literal loads, the low
12 bits an ADRP gets from its ADD / LDR / STR); the fingerprint adds the callees (function, import or SVC wrapper). A
rule applies where exactly one function of its size has one of its fingerprints, and each cut's original bytes are
checked there. The fingerprints are checked equal to nxnative's
(`nso_match::tests::arceus_capture_demo_function_matches_nxnative`).

## Relocations and module layout

`loader::layout` places modules in Horizon's order (`main`, `subsdk0..9`, `sdk`; rtld dropped), each aligned to
2 MiB from `0x70_0000_0000`, below 2^39; run-time NROs (`nro_<index>`) follow at fixed bases. The layout goes to
`layout.json` / `layout.h`. At start `src/runtime/src/loader.rs` maps each image there, applies RELATIVE, ABS64,
GLOB_DAT, JUMP_SLOT and packed RELR, and binds imports by name across modules in load order. The `HOST_OVERRIDE` rule
lets a host function registered under a symbol name replace that symbol in every module, also where a module defines
it: that is how the OS layer takes over SDK service clients ([services.md](services.md)). An import nothing provides
binds to a stub that names it when called (`UNIMPL Import`).

## setjmp, unwinding and fibers

Lifting breaks `longjmp`'s final `br x30` into the middle of a function whose frame is gone. The generator makes every
caller of setjmp (`discover::setjmp_owners`) and every function holding a landing pad (`landing_pad_owners`) an
"owner" entered through `rt_sj_owner` (`src/runtime/src/asm.rs`); a dispatch to an address strictly inside a live owner
jumps back to its frame and continues at `c->pc` (`src/runtime/src/sjlj.rs`). The invariant: no non-local jump ever
skips a live Rust frame (`src/runtime/src/boundary.rs`). Fibers get host fibers ([threads.md](threads.md)).

## Build speed

Translation of a large Switch 2 main takes about 20 s (MKW: translate 21 s, compile 373 s in PORT_PLAN M9); the
compile dominates and is incremental: generated files that come out unchanged are not rewritten, objects are cached
under a content key, so only changed chunks recompile; a runtime update only relinks.

## Checking a translator change

- `cargo test -p translator`: lifter unit tests (e.g. `stgp_is_not_a_32_bit_stp`), stub area tests, synthetic packages.
- `nx-oracle` (`src/oracle/src/main.rs`): random words of 71 instruction classes, lifted C against dynarmic from
  random states, every architectural result compared; `--interp` checks the interpreter the same way; `--fpcr` and
  `--words` narrow a case. PORT_PLAN T-lane: 140,520 cases, 0 mismatches. Not compared: FP with FPCR other than 0.
- `nxrt-dev e2e` (`src/runtime/src/bin/nxrt_dev.rs`): homebrew tests built, translated, linked and run.
- `src/translator/tests/jumptables_dump.rs`, `simd_words.rs` against the reference output.

## Known limits

- `RET Xn` with n != 30 is lowered as a return; PACGA is not modelled (PORT_PLAN T-lane).
- FPSR cumulative flags are not all tracked by the portable C.
- Only the portable C path exists; no inline-assembly path (`src/translator/src/xlate/lifter.rs`).
- nxnative's Capstone dependency and its patch-area reach limits do not apply: the translator has its own decoder and
  stub areas are separate code modules.
