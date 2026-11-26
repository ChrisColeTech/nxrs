# NVN2-PARITY — Switch 2 titles: pop-in and frame rate (NVN2 gaps against nxnative)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md`. Works in the **main tree** after OOT-FOREST2 has landed. MKW-RECORD runs
beside it in the worktree `D:\Projects\RecompLauncherEngine-mkw` and relinks Mario Kart World there: **do not
relink Mario Kart World from the main tree**; use the other Switch 2 titles for game checks (Donkey Kong Bananza
`dkb`, Ocarina `oot`, and any other 04… title in the library); hand MKW checks to the lead. Respect
`build/locks/machine.lock`.

## Brief (lead)

**Owner report (2026-11-25):** every Switch 2 title misses 60 FPS and has textures popping in and out; every Switch 1
title is fine. Two NVN2-only gaps have already caused visible bugs (dispatch size at c0+0x4, OOT-NAN; indirect
dispatch sizes, OOT-FOREST2). A read-only parity audit against nxnative (`D:\Projects\nxnative\dm`) found more. Fix
them, in this order, each as a general correction with a unit test, checked in game:

1. **Pushed buffer addresses and uniform-buffer handles resolve to nothing** (Switch 2 only).
   - `vk/exec.rs` `bank_word` returns 0 for banks 24-29: return the push data (slot 24: `qs.push[grp]`; 25-29: the
     graphics group's blocks), as nxnative `nvn_mtl.m:13227` (NVN_RB_SBUFQ).
   - `bank_u64` never decodes a handle: apply `ucb_handle_addr` (`nvn_mtl.m:1552`: a word with bits >= 45 set is
     `(v & (2^45-1)) << 4`); with no size, bound to its pool as the size-0 path does.
   - `c0_bytes`: on Switch 2 write compute's uniform table (0x210 + 16r) as `ucb_handle` (`nvn_mtl.m:1551, 13298`),
     size kept at +8; cap the size field in `nvn/query.rs:372` the same way (2^19-1).
2. **`nvnDeviceGetCurrentTimestampInNanoseconds` is on the wrong clock** (`nvn/device.rs:215` `now_ns()`, its own
   epoch): use the guest clock the counter reports and the tick counter use (nxnative `hle_nvn.c:3277` `guest_ns`).
   A frame-rate / dynamic-resolution controller comparing the two held Donkey Kong Bananza at 20 FPS in nxnative.
3. **Two-word texture handles**: the texture ID must come from `word | second_word` (UE5.4 on Switch 2 puts the
   sampler word first), `nvn_mtl.m:12637-12639`; nxrs takes the first word only (`exec.rs` handle kind 1).
4. **Inline uniform updates (`UPDATE_UB`)**: first count them per frame on a Switch 2 and a Switch 1 title. Then:
   CPU-side handle/address reads from uniform banks must see this submission's queued updates (nxnative `ubo_read`,
   `nvn_mtl.m:12619-12628`), and an update should end the render pass only when a later draw's GPU read overlaps it
   (nxnative defers inside a pass, `nvn_mtl.m:12854`). **Coherence change: audit.**
5. **Uniform size rounded up to 256** (NVN2 method 0xe02, `nvn_mtl.m:12846`): `ubo_info` range and `bank_word`
   bound use `min(round256(n), room, max_ubo)`.
6. **Sparse tiles**: check the logs of the Switch 2 runs for "tile memory over NXGFX_SPARSE_MIB" / "no memory for a
   tile"; if present, keep tile slots alive while the physical pool lives instead of per mapping (`vk/sparse.rs`
   443-556), as nxnative keeps physical memory.
7. **Small items**: device item 0x06 answers 16 (or accept 32 vertex buffers), 0x62 answers 32.
8. **Render size check (A/B, no code):** docked on a 4K display both projects answer 3840x2160 with scale tier 0, so
   Switch 2 titles render at their maximum. Measure one title with `NXOS_DISPLAY_MAX` / `NXOS_GFX_SCALE` lowered,
   to tell how much of the FPS gap is render size; report it, change no defaults.

