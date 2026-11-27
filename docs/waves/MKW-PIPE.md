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

| Run (`build/play/`) | switch | FPS / 5 % low [250, 325] | period | frame op | tail | main waits | route |
|---|---|---|---|---|---|---|---|
| `20261126-235351-mp-off-hold1` | `NXGFX_PARK_CHAIN=0` | 45.7 / 38.0 | 20.2 | 17.1 | 1.1 | 6.1 | 3/3 on |
| `20261126-234759-mp-chain3-hold1` | on | **47.3 / 37.0** | **18.6** | **15.2** | 1.8 | 5.3 | 3/3 on |
| `20261126-232459-mp-chain2-hold1` (earlier build, off path) | - | 45.0 / 38.0 | 21.2 | 17.9 | 1.0 | 7.1 | 3/3 on |

The frame op is ~2 ms shorter (prep on its path 7.0 → ~5 ms a frame, no retry hand-overs); the tail grows ~0.7 ms
(the last parts reach the GPU later); the period gains ~1.6-2.6 ms. FPS at hold 1: +1.6 mean, 5 % low unchanged:
within run-to-run noise on a shared machine (OoT ran on GPU 1 throughout). **At the default hold it does not lift the
30 hold**: the trial threshold is a cost ≤ 0.95 vsync (15.8 ms), and the frame is ~18.6 ms.

FINAL_RUNS

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

REPORT
