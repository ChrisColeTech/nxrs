# 3DS-W0 — Feasibility: the donor's front half vendored into `translator3ds`

**Status: wave in progress.** Brief: vendor 3dsrecomp's front half (rom, cro, image, discover, arm, thumb,
codegen, natives, overrides, the `recomp.h` v8 ABI) into a new workspace crate `src/translator3ds`, prove a
synthetic `.cxi` fixture translates to C that compiles with the repo's clang AND emcc, `cargo test -p
translator3ds` green, the workspace default `cargo test` untouched and seconds-fast. Plan: `docs/36-3DS-PORT-PLAN.md`
§2 (inputs), §3 (decision: vendor, option b), §8 (W0 row), §9 (file list), §10 R1/R2. Ledger row: `docs/32`
"3DS port — 2026-11-27", 3DS-W0. Base commit `a25c3c3`, branch `3DS-W0`.

## The donor, pinned

`D:\Projects\3dsrecomp` at `d143f2c51834bd89f32c0ac79465f1855d7d6f8e` (read-only for this wave; tree verified clean
at the pin). MIT per its Cargo.toml (`license = "MIT"`, `rust-version = "1.88"`); the workspace toolchain is 1.99.0,
past it. The pin rides this doc (R1): upstream diffs port hunk by hunk later.

## Expected files

New crate `src/translator3ds` (plan §9's table, W0 scope) plus the one outside file, the root `Cargo.toml` member
line:

| File | Source | State in W0 |
|---|---|---|
| `Cargo.toml` | new (authored) | workspace member; `license = "MIT"` like the sibling crates; dep: none outside the workspace (see deviations: Blob) |
| `src/lib.rs` | donor lib.rs | re-exports, `Mods`, `programs()`, `static_module`, `module_files`; donor's unit tests kept; `compile`/`build`/`port`/`verify` modules not carried |
| `src/rom.rs` | donor rom.rs, **reworked** | NCSD/NCCH/ExeFS/RomFS/.cia read + LZSS, encrypted refusals verbatim, over a `Blob` positional reader; donor tests kept |
| `src/cro.rs` | donor cro.rs | verbatim incl. tests |
| `src/image.rs` | donor image.rs | verbatim incl. tests |
| `src/discover.rs` | donor discover.rs | verbatim incl. tests |
| `src/arm.rs` | donor arm.rs | verbatim incl. tests |
| `src/thumb.rs` | donor thumb.rs | verbatim incl. tests |
| `src/codegen/{mod,arm,thumb,vfp,hoist,locals}.rs` | donor codegen | verbatim except `recomp_abi::` → `crate::abi::` and the deviations below |
| `src/abi.rs` + `src/abi/recomp.h` | donor abi/recomp.h | **verbatim header** (`include_str!`); plus the four mirror items codegen needs (`HEADER`, `GENERATION`, `NO_ORIGIN`, `CodeHash`) from donor abi/src/lib.rs; the full Rust ABI mirror is W2's (runtime3ds) |
| `src/natives.rs` | donor natives.rs | verbatim incl. tests |
| `src/overrides.rs` | donor overrides.rs | verbatim incl. tests |
| `src/compile.rs` | donor compile.rs | **not in plan §9**: W0 test/verification support so the donor's own compile lanes run; W1's `cc.rs` lane replaces its use; verbatim except its stress test gated |
| `src/hints.rs` | donor build.rs hints half (authored file) | `read_hints`, `section`, `code_hash`, `new_functions`, `apply_hints`, `bundled(list, id, hash)` shape; donor's tests adapted |
| `src/gen.rs` | donor build.rs::generate body (authored W0 form) | `GenOptions3ds`, `Event::{Generated, Note}`, writes generated C + overrides + natives under a dir |
| `src/title.rs` | new (authored, minimal W0 form) | `DsKind::{Cart, Cia, Cxi, App}` detection by magic, `open()` → `Game3ds { kind, title }`; donor covers Cart/Cia/Cxi detection, App is new |
| `tests/synthetic.rs` | new (authored) | fixture builder (donor rom.rs/cro.rs test shapes), end-to-end translate, C-shape assertions |
| `tests/real_titles.rs` | new (authored shell) | skipped without `NXRS_3DS_TITLE`; never opens a game the brief does not own |
| root `Cargo.toml` | workspace edit | member added; nothing else |

## Deviations from the donor (each a documented hunk)

1. **rom.rs reads through a `Blob`.** The plan says "the same seek/read abstraction the Switch package uses
   (`package::io::Blob`)". `package::io` is **private** in `src/translator` (mod.rs:27), and W0 may not touch a
   Switch crate file. rom.rs therefore carries a local `Blob` twin with `package::io::Blob`'s exact shape
   (positional `read_into`/`read_at`, the same "data at ... is cut short" rule) returning `rom::Error`; W1 swaps the
   import when `io` goes pub (one-line change there, recorded here for that wave). No `Mutex<File>` any more: Blob
   reads are positional, so `Title` keeps no lock (donor behaviour preserved, one less moving part).
2. **rom.rs splits `Title::load` into `open(blob)` + `from_ncch(blob, base)`.** Donor detection (NCSD at 0x100 /
   NCCH at 0x100 / CIA header size at 0) stays verbatim inside `open`; `from_ncch` is the partition-reading half so
   `title.rs` can add the bare `.app` case (NCCH at 0) the donor refuses. Error lines verbatim.
3. **codegen/mod.rs: `recomp_abi::` → `crate::abi::`** for `HEADER`, `NO_ORIGIN`, `GENERATION`, `CodeHash` (5
   non-test sites, 2 test sites).
4. **codegen/mod.rs: the three `#[cfg(feature = "host")]` tests and their `library()` helper are not carried.**
   They build a DLL and run it through the Rust ABI mirror (`Library::open`, `Context`, `Host`) — that mirror is
   runtime3ds/W2 work; the feature is not vendored here. The string-assertion tests all stay.
