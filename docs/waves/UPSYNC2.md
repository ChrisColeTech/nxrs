# UPSYNC2 — the second audit of the upload-sync code (commit 2f29f99)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26, worktree `D:\Projects\RecompLauncherEngine-upsync2`,
branch `upsync2` (base `ade9a6c` on `gameplay-harness`). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of
Time only; GPU 1. Deadline 3 h. Background: `docs/waves/UPLOAD-SYNC.md` (the wave, its audit, "## Audit fixes").

## Plan (agent, before coding)

Files expected: `vk/upskip.rs` (most), `vk/lazy.rs`, `vk/mem.rs` (`release`, `register`, `apply_writebacks`),
`vk/queue.rs` (`Post::Early` gets the submission's timeline value), `vk/gmem.rs` (the raw log passes it on).

1. **HIGH-1, a copy's source sync mid-recording keeps a hash the GPU no longer holds.** `upload_changed` step 3: a
   page stamped by the recording in progress (`stamp == EPOCH`, noted by a draw already recorded) is still uploaded, but
   keeps its stamp with hash and parked hash 0 instead of `COPYING`; step 5 then keeps no hash for it, and its landing
   re-arms it with the hash of the GPU's bytes (parked hash 0: never "unchanged"). Applies to partial pages too (they
   lost the stamp before, harmless but it also broke `gpu_writes`' same-range filter's assumption). Test: the audit's
   interleaving: `gpu_writes(P)`, `upload_changed(P)` with U, `landed(src=G, before=U)` -> hash h(G); the CPU writes U
   again -> uploaded.
2. **MEDIUM-2, brought-back records cleared too eagerly.** `read_page` waits for `last_fence()` = V before its copy,
   so the bytes include every submission with timeline value <= V. `Pg::brought_at = V`; `raw_written` (the `Post::Early`
   callback now receives its submission's value) and `landed` (value passed from `apply_writebacks`) clear the record
   only when their submission's value > `brought_at`. `gpu_writes` still clears at note time (its submission is unsent,
   its value > any V read before). Test: a record survives a landing/raw log of a value <= V, and goes with one > V.
3. **LOW-3, the bring-back's gap.** After `protect(p, true)` other threads see the old guest bytes until the copy fills
   the page; a game store there is overwritten. A second writable view needs file-backed guest memory (it is plain
   `VirtualAlloc`), so not feasible here: mitigation in `upload_changed` step 5: a page of the batch brought back while
   it was copied (record set since step 3) has its upload dropped (the copy may hold the old bytes) and, if whole,
   hash = the brought bytes' hash; the comment at `lazy.rs:416` corrected. Runs are built after step 5.
