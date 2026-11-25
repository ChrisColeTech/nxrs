# OOT-FOREST2 — Ocarina: indirect dispatch sizes, forest haze, forest frame time, Link moving

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md`. Works in the **main tree** (`D:\Projects\RecompLauncherEngine`).
Title: "The Legend of Zelda Ocarina of Time" (scenario `oot`, GPU 1).

**Runs beside MKW-RECORD**, which works in its own git worktree with its own `target/` and `dist/` and relinks only
Mario Kart World. So: **do not build, relink or run Mario Kart World**; your regression title is `wonder` (relink it
from the main `dist`). Both waves use the main tree's `build/locks/` — when MKW-RECORD holds
`build/locks/machine.lock` (a perf measurement), start no game or build until it is released. An independent
read-only audit of OOT-NAN's change may also be running; if the lead fixes something it will tell you.

## Brief (lead)

**State (OOT-NAN, `docs/waves/OOT-NAN.md`, commit `9903fc7`).** The forest renders since the NVN2 dispatch-size fix
(c0 + 0x4). Open, from that report:

1. **Indirect dispatches** still put CPU-side counts (probably 0) into c0 at both offsets (0x200 and NVN2 0x4),
   where nxnative copies the indirect record into c0 on the GPU. Any compute that reads its group counts under an
   indirect dispatch divides or loops by 0. Affects NVN1 titles too. Fix it as nxnative does (a GPU copy of the
   indirect record into the c0 buffer before the dispatch, with the right barrier), test it, and show the effect.
2. **The forest is very misty and blown out** (`build/play/20261125-153150-oot-nan-fix` shots 06-08: heavy white
   haze over the Deku Tree). The lead's read of the sheet is that it is too strong. First suspect: item 1 (fog or
   froxel computes dispatched indirectly). Prove it either way with a pass dump.
3. **Forest frame time**: frames of 70-380 ms (25-35 ms of pool preparation, ~56 MB uploaded per frame), and ~20 s
   with no frames presented entering Link's house (~617-637 s). Find what is uploaded every frame and why (memory
   that does not change should not be re-uploaded); fix as a general correction.
4. **Route to Link moving under input**: press A faster through the house dialogue, put the walk and the FPS window
   right after "Let's go together!". Target 59 FPS; report FPS vs 59 and 5% low.

**Done:** items 1-3 fixed with game evidence (sheet read), item 4 reached with a measured window; or parked at the
deadline with where each stands. Unit tests for each correction; `cargo test -p graphics --lib --release`.
Regression: `py scripts/play.py wonder --env NXGFX_GPU=0` (sheet read, red lines unchanged).

**Memory/descriptor/coherence changes: the lead runs an independent audit.** Never skip draws or dispatches; don't
weaken robustness; native NDVE stays off. **Exclusions:** `dmsh_xlate` itself, Mario Kart World, `vk/queue.rs`
(MKW-RECORD's), no commits, don't edit docs/32. Keep `vk/exec.rs` / `vk/mem.rs` edits minimal and local (the
lead merges MKW-RECORD's branch later). Back up and restore anything you move, and check the direction.

**Deadline:** 5 hours wall clock. Report per protocol §5 in this file under "## Report".
