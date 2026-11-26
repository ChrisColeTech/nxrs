# NVN2-PARITY — Switch 2 titles: pop-in and frame rate (NVN2 gaps against nxnative)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md`. Works in the **main tree** after OOT-FOREST2 has landed. MKW-RECORD runs
beside it in the worktree `D:\Projects\RecompLauncherEngine-mkw` and relinks Mario Kart World there: **do not
relink Mario Kart World from the main tree**; use the other Switch 2 titles for game checks (Donkey Kong Bananza
`dkb`, Ocarina `oot`, and any other 04… title in the library); hand MKW checks to the lead. Respect
`build/locks/machine.lock`.

## Brief (lead)

**Owner report (2026-11-25):** every Switch 2 title misses 60 FPS and has textures popping in and out; every Switch 1
title is fine. Two NVN2-only gaps have already caused visible bugs (dispatch size at c0+0x4, OOT-NAN; indirect
dispatch sizes, OOT-FOREST2). A read-only parity audit against nxnative (`D:\Projects\nxnative\dm`) found more. Fix
them, in this order, each as a general correction with a unit test, checked in game:

1. **Pushed buffer addresses and uniform-buffer handles resolve to nothing** (Switch 2 only).
   - `vk/exec.rs` `bank_word` returns 0 for banks 24-29: return the push data (slot 24: `qs.push[grp]`; 25-29: the
     graphics group's blocks), as nxnative `nvn_mtl.m:13227` (NVN_RB_SBUFQ).
   - `bank_u64` never decodes a handle: apply `ucb_handle_addr` (`nvn_mtl.m:1552`: a word with bits >= 45 set is
     `(v & (2^45-1)) << 4`); with no size, bound to its pool as the size-0 path does.
   - `c0_bytes`: on Switch 2 write compute's uniform table (0x210 + 16r) as `ucb_handle` (`nvn_mtl.m:1551, 13298`),
     size kept at +8; cap the size field in `nvn/query.rs:372` the same way (2^19-1).
2. **`nvnDeviceGetCurrentTimestampInNanoseconds` is on the wrong clock** (`nvn/device.rs:215` `now_ns()`, its own
   epoch): use the guest clock the counter reports and the tick counter use (nxnative `hle_nvn.c:3277` `guest_ns`).
   A frame-rate / dynamic-resolution controller comparing the two held Donkey Kong Bananza at 20 FPS in nxnative.
3. **Two-word texture handles**: the texture ID must come from `word | second_word` (UE5.4 on Switch 2 puts the
   sampler word first), `nvn_mtl.m:12637-12639`; nxrs takes the first word only (`exec.rs` handle kind 1).
4. **Inline uniform updates (`UPDATE_UB`)**: first count them per frame on a Switch 2 and a Switch 1 title. Then:
   CPU-side handle/address reads from uniform banks must see this submission's queued updates (nxnative `ubo_read`,
   `nvn_mtl.m:12619-12628`), and an update should end the render pass only when a later draw's GPU read overlaps it
   (nxnative defers inside a pass, `nvn_mtl.m:12854`). **Coherence change: audit.**
5. **Uniform size rounded up to 256** (NVN2 method 0xe02, `nvn_mtl.m:12846`): `ubo_info` range and `bank_word`
   bound use `min(round256(n), room, max_ubo)`.
6. **Sparse tiles**: check the logs of the Switch 2 runs for "tile memory over NXGFX_SPARSE_MIB" / "no memory for a
   tile"; if present, keep tile slots alive while the physical pool lives instead of per mapping (`vk/sparse.rs`
   443-556), as nxnative keeps physical memory.
7. **Small items**: device item 0x06 answers 16 (or accept 32 vertex buffers), 0x62 answers 32.
8. **Render size check (A/B, no code):** docked on a 4K display both projects answer 3840x2160 with scale tier 0, so
   Switch 2 titles render at their maximum. Measure one title with `NXOS_DISPLAY_MAX` / `NXOS_GFX_SCALE` lowered,
   to tell how much of the FPS gap is render size; report it, change no defaults.

Out of scope here (recorded for later waves): DLSS as a temporal upscaler (today a plain blit: depth, motion, jitter
not recorded), ray tracing, the c0 sample table / point size.

**Done:** items 1-3, 5, 7 fixed with tests; 4 and 6 measured and fixed or explained; 8 measured. Game evidence on
`dkb` and `oot` (sheet read; FPS vs 60 / 59 and 5% low; pop-in watched across consecutive shots, before vs after),
plus one Switch 1 regression (`wonder` or `smash`) unchanged. `cargo test -p graphics --lib --release`.

**Descriptor/memory/coherence changes: the lead runs an independent audit.** Never skip draws; don't weaken
robustness; native NDVE stays off. **Exclusions:** `vk/queue.rs` and the replay split (MKW-RECORD's), no MKW relinks,
no commits, don't edit docs/32. Back up and restore anything you move, and check the direction.

**Deadline:** 6 hours wall clock. Report per protocol §5 in this file under "## Report".
