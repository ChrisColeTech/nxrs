# OOT-NAN — Ocarina's forest is white (NaN in the HDR forward pass)

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md` (dist lock / machine lock; never two copies of one title). Runs
**alone** among engine waves (an independent read-only audit of OOT-RGB9E5's `storemu.rs` / `image.rs` / `exec.rs`
change may still be running; if the lead then fixes something there it will tell you). Title: "The Legend of Zelda
Ocarina of Time" (scenario `oot`, GPU 1); regression on Mario Kart World at the end (scenario `mkw`, GPU 0).

## Brief (lead)

**Problem (recorded, `docs/waves/OOT-RGB9E5.md` report).** With the E5B9G9R9 storage fix in, the 630 red lines are
gone (2 left, both known), but from the Kokiri Forest load every frame is white behind correct subtitles
(`build/play/20261125-140847-oot`). Pass dumps (`build/play/20261125-141630-oot-dump2`, `-143329-oot-dump3`,
`-144102-oot-dump4`): the forest's forward pass (pixel shader `9f36b8ee91e736c6`) writes NaN into 42-57% of the
HDR target; the volumetric-fog draw `888c1068ac8f7043` then makes 100% NaN, shown white. The 640x360 inputs are clean.

**First suspect (from that report, not chased):** about a dozen textures of unrelated sizes all report their
storage at `0x13dc17000`, the start of the physical pool that OOT-PIPE made GPU-backed (`vk/mem.rs`
`placement_for`). If textures in a physical pool resolve to the pool's base instead of base + offset, they alias
each other and inputs become garbage (NaN). OOT-PIPE's final run shows the same lines, so it predates the storage
fix. Check this first; then bisect the forward pass's draws and inputs (its 1280x720 and 3D-volume inputs) and
Maxwell's `0 * Inf = 0` (FMUL/FFMA `.FMZ`) rule in the shader translation (`dmsh_xlate` output; nxrs patches SPIR-V
after it in `vk/spvfix.rs`).

**Goal / done.** The cause found and fixed as a general correction; the `oot` run's forest shots show the scene,
no device loss, red lines unchanged or fewer. Then, if time allows, push the `oot` route to **Link moving under
input** (59 FPS target; report FPS vs 59, 5% low).

**Tests:** a unit test for the correction (e.g. a physical-pool placement at a non-zero offset resolves to base +
offset); `cargo test -p graphics --lib --release`. **Regression:** `py scripts/play.py mkw --env NXGFX_GPU=0` after
relinking it: race FPS (last 29.6), red lines (known: 1), sheet read.

**Memory/descriptor/coherence changes: the lead runs an independent audit.** Never skip draws; don't weaken
robustness (no NaN scrubbing that hides a real cause); native NDVE stays off. **Exclusions:** `dmsh_xlate` itself,
other titles' routes, no commits, don't edit docs/32. Back up and restore anything you move (pipeline caches,
saves), and check the restore direction.

**Deadline:** 4 hours wall clock. Report per protocol §5 in this file under "## Report".
