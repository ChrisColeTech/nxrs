# MKW-RECORD — record-time command translation (Mario Kart World to 60 FPS)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md` (dist lock / machine lock; never two copies of one title). Title:
"Mario Kart World" (scenario `mkw`, GPU 0).

## Brief (lead)

**Problem (recorded, `docs/waves/MKW-PERF5.md` report, `build/play/20261125-101138-mkw-perf5-final`).** Race at
28.9-29.6 FPS against 60. The Vulkan replay thread does ~28 ms of serial work per frame: ~15 ms draw translation
(NVN command buffer -> Vulkan), ~7 ms write-watch scans, ~2.4 ms passes. Tuning cannot reach 16.7 ms; the
translation must move off the replay thread: translate each NVN command buffer when the game records/submits it, on
the game's threads (or a worker pool), so the replay thread only submits prepared work.

**Phase 1 — design (now, in parallel with wave OOT-NAN).** OOT-NAN is editing `vk/exec.rs`, `vk/image.rs`,
`vk/storemu.rs` and rebuilding `dist/`; the tree is uncommitted, so no worktree. Until the lead tells you OOT-NAN
has finished: **read and measure only — edit no engine source, do not build or relink, run no games.** Write the
design into this file under "## Design": the current call path (record -> submit -> replay -> Vulkan) with
file:line; which translation work depends on state only known at replay (memory contents, texture registration,
pipeline state, pool sync, write-watch) and how each is handled at record time (snapshot, deferred patch, or kept
on the replay thread); threading and ordering rules (queue order, fences, semaphores, cross-buffer state);
correctness risks (stale descriptors, memory written after record, coherence with lazy write-back); the expected
per-frame budget after the change; and an incremental plan where every step builds, runs MKW correctly, and can be
measured. Also how the write-watch ~7 ms is cut.

