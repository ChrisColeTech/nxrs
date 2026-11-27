# WEB-W11 - Module size

Wave of `docs/35-WEB-TARGET-PLAN.md` W11 ("Module size"). Base `f37a15a`, branch `web-w11`, worktree
`D:\Projects\RecompLauncherEngine-web-w11`. Title: Beach Bounce Remastered (01004060145C0000), library entry output
name `Beach Bounce Remastered nsw2u`. Chrome on GPU 1 only (`--use-adapter-luid`), one game Chrome on the machine.
Protocol: `docs/31-DELEGATION-PROTOCOL.md`. Evidence: `build/WEB/W11/`.

## 1. Plan

1. **The launcher did not see Beach Bounce's web build.** The launcher's web folder is
   `<game folder>\nxrs-web\<outputName>` (`model::web_build::web_dir`, the same `-o` it passes to the build); the
   build in the library was made by hand with `-o "Beach Bounce Remastered"` (WEB-W7) and sat in
   `nxrs-web\Beach Bounce Remastered`. A CLI build without `-o` names its folder after the package's NPDM name
   (`pipeline::default_web_outdir`), which the launcher's output name (from the file name) need not equal. Fix:
   the folder renamed and rebuilt into the right place; Play and the "web build" fact look for the game's own folder
   first and otherwise for a web build beside it whose `build.json` names the same title id
   (`services::web_serve::played_web_dir`), so a hand build under another name is still found.
2. **Measure** where the bytes go (`build/WEB/W11/tools/wasmsize.py`: sections, functions by size, duplicate bodies;
   data segments; the web engine library's data per crate).
3. **Try each technique on its own**, keep what pays: unreachable guest functions at translation, identical-function
   folding (wasm-opt), wasm-opt levels against time, compile flags, names, data/table compaction.
4. **Verify**: cli/translator/profiles tests, the size table before/after, one Chrome run of the final module to the
   story scenes (as WEB-W7 run7), compile+instantiate time from the page log, FPS.

## 2. Expected files

| File | Change |
|---|---|
| `src/launcher/src/services/web_serve.rs`, `build_facts.rs`, `flows/web.rs`, `tests/web_found.rs` | item 1 |
| `src/profiles/build.rs` | data the web engine does not need left out (found in item 2) |
| `src/cli/src/cc.rs`, `pipeline.rs`, `websplit.rs` | link / wasm-opt choices that pay |
| `src/translator/*` | only if unreachable-function elimination pays |
| `docs/35-WEB-TARGET-PLAN.md` | the W11 row |

## 3. Results (2026-11-27)

**Where the bytes went** (`build/WEB/W11/sizes-baseline.txt`, `tools/wasmsize.py`): the baseline module (this
worktree's base, one module, no wasm-opt) was 138.1 MiB: code 110.1 MiB (87,633 functions; 4,491 hold half the code,
the largest 753 KiB), **data 27.6 MiB**, element 0.2 MiB, no name section. 27.3 MiB of the data was one segment;
`llvm-nm` on `libnxos.a` put 26.7 MiB of the web engine's data in the `profiles` crate: every profile's
`shaders.tsv.zst` (nxnative's shader tables, 26.2 MiB over 207 profiles) embedded with `include_bytes!`. Only
nxrs-build reads them (it writes a title's tables into the data folder); the engine never does. Already compressed,
so gzip could not shrink them: half the gzip module was data no web run reads.

**Kept:**

1. `profiles/build.rs` leaves the shader tables out when the target OS is emscripten (`shader_tables` answers empty
   there). Native builds unchanged.
2. `websplit::fold`: the one-module build runs `wasm-opt --duplicate-function-elimination` (8 binaryen threads)
   after the link; `NXRS_WEB_FOLD=0` skips it. A folded call reaches an identical body: no run-time cost.

| Module (Beach Bounce) | raw MiB | gzip MiB (build's copy) | Chrome compile (bench, 3 runs) | build |
|---|---|---|---|---|
| baseline (`f37a15a`) | 138.1 | 50.6 | 0.16 / 0.27 / 0.23 s | link 13.9 s |
| profiles' shader tables out | 113.4 | 25.8 | 0.12 / 0.17 / 0.16 s | link 3-4 s |
| + identical functions folded (**final**) | 103.8 | 25.2 | (dfe alone: 0.09 / 0.13 / 0.14 s) | link + fold 32 s |

**Measured and not kept** (each alone on the 113.4 MiB module; `optsweep.tsv`, zlib-6 bytes):

| Technique | raw | gzip | time | why not |
|---|---|---|---|---|
| unreachable functions left out at translation, every exported symbol a root | 3 functions | - | - | nothing to gain: vtables (RELATIVE relocations) and exports reach almost everything |
| same, only symbols another module imports as roots | 109.3 MiB (-3.6%) | 25.0 MiB (-3%) | - | small, and the runtime calls ~50 SDK functions by name (`hle::sym`): they would run in the interpreter. Diff kept in `build/WEB/W11/prune-attempt/` |
| `wasm-opt --merge-similar-functions` | -16.7% | -5.1% | 19 s | merged guest functions gain a forwarding call (FPS risk); not measured in game |
| `wasm-opt -O1` / `-O2` / `-Os` | -12% / -12% / -13.6% | -5.8% / -5.4% / -6.8% | 203 / 272 / 370 s | minutes of build for ~6% gzip |
| game code at `-Os` (`NXRS_CFLAGS=-Os`) | 106.8 MiB (-5.8%) | 24.8 MiB (-3.9%) | compile 179 s vs 161 s | flag helpers no longer inlined (FPS risk) for 4% |
| names stripped | - | - | - | already: the one-module link writes no name section |
| data/table compaction | - | - | - | after (1) data is 2.9 MiB (the function address tables); element 0.2 MiB |

Chrome compiles lazily: `WebAssembly.compile` of any variant is 0.1-0.3 s (`compile-bench-1.txt`); the cost of a
large module is its download and memory, so gzip and raw size are the measures.

**Game** (`build/WEB/W11/run1-final`, Chrome on LUID {0,113326} = PCI bus 4, chrome-gpu.txt `*ACTIVE*`; dev_server
port 8776, cold profile; X every 4.2 s from 32 s as run7): `game.wasm: fetched, compiled and instantiated in 0.45 s`;
title menu at 31 s, Grandma Umi in the hospital at 90 s, Tomoyo "I'll do it!" at 150 s, Yuuki at the Beach Bounce desk
at 210 s, the same scenes as WEB-W7 run7; 0 red lines, 0 WebGPU errors, audio -20 dBFS. FPS (`[vk] pacing`, 60-232
s): mean 53.3 over 34 windows (36.0-59.5); WEB-W7 run8 had 55.6 (42.8-59.8). Other sessions' games and W8's builds
were running on the machine (GPU 1 at 79% before the run), so the numbers are indicative.

**Launcher name fix:** the library folder renamed to `nxrs-web\Beach Bounce Remastered nsw2u` (with its `_build`,
`_data`, log) and rebuilt there with `-o "Beach Bounce Remastered nsw2u"`; `played_web_dir` finds a build under
another name by `build.json`'s `titleId` (test `tests/web_found.rs`).

**Anime Girls** (W8's title, not built here): the same engine library is in its module, so the same 26 MiB of data
leaves it (expected ~107 -> ~82 MiB gzip); not measured.
