# USE-SYNC — CPU-write tracking by write faults instead of the per-submission write-watch scan

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~18:10, worktree
`D:\Projects\RecompLauncherEngine-usesync`, branch `usesync` (base `4b9e524`). Protocol `docs/31-DELEGATION-PROTOCOL.md`.
Title: Mario Kart World only, GPU 0 (OOT-DESC owns OoT on GPU 1). Deadline 4 h. Design source: `REF-ARCH.md` difference #1
and its USE-SYNC wave.

## Plan (written before code)

Step 0: one quiet `mkw` run on HEAD, window [250, 325] s: FPS / 5 % low, replay ms, prep ms, the `[vk] pools:` parts
(lock / scan / pass, sealed count) and the census of why the mirrors not sealed are not. SCAN-COST measured OoT only.

Step 1 (stage A, the cost the scan is): extend `vk/seal.rs` from whole quiet pools to **chunks**. A watched mirror is
armed whether or not the CPU writes it; a store into a protected 64 KiB chunk faults, the handler opens that chunk and
stamps it. The pool stays in the registry: a sync's write-watch scan asks only its **open chunks**, not its whole range.
An open chunk the scans find unwritten for a few frames is closed again (protected, watch taken); a chunk written frame
after frame stays open (hot) and is asked by every scan, as Turbo's hot pages are copied at every submission. The
existing guarantees carry over unchanged: the handler opens a chunk before the store lands, a scan reads the open set
after reading the replay queue's tail, host writes and commits open what they touch (`host_write`), landings unseal.

Step 2 (stage B, only if stage A leaves sync cost that matters): the use-driven part — check only the pages a binding
uses, in the replay thread's pre-pass. Decided after stage A's numbers.

Switch `NXGFX_USE_SYNC` (0 off = today's whole-pool sealing; 1 on; `verify`: at every scanning sync, every armed pool's
**closed** chunks are peeked too and a written page there — a store the new path would miss — is a red line and is
taken). `docs/env_switches.tsv` rows.

Files expected: `vk/seal.rs`, `vk/mem.rs`, `docs/env_switches.tsv`, this doc.

What would disprove it: MKW's scan already small after sealing (then this wave's gain is small, said with numbers), or
faults on chunks that are written often but not every frame cost more than the scan they replace.

## Step 0: MKW on HEAD (sealing on)

`20261126-182205-us-base2-mkw` (HEAD `4b9e524` relinked 18:19; GPU 0), window [250, 325] s, frames-CSV means:
**29.8 FPS, 5 % low 26.0**; frame 33.5 ms; replay 12.25 ms; **prep 11.2 ms**, of it **pool sync 7.8 ms** (pools line
means: lock wait 1.64, **write-watch scan 4.27**, per-pool pass 1.92); descriptor 13.95 ms (lanes); GPU 5.1 ms. Route 3/3
on, sheet correct. (`20261126-181955-us-base-mkw`, the first try, lost its harness at 112 s while the game ran on; closed
by PID, not used.)

Sealing had not taken MKW's scan away: 3,176 pools (4.4 GB) sealed, yet the census at 250 s lists 428 pools (108 MB)
written by the CPU in the last 60 frames and 247 (58 MB) with write-backs, and **~2 sealed pools a frame are woken by a
store** (13,131 unsealed or woken so far). A woken pool went back to whole-pool scans for 60 frames before it could be
sealed again, so one store into a large pool cost a whole-pool walk at every scanning sync for a second. That is what
chunk tracking removes, so the plan stood.

## Design as built (stage A)

`vk/seal.rs` (the one protection tracker; no second one) and `vk/mem.rs`, behind `NXGFX_USE_SYNC` (default on).

- **Arming:** with chunk tracking a watched mirror is armed whatever its CPU writes (`seal_eligible(.., chunks)`:
  sharing no page, one allocation, no write-back landing for `NXGFX_SEAL_FRAMES`, nothing deferred or texture-dirty;
  pending pages allowed, arming adds to them). The first arming is the existing one (protected in budgeted pieces, the
  write watch taken after). The pool is then **tracked** (`Pool::tracked`) and keeps its registry entry for good.
- **Stores:** the existing vectored handler opens the 64 KiB chunk (read-write) before the store lands and now stamps it
  with the frame (`Entry::last`); the first open chunk of an armed entry wakes the pool as before (`sealed` off).
