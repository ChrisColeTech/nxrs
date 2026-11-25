# OOT-PIPE — Ocarina of Time: the compute pipeline that never finishes compiling

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
parallel rules: `docs/waves/PARALLEL-RULES.md` (binding). **GPU 1.** Title: "The Legend of Zelda Ocarina of Time"
only (scenario `oot`).

## Brief (lead)

**Problem (recorded, old hardware and an older engine).** OoT boots to nnMain and a first present, then after ~119-144
presents the replay sits in `vkCreateComputePipelines` for shader `d15370a382630d4e`, the game's NDVE upscaler
stage `up_64_32` (2,857,956 bytes of SPIR-V, 176,275 instructions), which never returns before shutdown; the screen
stays black (`docs/PORT_PLAN.md` M8, `docs/INTERFACES.md` ~line 82/140, `build/OOTWAIT/investigation.json`). Native
NDVE replacement **stays off** (owner rule; `docs/33` R4/R5 describe that path — not this wave).

**Goal / done.** OoT past that compile on the guest's own shader path, then the title and file menus and as far
toward first gameplay as the deadline allows. Done = a `play.py oot` run whose sheet shows the title/menus rendered
(not black), no hang, no device loss, with FPS vs 59.

**How.** 1) Re-run `oot` on today's build first (new machine: RTX 5090, current driver, current engine): does the
compile still hang, or is it just long (time it; `--diag pipeline`)? 2) If it does not return in reasonable time,
the fix is in how we translate that shader: find what makes the SPIR-V 176k instructions (fully unrolled loops,
duplicated helper inlining, huge constant arrays as instructions, unstructured control flow the driver chokes on)
and emit a compact equivalent in `src/shaders` (a general translator change, not keyed to OoT). Check the output
with `spirv-val` and the shader tests. 3) A compile that is long but finite must not stall the replay forever:
compile asynchronously / ahead (prewarm) as other titles do, but never skip the dispatch — the frame waits for its
pipeline if it must.

**Exclusions:** native NDVE (stays off), other titles, no commits, don't edit docs/32. A shader-translator change
affects every title: run `cargo test -p shaders` and say in the report which other titles could see the change.

**Deadline:** 3 hours wall clock (investigation + fix). Park with the evidence if it cannot be done.
Report per protocol §5 in this file under "## Report".

## Resume (lead, 2026-11-25, after VIOLET)

The wave runs **alone**. The premise changed: the compile no longer hangs with the pipeline cache present (parked
report below). New goal: (1) time one cold compile of `d15370a382630d4e` (move the title's `pipeline-cache.bin`
aside, restore it afterwards byte-identical); fix only if it is unreasonably long (async/ahead compile, never
skipping the dispatch, or a compact translation); (2) drive the `oot` route from the title through New Game / file
creation to **first controllable gameplay** (Link moving under input in the measured window), 59 FPS target, fixing
engine defects found on the way (general corrections, built under the dist lock, OoT relinked). Native NDVE stays
off. Deadline 3 hours from now.

## Report (agent, protocol §5; resumed alone 2026-11-25 ~12:15, ended ~14:40)

**Outcome.** The cold compile is ~60 s, it is finite, and the frame waits for it. The `oot` route now runs from the
title through New Game, the opening (which can't be skipped) and the title card into Kokiri Forest's Deku Tree / Navi
talk. **First controllable gameplay is not reached.** From the forest load (~340 s) on, every frame is plain black or
white behind correct subtitles. The cause found is E5B9G9R9 images the game writes as storage images: this device
can't use that format for storage, so the writes go to zero stand-ins. That was not fixed in the deadline (see
Unproven). On the way, two general engine defects were fixed: lazy write-back over uncommitted heap pages, and
physical pools with no GPU access.

