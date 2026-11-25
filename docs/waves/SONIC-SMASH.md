# SONIC-SMASH — SONIC X SHADOW GENERATIONS and Super Smash Bros. Ultimate: fix the recorded defects

Owner: one delegated agent. Lead: main session. Opened 2026-11-25; **rewritten fix-first 2026-11-25** after the owner
stopped the first attempt for rebuild-first churn (rebuild, run, look, repeat). Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Owner's direction.** Start from the defects already recorded in `docs/34-LIBRARY-VALIDATION.md` (sections
"SONIC X SHADOW GENERATIONS runtime" ~line 491, "Super Smash Bros Ultimate runtime" ~line 536, bug register ~line
565). **Change code first; build and run only to measure a correction.** Do not start by rebuilding the titles to
"see where they stand": the recorded state is the baseline. The old run folders (`nxrs-agent2/runs/...`) no longer
exist; the numbers quoted in doc 34 are the "before".

**Recorded baseline (doc 34, engine of 2026-11-22):**
- Shadow (program 0): reaches gameplay, 29.99 FPS of 30, p95 36.4 ms, 7,220 MB peak; diagnostics FAIL (218 lines /
  4 issues); five blacklisted pipelines. Sonic (program 1, successor process): Green Hill gameplay, ~31.5 FPS, clean.
- Smash: match to results, 58.39 FPS of 60 (p95/p99/max 23.7/107.8/516.9 ms, ~53 stutters), 2 gameplay audio
  underruns, memory 6,146 -> 6,912 -> 6,544 MB over loading/match/results, one VP9 decode error.

**Work, in order — each item ends with a source correction, or a proof from source that no correction is owed:**
1. **`SXSG-B01`, Shadow's unaligned `nvnMemoryPoolMapVirtual` requests** (215 requests with offsets `1..0x15` and
   64/128/256 KiB sizes; NVN documents 4 KiB alignment; the MIT reference refuses them). Decide from source and the
   request bytes what those values are — e.g. whether the 0x28-byte `MapReq` (`src/graphics/src/nvn/vpool.rs:46`) is
   read at the wrong stride/field for this title's NVN version, whether the values are page or tile indices, or whether
   they are storage-class/flag fields misread as offsets. Do not round blindly and do not treat them as pages without
   evidence. Correct `vpool.rs` / `vk/sparse.rs` (or the caller in `nvn/pool.rs:159`) at the cause.
2. **Shadow texture ID 519** (live 2D placeholder serial 7549 sampled as cube/3D, receives a zero view): the cube twin
   (`docs/waves/CUBE-TARGET.md`, `MK-PAIR.md`) may already cover the cube case — confirm from source; correct the 3D
   case if the twin does not cover it.
3. **Smash stutters and memory**: from source, check whether the ~53 stutters are the harness's own screenshots (as
   MK8D's were: `docs/waves/MK-PAIR.md`) or real (shader/pipeline compiles mid-match, pool syncs, audio). Correct a
   real cause; look at the memory curve's shape (loading reservation vs growth) before calling it a leak.
4. The five blacklisted Shadow pipelines stay unless a specific corrected pipeline is proven safe (protocol §3).

**Then measure, once per correction set.** The titles are not built on the current engine: build the engine only if
you changed engine code (`pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache`, ~4 min), then
`dist\nxrs\nxrs.exe --select-title "SONIC X SHADOW GENERATIONS" --build` and `... "Super Smash Bros Ultimate" --build`
(first builds on this engine may take long — Smash's was 2,516 s; run in the foreground with a timeout under 10 minutes
and poll it with bounded foreground checks if it runs longer), then `py scripts/play.py shadow`, `sonic`, `smash`.
Adjust `scripts/scenarios.toml` only if a route no longer reaches gameplay (fresh saves; `NXOS_SWKBD=auto` answers name
prompts). Read `report.md` and look at every `sheet-*.png` yourself.

**Done:** per title, a correction with its game result: window FPS vs target, 5% low, worst pacing, red lines,
memory, and the sheet showing controllable gameplay; or a parked blocker with evidence.

**Baseline tree:** branch `gameplay-harness`, HEAD at brief time (pull first); `dist/nxrs` = HEAD engine. Leave the
lead's uncommitted docs (`docs/32-*.md`, `docs/waves/MKW-PERF3.md`) alone.