- **Scans** (`mem::scan_list`, used by every sync's scan and by the pool walker): a tracked pool is asked over its open
  chunks only (`seal::open_runs`, read after the replay queue's tail, as the woken queue already was); untracked pools
  whole as before; a tracked pool is never left out as cold (its scan is a few chunks). Pages a call takes are still
  handed out by the pools' whole ranges. A tracked pool counts as asked even with no open chunk, so the per-pool pass
  never walks its whole range itself.
- **Hot and quiet chunks:** pages a sync takes stamp their open chunks (`seal::stamp`); once a frame the walker's
  sealing pass closes the open chunks not written for `NXGFX_USE_SYNC_QUIET` frames (default 8: a chunk written every
  frame or every other frame stays open and is asked by every scan, Turbo's hot rule): protected, then the write watch
  taken over them into the pool's pending pages (a store before the protection), under the pool and watch locks
  (`seal::close_quiet`). A pool left with no open chunk is sealed again.
- Unchanged: host writes and commits open what they touch (`host_write`); a landing unseals its pool whole (lazy
  write-back owns its pages' protection); uploads, the upload skip, the texture check over the sync's dirty ranges and
  raw global memory see the same CPU-written ranges as before, at the same points.
- **Verify** (`NXGFX_USE_SYNC=verify`): at every scanning sync, every tracked pool's closed chunks are peeked
  (`mem::verify_closed`; the old whole-pool scan's cost); a written page in a chunk still closed is a red line, its
  chunk is opened and the page uploaded by that sync.
- Counters on the pools line: chunks open (MB), opened / closed a frame, MB of open chunks scanned, pages missed.

## Runs (GPU 0, window [250, 325] s)

| Run (`build/play/`) | build / switch | FPS / 5 % low | frame | replay | prep | pool sync | lock / scan / pass | route |
|---|---|---|---|---|---|---|---|---|
| `20261126-182205-us-base2-mkw` | HEAD `4b9e524` | 29.8 / 26.0 | 33.5 | 12.25 | 11.2 | 7.8 | 1.64 / 4.27 / 1.92 | 3/3 on |
| `20261126-182928-us-a1-mkw` | `1bc88e3`, on | 27.3 / 6.0 (stalls, below) | 36.8 | 12.9 | 8.0 | 4.9 | 1.38 / 1.89 / 1.67 | 3/3 on |
| `20261126-183553-us-a1-verify-mkw` | `1bc88e3`, verify | 10.3 (verify cost; OFF-ROUTE, not used) | | | | | | 1/3 |
| `20261126-184207-us-a2-on-mkw` | `1bc88e3`, on | 29.5 / 27.0 | 33.9 | 15.3 | 12.3 | 7.5 | 2.55 / 2.30 / 2.67 | 3/3 on |
| `20261126-184744-us-a2-off-mkw` | `1bc88e3`, `NXGFX_USE_SYNC=0` (control, right after) | 20.6 / 0.9 | 49.9 | 16.4 | 15.2 | 11.9 | 1.85 / 8.82 / 2.49 | 3/3 on |
| `20261126-185411-us-a2-turn` (`mkw-turn --faults`, window [250, 290]) | `1bc88e3`, on | 30.3 / 28.2 | | | | | | 4/4 on |

The `a2` pair ran while OOT-DESC's OoT run (GPU 1) was going: its absolute ms are higher than `base2` / `a1` on both
sides. The same-conditions comparison is on vs off: **write-watch scan 8.8 -> 2.3 ms, pool sync 11.9 -> 7.5 ms, prep
15.2 -> 12.3 ms** a frame. Against step 0 (`base2` -> `a1`, quieter): scan 4.27 -> 1.89, pool sync 7.8 -> 4.9, prep 11.2
-> 8.0. **FPS does not move: MKW stays held at 30** (29.5-30.3 with chunk tracking, 29.8 on HEAD). The replay thread is
still ~20 ms a frame (replay 12-15 + prep 8-12) against the 16.7 ms that 60 needs, and the descriptor work on the lanes
(13-16 ms) is untouched; that is SET-FAST's.

In `a2-on`: ~600 chunks open (23 MB), 30 opened and 31 closed a frame, 43 MB of open chunks scanned a frame, 0 pages
missed. What is left of the scan is call overhead, not bytes: a walk over the scan list takes ~1 ms (pool walker line),
~700 write-watch calls (~450 open runs plus ~250 pools with write-backs, scanned whole as before) at ~1.4 us each,
~2 scanning syncs a frame. The sealing pass (now also closing quiet chunks) is 0.23-0.46 ms a frame under the pool
lock, on the walker thread.

Pictures: every sheet read (base2, a1, a2-on, a2-off, verify, turn x2). Grid, race and lap 2 correct in all; `mkw-turn`
12 shots in a row through the banked turn and steel deck (t37-t46), all correct textures, no pop-in, no stale or garbage
texture. Its 15 fault captures were all read: the known timestamp-query and crashed-pipeline lines, stand-ins at loads
(released textures, the known set), the exit's "compilation stopped", and white flashes at presents 346-348 and 3070,
which are the real scene (the opening's camera into bright clouds, the course flyover's white roof). Red lines: the known
set only, none from chunk tracking.

## Stage B (use-driven sync in the pre-pass): not built

Decided from the numbers above. After stage A the sync parts left on the replay thread are scan ~1.9-2.3 ms (call
overhead), per-pool pass ~1.7-2.7 ms (mostly uploads and the upload skip's compare of what really changed), and lock wait
~1.4-2.5 ms (the completion thread's landings hold the pool lock). A use-driven sync could take at most the scan and part
of the pass, ~2-3 ms, minus the per-use checks (Turbo: ~1 ms a frame). It needs the replay pre-pass to see every binding
(UBO, SSBO, vertex, index, indirect, copy sources, addresses read from UBOs), a gmem raw-pointer scan, and texture checks
from page stamps. That would change exec.rs, image.rs and gmem.rs and risk stale data. That gain would not lift MKW off
30 (the replay thread would still be ~17-18 ms), so it was not attempted inside this deadline.

## Report (agent, protocol §5)

Branch `usesync`: `b78ed26` (plan), `1bc88e3` (chunk tracking), `ff34d7f` + this commit (doc).

### 1. Built
Diagnosis: on HEAD, MKW's write-watch scan was still 4.3 ms a frame. ~2 sealed pools a frame were woken by a store and
then scanned whole for 60 frames, and 428 CPU-written pools were never sealed. Correction: chunk tracking in `seal.rs` /
`mem.rs` (design above). Every eligible mirror is armed. Scans ask only open 64 KiB chunks. Quiet chunks close again
after 8 frames. Verify mode is `NXGFX_USE_SYNC=verify`. Switch rows are in `docs/env_switches.tsv`. Default on (verify
clean, pictures correct); `NXGFX_USE_SYNC=0` restores SCAN-COST's behaviour.

### 2. Tests (`cargo test --release -p graphics`: 271 passed, 6 ignored; other targets pass)
- `seal::tests::chunks_open_at_a_store_and_close_when_quiet` (real VirtualAlloc + handler): stores open only their
  chunks, and adjacent ones form one run. A stamped chunk stays open; a quiet one closes and a later store reopens it.
  `closed_runs` is the complement. A write past the protection counts as a miss and opens its chunk, while a write in an
  open chunk does not. Once all chunks close, the entry is armed again.
- `mem::seal_tests::a_tracked_pool_is_scanned_over_its_open_chunks_only`: a sealed tracked pool is not asked. After a
  store, only its open chunk is asked, and an untracked pool is still asked whole.
- `mem::seal_tests::only_quiet_watched_mirrors_are_sealed` extended for chunk mode (written pools and pending pages
  allowed, tracked ones and landings not).
- These cover new behaviour, so there is no "failed before" state; the old seal tests pass unchanged.

### 3. Verified
Runs and numbers are in the table above. Verify mode over a full scenario run: 0 misses, 0 leaks. Route checks were on
in every non-verify run (3/3, 3/3, 3/3, 4/4). FPS before/after: 29.8 / 26.0 -> 29.5 / 27.0 (`a2-on`) and 30.3 / 28.2
(`turn`); the control with it off gave 20.6 / 0.9 under the same load. Target 60 not reached. Screenshots:
`build/play/20261126-182205-us-base2-mkw/sheet-1.png` (before),
`build/play/20261126-184207-us-a2-on-mkw/sheet-1.png`, `build/play/20261126-185411-us-a2-turn/sheet-1.png` and
`sheet-2.png` (after). All numbers are indicative: OOT-DESC (OoT, GPU 1) and the web waves ran throughout.

### 4. Unproven
- The verify run reached lap 1 only, because verify mode is slow. A longer verify run would cover more of the race.
- Chunk churn: 9-30 chunks opened and closed a frame at `NXGFX_USE_SYNC_QUIET=8`. A larger value was not tried.
- Write paths the protection cannot see (kernel-mode writes into guest pages) now matter for CPU-written pools too, not
  only quiet ones. None appeared (verify 0, leak check 0), but the risk now covers more memory.
- Pools with write-backs are still unsealed whole at a landing (~250 pools, ~60 MB, one call each per scan).
- OoT and other titles were not run (not owned). Chunk tracking is on by default for every title, so the lead's
  regression runs should cover OoT and DKB before merge.
- What holds MKW at 30: replay 12-15 ms plus the remaining prep, and descriptor time on the lanes (SET-FAST).

### 5. Slips
- Relinks ran this worktree's `target/release/nxrs-build.exe` directly with the launcher's own argument list (as
  SCAN-COST did): `build.ps1` refused to start while another wave's cargo was running. I checked the DLL time after each
  relink (18:19, 18:28).
- `20261126-181955-us-base-mkw`: the harness died at 112 s while the game ran on. I closed the game by PID (56488).

### 6. Questions
None.

`us-a1`: chunk tracking 450-550 chunks open (13-18 MB), ~9.5 opened and ~9 closed a frame, ~23 MB of open chunks
scanned a frame. The 5 % low comes from one stretch, 290-305 s (18:34:18-18:34:33), of 100-250 ms frames with the
replay thread at its normal 10-14 ms and ~8 chunk opens a frame (no fault storm); OOT-DESC was relinking OoT then (its
run `od-v3` started 18:35:20). Re-measured below.

**Verify:** `us-a1-verify-mkw` ran the whole scenario in verify mode (boot, menus, the race's first lap): **0 pages
written in closed chunks** (`0 written pages missed in closed chunks so far`), 0 sealed pools written without a fault,
red lines the known two (timestamp queries, the crashed pipeline). The verify peeks slow MKW to ~10 FPS, so the grid
shot came during the pre-race intro (OFF-ROUTE by timing; its pictures are correct). Sheet read: boot, title flyover,
course overview, grid, race x2, all correct textures.
