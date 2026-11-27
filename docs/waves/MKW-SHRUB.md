# MKW-SHRUB — the near-black roadside shrubs, and MKW's stale pipeline crash list

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~11:20, worktree
`D:\Projects\RecompLauncherEngine-mkwshrub`, branch `mkwshrub` (base `gameplay-harness` `bb49dc0`: MKW-RENDER merged).
Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0. Deadline 3 h (~14:20).

## Starting point

`docs/waves/MKW-RENDER.md` §4: the shrubs on the desert slope are drawn by one colour draw (vertex `def3f05b1930ac56`,
pixel `cb0bfd018d46e25c`), their pixels are written, the leaf albedo arrives, the screen-space shadow mask says lit,
yet the result is ~1/10 of the sand's brightness. Ruled out there: the depth test, the 18 skipped draws, the 165x165
mask texture. Named next step: the BC6H lighting texture tiles and the probe arrays the draw samples.

## Plan (written before code)

1. **Baseline** on the base build (`bb49dc0` relinked from this worktree): `mkw-pop --faults` with a raw pass dump
   in the race (`NXGFX_DUMP_AT_S`, `NXGFX_DUMP_RAW=1`); read the sheets and the log's sparse-image lines.
2. **Find which input is dark**: the pixel shader's colour is `T * lit + inscatter`; its direct term is multiplied by
   a sample of the sparse array `tcb_8` (an 8192x8192x68 terrain array) and the shadow mask. A diagnostic copy of the
   pixel shader (scratchpad, swapped into the title's shader folder for one run, original restored and hash-checked)
   writes chosen intermediate values to its colour output, read back raw from the pass dump. The dark factor names
   the input; then the engine path that feeds it (sparse binding, format, upload, constants).
3. **Fix at source** (expected: `src/graphics/src/vk/sparse.rs` or `image.rs`), with a test of the rule.
4. **Crash list**: back up MKW's `pipeline-crashed.bin` (3 entries, 2026-11-24, shown to compile cleanly in
   MKW-RENDER §5), remove the file (the engine's format holds nothing else; an empty list is no file), run `mkw-pop`
   in full: 0 skipped draws, clean exit, no new crash list.
5. **Verify**: `mkw-pop` and `mkw` after; shrub shots before/after; route checks; stand-ins in report.md; FPS
   against the baseline; `cargo test --release -p graphics`.

What would disprove the "lighting input" hypothesis: every factor of the direct and indirect terms in range for the
shrub pixels (then the darkening is in the vertex inputs or the constants the material reads).
