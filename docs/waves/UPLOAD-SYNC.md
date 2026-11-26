# UPLOAD-SYNC — guest writes to the GPU: correct upload skip, GPU-written pages, pool sync cost, L6

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~00:55, branch `gameplay-harness` (HEAD `2a2f4f9`,
MKW-RECORD merged as `16e1592`). Protocol `docs/31-DELEGATION-PROTOCOL.md`, shared rules
`docs/waves/PARALLEL-RULES.md`. Titles: `mkw`, `mkw-turn`, `oot`; regressions `dkb`, `smash`; GPU 1. Deadline 6 h.

## Plan (agent, before coding)

**Baseline first.** Relink MKW, OoT, DKB, Smash from today's `dist` (HEAD) under `dist.lock`, then paired "before"
runs under `machine.lock`, one title per lock hold. Numbers: FPS mean / 5% low in each scenario's window, per-frame
pool sync parts from the `[vk] pools:` line (lock wait, write-watch scan, per-pool pass, upload MB, completion lock).

**What the code reading found so far (before any run):**

- **`Sub::upload` hashes every uploaded byte.** It goes through `stage_bytes`, which memoises staged bytes by a
  128-bit FNV hash (two serial multiply chains over 8-byte words) plus a map insert. OoT uploads ~56 MB a frame:
  the hash alone is several ms of the 17 ms per-pool pass. A pool upload never repeats bytes within a submission
  usefully; it should stage fresh (`stage_bytes_fresh`).
- **OoT's per-pool pass** (17-18 ms of the ~24 ms sync) is the upload itself (hash + copy into staging of ~56 MB);
  98% of those bytes equal the page's last upload (OOT-FOREST2). The skip (`upskip`) is off by default after its audit.
- **MKW's prep** is mostly the write-watch scan (~6.5 ms) and the lock wait (~1.1 ms); uploads are ~3 MB.

**Problems and the intended fixes:**

1. *Upload skip correctness* (`upskip.rs`, `gmem.rs`, `mem.rs`):
   - H1: the gmem write log forgets hashes at completion (in the log callback, every logged range regardless of
     `WB_MAX`/identity), and every pool a log ever named is never skipped again (sticky set).
   - H2: `forget` walks all pools that can reach the range (bounded by the largest pool kept), not stopping at the
     first one that ends below it.
   - M1: hash exactly the bytes uploaded: each page is copied once out of guest memory into the run being staged,
     hashed there, and uploaded from that copy (one read of guest memory).
   - M2: the budget is charged for what is uploaded after the skip, not before.
   - Then measure default on vs off.
2. *GPU-written pages re-sent* (`upskip.rs`, `mem.rs` `note_gpu_write_version` / `apply_writebacks`): a GPU-write
   note stamps the page with the submission that wrote it and keeps the hash it had; at that submission's landing a
   page still stamped by it gets its hash back: the kept one where the GPU left the page unchanged (`src == before`),
   else the hash of the GPU's bytes. Any later GPU-copy change (another note, an upload, a forget) restamps it, so
   only the last writer re-arms. Lazy (held) pages are re-armed the same way.
3. *Pool sync time*: drop the upload memo hash; skip identical pages (item 1-2); then profile what remains with
   `NXGFX_UPLOAD_TRACE=1` and the pool line, and cut where the time goes (scan, lock waits, copies).
4. *L6*: `lazy::bring_back` resets the write watch after its copy without `WATCH_MU`. Taking the lock there can
   deadlock (fault on a thread holding it). Intended scheme: **no reset at all** — the copy's store then reads as a
   CPU write of the bytes the device already holds: one redundant upload, skipped by `upskip` when the page's hash
   is armed. That also closes a window where a game store between the copy and the reset lost its watch bit.
5. *Missed / late CPU writes* (owner: Switch 2 textures pop): audit list in this doc's report; candidates already
   seen: the reset above; uploads held behind in-flight write-backs (`WB_DEFERRED`, never logged — add to the pool
   line and measure); `sync_range` uploads pages without telling the texture check.