1. **Built.**
   - **Cold compile (diagnosis, no change).** `d15370a382630d4e` (NDVE `up_64_32`) with `pipeline-cache.bin` removed
     and `__GL_SHADER_DISK_CACHE=0`, in run `build/play/20261125-122103-oot-cold2`: the prewarm thread started it at
     0.52 s and it was made at 66.29 s (**~66 s**). The replay reached its dispatch at 7.86 s and **waited 58.4 s**
     (worst frame 58,530 ms), then went on normally. The title rendered and New Game started the opening, so no
     dispatch was skipped. The first cold run (`-121948-oot-cold`) hit the harness's 30 s stall limit at 51 s and was
     stopped while still compiling, not hung. Why the compile is slow: the SPIR-V is **195,001 instructions**. Its
     main function holds 190,379 of them in only 25 blocks, with **44,218 function-local variables** (48,935 loads,
     44,374 stores, 1,536 calls, 96 FMul): straight-line code over spilled locals. The SPIR-V comes from
     `dmsh_xlate`, the external Ryujinx-based translator (`chain: dmsh_xlate e11dc43d4ddc5b58`), not from
     `src/shaders`. The compile is long but happens once per driver (the result is cached), and the prewarm already
     starts it at boot when the key is known. I made no async change (it would gain nothing: the frame must wait) and
     no translator change (see Unproven).
   - **Lazy write-back over uncommitted heap pages** (`src/graphics/src/vk/lazy.rs`, `take_lazy` +
     `committed_region`). Diagnosis: the os heap is committed only on first touch (`os/src/mem.rs`). Pool pages that
     the GPU writes but the CPU never touched are reserved, not committed. Once such a page qualified for lazy
     write-back, `VirtualProtect(PAGE_NOACCESS)` failed on it silently, yet it stayed in the lazy table. At the next
     host touch, `bring_back` could not make it accessible and logged a red line. The os first-touch handler then
     committed a zero page over the device's bytes. In `-cold2` this happened to **11,616 pages (45 MB) of pool
     0x195eea000 at once at 93.3 s**: 11,616 red lines and an 873 ms frame made of 11,616 single-page GPU reads.
     Correction: `take_lazy` only holds pages that are committed (or already held). It checks one `VirtualQuery`
     region per run of pages. Uncommitted pages land eagerly, where their first changed byte commits them, and can go
     lazy later.
   - **Physical pools had no GPU access** (`src/graphics/src/vk/mem.rs`, `placement_for`). Diagnosis: NVN requires
     `PHYSICAL` pools to be `GPU_NO_ACCESS`, and the GPU reaches them through virtual pools. OoT maps a whole 305 MB
     virtual pool as an alias of physical pool 0x13dc17000 (flags 0x18c), so its buffers resolve into that pool. The
     pool was `CpuOnly`, which gave `[RED] pool 0x13dc17000 has no GPU access`, and two compute shaders' storage
     buffers were bound as zero stand-ins (a 287 MB read-only range and a 1.8 MB written range). Correction: a
     `PHYSICAL` pool ignores `GPU_NO_ACCESS` for placement. It gets a GPU buffer at its first GPU use, like any other
     pool, and the log now shows `memory pool 0x13dc17000+0x13100000: device-local, backed in 2048 KB granules on
     first use`. **This is a memory and coherence change: the lead should audit it** (sparse virtual pools also
     resolve contiguous ranges into physical pools, so they are affected the same way).
   - **Tried and reverted:** making E5B9G9R9 storage images as B10G11R11 (same texel size, has storage), including
     their alias views. The forest went from black/white to solid **blue**, so the game also reads those bits as
     E5B9G9R9 and a format swap is wrong. All of it is reverted, and that run was deleted.
   - **Route:** the `[oot]` entry in `scripts/scenarios.toml` is my entry only. Title at 25 s, A every 3 s, shots
     through the story, nightmare, house, forest load, Deku Tree and Navi, 470 s limit, window [100, 300] (the
     opening's cutscenes). I found the route with explore runs: play.py's own `start()` driven from a scratch script
     outside the repo, with A every 3 s and frequent shots. New Game starts the opening directly (there is no file
     menu, and the name comes later or not at all). A neither skips nor hurries the opening. The forest loads at
     ~330-345 s with one 6-9 s stall (texture uploads 7.1 s, 26 pipelines 1.4 s in one frame).
2. **Tests.** `vk::lazy::tests::only_committed_pages_can_be_held` (new) reserves four pages and commits one, then
   asserts that `committed_region` reports the reserved ones as not committed and the committed one as committed,
   with the right bounds. The function is new, so the test could not fail before the fix; the game run is the
   before/after proof. `cargo test --release -p graphics`: 203 lib tests passed, 0 failed (plus the small
   binaries), run after the final source state. No unit test for `placement_for`; its proof is the game log.
3. **Verified** (all `--env NXGFX_GPU=1`, the wave alone on the machine, so no machine lock was needed; none of these
   is a perf-measurement pair). The engine was rebuilt four times with `build.ps1 -KeepCache -SkipGnu` (~30 s each)
   and OoT relinked each time (`nxrs.exe --select-title ... --build`, exit 0), all under `build/locks/dist.lock`,
   which is now released. The first relink also moved OoT off its 11-23 engine (520 shaders retranslated).
   - **Final: `build/play/20261125-131719-oot-final`** (`play.py oot`): completed 139/139 steps, normal exit, 447 s.
     Window [100, 300]: **30.4 FPS mean, 5% low 28.0, min 17.0 (target 59)**. That window is the opening's cutscenes,
     which the game runs at 30 (held at 2 vsyncs), so it is not gameplay. Worst pacing: 6,046 ms at 347.5 s (the
     forest load). **Red lines: 632**, all from the forest on: 630 are `texture ID 294..304: storage image requested
     but Vulkan image lacks STORAGE usage (E5B9G9R9_UFLOAT_PACK32 ...); a zero storage stand-in is bound` (one per
     re-registered serial), plus one `a view of a E5B9G9R9 image ... has no usage (storage)` and one `a draw with no
     render target bound (0 colour slots ...)`. There are none of the lazy write-back or "no GPU access" kinds, and
     no device loss. **Sheet read:** title (moon over the field), the story tapestry ("These forest dwellers were
     called the Kokiri"), Link's nightmare in his bed, Link's face at the house, all correct. Then the forest load
     is black, and the Deku Tree, Navi and end shots are white with the subtitle "It would seem...I have but a
     little time left...". The picture is lost there.
   - Before/after the lazy fix: `-122103-oot-cold2` (before) had 11,616 lazy red lines at 93.3 s; `-122832-xoot1`,
     `-123455-xoot2`, `-130908-xoot5-noblack` and the final run (after) have 0.
   - Before/after the physical-pool fix: `-123455-xoot2` (before) has `pool 0x13dc17000 has no GPU access` and the two
     zero storage buffers; `-xoot5-noblack` and the final run (after) have neither. The forest picture stays
     black/white either way, so the E5B9G9R9 storage stand-ins remain the blocker.
   - **Old compiler blacklist:** `pipeline-crashed.bin` (2 entries, written 2026-11-23 on the old, unstable machine)
     skipped one pipeline's draws at ~340 s. In `-xoot5-noblack` the file was moved aside and that pipeline compiled
     without a compiler crash (no new crash entry, normal exit). The file is restored byte-identical. The lead may
     remove that entry. The second entry was not exercised.
4. **Unproven / next.**
   - **First gameplay is blocked by E5B9G9R9 storage images** (texture IDs 294-304, 2560x1440, NVN format 0x40,
     flags 0x8, written by formatless `imageStore` with `storage_image_write_without_format`). The RTX 5090 has no
     storage use for that format. The likely fix: give such images storage through a same-size R32_UINT view
     (`MUTABLE_FORMAT | EXTENDED_USAGE`), and have the shader translation pack and unpack RGB9E5 for an image bound
     with that format. The packing needs a per-binding format fact, a shader-variant key and the SPIR-V pack code,
     so it touches the translation path that every title uses. Not started.
   - Cold compile ~60 s once per driver: the only real cure is a compact translation in `dmsh_xlate` (SSA instead
     of 44k locals). Other titles are not affected by anything I changed in shaders (I changed none).
   - Other titles that see my engine changes: every title with lazy write-back on (the default; pages that were
     never committed now land eagerly), and every title with physical or virtual pools (now GPU-backed, where before
     no buffer existed). I ran only OoT.
5. **Slips.**
   - **The original `pipeline-cache.bin` (152,780,979 bytes, SHA-256 03AC92E1...) was lost.** My restore command
     swapped source and destination in PowerShell (`Copy-Item "<backup>" -LiteralPath "<live>"`) and copied the live
     files over the backups. `pipeline-keys.bin` was rebuilt byte-identical: the file is append-only, and its first
     18,901 bytes hash to the original EF463FC8.... `pipeline-crashed.bin` was restored byte-identical from its
     intact backup. The live cache is now this session's 357,747,895-byte file from the same device and driver.
     Pipelines it lacks recompile on first use.
   - Explore runs used a scratch driver outside the repo. One misconfigured explore run was stopped by PID about
     4 s into boot (no crash entry resulted). Two explore runs were ended early with the control pipe's `quit`.
     Superseded runs (`-xoot3`, `-xoot4`, the pool-log run) are deleted.
6. **Questions.** None. The E5B9G9R9 storage fix is owed implementation, not a product choice.

(The parked report's one run, `build/play/20261125-084021-oot-pipe-base`, a 70 ms compile from the warm cache and
the title rendered, is superseded by the runs above.)