Out of scope here (recorded for later waves): DLSS as a temporal upscaler (today a plain blit: depth, motion, jitter
not recorded), ray tracing, the c0 sample table / point size.

**Done:** items 1-3, 5, 7 fixed with tests; 4 and 6 measured and fixed or explained; 8 measured. Game evidence on
`dkb` and `oot` (sheet read; FPS vs 60 / 59 and 5% low; pop-in watched across consecutive shots, before vs after),
plus one Switch 1 regression (`wonder` or `smash`) unchanged. `cargo test -p graphics --lib --release`.

**Descriptor/memory/coherence changes: the lead runs an independent audit.** Never skip draws; don't weaken
robustness; native NDVE stays off. **Exclusions:** `vk/queue.rs` and the replay split (MKW-RECORD's), no MKW relinks,
no commits, don't edit docs/32. Back up and restore anything you move, and check the direction.

**Deadline:** 6 hours wall clock. Report per protocol §5 in this file under "## Report".

## Report

(agent, 2026-11-25; builds on HEAD 5489c92, dist built 20:09 with HEAD's upload-skip-off default)

### Built

1. **Pushed buffer addresses / uniform handles** (`vk/exec.rs`, `nvn/device.rs`, `nvn/query.rs`).
   Cause: `bank_word` answered 0 for banks 24-29, so every storage buffer whose address a Switch 2 shader takes from
   its push constants (nxnative NVN_RB_SBUFQ) was bound to the zero / scratch fallback; `bank_u64` never decoded a
   hardware bindless handle; compute's c0 uniform table held plain addresses where NVN2 shaders decode handles.
   Fix: `push_bank` gives c24 (the stage's group, compute = group 1) and c25-c29 (graphics group blocks 1-5) exactly as
   `push_info` binds them; `bank_u64` decodes `ucb_handle_addr` on NVN2 only (`decode_record_addr`: NVN1 words stay
   as they are) and returns size 0 for a handle or a pushed address (the size-0 path binds to the pool end, capped);
   `put_cs_ubos` writes `ucb_handle(a, n)` at 0x210 + 16r on NVN2, size kept at +8; `device::ucb_handle` caps the size
   field at 2^19 - 1 and `nvnDeviceGetUniformBufferHandle` uses it. The set cache keys and patches such a storage
   buffer by the push constants (`set_deps` / `res_class`: Sbuf in 24..=29 -> push / CL_PUSH).
2. **Current timestamp clock** (`nvn/device.rs`): `nvnDeviceGetCurrentTimestampInNanoseconds` returns
   `current_timestamp_ns()` = `counters::guest_ns()` (the OS guest clock the reports and the tick counter use), /256
   when `game_dynamic_resolution=0` as the reports are. It was `now_ns()`, graphics' own epoch.
3. **Two-word texture handles** (`vk/exec.rs`): `handle_word` ORs both words of a kind-1 handle (texture and sampler
   in either order); used at all five CPU-side handle reads (descriptor build, storage-image prep, scale table, DKB
   NDSR copy, compute image IDs); the sampler ID of kind 1 is the ORed word >> 20.
4. **Inline uniform updates**: counted (`perf` UbUpdates / UbPassEnds, in the "replay up to frame" line). Fixed the
   CPU-read half: `QState::ub_upd` keeps this submission's resolved UPDATE_UB bytes in order; `bank_word` on c3..c18
   overlays them (byte-wise, later wins); cleared in `exec::end`. Deferral inside a pass: **not done, explained** by the
   counts (below: 2 pass ends a frame in DKB).
5. **Uniform size rounded to 256** (`vk/exec.rs` `ub_bound`): `ubo_info` range and the `bank_word` bound are
   `min(round256(n), room (unless the binding itself runs past its pool), cap)`; applied to both NVN versions, as
   both drivers write the rounded size (NVN1 0x8e0, NVN2 0xe02; nxnative `ub_bound`).
6. **Sparse tiles**: measured, nothing to fix: DKB (before and after) and OoT logs have no "tile memory over
   NXGFX_SPARSE_MIB" / "no memory for a tile" lines; both titles bind 0 sparse texture tiles ("0 tiles live").
7. **Device items** (`nvn/device.rs` `device_integer`, split out of nvnDeviceGetInteger): Switch 2 item 0x06 answers
   16 (the host's tables hold 16 vertex buffers; forced even if an sdk constant says otherwise); run-time (`RT`)
   items take the X1 table's value wherever it has one (0x62 -> 32, as nxnative), not only below 0x60; an sdk
   constant still wins.
8. Lead's LOW (image.rs): `reg_tex_now`, the re-backing loop and `rescale_images` now give a slot with no storage
   view (no storage use, emulated storage format) the zero stand-in storage view of its target
   (`storage_or_stand_in`, as the placeholder paths already do) instead of leaving the previous view; `rescale_images`
   passed None always and now keeps the image's storage view like the re-backing loop.

### Tests

`cargo test -p graphics --lib --release`: 220 passed, 0 failed (6 ignored, pre-existing). New tests (each exercises
behaviour that was 0 / wrong before; they do not compile against the old code because the helpers are new):
`exec::set_patch_tests::pushed_buffer_addresses_and_handles_resolve` (c24/c26 words, pushed address without size,
handle decode NVN2-only, push keying), `nvn2_compute_uniform_table_holds_handles`, `two_word_texture_handles_or_both_words`,
`uniform_reads_see_this_submissions_inline_updates`, `uniform_bindings_round_up_to_256`;
`nvn::device::tests::switch2_items_the_host_answers`, `uniform_buffer_handles_round_trip` (PRAGMATA's pushed handle
0x5a0083055d124 = 0x83055d1240, size field 0x2d); `vk::counters::tests::the_current_timestamp_is_the_reports_clock`.

### Verified

Each FPS run held `build/locks/machine.lock` (no other game or build of mine during it). Build: `scripts/build/build.ps1
-KeepCache -SkipGnu` (dist 20:09), relinked DKB, OoT, Wonder. **Before vs after is not purely this wave:** the
"before" builds had the upload skip on; the "after" build has HEAD's skip-off default (lead's 5a41213).

| run (GPU) | FPS mean / 5% low / min in window | target |
|---|---|---|
| dkb before `build/play/20261125-200446-nvn2-dkb-before` (1) | 58.7 / 53.0 / 45.4 | 60 |
| dkb after `20261125-201034-nvn2-dkb-after` (1) | 59.2 / 53.8 / 52.8 | 60 |
| oot before `20261125-192048-oot-f2-final` (1; OOT-FOREST2's run, not mine) | 13.7 / 10.0 / 4.3 | 59 |
| oot after `20261125-201346-nvn2-oot-after` (1) | 15.0 / 8.7 / 1.0 | 59 |
| wonder (Switch 1, SDK 16) before `20261125-194749-wonder-f2` (0) | 59.5 / 57.1 / 26.0 | 60 |
| wonder after `20261125-202911-nvn2-wonder-after` (0) | 60.0 / 59.9 / 58.0 | 60 |
| oot A/B `NXOS_DISPLAY_MAX=1` `20261125-203246-nvn2-oot-1080` (1) | 13.1 / 10.0 / 9.0 | 59 |

- Sheets read: DKB before/after identical scenes (boot, continue, walk, punch, the tutorial cutscene, DK close-up),
  no red lines; OoT after reaches Link in his house, walks (sheet-2, 13-x5 .. 24-walk4) exactly as the baseline;
  Wonder after: title, course, gameplay, all normal. **Pop-in**: none seen across consecutive shots in either OoT run
  (h1..h7 hold the same textures shot to shot) or in DKB; the shots did not show the owner's pop-in before either, so
  these scenarios cannot prove a pop-in fix.
- **UPDATE_UB counts** (per frame, steady state): DKB 17 (2.0 ending a pass), OoT house 80-87 (3-4.4 ending a pass),
  Wonder 50 (2.0). At 2-4 pass ends a frame among 70-190 passes, deferring updates inside a pass is not where the
  frame time goes; not done.
- **Render size A/B (item 8)**: `NXOS_DISPLAY_MAX=1` changes nothing in OoT: its passes stay 2560x1440 (its swap
  chain is 2560x1440 from the start; neither OoT nor DKB logs a vi buffer-max query), FPS 13.1 vs 15.0 (noise). OoT's
  slow frames in the window (~70 ms) are CPU: replay 24-27 ms and pool sync 26-35 ms, GPU 10-11 ms (11 submissions).
  DKB already sizes itself (2880x1620 down to 2095x1179 with its own controller) and runs at 59.2. So render size is
  not the Switch 2 FPS gap in these two titles; OoT's gap is the replay and the pool sync (prep "pools").
- Sparse (item 6): no over-cap / no-memory lines in any of these runs; 0 sparse tiles live in DKB and OoT.

### Unproven

- No run shows a title whose output changed because of items 1, 3 or 5: DKB / OoT render as before. Whether the
  owner's pop-in is gone is not proven (the scenarios never showed it); MKW (the obvious NVN2 pop-in title) is the
  lead's to check, and the SBUFQ / two-word-handle paths are most likely to matter for UE5 / PRAGMATA-style titles.
- The DKB "20 FPS controller" (item 2) did not reproduce in nxrs (DKB was at 58.7 before); the fix is the parity
  correction, its effect not visible here.
- The ub_upd overlay only lives for one submission; a later submission that replays before the GPU wrote the update
  back to a mirrored pool still reads the old memory (nxnative has the same window). A cached stage set whose
  storage-buffer address comes from a uniform buffer is keyed by the binding, not its contents (pre-existing).
- `storage_or_stand_in` falls back to no write (the old behaviour) if the dummy image is not made yet.
- OoT FPS gap (15 of 59) is CPU: replay ~25 ms + pool sync ~26-35 ms a frame; next step is a profile of
  `mem::begin_submit` (pools) in the house scene.
- 5% low of OoT after (8.7) vs before (10.0) and min 1.0: one long hitch in the window; the baseline was not run by
  me, so whether it is a regression needs a same-build pair (it is on the CPU side, where this wave changed only the
  UPDATE_UB bookkeeping and `room()` lookups per uniform bind).

### Slips

- My first post-fix `build.ps1` ran while another wave held `dist.lock` (my `New-Item` failed but the script went
  on). The lock was released 20:08 right after; I then took it properly and rebuilt (20:09) before any relink, so
  every run above uses the 20:09 dist.
- The three relinks were started together (the launcher returned at once and built them in parallel); I briefly
  released `dist.lock` before they finished, then took it again until they had finished.
- OoT "before" is OOT-FOREST2's own run, not a run I made under the machine lock.
- The final `cargo test -p graphics --lib --release` (220 passed, image.rs included) ran at ~20:48-20:49 while
  MKW-RECORD held `machine.lock` (taken 20:48:14); I did not check the lock immediately before it. It was an
  incremental test build of one crate, but it overlapped MKW-RECORD's measurement window: their numbers from
  20:48-20:49 may carry it.

### For the lead to audit

Descriptor / coherence changes: `bank_u64` (handle decode, size 0 for pushed / handle addresses -> written storage
buffers bound to the pool end, capped by `sbuf0_cap`), `put_cs_ubos` (NVN2 c0 table now holds handles: any NVN2
shader path that read 0x210+ as a plain address on the GPU would now see a handle), `ub_bound` (uniform descriptor
ranges grow to the next 256 within the pool, both NVN versions), the `ub_upd` overlay, the set-cache keying of
push-sourced storage buffers, and `storage_or_stand_in` in image.rs.

Changed files: `src/graphics/src/vk/exec.rs`, `src/graphics/src/nvn/device.rs`, `src/graphics/src/nvn/query.rs`,
`src/graphics/src/vk/counters.rs`, `src/graphics/src/vk/perf.rs`, `src/graphics/src/vk/image.rs`, this file.

### Questions

None.
