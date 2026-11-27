# MKW-SHRUB — the near-black roadside shrubs, and MKW's stale pipeline crash list

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~11:20, worktree
`D:\Projects\RecompLauncherEngine-mkwshrub`, branch `mkwshrub` (base `gameplay-harness` `bb49dc0`: MKW-RENDER merged).
Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0. Deadline 3 h (~14:20).

**Outcome in one line:** the crash list is gone (0 skipped draws, clean exits); **the shrubs are still near-black**.
Their darkness is traced to one input — the shrub material's entry in the game's own texture-atlas table says "not
in the atlas" (scale 0, level 11, layer 0), so every shrub pixel samples the atlas arrays' last level at (0, 0) and
gets ~0 direct light and ~0 SH lighting — but why the game never places that material in the atlas is not found.

## Plan (written before code)

1. **Baseline** on the base build (`bb49dc0` relinked from this worktree): `mkw-pop --faults` with a raw pass dump
   in the race (`NXGFX_DUMP_AT_S`, `NXGFX_DUMP_RAW=1`); read the sheets and the log's sparse-image lines.
2. **Find which input is dark**: a diagnostic copy of the pixel shader (scratchpad, swapped into the title's shader
   folder for one run, original restored and hash-checked) writes chosen intermediate values to its colour output,
   read back raw from the pass dump. The dark factor names the input; then the engine path that feeds it.
3. **Fix at source**, with a test of the rule.
4. **Crash list**: back up MKW's `pipeline-crashed.bin`, remove its three entries, run `mkw-pop` in full.
5. **Verify**: `mkw-pop` and `mkw` after; shrub shots before/after; route checks; stand-ins; FPS against the baseline;
   `cargo test --release -p graphics`.

## 1. The crash list (done)

`pipeline-crashed.bin` of MKW (237 bytes, 2026-11-24, SHA-256 `fe2aa577…badf45`) held 3 entries; the engine's format is
a run of `Key::Gfx` records with nothing else in the file, and the engine has no tool to edit it, so removing the three
entries is removing the file (the engine treats a missing file as an empty list and writes one again only after a real
compiler crash, `pipe.rs` `load_crashed`). Backups: `<data>/pipeline-crashed.bin.20261127-mkwshrub-bak` (same hash)
and a copy in the wave's scratchpad. The older `pipeline-crashed.bin.20261124-bak` (5 entries) is untouched.

Every run since (`sh-trace-noblk`, `sh-v1-pop`, the final runs, §5): **0 draws skipped for "no pipeline"** (base:
18 a frame, 282,600 in the run), no "crashed the driver's compiler" red line, **normal exit**, and no new
`pipeline-crashed.bin` written. The three pipelines compile cleanly on this machine (as MKW-RENDER §5 found).

The two diagnostic builds in between (the copy trace, which took the images' lock inside the copy; the tail-sharing
change) had the gate's give-ups / re-translations at 2,328-4,526 a run; the final build (no trace, crash list still
removed) is back at **169** (base 167, MKW-RENDER 157-206), so the 18 draws being drawn do not cost the lanes.

## 2. What the shrubs read (diagnosis)

The shrubs are drawn by pixel shader `cb0bfd018d46e25c` (`DriveRefreshUber…var1815`), passes 145-149 of the colour
pass after the depth prepass. Its colour is `T·lit + inscatter` with the direct term multiplied by `tcb_8.x · shadow`
and the indirect term built from SH coefficients read from `tcb_8.yz`, `tcb_A.xyz` and `tcb_C.x`. Those three are the
8192x8192x68 sparse arrays (`ser33391` ASTC 8x8 sRGB shown as BC7, `ser33393` BC6H, `ser33395` ASTC as BC7). Each
is sampled at `(uv · rect.scale + rect.offset) · cell.scale`, layer `cell.layer`, LOD `max(cell.minLod, query)`, where
`rect` is the material's record in a storage buffer (96 bytes, `fp_s0`) and `cell` an entry of a table in constant
bank 9 (`c9 + 25904 + 64·record[20]`).

