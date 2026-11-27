# OOT-FIX — Ocarina of Time: washed-out colours (A), then the replay thread under 15.8 ms (B)

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 15:40, worktree
`D:\Projects\RecompLauncherEngine-ootfix`, branch `ootfix` (base `gameplay-harness` `2f1eaf3`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 0. Deadline 5 h (20:40).

## Plan (written first)

Baseline: the lead's run `build/play/20261127-145622-oot` (main checkout; 29.7 / 27.0 over [845, 915], replay busy
16.67 ms, pool sync: landings first 2.9 ms, lock 0.96, scan 1.95, per-pool pass 3.55 ms, 3.1 syncs a frame) and this
wave's first run with a pass dump in the house.

**A. Washed-out colours.** Reference: nxnative's OoT note (`D:\Projects\nxnative\dm\games\0400486024DEC000\README.md`,
"against a frame of console video of Navi's close-up the picture matches") and its finding *Float atomics in storage
memory* (`docs/reference/shaders.md`): lifted blacks, yellow skin, a pale veil = agl's scene analysis
(`probe_analyze` & co., float sums by `RED.F32`) not reaching the CPU, which sets the exposure from them. Steps: check
the output chain (present format, tone map targets and views), then the analysis dispatches, then fix where the
value goes wrong; before/after shots side by side; then whether the white house-floor frame is the same cause.

**B. FPS.** The pool sync (3.1 syncs a frame), the per-pool pass, the skipped draws (`no pipeline` / `no target`).
Each change measured against the baseline's diagnostics; one that moves nothing comes out.

## A. Washed-out colours: cause and fix

### What was compared against

- nxnative's OoT note and `docs/reference/shaders.md` "Float atomics in storage memory": with agl's scene-analysis
  sums at 0 the game "took its full correction, several times too bright": "too bright, yellow skin, a pale veil";
  "the frame's mean brightness at Navi's close-up went from 0.71 to 0.38" once the sums were right.
- Our Deku Tree / Navi shot (`04-house.png`, 263 s) of the lead's baseline: mean luminance **0.684** (Rec. 709 luma
  of the PNG, every 13th pixel) and the pale veil of that description; nxnative's screenshot
  `D:\Projects\nxnative\docs\images\zelda-ocarina-of-time.jpg` and Turbo's
  `D:\Projects\Switch2RECOMP-main\docs\images\screenshots\zelda-ocarina-of-time.jpg` have deep blacks.

### The output chain is not it (checked)

