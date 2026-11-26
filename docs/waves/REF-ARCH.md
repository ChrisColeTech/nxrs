# REF-ARCH — why nxrs's frame costs 2-3x the references', and what gets Mario Kart World to 60

Read-only investigation, 2026-11-26. No code was changed and nothing was built, run or committed. Deadline: 2 h plus
45 min (the kirikae addition).

References read:

| Reference | Root, revision | API / hardware | Licence |
|---|---|---|---|
| nxnative (current) | `build\Q\nxnative-up` `7ebb76e` (`dm/gfx/nvn`, `dm/games/0400C3F00006E000` MKW, `0400486024DEC000` OoT, `docs/`) | Metal on Apple silicon (unified memory) | MIT |
| Turbo | `D:\Projects\Switch2RECOMP-main`, branch `turbo`, `11e966f` (`runtime/nvn/vk`, `docs/GAMEPLAY_INVESTIGATION.md`, `MKW_PERFORMANCE_INVESTIGATION.md`) | **Vulkan, Windows, RTX 5090: this machine** | the owner's fork; MZ Labs' parts AGPL: reimplement designs, never paste code |
| kirikae | `D:\Projects\kirikae` `304a109` (`ARCHITECTURE.md`, `crates/rt/src/gpu`, `crates/rt/src/nvn`, `crates/maxwell`) | Vulkan 1.3 (`ash` 0.38) | MIT OR Apache-2.0 (designs and code reusable, credit in README / `LICENSES/`) |
| mkw-recomp (only to cross-check) | `D:\Projects\mkw-recomp-main` README | Vulkan | MIT, provenance unpinned |

nxrs numbers come from the wave docs named in the brief. Reference numbers are the references' own. Where a figure is
my estimate, it says "est.".

## 0. The answer in one paragraph

nxrs has the same architecture as the two references that ship MKW at 60. The game thread records and flattens; one
replay thread, in queue order, turns the stream into GPU commands at submit time. **None of the references translates at
NVN-call time**: nxnative replays at submit on its "NVN queue replay" thread, and Turbo on its translation thread
(kirikae is the exception, covered below). So the gap is not where translation happens. It is what nxrs does per
submission and per draw.

1. **Per submission**, nxrs scans every mirrored pool's write watch and walks every pool (`mem::begin_submit`, 7-11
   times a frame). nxnative has no step at all (unified memory). Turbo checks only the pages a binding actually uses,
   against stamps written by protection faults (`vkm_use` → `pool_sync`).
2. **Per draw**, nxrs builds or patches a descriptor set at ~2.3-10 µs. Turbo spends ~1 µs and nxnative less. nxrs also
   resolves each bound texture through a view lookup under the images' lock, and sets ~30 dynamic states
   unconditionally. Both references emit only the state that changed.

The six translation lanes were built to hide the per-draw cost. They double the CPU and lose 20-30 % of it to lock
contention, so the wall time barely moves.

The same game on the same machine and GPU: **Turbo's single translation thread does MKW's whole race frame, memory sync
included, in 9.5-11.3 ms at 57-59 FPS.** nxrs's replay thread needs ~20.5 ms (replay 11.0 + prep 9.5) plus 17-22 ms of
lane CPU, and is held at 30 FPS.

## 1. Side by side, stage by stage

Frame figures: MKW race unless marked. nxrs: lead's quiet run `20261126-083639-lead-tc-on` and TRANSLATE-REUSE.
Turbo: `GAMEPLAY_INVESTIGATION.md` §2 and `MKW_PERFORMANCE_INVESTIGATION.md` (RTX 5090). nxnative: MKW README "Frame
rate" and `rendering.md` (Apple M6).

