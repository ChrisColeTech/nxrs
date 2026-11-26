# OOT-FOREST2 — Ocarina: indirect dispatch sizes, forest haze, forest frame time, Link moving

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md`. Works in the **main tree** (`D:\Projects\RecompLauncherEngine`).
Title: "The Legend of Zelda Ocarina of Time" (scenario `oot`, GPU 1).

**Runs beside MKW-RECORD**, which works in its own git worktree with its own `target/` and `dist/` and relinks only
Mario Kart World. So: **do not build, relink or run Mario Kart World**; your regression title is `wonder` (relink it
from the main `dist`). Both waves use the main tree's `build/locks/` — when MKW-RECORD holds
`build/locks/machine.lock` (a perf measurement), start no game or build until it is released. An independent
read-only audit of OOT-NAN's change may also be running; if the lead fixes something it will tell you.

## Brief (lead)

**State (OOT-NAN, `docs/waves/OOT-NAN.md`, commit `9903fc7`).** The forest renders since the NVN2 dispatch-size fix
(c0 + 0x4). Open, from that report:

1. **Indirect dispatches** still put CPU-side counts (probably 0) into c0 at both offsets (0x200 and NVN2 0x4),
   where nxnative copies the indirect record into c0 on the GPU. Any compute that reads its group counts under an
   indirect dispatch divides or loops by 0. Affects NVN1 titles too. Fix it as nxnative does (a GPU copy of the
   indirect record into the c0 buffer before the dispatch, with the right barrier), test it, and show the effect.
2. **The forest is very misty and blown out** (`build/play/20261125-153150-oot-nan-fix` shots 06-08: heavy white
   haze over the Deku Tree). The lead's read of the sheet is that it is too strong. First suspect: item 1 (fog or
   froxel computes dispatched indirectly). Prove it either way with a pass dump.
3. **Forest frame time**: frames of 70-380 ms (25-35 ms of pool preparation, ~56 MB uploaded per frame), and ~20 s
   with no frames presented entering Link's house (~617-637 s). Find what is uploaded every frame and why (memory
   that does not change should not be re-uploaded); fix as a general correction.
4. **Route to Link moving under input**: press A faster through the house dialogue, put the walk and the FPS window
   right after "Let's go together!". Target 59 FPS; report FPS vs 59 and 5% low.

**Done:** items 1-3 fixed with game evidence (sheet read), item 4 reached with a measured window; or parked at the
deadline with where each stands. Unit tests for each correction; `cargo test -p graphics --lib --release`.
Regression: `py scripts/play.py wonder --env NXGFX_GPU=0` (sheet read, red lines unchanged).

**Memory/descriptor/coherence changes: the lead runs an independent audit.** Never skip draws or dispatches; don't
weaken robustness; native NDVE stays off. **Exclusions:** `dmsh_xlate` itself, Mario Kart World, `vk/queue.rs`
(MKW-RECORD's), no commits, don't edit docs/32. Keep `vk/exec.rs` / `vk/mem.rs` edits minimal and local (the
lead merges MKW-RECORD's branch later). Back up and restore anything you move, and check the direction.

**Deadline:** 5 hours wall clock. Report per protocol §5 in this file under "## Report".

## Report (agent, protocol §5; 2026-11-25 17:48-~19:50, beside MKW-RECORD)

**Outcome.** Item 1 fixed (GPU copy of the indirect record into a private c0). Item 2 proved **not** item 1 (A/B
pass dumps identical) and localized to the bloom composite; not fixed (no engine fault found; see Unproven). Item 3:
the forest/house frame time was mostly **pool snapshots** (8-24 a frame at 13-17 ms each), now ~0.1 ms a frame by
recycling snapshot buffers; plus 98% of the ~56 MB/frame upload is bytes identical to the last upload, of which the
safe part (~8-10 MB/frame) is now skipped. Forest 7.0 → 14.8 FPS, the house 3.2-4.6 → 15.1 FPS. Item 4: **Link
moves under input** in his house (Move / Change View prompt; in control by 600-800 s depending on the run): window
[845, 915] **13.7 FPS mean, 5% low 10.0 (target 59)**; the pool sync (~36 ms a frame) is now the limit.

1. **Built.**
   - **Item 1 — indirect dispatches** (`vk/exec.rs`). *Diagnosis:* `nvnCommandBufferDispatchComputeIndirect` records
     x/y/z = 0, and `c0_bytes` put those at 0x200 (and 0x4 for NVN2). nxnative `nvn_mtl.m` 15777-15780 copies the
     GPU record into that dispatch's c0 at both offsets, then a blit→dispatch barrier. OoT runs ~130 indirect
     dispatches/s from 352 s on, all reading c0 (`a22751e0…`, `88ad7608…`, `878cb27d…`, `d731a682…`, `58f47abf…`,
     `9264bdb2…`). *Correction:* `dispatch_guest` marks the dispatch (`INDIRECT_C0` thread-local); `QState::banks`
     then stages that compute set's c0 with `stage_bytes_fresh` (never shared with another set) and `set_key`
     returns None for it (no set-cache reuse / patch); after `bind_sets`, `cmd_copy_buffer` copies the (clamped)
     12-byte record to c0+0x200 and, for NVN2, c0+0x4 (`indirect_c0_copies`), then the full barrier, then the
     indirect dispatch. `NXGFX_INDIRECT_C0=0` (diagnostic) gives the old behaviour. Log: `indirect dispatch N:
     compute <key> reads c0, its group counts copied there on the GPU` (powers of two).
   - **Item 2 — haze** (diagnosis only). Pass dumps at 400 s with the copy (`-175459-oot-f2-copy`, dump in the
     scratchpad) and without (`-180716-oot-f2-nocopy`, `NXGFX_INDIRECT_C0=0`): the final frames show the same
     milky haze, so item 1 is not the cause. The HDR scene is clear through the forward/translucent passes
     (pass 122 of the copy dump); the haze is added by the post composite (pass 139: shader `99d1ecbf2947464b`
     reads the 240x135 bloom accumulation ser19765 + the scene). The bloom pyramid is the whole scene downsampled
     (`e3db3f15…`/`e894c441…`, no threshold visible) and upsampled with blend `ONE*src + CONSTANT_COLOR*dst`, the
     game's blend constant being [1,1,1,1] (other draws set [0,0,0,1], [1,1,1,0.2]: the constant is the game's,
     not a default). The conditional clear before the composite (render condition mode 1, counters 1/0 → render)
     clears the composite's own target ser50217, as the game binds it — correct. Dump diagnostics added for this:
     blend constant in the per-draw line, render-condition counters/outcome, the clear's target serial.
   - **Item 3 — frame time** (`vk/tables.rs`, `vk/upskip.rs` new, `vk/mem.rs` hooks, `vk/uptrace.rs` new).
     *Diagnosis:* in the forest/house the replay was 50-160 ms a frame, of which **pool snapshots** were
     49-135 ms (route7 616 s: 8.1 made a frame, 135 ms): every texture-pool descriptor change made a new 10 MB
     host-visible descriptor buffer (`vkAllocateMemory` + whole 10 MB copy over BAR). Separately the pool sync
     uploaded ~56 MB a frame; the new `NXGFX_UPLOAD_TRACE=1` showed 98% of it identical to the page's last
     upload (double-buffered 15 MB / 9 MB / 6.6 MB mirrors at 0x29c0ee000-0x2a192a080 rewritten each frame).
     *Corrections:* (a) `PoolSnapshot` returns its buffer to a recycle list (≤32) when the GPU is done with it;
     `Tables::snapshot` takes one and copies only the shadow ranges written since that buffer's generation
     (`Inner::edits`, every shadow write logged as (pool generation, offset, size) in `put_pool_descriptor`, the only
     shadow writer; past 65,536 entries the oldest half is dropped and an older buffer is copied whole). (b) `upskip`:
     each whole page uploaded by the pool sync keeps an xxHash64-style hash; a CPU-written page whose bytes equal
     that hash is not uploaded. The hash is forgotten whenever anything else changes the GPU copy:
     `note_gpu_write_version` (virtual and translated address), `sync_range`, `upload_deferred`, granule binding, a
     new pool buffer, `pool_add` / `release`. Partial pages are always uploaded. `NXGFX_UPLOAD_SKIP=0` turns it
     off. *Not done:* most of the remaining upload (46.7 MB/frame "no hash kept", 220 MB/frame of forgets) is pages
     whose GPU copy is declared GPU-written each frame (storage bindings / raw writes over whole buffers); skipping
     those safely needs the write-back landing to re-arm the page hash with the GPU bytes, which needs a per-write
     sequence carried through the write-back records (`queue.rs` territory). Next step, see Unproven.
   - **Item 4 — route** (`scripts/scenarios.toml` `[oot]`, new `scripts/inputs/oot_house_walk.txt`): the first
     565 s unchanged (through the Deku Tree talk, x1-x5); then A every 1.5 s in seven ~33 s blocks (h1-h7,
     ~603-836 s; the house dialogue and dressing, which finished by 600 s in the final run and by ~800 s in route2:
     the faster frames move it), then three 24 s walk squares in the house (left/down/right/up 1.5 s each) with
     shots; window [845, 915] = the walks; 935 s. A in gameplay does nothing visible there. The entry's comment
     is updated.
   - **Folded in from the lead's OOT-NAN audit:** M1 gap (`image.rs` `reg_tex_now` and the re-backing loop): no pool
     storage view when the slot's *view* format is emulated either (`emulated_storage(view_format(im, v))`). LOW
     (`exec.rs` pass-dump reads of uniform banks and storage buffers): skipped for a sparse virtual-pool address.
   - `docs/env_switches.tsv`: rows `NXGFX_INDIRECT_C0`, `NXGFX_UPLOAD_SKIP`, `NXGFX_UPLOAD_TRACE` (by hand).
2. **Tests** (`cargo test -p graphics --lib --release`: **212 passed, 0 failed, 6 ignored**, after the final source).
   - `exec::set_patch_tests::indirect_dispatch_copies_its_record_into_a_private_c0`: the copy regions (0x200 always,
     0x4 only for NVN2, 12 bytes from the record), and `private_c0` only while marked, only for a compute set that
     binds c0. New helpers, so it could not fail before; the game evidence is the log above.
   - `tables::registration_boundary_tests::recycled_snapshot_gets_the_writes_since_its_generation`: a recycled
     snapshot of generation G gets exactly the writes made at G or later, bytes copied; after log overflow an old
     snapshot is copied whole.
   - `upskip::tests::unchanged_pages_are_not_uploaded_again`: same bytes skipped, a changed page / a forgotten page /
     a partial page / a dropped pool uploaded; hash distinguishes length and word position.
   - No unit test for the M1-gap fix (needs a device); `py -m unittest scripts.test_play`: OK.
3. **Verified** (OoT GPU 1; builds `build.ps1 -KeepCache -SkipGnu` and relinks under `dist.lock`, `@result ok` and
   DLL time checked after each).
   - `build/play/20261125-175459-oot-f2-copy` (item 1 build, trace + dump at 400 s, 700 s): 233 steps to the
     limit, no device loss; red lines: the 2 known + 4x `queue … parked for 2 s` at 404 s (the pass-dump frame's
     11 s stall; also in the no-copy run, so the dump). Sheet: title, story, nightmare, **04 now Ganondorf** (OOT-NAN's
     run had a white movie frame there), forest load black, Deku Tree / Navi / roots render, hazy as before.
   - `build/play/20261125-180716-oot-f2-nocopy` (`NXGFX_INDIRECT_C0=0`, dump at 400 s, 440 s): same haze.
     **Ran while MKW-RECORD held `machine.lock`** (see Slips).
   - `build/play/20261125-184737-oot-f2-route2` (all fixes, dump at 400 s; killed by PID at ~853 s once gameplay
     was seen, to shorten the route): forest 470-600 s **15.3 FPS (5% low 5.5)** vs route7 7.0 (1.8); replay
     16.9 ms/frame, pool snapshots 0.09 ms/frame (1.7 made) vs 49-135 ms; upload 47.7 MB/frame (8.3-8.5 skipped).
     **Link in control in his house from ≤803 s** (h6, h7: "Move / Change View", Navi beside him); standing there
     (A presses) 805-850 s **14.7 FPS (5% low 12.6)** vs route6/7 at the same point (1180-1230 s) 3.2-3.6 FPS.
     Also overlapped MKW-RECORD's lock.
   - **Final, `build/play/20261125-192048-oot-f2-final`** (`play.py oot --env NXGFX_GPU=1`, the new 935 s route,
     **alone: `machine.lock` held** and the MKW run waited out): 363/363 steps, normal exit, **2 red lines** (the
     known `no render target bound` and `pipeline-crashed.bin` skip), no device loss. **Window [845, 915] (Link
     walking squares in his house under input): 13.7 FPS mean, 5% low 10.0, min 4.3 — target 59.** Other stretches
     (frames CSV): forest 470-560 s 14.8 FPS (5% low 4.2; route7: 7.0 / 1.8), house cutscene + standing 600-830 s
     15.1 (11.8; route6/7 in the same scene 3.2-4.6 / 1.0-1.7). Per frame at 908 s: GPU 8.7 ms, replay 12.4 ms,
     pool snapshots 0.14 ms (1.9 made), **pool sync 36 ms** (upload ~47 MB of which 0.65 MB changed pages, 46.4 MB
     pages whose hash was dropped by GPU-write notes) — the pool sync is now the limit. **Sheet:** 1: title, story,
     nightmare, Ganondorf, forest load, Deku Tree / Navi / roots (hazy as before); 2: from x5 (600 s) Link stands in
     his house with "Move / Change View" and Navi; walk1-walk4 he runs left, toward the camera, right (bed with the
     curtain, window), the camera following; 3: walk5-walk9 and last: he crosses the room both ways, reaches the
     bed — **Link moving under input**. Memory 21.5 GB at 600 s vs route7's 17.3 GB: the allocator shows textures
     6.3-9.4 GB vs 4.5 GB (the game is further on and the walk looks around the room); not chased.
   - **Regression, Wonder** (`build/play/20261125-194749-wonder-f2`, relinked on the final engine, GPU 0, not a perf
     run): 30/30, normal exit, **0 red lines** (as `build/play/20261124-121843-wonder`), window 59.5 FPS (5% low 57.1; before
     60.1 / 59.4, one 451 ms hitch at 47 s in the menus). Sheet: title, course, gameplay, later, end, all correct.
4. **Unproven / next.**
   - **Haze (item 2):** whether the bloom strength is the game's intent. The engine's inputs to it look right
     (blend constant from the game, conditional clear on the right target, clean scene). Next: compare with a
     reference capture of this scene (nxnative on the Mac) or the bloom pyramid's values (raw read-back of ser19765
     vs the scene), and check the downsample shader `e894c441…`'s threshold input (c3 word 3 = 0.5?) against its SPIR-V.
   - **Remaining upload (item 3):** ~47 MB/frame of rewritten pages that are inside ranges noted as GPU-written each
     frame. Next: carry a per-write sequence from `note_gpu_write_version` through `note_writeback` / `apply_writebacks`
     to `land`, and re-arm `upskip`'s page hash there with the GPU bytes (`src`) when no later GPU write was noted;
     `NXGFX_UPLOAD_TRACE=1` now lists the forgotten ranges for this. Pool sync is still ~36 ms a frame on the prep
     thread, the main limit to FPS (GPU ~9-11 ms, replay ~17 ms).
   - 59 FPS is far off (see §3).
   - **For the lead's audit** (memory/descriptor/coherence): (1) `tables.rs` snapshot recycling: that the edit log
     sees every shadow write (`put_pool_descriptor` is the only writer) and that a recycled buffer comes back only
     after its last GPU use (`defer_after_use` → `Drop`); (2) `upskip.rs` + its `mem.rs` hooks: that every path that
     changes a pool's GPU copy forgets the hash (an untracked GPU write would now keep its bytes where the CPU
     rewrote identical old ones); (3) `exec.rs` indirect c0: the private c0 is never shared or cached, the copy reads
     the clamped record after its barrier. `vk/mem.rs` edits are seven one-line hooks.
5. **Slips.**
   - Two of my runs overlapped MKW-RECORD's `machine.lock` (taken at 18:08 and 18:48, each ~1 min after I had checked
     it free and started a run: `-180716-oot-f2-nocopy`, `-184737-oot-f2-route2`, and the crashed `-182802-oot-f2-route`).
     Their measurements in those spans may be disturbed; my FPS numbers from those runs are indicative only.
   - `-182802-oot-f2-route` died at 402 s (exit 70, host access violation, read of 0x10): **my own new dump
     diagnostic** read the render-condition counters at address 0 + 16 for a mode-2 (unconditional) record. Fixed
     (counters logged only for conditional modes with an address); only with a pass dump active. Its minidump in the
     OoT build folder and its run folder were deleted. Pass-dump PNGs stayed in the session scratchpad (not kept).
   - The final run held `machine.lock` ~22 min (over the 20-minute guideline) because it waited for an MKW run to end.
   - `scripts/scenarios.toml` `[oot]` is now 935 s.
   - Saves: harness copies only.
6. **Questions.** None.

## Report (independent audit of `1eb9180`, read-only, 2026-11-25)

Audited the committed source (`git show 1eb9180:<path>`; the working tree's `src/graphics` already has other edits).
No builds, no games. Line numbers are in the committed files.

**HIGH**

- **H1 — `upskip` never learns of raw global-memory (gmem) stores in time, and never at all for pools > 64 MB.**
  `vk/gmem.rs:236` (`hi - lo <= WB_MAX`), `vk/gmem.rs:276-281`, `vk/queue.rs:1761-1765`. The gmem write log is read in
  the submission's completion callback, and `gmem::begin` notes those pools (→ `note_gpu_write_version` → `forget`)
  at the start of a *later* submission, *after* that submission's `mem::begin_submit` has already run
  `changed_runs`. Ranges over 64 MB, or with identity 0, are never noted. *Scenario:* sub K uploads page P (bytes X,
  hash h(X)); a raw-pointer shader in K stores Z into P (DKB/MKW `c9a56f796ea4a4cd` write indirect records this way).
  Frame N+1 the CPU writes X into P again (reset of the record): K+1's pool sync finds h(X) and **skips the upload**
  — K has not completed, or has completed but `gmem::begin` runs after the sync — so the shader reads Z, not the
  CPU's reset. In a > 64 MB pool the hash is never forgotten: every identical CPU rewrite is skipped for good. Before
  this commit the rewrite was always uploaded. *Fix:* (a) call `upskip::forget` for every logged range in the
  completion callback, regardless of `WB_MAX`/identity; (b) keep a sticky set of pools ever named by a gmem log (and
  of every pool while a submission that bound a gmem log with stores is unretired) and return the whole range from
  `changed_runs` for them; at minimum move `gmem::begin` before `mem::begin_submit`.
- **H2 — `upskip::forget` stops early on nested pools.** `vk/upskip.rs:119-125`: `range(..a+n).rev().take_while(end >
  a)` assumes pools do not overlap ("pools do not overlap"), but `mem.rs` supports pools laid over each other
  (`find` comment at mem.rs:1471, `room_hit` mem.rs:740, `distribute_watched_pages` mem.rs:1706). *Scenario:* A =
  [0x1000_0000, +16 MB) and B = [0x1000_2000, +4 KB) both have hashes; a storage write at 0x1008_0000 (resolves to A)
  calls `forget`; the reverse walk meets B first (end ≤ a), `take_while` stops, A's page keeps h(X); the next identical
  CPU rewrite of that page is skipped and A's GPU copy keeps the shader's bytes. *Fix:* skip rather than stop on
  entries ending at or below `a`; bound the walk by the largest pool size kept in `H` (track `max_len`; stop when
  `base + max_len <= a`).

**MEDIUM**

- **M1 — the hash and the upload read guest memory at different times.** `vk/upskip.rs:91-99` hashes a page, then
  `mem.rs:2088-2089` (`upload_now` → `sub.upload`) copies it later (after the whole run is hashed: up to several MB,
  ~1 ms). The game threads write concurrently. *Scenario:* page content X is rewritten each frame by "clear, then
  fill" (or any transient different value); the hash sees X, the copy catches the half-cleared page M; the write watch
  flags the page, the CPU finishes with X, the next sync finds h(X) == stored hash and **skips** — the GPU keeps M for
  as long as the bytes stay X. Before the commit the flagged page was re-uploaded and the race healed itself. *Fix:*
  hash exactly the bytes uploaded: copy each page to a local 4 KB buffer, hash it, upload from that copy (or hash the
  CPU-side source after copying it into staging, never guest memory twice).
- **M2 — skipped bytes still consume the upload budget.** `mem.rs:2078` charges `budget_take(n)` for the whole range
  before `changed_runs` drops 98% of it, so real changes behind it are deferred a submission earlier than needed
  (deferred ranges are then re-uploaded whole via `upload_deferred`, which also forgets their hashes). *Fix:* run
  `changed_runs` first and charge `budget_take` per returned run (defer the unchanged-hash update for deferred runs).

**LOW**

- **L1 — `image.rs` M1-gap fix leaves the slot's old storage descriptor.** `image.rs:2483-2490` / `3086-3093` pass
  `sview = None`; `tables.rs:1022`/`1053` then write no `B_IMG` descriptor, so the ID keeps whatever storage view was
  registered there before (another image, if the ID was reused), while `storage_tex` forgets the ID (stores through it
  go unnoted). Pre-existing for emulated `im.format`; the fix widens it. *Fix:* write a null / zero storage descriptor
  at `B_IMG` when `storage` is None (as placeholders do).
- **L2 — NDVE pool spans are not noted.** `newndve/kernels.rs:185` zero-fills and NDVE kernels write
  `prepared_span` ranges (exec.rs:7429 `ndve_info`) without `note_gpu_write`, so no `forget`. Native NDVE is off; matters
  only if it is enabled. *Fix:* `forget` (or note) every `prepared_span` bound writable.
- **L3 — recycled snapshots are never returned.** `tables.rs:38-53`: up to 32 × 10 MB host-visible (BAR) buffers stay
  allocated after a burst, and leak at shutdown. The count never exceeds the peak in flight, so no new peak; fine on
  ReBAR. *Fix (optional):* free recycled buffers beyond a small count after N frames.
- **L4 — `uptrace::Trace::last` grows without bound** (one entry per page ever uploaded) while `NXGFX_UPLOAD_TRACE=1`.
  Diagnostic only.

**Checked and found correct**

- *tables.rs:* `Inner::shadow` has a single mutable access (`put_pool_descriptor`, tables.rs:495) besides the zero
  init, so texel-buffer (B_TBUF/B_IBUF), image (B_TEX/B_IMG incl. placeholders and re-backing), and sampler writes are
  all logged; the shadow has a fixed size (no pool resize). Generation logic: writes are logged under the `inner` lock
  with the generation read there, `snapshot` reads its generation under the same lock, so a write logged below `from`
  is always in the snapshot; `refresh` copies `gen >= from` (conservative for writes at `from`). Overflow: gens in the
  log are non-decreasing, `edits_from = last dropped gen + 1`, so any buffer at or below a dropped generation is copied
  whole, every newer one gets all its writes. Recycling: every `snapshot()` call registers `defer_after_use` for the
  submission being recorded (`after_current`, attached at `submit_sub` to that submission's timeline value); there is
  one Vulkan queue, a device loss ends the process (`lost::on_lost`), so a buffer reaches `RECYCLED` only after the
  last submission that bound it completed. Lock order inner → RECYCLED is consistent (Drop takes only RECYCLED).
  One `Tables` per process, so the global list cannot mix shadows.
- *upskip forget paths:* `note_gpu_write_version` (both addresses) covers storage buffers, storage texel buffers,
  transform feedback (`tf_begin`), report/fill (exec.rs:1000-1003), semaphores/copies (exec.rs:8722-8764), texture →
  buffer copies (exec.rs:8834), gmem (late, H1). `sync_range`, `upload_deferred`, granule `bind`, `ensure_buffer`,
  `pool_add`/`release` forget or drop. Write-back landings (`land`, `land_changed`, lazy bring-back) change only guest
  memory, never the GPU copy, and the GPU write was forgotten when noted: correct. Image write-backs (`tex_wb`,
  ReadTexels) write guest memory only: the GPU copy still equals the last upload, so a skip is right. Unbound
  granules: hash set without an upload, but `bind` forgets and uploads the guest bytes: correct. Sparse pools
  (`make_sparse`) skip `ensure_buffer`'s drop but `pool_add` already dropped; buffers are never recreated. Ordering:
  `begin_submit` runs before the same submission's replay (queue.rs:1761/1778), uploads land in `pre`, so a write
  noted while recording always clears a hash set by that submission's sync. vpool `translate` failure: no binding is
  made either (`resolve_in`), so nothing to forget. Unwatched mirrors already diff against their snapshot. Hash:
  xxHash64 round/merge (P3 for P4 in the merge, byte tail with P3: still 64-bit avalanche), length mixed in, never 0;
  collision odds ~2^-64 per compare, acceptable; partial pages always uploaded and their hash cleared. Locks: `forget`
  takes `H` without the pool lock; `changed_runs` takes it under the pool lock: no inversion.
- *exec.rs indirect c0:* staged with `stage_bytes_fresh` (not memoised), the bank's `staged` cache untouched;
  `set_key` → None, so no `hit_version`, no `patch_set`, no `store_version`; `write_db_set` clears the shadow's key on
  the whole build, so no later direct dispatch patches from the private set; non-descbuf `memo_sets` keys by buffer +
  offset (private offset is unique). `banks` runs once per compute set in `bind_sets`; `prepare_storage_bindings`'
  call precedes the flag. Copy: 12 bytes to 0x200 always and 0x4 for NVN2, as `put_dispatch`; staging usage has
  TRANSFER_DST; source is the clamp output after the clamp's full barrier (or the pool record after the previous
  dispatch's full barrier); copy → full barrier → dispatch. Under pass dump / scaling `need` is forced (true,true) and
  `set_key` is None: consistent. `NXGFX_INDIRECT_C0=0`: flag never set, identical to the old path.
- *dump-only:* the blend constant, clear target, render-condition lines are inside `passdump::active()`; the
  render-condition counters use `rc_value`, the same lookup the live path already makes; the uniform/storage reads
  skip sparse virtual addresses before `room`. No cost when not dumping. `uptrace` is behind its OnceLock.
- *image.rs M1 gap:* the condition matches `image_view_kind`'s remap (view format emulated → no pool storage view)
  in both `reg_tex_now` and the re-backing loop; see L1 for the leftover slot.
