# MKW-HEAVY — the heavy-draw frames, the frame's tail, and the pacer's trial back-off

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~08:37, worktree
`D:\Projects\RecompLauncherEngine-mkwheavy`, branch `mkwheavy` (base `gameplay-harness` `9628e36`: MKW-SPIKE merged,
`NXGFX_UPLOAD_UNLOCKED` opt-in and left off). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only,
GPU 0 (OOT-MEM owns OoT on GPU 1). Deadline 4 h (~12:37).

## Starting point (MKW-SPIKE)

Heavy-draw stretches (2,200-2,500 draws) make the replay op 14-16 ms and the frame shown op + ~3.2 ms = 19-20 ms: late,
so the pacer leaves 1 vsync; a failed 1-vsync trial doubles the hold-off before the next (1 → 8 s at 30 FPS), which is
the 5 % low of ~29. Quiet-machine numbers of one build: 42.7-52.8 FPS mean, 5 % low ~29.5.

## Plan (written before code)

1. **Baseline** (`mkw`, default pacer, GPU 0) with the diagnostics on: frames CSV (op = `replay_ms` + `prep_ms`, by
   draw count), the frame step's summaries, the `pipelining` lines, and a chain trace over a heavy stretch
   (`NXGFX_CHAIN_TRACE`) with one new note, `cpl_land` (the completion thread's write-backs and posts done, before its
   clean-up), to split op end → next op start into: submit, GPU + wake, landing, clean-up before publication, the game's
   wake, the game to its next submit, the replay queue.
2. **The tail (~3.2 ms)**: cut what is ours. Expected: the completion thread publishes a submission's fence only after
   freeing its resources (views, buffers, descriptor pools of the parent and every child submission, recycling); the
   publication can come right after the effects the game sees (write-backs, events, queries) and the clean-up after.
3. **Heavy-draw frames**: per-draw cost on the lanes against the replay thread's serial part (joins, stitching,
   submits), from the frames CSV by draw count and the `replay up to frame` / segment lines; cut the serial part.
4. **Pacer** (`src/os/src/fstep.rs`): a failed trial no longer doubles the hold-off to 8 s; trials short and frequent,
   behind a switch; game time against real time stays ~1.000 (the summaries), frames shown late are paid back.
5. **Verify**: `mkw`, `mkw-turn` on GPU 0, route checks, every sheet; `cargo test --release -p graphics -p os`. The final
   run compared with the baseline and its diagnostics (no switch-off controls).

Expected files: `src/graphics/src/vk/queue.rs` (completion), `src/os/src/fstep.rs`, maybe `exec.rs`,
`docs/env_switches.tsv`, this doc. What would disprove step 2: the tail is GPU time or the game's own time, not the
completion's work.