**Shared machine.** Another agent (MKW-PERF3) may be building or running MKW, and another session may run short
`cargo test`s in this checkout. **One build or game at a time:** check `tasklist` for cargo/rustc/nxrs-build and game
exes before starting a build or a run, and wait for them. Do not edit `src/graphics/src/vk/exec.rs`, `vk/mem.rs` or
`perf.rs` (MKW-PERF3's files) without telling the lead in your report; never edit `.github/workflows/*`,
`src/launcher/**`, `src/runtime/tests/abi_layout.rs`.

**Exclusions:** blacklists stay unless the exact pipeline is proven safe; no skipping draws, frame skipping or weakened
robustness; native NDVE off; only these two titles; **no commits**.

**Rules / commands:** `AGENTS.md` (editor tools only, no file-rewriting scripts; `py` with `PYTHONDONTWRITEBYTECODE=1`;
kill by PID, never by image name), protocol §3. Never end a turn waiting on a background notification. Tests:
`cargo test --jobs 4 -p graphics --lib` (+ `-p os` if touched). A device loss stops that path: fix it, never re-run it
unchanged.

**Deadline:** 2 hours wall clock (build time excluded only if a first title build exceeds 30 minutes; say so). Park a
blocker after ~90 min; no early hand-off.

**Files expected:** `src/graphics/src/nvn/vpool.rs`, `nvn/pool.rs`, `vk/sparse.rs`, the texture-view fallback code,
`scripts/scenarios.toml`, this file for the report.

## Report (agent, protocol §5)

**Verified now (engine of 2026-11-25 04:00 on GPU 1, with MKW-PERF3's uncommitted exec.rs/mem.rs and the lead's
device.rs in the same build):**

| Title | Run | Gameplay FPS (target) | p95 / p99 / max ms | >50 ms frames | Memory | Red lines | Audio |
|---|---|---|---|---|---|---|---|
| Shadow (p0) | `sxsg-final`, alone | 29.99 (30); window 5% low 29.5 | 34.4 / 35.1 / 38.7 | 0 | peak 6,959 MB, flat 6,947 -> 6,948 | **1** (blacklist notice) | 0 underruns after 182 s |
| baseline | doc 34 | 29.99 | 36.4 p95 | – | 7,220 MB peak | 218 lines / 4 issues | 0 gameplay |
| Sonic (p1) | `sonic-final`, **overlapped MKW** from 4:05:59 | 31.50 (30) | 32.2 / 32.5 / 44.8 | 0 | peak 6,213 MB | 1 (blacklist notice) | 0 underruns |
| Smash match | `smash-prewarm`, alone | 58.15 (60); window 58.3, 5% low 47.7 | 17.9 / 21.5 / 271.2 | 18 | 6,308 -> 6,459 MB, peak 6,469 | **0** | 17 underruns over the run |
| baseline | doc 34 | 58.39 | 23.7 / 107.8 / 516.9 | ~53 | 6,146 -> 6,912 -> 6,544 MB | VP9 decode error | 2 gameplay |

Sheets looked at: Shadow in Space Colony ARK with 9 rings collected and a moved camera between shots; Sonic in Green
Hill with 14 rings; Smash Mario vs CPU Samus with damage and positions changing across the match. Device line
`[vk] device 1 NVIDIA GeForce RTX 5090` in all three (both Shadow programs).

**Built**

1. `SXSG-B01`, the unaligned `nvnMemoryPoolMapVirtual` requests: **a guest bug; no change to mapping semantics owed,
   the diagnostic was wrong.** Diagnosis from the game's own code (decompressed `main`, generated C): the proc table
   stores `nvnMemoryPoolMapVirtual` at GOT+0x470; its users are the texture streamer's map path (main
   `0x70017742d0` -> `f_7001767c30`), which passes `tile_size * cumulative_tiles[k-1]` bytes, and its inlined
   unmap at `0x7001774130`, which builds `{phys NULL, poff 0, voff = cumulative_tiles[k-1], size = tiles * tile_size}`
   — the offset is never multiplied by the tile size. The 215 log lines fit exactly: e.g. pool 0x160000 = mips of
   16+4+1+1 tiles, unmaps at 0x10, 0x14, 0x15; every one had physical offset 0 (an unmap). The `MapReq` layout and
   stride are right (the map path's offsets are in bytes). Correction (`nvn/vpool.rs`): such an unmap (`unscaled_unmap`:
   NULL pool, poff 0, non-page offset below 64 KB, whole pages) is still refused, mapping unchanged, as NVN's
   alignment rule and the MIT reference say, but logged once per pool as the guest's tile-count unmap instead of a
   red line per request. Also, `remap` of sparse images now runs only for requests that were applied (it ran for
   refused ones too).