Diagnostic shaders (three-way 16-pixel stripes, raw `B10G11R11` readback of the pass; scratchpad
`instr.py` / `instr4.py`, runs `sh-instr1`..`sh-instr4`):

| value (all shrub pixels) | result |
|---|---|
| `tcb_8.x` (direct-light factor) | **0.0286** everywhere (p5 = p50 = p95) |
| screen-space shadow mask | 1.0 (lit) |
| fog transmittance (`tcb_1C.w`) | 0.97 |
| BC6H SH (`tcb_A.xyz`) | 0 on almost every pixel |
| `tcb_8.yz`, lit sum | 0, 0.04 |
| material rect (`record[0..3]`) | (0.39, 0.079, 0.0033, 0.67): one material record for every shrub |
| `cell.scale` (both), `cell.minLod`, `cell.layer` | **0, 0, 11, 0** → sample coordinates (0, 0), level 11, layer 0 |
| probe: `tcb_8` at (0.5, 0.5) layer 0, LOD 11 / 6 / 4 | 0 / 0 / (1.0, 0.0003, 1.0) (ASTC error magenta) |

The constant bank as the **guest memory** holds it (new: `NXGFX_DUMP_RAW=1` also writes each dumped stage's constant
banks, `cbuf-<addr>-<size>.bin`) says the same: of the table's 274 entries only 21 are filled (`130-133, 146-149,
162-165, 178-181` with min LOD 3, and `256-263`); entry 112 — the shrub material's `record[20]` — is
`scale 0, minLod 11, layer 0`. So the engine passes the game's values through faithfully; the game has not placed
the shrub material in its atlas, and the fallback (layer 0, last level) holds nothing usable.

What fills the atlas (log of every copy into a sparse texture, a temporary trace since taken out): at boot
(12-17 s) the game copies a default 8192x8192 ASTC (and BC6H) texture into every layer and every level 0-11 of the
first set of arrays (`ser33398/33401/33404`) and places other materials' small levels (e.g. a 773x1020 texture's
level 7 into layer 4 level 10) into the sampled set (`ser33391/33393/33395`). **After 17 s no copy into these arrays
happens for the rest of the run**, through the menus, the course load and the race: the course's materials (the
shrub's among them) are never put into the atlas.

Ruled out on the way (each by a run, not by reading):

- Not the copies or their source: the source's storage is filled (1,398,104 of 1,398,144 sampled 16-byte words not
  zero), not GPU-written, and every copy is a same-format `vkCmdCopyImage` of whole levels.
- Not lazy write-back of GPU results to the CPU (a feedback buffer the streamer reads late or never):
  `NXGFX_WB_LAZY=0` (`sh-x-wblazy0`) — shrubs unchanged.
- Not the 18 skipped draws (§1): shrubs unchanged with them drawn.
- Not the sparse arrays' mip tails being private to one image of a pair. The storage has one page a layer past every
  layer's tiles (BC7 arrays 23,256 pages = 68·341 + 68; BC6H 92,888 = 68·1365 + 68; the D16 shadow array
  1,111 = 101·10 + 101), the BC6H pools are mapped to their end before the images exist, and the engine gives each
  image's levels outside the tiles memory of its own. A change that bound those levels to slots of the physical page
  mapped at each layer's tail page (shared between the two arrays of a pair as tiles are) was built and run
  (`sh-v1-pop`): **shrubs unchanged**, so it was taken out (protocol §4).
- Not the depth test, the shadow mask, the 165x165 mask texture (MKW-RENDER §4).

## 3. Built

- `exec.rs` (pass dump): with `NXGFX_DUMP_RAW=1`, each constant bank a dumped stage binds is written once per address
  as the guest memory holds it (`cbuf-<addr>-<size>.bin` in the dump folder). This is how the table entry above was
  read. `docs/env_switches.tsv` says so.
- `sparse.rs`: one more log line per sparse image — its texture serial, storage size in pages against layers x
  pages, level count and the Vulkan mip tail (first level, size, stride) — so a pass dump's `serN` can be tied to its
  sparse image and the tail page layout is visible.
- No engine behaviour changed. The tail-sharing change and the copy trace were taken out.

## 4. Pictures (before / after)

| what | before (base `bb49dc0`, crash list in place) | after (final, crash list removed) |
|---|---|---|
| side by side (different spots: game time drifts between runs) | `build/play/20261127-123401-sh-final-pop/shrub-before-after.png` (left: base `27-pop`, right: final `26-pop`) | |
| near shrub, bridge exit / desert curve | `build/play/20261127-112648-sh-base-pop/27-pop.png` | `build/play/20261127-123401-sh-final-pop/26-pop.png`, `28-pop.png` |
| race, 0.5 s apart | `20261127-112648-sh-base-pop/sheet-2..8.png` | `20261127-123401-sh-final-pop/sheet-2..9.png` |
| pass 146 (shrub draw) | `20261127-112648-sh-base-pop/shrub/base-146-view.png`, `base-145-146.png` (pixels the draw changed: near shrub median 0.017 linear, far shrubs 0.27) | - |
| diagnostic stripes (far shrubs) | `20261127-112648-sh-base-pop/shrub/i2-145.png` | - |

Kept beside the base run in `shrub/`: the scratch tools (`tools/instr.py`, `instr4.py`: SPIR-V text patches of the
shader's colour stores; `r11load.py`, `stripes.py`: raw `B10G11R11` readback statistics) and the constant bank
`cbuf-226033a00-a9c0.bin` (the atlas table at +25904).

**What still looks wrong** (literal): every roadside shrub in every race shot is a near-black silhouette (the leaf
texture is visible only when brightened); this run's race shots look the same as the base run's apart from the kart's
position. The kart drives into the sand near the end of `mkw-pop` (the scripted input).

## 5. Measured (GPU 0; other waves' builds ran on the machine during these runs)

| run (`build/play/`) | build | crash list | scenario | FPS / 5 % low (window) | skipped draws (run) | gate (run) | route | exit |
|---|---|---|---|---|---|---|---|---|
| `20261127-112648-sh-base-pop` | base `bb49dc0` | in place | `mkw-pop --faults` + pass dump at 262 s | 37.7 / 0.0 (the 12 s dump stall is in the window) | 282,600 (18 a frame) | 167 | 2/2 on | normal |
| `20261127-113336-sh-instr1`, `-113920-sh-instr2`, `-114646-sh-instr3`, `-115513-sh-instr4` | base + diagnostic shader | in place | `mkw-pop` to 262-280 s, dumps | (diagnostics) | 18 a frame | - | - | killed at limit |
| `20261127-120343-sh-trace-noblk` | + copy trace | **removed** | `mkw-pop --faults` | 47.2 / 39.6 | **0** | 4,526 | 2/2 on | normal |
| `20261127-121953-sh-x-wblazy0` | + copy trace | removed | `mkw-pop`, `NXGFX_WB_LAZY=0` | (experiment) | 0 | - | - | killed at limit |
| `20261127-122603-sh-v1-pop` | + tail sharing (taken out) | removed | `mkw-pop --faults` | 53.5 / 45.4 | 0 | 2,328 | 2/2 on | normal |
| `20261127-123401-sh-final-pop` | `808d286` | removed | `mkw-pop --faults` | **53.9 / 47.0** | **0** | **169** | 2/2 on | normal |
| `20261127-123920-sh-final-mkw` | `808d286` | removed | `mkw` | (window closed from outside at 91 s: `window: closed by the user or the system`) | 0 | 92 | - | normal (exit code 0) |
| `20261127-124113-sh-final-mkw2` | `808d286` | removed | `mkw` | invalid: **OFF-ROUTE** (the menu presses landed in Free Roam, then a balloon battle; menu timing, not a fault) | 0 | 295 | 0/3 | normal |
| `20261127-124710-sh-final-mkw3` | `808d286` | removed | `mkw` | **59.2 / 55.0** [250, 325] | **0** | 164 | **3/3 on** | normal |

Against the baselines: `mkw` 59.2 / 55.0 against MKW-RENDER's `rd-final-mkw` 57.4 / 48.3 on the same engine code (this
wave's own base `mkw-pop` window is invalid because of its pass dump); `mkw-pop` 53.9 / 47.0 against MKW-RENDER's
`rd-v3-pop` / `rd-final-pop` 53.2 / 45.0 and 53.0 / 44.0. Not worse. Stand-ins: 1 per run (the known boot one,
`texture ID 0 … nothing is registered there`), as in the base run. No new red lines, no exit 71 / 72, no device loss.

## 6. What is left

- **Shrubs**: find why the game never streams the course's materials into its atlas after boot. The game's atlas
  table is in constant bank 9 (`c9 + 25904`, 64 bytes an entry: scale xy, …, min LOD at +32, layer at +48); the
  streamer's decision is CPU-side (the table is guest-written). Next: find the game thread that writes the table and
  what it waits on (a GPU readback, an event, a query result, a file read) — `NXRS_DUMP_FRAMES` / a watch on the
  table's address, and the copies of the menus (2 a frame at 103-128 s, into non-sparse textures) for comparison.
- The ASTC error colour at layer 0 level 4 centre of `ser33391` (probe) — the default texture's level 4 decodes to
  magenta there; not followed further (layer 0 is the "not in the atlas" fallback layer).

## Report (agent, protocol §5)

Branch `mkwshrub` (base `bb49dc0`): `d831589` (plan), `808d286` (diagnostics), this commit (doc).

### 1. Built
- **Crash list**: removed (backed up), 0 skipped draws, clean exits, no new list (§1).
- **Diagnosis** (§2): shrub darkness = the shrub material's atlas entry is "absent" in the game's own table; the shader
  samples the arrays' last level of layer 0 (~0). Cause of the missing atlas placement not found.
- **Code**: two diagnostics (raw constant banks in pass dumps; sparse image serial / storage line). No behaviour
  change. Taken out: shared mip tails for sparse image pairs (moved nothing), a sparse copy trace.

### 2. Tests
`cargo test --release -p graphics`: lib 310 passed, 6 ignored. No behaviour was added, so no new test (the
tail-sharing change had two tests; they went with it).

### 3. Verified
Runs and numbers §5; sheets read for every run listed there. Route checks on in every completed `mkw-pop` / `mkw`.
No exit 71 / 72, no device loss.

### 4. Unproven
- Why the game does not stream course materials into its atlas (§6) — the shrubs stay black.

### 5. Slips
- `cargo fmt -p graphics` reformatted the whole crate (it is not fmt-clean); I put every file back with
  `git checkout -- src/graphics` (my own uncommitted edits only; the tree was clean before) and re-applied my two
  small edits by hand. No `cargo fmt` afterwards.
- Two MKW processes overlapped for ~45 s (12:19:53-12:20:40): I started `sh-x-wblazy0` while `sh-trace3` was still
  running (its kill at the time limit came later than I assumed). No crash list or compile marker was written; the
  pipeline cache was written by the first at its exit.
- The pixel shader `cb0bfd018d46e25c.spv` in the title's shader folder was swapped for diagnostic copies for four runs
  (`sh-instr1`..`sh-instr4`) and restored after each; final SHA-256 `30b7dae4…08265e`, identical to the original.
- Scratch Python (scratchpad only) patched SPIR-V text and read raw dumps; none writes repository files.

### 6. Questions
None that the sources do not answer; the crash-list removal followed the brief.
