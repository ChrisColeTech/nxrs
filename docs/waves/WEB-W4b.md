# WEB-W4b - Anime Girls in Chrome: pool sync by use instead of whole-pool compare

Wave of `docs/35-WEB-TARGET-PLAN.md` W4 (row "Pool sync without write watch"). Base `055c41d`, branch `web-w4b`,
title Anime Girls Sun of a Beach (0100C6801E0FA000), dev server port 8772, GPU 1 for native runs. Follows
`docs/waves/WEB-W4.md` §5.2.

## 1. Evidence (W4)

`build/WEB/W4/m4-page.log`: in beach gameplay the `pools:` line reads `43 mirror (706 MB) ... 6 mirrors without write
watch (96 MB); pool sync a frame: ... write-watch scan 0.00 ms, per-pool pass 84-86 ms; syncs a frame 1.0 replays, 1.0
presents`; pacing 9.7-10.8 FPS; replay 2-3 ms. On the web every pool is unwatched (`watch::take` answers None), so a
pool with a GPU buffer has a snapshot and `take_cpu_writes` compares all of it with guest memory at every sync
(`begin_submit_ex` keeps unwatched mirrors in `Pools::busy`, so even "covered" syncs visit them). Pools without a GPU
buffer have no snapshot and are not compared at all (unchanged by this wave, §5).

## 2. Route chosen: compare at use (no translator change)