**Files expected:** `vk/mem.rs`, `vk/upskip.rs`, `vk/lazy.rs`, `vk/gmem.rs`, `vk/queue.rs` (upload staging, landing
hand-off), `vk/uptrace.rs`, `docs/env_switches.tsv`.

**Added while waiting for the baseline (MKW's ~6.4 ms write-watch scan a frame):** the walker's walks for syncs leave
the cold pools out, as a mid-frame scan does (MKW's full walks took ~10 ms, ~3 a frame, so ops queued during one
waited for the next and scanned for themselves). Such a walk counts only as a mid-frame scan (never a frame's first
sync, never a present's), covers nothing once a cold pool was found written since it started, and stops early when a
present asks for its full walk. `NXGFX_WALKER_WARM=0` off.

**Baseline finding (OoT on HEAD, forest at ~400 s):** per-pool pass **51.9 ms a frame** for 56.7 MB uploaded, and the
completion thread waits 40.7 ms a frame for the pool lock behind it. The written pages are scattered (one-page
ranges), so this is thousands of `Sub::upload` calls a frame, each hashing its bytes into the staging memo and inserting
them, then a copy command (inferred from the code; the after-run's numbers check it).

## Report (agent, protocol §5; 2026-11-26 00:55 - ~06:00)

**Outcome.** OoT's pool sync 26.2 -> 14.3 ms a frame and its window **14.1 / 7.0 -> 18.8 / 13.0 FPS** (v5; v2-v4
18.6-18.9 / 14-16; target 59): the
upload skip is correct (H1, H2, M1, M2 fixed), on by default, and now also covers GPU-written pages (re-armed at their
landing: OoT's forest uploads 56.7 -> 1.2 MB a frame). MKW's prep 10.7 -> 8.7 ms, FPS at its 30 cap (v5 30.4 / 29.5); DKB and
Smash unchanged. L6 solved without a lock. Two missed / late CPU-write paths fixed (`bring_back` reset window,
`sync_range` not telling the texture check). Not reached: 60 FPS in MKW or OoT; MKW's ~4 ms scan.

### 1. Built (diagnosis kept apart from correction)

**Problem 3 first, because it was the biggest: where OoT's pool sync went.** Baseline `014812-base-oot`, forest
(~400 s): per-pool pass 51.9 ms a frame for 56.7 MB uploaded; in the measured house window 18.5 ms, plus 6.3 ms of
write-watch scan and the completion thread waiting 12.8 ms a frame for the pool lock behind the sync. Causes:
(a) `Sub::upload` staged through `stage_bytes`, which memoises by content: every uploaded byte went through a serial
128-bit FNV hash and a map insert (thousands of page-sized calls a frame); (b) ~98% of the bytes were identical to what
the GPU copy held, but the skip was off (audit) and could not cover the 46 MB a frame of GPU-noted pages anyway.
*Corrections:* `Sub::upload` stages fresh (`queue.rs`); the upload skip below, on by default.

**Problem 1, upload skip correctness (`upskip.rs` rewritten, `mem.rs`, `gmem.rs`, `queue.rs`).**
- M1: `upload_changed` copies each page once out of guest memory, hashes that copy and uploads from it (a page whose
  guest bytes already hash as the GPU copy's is left out without a copy: nothing is uploaded, the kept hash still
  describes the GPU copy). The uploads of a batch go out with the hash lock released (staging can wait on a fence).
- M2: the upload budget is charged with the bytes actually uploaded (`budget_fits` / `budget_charge`).
- H2: `forget` and the other lookups reach every pool entry over a range: a take-while walk down from the range plus
  an explicit list of pools that lie over another (`State::over`), instead of stopping at the first pool ending below.
- H1: the raw-memory write log is read in a new `Post::Early` callback, which the completion thread runs **before** the
  submission's write-backs land and before its events are published; every range it names (any size, any identity) is
  forgotten and its pool marked `raw`: never skipped again (also a pool entry made later over a raw range).
- Default **on** (`NXGFX_UPLOAD_SKIP=0` off); measured below.

**Problem 2, GPU-written pages re-sent (`upskip.rs`, `mem.rs` `note_gpu_write_version` / `apply_writebacks`,
`queue.rs` `submit_sub`).** A per-write sequence: a recording epoch (`upskip::next_epoch`, moved in `submit_sub` once
every note of the recording is made, carried in `Inflight.epoch`). A GPU-write note parks the page's hash and stamps
the page with the epoch (`gpu_writes`); its landing (`landed`, called per write-back before the lazy/eager split)
re-arms only pages still stamped by that epoch: with the parked hash where the GPU left the page unchanged
(`src == before`), else with the hash of the GPU's bytes. Any later note, upload or forget restamps the page, so only
the last writer re-arms, and a page held lazily on the device is re-armed the same way. The pages it compared equal are
handed to `land_changed`, which no longer compares them again. OoT forest: **58.6 MB a frame re-armed, 56 MB a frame
not uploaded** (upload 56.7 -> 1.2 MB a frame).

**Problem 4, L6 (`lazy.rs`, `upskip.rs`).** `bring_back` no longer resets the write watch after its copy (no lock, so
no deadlock). It records the page and the hash of the bytes it put there (`upskip::brought_back`); the copy's store
reads as a CPU write, and the next sync leaves the page out while it still holds exactly those bytes, whatever else it
knows (also in a raw pool): only a real CPU change since is uploaded. That keeps the old reset's meaning (a CPU read of
a held page uploads nothing, so the device's older bytes never go back over GPU writes made since, e.g. raw stores
into pools over 64 MB, which are never written back) and closes the lost-write window: a game store between the copy
and the old reset had its watch bit erased and never reached the GPU. With `NXGFX_UPLOAD_SKIP=0` the old reset stays.
(v1-v3 had the no-reset part only; v4 added the record.)

**Problem 3, the rest (`mem.rs`, `upskip.rs`).**
- Scanning syncs skip pools the scan found unwritten with nothing pending or deferred before any per-pool work.
- The walker's walks for syncs leave the cold pools out, as a mid-frame scan does (they took ~10 ms each, ~3 a frame,
  in MKW). Such a walk counts only as a mid-frame scan (`note_walk(.., full=false)`), covers nothing once a cold pool was
  found written since it started (`COLD_GEN`), and stops at its next piece when a present asks for a full walk; a
  present's sync waits at most 2 ms for that full walk to start, else scans itself as before. **Made opt-in in v5**
  (`NXGFX_WALKER_WARM=1`): no FPS gain anywhere, and the only builds that showed the OoT white-objects frame had it on
  (see Unproven).
- Large ranges (>= 4 MB) are hashed, and large landings compared, on a small persistent thread pool
  (`par_for`, `vk page hash` threads) with the caller.

**Problem 5, missed or late CPU writes (audit by reading, plus counters).**
- Fixed: the `bring_back` reset window above (a store could be lost for good).
- Fixed: `sync_range` (cold-pool copy source) uploaded pages it took from the watch without telling the texture check:
  a texture over those pages kept stale bytes until rewritten. They now go to the next sync's dirty list
  (`Pool::tex_dirty`).
- Measured, new counters in the pool line: CPU writes **held back under in-flight write-backs** (OoT 0.06-0.09 MB a
  frame, 1.3-2.2 syncs) and **deferred past the upload budget** (0 now; OoT's 56.7 MB a frame was near the 64 MB budget
  before, which defers ranges a submission or more, and buffer textures bound through the pool descriptor table do not
  resolve, so they would read the old bytes until a later sync).
- Unchanged policies (by design, not fixed): cold pools (no write for 60 frames) are asked once a frame; the
  in-flight write-back guard holds a CPU write under a GPU write still in flight until it lands.
- Upload skip, accepted window: the first raw-memory store ever into a pool is learned at its submission's completion;
  a CPU rewrite of the same bytes made before that completion, without waiting for it, can be skipped (the game races
  the GPU there on hardware too); from then on the pool is never skipped.

### 2. Tests (`cargo test --release -p graphics`: lib 244 passed, 6 ignored; integration tests pass; `py -m unittest scripts.test_play` OK)

- `upskip::tests::a_page_brought_back_unchanged_is_not_uploaded` (L6): a page a lazy write-back brought back is left
  out once while it holds exactly those bytes, also in a raw pool; a CPU change since is uploaded.
- `upskip::tests::unchanged_pages_are_not_uploaded_again` (reworked for `upload_changed`): same bytes skipped; changed,
  forgotten, partial pages and a dropped pool uploaded; the bytes handed to the upload are the copy that was hashed
  (M1).
- `upskip::tests::forget_reaches_a_pool_under_a_nested_one` (H2): a forget past a nested pool that ends below the range
  reaches the outer pool. The old `take_while` stopped at the nested pool (fails there by construction; not re-run on
  the old source).
- `upskip::tests::a_landing_rearms_only_the_last_writer` (problem 2): a landing re-arms unchanged pages with the parked
  hash and changed pages with the GPU's bytes, leaves pages re-uploaded meanwhile alone, and a later note before the
  landing keeps them unknown until the later submission lands.
- `upskip::tests::raw_written_pools_are_always_uploaded` (H1): a pool a raw write log named is never skipped, also when
  its entry is made later.
- `upskip::tests::a_large_range_is_hashed_in_parallel_alike`: the thread-pool hash and compare paths (>= 4 MB) skip,
  upload and re-arm as the serial ones; the landing returns exactly the unchanged pages.
- `mem::budget_tests::the_budget_counts_what_is_uploaded` (M2).
- `mem::scan_cover_tests::a_scan_after_the_op_was_queued_covers_it`, extended: a walk without the cold pools covers a
  mid-frame sync, never a frame's first; a full walk covers both.
- All new or reworked; the game runs are the real check.

### 3. Verified (GPU 1, each run alone under `machine.lock`; windows as the scenarios define them)

Builds: `scripts/build/build.ps1 -KeepCache -SkipGnu` and relinks of MKW, OoT, DKB, Smash under `dist.lock` (DLL
times checked). "base" = HEAD `2a2f4f9` relinked from today's dist; v1 = first build; v2 = + shared landing compare,
parallel hashing; v3 = + persistent hash threads, parallel landing compare, present wait <= 2 ms; v4 = + the
brought-back record for L6; **v5 = final** (warm walks opt-in; MKW and OoT relinked). Per-frame ms from the frames CSV; pool parts and MB from the `[vk] pools:` lines in
the window.

| Run (`build/play/`) | FPS / 5% low | replay | prep | pool sync | lock / scan / per-pool pass | upload MB | completion write-back (lock wait) |
|---|---|---|---|---|---|---|---|
| `20261126-013500-base-mkw` | 30.1 / 28.5 | 12.76 | 10.73 | 8.19 | 1.52 / 4.00 / 2.57 | 3.05 | 4.78 (3.03) |
| `20261126-030929-after-mkw` (v3) | 30.1 / 26.8 | 12.31 | 9.40 | 7.18 | 1.30 / 3.95 / 1.87 | 1.26 | 4.69 (2.67) |
| `20261126-044656-v4-mkw` (v4) | 29.5 / 22.8 | 11.69 | 8.68 | 6.73 | 1.18 / 3.72 / 1.67 | 1.16 | 4.40 (2.44) |
| `20261126-055832-v5-mkw` (**v5, final**) | **30.4 / 29.5** | 12.02 | 8.68 | 6.54 | 1.12 / 3.50 / 1.81 | 1.48 | 4.40 (2.37) |
| `20261126-014039-base-mkw-turn` | 29.4 / 26.0 | 12.89 | 13.17 | 10.65 | 1.40 / 6.55 / 2.64 | 3.10 | 5.80 (3.98) |
| `20261126-031507-after-mkw-turn` (v3) | 29.5 / 25.0 | 12.47 | 9.82 | 7.47 | 1.27 / 3.98 / 1.89 | 1.34 | 4.74 (2.66) |
| `20261126-014812-base-oot` | **14.1 / 7.0** | 28.68 | 29.65 | 26.22 | 1.31 / 6.31 / 18.54 | 57.31 | 21.93 (12.82) |
| `20261126-021536-after-oot` (v1) | 17.0 / 13.8 | 22.96 | 23.63 | 18.85 | 4.32 / 0.14 / 8.08 | 1.44 | 16.86 (2.26) |
| `20261126-023504-v2-oot` (v2) | 18.7 / 16.0 | 22.63 | 19.30 | 15.56 | 1.06 / 0.17 / 8.06 | 1.58 | 11.92 (1.97) |
| `20261126-025416-v3-oot` (v3) | 18.9 / 14.0 | 20.41 | 18.14 | 14.16 | 1.58 / 0.74 / 5.65 | 1.55 | 9.54 (2.31) |
| `20261126-043138-v4-oot` (v4) | 18.6 / 15.0 | 21.36 | 18.47 | 14.76 | 1.56 / 0.61 / 6.13 | 1.63 | 9.14 (1.79) |
| `20261126-060428-v5b-oot` (**v5, final**) | **18.8 / 13.0** | 21.16 | 18.19 | 14.28 | 1.62 / 5.75 / 6.10 | 1.39 | 9.91 (2.67) |
| `20261126-014536-base-dkb` | 59.2 / 53.0 | 5.70 | 5.13 | 4.92 | 0.41 / 3.32 / 1.14 | 2.14 | 2.50 (1.69) |
| `20261126-032005-after-dkb` (v3) | 59.3 / 53.0 | 5.47 | 4.79 | 4.64 | 0.24 / 0.79 / 0.56 | 0.46 | 1.81 (1.00) |
| `20261126-045234-v4-dkb` (v4) | 59.3 / 53.0 | 5.10 | 4.61 | 4.48 | 0.22 / 0.75 / 0.51 | 0.46 | 1.83 (0.98) |
| `20261126-020356-base-smash` | 59.9 / 59.1 | 2.41 | 2.46 | 2.41 | 0.10 / 1.43 / 0.42 | 0.54 | 0.69 (0.62) |
| `20261126-032241-after-smash` (v3) | 60.0 / 59.1 | 2.36 | 2.47 | 2.38 | 0.08 / 0.58 / 0.29 | 0.31 | 0.29 (0.23) |
| `20261126-045510-v4-smash` (v4) | 60.0 / 59.0 | 2.39 | 2.51 | 2.47 | 0.08 / 0.63 / 0.27 | 0.29 | 0.27 (0.21) |

Targets: MKW 60, OoT 59, DKB 60, Smash 60. "pool sync" (frames CSV) also holds the present's sync finishing the
walker's walk, which is not in the three parts (OoT: ~5-6 ms a frame, before as the present's own full scan).