5. **codegen/mod.rs `compiles()` and compile.rs's long-path stress test are env-gated** (`NXRS_3DS_CC_TESTS=1`) and
   skip (not fail) when no C compiler is found. Donor default was "always, panic without a compiler"; the workspace
   rule needs the default `cargo test` seconds-fast, and W0's compile proof runs explicitly with the gate on.
6. **compile.rs vendored although plan §9 does not list it** (see table): the donor's compile lane is what makes its
   codegen tests compile-check as upstream; W1's `cc.rs` 3DS flags take over in the pipeline.
7. **hints.rs carries the mechanism, not the data plumbing**: donor `hints()` reads the installed-library dir and
   `apply_hints` pulls `bundled` itself; here `apply_hints(program_id, extra_hints, programs, analyses)` takes the
   hint addresses as a parameter (the pipeline/runtime feed them from W1/W2), and `bundled(list, program_id, hash)`
   takes the list text. The donor's `hints.txt` bundle, `cache_dir`, `install`, `library_dir/name` (install
   machinery) are not vendored.
8. **gen.rs/title.rs are W0-minimal**, per the brief: open + kinds + programs + generate; `write_data`, the RomFs
   adapter, SMDH control (`control.smdh`), the `romfs.ds` marker, and the pipeline's progress-line shapes are W1 —
   planned API noted in §"W1 owns" below, nothing stubbed as working.
9. **abi.rs is the header verbatim plus four mirror items**, not the donor's whole Rust ABI crate (815 lines with
   its own `load` feature and its own tests): W2 vendors the mirror next to the host that needs it.
10. **lib.rs doc example** reworded to W0's surface (the donor example names `compile`).
11. Headers: the donor files carry no attribution headers (module doc comments are behavioural and stay as upstream);
    the crate's Cargo.toml keeps `license = "MIT"` like its siblings; the README credits table is a later wave's
    (plan §9).

## Vendor method (declared, for the lead's diff check)

Upstream-verbatim files (cro, image, discover, arm, thumb, natives, overrides, codegen/*, compile.rs, recomp.h) are
copied byte-for-byte from the donor at the pin and their SHA-256 hashes recorded against the donor's (evidence under
`build/DS/W0/`), then the deviations above are applied as Edit hunks. This follows the house vendoring precedent
(`src/os/third_party/renhdrum` copied in whole, RENHDRUM.md) — a pinned upstream copy is the reviewable unit, and
hash equality proves fidelity in a way retyping cannot. Authored/reworked files (Cargo.toml, lib.rs, rom.rs, abi.rs,
hints.rs, gen.rs, title.rs, tests) go through Write/Edit as the protocol directs; no script writes or rewrites any
`.rs`/`.toml`/`.md` content.

## Verification plan (what the report will carry)

- `cargo test -p translator3ds` — every test listed with what it asserts; then
  `NXRS_3DS_CC_TESTS=1 cargo test -p translator3ds` for the compile lanes.
- The synthetic fixture's generated C compiled with the clang cc.rs's discovery finds (`candidates()` order:
  NXRS_CLANG → beside-exe → NXRT_LLVM → `C:\Program Files\LLVM\bin\clang.exe` → PATH; on this machine it lands on
  `C:\Program Files\LLVM\bin\clang.exe`, clang 21.1.8) at the donor's measured flags (`-O2 -ffp-contract=off
  -fno-math-errno -w`), archived to a static lib; invocation, sizes and SHA-256 recorded.
- The same C through emcc to a wasm object, driven exactly as `cc.rs::find_emcc` drives it (emsdk's own python, `-E
  emcc.py`, `EM_CONFIG`): the SDK is the dev tree's `build\tools\emsdk` (this worktree's `build\tools` junction is
  absent, so the path points into the main checkout's tool folder — a tool read, not a worktree edit).
- `cargo test` at the workspace root — default suite green, seconds-fast, no Switch crate touched.
- Evidence under `build/DS/W0/` (compiler logs, hashes, test output). `build/` is not committed.

## W1 owns (planned API shape, not built here)

`Game3ds::write_data` (staging `exefs`-equivalent programs, RomFs extraction through a
`translator::package::romfs`-shaped adapter, `control.smdh` from the ExeFS `.icon`, the `romfs.ds` marker),
`SourceKind`-adjacent detection wiring in `pipeline.rs`, `cc.rs`'s 3DS flag set + `find_engine3ds`, the progress-line
contract (`Analyzing the code...`, `C generated: n/m chunks`, `C code: ... functions`), the hints file location
replacing the donor's library-dir read, and the launcher's `.3ds/.cia` import. W0 ships none of that; nothing here
pretends to.

## Outcome

(filled in when the wave closes)
