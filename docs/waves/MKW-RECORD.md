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