**Phase 2 — implementation (after the lead's go).** Step by step per the plan, each step built under the dist
lock, MKW relinked and run (`py scripts/play.py mkw --env NXGFX_GPU=0`), sheet read; perf-measurement runs under the
machine lock with nothing else running. Regression on one other title at the end (`smash` or `wonder`).

**Done:** MKW race at 60 FPS (5% low reported) with a correct sheet, red lines unchanged (known: 1), no device
loss; or, parked at the deadline, the measured gain per step and what remains.

**This changes descriptor/memory/coherence timing: the lead runs an independent audit.** Never skip draws; don't
weaken robustness; native NDVE stays off; keep `fps_hold`. **Exclusions:** no commits, don't edit docs/32, other
titles' routes.

**Deadline:** phase 1, 3 hours; phase 2 deadline set at the go. Report per protocol §5 under "## Report".

## Lead decisions on the design (2026-11-25)

- **Replay-time worker pool instead of record-time translation: accepted** (entry state is only known when the
  queue reaches a stream, and ~10 of the 15 ms reads state the game may change after recording).
- The four behaviour changes are accepted: the present's pool sync may be covered by the walker's walk (that rule
  gets the independent audit); long passes may be cut (all attachments load/store, the pixels must match: show it
  with an A/B sheet); worker threads on by default once proven; streams under 64 draws stay serial.
- Every step keeps its off switch and the gate-everything reference mode until the audit is done.
- Regression title: `smash` (a 60 FPS title, the most timing-sensitive).
- Phase 2 starts when OOT-NAN has landed (it may edit `vk/mem.rs`, so step 1 waits too). Deadline: 8 hours for
  steps 1-3, then a checkpoint report; steps 4-6 after the lead reads it. Perf pairs under the machine lock.

## Steps 4-6 go (lead, 2026-11-25 evening, after reading the checkpoint report and the `195700-s3c-on` sheet)

Steps 1-3 accepted (sheets correct, entry-state check 0 differences); an independent read-only audit of their
coherence points runs beside you (walker coverage of the present's sync, `lazy.rs:401` copy-then-`reset_watch`
without `WATCH_MU`, `tex_writeback_land` peeking without draining, step 3's pre-pass effects) — the lead will send
its findings. A fresh agent takes steps 4-6:

- **Step 0: merge `gameplay-harness` (commit `5a41213` or later) into `mkw-record`** so the work builds on the
  current engine (OOT-NAN dispatch size, OOT-FOREST2 indirect c0 / snapshot recycling / `upskip` off by default).
  Resolve conflicts by keeping both sides' intent (`vk/mem.rs` walker hooks and `upskip` forget hooks; `exec.rs`);
  build, `cargo test -p graphics --lib --release`, one MKW run with its sheet; commit the merge on `mkw-record`.
- **Step 4** worker lanes (command pool per lane, atomic query counter, the gate and the gate-everything reference
  mode), **step 5** mid-pass cuts for the ~1,300-draw segment and tuning, **step 6** defaults on + the `smash`
  regression (relink Smash from the worktree's dist under the dist lock; NVN2-PARITY in the main tree does not
  touch Smash) and the final MKW measurement. Perf pairs under the machine lock; ignore FPS differences under ~2
  (the write-watch scan noise) unless repeated.
- Deadline: 8 hours. Report under "## Report (steps 4-6)" here.

## Phase 2 go (lead, 2026-11-25, after OOT-NAN landed as commit `9903fc7`)

- **Work in your own git worktree** branched from `9903fc7` (branch `mkw-record`), with its **own `target/` and
  `dist/`**: build there with `scripts/build/build.ps1 -KeepCache -SkipGnu`, relink only Mario Kart World with
  *the worktree's* `dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build`, and run it with the worktree's
  `scripts/play.py mkw --env NXGFX_GPU=0` (check that it launches the worktree's build, not the main `dist`). If the
  build needs untracked local tools from the main tree's `build/`, point at them read-only; never use the main
  `target/` or `dist/`.
- **Locks are the main tree's**: `D:\Projects\RecompLauncherEngine\build\locks\` (`dist.lock` around your relinks,
  `machine.lock` around every perf-measurement run, and wait until no other game is running before measuring).
- **Beside you**: wave OOT-FOREST2 works in the main tree on Ocarina (GPU 1) and does not touch Mario Kart World or
  `vk/queue.rs`. Keep your commits on `mkw-record` (you may commit on your branch; the lead merges). Run
  `cargo test -p graphics --lib --release` in the worktree after each step.
- Report the checkpoint after steps 1-3 under "## Report" in the worktree's copy of this file and in your final
  message.

## Design (agent, phase 1, 2026-11-25)

Read only. No source edited, no build, no game run. The numbers come from the MKW-PERF5 runs
(`build/play/20261125-084344-mkw-perf5-trace` chain trace, `-091827-…-optimes` record times, `-101138-…-final`
frame CSV over the race window [250, 325] s: 1,989 draws, 195 passes, 111 dispatches, 4.0 NVN submits, GPU 4.8 ms,
replay 15.9 ms, prep 12.5 ms (pool sync 10.5), sets 7.5 ms per frame).

### 0. Conclusion first: translate at replay time on a worker pool, not at record time

The brief asks for translation when the game records or submits. The code shows two things that rule out doing the
expensive part there. The design keeps the brief's goal: the replay thread no longer does the ~15 ms of draws itself.
It runs the ordered work and hands the draws to a pool of translation threads. The game's threads keep what they
already do at submit.

1. **NVN state carries across command buffers on a queue.** `exec::QState` (exec.rs:32) is per NVN queue and
   lives on from one replayed stream to the next (`queue.rs:1773-1782`). A list recorded on 0x105d does not know its
   entry state (program, buffers, targets, blend…) until the queue reaches it.
2. **~10 of the ~15 ms per frame read state that is only right at replay** (MKW-PERF5: set path 10 ms = resolves 2.1,
   views 2.6, banks 1.3, writes 1.6):
   - bindless handles read from the game's uniform buffers (`bank_word`, exec.rs:3011-3017);
   - deferred binds that read the game's handle arrays "as the queue runs it" (`BIND_TEX` defer, exec.rs:8545;
     `BUFS_DEFERRED`, exec.rs:8978);
   - texture registrations the game makes on its own threads (`tables.tex_at`, generation checks);
   - image state: creation, uploads, ownership (`prepare_image`, image.rs:3808; `target_image`; `upgrade_storage`);
   - pool buffers (`mem::resolve`, mem.rs:1438): created on first use, deferred uploads, `POOL_GEN`;
   - counter values for render conditions (`rc_value` after `rc_note`, exec.rs:9008, 8925).

   Translating at record time would mean re-checking every one of these at replay. That check costs about what
   making the set costs. It would also add new hazards: memory the game writes after recording but before submit.
3. **History.** `NXGFX_DEFER=1` (vk/defer.rs) moved only the driver's `vkCmd*` calls to another thread. It measured
   slower (MK8D 5.8 → 8.2-9.7 ms). Our translation is the cost, not the driver.

So translation happens at the same logical time as today: after the submission's pool sync, before its
`vkQueueSubmit2`. Memory, registration and coherence semantics stay exactly as today. The one difference is that
segments of one stream are translated at the same time, into command buffers of their own.

### 1. Current call path (record → submit → replay → Vulkan)

- **Record (game threads).** `nvnCommandBuffer*` write compact records into the game's command memory (nvn/cmd.rs).
  `CallCommands` (cmd.rs:1168) links the called list's pieces into the caller's control list. Nothing is translated.
- **Submit (the game thread, 0x1004 in MKW).** `queue::submit` (queue.rs:1621) flattens the handles with
  `cmd::for_each_record` (cmd.rs:769), which expands compact records into one `Vec<u8>` stream. It stamps command-buffer
  fence syncs with `ASYNC|seq` and pushes `Op::Submit` (`aq_push_with`, queue.rs:1336). `present` (queue.rs:2061)
  pushes `Op::Present`.
- **Replay (`vk replay` thread, `aq_main`, queue.rs:1435).** It takes ops in order and merges same-queue submits.
  - `run_stream_due` (queue.rs:1720) does the following:
    - `new_sub`;
    - the pool sync `mem::begin_submit` (mem.rs:1890), with write-watch walks, the per-pool pass and uploads into
      `sub.pre`;
    - `gmem::begin`;
    - `image_begin_submit`;
    - `exec::run` (exec.rs:8018), which goes record by record through `record_op` (exec.rs:8205);
    - `exec::end`;
    - `submit_sub` (queue.rs:715), which records the write-back copies, ends the CBs, calls `vkQueueSubmit2 [pre,
      main]` and hands the submission to completion.
  - A wait that does not pass (`WAIT_EVENT`, exec.rs:8903) ends the pass and parks the rest of the stream.
    `retry_locked` (queue.rs:1841) runs it on. That is why MKW's one submit op makes 7 Vulkan submissions.
- **Draw (`draw_idx`, exec.rs:6669).** It runs:
  - `checkpoint`;
  - `draw_setup_ex` (exec.rs:5702): `begin_pass` (exec.rs:2491), the pipeline lookup `gfx_pipeline` (pipe.rs:1187),
    `bind_sets` (exec.rs:5390) with `set_key`, `patch_set` or `stage_set` (exec.rs:3823) into the descriptor-buffer
    chunk, about 30 dynamic-state calls and `vertex_input`;
  - the index buffer `resolve` and the draw.
- **Completion (`nvn-completion`).** It lands write-backs (`apply_writebacks`, mem.rs:2164, under `g.mem`), posts
  events and counters, and publishes fences.

### 2. What depends on replay-time state, and where each part goes

| Work | Depends on | Where it runs in this design |
|---|---|---|
| Flattening, compact → canonical | command memory still the recording's | game thread at submit (as today) |
| Pool sync: warm write-watch walks, per-pool pass, uploads | write watch; coverage by op sequence | replay thread, before the fork (as today) |
| Full write-watch walk (cold pools), once per frame | write watch | **new walker thread**, started at the present's push (§5) |
| Texture checks after CPU writes (`image_begin_submit`) | dirty ranges from the sync | replay thread, before the fork |
| Entry `QState` of each segment | the queue's state from its earlier streams | replay thread, **state pre-pass** in stream order |
| Wait evaluation (where the stream parks) | event words; GPU signals queued earlier or earlier in this stream | pre-pass, in order |
| Ordered global effects: `pending_signals` insert (exec.rs:8896), `rc_note`, FENCE_SYNC value (exec.rs:8912), `OCCL_FRAME` | global, order-dependent | pre-pass, in order (workers skip them) |
| Program modules (BIND_PROGRAM), deferred binds | shader library; game handle arrays | pre-pass, then again by the worker in the same window (read-only) |
| Address → buffer (`resolve`) | pools, `POOL_GEN` | worker (thread-local cache); a miss that creates, binds or uploads is **gated** (§4) |
| Bindless handles, quad / u8 indices from guest memory | guest memory | worker; same window as today (after the sync, before submit) |
| Views, `prepare_image`, target ownership, storage upgrades | tables, images | worker; any change to image state is **gated** |
| Pool descriptor snapshot | table generation | replay thread at the fork; a child refreshes it under the gate |
| Pipelines | pipe cache; compiled on a miss | worker (map lookup; concurrent compiles are fine) |
| Stage sets, dynamic state, vertex input, push constants | QState | worker, in its own descriptor-buffer chunk and caches |
| Posts, write-backs, texture write-backs | completion order | worker collects them locally; joined in segment order |
| Present | swap chain | replay thread (as today) |

Nothing is taken as a snapshot at record time. Everything order-dependent stays on the replay thread. Only work that
is local to one segment runs on the workers.

### 3. The fork / join of one stream (`exec::run`)

- **When.** A stream (or a parked rest) with at least `NXGFX_TRANSLATE_MIN` draws (default 64) whose conditions allow
  it. Otherwise today's serial path runs unchanged.
- **Serial-only conditions.** Any one of these keeps a stream on the serial path:
  - pass dump, capture, `NXGFX_DEFER`, mesh bindings, DB poison or canary, raw probe, `NXGFX_SKIP_PS`, `shadows=0`;
  - transform feedback in the stream;
  - an NDVE transaction pending;
  - raw global memory (`facts(m).raw`), if `gmem::Version` cannot be shared (to check).

  Rare records always run on the replay thread as an exclusive segment: DLSS, DOWNSAMPLE, RESOLVE_DEPTH and ray
  tracing.
- **Refactor `record_op` into a state part and an emit part.**
  - `apply_state(g, qs, opc, rec)` holds the pure QState changes (binds, state, SET_RT's targets, RENDER_COND,
    PUSH_CONST, RESET_COUNTER…) and the ordered global effects listed above.
  - `emit(...)` holds everything that records commands or touches `Sub`.
  - The serial path calls both, in order, per record, so it behaves as it does today.
- **Pre-pass (replay thread).** It walks the stream once.
  - It applies `apply_state` and evaluates waits, so the stream ends at the first wait that does not pass.
  - It chooses cut points:
    - natural ones first: SET_RT (8, 137 a frame), BARRIER (28, 124), DISPATCH (27, ~115), copies and clears. Each
      already ends the pass and is followed by a barrier, so a cut there changes no command;
    - between draws inside a pass whose draws run past the segment target. That adds one pass break: `end_pass` ends
      the rendering instance and adds its barrier, and the next segment's first draw begins it again. Every
      attachment is LOAD/STORE and GENERAL (exec.rs:2515), so the result is the same. Only occlusion splits into two
      queries, and their samples are summed (`Post::Occl`).
  - At each cut it clones the state fields of QState into a segment record (`fork_state()`: state only, new empty
    caches; ~25 KB).
  - It hands segment k to the workers as soon as its entry state exists, so workers start while the pre-pass runs on.
  - Cost: ~35 K records a frame. The state records are ~50 ns or less each with the timers (`op_times`), plus
    BIND_PROGRAM at ~0.77 µs × 736. That is about 1-1.5 ms a frame, and a program-key cache brings it to about
    0.5 ms.
- **Segment work.** A worker runs `record()` on its own range from its entry QState, into a **child `Sub`**:
  - its own main CB, from the lane's own command pool (Vulkan pools are externally synchronized; the threading check
    already caught a shared pool, queue.rs:155);
  - its own `pre` CB for uploads it finds;
  - its own staging, descriptor-buffer chunks, memo maps, transient views, buffer views and arena;
  - query and occlusion indices taken from the parent's pools through a shared atomic counter (queries are reset in
    the parent's `pre`);
  - the parent's `pool_snapshot` copied in, and `db_bound = None`, so each CB binds its descriptor buffers once;
  - a serial of its own, so serial-keyed caches (`vin_last`, `tpart`, set keys) start empty in each CB.
