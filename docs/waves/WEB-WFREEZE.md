# WEB-WFREEZE - a web build freezes the whole computer

**Owner:** "a web build freezes the whole computer while it runs." Find the phase and the cause, fix it, show the
machine stays responsive. Title: Anime Girls Sun of a Beach (0100C6801E0FA000), its library web folder.

## Plan

1. Measure one full web build (the launcher's arguments, `--target web --shaders-from <native data>`, into
   `<base>\nxrs-web\Anime Girls Sun of a Beach`; objects and translated shaders of that folder removed first so every
   phase runs), sampled every second: total CPU, CPU per image (clang, python, node, dmsh_xlate, spirv-val, wasm-ld,
   nxrs-build), commit and available memory, disk queue / bytes / latency per drive, and a responsiveness probe every
   250 ms (a 1 ms timer's latency and a `WM_NULL` round trip to the taskbar at normal priority). Evidence:
   `build/WEB/WFREEZE/*.tsv`, the phase of each second taken from the timestamped build log.
2. Fix what the data points at. Minimum: `nxrs-build` lowers its own process to below-normal priority at start (every
   compiler, Python, node and wasm tool it starts inherits it); the launcher starts its engine jobs below normal.
   A phase bound by disk or threads gets a cap justified by the numbers.
3. Measure the same build again; table phase / duration / CPU / probe p50, p99, max, before and after.

## Expected files

- `src/cli/src/main.rs` (+ a small priority module): below-normal priority for the whole build tree.
- `src/launcher/src/services/platform/windows_tree.rs`: `BELOW_NORMAL_PRIORITY_CLASS` on the engine job (Linux:
  the engine's own nice 5 covers it; the launcher's Linux twin is unchanged).
- whatever phase the measurement names: none (no phase was disk- or memory-bound; see Results).

## Results (2026-11-27)

Three full web builds of Anime Girls with the launcher's arguments (`--jobs 8`: `library.json` `jobs` = 8, the
launcher's value; `--shaders-from` the native data folder, `--target web`), the web folder's `_build` and
`_data\shaders` removed before each so every phase ran (C written, 417 emcc compiles, 53,109 shaders translated and
validated, link, data extents, gzip). Other sessions' Ocarina of Time and Mario Kart World ran during all three (and
rustc builds of other waves at times), so durations are indicative. Evidence: `build/WEB/WFREEZE/` (`*-samples.tsv`
sampler rows, `*-probe.tsv` 4 Hz probe, `*-build.log` timestamped log, `*-per-second.tsv` joined with the phase,
`summary.txt`, and the scripts). `before1` had no CPU-work probe yet; `before` and `after` are the pair compared.

What the data shows (before):

- **CPU, not memory or disk.** Machine CPU averaged 88-91% through the compile (max 99-100%) with the run queue at
  7-15 on average and spikes of 58-114 ready threads; the build tree (8 clang + their emcc Pythons, 4 dmsh_xlate,
  8 spirv-val at a time, nxrs-build's own 2-12 cores in translate and scan) ran at **Normal** priority beside the
  games. Commit peaked at 93 GB of 146, available memory never under 73 GB, hard faults negligible; disk queues
  stayed under 7 (P:) / 3 (C:) with latency under 25 ms. Link (6-21 s) and pack + gzip (2-5 s) are short and not the
  cause.
- **What a normal-priority program got:** a fixed 2 ms piece of CPU work took 3.0 ms p50, 5.3 ms p99, 643 ms max
  during the compile (2.4 ms p50 idle beforehand): every normal-priority thread (the desktop, the games) shared the
  cores with the build at equal priority.
- Rare whole-machine hitches (0.1-0.6 s: the sampler's own second stalled too) appeared once or twice per build,
  every time while spirv-val validation overlapped the compile; a 180 s spirv-val process storm alone (259
  processes/s, below normal) produced none, so they are not the process storm by itself (see Unproven).

Fix: `nxrs-build` sets its own process to below normal before anything else (`src/cli/src/priority.rs`; Linux nice
5), so emcc's Python, clang, node, wasm-ld, dmsh_xlate and spirv-val all inherit it (seen in the after run: every
clang, python, dmsh_xlate and nxrs-build of the tree BelowNormal); the launcher starts its engine jobs (build, update
check, DLC sync) with `BELOW_NORMAL_PRIORITY_CLASS` too. No cap: nothing was disk- or memory-bound, and below-normal
work still takes every idle core, so an idle machine builds as fast as before.

| run | phase | duration s | CPU avg/max % | sleep(1) p50/p99/max ms | taskbar WM_NULL p50/p99/max ms | 2 ms work p50/p99/max ms |
|---|---|---|---|---|---|---|
| before | translate | 21 | 69/86 | 1.9/2.5/2.5 | 0.08/0.2/0.2 | 3.0/3.3/3.3 |
| before | compile (+ shaders) | 508 | 91/99 | 2.0/2.5/254 | 0.11/1.1/6.0 | 3.0/5.3/643 |
| before | link | 6 | 66/72 | 2.0/2.0/2.0 | 0.10/0.3/0.3 | 3.0/3.1/3.1 |
| before | waiting on shaders | 136 | 68/96 | 2.0/2.2/2.4 | 0.09/0.6/16.1 | 2.7/3.2/3.3 |
| before | pack + gzip + host | 2 | 76/77 | 1.5/3.1/3.1 | 0.08/0.1/0.1 | 3.0/3.1/3.1 |
| before | **total** | **673** | | 2.0/2.5/254 | 0.10/0.9/16.1 | **3.0/4.5/643** |
| after | translate | 25 | 77/100 | 2.0/2.0/2.5 | 0.09/0.3/0.3 | 2.5/3.4/5.1 |
| after | compile (+ shaders) | 494 | 78/100 | 2.0/2.3/128 | 0.09/0.7/2.6 | 2.2/3.4/6.8 |
| after | link | 9 | 32/57 | 2.0/2.3/2.3 | 0.09/0.2/0.2 | 2.2/3.1/3.1 |
| after | pack + gzip + host | 2 | 58/58 | 2.0/2.0/2.0 | 0.11/0.1/0.1 | 2.4/2.6/2.6 |
| after | **total** | **531** | | 2.0/2.3/128 | 0.09/0.7/2.6 | **2.3/3.4/6.8** |

(After: the shaders finished during the compile, so there is no waiting phase. Idle before each run: 2 ms work
2.4 / 2.3 ms p50.) Build time 673 s -> 531 s under the same kind of load (shaders 648 -> 482 s, compile 509 -> 494
s); the games' load varied, so this is not a speed-up claim, only that below normal cost nothing measurable.
