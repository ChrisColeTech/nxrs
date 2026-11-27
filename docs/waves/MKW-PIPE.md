# MKW-PIPE — Mario Kart World's next frame overlapping the replay of the previous one

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~23:00, worktree
`D:\Projects\RecompLauncherEngine-mkwpipe`, branch `mkwpipe` (base `setfast` `70373e2`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0 (OOT-OCC ran OoT on GPU 1 during every run
here). Deadline 4 h.

## Starting point

SET-FAST: in every MKW run the frame = replay thread (replay + prep) + ~13 ms. Turbo runs MKW at 57-59 FPS with a
9.5-11.3 ms translation thread on this machine (REF-ARCH), so its game overlaps translation.

Read before measuring (code): MKW reports no samples-passed counters (no "samples-passed ... in use" line in any MKW
log), so OoT's `occl_sync` wait does not apply to it. The throttles left on the game's side are `aq_wait_room`
(63 ops or 2 presents queued: the same numbers as Turbo's `AQ_N` / `AQ_PRESENTS`), `nvnSyncWait` (publication wait
and the fence), the acquire's `present_wait` of the texture's previous present, and the frame step (`os/fstep.rs`).

## Plan (written before code)

1. Host sampler over a race stretch on `vk replay`, the render thread (`guest 0x1004`, the acquiring thread), the
   main thread (`guest 0x1001`) and `nvn-completion`; plus runs with the hold kept at 1 vsync (`NXOS_FPS_HOLD=off`)
   so the frame is not quantized. What the game's threads block on, how long, where, and whether the replay thread is
   busy or idle meanwhile.
2. Remove the serialization found, behind a switch (0 = old), answering what the game needs as late as NVN allows.
3. The frame step with frames overlapping: 60 reachable, or the numbers that show what holds it.
4. Verify: `mkw`, `mkw-turn` (routes, every sheet), same-build switch-off controls back to back; `cargo test`.

## 1. What blocks: the frame is not "replay + 13 ms"

**The "+13 ms" is the frame step's quantization.** The race frames of SET-FAST's `214047-sf-s2-on2-mkw` are 1 vsync
(636) or 2 vsyncs (1,763) long, nothing between; the 2-vsync hold is the pacer's. Its 1-vsync trials fail at a cost
of 1.13-1.17 vsyncs (18.8-19.5 ms, `frame step: hold 1 -> 2 ... (cost 1.13-1.17)`), past `LATE` (1.1). With the hold
kept at 1 vsync (`NXOS_FPS_HOLD=off`, same build) the race runs **42.5-45.7 FPS (5 % low 35-38), frames ~21 ms,
game time 1.000 of real time**, against 29-32 FPS at the default hold.

**What the game waits on (chain trace, `NXGFX_CHAIN_TRACE=280:0.25`, hold 1, 10-13 frames a run).** The game keeps
one frame in flight by its own sync, not by an engine throttle:

- the render thread (`guest 0x1004`) records nothing itself: it is 95.7 % blocked on a guest sync object (waiting for
  the main thread) and 3.9 % in `nvnQueueSubmitCommands`. Right after the frame's submit it fences sync
  `0x13a917020` (`nvnQueueFenceSync`, an async fence = the submit's op), then presents;
- the main thread (`guest 0x1001`) runs the next frame's update (12.2 ms median) overlapped with the replay, then
  waits in `nvnSyncWait` on that sync (its frame update, main+0x69d034; 30.5 % of its samples, 5-9 ms a frame) for the
  previous frame's GPU completion, and only then lets the render thread submit the next frame;
- the engine's own throttles are idle: `aq_wait_room` 0.00-0.2 ms a frame (ops), 0.00 (presents), fence publication
  0.00, acquire 0.00. The replay thread is busy 19-21 ms and idle 1.5-2.5 ms of a ~21 ms frame.

So the game already records frame N+1 while frame N replays; what it will not do is *submit* N+1 before N's fence.
Per frame (medians, `230852-mp-chain-hold1`): **period 21.0 ms = replay of the frame's submit op 18.1 + GPU tail and
landing 1.0 + main released to the next submit 1.5**. The game's own CPU (12 ms) is fully hidden. NVN's fence means
"the GPU finished that work, its effects landed"; answering it before the op is replayed is wrong (translation reads
guest memory the game would then overwrite), and answering it at "replayed" instead of "GPU done" would save only the
1.0 ms tail. The lever is the latency of that one op.

**Inside the op: two queues hand work to each other.** With park / resume notes added to the chain trace
(`232459-mp-chain2-hold1`): the frame's main queue stream parks at a GPU event wait (`0x2264c7140 >= k`) that a second
NVN queue (`0x13`, parked on `0x2264c7100`) signals, which waits in turn for the main queue's signals: A1, B1, A2, B2,
A3, B3, A4 (7 parts a frame, each a Vulkan submission). Every wait is passed by a GPU signal already *recorded* (the
`pending_signals` rule), so no part waits for the GPU; but every hand-over went through the parked-work retry and a
new submission with its own pool sync and image check, in series on the replay thread: **prep 7.2 ms a frame on that
path** (pool sync 4.3, textures 2.8) against translation 10.4. A2's sync was a whole write-watch scan (`scan 0x2b5`
pools) every frame: the game's other threads make CPU event signals on unrelated events during the replay
(`cpu_sig 0x2264c7000..c0`), which voids `scan_covers`, and A2's scan then uploads the next frame's CPU writes.

**How Turbo avoids it** (`D:\Projects\Switch2RECOMP-main`, `runtime/nvn/hle_nvn.c`): it does not overlap more. The
same throttles (`AQ_N = 64`, `AQ_PRESENTS = 2`, line 1302), the same fence rule (a queued fence resolves after
translation, then waits for the GPU: `aq_resolve` 1486, `nvnSyncWait` 735), the same acquire (texture's previous
present). Its parked submissions are fibers: at an unmet wait it commits what is recorded (`gfx_sub_commit`) and the
fiber resumes later with `gfx_sub_begin` (`nvn_aq_park`, 1339), with **no memory sync at a resume** (Turbo syncs
per use) and ~1 µs sets, so the whole frame op is translated in its 9.5-11.3 ms. MKW's frame there is that op plus
the same ~2.5 ms tail, i.e. under 16.7 ms.

## 2. Built: parked queues run on at once without a pool sync of their own (`NXGFX_PARK_CHAIN`, default on)

`vk/queue.rs` `run_stream_due` → `chain_parked`: when a stream parks, its rest parks first; then every parked queue
whose head wait a GPU signal recorded so far passes (`exec::wait_passes_chained`: the `pending_signals` WRITE rule,
or the event's value when no CPU signal was made since the last pool sync) runs on at once on the replay thread, in a
submission of its own (`chained_sub`: scale, gmem notes and the textures' own preparation, **no pool sync**); what was
recorded before is submitted first, so the GPU works on part k while part k+1 is translated. Receipts of streams that
end resolve with the value of the submission holding them. Parked entries with presents, later waits or receipt waits
are left to the retry as before. Why one sync is enough: every parked stream was queued before the op whose sync ran,
and its wait passes by GPU work recorded after that sync; a wait passed by a CPU-signalled value after that sync is
not chained (the CPU may have written what it guards). CPU writes made since stay in the write watch for the next
ordinary sync, as for a console GPU that had not reached them. `NXGFX_PARK_CHAIN=0`: the old path exactly.

Diagnosis support kept: `[vk] pipelining up to frame N` line (game threads waiting for queue room by ops / presents,
for fence publication; replay thread busy / idle; streams chained), chain-trace notes `park`, `resume`, `chain`,
`gpu_sig`.

Tried and rejected: all chained streams in **one** submission (`233903-mp-chain-on-hold1`): the op fell to 12.8 ms
but the GPU then started the frame's work only after its last translation: tail 10.8 ms, period 25.2, 37.6 FPS.

## 3. Measured effect

Hold kept at 1 vsync (the frame unquantized), same build, back to back; per-frame chain medians from each run's trace:

| Run (`build/play/`) | switch | FPS / 5 % low [250, 325] | period | frame op | tail | main waits (mean) | route |
|---|---|---|---|---|---|---|---|
| `20261126-235351-mp-off-hold1` | `NXGFX_PARK_CHAIN=0` | 45.7 / 38.0 | 20.2 | 17.1 | 1.1 | 6.1 | 3/3 on |
| `20261126-234759-mp-chain3-hold1` | on | **47.3 / 37.0** | **18.6** | **15.2** | 1.8 | 5.3 | 3/3 on |
| `20261126-232459-mp-chain2-hold1` (earlier build, off path) | - | 45.0 / 38.0 | 21.2 | 17.9 | 1.0 | 7.1 | 3/3 on |

The frame op is ~2 ms shorter (prep a frame 7.1 → 5.5 ms in the frames CSV, no retry hand-overs); the tail grows
~0.7 ms (the last parts reach the GPU later); the period gains ~1.6-2.6 ms. FPS at hold 1: +1.6 mean, 5 % low
unchanged: within run-to-run noise on a shared machine (OoT ran on GPU 1 throughout). Translation (`replay_ms`) rose
~2 ms a frame in the same runs (10.0 → 12.4 median): part of what the per-part syncs did moved into the translation
(first-use and cold-pool uploads at resolve) or is noise; the chain trace's op latency is the figure that matters
for the game's fence.

Default pacer (`fps_hold` as the profile has it), final build `a59c2aa` (relinked 00:00), back to back, window [250, 325]:

| Run (`build/play/`) | switch | FPS / 5 % low | frame mean / p50 | replay / prep p50 | frame step: held 1 vsync of 1,800 | route |
|---|---|---|---|---|---|---|
| `20261127-000150-mp-final-on` | on | **33.2 / 28.0** | 30.2 / 33.2 | 12.0 / 5.5 | 1,165 then 616 | 3/3 on |
| `20261127-000728-mp-final-off` (right after) | `NXGFX_PARK_CHAIN=0` | 30.6 / 29.0 | 32.7 / 33.3 | 10.7 / 7.8 | 104 then 119 | 3/3 on |
| `20261127-001814-mp-final-on2` (repeat) | on | 32.1 / 29.0 | 31.0 / 33.2 | 11.9 / 5.3 | 1,209 then 302 | 3/3 on |
| `20261127-001306-mp-final-turn` (`mkw-turn`, [250, 290]) | on | 41.0 / 27.7 | | | | 4/4 on |

The pacer holds 1 vsync far more often with the switch on (its 1-vsync trials pass more: frames near the 18.3 ms
`LATE` line), so the mean rises ~2 FPS; the median frame is still 2 vsyncs and the 5 % low is unchanged. **MKW is
still held near 30 at the default pacer.** For reference, SET-FAST's base was 32.4 / 28.7 and its final 28.8 / 13.9
on the same machine; run-to-run spread is ±2 FPS.

Pictures (every sheet read): boot, title, menus, grid, race and lap 2 drawn correctly in every run, both switch
states; `mkw-turn` t37-t46 on the banked desert slope (at ~41 FPS the kart has not reached the steel deck by t46),
lit and textured correctly, coins, items and HUD present; no pop-in, stale textures, flicker or missing geometry.
Anomalies: the known set only (zero stand-ins at loads and the known released-image stand-in pair, which the base
`210150-sf-base-mkw` also has in its race at 313 s; skipped draws of the crashed pipeline; gate give-ups;
held-back CPU writes, at the off runs' rate). **The title shot (`02-title`, 138.6-138.7 s) was black in
`final-on` and `final-on2`, a dim fade-in in `final-turn`, the full title in `chain3` and in every off run**: the shot
falls on the opening-to-title cut, which the game fades in from black, with a 300 ms title-scene load right after it
(139.1 s on, 139.6 s off); the race and menus are unaffected. Read as a timing difference of the cut, not proven:
see Unproven. A fifth switch-on run with fault captures (`20261127-002601-mp-final-faults`, 32.8 / 29.0, 3/3 on)
shows the title scene at the same shot time, and its 11 captures are all of the known kinds: the timestamp-query and
crashed-pipeline red lines, zero / released-image stand-ins, and four "white flash" triggers that are bright sky and
a sunlit roof in the opening and course flyover (read: correct pictures); no black frame was captured anywhere.

Sheets: hold 1 `build/play/20261126-234759-mp-chain3-hold1/sheet-1.png` (on), `20261126-235351-mp-off-hold1` (off);
default `20261127-000150-mp-final-on`, `20261127-000728-mp-final-off`, `20261127-001814-mp-final-on2`,
`20261127-002601-mp-final-faults` (each `sheet-1.png`); `mkw-turn` `20261127-001306-mp-final-turn/sheet-1.png`,
`sheet-2.png`.

## 4. The frame step

It allows 60: a run of 30 frames costing ≤ 0.95 vsync starts a 1-vsync trial; late (> 1.1 vsyncs) frames step back.
At ~19-21 ms frames it holds 2 vsyncs (30 FPS, even); `fps_hold off` gives 43-47 FPS uneven with correct game speed.
That choice is the owner's pending decision (ledger: "keep `fps_hold` (steady 30) or drop it (~35-36 uneven)"); the
uneven figure is now 43-47, not 35-36. Not changed here.

## 5. What is left to 60

The frame is the game's own chain: replay of the frame op (15.2 ms) + tail (1.8) + the render thread's submit after
the main thread's release (1.5). 60 at the default pacer needs the op under ~12.5 ms; holding 1 vsync without the
pacer needs it under ~14.5 ms. In that op now: translation ~10 ms (sets and draws: SET-FAST's ground), the first
part's pool sync ~2.5 ms (USE-SYNC's per-use sync removes it), the texture walk per part (two walks over every image
per submission, `image_begin_submit`; OOT-DESC's change 6 merges them, on its branch). Translating the main queue's
later parts ahead (A2-A4 on lanes while B parts translate) is the structural next step: plan through the waits,
defer those parts' global effects (signals, fence values) until their release in queue order. Not attempted here.

## Report (agent, protocol §5)

Branch `mkwpipe` (base `setfast` `70373e2`): `55d3735` (plan, counters, trace notes), `a59c2aa` (`NXGFX_PARK_CHAIN`,
switch row), `fa8401d` (test), this commit (doc).

### 1. Built
- **Diagnosis** (§1): the "+13 ms" is the frame step holding 2 vsyncs; unquantized the race frame is ~21 ms and equals
  the game's own chain: its main thread waits (`nvnSyncWait`, main+0x69d034) for the previous frame's fence before
  the render thread may submit the next frame, so the frame = replay of that frame's submit op (18.1 ms) + GPU tail
  (1.0) + the next submit (1.5). The engine's throttles (`aq_wait_room`, presents, publication, acquire) cost
  0.0-0.2 ms a frame; the game's 12 ms of CPU already overlaps the replay. Inside the op the two NVN queues hand work
  to each other 6-7 times a frame, each hand-over a new submission with its own pool sync (7.2 ms of prep a frame on
  the critical path). Turbo has the same throttles and fence rule; it is faster per op (no per-resume sync, ~1 µs
  sets), not more overlapped.
- **Correction** (§2): `NXGFX_PARK_CHAIN` (default on): parked queues released by GPU signals already recorded run on
  at once, a submission each, without a pool sync of their own. Op latency 17.1 → 15.2 ms, period 20.2 → 18.6 ms
  (hold 1, medians). The fence is still answered at GPU completion (NVN's meaning); nothing is answered early.
- Diagnosis tools kept: `[vk] pipelining` line (game waits by cause, replay thread busy / idle, chained streams),
  chain-trace notes `park` / `resume` / `chain` / `gpu_sig`.
- Rejected with numbers: all chained streams in one submission (tail 10.8 ms, 37.6 FPS at hold 1).

### 2. Tests (`cargo test --release -p graphics`: lib 274 passed, 6 ignored; other targets pass)
- `exec::chain_tests::a_wait_chains_on_recorded_gpu_signals_or_a_quiet_cpu`: a wait is chained when a recorded GPU
  WRITE signal passes it, or when the event's value passes with no CPU signal since the sync; not when the recorded
  signal falls short, is an ADD, nothing passes it, or the value passes after a CPU signal (even with a recorded
  signal). New function, no failing-before state. (Its first version expected the last case the other way; the test
  was wrong, the rule is the intended one.)
- `queue::occl_tests` unchanged and passing. The chaining itself (submission order, receipts) is verified in the game
  only.

### 3. Verified
Runs and numbers in §3 (hold 1: `234759-mp-chain3-hold1` vs `235351-mp-off-hold1`; default: `000150-mp-final-on`,
`000728-mp-final-off`, `001814-mp-final-on2`; `001306-mp-final-turn`). Route checks on in every run (3/3, turn 4/4).
Every sheet read. **Target 60 not reached; MKW stays near 30 at the default pacer (+~2 FPS mean, 5 % low unchanged);
at hold 1 it runs 47 / 37 FPS against 45.7 / 38.0 with the switch off.** OoT (OOT-OCC) ran on GPU 1 during all runs.

### 4. Unproven
- The black / dim title shot in three switch-on runs (§3): a fade-in timing difference at the opening-to-title cut
  is the reading; a series of shots across 137-141 s with the switch on and off would settle it (the scenario takes
  one shot there). The fourth on-run showed the title, and its fault captures found no black frame.
- Gains are near the noise of a shared machine; the lead's quiet A/B decides (on/off, default pacer and hold 1).
- Validation layers were not run with the chained submissions.
- Next for 60 (§5): the main queue's later parts translated ahead on the lanes while the other queue's parts are
  translated (planned through the waits, their signals and fence values released in queue order); per-use sync
  (USE-SYNC) for the first part's sync; OOT-DESC's single image walk. The `fps_hold` decision (now 43-47 FPS uneven at
  hold 1, not 35-36) is the owner's.

### 5. Slips
- `cargo fmt -p graphics` reformatted the whole crate (the tree was not fmt-clean) and that went into a commit; it was
  undone at once (`git reset --soft HEAD~1`, the untouched files restored with `git checkout HEAD -- <files>`, my
  three files' edits made again with the editor). The branch history has no formatting commit; the committed diff is
  only this wave's lines (`git show --stat a59c2aa`: 5 files, +215 / −6).
- Relinks ran this worktree's `target\release\nxrs-build.exe` directly with the launcher's argument list and
  `NXRS_LINK_LIBS=-Wl,/DEBUG ...` (a PDB beside the DLL, for the host profile), as USE-SYNC and SET-FAST did. The MKW
  DLL in the library is this branch's `a59c2aa` build with that PDB.

### 6. Questions
None.