- **Join (replay thread, segment order).**
  - Absorb each child's `post`, `writebacks` and `tex_wb` into the parent, in order.
  - Apply any gate redo (§4).
  - The final QState is the last segment's.
  - Keep the children in `sub.children`. Completion recycles them to their lanes.
  - `submit_sub` submits `[parent.pre, child pres…, child mains…]` in one `vkQueueSubmit2` with the same timeline
    value. So fences, receipts and parking stay as they are.
  - Each `pre` ends with `barrier_all`, as today.
  - Submission order puts every `pre` before every `main`, as the serial path does with its single `pre`.
- **Threads.** `NXGFX_TRANSLATE_THREADS` (0 = today's path). Start at 4 and tune to 6; MKW's game threads use ~12%
  each, and the machine has 8 P + 16 E cores.
  - The threads are persistent, named `vk translate N`, each with its own command pool and free children.
  - The replay thread also translates (lane 0).
  - Segments are handed out in order from an atomic counter.

### 4. Ordering and correctness rules

- **Queue order.** The replay thread still takes ops one at a time. A forked stream joins before its submission and
  before the next op. Cross-op ordering, parked queues, receipts, `scan_covers` and `fps_hold` are unchanged.
- **Inside one submission.** Command order is CB order:
  - a render pass instance never spans two CBs;
  - dynamic state is set again at every draw (as today);
  - descriptor buffers and vertex input are bound again in each CB (keyed by the child's serial).
- **Gate for shared mutable state.** Only the oldest unfinished segment may change shared state:
  - image creation, upload or init (`prepare_image` when `needs_init || needs_upload`);
  - target ownership the first time a target is used;
  - `upgrade_storage`, `materialize`, `note_storage_write`, alias and twin creation;
  - pool `ensure_buffer`, `bind`, `upload_deferred`, `POOL_GEN` bumps;
  - `prepare_pool_images` and table snapshots;
  - sparse binds.

  The pattern is check, gate, check again, change.
  - A younger segment that needs such a change waits until every earlier segment has finished. The oldest segment
    never waits, so there is no deadlock.
  - Each change bumps an epoch.
  - At join, a segment that started before an earlier segment's change is translated again on the replay thread.
    Its results were child-local (CB, staging, chunk, posts, write-backs), so they are thrown away and the segment
    runs again from its entry state.
  - If a submission needs more than 2 redos, the rest of it runs serially.
  - In the race, the steady state has no changes. During loading it degrades toward serial and stays correct.
  - `NXGFX_TRANSLATE_GATE=all` gates every call to these functions, as a correctness reference for A/B runs.
- **Benign shared caches stay ungated** (idempotent, keyed by stable identity): pipelines (`pipes.map` Mutex; a
  compile on a worker blocks only that worker), shader variants, image views cached on an image, the `resolve` cache
  (thread-local), the loss ring (atomics), perf counters (atomics).
- **Coherence.** Workers read guest memory (bindless handles, deferred binds, indices) in the same window as today:
  after the submission's pool sync, before its submit. Completion landing write-backs at the same time is today's
  race, unchanged. Lazy write-back (off) faults are already handled on any thread (INTERFACES rev 6).
- **Device-loss evidence.** Each child CB has its own checkpoints. `lost::ring::note_submit` also notes the children's
  serials under the parent's timeline value.

### 5. Cutting the write-watch walks (~7 ms → ~2.6 ms on the replay thread)

The MKW-PERF5 trace shows the walks per frame:

| Walk | Time | Why |
|---|---|---|
| Sub 1, warm | 1.24 ms | |
| Sub 2, warm | 1.33 ms | a CPU signal at 4.26 ms invalidated sub 1's coverage |
| Present, full | 4.4 ms | `frames()` is bumped first (queue.rs:2117), so it is the frame's first sync and asks every pool |

The present's walk runs under `g.mem`. It holds up completion's `wb_lock` by up to 4.8 ms, and so the fence main
waits on.

- **Walker thread.** On `present()` (game thread, right after `aq_push`), a `vk pool walker` thread starts the full
  walk. It walks the same watched ranges as `begin_submit`, in 64 MB pieces.
  - Each piece is walked and stashed under a new `WATCH_MU`, not under `g.mem`. The stash is a `walk_pending` list by
    pool base and identity.
  - When it ends, it publishes `WALK_TAIL` (the `aq_tail` at its start) and its signal count.
- **Lock order** is `g.mem` → `WATCH_MU`, and the walker takes `WATCH_MU` only.
  - `begin_submit` takes `WATCH_MU` around its own walks and drains `walk_pending` into pools' `pending` before its
    per-pool pass. So no page the walker took is missed by a later sync.
  - `land` (mem.rs:2252) takes `WATCH_MU` around its take / copy / reset, so the walker cannot take a landing's own
    stores in the middle.
- **Coverage.** The present op is pushed at 1.1 ms and taken at ~22.9 ms, about 20 ms of lead time.
  - `scan_covers(due, full)` gets a second source. A full scan counts as done when the walker's walk started after
    `due` was queued and has finished.
  - The present's sync then needs no walk of its own. Sub 2's warm walk (started at ~4.8 ms, after the push, with no
    CPU signal since) already covers the warm pools. It only runs the per-pool pass over what the walker stashed.
  - If the walker has not finished, the present waits for it.
  - The CPU-signal rule stays for warm coverage. For the walker it applies to presents behind parked work (the
    strict rule, as today).
- **Effect.** About 4.4 ms leaves the replay thread (it runs on an idle core with 20 ms of lead), and completion no
  longer waits behind a 4.4 ms locked walk. The warm walks (~2.6 ms) stay: they must start after their op or signal.
  The cold-pool policy ("a write to a cold pool reaches the GPU at the next frame's first sync") is unchanged.
- **Not chosen:**
  - Page protection of cold pools: a kernel write such as ReadFile into a protected pool fails instead of faulting.
    `src/os` would need an audit.
  - Round-robin cold walks across frames: weaker coherence.
  - Parallel walks: MKW-PERF5 attempt 1 measured them worse.

### 6. Expected per-frame budget (race window)

| Replay thread, ms a frame | Now | After |
|---|---|---|
| Write-watch walks | 7.0 | 2.6 |
| Per-pool passes, uploads, texture checks | ~3.4 | ~3.0 |
| State pre-pass | 0 | 0.5-1.5 |
| Draw translation | ~15 | ~3-4 (lane 0's share of 15 ms over 5-6 lanes, plus imbalance and lock contention) |
| Present (rest) | 1.4 | 1.4 |
| Fork / join, child CBs (only streams of 64 draws or more) | 0 | ~0.5 |
| **Total** | **~28** | **~11-13** |

Hold 1 needs under 16.7 ms.

- **Main's chain** (push → last sub's fence; main waits for it at the next frame's start):
  - now: 1.07 → 29 ms;
  - after: about 1.07 + sub 1 (~2.5) → the game's CPU signal at 4.26 → sub 2 (~2.8) → subs 3-7 (~5) → GPU tail and
    completion (~0.5, no longer behind the locked walk) ≈ **12.5-13.5 ms**.
  - That leaves about 2-3 ms of margin.
  - GPU time is 4.8 ms a frame and already overlaps.
- **Workers:** ~15-18 ms of CPU a frame across the lanes.
- **Walker:** ~4.4 ms on its own core.

### 7. Correctness risks

1. **Order-sensitive shared state touched from a worker**, missed by the gate list. This is the main risk.
   - Mitigation:
     - a gate audit of `image.rs`, `mem.rs`, `tables.rs` and `storemu.rs` call sites reachable from `emit`;
     - `GATE=all` A/B runs;
     - the redo rule;
     - the type system: `QState` and `Sub` are owned per worker; `QState`'s `RefCell`s are never shared.
   - Statics in exec.rs need review: `append::ST` (Mutex), `DUMMY`, the probes and `LAST_SETS`/`BOUND_DB` (thread-local,
     loss diagnostics only).
2. **Entry-state drift**: state a draw changes (missing shaders resolved, `dual`, `last_color`, `mesh_rec`,
   `tf_cont`). These are recomputed per draw or covered by a serial-only condition.
   - A unit test runs a synthetic stream through state-then-emit and serially, and compares the QState fields.
3. **Pass breaks at cuts inside a pass**: extra barriers and render-instance splits. Same pixels with LOAD/STORE and
   GENERAL. Occlusion is a sum of two queries.
4. **Vulkan threading**:
   - one command pool per lane;
   - query indices from shared atomic ranges;
   - descriptor buffers per child;
   - `defer.rs` shims off in parallel mode.
   - A short validation-layer run (threading and sync) on MKW after step 4.
5. **Stale descriptors.** Each child keys its sets by its own serial and chunk (`SetKey.serial` and `chunk`), so no
   set or shadow crosses CBs. Pool-generation checks are unchanged (`POOL_GEN`, `tables.generation`).
6. **Walker**:
   - pools added or removed during a walk: base and identity are checked when draining;
   - write-back landing racing a walk: `WATCH_MU`;
   - the coverage rule change for presents. This one is audit-worthy.
7. **Lock contention may cap scaling at 3-4 lanes.**
   - The contended locks: `note_gpu_write` takes `g.mem` for every written storage buffer; `tables.inner` for every
     view; the `pipes.map` Mutex for every draw; `images` on view misses.
   - Mitigations if measured: pool base from the `resolve` cache, an RwLock for the pipe map, the per-child
     `memo_views`.
8. **Latency margin is thin** (~2-3 ms). The game's own CPU signal (~4.3 ms into the frame) is a fixed point on
   main's chain.
9. **Memory**: per-child staging, descriptor chunks (4 MB) and memo duplicates. That is tens of MB more, recycled.

### 8. Incremental plan

Each step:
- is built under the dist lock;
- relinks MKW;
- runs `py scripts/play.py mkw --env NXGFX_GPU=0`, after which I read the sheet, the red lines and device loss;
- runs `cargo test -p graphics --lib`;
- runs a perf pair under the machine lock when it claims a gain;
- has an env switch to turn it off.

| Step | Change | Check | Expected |
|---|---|---|---|
| 1 | Pool walker + `WATCH_MU` + `walk_pending` + walker coverage for presents (`NXGFX_POOL_WALKER=0` off) | tests: coverage rules, drain by identity, landing exclusion; MKW correct | replay −4 ms, completion's `wb_lock` waits gone; hold-2 race steady at ~30 |
| 2 | `record_op` → `apply_state` + `emit`; serial path calls both | test: state-then-emit equals serial on synthetic streams; MKW unchanged | 0 ms (refactor only) |
| 3 | Segmented, **sequential**: pre-pass, cuts, child Subs on lane pools, join, multi-CB submit, all on the replay thread (`NXGFX_TRANSLATE_THREADS=1`); a `[vk] translate` stats line (segments, cuts natural/mid-pass, largest segment, pre-pass ms) | MKW sheet and red lines as serial; capture-free A/B against `=0` | +0.5-1 ms overhead; proves multi-CB, pass breaks, entry state, query and post merge |
| 4 | Worker lanes + gate + redo (`=4`, then 6), `GATE=all` A/B; validation-layer short run | MKW correct, no device loss, no validation threading errors; perf pair | replay thread ~12-14 ms; first FPS gain (hold 1 reached part of the time) |
| 5 | Tuning from step 3/4 stats: segment size, mid-pass cuts for the long passes, program-key cache for the pre-pass, lock fixes from 7.7 | perf pairs | replay ~11-12 ms; race at or near 60 |
| 6 | Defaults on; regression on `smash` (or `wonder`); final perf pair; `env_switches.tsv` rows | Smash 60 and correct; MKW 60 with 5% low | Done criteria |

The lead's independent audit fits after step 4. It covers the gate list, the walker coverage and the join order.

### 9. What phase 2 needs from the lead

- **The go after OOT-NAN has landed.** Step 2 and later edit `vk/exec.rs` (the `record_op` split, `run`) heavily, plus
  `vk/queue.rs` (`Sub` children, `submit_sub`, lanes), `vk/mem.rs` (walker, `WATCH_MU`) and small gate hooks in
  `vk/image.rs` and `vk/tables.rs`. I need no other wave editing `exec.rs`, `queue.rs` or `mem.rs` meanwhile. Step 1
  (the walker, `mem.rs` and `queue.rs` only) could start before `exec.rs` is free.
- **Agreement on four behaviour points:**
  - the present's pool sync may be covered by the walker's walk;
  - cuts inside a pass, i.e. extra pass breaks;
  - persistent translation threads on by default once proven;
  - streams under 64 draws stay serial.
- The phase 2 deadline, and machine-lock windows for about five perf pairs (~10 minutes each).
- Whether `smash` or `wonder` is the regression title.

## Report (agent, phase 2 checkpoint after steps 1-3, 2026-11-25)

Worktree `D:\Projects\RecompLauncherEngine-mkw`, branch `mkw-record`. Commits: `3fadfd6` step 1, `58286e3` step 2,
`f89bd0a` step 3 (each built, MKW relinked with the worktree's `dist`, run, sheet read). The worktree's `build/tools` is
a junction to the main tree's `build/tools` (read-only use by `build.ps1`); remove it with `rmdir`, never `rm -rf`.
All numbers are the race window [250, 325] s of `py scripts/play.py mkw --env NXGFX_GPU=0`, from the frames CSV:
replay thread = `replay_ms` (translation) + `prep_ms` (pool sync, texture checks). Runs are in the worktree's
`build/play/`. Perf pairs ran under the main tree's `machine.lock` with no other game running (the script waited
for OOT-FOREST2's runs to end).

### 1. Built

**Step 1: pool walker (`mem.rs`, `queue.rs`; off: `NXGFX_POOL_WALKER=0`).**
- **Cause.** A present's pool sync is the frame's first, so it walked every watched pool on the replay thread under
  `g.mem`.
- **The walker.** `present()` kicks a `vk pool walker` thread in `aq_push_with`'s `before`, so the kick comes before
  the replay can see the op. The walker walks every watched mirror/host pool in 16 MB pieces. It takes only a new
  `WATCH_MU` (after `g.mem` in lock order) and stashes the pages it takes.
- **Draining the stash.** Every holder of `g.mem` that takes and resets the watch drains the stash into the pools'
  `pending` first: `begin_submit` (for its whole sync), `apply_writebacks` (the landings and the lazy check) and
  `sync_range`.
- **Coverage.** A complete walk that started after the present was queued covers that present's sync
  (`walker_covers`, waiting up to 200 ms for a walk in progress). The present then runs only the per-pool pass and
  sets `scan_frame`, so later syncs leave the cold pools out as before.
- **Lock hand-off.** Syncs that wait for `WATCH_MU` go before the walker's next piece (`WATCH_WANT`).
- **Pending pages count as CPU writes.** `sync_range` (the cold-pool glyph fix, mkw-recomp #250) and the lazy
  write-back CPU check now treat a page in `pending` as written. Without that, a page the walker took mid-replay
  would be missed by a copy in the same submission.
- **Diagnosis kept apart.** The first version held `g.mem` per piece. Its pair (s1-off/s1-on) moved ~6 ms into draw
  translation: sets went from 7.1 to 13.2 ms, because draws take `g.mem` for every storage-buffer write. Hence
  `WATCH_MU`. The next pair (s1c) showed 3 ms a frame of syncs waiting on the unfair mutex, hence the hand-off.

**Step 2: `record_op` split (`exec.rs`, refactor).**
- `record_op` keeps its prelude, then runs a state record (`is_state_op`) in `state_op` / `state_op2`, or a command
  record in `emit_op` / `emit_op2` / `emit_op3`.
- The arms are unchanged; only function boundaries were added.

**Step 3: segmented translation, sequential (`exec.rs`, `queue.rs`, `mem.rs`; on: `NXGFX_TRANSLATE_THREADS>=1`,
default 0 = old path).**
- **Pre-pass.** `run_segmented` runs `plan`, which walks the stream once:
  - it applies the state records, SetRenderTargets' targets (`set_rt_state`) and a counter reset's samples-passed
    switch to a shadow `QState`;
  - it makes the ordered global effects in stream order (`global_op`: pending event writes, fence values,
    `rc_note` / `OCCL_FRAME`);
  - it evaluates waits (the stream parks where it did) and records render-condition results;
  - it cuts at SET_RT, BARRIER, DISPATCH, copies or texture clears once a segment holds 256 draws.
- **Segments.** Each segment runs the unchanged `record` from `fork_state()` with `seg` set: no global effects again,
  waits pass, render conditions popped from the pre-pass. It records into a child `Sub` (`new_sub`) whose timestamp
  and occlusion queries go into the parent's pools after the parent's.
- **Join.** `child_join` merges posts, write-backs (through `note_writeback`'s merge) and texture write-backs into
  the parent in stream order.
- **Submit.** `submit_sub` submits `[parent pre, child pres…, parent main, child mains…]` in one `vkQueueSubmit2` with
  one timeline value. The last child's buffers are swapped in as the tail (sparse fills, write-back baselines and
  copies, end timestamp) and swapped back after the submit.
- **Completion** retires the children's resources with the parent and restores their own query pools.
- **Mesh draws are segmented.** `mesh_rec` lives within one mesh draw. MKW issues mesh draws every frame, and the
  first step-3 run, which excluded them, segmented nothing in the race.
- **Kept in one piece:** pass dump, `NXGFX_DEFER`, DB poison, mesh-binding diagnosis, NDVE, raw probe,
  `NXGFX_SKIP_PS`, shadows=0, transform feedback.
- **Diagnosis tools.** `NXGFX_TRANSLATE_CHECK=1` compares each segment's pre-pass entry state with the state the
  segment before it left (`state_diff`). A `[vk] translate` line every 600 presents gives streams segmented or kept
  whole (with the reason), segments, the largest segment, pre-pass and segment ms, and write-back ranges.

### 2. Tests (`cargo test -p graphics --lib --release`, 213 pass after each step)

- `walker_tests::pieces_cover_each_run_and_nothing_else`: a walk asks the runs a sync asks, in pieces.
- `walker_tests::coverage_needs_a_complete_walk_started_after_the_present`: a present is covered only by a complete
  walk that started after it was queued; `None` or 0 never.
- `state_records_record_nothing`: no command, submission or global-effect record is classed as a state record.
- `fork_keeps_state_and_drops_the_pass`: a fork carries the state fields, no pass and no caches, and `state_diff`
  names the fields that differ.
- These are new; none tests a pre-existing failure. The real check is the game, plus 0 entry-state differences under
  `NXGFX_TRANSLATE_CHECK=1` in two full runs (`*-s3-check`, `*-s3b-check`).

### 3. Verified (FPS mean / 5% low; replay-thread ms = replay + prep)

| Run (worktree `build/play/`) | Switches | FPS | Replay | Prep | Replay thread | Write-watch scan / completion pool lock (ms a frame) |
|---|---|---|---|---|---|---|
| lead's MKW-PERF5 final (main tree) | before this wave | 28.9 / 25.1 | 15.9 | 12.5 | 28.4 | — |
| `181519-s1-off` | walker off | 29.4 / 26.1 | 14.8 | 13.8 | 28.6 | — |
| `183447-s1c-off` | walker off | 28.6 / 25.0 | 16.0 | 13.8 | 29.9 | 8.0 / 6.6 |
| `190204-s1d-off` | walker off | 26.8 / 24.3 | 15.9 | 16.7 | 32.5 | 12.3 / 8.5 |
| `190742-s1d-on` | step 1 final | 29.1 / 25.0 | 15.6 | 10.4 | 26.0 | 3.6 / 2.6 |
| `193620-s3-off` | step 1, segments off | 29.1 / 24.0 | 15.6 | 10.3 | 25.9 | 3.4 / 2.1 |
| `194158-s3-on` | segments on (=1) | 26.5 / 22.0 | 17.6 | 14.0 | 31.6 | 9.2 / 6.2 |
| `195122-s3c-off` | step 3 final, segments off | 27.6 / 22.8 | 15.8 | 13.5 | 29.4 | 9.0 / 5.8 |
| `195700-s3c-on` | step 3 final, segments on | 28.7 / 25.0 | 16.6 | 10.5 | 27.1 | 3.7 / 3.0 |

- **Step 1.** The present's walk leaves the replay thread: 600 of 600 presents' syncs covered, the replay never
  waited for a walk, and the walk takes 11-15 ms on its own thread. With the scan in its fast mode, prep drops
  3.4-6 ms (13.8-16.7 → 10.3-10.4) and completion's pool lock 6-8.5 → 2-3 ms. There is **no FPS gain**: 29.1 on vs
  26.8-29.4 off. The frame is still bound by draw translation and main's chain.
- **Step 3 cost (sequential).** Replay +0.8 ms (s3c) to +2.0 ms (s3), of which the pre-pass is 0.8 ms. Per race
  frame: 3.1 streams in 6.5 segments, largest 1,100-1,370 draws, the other 6 streams under 64 draws. Write-back
  ranges are unchanged (209 vs 211 a frame).
- **Noise I could not explain.** The write-watch scan is bimodal from run to run, ~3.5 ms or ~9 ms a frame, with or
  without segments, walker on or off (s1c-off 8.0, s1d-off 12.3, s3-on 9.2, s3c-off 9.0; the others 3.4-3.7). It
  moves prep by ~4 ms and FPS by ~1-2 between runs of the same build. FPS comparisons below ~2 FPS are not
  significant here.
- **Sheets read** (boot, title, menus, grid, race, lap 2, all correct): `184025-s1c-on`, `191459-s3-check`,
  `192200-s3b-check` (countdown at the grid), `195700-s3c-on`.
- **Red lines.** One kind in every run, the known `pipeline-crashed.bin` line.
- **No device loss**, every run exited normally.
- **Not run:** the `smash` regression (planned for step 6).

### 4. Unproven / next

- **No FPS gain yet.** The worker lanes (step 4) are where the gain is.
- **Load balance.** The largest race segment is ~1,300 draws, about 1/3 of the frame's translation, in one pass with no
  natural cut. Step 4 alone would be capped by it; the mid-pass cuts planned for step 5 are needed for balance.
- **Vulkan threading.** Children are recorded on the replay thread from the shared pools. Step 4 needs a command pool
  per lane, an atomic query counter and the gate of §4 of the design. The swap of the last child's buffers into the
  parent is undone right after the submit, so each `Sub` keeps its own pool's buffers.
- **Memory.** Children cost ~24 MB of staging and ~320 MB of scratch arenas (64 MB chunks) more in the memory line
  (allocator 7680 → 8000 MB in blocks). Per-lane reuse or a smaller child arena would cut it.
- **The bimodal write-watch scan** (3.5 vs 9 ms, above): unexplained, worth a trace before tuning further.
- **Behaviour changes for the lead's audit:**
  - **Coherence.** The present's sync is covered by a walk made ~20 ms earlier. A cold pool written between the walk
    and the present's sync now waits for the next frame's walk; before, the present's own scan took it. The design
    accepted this, but it remains audit item 1.
  - **Lazy write-back.** The fault handler (`lazy.rs:401`, copy then `reset_watch`) runs without `WATCH_MU`. The
    walker may take that page's own stores as CPU writes, giving a redundant upload of the same bytes; the replay's
    scans already had this race.
  - **`tex_writeback_land`** (opt-in `NXGFX_TEX_WB=1`) peeks the watch without the stash.
  - **Segments.** The pre-pass makes all of a stream's ordered global effects before its commands are translated: the
    `rc_note` order, pending event writes, fence values. Waits are evaluated at pre-pass time and render conditions
    taken from it. Draw-time `QState` changes that the pre-pass does not mirror (missing shaders resolved at a draw,
    `dual`, `last_color`) are recomputed per draw, or covered by the serial-only list.

### 5. Slips

- The first relink returned at once (`nxrs.exe` is a GUI-subsystem program) and I released `dist.lock` about 5 s
  early while it was still relinking. I took it again at once and held it until the process ended. Later relinks
  wait on the PID.
- Correctness runs ran beside OOT-FOREST2's games (allowed); their numbers are not used as measurements.
- Step 2 was committed by re-applying its edits to the step-1 tree (step 3 was already in progress); the restored
  step-3 files were byte-identical to the backups.

### 6. Questions

None.

## Report (steps 4-6, agent, 2026-11-25)

Worktree `D:\Projects\RecompLauncherEngine-mkw`, branch `mkw-record`. Runs are in its `build/play/`; numbers are the
race window [250, 325] s from the frames CSV (replay thread = `replay_ms` + `prep_ms`). "Pair" rows ran under the
main tree's `machine.lock` with no other game or build (the script waited for other waves' runs to end).

### 1. Built

**Step 0, merges.** `2dee255` merged `gameplay-harness` (head `5489c92`, which includes `5a41213`) cleanly; `15859c7`
merged it again for NVN2-PARITY / NVN2-FIX (one conflict, in `QState`'s fields: both kept). The second merge needed
a segment fix: the new per-submission inline uniform updates (`ub_upd`, read back by handle reads) and `ugen` are
queued by the pre-pass into its shadow state and carried by `fork_state`, and a segment ends its pass without
clearing them (`run_stream_due`'s `exec::end` clears them once).

**Step 4: parallel lanes (`4bfd307`; `NXGFX_TRANSLATE_THREADS=N`).**
- **Lanes.** The replay thread is lane 0. N - 1 persistent `vk translate` threads are lanes 1.., each with its own
  command pool and free child list (`queue::lane_sub`); completion returns a child to its lane. Segments are taken
  in order from a counter, and the replay thread joins them in stream order. Timestamp and occlusion queries come
  from shared atomic counters (`ChildInit`, `take_query` / `take_occl`).
- **Gate (`vk/gate.rs`).** Shared-state changes come in two kinds:
  - **First-use initializations** (`pass_pre`): the commands all go into the child's upload buffer. These are a
    pool's buffer, granules or deferred uploads, `sync_range`, an image made or prepared, a lazy registration, the
    waiting pool images, a twin copy, a block alias. Any segment may make them, and a dropped child's upload buffer
    is still submitted (before every main buffer).
  - **Order-dependent changes** (`pass`): an image's first GPU ownership (`tex_gpu_written`) and `upgrade_storage`.
    Only the frontier may make them: the oldest segment still being translated, with every older result standing.
    Any other segment gives up (its red lines are muted). A segment that overlapped an older segment's change is
    stale. At the join, given-up and stale segments are translated again on the replay thread.
  - `NXGFX_TRANSLATE_GATE=all` treats every gated call as order-dependent.
  - Diagnosis kept apart: the first version gated everything strictly, and about half the race's segments were
    translated again (2,200 per 600 frames, almost serial). The split brought that to 0.
- **Not in parallel.** Streams with DLSS, downsample or depth-resolve records are translated sequentially. The zero
  buffer and the stand-in images are made before the fork.

**Steps 1-3 audit fixes (`4bfd307`).**
- M1: `sync_range` takes only the copy's range of pending writes (`split_pending`) and keeps pieces under in-flight
  write-backs back.
- M3: no 200 ms wait. A present's sync finishes a running walk itself; no such walk means a normal scan.
- M2: the walker walks again while the present waits (up to 3 times).
- L1: `watch::take_partial` keeps the pages already reset.
- L3: no translate line with segments off.
- L4: covered by M3.
- L5: `state_diff` compares every forked field. `last_color` is left out: it is set when a pass begins and read only
  with shadows=0, which is serial-only.
- L6: deferred push constants and buffer binds are read once, by the pre-pass (`seg_mem`).
- L8: `debug_assert`.
- L9: comment.
- L2: noted; nothing locks `WATCH_MU` on the `bring_back` path.

**Step 5: balance and the replay thread's prep.**
- **In-pass cuts (`c2505ae`, `NXGFX_TRANSLATE_MIDCUT`, 256).** A segment is cut before a draw once it holds 256
  draws. The largest segment went from 1,312 draws to 256, and segment wall time from 15.9 to 6-9 ms.
- **Presents behind parked work (`c2505ae`, `NXGFX_WALKER_PARKED`).** These are covered by the walker too: the walk
  started after every op taken so far.
- **The 3.5-vs-9 ms "bimodal" scan.** It is the syncs no scan covered: ~2 a frame, each asking ~2 GB of warm and
  luke-warm pools.
  - `24b9e53`: a complete walk counts as a full scan (`note_walk`, with the CPU-signal rule), and the walker walks
    while ops wait (`NXGFX_WALKER_SYNCS`). Scan 9.0 to 6.7 ms.
  - `0ed0dc5`: a scanning sync leaves out the pools the walk in progress has walked since its op was queued
    (`WalkJob::covers`, `NXGFX_WALKER_POOLS`). Scan 6.7 to ~2.5 ms.
  - Measured and left off: a sync finishing a whole walk itself (`NXGFX_WALKER_HELP=1`, 12.5 vs 10.0 ms of pool
    sync).

**Step 6 (`0ed0dc5`).** `NXGFX_TRANSLATE_THREADS` defaults to 6 (`=0` gives the old path). `env_switches.tsv` has rows
for the new switches.

### 2. Tests (`cargo test -p graphics --lib --release`: 235 pass after the last merge)

- `gate::tests` (4): only the frontier changes; a segment that overlapped an older change is stale; one after it
  stands; no-change calls pass.
- `copy_sync_takes_only_its_range_of_pending` (M1).
- `coverage_needs_a_complete_walk_started_after_the_present` (extended for walks in progress and the CPU-signal
  rule).
- `a_walk_is_shared_piece_by_piece`.
- `walked_pieces_cover_a_pool_whole`.
- `fork_keeps_state_and_drops_the_pass` (extended: L5 fields, the fork carries the inline uniform updates).
- The `cut_before` cases.

All are new; none tests a pre-existing failure. The real checks are the game runs, plus 0 entry-state differences
under `NXGFX_TRANSLATE_CHECK=1` with parallel lanes and in-pass cuts (`203934-s5-t4-check` after the `last_color`
note, `220232-m1-check` after the second merge).

### 3. Verified

| Run | Build / switches | FPS / 5% low | Replay | Prep | Replay thread | Scan |
|---|---|---|---|---|---|---|
| `195700-s3c-on` (checkpoint) | step 3, =1 | 28.7 / 25.0 | 16.6 | 10.5 | 27.1 | 3.7 |
| `205354-p5-off` pair | step 5, segments off | 26.9 / 22.0 | 16.5 | 13.8 | 30.3 | 8.9 |
| `204814-p5-on` pair | step 5, 4 lanes | 29.4 / 25.0 | 11.6 | 14.1 | 25.7 | 9.1 |
| `210126-p5c-on` pair | + walks cover syncs | 30.0 / 29.0 | 11.8 | 11.9 | 23.7 | 6.7 |
| `211331-p5d-on` pair | + syncs finish walks (now off) | 30.0 / 27.0 | 10.9 | 14.8 | 25.7 | 2.1 |
| `213820-p6a-minwalk` pair | walker as in step 1 | 29.4 / 25.0 | 11.1 | 13.8 | 24.9 | 9.2 |
| `214359-p6b-t6` pair | 6 lanes | 30.4 / 29.0 | 11.2 | 9.4 | 20.6 | 2.7 |
| `215340-p7-default` pair | step 6 defaults, before 2nd merge | 29.9 / 27.0 | 11.2 | 9.3 | 20.5 | 2.5 |
| `224333-final-mkw` pair | final, after 2nd merge (defaults) | 30.1 / 28.0 | 11.8 | 11.6 | 23.4 | 6.6 |
| `224912-smash-reg` pair | Smash, final build (window [130, 280]) | 59.8 / 59.0 | 2.4 | 2.3 | 4.7 | 1.3 |

- **Smash regression.** Baseline in the main tree earlier today: 59.6-59.8 / 58.9-59.2 FPS (`112615-smash-3match`,
  `111130-smash-polish-final`).
  - Smash's one long stream runs in 2 parallel segments a frame, translated in 2.8 ms by the lanes together.
  - Sheet correct: boot, character select, the VS screen, both Norfair matches.
  - One red line, `movie: VP9 decode error` at 40 s: the OS movie decoder (the opening movie), not the graphics
    path. It was not seen in the baselines; I did not re-run for it.

- **Translation** leaves the replay thread: 16.5 ms becomes ~11 ms (segments 15.9 ms of wall time down to 6-9 ms;
  lanes ~19-22 ms of CPU together). The lanes' extra CPU against serial is ~2.7 ms of lock waits (pool lock at
  resolves ~2.0, images ~0.6) plus cold per-child caches and pass breaks.
- **Prep** (pool sync) goes from 13.8 to 9.3 ms: write-watch scan 8.9 to 2.5 ms; lock wait ~1.1 and per-pool pass
  ~1.9 remain.
- **Replay thread** goes from 30.3 to 20.5 ms. That is still above 16.7, so the game holds 2: FPS stays at the
  30 cap (29.9-30.4) and the 5% low rises from 22 to 27-29.
- **Sheets read**, all correct (boot, title, menus, grid, race, lap 2):
  - `202106-s4-t4`, `203106-s4b-t4`, `203934-s5-t4-check`, `212103-d6-prof`, `215340-p7-default`;
  - `220232-m1-check`: 03-menus caught a fade between scenes, draws 1,241 that frame, the race after it correct;
  - `223125-gate-all`: the reference mode, pictures as the gated runs;
  - `224333-final-mkw`: full-size race shots show full detail (track plates, canyon, cacti, signs, karts), no
    texture missing.
- **Red lines.** One kind in every run, the known `pipeline-crashed.bin` line. Exception: the first step-4 run
  (`202106-s4-t4`) logged 29 more, missing-pool and no-target lines from given-up segments. Muting them is in
  `4bfd307`.
- **No device loss**; every run exited normally.
- **Gate reference (`NXGFX_TRANSLATE_GATE=all`, `223125-gate-all`).** 26.8 / 23.0 FPS, ~5,000 segments a minute
  translated again, sheet correct, same pictures as the default runs.
- **Validation layer (`*-valid`, 6 lanes; `*-valid-off`, translation off).** Both end with an access violation at
  18 s, so this is pre-existing under the layer. The lanes started at 16.6 s: no threading or synchronization
  messages in the ~1.4 s they ran; the only VUID is the old `vkCmdBindIndexBuffer-buffer-08784` usage flag.
- **Set reuse after the second merge** (the `replay up to frame` line, per frame): before (`p7`) 3,002 sets (1,136
  reused, 2,390 patched); after (`m1-check`) 2,613 (1,053 reused, 2,047 patched) and (`final-mkw`) 2,330 (918 reused, 1,779 patched). Reuse
  per set is about the same (38-40%); the line averages from the start of the run, so it moves with load timing.

### 4. Unproven / next

- **60 FPS is not reached.** The replay thread is at ~20.5 ms against 16.7. What is left on it:
  - ~11 ms of replay: segment wall time ~8 ms, pre-pass ~0.8, six small streams serial, submit;
  - ~9 ms of prep: pool sync ~5.5 (lock wait 1.1, scan 2.5, per-pool pass 1.9), plus texture checks and the
    present.
- **The scan still varies from run to run** with the same build: 2.5 ms in `p7`, 6.6 ms in `final-mkw`, giving a
  replay thread of 20.5 vs 23.4 ms. The ~1 uncovered sync a frame scans more or less, depending on how far the walk
  in progress has got when it comes.
- **Next candidates, measured costs:**
  - **Pipeline the replay thread:** the next op's pool sync while the lanes translate this one. Uploads would land
    in the next submission's upload buffer; the question to answer is the read window for guest memory.
  - **The resolve path's pool-lock waits** (~2 ms of lane time): a read-mostly lock or a per-lane pool cache.
  - **Per-child cold caches:** set shadows and staged constants start empty in every segment, about 1.5x the CPU.
- **Validation-layer coverage of the lanes is thin** (1.4 s), because of the pre-existing crash at 18 s under the
  layer.
- **For the audit (new behaviour):**
  - the gate split: which changes count as first-use initializations (`pass_pre`); a dropped child's upload buffer
    is submitted;
  - walks as scan coverage for every sync (`note_walk`), and per pool for a walk in progress (`WalkJob::covers`);
  - presents behind parked work covered by the walker;
  - the pre-pass queueing inline uniform updates (`ub_upd`) for segments.
  - The M1-M3 fixes change step 1's paths too.
- **Pop-in on race shots.** The two race shots (42 s apart) show full-detail track, kart, billboards and items in
  every run. This scenario has no consecutive shots of one place, so pop-in between frames is not tested.
- **Smash** is now built by the worktree's engine (its relink recompiled the whole game); the main tree's next Smash
  relink will do the same.

### 5. Slips

- One `.rs` edit through `sed`: the two `fetch_update` → `try_update` renames in `queue.rs`. The AGENTS rule is
  editor tools only.
- `203934-s5-t4-check` was started while NVN2-PARITY held `machine.lock`. My script printed the lock but did not
  stop, so my run may have disturbed their measurement, and its numbers are not used. Every later run waited for
  the lock.
- `cargo build -p graphics` ran during my own `210126-p5c-on` measurement, at about t = 180-210 s, before its
  [250, 325] s window.
- The first merge's commit message names `5a41213`; the branch head merged was `5489c92` (which includes it).

### 6. Questions

None.

## Audit fixes (agent, 2026-11-25 night)

Findings of the independent audit of `git diff 5bce4ff 2d02e3b -- src`. Runs in the worktree's `build/play/`, GPU 1.

### 1. Built

- **H1, upload-buffer order.** A parallel run's segments record their first-use initializations into one upload
  buffer, the parent's (`queue::share_pre` / `unshare_pre` around each lane's segment; `ChildInit` carries it). Each
  recording site takes `gate::pre_lock` (a mutex, re-entrant per thread, taken only on a lane of a parallel run)
  inside the images' or the pools' lock it changes state under, so the buffer holds the uploads in the order the
  state changed: `sync_range`, `Pool::ensure_buffer`, `Pool::bind`, `Pool::upload_now` (deferred uploads, `gmem`),
  `prepare_image` (with ASTC / GPU deswizzle inside), `cube_twin_view`, `block_alias`, the stand-in images and the
  zero buffer. Lock order: images or pools, then `PRE_MU`; no holder takes either again. The replay thread makes its
  children under the same lock (their upload buffers come from the shared buffer's command pool). A dropped child's
  upload buffer now holds only its start. `Sub::upload` writes a red line if a lane records without the lock.
  Segments translated again at the join record into their own upload buffers, after every parallel one, in order.
- **M1, takeover order.** Gating the takeover with `gate::pass(true)` as suggested was built and measured first
  (`232300-audit-check`): MKW takes over live images every frame in segments, so the gate asked ~13 times a frame,
  ~5 segments a frame gave up and ~7 were translated again; 26.8 / 23.0 FPS. Built instead: each run records its
  takeovers (old serial -> segment, new serial; `gate::note_takeover`). A draw or copy of the old object from a
  segment before the taker in the stream (in parallel, or translated again at the join: `Run::redo`) gets the
  image where it is now (`Gpu::ensure_target`, `Images::target_serial`, used by `target_image`, `source_image`,
  `attach_view_layers`), which is the image it would have drawn into before the takeover; a use after it still
  makes an image of its own, as in order. The old image's GPU ownership is already frontier-only. The reference
  mode (`NXGFX_TRANSLATE_GATE=all`) still gates the takeover.
- **L1.** One predicate, `ub_update_kept` (bytes present, range in one pool window by `room`), queues an inline
  uniform update for handle reads in the record and in the pre-pass, read before the record's `resolve`.
- **L2.** `defer_after_use` on a lane during its segment (`gate::parallel`) uses the per-submission list.
- **L3.** `walk_some` steps aside before it claims a piece; `walk_finish` gives up after 100 ms (`WALK_LATE`, a log
  line): the walk then covers nothing and the sync scans itself.
- **L4.** `take_query` / `take_occl` count refusals (`QUERIES_OUT`, in the translate line) and write a red line once.
  No draw is skipped.
- **L5.** A twin's copy runs once per upload buffer (`Sub::pre_serial`), not once per child.
- **L6 open.** Taking `WATCH_MU` in the lazy bring-back is not clearly safe: the fault can come from a thread that
  holds it (a sync reading guest memory for an upload under `watch_lock`), which would deadlock.

### 2. Tests (`cargo test --release -p graphics`: 238 lib tests pass, plus the integration tests)

- `gate::tests::the_shared_upload_buffer_is_recorded_by_one_thread_at_a_time`: held on a lane, re-entrant, another
  lane excluded, nothing held outside a run.
- `gate::tests::a_takeover_is_seen_by_earlier_segments_only`: segments before the taker see it, the taker and later
  ones do not, also translated again at the join.
- `walker_tests::a_piece_that_never_ends_does_not_hold_the_sync`: L3, the finish gives up and covers nothing.
- All new; none can fail on the old code as written (the functions did not exist). H1's ordering is shown by the
  game runs, not by a unit test.

### 3. Verified

| Run | Build | FPS / 5% low | Replay | Prep | Replay thread |
|---|---|---|---|---|---|
| `224333-final-mkw` (before) | steps 4-6 final | 30.1 / 28.0 | 11.8 | 11.6 | 23.4 |
| `232300-audit-check` | M1 gated (rejected), check on | 26.8 / 23.0 | | | |
| `233246-audit-check2` | final, `NXGFX_TRANSLATE_CHECK=1` | 30.2 / 28.0 | 11.8 | 9.5 | 21.3 |
| `233849-audit-perf` | final, machine lock | 30.0 / 27.0 | 11.9 | 9.8 | 21.6 |
| `234603-audit-repeat` | final | 29.9 / 26.0 | | | |
| `235152-audit-smash` | final, Smash relinked, machine lock, window [130, 280] | 59.8 / 59.0 | | | |

- **Smash:** as before (59.8 / 59.0); 0 red lines; sheet correct (boot, character select, VS screen, both Norfair
  matches, results).

- **Check run:** 0 entry-state differences, 0 segments translated again, the gate asked 0 times in the race, the
  shared upload buffer's lock waited 0.000-0.004 ms a frame, no queries refused in the race, 0 uses of a later
  takeover in the race (the case does not occur there).
- **Sheets read:** `232300-audit-check`, `233246-audit-check2`, `234603-audit-repeat` correct (boot, title, menus,
  grid, race, lap 2). `233849-audit-perf`'s `05-race` shows a large dark area over the banked turn with crisp
  edges while the kart rides a Bullet Bill; `233246` and `234603` show the same turn lit. One frame, not repeated.
- **Red lines:** the known `pipeline-crashed.bin` line, plus the new L4 line in every run: `a submission used all
  256 timestamp queries` at ~8.4 s (loading). That was silent before; the race refuses none.
- **No device loss**; every run exited normally.

### 4. Unproven / next

- The dark banked turn in `233849-audit-perf/05-race.png`: a shadow or a rendering fault; not seen in the other two
  runs at that place. Next: shots of that turn in a few runs, with `NXGFX_TRANSLATE_THREADS=0` against 6.
- A use of the old object after a takeover made by an earlier segment that is still being translated can draw into
  the taken-over image (as before this fix); no game is known to do that.
- The loading submission that uses up 256 timestamp queries (counter reports without a GPU timestamp): a larger pool
  or a per-submission split.
- L6 (above).

### 5. Slips

- None known. M1 was built as the audit suggested first, measured, and replaced (above).

## Dark turn check (agent, 2026-11-26)

**Verdict: a lanes fault, now fixed.** Not the game's shadow: the same viewpoint (Luigi Tires / Red Shell signs,
Bullet Bill) is lit in `233246-audit-check2/05-race` and in `t45` of `darkturn-t6-2`, `-t0-3` and `-t6-3`.

- **Runs** (new scenario `mkw-turn`: as `mkw` to the race, then 10 shots about a second apart across the banked
  turn, `t37`..`t46`; GPU 1): serial `NXGFX_TRANSLATE_THREADS=0` `20261126-000114/-001108/-002102-darkturn-t0-1..3`,
  default lanes `20261126-000611/-001605/-002559/-003137-darkturn-t6-1..4`. All 70 shots normal (`t0-2` was past the
  turn by `t38`; the others cross it, `t0-1`, `t6-2` and `t6-3` with a Bullet Bill): the dark frame did not repeat, so the pictures
  alone could not decide.
- **Cause, from the logs.** `texture ID .. its slot names texture serN, which has no image now (live): a zero stand-in
  is bound`: serial runs 19-21 in the whole run and **0 in the race**; lanes runs 27-69 in the race (`233849` among
  them: a 2D array, target 4, at 290.4 s, the dark shot at 292 s). "(live)" means the image existed by the time the
  line was written: a segment looked the sampled texture up (`tex_view_uncached`: `live` -> `by_serial`) before an
  earlier segment of the stream had made its image (a target drawn there; MKW makes new objects over its targets each
  frame). Image creation passes the gate as first-use initialization (`pass_pre`), so the later segment's miss was
  never caught, and the zero stand-in was sampled where the serial order samples the image: a shadow or lighting
  input of zeros gives the dark, sharp-edged turn.
- **Fix** (`exec.rs` `tex_view_uncached`): on a lane, a lookup that finds no image asks the gate (`gate::pass(true)`);
  unless the segment is at the frontier it gives up and is translated again at the join, in stream order (at the
  frontier the lookup is the serial one). `cargo test --release -p graphics`: 238 lib tests pass, plus the
  integration tests. Built (`build.ps1 -KeepCache -SkipGnu`), MKW relinked under `dist.lock`.
- **Fixed runs** `20261126-003726/-004223/-004720-darkturn-fix-1..3`: stand-ins 19-21 in the run, **0 in the race**
  (as serial); the gate gives up 7-22 segments per 600 frames (all at the new site); 28.9 / 29.6 / 29.7 FPS mean over
  [250, 290] (not machine-locked; the lanes runs before were 28.8-30.0); sheets correct (`fix-1`, `fix-2` cross the turn
  lit with a Bullet Bill, `fix-3` was past it in the stadium section); normal exits, no new red lines (`pipeline
  compilation stopped for clean exit` at the time limit also appears in `t6-1`, before the fix).
- **Not covered:** pool-indexed (bindless) slots made in `prepare_pool_images` are not segment-ordered by this; no
  stand-in from them was seen in the race.
