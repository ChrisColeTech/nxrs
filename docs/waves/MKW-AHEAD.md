# MKW-AHEAD — the frame op's later parts translated ahead, off the replay thread

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~00:30, worktree
`D:\Projects\RecompLauncherEngine-mkwahead`, branch `mkwahead` (base `mkwpipe` `ded6464`: USE-SYNC chunk tracking,
SET-FAST step 1, MKW-PIPE park chain). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0.
Deadline 4 h.

## Starting point (MKW-PIPE)

The race frame is the game's own fence chain: period 18.6 ms = replay of the frame's submit op 15.2 + GPU tail 1.8 +
the next submit after the main thread's release 1.5 (hold 1, medians). 60 at the default pacer needs the op under
~12.5 ms. Inside the op the two NVN queues hand work to each other (A1, B1, A2, B2, A3, B3, A4), each part a Vulkan
submission of its own, translated one after another on the replay thread (each part's segments over the lanes).

## Plan (written before code)

1. **Split the op.** Chain-trace notes for each part (`xlated`: bytes, segments) beside the existing `prep`, `chain`,
   `park`, `vk_submit`, over one race stretch at hold 1: per part, prep / translation / submit and whether its
   translation used the lanes. That says what translate-ahead can move off the replay thread.
2. **Translate-ahead** (`NXGFX_AHEAD`, 0 = old): the parts that will run on are known from the streams (each part's
   GPU signal records release the other queue's wait); translate the later parts on the lanes while earlier parts are
   translated, through the gate (shared state changes in queue order), and submit each part's submission in queue
   order, releasing its signals and receipts then. Built only if step 1 shows the lanes idle for part of the op.
3. **Smaller levers:** OOT-DESC's single image walk (cherry-picked: `f1a363a`, no switch, the same behaviour); the
   use-driven half of USE-SYNC if the first part's sync is still ~2.5 ms.
4. Verify: `mkw`, `mkw-turn` (routes, every sheet), default pacer and `NXOS_FPS_HOLD=off`, same-build switch-off
   controls back to back; `cargo test --release -p graphics`.

Files expected: `vk/queue.rs`, `vk/exec.rs`, maybe `vk/gate.rs`, `docs/env_switches.tsv`, this doc.

What would disprove it: the parts' translation already fills the lanes (the replay thread waits for them, not for its
own serial work), or the gate gives up on most ahead segments (their shared-state changes cannot run early).