2. Texture 519 / serial 7549, the 2D placeholder sampled as cube **and 3D**: the cube twin covered the cube case
   (log: `sampled as a cube: a six-face twin`); the 3D case was still red + zero view (the 2026-11-25 00:29 run had
   `texture ID 365: target 1 bound where the shader samples target 2`). Correction (`vk/image.rs`, and two lines in
   `vk/exec.rs` `cube_twin_target`): a one-layer 2D colour texture sampled as 3D gets a one-slice 3D twin (depth 1,
   every level, copied when the source changes, like the cube twin; 3D support checked per format). Run log now:
   `texture ser7549 ... sampled as a 3D texture: a one-slice 3D twin of its one layer`, and no target-mismatch line.
3. Smash stutters: **real, not the harness's screenshots.** The frames CSV of the uncorrected run
   (`smash-gpu1`) shows every >50 ms match frame with `pipelines_made` 1-36 and `pipeline_ms` ~50 ms each (up to
   881 ms). Cause: prewarm built 258 of 1,587 keyed pipelines and stopped "at the budget": its 2 GB budget measured
   the whole process's private-memory growth while the game boots, and +2,039 MB in 3 s was mostly the guest's own
   commits. Correction (`vk/pipe.rs` `engine_private_bytes`): the budget measures private memory less the guest's
   committed heap and physical maps (`nxrt` kernel space, the counter the os heartbeat reports). Next run: 874 of
   1,712 built; in the same match window (130-192 s) 45.84 -> 58.15 FPS, >50 ms frames 117 -> 18, max 829.8 -> 271.2
   ms (both runs alone on GPU 1).
4. Smash memory: no leak in this scope. 6,308 -> 6,459 MB over the match, peak 6,469 MB (baseline dropped back after
   results too); the shape is the match's stage set loading, not a slope.
5. Blacklist: the one `pipeline-crashed.bin` entry stays (no pipeline proven safe). Shadow route
   (`scripts/scenarios.toml`): the engine now reaches Shadow's selection screen before 40 s, where the old first A
   started Sonic (run `sxsg-fix` went to Green Hill); two Rights now pick Shadow in either state.

**Tests** (`cargo test -p graphics --lib`, 193 passed, 0 failed)

- `nvn::vpool::tests::an_unmap_at_a_tile_count_is_refused_and_changes_nothing`: the Shadow request shape is
  classified, refused, leaves translation and generation unchanged, stops the batch, while the aligned unmap of the
  same tile applies. It pins behaviour the old code also had (refusal), so it did not fail before beyond not compiling
  without `unscaled_unmap`.
- `vk::image::volume_twin_tests::a_one_layer_2d_colour_image_of_any_shape_gets_a_3d_twin` and the extended
  `cube_twin_target` test (2D sampled as 3D gets a twin; 3D as 3D does not).
- The prewarm budget has no unit test (needs a live kernel and device); the game run is its check.
- An earlier full run had one failure in MKW-PERF3's in-progress `set_patch_tests::host_copies_are_kept_per_module_newest_first`;
  it passed alone and in the later full run.

**Verified** — `scripts/build/build.ps1 -SkipGnu -KeepCache`, `nxrs.exe --select-title ... --build` for both titles
(seconds; no translation changes), then `py scripts/play.py shadow|sonic|smash --env NXGFX_GPU=1` (smash with
`--diag pipeline`). Evidence: `build/play/20261125-035944-sxsg-final`, `-040448-sonic-final`,
`-035528-smash-prewarm` (corrected), `-034948-smash-gpu1` (before the prewarm correction), `-033242-sxsg-fix2`
(GPU 0, first Shadow run with items 1-2: 0 unaligned lines, 3D twin of ser7549).

**Unproven**

- What NVN's own driver does with the unaligned unmap (refuse vs round down to tile 0). Both leave mips k>=1 mapped,
  which is harmless when the streamer unmaps mip 0 in the same sweep; the engine keeps the reference's refusal.
- Smash prewarm still stops at the budget (874 of 1,712, +2,285 MB not counted as guest memory), so 18 mid-match
  compile hitches remain. Part of the improvement is a warmer cache: the key file and pipeline cache grew with the
  previous run. Next: measure what the 874 pipelines really cost versus other engine memory, and then raise or
  scale the budget, or prewarm the match's keys first.
- Smash audio: 17 underruns over the run, against 2 gameplay underruns in the baseline; not investigated.
- No repeated-match memory slope for Smash.

**Slips**

- My Shadow title build at about 3:43:55 overlapped the start of MKW-PERF3's measured `mkw-perf3-final` run (3:43:57).
- Edited `vk/exec.rs` (MKW-PERF3's file): only `cube_twin_target` and its test (T3D added).
- Earlier runs (`sxsg-fix`, `sxsg-fix2`) ran on GPU 0 before the device-index change; `sxsg-fix2`'s last ~75 s
  overlapped an MKW run, so none of its numbers are quoted above.

**Questions:** none.