4. **LOW-4.** `COPYING` per call: high bit plus a sequence number.
5. **LOW-5.** `release`: lazy pages brought back first (`touch`), then the entry dropped only if its end matches the
   released pool's; raw logs inside it kept where another entry still overlaps them. `register`'s pre-insert drop goes
   (the replaced pool's `release` does it, after its touch).
6. **LOW-6.** Read whether `take_lazy` can hold a page whose CPU bytes are queued in an unsent submission; fix if cheap,
   else report.
7. **Perf-only.** `landed` no longer clears another pool's stamp (its own landing of the same epoch re-arms it); a
   brought-back skip sets hash = brought on an unstamped page.

Verification: `cargo test --release -p graphics`; OoT on GPU 1: `oot-house --faults` until >= 60 dense shots (two
runs), one `oot`; every sheet and report.md read.

## Report (agent, protocol §5)

### 1. Built

- **H1** (`upskip::upload_changed` step 3): a page whose stamp is the recording in progress (`EPOCH`) keeps the stamp
  with hash and parked hash 0 when it is uploaded (whole or partial), so step 5 keeps no hash for it and its landing
  re-arms it with `page_hash(src)` (the parked hash 0 never counts as "unchanged"). Cause: the upload is recorded into
  `sub.pre`, which runs before the recording's draws, so the GPU copy ends with the draw's bytes; `COPYING` replaced the
  stamp and step 5 kept h(U). Also restores `gpu_writes`' same-range filter assumption (the page stays stamped).
- **M2** (`upskip`, `lazy`, `queue`, `gmem`, `mem`): `read_page` returns the timeline value it waited for; the record
  keeps it (`Pg::brought_at`). `landed` (value passed by `apply_writebacks`) and `raw_written` (`Post::Early` now
  receives the submission's value) clear the record only for a submission with a higher value. A pool becoming raw
  keeps a record its bytes include. `gpu_writes` still clears at note time (its submission is unsent).
- **L3**: a second writable view is not available (guest memory is plain committed memory, no section), so the
  mitigation: step 5 drops the upload of any page of the batch whose record was set since step 3 (a bring-back during
  the copy, which may have read the old guest bytes in the gap); a whole `COPYING` page gets hash = the brought hash.
  Runs are built after step 5. Counted in the pools line (`N brought back while copied, upload dropped`). The comment
  in `lazy::bring_back` now states the gap (reads see old bytes, a store there is overwritten) instead of claiming
  other threads wait.
- **L4**: `COPYING` is bit 63 plus a per-call sequence number.
- **L5**: `release` brings held pages back first, then `drop_pool(base, end)`, which drops the entry only when its end
  matches and keeps raw logs another kept entry still overlaps. `register` drops beforehand only an entry of the same
  extent (it cannot be told apart from the new pool's).
- **L6** (reading): the in-flight write-back guard keeps CPU writes over a write-back in flight in `pending` (both
  `begin_submit` and `sync_range`), and `inflight_add` runs in `submit_sub` before the next recording's syncs, so the
  pool syncs cannot queue such bytes in an unsent submission. One cheap gap fixed: writes the upload budget parked in
  `Pool::deferred` (taken from the watch, not on the device) were not excluded from `take_lazy`; now they are. Not
  fixed: `upload_deferred` (gmem table build, buffer textures) uploads parked writes into the recording without the
  guard; a landing between that and the submit could hold such a page (needs parked writes, 0 MB a frame in OoT now),
  and everything with `NXGFX_WB_GUARD=0`. A full fix would track per page "uploaded by the recording in progress".
- **Perf**: `landed` leaves another pool's stamp alone (its own landing of that submission re-arms it; with none the
  old stamp never matches again, hash stays 0); a brought-back skip sets hash = brought on an unstamped page.

### 2. Tests (`cargo test --release -p graphics`: lib 252 passed, 6 ignored; other targets pass)

- `an_upload_after_a_note_in_the_same_recording_rearms_with_the_gpu_bytes` (H1): note P, CPU writes U, upload,
  repeat note filtered, landing (baseline U, GPU G): guest G skipped, CPU U again uploaded; a partial page uploaded
  after its note is re-armed at its landing. **Fails with the fix disabled** (checked: "guest G = the GPU copy G").
- `a_brought_back_record_outlives_only_submissions_it_includes` (M2): landing 7 / raw log 10 keep a record read at 10
  (left out, reported back); landing 11 / raw log 12 drop it (uploaded). **Fails with the fix disabled** (checked).
- `dropping_a_pool_keeps_what_belongs_to_others` (L5): a nested pool's raw log survives the outer pool's drop; a drop
  with another extent keeps the entry. Fails on the old code by construction (signature and unconditional drop).
- `each_pools_own_landing_rearms_it` (perf): the outer pool's landing first, then the inner pool's own landing
  re-arms it. Fails on the old code by construction (the stamp was cleared).
- Existing upskip tests updated for the new signatures (values that clear, as before) and pass unchanged otherwise.
- L3's race and L4 have no unit test (the race needs a fault inside step 4).

### 3. Verified (GPU 1; OoT relinked from this worktree's dist at 08:58; numbers indicative, shared machine)

| Run (`build/play/`) | FPS / 5% low (window) | route | house shots wrong | notes |
|---|---|---|---|---|
| `20261126-085837-u2-faults-oot-house` | 15.8 / 13.9 | on route (5/5) | 0 of 30 | 11 fault captures, all legitimate content (Deku Tree fog, Navi title card, Saria close-up, cuts) |
| `20261126-091025-u2-faults2-oot-house` | 17.0 / 13.9 | on route (5/5) | 0 of 30 | fault 8: a full black frame at 441.9 s after 9 zero stand-ins in the frame before (the known scene-change stand-in burst, outside upload sync) |
| `20261126-092217-u2-oot` | 17.7 / 15.0 | on route (6/6) | `23-walk3` @854 s: **the white-props frame** | see below |

- Pool line (oot window): upload 1.6-1.8 MB a frame, ~56 MB a frame left out, per-pool pass 6.9-7.4 ms (wave final
  `af-oot`: 1.56 MB, 7.35 ms; FPS 17.8 / 14.0): the skip still works after the H1 change.
- The new counter "brought back while copied, upload dropped" (L3) stayed 0 in all three runs.
- Red lines: the known ones (occlusion query use-up, `pipeline-crashed.bin`, "no render target bound ... depth none"
  twice ~340 s); `oot` also has one `... (0 colour slots, 0 with a texture, depth set): skipped` at 910 s, the line
  the previous wave saw in v2-v5 and not since.
- **`23-walk3` in `u2-oot`**: hanging herbs, baskets, pots and the vines drawn pure white with lightning-like arcs, as
  in `023504-v2-oot/15-h2`, during the walk (Link running, camera moving), no anomaly logged in the 5 s before. The
  same shot in `072706-af-oot` and `081300-af-faults-oot` (previous fix) is clean. So the previous wave's 0-in-90 did
  not mean the cause was gone; this wave's count is 1 in 61 shots of the house. A follow-up `oot --faults` run is
  below.
- `20261126-093834-u2-faults-oot`: stopped by the harness as hung at 378 s: one frame at the forest load took 31.3 s,
  29.6 s of it texture uploads (`u2-oot`'s same frame: 8.4 s, 6.9 s texture uploads); not pool sync. Not counted.
- `20261126-094543-u2-faults-oot-b` (`--stall 90`): **OFF-ROUTE** (FPS invalid). Eleven frames of 6-68 s, mostly with
  the guest idle (no submits, all cores asleep, replay < 10 ms: the game was not waiting on the GPU), from 45 s on,
  so the timed inputs ran ~100 s ahead of the game. Its pictures are all correct, only late. Both this run and the one
  above (09:38-10:01) were slow; the runs before and after on the same binary had one 7-9 s load stall each, as the
  previous wave's runs did: machine-side, not identified (nothing heavy was running when I looked at 10:02).
- `20261126-100211-u2-faults-oot-c` (`--faults --stall 90`): completed, on route (6/6), 18.5 / 13.0 FPS; all 30 shots
  correct, house and walk included; 11 fault captures, the same legitimate set as the dense runs plus the known
  full-black frame after the scene-change stand-in burst (449 s); no capture in the house. Raced-copy counter 0.
- House pictures over the wave: 1 wrong (`u2-oot/23-walk3`) in 96 on-route shots (60 dense, 36 in two `oot` runs).

### 4. Needs audit

- `upload_changed` steps 3 and 5: the stamp kept for a page noted by the recording in progress (`EPOCH` read under
  `H`; relies on `next_epoch` moving only after a recording's last note and on uploads going into `sub.pre`), the
  per-call `COPYING`, the L3 drop of pages whose record appeared since step 3 (and hash = brought for them).
- `Pg::brought_at` and the rule "clear only for a submission value > brought_at" in `landed` and `raw_written`: rests on
  `read_page` waiting for `last_fence()` before its copy, and `q.last` being stored after `vkQueueSubmit2` (a lag only
  makes the value smaller: more clearing).
- `Post::Early(FnOnce(&Gpu, u64))` and `apply_writebacks(.., value)`.
- `release` ordering (touch, then `drop_pool(base, end)`), `register`'s same-extent drop, raw logs kept while another
  entry overlaps.
- `landed` no longer clearing another pool's stamp; brought-back skip arming hash = brought.
- `apply_writebacks`: `Pool::deferred` pages kept off the device (L6).

### 5. Unproven

- **The white-props frame is not fixed** (1 in 96 house shots here, `u2-oot/23-walk3`; 0 in the previous wave's 90
  fixed shots; seen with the skip off in the first wave). This wave's changes do not show a path to it, and nothing
  logged near it. Next: catch it with `--faults` (the white area there is large) to see whether it is one frame and
  which draws; an A/B against `ade9a6c` needs ~100+ shots a side at this rate.
- L3: the race itself has no test and no game hit (counter 0); the lost game store in the bring-back gap stays.
- L6: uploads by `upload_deferred` into the recording in progress, and `NXGFX_WB_GUARD=0`, can still let a held page
  miss CPU bytes queued in an unsent submission.
- Perf is indicative (shared machine).

### 6. Slips

- To check that the H1 and M2 tests fail without their fixes I disabled each fix in place (`if false && ...`,
  `if true || ...` in `upskip.rs`), ran the tests, and restored both lines before committing (Edit tool each time).
- A warm-up `cargo build` started at "go" failed (exit 101) because I was editing `upskip.rs` during it; later builds
  and tests were clean.

### 7. Questions

None.

## Send-back 2 (third audit of `git diff ade9a6c upsync2 -- src`; commit `4f575e0`)

### 1. Built

- **MEDIUM, a hash the GPU no longer holds after a brought-back skip.** Diagnosis (the audit's interleaving): page P is
  held; thread T faults, `read_page` waits for `last_fence()` = 100 and copies B; submission 101 (noted P, writes G,
  submitted after T's copy) lands while P is in `returning`: `lazy::outside` counts P as held, so the landing skips it,
  and `make_lazy` skipped a returning page too, so G never reached guest memory; `upskip::landed` had no record to drop
  yet and re-armed hash = h(G); T then recorded brought = h(B) at 100 and copied B; the sync left P out (guest = B =
  record) and set hash = h(B). GPU G, hash B: the CPU writing B again was skipped. Corrections:
  - `upskip::upload_changed` step 3: the brought-back skip no longer sets `hash = brought` (removed, not needed: a
    skip by the record is a skip either way; the next upload re-arms). The L3 path in step 5 likewise leaves the page
    unknown (`hash 0`, record kept) instead of `hash = brought`.
  - Root, `lazy`: `St::relanded` flags a page of `returning` that a landing skipped (`outside`, which every landing
    goes through via `mem::land`, and `make_lazy`). `bring_back` reads the page again while the flag is set before the
    guest copy (`take_relanded`), and after the copy `settle` takes the flag and the end of `returning` under one lock:
    a flag set after the first check makes it read again, record the new bytes (`brought_back`, new value) and merge
    only what the device changed since the last read into the guest (`mem::merge_changed_bytes`, as a landing does,
    so a CPU store made since the copy stays). The page stops being returning only with no flag pending, so a landing
    either flags it or finds it accessible and lands normally. Counted in the lazy line (`N read again for a landing
    during the read`).
- **LOW, H1 undone by a same-recording forget.** `upskip::forget` keeps a stamp equal to the current `EPOCH` (hash,
  parked hash and record cleared): the page noted by the recording in progress, forgotten by `upload_deferred` or a
  granule binding (both recorded into `sub.pre`, before the recording's draws), then uploaded, still re-arms at its
  landing with the GPU's bytes although `gpu_writes`' RECENT filters the repeat note.
- **LOW, a game store lost when a page is made lazy.** `make_lazy` returns the pages it newly protected;
  `apply_writebacks` then asks the write watch again (under the same watch lock) for those pages: a written one is
  released (`lazy::unhold`: out of `lazy`, made accessible) and this write-back lands on it (`land`: the watch bits go
  to `pending`, the GPU-changed bytes merged). Counted (`N released at once for a CPU store`).

### 2. Tests (`cargo test --release -p graphics`: lib 256 passed, 6 ignored; other targets pass)

- `upskip::a_brought_back_skip_does_not_arm_the_hash` (MEDIUM, the unit-testable part): note, landing 101 re-arms h(G)
  before the record, record h(B) at 100, sync skips P (back), the CPU writes B again -> uploaded. **Failed with the old
  assignment restored** (`left: []`), passes now.
- `upskip::a_forget_after_a_note_in_the_same_recording_keeps_the_stamp` (LOW): note, forget, upload U, repeat note
  (filtered), landing G, the CPU writes U before the next sync -> uploaded; and a forget after the note's recording
  still clears that stamp. **Failed with the old forget** (`left: []` at "U must be uploaded"), passes now.
- `lazy::a_landing_during_a_read_back_reaches_the_guest` (root): a returning page counts as held in `outside` and is
  flagged; `take_relanded` reports it once; `make_lazy` on a returning page holds nothing and flags it; `settle` reads
  again once, merges the GPU's bytes, keeps a CPU store made after the copy, and ends `returning` (the next `outside`
  is None). New mechanism (no flag existed before).
- `lazy::a_page_just_held_can_be_released` (LOW 2, the lazy half; Windows): `make_lazy` returns the newly held page
  (inaccessible), `unhold` makes it accessible and holds it no more. The watch re-check in `apply_writebacks` needs a
  device: no unit test.
- To show the two upskip tests fail without their fixes I disabled each fix in place with the Edit tool, ran them, and
  restored both before the commit (checked: no leftover).

### 3. Verified (GPU 1; OoT relinked from this worktree's dist with its `nxrs-build` before each run, the code DLL's
time checked unchanged after each run; OOT-FLASH relinked and ran OoT between my runs until the lead stopped it)

| Run (`build/play/`) | FPS / 5% low (window) | route | house shots wrong | notes |
|---|---|---|---|---|
| `20261126-105129-s2-faults-oot-house` | 19.3 / 18.0 | on route (5/5) | 0 of 30 | 12 captures: Deku Tree fog, Navi in the dark cut, the known no-target / `pipeline-crashed.bin` reds, the house frame of the 573 s red (correct picture); **1 bring-back read again** (365 s) |
| `20261126-111956-s2-faults2-oot-house` | 19.2 / 18.0 | on route (5/5) | 0 of 30 | 11 captures, same legitimate set (forest fog with Navi, the known full-black scene-change frame at 446 s); 6 zero stand-ins at 688 s near shots 31-33, those pictures correct; **2 read again** |
| `20261126-120401-s2-oot3` (relink 12:03:15, `_code.dll` 337,792,512 bytes = mine, unchanged after) | 17.8 / 15.0 | on route (6/6) | 0 of 30 (walk3 included) | the known reds only; 44 stand-ins over the run, no picture wrong; **1 read again** |

Which runs were on this build: `105129-s2-faults-oot-house` (relink 10:50:51), `111956-s2-faults2-oot-house` (relink
11:19:26), `120401-s2-oot3` (relink 12:03:15), and the partial `113428-s2-oot` (relink 11:34:00; its harness process
ended at 385 s for a reason I did not find, game closed by PID; 6 opening shots correct, not counted). **Invalid:
`114517-s2-oot2`**: my relink failed (`_code.dll` in use by OOT-FLASH's game, which started during my relink), the run
used OOT-FLASH's build of 11:41 and overlapped its run `114437` (two copies at once; the lead confirmed
`pipeline-crashed.bin` unchanged). Not counted.

- House pictures over this send-back: 0 wrong in 90 valid shots on route (60 dense + 30 in `s2-oot3`).
- Lazy line: `released at once for a CPU store` stayed 0 in all three runs; `read again for a landing during the
  read` 1, 2 and 1: the MEDIUM's root race happens in OoT, and those landings now reach the guest. Raced-copy counter (L3) 0.
- Pools line (house window): upload ~1.45-1.5 MB a frame, per-pool pass ~6.2 ms, ~56 MB a frame left out: the skip
  still works without the brought-hash arming.
- Red lines: the known set (no render target x2-3, occlusion query use-up, `pipeline-crashed.bin`).

### 4. Needs audit

- `lazy::bring_back` / `settle` / `take_relanded`: the flag protocol (`relanded` set under `ST` by `outside` and
  `make_lazy`; the end of `returning` taken under the same lock only with no flag), the second `brought_back` record
  and the merge into a guest page other threads may already use.
- `upskip::forget` keeping a current-epoch stamp: rests on every forget during a recording describing a GPU change
  that runs before that recording's draws (`sub.pre`); a forget from a path that writes the GPU copy after the draws
  would re-arm wrongly.
- `apply_writebacks`' watch re-check and `unhold` + `land` for pages just held.

### 5. Unproven

- A landing whose write-back comes from another pool's buffer than the one the page is read from (nested pools): the
  read again gets the held pool's buffer, as the held-page skip always assumed.
- `unhold` cannot release a page another thread faulted on between its protection and the re-check (it is being
  brought back): a store in that window is still overwritten by the bring-back (a few microseconds; counter shows 0
  releases at all in OoT). The L3 gap inside `bring_back` stays as before.
- The white-props frame of the first report: not seen in these runs (0 of 90 house shots, `walk3` clean); its rate
  was 1 in 96, so this proves nothing about it.
- Perf indicative (shared machine: OOT-FLASH builds between runs).

### 6. Slips

- The in-place disabling of two fixes to check the tests (Edit tool, restored). One `git stash` / `stash pop` to
  check a compiler warning was older than my change (restored, verified with `git status`).
- My first relink went through `dist\nxrs\nxrs.exe --select-title ... --build` while OOT-FLASH's launcher was
  running; it returned at once and OOT-FLASH's build ran instead (not mine). Later relinks ran this worktree's
  `nxrs-build` directly with the launcher's own command line.
- **Two copies of OoT at once** (`114517-s2-oot2` with OOT-FLASH's `114437`): my wait script took a quiet gap for
  idle, its relink then failed on the DLL in use, and the script still started the run (it printed BUSY but did not
  stop). Fixed in the script (stop on a failed relink or anything running); the last run was started by hand after the
  lead gave me OoT alone, with the game detected by its full path (`Win32_Process.ExecutablePath`).

### 7. Questions

None.