- **OoT forest, ~400 s** (not the FPS window): per-pool pass 51.9 -> 4.96 ms a frame, upload 56.7 -> 1.26 MB, the
  completion thread's pool-lock wait 40.7 -> 3.4 ms and its write-back 49.1 -> 9.3 ms; 54.6 MB a frame left out and
  57.6 MB re-armed at landings; copy and hash 3.7 ms, re-arm compare 3.5 ms (v3).
- **Skip on vs off (v3 build, `20261126-033042-v3-skip0-oot`, `NXGFX_UPLOAD_SKIP=0`)**: 12.6 / 4.0 FPS, per-pool pass
  20.2 ms, upload 57.6 MB, completion lock wait 13.0 ms, against v3's 18.9 / 14.0, 5.65 ms, 1.55 MB, 2.31 ms. The
  fresh staging alone does not move the pass (20.2 vs base 18.5): the uploads' copies were the cost. **Default on.**
- **OoT window** pool sync p50 / p90: base 25.9 / 28.0, v3 13.7 / 19.7 ms. Frame 71.0 -> 52.5 ms. The replay
  (translation) is now the larger part (20-23 ms), then prep 18 ms (of which ~5-6 ms the present's walk).
- **MKW**: upload 3.05 -> 1.16-1.26 MB, prep 10.7 -> 8.7-9.4 ms, replay thread 23.5 -> 20.4-21.7 ms, completion
  -0.4 ms; FPS unchanged at the 30 cap (16.7 ms needed for 60). The 5% low varies 22.8-28.5 across my runs (v4 22.8):
  frames over 50 ms in the window were base 53, v3 76 and 62, v4 86, against 48-72 in MKW-RECORD's runs of the HEAD
  engine; those frames have a normal replay (~12-16 ms, prep 3-12 ms) and come from elsewhere (texture streaming: one
  has 78 ms of texture uploads); v5 (warm walks off): 30.4 / 29.5 with 34 such frames. The walk for syncs ran only
  22 of 600 walks: the walker is mostly busy with the present's full walks (re-walks while a present waits), so the
  ~4 ms of scan stayed. A/B `20261126-034619-v3-warm0-mkw` (`NXGFX_WALKER_WARM=0`): 30.2 / 28.0, scan 4.00 ms,
  prep 9.63 ms, the same as v3 with them: the warm walks neither help nor cost in MKW; DKB's scan fell 3.32 -> 0.79 ms
  with them.
- **DKB**: scan 3.32 -> 0.79 ms, upload 2.14 -> 0.46 MB; FPS unchanged (already at its cap). **Smash** unchanged.
- **Sheets read** (all): base and v3 of every title, plus v1 and v2 OoT. MKW boot/title/menus/grid/race/lap 2 correct;
  mkw-turn's ten turn shots lit (base and v3); OoT title, story, nightmare, house flash (`04-house`, white in base too),
  forest, Deku Tree, Navi, Link woken, house, Link moving in every walk shot; DKB boot to the punch tutorial; Smash
  boot, character select, two matches (random fighters and stages: v3's second match is on another stage; its
  `06-results` frame is black, a fade between the match end and the results; base's shows "TIME!").
- **Red lines**: the known ones (timestamp/occlusion query use-up, `pipeline-crashed.bin`, OoT's two "no render target
  bound"). New in v2 and v3 OoT, once each: `a draw with no render target bound (... depth set): skipped` (889 s / 794 s,
  in the house); not in base or v1. Not explained (see Unproven).
- The `depth set` red line appears in v2-v5 OoT once each (see Unproven); MKW v5 has an OS audio-device red line
  (unrelated). v5 sheets (`055832-v5-mkw`, `060428-v5b-oot`) correct; OoT's v5 scan is back to ~5.8 ms (the present's
  own scan, warm walks off).
- **No device loss**; every run exited normally.

### 4. Unproven / next

- **A white-objects frame in Link's house** (`20261126-023504-v2-oot/15-h2.png`, 666 s): the hanging herbs, baskets and
  pots pure white with lightning-like arcs on the vines, one frame. New scenario `oot-house` (as `oot` to Link in
  control, then 30 shots ~2 s apart): **with the skip on** 1 of 30 (`20261126-035202-v3-diag-oot-house`, d09 @654 s),
  **with the skip off** 1 of 30 (`20261126-040408-v3-diag-skip0-oot-house`, d19 @676 s): not the upload skip. Either
  the game's own effect (it looks like a lightning flash; the opening's nightmare has lightning) or older than this
  wave: the sparse base / v1 / OOT-FOREST2 shots (every 33 s) would catch a ~1-in-30 frame only by luck. A third dense
  run with the skip and the warm walks off and the old lazy reset back (`20261126-041959-v4-diag-off-oot-house`): 0 of
  30; the **HEAD engine** (OoT relinked from the MKW-RECORD worktree's dist, `20261126-051028-base-diag-oot-house`): 0 of
  30. Pooled: warm walks on 3 in ~67 shots, off 0 in 60 (13% chance at the same rate): suggestive, not proof. The
  v3 runs with it also had the lazy page's redundant upload (fixed in v4). **v5 makes the warm walks opt-in.** The v5
  dense run (`20261126-053140-v5-diag-oot-house`) ran faster through the house and the A presses walked Link out to
  Saria, into the pause menu and a save (harness copy): all 30 pictures correct but a different scene, so it does not
  count. Next: a dense route that holds Link in the house (no A), on v5 and on HEAD.
- The new `no render target bound (... depth set)` red line, once a run late in the OoT house walk (v2, v3, v4, v5;
  not in base, v1 or the skip-off run, which walked a different route at their frame rates): command-stream state,
  not pool memory, as far as I can tell; unexplained. A first v5 OoT run (`20261126-054320-v5-oot`) desynced at the
  title (A opened Options) and is not used.
- **MKW's write-watch scan (~4 ms) and lock wait (~1.3 ms)** remain; walks for syncs rarely run there because the
  walker re-walks for the present while it waits (MKW-RECORD's M2 rule). Next: let a walk for syncs go between the
  present's re-walks (coherence trade-off: cold-pool freshness for the present).
- **OoT's present sync** still finishes a full walk itself (~5-6 ms a frame): its presents run behind parked work, so
  only a walk started after the parked work was taken covers them.
- **Completion thread**: the re-arm compare (3.5 ms a frame in the forest) reads the whole write-back range twice
  (result and baseline). Next: compare on the GPU, or write back only what shaders can write.
- 60 FPS is not reached in MKW or OoT; OoT's replay (translation, ~20-23 ms) is now its largest part.
- The in-flight write-back guard holds CPU writes under a GPU write in flight (0.06-0.15 MB a frame in OoT): a
  submission or more late, by design (MKW8D's black stretches); unchanged.

**For the lead's audit (memory / coherence):**
1. `upskip.rs` as a whole: the page states (hash, parked hash, stamp), the epoch taken in `submit_sub` after the
   recording's last note, `landed` before the lazy split under the pool lock, `gpu_writes`' per-thread repeat filter
   (same epoch, same range), the over-list replacing the take-while (H2), the batch loop releasing the lock before its
   uploads, the guest-hash skip without a copy.
2. `Post::Early` in `completion_loop` (runs before `apply_writebacks`), and `raw_written` marking pools for good.
3. `lazy::bring_back` without the watch reset (L6), and `upskip::brought_back` / `still_brought_back` leaving the
   page out while unchanged (a CPU rewrite of the very same bytes after a GPU raw store is then not uploaded: the old
   reset had the same blind spot only for its lost-write window, and raw stores to pools over 64 MB never reach guest
   memory anyway).
4. Walks for syncs without the cold pools (opt-in `NXGFX_WALKER_WARM=1` since v5; the code paths that stay live by
   default: `note_walk(.., full)`, `walker_covers_ex` only taking full jobs and waiting up to 2 ms for one to start);
   and `begin_submit`'s new skip of unwritten pools.
5. `land_changed` trusting `landed`'s unchanged pages; `par_for` (lifetime transmute; the caller waits for every call).
6. `Sub::upload` staging fresh; `budget_fits` / `budget_charge`; `Pool::tex_dirty`.

### 5. Slips

- The Smash relink of the baseline (a full recompile, 45 min) was still running its last compile unit (one core) while
  the base runs of `mkw`, `mkw-turn` and `dkb` ran; its link step ended at 01:49:54, two minutes into the base `oot`
  run (before its window). Those three baselines may be slightly pessimistic.
- I held `dist.lock` from 00:59 through the baseline runs (to keep the relinked baseline DLLs), and twice used it as
  `UPLOAD-SYNC-BUILD` to hold my own queued perf runs while I rebuilt.
- v1 and v2 runs exist only for OoT; MKW/DKB/Smash "after" runs are v3 and v4.
- For the base-engine dense check I relinked OoT from the finished MKW-RECORD worktree's `dist`
  (`D:\Projects\RecompLauncherEngine-mkw`, `06f2a6e`, src identical to HEAD) into OoT's normal build folder, then back
  from the main `dist`: each relink recompiled OoT (~7 min).
- New scenario `oot-house` in `scripts/scenarios.toml` (a diagnosis route; keep or drop).
- **Build state left:** dist = v5; MKW and OoT relinked from v5; **DKB and Smash still carry v4** (warm walks on by
  default there): relink them from today's dist.
- No commits; no edits to docs/32; saves only the harness's copies.

### 6. Questions

None.