| Stage | nxrs (`src/graphics/src/vk`, `nvn`) | Turbo (`runtime/nvn/vk`) | nxnative (`dm/gfx/nvn`) | kirikae (`crates/rt/src`) |
|---|---|---|---|---|
| **What is emulated** | NVN API (HLE): the game's driver is replaced | NVN API (HLE), a port of an older nxnative | NVN API (HLE): `hle_nvn.c` takes over `nvnBootstrapLoader`; no driver instruction runs | **The GPU (LLE).** The game's own NVN driver runs (recompiled); its pushbuffers are executed by an emulated command processor: Maxwell 3D / compute / 2D / DMA class methods, register file, MME macros (`gpu/mod.rs` `submit` → `run_pushbuffer`, `threed.rs`, `compute.rs`, `mme.rs`). Host NVN (`nvn/shadow.rs`, `exec.rs`, `KIRIKAE_NVN=exec`) is in progress, "to cut the command processor's cost" (README "Status") |
| **Record** (game threads) | `nvn/cmd.rs`: compact records in game command memory | same (nxnative's format) | `hle_nvn.c` appends records into game command memory (`nvn_obj.h`) | the game's driver writes real Maxwell methods |
| **Submit** (game thread) | `queue::submit` flattens (`cmd::for_each_record`), `Op::Submit` to the replay queue | `hle_nvn.c` `aq_push`; fibers park a submission at an unmet wait | `nvn_queue_submit` (`nvn_queue.inc:268`): `nq_flatten` copies records, snapshots program/texture objects, enqueues a job; no translation | `nvdrv` `SUBMIT_GPFIFO` runs the pushbuffer **synchronously on the calling guest thread** (`services/nvdrv.rs:386` → `Gpu::submit`) |
| **Translate** (thread, time) | replay thread `aq_main` (`queue.rs:1756`) → `run_stream_due` (`queue.rs:2041`) → `exec::run` (`exec.rs:8891`); long streams forked to 6 lanes (`run_segmented` 9288, `translate_segment` 9490) with a state pre-pass and gate | one translation thread, serial (`vk_replay.c`) | one thread `nq_worker` (`nvn_queue.inc:123`) → `mtl_exec` (`nvn_mtl.m:17725`), serial. Optional encode thread (`DM_NVN_ENC_THREAD`, iOS default): BotW's submitting cost 8 → 4 ms | at ioctl time on the game's thread, one draw at a time from register state (`vulkan/draw.rs` `try_draw`) |
| **Per-submission memory sync** | `mem::begin_submit_ex` (`mem.rs:3013`) at every op: write-watch scan of non-sealed mirrors, per-pool pass, uploads, then `image_begin_submit` (`image.rs:3768`). MKW 7.3 ms a frame (lock 1.5 / scan 4.4 / pass 2.2), OoT 11.9 ms after SCAN-COST | **none.** Sync happens per use (next row) plus a budgeted round-robin `vkm_sync_all` (`vk_mem.c:585`) | **none** (unified memory) | none; per use (next row) |
| **Address → buffer** | `mem::resolve` (`mem.rs:1631`) / `room` under `g.mem`, per-thread memos `resolve_cached` / `room_cached` (`exec.rs:4298`, `4215`); lanes miss into the pool lock (8.8 % + 3.5 % of lane samples) | `vkm_use_` (`vk_mem.c:536`): 8192-entry hash of (addr, len) → (buffer, offset), valid while the submission epoch and the host-write generation hold. 16,000 uses a frame, ~1 ms before `use_fast 3`, less after. A miss runs `pool_sync` on **that range's pages only** | `mtl_addr` (`nvn_mtl.m:1216`): cache, then a binary search under `g_mu`; GPU address = CPU address | `Gpu::translate` over nvdrv address-space mappings |
| **CPU-write detection** | `GetWriteWatch` scans; SCAN-COST seals pools quiet for 60 frames with write protection (`seal.rs`); upload skip by page hash (`upskip.rs`) | write protection per page (`nvn_dirty.c`, nxnative's): a fault stamps the page's generation; `page_needs` (`vk_mem.c:411`) compares the page's copy stamp. Pages written 3 frames in a row turn **hot**: left writable, copied once per submission at first use (`page_rewritten` 403). No scan | `nvn_dirty.c` write protection, only to know when **texture** storage changed (`nvn_dirty_since_span`); buffers need nothing | none: each constant / vertex / index range is **compared byte for byte with a shadow copy at every use** (`buffer_cache.rs:72` `lookup`); stable ranges are promoted after 2 identical uses into a BAR arena |
| **GPU-write coherence** | write-backs at completion (`apply_writebacks` `mem.rs:3437`, under `g.mem`): MKW ~4.4 ms a frame on the completion thread; lazy write-back on by default (`NXGFX_WB_LAZY`) | write-backs at completion, 4.8 ms a frame in MKW (14.5 MiB); write-back on demand (`PG_LAZY`) | none: GPU writes land in the game's memory | `readback.rs`: render targets written back when read; `guest_writes.rs` tracks CPU writes over them |
| **Descriptor model** | `VK_EXT_descriptor_buffer`; one universal layout: every stage set has 176 bindings (`layout.rs`: 31 banks, 32 SSBO, 64 combined image samplers, ...). Per set: `stage_set` (`exec.rs:4415`) → `SetKey` / patch classes → `patch_set` (5027) or a whole build → `write_db_set` (5088), `vkGetDescriptorEXT` per descriptor into a host shadow, then a copy into the BAR chunk. c0 / c2 driver banks rebuilt and staged per change (`QState::banks` 797) | classic sets, **per-shader layout** (`sh->dsl`), `vkUpdateDescriptorSets`. Reuse key = O(1) compare of (shader, state, per-stage bind version, submission, registration generation, pool generation, base instance) (`write_set` `vk_replay.c:1158`, `g_lset`) | **argument buffer per shader** (`arg_len` exact): flat 64-bit slots (GPU addresses, `gpuResourceID`s) bump-allocated in an arena (`arena_take` 8070). `encode_args_full` (13247): reuse if generations match → bind the same block; if only uniform slots moved, `memcpy` the old block and rewrite those slots (`args_cbuf_only` 13181); else full fill under one `g_rmu` hold | `VK_KHR_push_descriptor` per draw (`descriptors.rs:242`); a per-draw set as fallback |
| **Textures per draw** | `tex_view` → memo keyed by `view_version(id)` → `tex_view_uncached` (`exec.rs:3788`) under the images' lock (9 % of lane samples); then `vkGetDescriptorEXT`. ~12,800 `BIND_TEX1` a frame, ~6.6 textures a draw | lookups in the set fill (5 ms of the thread for all set contents, MKW) | `tex_fast_rid` (`nvn_mtl.m:7952`): `g_treg[tid]` array index; cached view and resource ID, checked by epoch; ~10 ns | TIC headers (`tic.rs`) → image cache |
| **Bindless pools** | pool set 6 by texture / sampler ID (`layout.rs`, `tables.rs` shadow), used only by pool-indexing shaders | set 5, update-after-bind (`vksh_pool_dsl`) | `TEX_POOL` / `TEX_HTAB` buffers of resource IDs per generation (`pool_gpu` 12370) | n/a |
| **Uniform buffers** | a UBO descriptor per slot per set (address + range), resolved per binding | descriptors, same | a raw GPU address in the argument slot (`cbuf_slot` 13133); inline updates inside a pass copied (`ubp_get`) | content-compared shadow; uploaded or BAR-promoted |
| **Dynamic state** | ~30 `vkCmdSet*` **every draw, unfiltered** (`draw_setup_ex`, `exec.rs:6932-7124`) | shadowed per command buffer, set only on change (`ds_same`, `DS_U`, `vk_replay.c:1314-1716`; the comment there: "~25 a draw") | shadowed (`g_ed`, `nvn_mtl.m:15978-15990`); pipeline fetched only when dirty (15943) | derived from Maxwell registers per draw |
| **Pipelines** | `gfx_pipeline` (`pipe.rs`); EDS3 + dynamic vertex input; disk cache + prewarm (`pcache.rs`); lookups ~0.4 ms a frame | `vk_pipe.c`, `vk_pcache.c`, prewarm list per title | PSO by state hash; manifests `pso_manifest.tsv` / `pso_user.tsv`; MTL4 archive; **hybrid** vertex-pulling stand-in while the real pipeline compiles | async compile on 1-4 worker threads (`pipeline.rs:162`); a draw whose pipeline is not ready is skipped; disk cache |
| **Sync, waits** | timeline per submission; an unmet event wait parks the queue; a queued GPU signal satisfies a wait (`exec.rs:8887`); MKW: one submit op becomes 7 Vulkan submissions, each with its own pool sync | the same rules (fibers) | `nq_worker`: a queued exact signal satisfies a wait (`nq_event_write`); fences and events published after completion | syncpoints (`Gpu::increment`) |
| **Present** | `Op::Present` on the replay thread; `vkQueuePresentKHR` 5.8 % of it (~1.2 ms) | `vk_present.c` | enqueued; a presentation thread draws to the drawable (`mtl_present` 19224); pacer `nvn_pace.h` | `display.rs`, frame pacing |
| **Shaders** | nxnative's `dmsh_xlate` (Ryujinx, Ampere + Maxwell) → SPIR-V at build time | older nxnative translator, at build time | the same chain → MSL → metallib, offline | **own Rust Maxwell (SM 5.x) → SPIR-V translator** (`crates/maxwell/src/translate.rs`, 4,044 lines; HSET2, ATOMS, ...): **no Ampere / NVN2, so no Switch 2 title** |
| **MKW race** | 30.7 FPS (held at 30); replay thread 20.5 ms (11.0 + prep 9.5); lanes 17-22 ms CPU; descriptor 12.8 ms (lanes); GPU 4.8 ms | **57-59 FPS**; translation thread **9.5 ms** (`GAMEPLAY` §2 after `5758e24`), 11.2-11.3 ms in a later telemetry run; sets ~5 ms of it (allocation + `vkUpdateDescriptorSets` 1.5); driver ~4 ms; GPU 4.2-6.5 ms; write-back 4.8 ms on completion | **60 FPS** (M6); replay CPU **6.36-6.94 ms** a frame, encoding and residency commits; "Submit" 3.7-5.2 ms; GPU 7-13 ms | not run (Switch 1 only) |
| **OoT** | ~19.4 FPS; frame ~51 ms: replay 18.9, prep 16.1, descriptor 19 (lanes) | not playable (27-50 FPS in the first scene, then stops) | ~59 FPS (README); NDVE network by host kernels | not run |

**kirikae in short.** It is the opposite design point: LLE of the GPU, which costs more CPU per draw than HLE, not less.
Its own roadmap replaces the command processor with a host NVN layer (`nvn/`), which confirms nxrs's HLE choice. It
publishes no MKW or OoT figure (README: "complete commercial 3D games ... at 30 fps on a modest GPU", Switch 1). Its
translator is Maxwell-only and of no direct use for Switch 2. Three of its designs are worth noting (§2 item 5, §3).

## 2. The structural differences, ranked by estimated per-frame ms saved

"Critical path" means the replay thread, which today decides MKW's frame. MKW must get under ~16.7 ms with margin.

### 1. Memory sync per submission → per use, from fault stamps (MKW est. 5.5-7 ms, OoT est. 8-12 ms on the critical path)

**Evidence.** MKW prep 9.5 ms a frame. Pool sync is 7.3 ms of it: lock 1.5, scan 4.4, pass 2.2 (DIRECT-MEM
`131020-dm-final-off`; lead run `083639`). The completion thread holds `g.mem` ~4 ms a frame, which the lanes' `room` /
`resolve` misses wait for (TRANSLATE-COST). OoT after SCAN-COST: prep 16.1, pool sync 11.9 (scan 2.6, pass 5.9 of which
~4.8 is the upload skip's copy-and-hash of ~57 MB, lock 2.5).

Turbo does the same discrete-GPU job inside its 9.5 ms thread, with no scan:

- `vkm_use_` hashes the bound (addr, len). Within one submission a range synced once is current (`use_fast 3`: the game
  may not write memory a submitted command buffer reads).
- On a miss, `pool_sync` looks only at that range's pages. Each page carries the stamp of its last copy (`up[pg]`),
  compared with the protection tracker's generation (`nvn_dirty_since_span`).
- Behind pages are copied, then re-protected. Hot pages (written 3 frames running) stay writable and are copied once per
  submission at their first use.

nxnative needs none of this, because the memory is the GPU's.

**Why it is the biggest.** The scan's cost is proportional to the pools that exist (MKW ~3,950, 4.8 GB; OoT ~4,400) and
is paid 7-11 times a frame. Use-driven cost is proportional to the pages the frame binds, and is paid once per
submission per range.

**Estimate.** MKW: the 7.3 ms pool sync plus some of the 2.2 ms rest of prep become ~1-1.5 ms of use checks and copies
(Turbo's whole-thread total bounds it), so ~5.5-7 ms is saved. OoT: scan and pass become the copies of what is bound,
est. 8-12 ms; low confidence, because how many of OoT's 57 MB of CPU-rewritten pages its draws really bind is not
measured.

**Already part-way there.** SCAN-COST's sealed pools are the same protection mechanism (fault → wake), but applied per
pool and only to quiet pools: a pool written every frame is never sealed and is still scanned. MKW has **not** been
measured since sealing merged (`0e04404`); its scan may already be lower. Measure that first (§3, step 0).

### 2. Per-set descriptor cost (MKW est. 7-9 ms lane CPU → 2.5-4 ms wall; OoT est. 10-13 ms of its 19 ms descriptor time)

**Evidence.**

- nxrs: ~2,300-3,300 sets a frame. Patched sets cost 2.3-2.7 µs, whole ones 7-10 µs, 11-14 ms of lane CPU in all
  (views 3-6, banks 1.3-1.9, writes 1.7-2.5; TRANSLATE-REUSE). MKW-PERF3: "a patched set costs nearly as much as a whole
  one".
- Turbo: ~5,000 sets a frame in ~5 ms, ~1 µs each, lookups included.
- nxnative: the common case is the reuse branch (compare 5 generations, bind the old block). The next case is
  `memcpy(arg_len)` plus rewriting only the uniform slots (`encode_args_full`, `args_cbuf_only`).

**What makes nxrs's set expensive** (from the code and the profiles):

- `SetKey` construction and patch-class logic, where Turbo has a version-counter compare.
- Every texture resolved through `tex_view`, with a lock on a miss. nxnative uses an array index; nxrs already holds
  each ID's descriptor bytes in the pool set's shadow (`tables.rs` `shadow`, immutable `PoolSnapshot`).
- The universal 176-binding layout: every set is sized and copied for 64 textures and 32 buffers (~1.5 KB host → BAR per
  set). Turbo's and nxnative's sets are per shader.
- The c0 / c2 bank bytes rebuilt and staged per change (`QState::banks`).

**Estimate.** At Turbo's ~1 µs a set, MKW's ~2,800 sets cost ~3 ms instead of ~12 ms of lane CPU. That is ~2.5-4 ms off
the lanes' wall: segments are ~16 µs a draw on a lane against ~7.5 µs serial (TRANSLATE-REUSE), because the lanes also
lose 20-30 % to locks that this removes.

### 3. Lanes vs one fast thread (indirect; MKW ~0-2 ms wall, frees 10+ ms of CPU)

**Evidence.**

- Segments' wall 7.3-8.5 ms for ~20 ms of lane CPU.
- 30 % of lane samples in contended mutexes (pool lock, images lock).
- 97 % of whole-set builds are "no copy of the module" at a segment start (`fork_state` empties caches); carrying copies
  between segments never happened (TRANSLATE-REUSE).
- 10 lanes collapsed MKW to 4 FPS.

Neither reference uses lanes: Turbo at 9.5 ms and nxnative at ~6.4 ms are single-threaded.

**Reasoning.** Lanes multiply per-draw overhead and lock traffic. Once items 1, 2 and 4 bring the serial cost near
Turbo's (~4-5 µs a draw all-in), serial translation is est. 7-9 ms. Lanes then save at most a few ms, and the gate's
give-ups at load cost real time (937 segments in 600 frames). The architecture stays; the lane count becomes a measured
choice (`NXGFX_TRANSLATE_THREADS`), and use-driven sync must not run inside the lanes (§3, risks).

### 4. Dynamic state only when it changes (MKW est. 1-2 ms CPU, 0.3-0.7 ms wall)

**Evidence.** `draw_setup_ex` emits ~30 `vkCmdSet*` and `vkCmdSetVertexInputEXT` for every draw. The NVIDIA driver is
13 % of lane samples (`nvoglv64` self; TRANSLATE-REUSE), and TRANSLATE-COST put the per-draw driver calls at ~6 %.
Turbo's `ds_same` / `DS_U` (reset when the command buffer or a non-draw pipeline changes) and nxnative's `g_ed` emit only
what changed. mkw-recomp's roadmap names the same item ("dynamic state only when it changes").

**Estimate.** ~2,000 draws × ~25 avoided calls × 20-40 ns ≈ 1-2 ms of CPU. It is the smallest change and the lowest
risk.

### 5. Smaller items (each under ~1.5 ms; listed so they are not lost)

- **Present off the replay thread.** `vkQueuePresentKHR` is 5.8 % of the replay thread, ~1.2 ms. nxnative enqueues and
  draws on a presentation thread.
- **Render-target churn under dynamic resolution** (nxrs: 10-25 ms a frame when GPU time rises; DIRECT-MEM).
  nxnative recycles render-target memory (`rendering.md`, "Render targets that move with the dynamic resolution get new
  Metal memory each time"; replay CPU 6.94 → 6.36 ms, 970 → 272 residency commits per 300 frames). nxrs also
  "uploads each new render target from storage", which a fresh target should not need. It only matters if GPU time
  rises, but direct memory showed it does.
- **Push descriptors** (kirikae `descriptors.rs:242`, `VK_KHR_push_descriptor`): no set allocation or copy into a chunk
  for the few per-draw-changing uniform buffers. An option for item 2 if the descriptor-buffer patch path stays costly.
  It would need descriptor buffers' push-descriptor support (`bufferlessPushDescriptors`) or a classic layout for that
  set; not checked on this driver.
- **Async pipelines with a stand-in draw** (nxnative hybrid pulling pipelines; kirikae skips the draw until its pipeline
  is built, `pipeline.rs:162`). This addresses hitches, not average FPS; Turbo's MKW worst frames are pipeline creation
  (300-645 ms).
- **kirikae's content-compared constant buffers** (`buffer_cache.rs`): a `memcmp` of each bound range per use. It needs
  no OS tracking, but costs bytes compared every use. Not recommended for MKW's 16,000 uses a frame; it is a safe
  verifier for the use-driven sync (§3 risk 1).

### Ranking summary (est., ms a frame)

| # | Difference | MKW critical path | MKW CPU total | OoT | Confidence |
|---|---|---|---|---|---|
| 1 | Use-driven memory sync from fault stamps, no per-submission scan | **5.5-7** | 6-8 (+ less pool-lock contention) | **8-12** | M-H for MKW (Turbo same game and machine), M-L for OoT |
| 2 | O(1)-keyed sets, texture descriptors by ID from the pool snapshot, smaller sets | 2.5-4 | 7-9 | 10-13 (of 19) | M |
| 3 | Lanes become optional once 1, 2 and 4 land | 0-2 | 10+ freed | ? | M |
| 4 | Dynamic-state dedupe | 0.3-0.7 | 1-2 | 1-2 | H |
| 5 | Present thread, render-target recycling, push descriptors, async pipelines | ~1.2 + churn | — | — | M |

Items 1 + 2 + 4 together: est. replay thread 20.5 → ~10-12.5 ms in MKW, which is Turbo's measured envelope (9.5-11.3 ms)
on this machine, and inside 16.7 ms.

## 3. Recommended next waves

### Wave USE-SYNC (first): pool coherence per use, not per submission

**Step 0 (no code, ~20 min).** One quiet MKW run on current HEAD (sealing on): `[vk] pools:` parts (lock / scan / pass,
sealed count). If sealing already took MKW's scan from 4.4 to ~1 ms, the wave's expected gain shrinks to ~3-4 ms; note
it and continue.

**Build** (reimplemented from Turbo's and nxnative's designs; AGPL parts of Turbo are not pasted):

1. **Per-page copy stamps and a write generation** (`vk/mem.rs` `Pool`, beside `page_gen`): `up[pg]` is the generation
   at the page's last upload; `hot[pg]` and a streak counter; `hot_at[pg]` is the submission that copied it.
   - The fault handler (fold into `vk/seal.rs`, which already has the vectored handler, the commit hook and the
     host-write wake) stamps the page's write generation and unprotects it.
   - After an upload the page is re-protected unless hot.
   - Pages written 3 frames running turn hot (writable, copied once per submission at first use); a hot page quiet for
     N frames turns cold again and is protected.
2. **Sync at use, done in the pre-pass on the replay thread** (`exec.rs` pre-pass / `plan` 9194, `segmentable` 9138).
   For every record that binds memory, check the range's pages against their stamps and upload what is behind into
   `sub.pre`. That covers UBO, SSBO, vertex and index buffers, indirect records, copy sources, and SSBO addresses read
   from UBOs. A per-submission (addr, len) hash like Turbo's `g_seen` skips repeats.
   - This keeps lane `resolve` read-only, so the gate does not fire.
   - The serial path calls the same check at its resolves.
3. **Raw global memory** (`gmem.rs`: pools reached through loaded pointers). Keep the existing scan, but **only over
   pools in the gmem table while such shaders draw**, as Turbo does: every written page goes up once a frame while
   raw-load shaders are bound.
4. **Texture CPU-write checks** (`image_begin_submit`). Read the page stamps of the texture's storage at bind, instead
   of the sync's dirty list (nxnative: `nvn_dirty_since_span` with a stamp per texture).
5. **`begin_submit`**: no scan and no per-pool pass when `NXGFX_USE_SYNC=1` (new switch, `docs/env_switches.tsv`).
   Keep a budgeted round-robin background sync (Turbo `vkm_sync_all`) for pools no use reaches. Default stays off until
   the lead's audit.
6. **Upload skip** (`upskip.rs`) stays: it still saves uploads of rewritten-but-equal pages at use.

**Files:** `vk/mem.rs`, `vk/seal.rs` (→ the one protection tracker), `vk/exec.rs` (pre-pass and resolves), `vk/image.rs`
(texture check), `vk/gmem.rs`, `vk/lazy.rs` (page ownership: lazy write-back and protection must agree on who owns a
page's protection), `src/os/src/mem.rs` (host-write wake, already hooked by SCAN-COST).

**Measure** (`mkw` window [250, 325]; `oot` window [845, 915]; off / on):

- FPS and 5 % low.
- replay / prep ms, and `[vk] pools:` parts.
- New counters: uses a frame, use-cache hits, ranges synced, pages copied, hot pages, faults a frame, bytes uploaded.
- Completion `g.mem` hold time.
- `NXGFX_TRANSLATE_CHECK=1` (0 entry-state differences).
- A verify mode `NXGFX_USE_SYNC=verify`: the old scan runs too and every page it would upload that use-sync did not
  (and that a later draw binds) is a red line; it must be 0.
- Sheets read.

**Expected:** MKW replay thread 20.5 → est. 14-15.5 ms. That alone may lift the 30 hold, but too close to 16.7 to count
on. OoT prep 16 → est. 4-8 ms.

**Risks.**

1. A missed CPU write: an unprotected path writes guest memory. Mitigations: SCAN-COST's commit hook and host-write
   wake, the verify mode, and kirikae-style `memcmp` of bound ranges in a debug mode.
2. Fault storms on pages that are not quite hot; the streak and hot-gap rule bounds them (Turbo's `hot_gap 2`: a
   double-buffered upload writes each buffer every other frame).
3. Kernel-mode writes into a protected page fail instead of faulting. SCAN-COST found none by reading; keep its leak
   check.
4. Ownership conflicts with lazy write-back's protections.
5. The ~7 submissions a frame in MKW (event-wait parking): hot pages are copied per submission. Turbo has the same shape
   and lives with it.

### Wave SET-FAST (second, can overlap in another worktree; different files)

1. **Dynamic-state dedupe** (`exec.rs` `draw_setup_ex` 6561, `vertex_input` 7138): a per-command-buffer shadow of every
   dynamic state, reset at a new CB, a child CB, or any non-draw pipeline bind (clears, blits, host kernels).
   `NXGFX_DYN_DEDUPE=0` restores the old behaviour.
2. **O(1) set reuse.** Per stage, a bind version bumped by any bind that changes a handle or range (TRANSLATE-REUSE's
   content generations already exist: `content_gens`). Reuse when (module, bind version, sub serial, image epoch, pool
   generation, base instance) are unchanged, without building `SetKey`. Patch when only the UBO class moved: copy the
   shadow, rewrite only the UBO descriptors (nxnative `args_cbuf_only`).
3. **Textures by ID.** For a slot texture whose declared view type matches the ID's registered view (a per-ID type tag
   array), `memcpy` the ID's sampled-image descriptor bytes from the current `PoolSnapshot` into the set. No `tex_view`,
   no images lock; a mismatch (cube vs array, integer vs float class, buffer texture) falls back to today's path.
   - Later step, a translator change, not this wave: slot textures read the pool set directly by the handle's ID, as
     Maxwell does, which removes per-draw texture work entirely.
   - Combined image samplers need the sampler too: the same from the sampler pool, or split the slot into a separate
     sampled image and sampler.
4. **Per-module set size** (optional, measured first): write and copy only the descriptor range the module's bindings
   reach (the highest binding offset), not the whole 176-binding stage size.
5. **Measure:** sets a frame by path (reused / patched / whole), µs a set, lane CPU, segments' wall, replay ms, FPS.
   Then **`NXGFX_TRANSLATE_THREADS` = 0 / 2 / 6** on the new build: keep the smallest that holds 60.

**Expected:** sets 12.8 → est. 3-5 ms of CPU; replay 11 → est. 7-8.5 ms. With USE-SYNC, MKW replay thread est.
10-12.5 ms, Turbo's envelope.

**Risks:**

- A reused set naming a stale view: the epoch must move on every image backing or ownership change. That was
  TRANSLATE-REUSE's change 7, which needs audit.
- Dedupe across a pipeline bind with static state: Turbo's rule is to forget after any non-draw pipeline.
- Ordering in lanes (the dark-turn class of fault): keep the shadow per child CB.

### Not recommended now

- **Translating at NVN-call time.** No reference does it. MKW-RECORD showed ~10 of 15 ms reads replay-time state.
- **Direct (imported) guest memory.** Measured a net loss on this dGPU: per-draw constants over PCIe raised GPU time
  5 → 8-9 ms and triggered render-target churn (DIRECT-MEM).
- **One sparse buffer spanning the guest address space** (making resolve pure arithmetic, nxnative's "GPU address = CPU
  address"). Plausible on NVIDIA, but its gain is bounded by Turbo's cached lookup, which already costs ~nothing. Revisit
  only if resolve shows in a profile after USE-SYNC.
- **kirikae's command-processor (LLE) path.** It costs more per draw by construction, and kirikae itself is moving away
  from it.

## 4. What nxrs already has that matches the references (do not redo)

- Record / flatten on the game thread at submit; one replay thread in queue order (`queue.rs` `submit`, `aq_main`):
  matches nxnative `nvn_queue_submit` / `nq_worker` and Turbo.
- Event waits park their queue only, and a queued GPU signal satisfies a wait (`exec.rs:8887`): nxnative's
  `nq_event_write` rule.
- Lazy write-back on demand (`lazy.rs`, `NXGFX_WB_LAZY` on): Turbo's `PG_LAZY`. Write-back page comparison and the
  upload skip for equal pages (`upskip.rs`) go beyond the references.
- Write protection of quiet pools with a fault handler, a commit hook and a host-write wake (`seal.rs`, SCAN-COST): the
  mechanism USE-SYNC needs. Extend it; do not write a second one.
- Bindless pool set by texture and sampler ID (set 6, `tables.rs` shadow and `PoolSnapshot`): matches nxnative
  `TEX_POOL` / `pool_gpu` and Turbo's set 5. SET-FAST item 3 reads from it.
- Bind-same / content generations for banks (TRANSLATE-REUSE changes 1 and 3): nxnative's "do not mark dirty if the
  state did not change".
- Lock-free pool bases, `vpool` span fast path, the tables' reverse index (TRANSLATE-COST 1-3).
- Pipeline disk cache, prewarm and blacklist (`pcache.rs`, `pipe.rs`); EDS3 and dynamic vertex input; pipeline lookup
  is only ~0.4 ms a frame.
- The NDSR host blit (DKB); OoT's NDVE host graph is roadmap R4 / R5 (nxnative `nvn_ndve.inc`; Turbo `ndve_host`
  skips the network for 60 FPS movies).
- Host-side profiling with stacks (`NXRS_HOST_PROF_STACK`) and the `NXGFX_PROFILE` set buckets: enough to measure both
  waves.

## 5. Uncertainties, stated plainly

- Apple M6 and this machine's cores differ; nxnative's 6.4 ms is a shape, not a target. Turbo's 9.5-11.3 ms (same game,
  same machine, same API, same GPU) is the target.
- MKW has not been measured on HEAD since sealing (SCAN-COST measured OoT only). USE-SYNC's MKW gain may be smaller than
  estimated if sealing already took much of the scan.
- OoT's frame (51 ms) is not bounded by the replay thread alone (replay + prep 35 ms; SCAN-COST). The guest side and the
  NDVE / NDSR work are outside both waves. OoT will not reach 59 from these waves alone; nxnative's OoT also runs the
  network by host kernels and stubs dynamic resolution.
- Per-set cost attribution (key building vs copies vs lookups) is from earlier profiles. SET-FAST should take one
  `NXGFX_PROFILE` split on HEAD before changing the path.
- kirikae publishes no frame-time numbers for 3D titles beyond "30 fps on a modest GPU", and none for Switch 2 titles.
