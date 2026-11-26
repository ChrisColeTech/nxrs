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

`us-a1`: chunk tracking 450-550 chunks open (13-18 MB), ~9.5 opened and ~9 closed a frame, ~23 MB of open chunks
scanned a frame. The 5 % low comes from one stretch, 290-305 s (18:34:18-18:34:33), of 100-250 ms frames with the
replay thread at its normal 10-14 ms and ~8 chunk opens a frame (no fault storm); OOT-DESC was relinking OoT then (its
run `od-v3` started 18:35:20). Re-measured below.

**Verify:** `us-a1-verify-mkw` ran the whole scenario in verify mode (boot, menus, the race's first lap): **0 pages
written in closed chunks** (`0 written pages missed in closed chunks so far`), 0 sealed pools written without a fault,
red lines the known two (timestamp queries, the crashed pipeline). The verify peeks slow MKW to ~10 FPS, so the grid
shot came during the pre-race intro (OFF-ROUTE by timing; its pictures are correct). Sheet read: boot, title flyover,
course overview, grid, race x2, all correct textures.
