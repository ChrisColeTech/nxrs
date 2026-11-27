# OOT-FIX — Ocarina of Time: washed-out colours (A), then the replay thread under 15.8 ms (B)

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 15:40, worktree
`D:\Projects\RecompLauncherEngine-ootfix`, branch `ootfix` (base `gameplay-harness` `2f1eaf3`). Protocol
`docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of Time only, GPU 0. Deadline 5 h (20:40).

## Plan (written first)

Baseline: the lead's run `build/play/20261127-145622-oot` (main checkout; 29.7 / 27.0 over [845, 915], replay busy
16.67 ms, pool sync: landings first 2.9 ms, lock 0.96, scan 1.95, per-pool pass 3.55 ms, 3.1 syncs a frame) and this
wave's first run with a pass dump in the house.

**A. Washed-out colours.** Reference: nxnative's OoT note (`D:\Projects\nxnative\dm\games\0400486024DEC000\README.md`,
"against a frame of console video of Navi's close-up the picture matches") and its finding *Float atomics in storage
memory* (`docs/reference/shaders.md`): lifted blacks, yellow skin, a pale veil = agl's scene analysis
(`probe_analyze` & co., float sums by `RED.F32`) not reaching the CPU, which sets the exposure from them. Our
pictures show exactly that (lifted blacks in the house, yellow-orange skin, the Deku Tree / forest load shots under a
pale veil); nxnative's screenshot `docs/images/zelda-ocarina-of-time.jpg` has deep blacks. Steps: (1) check the output
chain: present format (`present.rs`: the game's 2560x1440 RGBA8 UNORM blitted to a B8G8R8A8_UNORM swapchain, no
second encode), the tone map's targets and views (pass dump); (2) the analysis dispatches (`6e28359aa3558f61`,
`1bba09ec4bc8ff67`, ...): their storage buffer, what the GPU sums and what guest memory holds when the CPU reads it
(pass dump's storage words); (3) fix where the value goes wrong; before/after shots side by side. Then whether the
white house-floor frame shares the cause.

**B. FPS.** The pool sync (3.1 syncs a frame: why, can it be one), the per-pool pass, the skipped draws (`no pipeline`
38,070, `no target` 15,855: is work missing). Each change measured against the baseline's diagnostics; one that moves
nothing comes out.

Expected files: `src/graphics/src/vk/exec.rs`, `mem.rs`, `lazy.rs` / `queue.rs` (by the findings),
`docs/env_switches.tsv`, this document.