- Present (`vk/present.rs`): the game's picture is `ser4` 2560x1440 RGBA8 **UNORM** (`present: texture ser4 ... format
  0x25`), blitted to a `B8G8R8A8_UNORM` swapchain (sRGB-nonlinear colour space): bytes pass through, no second encode.
  The harness's screenshots are the game's texture itself (before the sharp upscale), and they are washed out too.
- Pass dump at 405 s (`build/dumps/base/`, run `20261127-155722-of-base`): the last pass (`pass-016789-143-...-R8G8B8A8_UNORM.png`)
  is one sRGB encode of the scene before it (`pass-016789-142-...-B10G11R11_UFLOAT_PACK32.png`): mean |sRGB(a) - b| =
  0.013 of 255-scale against 0.172 for identity, over the whole frame. One encode, as the console does. The tone map
  itself is right; the scene values going into it are too bright.

### The cause: light-probe data in pools that were never in the raw global memory table

1. The analysis dispatch `6e28359aa3558f61` (agl's scene analysis; 4 float CAS-loop sums into words 0-3 of its storage
   buffer `cp_s3`) wrote **0** to its sums and to its per-tile colour words in every frame: guest memory **and the GPU
   copy** of that buffer read `[0, 0, 0, 0, 0, 0, 0, -10]` (run `20261127-163905-of-e2-peek`, a temporary diagnostic
   that read both, removed again). The write-back was not the problem: the probe pass `1bba09ec4bc8ff67`'s sums in
   words 32-35 of its buffer were non-zero in both (`0.164, 0.108, 0.038, 0.117`).
2. `6e28359aa3558f61` reads its inputs through **raw global memory**: 64-bit pointers in its storage buffer `cp_s0`
   (`0x1afc58000`, `0x1afc58040`, `0x1afc58080`, `0x1afc58680`, `0x1b1de8680`) are looked up in the memory pool table
   (`vk/gmem.rs`) and loaded through buffer device addresses. The guest memory there holds the probe data
   (`0x1b1de8680`: `-80, 98, 2, 0, 0.0150, 0.00719, 0.00288, 1.555, ...`), but **no pool with a GPU buffer covered any of
   these addresses** (`20261127-165102-of-e3-peek`, 636 samples over 0-300 s, every one `gpu []`).
3. `gmem::build` lists only pools whose buffer exists ("Pools are listed once their buffer exists"), and a pool gets its
   buffer at its first `mem::resolve`, i.e. when some binding names it. A pool only ever reached through loaded
   pointers was never in the table, so the shader's lookup failed and every load read 0. On the Mac (nxnative) every
   pool is an MTLBuffer and in its table. 2,123 pools of OoT had no buffer (`raw global memory: ... pools no binding
   had named got their buffer` lines, run `of-v2`).

### Fix (`vk/gmem.rs` `bindings`, `vk/mem.rs`)

- **Every GPU-visible pool is in the table** (`NXGFX_GMEM_BACK_ALL`, default on; `0` = as before): when a table is
  made, pools with no buffer get theirs (`Pool::ensure_buffer`, now `pub(super)`), at most `NXGFX_GMEM_BACK_MB`
  (256 MB) of them per table, the rest with the next tables (all 2,123 by 9.7 s in OoT).
- **Granule pools made for the table are bound where needed, not whole**: on a device with
  `residencyNonResidentStrict` (an unbound granule reads 0, which uncommitted guest memory is), they are bound over the
  guest's committed runs at creation, and the pool sync binds the granules of every CPU-written range of such a pool
  before uploading it (`Pool::bind_written`, `gmem::table_only`): nothing else would bind them. Bound whole (as the
  table already did for pools with a buffer, run `of-v2`) the pools' granules were 4.49 GB at the end of the run
  (240 granules = 0.48 GB before the change); bound this way 3.19 GB (`of-v3`). Private memory: v1 (before) 26.1 GB at
  its last memory line, v2 29.9, v3 29.8; report peaks 26.9 (base), 30.6 (v2), 30.7 (v3), 31.4 GB (`of-final`)
  against the 32 GB process cap.
- **Pools over 1 GB stay out of the table** until a binding gives them a buffer, as every pool did before
  (`NXGFX_GMEM_BACK_MAX_MB`, default 1024, `0` no limit; the pools left out are logged once). The pools backed for the
  table (`NXGFX_GMEM_BACK_LOG=1`, run `20261127-180413-of-poolsizes`, 40 s): 3,868 pools, 5,267 MB, 1,343 MB committed at 9 s;
  the largest, `0x1bf700000+2432 MB` (flags 0x24, the game's heap), grows to ~1.4 GB of granules as the game fills it;
  the probe data the analysis reads is in `0x1afc58000+149 MB`. Left out (`20261127-180743-of-max1g`): granules 3.19 ->
  1.42 GB, **peak 31.4 -> 28.5 GB**, the same pictures (`side-max1g-x1.png`: identical but for Navi's place) and draw
  counts (2,003-2,040 a frame at the x1 shot in both). Whether some shader ever reads that heap through a pointer is
  not shown (it was out of the table before this wave too).
- **Table entries carry their span** (word 7): the translator's helper keeps the entry an invocation used last and
  takes it again when `addr_hi == start_hi && addr_lo - start_lo < span`; the engine wrote 0, so every raw access
  searched the table (hint, then a binary search). `gmem::span`: the size when the range lies in one 4 GiB window,
  else 0 (exact lookup).

### Result (pictures)

Final build (`51e4cc5`), run `build/play/20261127-182603-of-final2` (this worktree), against the lead's baseline
`D:\Projects\RecompLauncherEngine\build\play\20261127-145622-oot`. Side by side (left baseline, right final, half
size), in the final run's folder:

- `side-04-house.png`: the Deku Tree scene at 263 s, the same line ("Yes, Great Deku Tree. I'm here!") in both: the
  white veil, the white-yellow bark and the lifted blacks are gone; dark canopy, blue-green haze, Navi's glow,
  fireflies; the black hole beside the tree's mouth in the baseline is gone. Mean luma (Rec. 709, every 13th pixel)
  **0.684 -> 0.194** (nxnative's Navi close-up: 0.71 broken -> 0.38 fixed; not the same frame).
- `side-14-h1.png`: Link in his house (630 s), same camera: lifted, yellow-green floor and walls -> a dark interior lit
  by its lanterns; pots, baskets and herbs on the shelves that were missing or black shapes are drawn and lit. Mean luma
  0.301 -> 0.138.
- `side-07-navi.png` (406 s): Navi waking Link (different moments of the scene).
- `passdump-405s/`: the base run's last two passes (the HDR scene and the RGBA8 output; see "output chain").
- `side-max1g-x1.png`: the 1 GB pool limit (below) changes nothing visible.
- The runs `of-v2`, `of-final` (`7dec77d`) and `of-max1g` show the same.

Every sheet of the final run read (`sheet-1..3.png`): title, opening (Link asleep, the title card), the Deku Tree talk,
the forest load, Navi waking Link, the house and the walk: correct pictures, Link in his house on the route at every
check shot. **The harness marks the run
OFF-ROUTE**: its route references (`scripts/checkpoints.json`, from `20261126-023504-v2-oot`) are washed-out pictures
and the colour-histogram distance is now 0.54-0.61 (> 0.3) at all six check shots. The route is right; the references
need to be taken again from a corrected run (`py scripts/play.py checkpoint <run>`, the lead's call after reading the
sheets). Until then every corrected OoT run reads OFF-ROUTE and its FPS line says "INVALID".

### Is the white house-floor frame (OOT-FLASH "Fault A") the same cause?

**No.** `oot-house --faults` on the final build (`build/play/20261127-174939-of-final-house`, `NXGFX_FAULT_MAX=200`,
`NXGFX_FAULT_PER_REASON=80`): 7 one-frame white flashes from 482 s on, each "100 % of the white area gone again" at the
next present: 482.7 s 32.0 %, 541.1 s 10.5 %, 550.0 s 10.3 %, 553.5 s 31.6 %, 575.4 s 32.3 %, 581.8 s 9.9 %, 647.4 s
6.7 % of the picture (`faults/fault-106..113-*.png`). The pictures are OOT-FLASH's Fault A on the corrected lighting:
`fault-107-p20047.png` (541 s) the floor, the shelves and the grass blown to white with Link, the door and the lanterns
correct; `fault-111-p21073.png` (575 s) everything beyond the nearest geometry white. The rate is OOT-COMBO's (~5 a
minute in the house) and the shape is the same, so the cause fixed here is not its cause. That run's house window [610, 690]: 29.1 / 27.0 FPS
(report.md marks it INVALID: OFF-ROUTE by the stale references), peak 29.9 GB. Again on the final build `51e4cc5`
(`build/play/20261127-184123-of-final2-house`): 3 one-frame white flashes from 400 s on (511.6 s 9.0 %, 605.4 s 7.7 %,
621.8 s 9.9 %, all gone at the next present; `faults/fault-108-p22577.png` the floor, shelves and herbs white with
lightning-like streaks, Link and the door correct), 34 shots all correct, house window 29.1 / 27.0 (INVALID by the
references), peak 27.7 GB. The harness's 34 shots of that
run (sheets 1-3, 594-694 s) are all correct pictures; no displaced-props frame was caught.

## Skipped draws (B, "are frames missing work?")

- **`no pipeline` (2 draws every frame from ~200 s, 41,190 in the lead's run): yes, missing work.** Two of OoT's
  scene pipelines were on the compile crash list `pipeline-crashed.bin` (written 2026-11-27 05:43): `6f6cef0b90392b8c +
  6549feb5cc0d721d` (one RGBA16F target + D32) and `c6cd5ff91721b31d + 038ebabea2624bee` (two RGBA16F targets + D32; its
  pixel shader samples two 3D textures), the 1280x720 half-resolution passes 95-97 of the house frame. The list takes
  whatever was compiling when a session ended abruptly; at 05:26-05:43 OOT-COMBO's runs were stopped at their time
  limit and died at the 32 GB cap, neither a compiler crash. Shown safe: the list moved aside (backup
  `pipeline-crashed.bin.bak` in the wave's scratchpad), run `20261127-162228-of-v1` compiled both and completed (no
  red line, no new list); the file is not restored (it held only these three entries).
- **Cause fixed** (`vk/pipe.rs`): only a session that died of an **unhandled exception** with compiles in flight lists
  them: the process's unhandled-exception filter (`crash_filter`, chained to the previous one) writes
  `pipeline-compiling.crash` beside the in-flight marker; at the next start the in-flight keys are listed only with that
  mark (Windows). A kill by PID or a Rust abort leaves no mark: those keys are compiled again (logged). Linux keeps the
  old rule. The crash-listed pipeline's shaders are now logged when first skipped.
- **`no target` (~0.8 draws a frame)**: pixel shader `e55f45d0df50c572`, which binds only the support buffer and writes
  no output, drawn with no colour or depth target: nothing is lost by skipping it. Not changed.

## B. FPS

### Change: landings first only while the game waits (`queue.rs` `land_first`, `GuestWait`)

`NXGFX_LAND_FIRST` (MKW-HEAVY) made every pool sync on the replay thread wait (up to 1.5 ms) for finished submissions
the completion thread had not landed yet, so a fence the game waits for is not held up behind the sync. In OoT the
game waited 0.00 ms a frame for fences (baseline), and this wait was 0.0-4.6 ms a frame of replay-thread time
(`landings first` in the pools line; 2.913 ms at 884.85 s in the lead's run). Now it waits only while a game thread is
in a fence wait (`nvnSyncWait`'s wait, the window's acquire wait: `queue::GuestWait`); `always` = the old rule, `0` =
off. Measured on the build before the raw-table fix (same frame content as the baseline):

| Run (`build/play/`, this worktree) | build | FPS / 5 % low [845, 915] | replay + prep mean / p50 / p95 (ms) | <= 15.8 ms | pool sync mean | landings first (pools lines in the window) |
|---|---|---|---|---|---|---|
| lead `20261127-145622-oot` (main checkout) | `2f1eaf3` | 29.7 / 27.0 | 18.78 / 18.12 / 28.91 | 32 % | 7.27 | 0.005-4.63 ms, 2.9 at 884.85 s |
| `20261127-155722-of-base` (pass dumps at 405 / 640 s) | `2f1eaf3` + crash-log line | 29.6 / 27.0 | 16.30 / 15.87 / 25.57 | 50 % | 6.24 | 0.014-2.54 ms |
| `20261127-162228-of-v1` | + land-first rule, crash list moved aside | 29.6 / 26.9 | 15.71 / 15.39 / 24.65 | 53 % | 5.69 | 0.00-0.55 ms (one line 2.26) |

(frames CSV: replay = `replay_ms`, prep = `prep_ms`; window [845, 915] s.) The replay thread's sync lost ~0.6 ms a
frame on average; FPS did not move (the pacer needs ~75 % of a window under 15.8 ms to hold 1 vsync). Kept: it moves
the stage it targets, and it is the rule MKW-HEAVY meant (the game waiting for that fence).

### The frame is not the same frame any more (A changed it)

With the raw table complete the game draws what it culled before (objects whose data it reads through the same kind
of pointers): **~880 draws a frame in the walk window before, 2,800-3,170 after** (`replay up to frame` lines),
dispatches 110 -> 150-162, GPU 9.1 -> 10.7-12.1 ms a frame. The 29.7 FPS of the baseline was the speed of an
incomplete, wrongly lit frame. On the complete frame:

| Run | build | FPS / 5 % low [845, 915] (route check: see below) | frame mean | replay + prep mean / p95 | pool sync mean | GPU | draws a frame | private peak |
|---|---|---|---|---|---|---|---|---|
| `20261127-165825-of-v2` | raw table, granule pools bound whole | 20.3 / 12.0 | 48.4 | 24.85 / 39.63 | 10.80 | 11.4 | 3,166 | **30.6 GB** |
| `20261127-171750-of-v3` | + granules where committed / CPU-written (with a diagnostic thread) | 28.1 / 22.6 | 35.4 | 20.94 / 32.63 | 10.42 | 10.7 | 2,800-3,060 | 30.7 GB |
| `20261127-173415-of-final` | `7dec77d` (no pool size limit) | 28.4 / 26.0 | 35.1 | 21.50 / 33.90 | 10.79 | 10.2 | 990-2,310 | 31.4 GB |
| `20261127-180743-of-max1g` | `7dec77d`, `NXGFX_GMEM_BACK_MAX_MB=1024` | 29.0 / 24.6 | - | 20.01 / 31.97 | - | - | 2,003-2,040 at x1 | 28.5 GB |
| **`20261127-182603-of-final2`** | **final `51e4cc5` (limit 1024 by default)** | **27.7 / 19.3** | 35.7 | 21.48 / 33.56 | 10.65 | 10.1 | 830-3,180 | **28.6 GB** |

All runs from v2 on are marked INVALID by report.md (OFF-ROUTE against the stale references, see A); the sheets show
the route. Final against the lead's baseline (report.md, [845, 915]): **29.7 / 27.0 -> 27.7 / 19.3 FPS** (replay + prep 18.78 ->
21.48 ms mean, pool sync 7.27 -> 10.65 ms, frames CSV) on a frame with up to ~3x the draws (830-3,180 a frame in the
window against ~860-1,240) and correct exposure. Target 59 not reached; the replay thread is further from 15.8 ms than
before because the frame now holds its real work. (Why v2 read 20.3 FPS and v3 / final 28.1-28.4 is not established:
v2 bound granule pools whole and its frames in the window carried more draws (3,166 a frame at 862 s against
2,800-3,060 in v3 and 990-2,310 in `of-final`); the walk's camera takes in the cluttered corners at slightly different
moments from run to run.)

Where the replay thread's time goes now (v3, pools lines over [845, 915]): **landings first 2.7-4.0 ms and pool-lock
wait 2.5-4.9 ms a frame in the heavy stretches** (the game now waits 16-18 ms a frame for its fences, so the
landings-first rule fires as designed), write-watch scan ~2.1 ms, per-pool pass 3.4-4.6 ms (copy and hash 2.7-3.4).
Behind the first two is the **completion thread**: "re-arming 5.8-7.6 ms before the pool lock" a frame
(`upskip::landing_pre`: every written-back page compared with its baseline and hashed, ~59 MB a frame, 190-200
write-backs a frame, most under 4 MB and so done on the completion thread alone). That, not the replay thread's own
work, is what the game's frame now waits for.

### Why 3 syncs a frame, can it be one

3.0-3.1 scanning syncs a frame on replays, ~1.0-1.7 of them covered by an earlier scan (cheap), 7 parked streams run
on without a sync (park chain), the present lean (no sync). The two that scan are the first submission of the frame
and a later one after the game wrote more (CPU writes between submissions that the later GPU work reads: the game
rewrites ~55 MB a frame of the same bytes, "upload skip: 54.8 MB of CPU-written pages not uploaded"). Making them one
would mean uploading after the GPU work that reads them, i.e. stale data: not attempted. The cost inside them is the
compare-and-hash of those 55 MB (`copy and hash` 2.4-3.4 ms) and the write-watch scan (~2.1 ms).

## What is still wrong

- **Memory**: with the 1 GB limit the peak is 28.6 GB (`of-final2`; 28.5 in `of-max1g`) against the 32 GB cap, ~2 GB above the
  26.9 of this wave's baseline: the pools mirrored for the table (~1.4 GB of granules, ~0.55 GB of small pool buffers;
  private memory counts the device allocations). Next: import such pools from host memory (`Placement::Host`,
  VK_EXT_external_memory_host) instead of mirroring them (no device copy), or find which of the ~3,800 pools the
  shaders' pointers really reach. The pools over 1 GB are out of the table as before; if a shader reads one through a
  pointer it still reads 0 (not seen in OoT's opening and house).
- **FPS: 27.7 / 19.3 (final), target 59.** The replay thread is at 21.5 ms (replay + prep, mean) on the complete frame; the
  game also waits 16-18 ms a frame for fences in heavy stretches. The next cut is the completion thread's landing work
  (~6-7.6 ms a frame of `landing_pre` compares and hashes of ~59 MB of write-backs, most under the 4 MB that splits one
  over threads), which the replay thread's syncs wait for (landings first, pool lock) and the game's fence waits stand
  behind; then the 55 MB a frame of same-byte CPU rewrites the syncs compare (`copy and hash` ~3 ms).
- **Route references**: `scripts/checkpoints.json`'s OoT shots are the washed-out pictures; every corrected run is
  OFF-ROUTE until they are taken again.
- The span (word 7) of the raw table was not measured on its own (it went in with the table fix; GPU time moved with
  the larger frame).
- `no target` skips (~0.8 a frame) are an empty pixel shader's draws with no target: harmless, not changed.

## Report (agent, protocol §5)

Branch `ootfix` (base `2f1eaf3`): `4fd9809` (crash list, plan), `7dec77d` (raw table, span, landings-first rule),
`51e4cc5` (1 GB limit, pool log), docs.

### 1. Built

- **A, diagnosis**: not the output chain (one sRGB encode, UNORM swapchain, checked by pass dump). agl's scene analysis
  (`6e28359aa3558f61`) read its light-probe inputs through loaded pointers into pools that had no GPU buffer and so were
  not in the raw global memory table: every load read 0, its sums (exposure) were 0, the picture several times too
  bright under a pale veil, and objects culled through the same data were missing (~880 draws a frame against
  2,000-3,000 after).
- **A, correction**: every GPU-visible pool up to 1 GB is in the table (buffer made for it; granule pools bound where
  committed and where the CPU writes); entries carry their span (`gmem.rs`, `mem.rs`). Switches `NXGFX_GMEM_BACK_ALL`,
  `NXGFX_GMEM_BACK_MB`, `NXGFX_GMEM_BACK_MAX_MB`, diagnostic `NXGFX_GMEM_BACK_LOG`.
- **Skipped draws**: two scene pipelines were on the compile crash list from sessions that were killed or aborted, not
  crashed; the list now takes only sessions that died of an unhandled exception (`pipe.rs`); the three false entries
  shown safe and removed (the file held only them).
- **B**: landings first only while a game thread waits for a fence (`queue.rs`, `sync.rs`, `window.rs`): replay +
  prep 16.30 -> 15.71 ms mean on the old frame, FPS unchanged. The 3 syncs a frame analysed (they cannot be one).
- **Fault A**: not this cause (7 one-frame white flashes in the house on the final build).

### 2. Tests (`cargo test --release -p graphics`: lib 318 passed, 6 ignored; other targets pass)

- `pipe::compile_journal_tests::an_end_without_a_crash_lists_nothing` (new): a journal left by a kill (no crash mark)
  lists nothing and keeps the list as it was. Failed before the change (the key was listed).
- `pipe::compile_journal_tests::dying_with_nothing_compiling_leaves_no_mark` (new): the crash mark is written only with
  compiles in flight.
- `pipe::compile_journal_tests::abrupt_exit_still_promotes_whole_and_part_markers` and
  `clean_exit_before_any_compile_preserves_unconsumed_previous_crash` (changed): a death with the crash mark still lists
  whole and part keys.
- `gmem::tests::entries_carry_their_span_within_one_4gib_window` (new): word 7 is the size in one 4 GiB window, 0 across
  a boundary, held to u32. Failed before (word 7 was 0).
- The table backing, granule binding and land-first rule need a device: shown by the runs (sums non-zero on GPU and in
  guest memory, pictures, `pools no binding had named got their buffer` lines, `landings first` figures).

### 3. Verified

Section A (pictures, side by side in `build/dumps/`), "Skipped draws", section B (tables). Runs in this worktree's
`build/play/`. Every sheet of `of-final`, `of-final-house`, `of-v2` (1-2) and `of-max1g` (2) read: correct pictures,
Link in his house on the route; report.md marks them OFF-ROUTE only because the route references are washed-out
pictures. Fault captures of `of-final-house` read: the known nightmare / forest-load ones and the 7 white flashes
(Fault A). Final build `51e4cc5`: `of-final2` (`oot`, sheets 1-3 read, 27.7 / 19.3, peak 28.6 GB) and
`of-final2-house` (`oot-house --faults`, sheets 1-3 and the house flashes read, 29.1 / 27.0, peak 27.7 GB); both
completed, normal exit, one red-line kind (the known no-target draw), no `no pipeline` skips, no crash list written.

### 4. Unproven

- FPS on a quiet machine with valid route checks: needs new OoT route references (`play.py checkpoint` on a corrected
  run, after reading its sheets).
- Whether a shader reads the 1 GB+ pools through pointers (left out of the table, as before).
- The span's own effect on GPU time (not measured apart).
- Fault A's cause.
- The land-first rule on MKW (not run: OoT only): MKW-HEAVY's case is the game waiting for the fence, which the rule
  keeps.

### 5. Slips

- A temporary diagnostic (`NXGFX_PEEK`: a thread logging guest and GPU bytes of given addresses, `mem::gpu_peek`) was
  in the builds of runs `of-v1` to `of-v3` and removed before `7dec77d`; never committed.
- The crash list `pipeline-crashed.bin` of the library's OoT data folder was moved aside for `of-v1` (backup in the
  wave's scratchpad, `pipeline-crashed.bin.bak`) and not restored: its three entries were shown false (above).
- One `taskkill //PID` was sent with a wrong PID argument ("Legend", from a column mix-up) and failed harmlessly; the
  e3 game had already ended at its time limit.
- One `sed -i` edited this document (`docs/waves/OOT-FIX.md`: the GPU cell of the final run's table row) instead of the
  editor tool; the lead may check that line's diff.
- Engine builds and test compiles ran during the boot of some runs (never inside a measured window except the
  pool-size check, a 40 s run).

### 6. Questions

None.
