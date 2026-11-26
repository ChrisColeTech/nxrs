# WEB-W4 - Anime Girls' real frames in Chrome (command ring + WebGPU worker)

Wave of `docs/35-WEB-TARGET-PLAN.md` phase W4 (rows "Graphics split", "GPU worker", "SPIR-V -> WGSL", "Pool sync
without write watch", "Anime Girls frames in Chrome"). Base `3d12ad0`, branch `web-w4`, title Anime Girls Sun of a
Beach (0100C6801E0FA000), dev server port 8772, GPU 1 for native comparison runs. Decisions in force: D1 (web renders
with WebGPU, native stays Vulkan only), D3 (command ring + JS GPU worker, nxrs streams the replay's resolved output).

## 1. Where the replay meets Vulkan (evidence)

The replay is not one layer that can be lifted off Vulkan; Vulkan is its vocabulary.

| Coupling | Where | Size |
|---|---|---|
| The back end object is a Vulkan device | `vk/mod.rs:76-89` (`Gpu { d: device::Device, alloc: gpu_allocator::vulkan::Allocator, ... }`), `vk/device.rs:107-136` (`ash::Device` + 13 extension loaders) | every module takes `&Gpu` |
| One submission's replay writes Vulkan commands directly | `vk/exec.rs` (11,377 lines): `dev.cmd_*` at each draw (`exec.rs:6933` viewport, `exec.rs:4732` descriptors, pass begin/end, barriers) | 73 distinct `cmd_*` entry points, 230+ call sites in `vk/` |
| Resources are Vulkan objects end to end | `vk/image.rs` (5,401 lines: images, views, uploads, layouts), `vk/mem.rs` (4,106: pool buffers, staging, write-back), `vk/pipe.rs` (1,827: pipeline state from NVN state), `vk/shader.rs` (SPIR-V rewrite to the shared layout, `layout.rs:1-20`) | ~100 distinct non-`cmd` entry points |
| Pipelines are built with Vulkan state, partly dynamic | `pipe.rs` + EDS2/EDS3/vertex-input dynamic state set per draw in `exec.rs` | required by `device.rs:313-322` |
| Shaders are rewritten per pipeline state at run time | `shader.rs:1098-1198` (fragment variants, vertex-input retyping, storage emulation) | the final SPIR-V exists only at run time |

Splitting "what to draw" from "issue Vulkan calls" inside these files means rewriting the replay (exec, image, mem,
pipe, queue: ~25,000 lines) against a new abstraction, with every native path re-verified. That is the rewrite the
brief asks to avoid.

## 2. The cut: a WebGPU-backed Vulkan device under the replay (web build only)

The replay reaches Vulkan only through ash's function tables, which ash fills from `vkGetInstanceProcAddr`
(`ash::Entry::from_static_fn`). The web build gives ash **our own** `vkGetInstanceProcAddr`, implemented in Rust in
`src/graphics/src/web/` (compiled only for `target_os = "emscripten"`). The replay runs unchanged above it; native
builds never compile it, so the Vulkan path stays byte-for-byte the same (`device.rs` gains one `cfg` arm choosing the
entry).

What the web device does with each call is the "resolved replay output" of D3:

- **Recording** (`vkCmd*`): commands are kept per command buffer as Vulkan-level records.
- **Submission** (`vkQueueSubmit2`): the records are resolved into WebGPU-shaped packets in the command ring: render
  passes with load/store/clear, `setPipeline` with a WebGPU pipeline id (created from the bound pipeline + the dynamic
  state at that draw + the pass's formats, cached by hash), bind groups by id (built from the bound descriptor sets,
  cached by content), draws, dispatches, copies, clears, blits; plus the uploads the submission needs from host-visible
  memory **by linear-memory address** (the worker writes them straight from the shared memory), the read-backs it
  owes host-visible memory, and the timeline values to signal when done.
- **Objects** (buffers, textures, views, samplers, shader modules, bind group layouts): created by packets as Vulkan
  asks for them, by 32-bit ids.
- **Sync**: timeline semaphores and fences are words in shared memory; the worker writes them after the GPU finished
  (and read-backs landed) and wakes the engine thread (`Atomics.notify` / `memory.atomic.wait32`).
- **Present**: the web device offers one surface (the page canvas); swap chain images are textures of the canvas's
  format; `vkQueuePresentKHR` copies the image to the canvas's current texture.
- **What it reports**: no descriptor buffers, no buffer device addresses, no sparse, no mesh/geometry/tessellation,
  no transform feedback, no ray tracing, no graphics pipeline libraries. The replay already has paths for each of
  those being absent (`caps.descbuf`, `caps.bda`, `caps.sparse*`, `caps.gpl`, `caps.mesh`, `caps.xfb`). A draw that
  needs a stage WebGPU lacks is refused loudly (red line naming the pipeline), never dropped silently.

**Ring** (`web/ring.rs`, read by `src/web/gpu-worker.js`): a static block in linear memory: header words (magic,
head, tail, worker state, canvas size, error and stats counters) and a byte ring of packets `[op u32, len u32,
payload...]`. Producers append under a lock and notify; the worker waits with `Atomics.waitAsync` and drains, yielding
to the event loop between submissions. Large payloads (WGSL text, pipeline descriptors) are JSON in the packet.

**Shaders** (`src/shaders/src/wgsl.rs`): final SPIR-V -> WGSL with naga, the W0 rules ported (trim unused
capabilities/interlock, split combined image-samplers, drop PointSize/clip members, exact isnan/isinf, base instance
from a per-draw uniform, push constants as a uniform, `diagnostic(off, derivative_uniformity)`, unwritten vertex-stage
storage buffers declared `read`), plus two the web device needs: Vulkan's negative-height viewports become a per-draw
Y flip of the position (WebGPU has no negative viewport) and depth clip "negative one to one" a per-draw z remap.
Descriptor set -> bind group: stage set 0/5 -> group 0, set 1 -> group 1, pool set 6 -> group 2, per-draw uniforms ->
group 3. Converted at `vkCreateShaderModule` because the module's final SPIR-V is produced at run time (section 1);
build-time WGSL packs of the base modules are a cache for W6 (lazy packs).

**Pool sync without write watch** (`vk/mem.rs` web arm): wasm has no write watch, today's web answer is "not watched"
-> snapshot compare of every pool at every submission. Replacement: only pools the submission's draws, dispatches and
copies bind are compared (the resolve path already names each range it binds); cost measured per frame.

## 3. Files

| File | Change |
|---|---|
| `src/graphics/src/web/mod.rs`, `ring.rs`, `icd*.rs` | new: the web device (Vulkan entry points -> ring), emscripten only |
| `src/graphics/src/vk/device.rs` | `cfg(target_os = "emscripten")`: the entry from `web::entry()` |
| `src/graphics/src/vk/present.rs` | web arm of `create_surface`: the canvas surface; the canvas as the window |
| `src/graphics/src/vk/mem.rs` | web arm of pool sync (bound pools only) |
| `src/shaders/src/wgsl.rs` (+ `Cargo.toml`: naga) | SPIR-V -> WGSL with the W0 rules; tests on the title's used set |
| `src/web/gpu-worker.js`, `src/web/gpu.js`, `src/web/nxgfx.lib.js` | new: the WebGPU executor, its page-side start, the Emscripten JS library (ring start, shader file fetch) |
| `src/web/boot.js` | one GPU-start hook (W3 owns the rest) |
| `src/web/index.html`, `dev_server.py` | the canvas; serve the new files and the shader folder |
| `src/cli/src/cc.rs`, `pipeline.rs` | link the JS library; the web build's shader prebuild |

## 4. Milestones

M1 ring + worker + a presented clear of the game's first render target; M2 pipelines with converted shaders; M3
textures and uploads; M4 the title menu and beach gameplay compared with the native sheet. Each committed on
`web-w4` when verified. Native: graphics/shaders/os/runtime/cli tests and a native Anime Girls run at 60 FPS.

## 5. Results

(filled as milestones land)