The software write watch (a dirty bit per guest store in the translator's web store macros) was not taken:

- it costs every guest store of the game, not only the frame's GPU-visible ranges;
- it is only correct if every host write into guest memory also marks pages (file reads into pools, IPC buffers,
  service copies, the I/O worker's slots, audio, write-backs), with no list of those paths and no way to test that
  none is missed; a missed one is a stale texture or vertex buffer with no error;
- it needs a rebuild of all 417 generated units and a native identity check of the headers.

Compare at use keeps the only source of truth the snapshot already has, and reads only what the GPU will read:

1. **Buffers (`mem::resolve_in`, web arm).** Every draw, dispatch and copy resolves the range it binds. For a pool with
   a snapshot, the pages of that range not yet compared in this sync (`Pool::web_seen[page] != Pools::gen`) are
   compared now; changed bytes go to the snapshot and are uploaded (through `upskip` and the in-flight write-back guard,
   as `sync_range` does), and are handed to the next sync's texture check (`tex_dirty`, as `sync_range` does).
2. **Use caches.** `resolve_in`'s 16-window cache and `exec::resolve_cached` skip `resolve_in` while `POOL_GEN` stands.
   The web sync moves `POOL_GEN` once per sync, so the first use of a range in each submission reaches the compare;
   later uses in the same submission hit the cache (the page was compared in this sync already).
3. **Textures (`image_begin_submit`, web arm).** Texture memory is not resolved as a buffer: the textures' storage
   ranges (non-sparse, with storage) are compared the same way (`mem::web_compare`), and what changed joins the dirty
   ranges the texture check already handles (page hashes, GPU-owned rules, `needs_upload`). Changed bytes are also
   uploaded into the pool's buffer, so its GPU copy stays the guest's.
4. **`begin_submit` on the web** no longer compares whole snapshots: it still uploads what pools hold (pending,
   deferred, texture-dirty ranges).
5. **Compare speed.** Pages are compared 64 bytes at a time as u64 words (SIMD under `+simd128`) instead of slice
   equality (memcmp).

**Lock order:** images -> mem. `image_begin_submit` holds the images lock and calls `mem::web_compare`, which takes the
pool lock. This order exists already (`queue::oneoff` records with the images lock held, and its recording resolves
ranges); `mem` never takes the images lock while holding the pool lock (`pool_del` drops it first).

**Correctness:** a page is compared before every use that can read it in a submission; nothing is skipped to gain
FPS. Writes after a page's compare in one sync are found at its next use in a later sync (the snapshot keeps the
older bytes). Write-back landings already update the snapshot (`land`).

## 3. Files

| File | Change |
|---|---|
| `src/graphics/src/vk/mem.rs` | `Pool::web_seen`; web arms of `take_cpu_writes`, `resolve_in`, `begin_submit_ex`; `web_compare`; fast page compare; tests |
| `src/graphics/src/vk/image.rs` | web arm of `image_begin_submit`: textures' storage compared, joined to the dirty ranges |
| `docs/35-WEB-TARGET-PLAN.md` | the "Pool sync without write watch" row |

## 4. Verification plan

- `cargo test --release -p graphics`; new tests: a write inside a used range is found and uploaded; a write outside
  it is not taken then but is when that range is used later; a range compared once is not compared again in the same
  sync but is in the next; nothing lost across syncs.
- Chrome, port 8772: title menu and beach gameplay; `pools:` and `pacing:` lines before (W4 log) and after on the
  beach; screenshots compared with `build/WEB/W4`.
- Native: only `cfg(target_os = "emscripten")` arms change; engine tests.

## 5. Results (2026-11-26): built and unit-tested; not measured in the game (blocked)

| Item | State | Evidence |
|---|---|---|
| Compare by use (buffers, textures, POOL_GEN per sync, word compare) | Built (`24061d1`) | `cargo test --release -p graphics`: 275 passed, 5 new (`compare_by_use_tests`, `merge_ranges_tests`); the web library builds and links (`web.ps1 -Crates os`, `nxrs-build ... --target web`) |
| `NXGFX_WEB_USE_SYNC=0` | Built | the old whole-pool compare on the same build, for a same-scene before/after |
| Chrome: per-frame sync and FPS before/after on the beach | **Not measured** | the game stops before the title menu on every run, also on the untouched base (below) |

### 5.1 Blocker: the web build stalls after its 4th submission, base included

Every Chrome run on this machine today (15 runs, `build/WEB/W4b/run*-page.log` in the main checkout) stops on the
studio logo: `heartbeat: ... 4 submits, 3 presents` from ~20 s on, `warning: 6 s without a new image`, nothing
after. W4's `m4-page.log` paused at the same point for 4.2 s (`slow frame 4: 4212 ms`) and then went on.

- **Not this wave's change:** the same stall with `?env=NXGFX_WEB_USE_SYNC=0` (`run2-off`) and with a library
  built from the untouched `055c41d` (`run15-base`). `055c41d` equals W4's tested tree in graphics, os, runtime and
  web (`git diff 055c41d^2 055c41d` is empty there).
- **Not:** synchronous replay (`NXGFX_ASYNC=0`, `run9`), a 64-worker pthread pool (`run10`), a warm OPFS cache
  (`run14b`), the network (only `favicon.ico` 404s), WebGPU errors (none; the GPU worker drained the ring and
  completed all 7 of its submissions: `H_DONE`=7, head == tail, `run6`).
- **Where it waits** (wasm stacks named from `game.js.symbols`, `run8/stacks-named.txt`): the game's main thread is
  in `nvnSyncWait -> queue::wait_fence -> queue::physical`, polling for a fence that never resolves; the loader thread
  keeps reading RomFS slowly. Threads the engine starts lazily (the `nvn-parked` retry thread, the
  `NXGFX_WAIT_DIAG` observer) never log anything, so whether the 4th submission is parked could not be read.
- **Next step:** a log line at `run_stream_due`'s park and at `request_retry` (on the web) to see whether the 4th
  submission parks at an event wait and whether the retry thread starts; then fix what it shows.

### 5.2 What remains unproven

The FPS gain, the per-frame sync time after the change, and the picture comparison with `build/WEB/W4` all need the
game past the logo. Native is unaffected by construction (every behavioural line is behind
`cfg(target_os = "emscripten")` or `web_by_use()`, which is false off the web); no native run was made.
