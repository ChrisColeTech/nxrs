# OOT-MEM — Ocarina of Time's image growth (the 32 GB cap at ~840-895 s), then the pool-lock wait

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 07:05, worktree
`D:\Projects\RecompLauncherEngine-ootmem`, branch `ootmem` (base `gameplay-harness` `d3e4fc2`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 1 (MKW-SPIKE has MKW on GPU 0). Deadline 4 h
(11:05).

## Plan (written first)

1. **Census diagnostic** (`image.rs` `image_census`, logged with the memory line every 600 frames): images made by
   how (new, parked reuse, takeover, alias), released and destroyed; nvnTextureInitialize / Finalize / Initialize
   over a live object; and the live images whose storage a newer live image overlaps (the game made another texture
   over that memory without finalizing this one), grouped by format and size.
2. **Baseline** `oot` with the census on (GPU 1): memory and image lines over the run, FPS / 5 % low per window
   (house [600, 830], walk [845, 915]), replay busy, pool-sync split.
3. **Fix the cause at source**: free or reuse what the game abandoned or finalized; no caps.
4. **Pool-lock wait** behind write-back landings (completion thread), if time is left.
5. **Verify**: full `oot` to its end without the cap, `oot-house --faults`, route checks, every sheet;
   `cargo test --release -p graphics`.

Expected files: `src/graphics/src/vk/image.rs`, `vk/perf.rs`, `nvn/tex.rs`, possibly `vk/queue.rs` / `vk/mem.rs`,
`docs/env_switches.tsv`, this document.
