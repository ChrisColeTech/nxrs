# WEB-W10c: Mario Kart 8 Deluxe past its title logo in Chrome (a pass sampling its own depth attachment)

Branch `web-w10c` (from `gameplay-harness` b0ba530). Title: Mario Kart 8 Deluxe (0100152000022000), web folder
`P:\Games\Nintendo\DUMPS\Mario Kart 8 Deluxe\nxrs-web\Mario Kart 8 Deluxe 1G1U1D`; regression: Anime Girls (beach).
Chrome on GPU 1 only (`--use-adapter-luid=0,113326`), one game Chrome at a time. Evidence `build/WEB/W10c`.

## Symptom (WEB-W10b, `build/WEB/W10b/mk8d/run1`)

The title logo at 60 FPS, then nothing changes; 14,940 WebGPU errors by 157 s, all one cause:

```
[Texture (1920x1080 Depth32Float)] usage (TextureBinding|RenderAttachment) includes writable usage and another usage
in the same synchronization scope.
```

A render pass samples the depth texture it has as its depth attachment. Vulkan allows that while the pass's draws do
not write depth (the attachment is only read); WebGPU allows it only when the attachment is declared
`depthReadOnly` (`stencilReadOnly` for the stencil aspect), which forbids load/store ops on that aspect and pipelines
that write it. The web replay always declared the attachment writable.

## Plan (native behaviour is the reference)

The replay (`src/graphics/src/web/submit.rs`) resolves a pass's draws one at a time and only then knows which draws
sample the attachment and which write it. So a pass is replayed as **segments**:

1. A segment starts at the pass begin (as recorded). It tracks, per aspect (depth, stencil), whether a draw of it
   sampled the attachment's image (a bound texture view of that image, its aspect, overlapping the attachment's mip
   and layers) and whether something wrote it (a clear load op, a draw whose pipeline writes the aspect, a
   `vkCmdClearAttachments` of it).
2. A draw that samples an aspect the segment has written, or writes one the segment has sampled, ends the segment
   before the draw: the segment's colour and depth/stencil stores are kept (`store`), and a continuation segment
   begins with every attachment loaded (no clears), the same views, resolves and formats. Vulkan sees one pass;
   WebGPU sees two that leave the same contents.
3. When a segment ends (split or pass end), each aspect it sampled and never wrote is marked read-only in its begin
   packet (worker mode 3: `depthReadOnly` / `stencilReadOnly`, no load/store ops). Depth and stencil separately.
4. A draw that samples and writes the same aspect at once (a real feedback loop, undefined in Vulkan without the
   feedback-loop extension) is left as is and named in a red line; evidence decides whether it needs more.

Worker (`src/web/gpu-worker.js` `PASS_BEGIN`): mode 3 = read-only aspect.

Tests: the segment rules and the packet edits are pure functions (`src/graphics/src/web/seg.rs`), compiled into a
host test (`cargo test --release -p graphics --lib web_seg`).

Then the next blocker on the way to the title menu and a race, as time allows (seen in W10b: `uniform bank c3 ... in
no memory pool` red lines at 26 s, a movie decoder event not delivered at 57 s). Anime Girls re-run once with the
current engine (web build rebuilt into its library web folder after a backup).

Expected files: `src/graphics/src/web/{submit,seg,mod}.rs`, `src/graphics/src/lib.rs` (test include),
`src/web/gpu-worker.js`, this document, `docs/35-WEB-TARGET-PLAN.md` (W10c row).

## Results

(filled in as the work lands)
