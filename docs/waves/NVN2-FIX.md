# NVN2-FIX — the NVN2-PARITY audit's findings

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md`. **Main tree.** MKW-RECORD works in the worktree
`D:\Projects\RecompLauncherEngine-mkw`: never relink or run Mario Kart World from here, don't edit `vk/queue.rs` or
the replay split, respect `build/locks/machine.lock` (and check it immediately before starting a measured run).

## Brief (lead)

The independent audit of NVN2-PARITY (commit `78c7f82`; "Independent audit of 78c7f82" in
`docs/waves/NVN2-PARITY.md`) found four MEDIUM issues; two of them can bind storage to stale or zero data, which is
what texture pop-in looks like. Fix each as a general correction with a unit test:

1. **Set cache vs inline uniform updates** (`vk/exec.rs:515`): a storage buffer whose address the shader reads from
   a uniform buffer (c3-c18) is keyed by the binding only, so a set built before an `UPDATE_UB` that changed the
   address is reused after it. Key such sets by a uniform-update counter (or the resolved address).
2. **Size-0 pushed / handle-addressed storage buffers in sparse pools** (`exec.rs:4178`): the size runs to the end
   of the physical pool, `resolve` fails at the first unmapped page and the whole binding falls back to the zero
   buffer (nxnative's PRAGMATA / Resident Evil 4 case: a 0xc0000 run mapped at the base of a 1 GB sparse pool).
   Bound the size to the contiguous mapped run.
3. **A decoded handle's size is discarded** (`exec.rs:3194`): use `(v >> 45) * 16` (buffers > 1 MB are cut off
   today; small written buffers are noted GPU-written over up to 1 MB each submission).
4. **Stand-in storage views in the storage-view list** (`vk/tables.rs:985`, walked per draw at `exec.rs:5695`):
   write the stand-in descriptor but keep it out of the list.
5. LOWs, if cheap: on Switch 2 always decode compute c0 rows 0x210-0x310 (a size-0 handle looks like an address);
   reset the global guest clock in the counters test.

**Game checks:** `dkb` and `oot` (sheets read, FPS vs 60 / 59, red lines), `wonder` (Switch 1, unchanged).
`cargo test -p graphics --lib --release`. **Descriptor change: the lead audits the result.** No draws skipped, no
robustness weakened, native NDVE off, no commits, don't edit docs/32. Deadline 3 hours. Report under "## Report".

## Report

(agent, 2026-11-25; on HEAD 13a148f, tree clean before this wave; dist built 21:08, DKB / OoT / Wonder relinked
21:08-21:10)

### Built

1. **Set cache vs inline uniform updates** (`vk/exec.rs`). Cause: `SetKey` held a uniform buffer's binding
   `(a, n)` only, so a set whose storage-buffer address is read from c3..c18 *memory* was reused (`hit_version`,
   `patch_set` with `old == k`) after an `UPDATE_UB` rewrote that address under the same binding. Fix: `QState::ugen`,
   a new generation at every `UPDATE_UB` (before the copy, whether it resolved or not); `SetDeps::ubo_read` (bit per
   c3..c18 a `Res::Sbuf` reads its record from); `SetKey::ugen = (ugen, ubo_read)` for such sets, `(0, 0)` for all
   others (`ub_read_key`), so sets that only bind uniform buffers keep their reuse. `patch_classes` remakes the
   `ubo_read` classes when the generation differs (the Sbuf slots re-resolve through `bank_u64`, which sees the
   `ub_upd` overlay); `note_set_miss` counts it under "uniforms read as addresses".
2. **Size-0 / sparse storage buffers** (`nvn/vpool.rs`, `vk/exec.rs`). Cause: the size-0 path took `room(a)` (the
   *physical* pool window behind the first page) capped at 1 MB; `resolve` needs every page mapped and contiguous, so
   one unmapped page sent the whole binding to the 64 KiB zero / scratch buffer. Fix: `vpool::run_len(a, max)`, the
   mapped, physically contiguous run from `a` (bounded by the pool's size and `max`); `exec::sparse_bound(a, n)` holds
   every storage binding in a sparse pool to that run (size 0, a handle's size, a record's size alike; kept when `a`'s
   own page is unmapped, i.e. no binding as before). Past the run the shader reads 0 / writes drop
   (robustBufferAccess2), as unmapped sparse memory does. Non-sparse bindings unchanged.
3. **A decoded handle's size** (`vk/exec.rs`, `nvn/device.rs`). `bank_u64` now goes through `record_addr_size`: a
   decoded NVN2 handle returns `(addr, (v >> 45) * 16)` (`device::ucb_handle_size`); only a pushed plain address
   (c24..c29) keeps size 0 = to the pool end. New perf counter `SbufSize0Written` ("written storage buffers with no
   size" in the "replay up to frame" line) shows the over-wide GPU-write notes that remain.
4. **Stand-in storage views off the list** (`vk/tables.rs`, `vk/image.rs`). `write_image` takes `own`; the new
   `tables::list_storage` lists a slot's storage view only when it is the image's own. `storage_or_stand_in` returns
   whether it fell back to the stand-in; the lazy-registration and `scrub_slots` stand-ins pass `own = false`. The
   descriptors written are unchanged; only `storage_views` (walked per draw of a storage-pool shader, whose only
   consumer `note_storage_write` already ignored stand-ins) shrinks back to real storage images.
5. **LOWs.** (a) On NVN2, compute's c0 rows 0x210 + 16r (to 0x310, row starts only) are always decoded as
   `ucb_handle`s in `record_addr_size`, so `ucb_handle(a, 0) = a >> 4` is no longer read as a plain address.
   (b) `counters.rs`: `guest_ns` checks a test-only thread-local clock first; the timestamp test sets and restores
   it, the process-wide `GUEST_CLOCK` is no longer set by any test.

### Tests

`cargo test -p graphics --lib --release`: **224 passed, 0 failed, 6 ignored** (pre-existing ignores). New / changed:

- `exec::set_patch_tests::inline_updates_renew_sets_reading_addresses_from_uniform_memory`: an Sbuf read from c5
  gives `ubo_read = 0b100`; a bump of `ugen` changes the key, a Cbuf-only set keeps `(0, 0)`; `patch_classes` remakes
  class `0b100`. (Before: no `ubo_read` / `ugen`; the key was identical across the update.)
- `a_patch_remakes_every_slot_whose_inputs_changed` (+ the `ugen` case), `the_key_covers_only_what_the_set_reads`
  (`ubo_read` asserted).
- `handle_records_keep_their_size_and_the_compute_table_always_decodes`: a 3 MB handle keeps 3 MB (before: 0 -> 1 MB
  cap), a pushed address 0, plain / NVN records their size word, a size-0 handle at c0+0x280 of an NVN2 compute set
  decodes to its address (before: `a >> 4`), not outside the table / for graphics / NVN.
- `sparse_storage_bindings_cover_the_mapped_run`: a 0xc0000 run mapped at the base of a 4 MB sparse pool: 1 MB does not
  translate (the old binding), `sparse_bound` gives 0xc0000 which translates; inside-run / unmapped / non-sparse kept.
- `nvn::vpool::tests::alias_then_sparse_page_table`: `run_len` stops at an unmapped page, at a page mapped elsewhere,
  at the cap; 0 for an unmapped page or no sparse pool.
- `tables::registration_boundary_tests::stand_in_storage_views_stay_off_the_storage_list`: own view listed, stand-in
  and none not, own replaced by a stand-in removed (before: the stand-in was listed).
- `counters::tests::the_current_timestamp_is_the_reports_clock`: now also asserts the clock is restored.

The new tests do not compile against the old code (new helpers / fields), so "failed before" is by construction.

### Verified

Build `scripts/build/build.ps1 -KeepCache -SkipGnu` under `dist.lock` (21:07:49-21:10:35, held through the three
relinks, each waited for by PID; exes 21:08:40 / 21:09:27 / 21:10:16). `machine.lock` checked immediately before the
tests and before each run (MKW-RECORD held it 21:01-21:07, and again when I checked after the DKB run until 21:19; I
waited both times). Runs one at a time, nothing else of mine running. These are FPS-vs-target readings, not a
before/after pair, and were not taken under my own `machine.lock`.

| run (GPU) | FPS mean / 5% low / min in window | target | red lines |
|---|---|---|---|
| dkb `build/play/20261125-211039-dkb` (1) | 59.3 / 53.0 / 53.0 | 60 | 0 |
| wonder `build/play/20261125-211913-wonder` (0) | 60.0 / 59.0 / 58.6 | 60 | 0 |
| oot `build/play/20261125-212250-oot` (1) | 14.9 / 8.0 / 6.0 | 59 | 2 (both pre-existing) |

- DKB sheet: boot (mode select), Continue, walk, punch, the Y-Punch tutorial, the cutscene, DK close-up: the same
  scenes as NVN2-PARITY's after-run (59.2 / 53.8), nothing missing or black. Counters: 17.0 uniform updates a frame
  (2.0 ending a pass), 2.2 written storage buffers with no size a frame, 1,044 sets (1,203 reused, 877 patched)
  against 1,064 draws; 0 sparse tiles live (sparse paths not exercised by DKB).
- Wonder (Switch 1) sheet: title, course start, gameplay, later gameplay, end: normal; 0.0 written size-0 buffers.
- OoT sheets 1-3: title, story tapestry, nightmare, house (white fade), forest load (black), Deku Tree, Navi,
  Link woken, the house h1..h7 (the same textures shot to shot: no pop-in seen), walk1..walk9, last: the same route
  and pictures as NVN2-PARITY's after-run (15.0 / 8.7 / 1.0) and OOT-FOREST2's (13.7 / 10.0 / 4.3). Red lines: the
  same two as both baselines (one "draw with no render target bound" at ~337 s, one blacklisted pipeline
  `pipeline-crashed.bin` at ~346 s). Counters: 80.0 uniform updates a frame (4.1 ending a pass), 0.0 written size-0
  storage buffers, 886 reused / 2,776 patched sets a frame.

### Unproven

- No scenario here exercises items 1-3 visibly: DKB / OoT / Wonder render as before. The sparse run (item 2) and the
  handle sizes (item 3) are what PRAGMATA / RE4-style titles and UE5 hit; none is in the scenarios. Whether the
  owner's pop-in is gone is not shown (MKW is the lead's).
- Item 1 bumps `ugen` on every `UPDATE_UB` (not only one overlapping a bound c3..c18 range, which would miss a set
  built before a rebind): sets that read storage addresses from uniform memory are remade after each update. DKB's
  reuse stayed high (1,203 reused / 877 patched of 1,044 a frame); MKW's count is not measured here.
- A sized handle whose rounded-up-to-16 size runs past the end of a non-sparse pool still fails `resolve` (zero
  buffer, red line) as before; not clamped, to not hide a wrong size.
- Mapping changes during a submission's replay: a set keyed in the submission keeps the run it saw (pre-existing,
  as `translate`).

### For the lead to audit

`SetKey::ugen` / `patch_classes` / `set_deps` (item 1), `sparse_bound` applied to every storage binding after the
to-end step (item 2), `record_addr_size` (items 3, 5a), `write_image(.., own)` and `list_storage` (item 4).

Changed files: `src/graphics/src/vk/exec.rs`, `src/graphics/src/vk/tables.rs`, `src/graphics/src/vk/image.rs`,
`src/graphics/src/vk/perf.rs`, `src/graphics/src/vk/counters.rs`, `src/graphics/src/nvn/vpool.rs`,
`src/graphics/src/nvn/device.rs`, this file.

### Slips

None of mine. For the lead: `machine.lock` was free when DKB started (21:10:39) and held by MKW-RECORD when I
checked right after it ended (~21:13:20), so MKW-RECORD's measurement may have overlapped the tail of my DKB run.

### Questions

None.

### Independent audit of NVN2-FIX

(auditor, 2026-11-25; read-only review of `40ff148` `git show -- src/`, against the code at HEAD and nxnative
`dm/gfx/nvn/nvn_mtl.m`; no builds, no games.) **No HIGH. One MEDIUM (a regression the report calls "as before"),
six LOW.**

**MEDIUM 1: a handle's size past its pool's end now loses the whole binding** (`vk/exec.rs:3265-3266`, used at
`exec.rs:4244`). Before this commit a decoded handle had size 0 and took `room(a).min(1 MB)`, which always resolves.
Now it takes `(v >> 45) * 16` as is. `sbuf_to_end` (`exec.rs:4266`) can only make the size larger, and `sparse_bound`
leaves non-sparse sizes alone, so a handle whose size runs past the end of its physical pool (or past the end of a
large pool's window, see `mem::room`) now fails `resolve`. The shader then gets the 64 KiB zero or scratch buffer
for the whole binding (red line), where it used to read the real data. This is the pop-in symptom the wave set out to
fix. Two cases make it likely: a game makes its handles with a fixed size (64 KB, the largest constant buffer) for
buffers near a pool's end, or `ucb_handle` rounds the size up past the end. The report's "Unproven" line ("still fails
`resolve` ... as before") is wrong for handles: only plain sized records failed before. nxnative does not use a
handle's size for these bindings either. `NVN_RB_SBUFU` / `NVN_RB_SBUFQ` (`nvn_mtl.m:13220-13232`) decode the address
and bind "size not known: bounded by the buffer that holds it". Fix: for a decoded handle outside sparse pools, use
`n = handle_size.min(room(a))` (note it once when it clamps). This keeps the smaller write range the item wanted and
cannot lose the binding. Add a test where the handle's size runs past `room`.

**LOW 2: a saturated handle size is taken as exact** (`exec.rs:3266`, `nvn/device.rs:168`). `ucb_handle` caps the
field at 2^19 - 1, so `0x7ffff * 16` (8 MB - 16) means "8 MB or more". A handle-addressed written buffer over 8 MB is
cut there and the GPU's writes beyond it are dropped. That is still better than the old 1 MB cut, so it is not a
regression, but treating the cap as exact is not right. Fix: when `v >> 45 == (1 << 19) - 1`, take the size-0 path
(to the pool end), or `room(a)`.

**LOW 3: the mapped run does not stop where the physical pool ends** (`nvn/vpool.rs:359-378`, `exec.rs:2916`).
`run_len`, like `translate`, checks only that the host pointers are contiguous. Suppose two of the game's physical
pools lie back to back in host memory and the sparse pool maps consecutive pages from both. The run then crosses
the pool boundary, `mem::find` fails in `resolve_in`, and the whole binding falls to the zero buffer. Size-0
bindings are already held to `room(a)`, which goes through `translate` to the physical pool. Sized, handle and
record bindings are not. Fix: in `sparse_bound`, `min` the run with `mem::room(a)` (it translates sparse addresses
already).

**LOW 4: the doc comment overstates what lies past the run** (`exec.rs:2911-2915`). "Past the run the shader reads 0 /
writes drop, as unmapped sparse memory does" is true only when the run ends at an unmapped page. When it ends at a page
mapped to other, non-contiguous physical memory, the console reads and writes real data there. Here, reads get 0 and
writes are lost (no write-back is noted for them). This still beats the whole binding on the zero buffer, so it is a
known limitation, not a regression. Say so in the comment. A full fix needs a sparse `VkBuffer` or a per-page binding.

**LOW 5: sparse bindings walk the page table three times** (`exec.rs:4278`, `mem.rs:1453`, `mem.rs:2132`).
`sparse_bound` (via `is_sparse` + `run_len`), `resolve_in` (`translate`) and `note_gpu_write` (`translate` again) each
take the `POOLS` read lock and walk every page of the binding. That is 256 steps for 1 MB, and tens of thousands for
a large sized binding, at every set build or patch of a sparse-pool shader. The lock is read-only and only sparse
titles pay, so this is not urgent. Fix, if PRAGMATA- or RE4-class titles show it: have `run_len` return the
physical start too and pass the translated address on, or cache `(a, n, vpool::generation()) -> run`.

**LOW 6: `ugen` changes at every UPDATE_UB** (`exec.rs:9095`, `exec.rs:550-556`). This is correct and conservative.
Every set that reads an address from c3..c18 memory misses after any update, even one to unrelated memory. The cost
is one patch of only the `ubo_read` slots: `bank_u64` + `room` + `resolve` per such storage buffer. Every UPDATE_UB
already ends the render pass, which costs far more, so I expect little cost at MKW scale. MKW's reuse was not
measured. Cheaper option, if MKW shows "uniforms read as addresses" high in `note_set_miss`: key such sets by the
8-byte address words they read (through `ub_upd.word`) instead of the generation. A set is then reused whenever the
address did not change, which also covers a rebind.

**LOW 7 (pre-existing, not this commit): a sparse remap inside a submission does not invalidate cached sets.**
`vpool::map` changes neither `POOL_GEN` (which `hit_version` / `patch_set` check) nor anything in `SetKey`. A set reused
or patched in the same submission therefore keeps its old translation, and now its old run length too (the report
says this). If it matters: add `vpool::generation()` to the key of sets that bind sparse addresses, or bump
`POOL_GEN` on a sparse map.

**LOW 8 (pre-existing, same as nxnative):** outside compute's c0 table, a size-0 handle (`ucb_handle(a, 0) = a >> 4`)
cannot be told apart from a plain address and is read as `a >> 4`. `ucb_handle_addr` in nxnative behaves the same.
I know of no game that pushes or stores a size-0 handle.

**Checked and found correct:**

- (1) `set_deps` sets a `ubo_read` bit only for a `Res::Sbuf` whose `cb_slot` is 3..18, and the same bit is in `ubo`,
  so the binding stays keyed. `ub_read_key` gives `(0, 0)` to every other set, so sets that only bind uniform
  buffers keep their reuse. `new_bgen` starts at 1 and is a shared u64, so a reading set never gets generation 0,
  values never repeat across queues, and it cannot wrap in practice. `ugen` changes before the `resolve`. An update
  that does not resolve adds nothing to `ub_upd` but still changes the key, which costs at most a miss. `SetKey`
  equality covers `hit_version`, the `old == k` reuse in `patch_set`, and the key recomputed after `stage_set`
  (`exec.rs:5943`). `patch_classes` adds exactly `new.ugen.1`: the Sbuf slots `res_class` maps to the same bit
  (`1 << (cb_slot - 3)`) re-read through `bank_u64`/`bank_word`, which applies the `ub_upd` overlay. The key holds
  `serial` and `chunk`, and `end` clears `ub_upd`, so nothing carries across submissions. `QState` (`sets`, `shadow`,
  `ugen`, `ub_upd`) is per queue. Indirect dispatches with a private c0 are never keyed. The non-descriptor-buffer
  path keeps no set cache. `UPDATE_UB` is the only op that pushes to `ub_upd`. Writes to uniform memory by copy, fill
  or a shader were outside the overlay before and still are (nxnative reports the same case red, `ubo_read`
  RED_ONCE 47).
- (2) `run_len` returns 0 for no pool, an unmapped first page, or an offset past `size`. It stops at the first page
  that is unmapped or non-contiguous, and at the pool's `size` and at `max`. The value matches what `translate`
  accepts. Alias pools are dropped from `POOLS`, and physical pools were never in it, so `sparse_bound` leaves both
  unchanged. Undecided virtual pools have no PTEs, so `run_len` is 0 and the size is kept, as before. `sparse_bound`
  runs after the size-0 and to-the-end steps and before `append::bound`, `resolve` and `note_gpu_write`, so the bound
  range, the resolved range and the write-tracking range are the same `n`. A run ends on a page boundary.
  `robustBufferAccess2` covers the cut-off part where the device enables it (`vk/device.rs:740`). The `Res::Sbuf`
  arm of `stage_set` is the only place that binds a game storage buffer by address. Raw gmem, `MeshDraw`, transform
  feedback and vertex buffers are separate paths, outside this item.
- (3) `ucb_handle_size` is the inverse of `ucb_handle`'s field: a multiple of 16, at most 8 MB - 16. A plain address
  never has bits at 45 or above, so it gets the record's size word as before. Pushed plain addresses (c24..c29) keep 0.
- (4) `write_image` has five callers. Lazy registration (`image.rs:2387`) and `scrub_slots` (`image.rs:3196`) write
  stand-ins and pass `false`. `reg_tex_now`, the alias rebuild and the recreate path (`image.rs:2516/2926/3126`)
  pass `own` from `storage_or_stand_in`, which is `true` exactly when the view is the image's own `storage_view`.
  `list_storage` runs before the early return on an unchanged descriptor. `storage_views` has a single consumer,
  `note_storage_write`, and its own `real` test (`im.views` contains the view) already ignored stand-ins. No real
  storage view drops off the list, and no descriptor that is written changes.
- (5) c0 rows 0x210 + 16 r (r < 16, ending 0x310) are written only by our own `put_cs_ubos`: a `ucb_handle` on NVN2
  when `a != 0`, otherwise 0. A game cannot write a plain address there. The check takes row starts only, compute
  only, NVN2 only, which matches nxnative `DMSH_C0_CS_UBO` (`nvn_mtl.m:13298`, `if (ounce) a = ucb_handle(a, n)`).
  The size taken there is the uniform buffer's bound size rounded up to 16, which is what nxnative's `NVN_RB_UBOP`
  binds (`u->size`).
- (6) `TEST_CLOCK` and its check in `guest_ns` are `#[cfg(test)]`, so release builds are unchanged. The test now
  restores the clock and never sets the process-wide `GUEST_CLOCK`.
- `perf`: `SbufSize0Written` sits before `N`, its name is in `NAMES`, and the format string and its arguments both
  gained one entry.
