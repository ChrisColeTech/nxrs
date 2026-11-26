# TRANSLATE-REUSE — stop re-translating unchanged NVN command streams every frame (MKW to 60)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~15:30, worktree
`D:\Projects\RecompLauncherEngine-reuse`, branch `reuse` (base `gameplay-harness` `1619d31`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World (`mkw`, `mkw-turn`), GPU 0. Deadline 8 h (profiling and
design 2 h). Another wave (SCAN-COST) owns Ocarina of Time on GPU 1.

## Goal and baseline

MKW presents at its 30 cap: the replay thread needs ~21-22 ms a frame (translation ~11-12 ms, prep ~9-10.7 ms; 16.7
needed). Lead's quiet run `20261126-083639-lead-tc-on` (window [250, 325] s, frames CSV means): 30.7 / 29.0 FPS,
replay 11.0 ms, prep 9.5 (pool sync 7.3), descriptor time 12.8 ms a frame (all lanes), texture uploads 0.38 ms, GPU
4.8 ms; translate lines: 3.0 streams a frame in segments (~10 segments, 6 lanes, segments' wall 7.3-8.2 ms, pre-pass
0.8 ms, lanes' CPU 17-22 ms) and 6 short streams in one piece.

## Plan

1. **Profile first** (2 h limit, diagnosis only):
   - `NXGFX_PROFILE=1` breakdown (sets: memory, views, banks, keys, writes; pipeline lookups; passes; vertex input).
   - New `NXGFX_REUSE_PROBE=1`: how much of each frame's segmented streams repeats the bytes of one of the 4 frames
     before it, by whole segment and by record kind. This decides direction (1): whole-segment reuse needs the
     segment's bytes (and its entry state, and every guest word translation reads) to repeat.
   - `NXRS_HOST_PROF` / `NXRS_HOST_PROF_STACK` on the lanes if the counters leave the split unclear.
2. **Pick by measured share** among: (1) reuse of whole translated segments; (2) descriptor sets kept across
   segments and frames, keyed by content; (3) the largest per-draw costs. The choice and its numbers go below.
3. **Build** it behind a switch (`docs/env_switches.tsv`), with unit tests; `cargo test --release -p graphics`.
4. **Verify**: `mkw` off / on (FPS and 5% low against 60, replay / translation / prep ms, reuse hit rates),
   `NXGFX_TRANSLATE_CHECK=1` once (0 entry-state differences), `mkw-turn` once for pictures; every sheet read.
   Also measure whether MKW's render-target churn (dynamic resolution, each target uploaded from storage) is part
   of the frame cost here (`tex_upload_ms`, `uploads from storage` lines).

## Profile (2026-11-26 15:40-16:00; GPU 0; SCAN-COST's OoT builds and runs on GPU 1 overlapped)

`build/play/20261126-154209-tr-probe-mkw` (base build + `NXGFX_PROFILE=1` + `NXGFX_REUSE_PROBE=1`; the probe hashes
every segment and record on the replay thread, so its own ms are inflated: shares, not ms), window [250, 325] s.

- **Nothing repeats from frame to frame, so whole-segment reuse (direction 1) is ruled out.** Segments a frame: 0.0
  repeated / 9.5 new (1.3 MB of records). By record, only 22 % repeat the bytes of one of the 4 frames before:
  BIND_TEX1 (12,800 a frame) 13 %, BIND_UBO_SMALL (7,500) 40 %, BIND_STATE 3 %, BIND_SSBO 26 %, DRAW_IDX 33 %,
  vertex stream / attribute binds 2-5 %. The texture handles themselves change every frame (a per-frame ring of
  texture IDs), as do the uniform ranges: a stream's translation cannot be keyed by its bytes, and anything keyed by
  handles (sets, views) cannot live past the frame. Descriptor sets keyed by content across frames (direction 2's
  persistent part) fall for the same reason.
- **Set work is most of the lanes' CPU** (`replay on the CPU` lines, ms a frame over all lanes): sets 11-14 (views
  3-6, banks 1.3-1.9, writes 1.7-2.5, memory 0.1-0.5), passes ~1, vertex input ~0.9, pipeline lookups ~0.4.
  ~2,300-3,300 sets a frame: ~40 % bound again, ~2,000 patched (2.3-2.7 us each), ~500 built whole (7-10 us each).
- **Why patched:** uniform buffers 93 % of patches (new uniform ranges at nearly every draw: inherent), and **c2 +
  textures 35-47 %**: every texture bind moved the stage's handle-bank generation, so the next set of the stage
  re-resolved all its textures (views under the images' lock). The game binds textures one at a time (BIND_TEX1,
  ~6.6 a draw), and 45 % of all bank binds (16,000 of 35,000 a frame, counted after the change) bind the handle or
  range already in the slot.
- **Why whole:** 97 % "no copy of the module": each segment (and each child submission) starts with empty set
  caches and host copies (`fork_state`), so the first set of each module in each segment is built whole.
- **Lanes idle:** 3 streams a frame are segmented into ~9.5 segments with the fixed 256-draw size; the shorter
  streams get 1-2 segments, so most lanes wait while one or two translate them (segments' wall 8.5 ms against
  ~20 ms of lane CPU over 6 lanes).
- Texture uploads (`tex_upload_ms`) 0.3-0.6 ms a frame and `uploads from storage` 0.1-0.6 ms a frame: the
  render-target churn seen in DIRECT-MEM comes only when GPU time rises; at today's 5 ms GPU it is not a frame cost.

## Changes

1. **Bank binds of what is bound already keep the bank** (`exec.rs` `bind_changed`, `QState::bind_tex1` /
   `bind_ssbo`, the multi-handle binds and compute's uniform binds): the c0 / c2 generation moves only when a handle
   or range changes. c0 / c2 bytes are made only from those arrays, so an unmoved generation means the same bytes and
   the same sets. `NXGFX_BIND_SAME=0` off. Effect (`20261126-155238-tr-same-prof-mkw`): patches remaking c2 18-21 %
   (was 35-47 %), textures 22-25 % (was 36-48 %); sets bound again ~47 % (was ~40 %).
2. **Segments spread over the lanes** (`spread_target`): a parallel stream's segment size is
   `ceil(draws / lanes)` within [`NXGFX_TRANSLATE_SPREAD_MIN` (32), the fixed size], mid-pass cuts at the same size.
   `NXGFX_TRANSLATE_SPREAD=0` off.

3. **Bank generations by content** (`content_gens`, `bterm`, `QState::put_handle` / `bind_ssbo` / `bind_ubo` /
   `hash0` / `hash2`): a stage's c0 / c2 generation is the XOR of a 64-bit hash of every non-empty slot of the tables
   the bank is made from, updated per changed slot, recomputed whole at the rare bulk writes (compute snapshots,
   deferred buffer binds). The same tables always give the same generation: a texture bound and bound back, the
   pre-pass and the segment, one segment and the next. (Replaces change 1's mechanism; same switch,
   `NXGFX_BIND_SAME=0`: a new number at every bind.) Collision risk: two different tables with one generation needs a
   64-bit hash collision.
4. **Set copies carried between a lane's segments** (`set_carry`, `NXGFX_SET_CARRY=0` off; `SetKey::run`,
   `patch_classes`' sibling rule, `gate::Run::epoch`): a lane hands its stage sets' host copies and c0 / c2 banks from
   one segment of a parallel run to the next it takes, so the first set of a module is patched instead of built whole.
   A sibling's set may be patched because its staged descriptors (c0 / c2 / c1 / support / push) live in a sibling
   child's staging, retired with the same parent. Only from a segment that did not give up and while the run's change
   epoch is the one the earlier segment began in.
5. `NXGFX_TRANSLATE_SPREAD=<n>`: segments a lane (default 1; 0 = the fixed size).

### Checks after changes 1-4 (`20261126-163708-tr-carry-prof-check-mkw`, `NXGFX_PROFILE=1 NXGFX_TRANSLATE_CHECK=1`)

- **0 entry-state differences** (16 translate lines), 3/3 on route, sheet correct (boot, title, menus, grid, race,
  lap 2), stand-ins only at load (19, none after 166 s; base run 21), red lines the known two.
- Gate: in the window 0; at load (90 s) up to 937 segments gave up in 600 frames (base run peak 42): more segments
  per stream at load mean more first-use changes outside the oldest segment; they are translated again at the join
  (correct, slower loading).
- **Where the wall is** (new streams-by-size line): 2 streams a frame run in parallel, one of 512-1,023 draws and one
  of 1,024 or more, each in ~4.4-6 segments, i.e. one segment a lane: each run takes ~4-4.5 ms, about what one lane
  needs for its ~200-250 draws (~16 us a draw against ~7.5 serially). So **carrying never happened** (0.0-0.5
  segments a frame began with a carried copy): no lane takes a second segment of a run.

### Series 2 (16:45-17:08, same build, switches per run; window [250, 325] s)

| Run (`build/play/`) | switches | FPS / 5% low | replay | prep | segments wall | lanes CPU | segments a frame | valid |
|---|---|---|---|---|---|---|---|---|
| `20261126-164517-tr-r2-off-mkw` | all off | 15.0 / 11.0 | 20.1 | 29.1 | 13.5-14.5 | - | 7-10 | **no**: 20 compiler processes (another wave's build) through the window |
| `20261126-165058-tr-r2-on1-mkw` | defaults (spread 1) | 29.9 / 27.0 | 10.9 | 14.5 | 7.3-7.9 | 22-24 | 10-12 | yes |
| `20261126-165637-tr-r2-on3-mkw` | spread 3 | 28.7 / 24.0 | 11.7 | 16.5 | 7.5-7.9 | 29-32 | 28 | yes (carry: ~15 segments a frame) |
| `20261126-170215-tr-r2-on3t10-mkw` | spread 3, 10 lanes | 4.4 / 1.7 | 30.1 | 29.5 | 27 | 64 | 47 | collapsed: 7,638 segments translated again, images' lock 15.8 ms |

- More, smaller segments do not shorten the wall (lanes' CPU rises with them); 10 lanes make the gate give up
  thousands of segments. Spread 1 stays the default; `NXGFX_TRANSLATE_SPREAD=3` and carrying help nothing measurable.
- **The machine is shared and noisy:** OoT (SCAN-COST) runs throughout; another title (`Anime Girls Sun of a Beach`,
  nxrs) ran on GPU 0 at 16:22; builds of other waves ran during `tr-same-off`, `tr-ab-on`, `tr-r2-off`. Two runs
  collapsed to ~3-14 FPS in the race (`tr-same-off` with the old bind behaviour, `tr-ab-on` with the new): GPU time
  rose to 11-19 ms a frame (texture uploads of 30-120 ms in single frames first), then MKW's dynamic resolution
  churned render targets; the same collapse with either switch setting: not attributed to this wave's changes.

### Lane profile (`20261126-171056-tr-prof-stack-mkw`, replay thread + 5 lanes, stacks, PDB; [260, 290] s)

21,786 samples inside `par::work`: **contended mutexes 30 %** (`Mutex::lock` inclusive; 26 % in `WaitOnAddress`), of
it: `room_cached` misses into `mem::room` (the pool lock) 8.8 %, `tex_view_uncached` (the images' lock) 9.0 %,
`resolve_in` (pool lock) 3.5 %, `target_image` (images) 2.0 %, `note_gpu_write` (pool lock) 1.4 %. The driver
(`nvoglv64`) 13 % self. `stage_set` / `patch_set` / `bind_sets` hold most of the rest.

- **Room misses are compulsory:** keying the per-thread room memo by the pools' layout generation instead of
  `POOL_GEN` (change 6) left the misses as they were (700-900 of ~6,500 a frame): new uniform addresses each frame
  (the game's per-frame rings), not deferrals. The lasting cure is a pool map readable without the pool lock
  (`mem.rs`, outside this wave).
- **The view memo was useless:** the tables' generation moved ~200 times a frame (`NXGFX_PROFILE` site counts over
  600 frames: `write_image` pool descriptor writes 57,000, `note_tex` 50,000, `ensure_image` 7,500, `tex_release`
  6,900, placeholders ~400). Every move emptied every segment's view memo and moved every stage set's `image_gen`.
  Change 7 keys them finer.

6. `mem::LAYOUT_GEN` (moves with pools coming, going or getting buffers, not with deferrals) keys `room_cached`.
   No measured effect; harmless.
7. **Views keyed per texture ID, stage sets without pool-descriptor churn** (`NXGFX_VIEW_ID_KEY=0` off):
   `Tables::view_version(id)` = (image epoch: image backing / ownership changes, every ID) + (the ID's registration
   version: a texture, placeholder or buffer texture registered there; 2^20 slots, IDs sharing a slot share it);
   `exec::tex_view`'s memo is keyed by it. `Tables::set_generation` is the generation without the pool's own
   texture / texel-buffer descriptor writes (`pool_descriptors_changed`: `write_image`, `write_texel_*`), which no
   stage set reads (sets look their views up themselves; buffer textures make a set unkeyed); `SetKey::image_gen`
   and `TexPart` use it. Samplers' writes still move it.
8. Translation lanes join the frame threads' scheduling class (`rtclass::join`, MMCSS "Games"; `NXGFX_RT_LANES=0`).

Expected files: `src/graphics/src/vk/exec.rs` (and `gate.rs` / `queue.rs` if the chosen direction needs them),
`docs/env_switches.tsv`, this document.
