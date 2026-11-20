# NVN objects

The record layer: the `nvn*` functions a game calls, implemented on the game's own objects in its own memory
(`src/graphics/src/nvn/`). Functions with CPU-visible meaning (device, memory pools, textures and samplers, queues,
syncs, events, command memory, windows) do what NVN defines; functions that only change GPU state (state objects,
bindings, draws, clears, copies) are recorded into the game's command memory and replayed on Vulkan at submit
([rendering.md](rendering.md)). GPU address = CPU address: a pool's storage address is its GPU address, as on the
console's unified memory.

| File | Covers |
|---|---|
| `src/graphics/src/nvn/types.rs`, `obj.rs` | public types at the SDK's sizes; host object layouts; the record format |
| `device.rs` | builder, initialization, device properties, handles, origin and depth modes |
| `pool.rs`, `vpool.rs` | memory pools, buffers, texture / sampler descriptor pools; virtual pools |
| `tex.rs`, `state.rs`, `query.rs` | textures, views, samplers; state objects; getters and setters that record nothing |
| `prog.rs`, `prog2.rs` | programs and shader keys (NVN, NVN2) |
| `cmd.rs`, `cmd2.rs`, `devbuf.rs` | command buffers and their records; NVN2 device command / message buffers |
| `queue.rs`, `sync.rs`, `window.rs` | submission, finish, present; syncs and events; swap chains and pacing |
| `native.rs`, `dlss.rs`, `nops.rs` | native images; NVN2 DLSS objects; entry points with nothing to do |
| `entry.rs`, `mod.rs` | AAPCS64 wrappers and the proc table (`nvn::procs()`) |

## Object sizes

Objects live in place, in the memory the game allocated, and never exceed the size the game's SDK reserves
(`fits!` assertions in `obj.rs`). Newer SDKs shrank NVNtexture to 0x80 and NVNtextureView to 16 bytes, and a game's
NVNprogram can be smaller than nvn.h's 0xC0. Some games read bytes directly, so these stay where the SDK keeps them:
NVNcommandBuffer +0x10 (inlined SDK code), NVNbuffer offset and size at +0x18 / +0x20, a sampler's state from byte 8.
Textures are found by their initialization serial, not by address, because games copy NVNtexture objects by value.

## Builder and state defaults

Builders' `SetDefaults` and the state objects (`state.rs`: blend, color, channel mask, multisample, polygon,
depth-stencil, vertex attribute and vertex stream states) are packed in place; the game copies them by value and binds them into command buffers.
Values a getter returns live in the object's own memory (`query.rs`).

## Device properties

`nvnDeviceGetInteger` answers items 0..0x68 as measured on the Tegra X1 (`device.rs`). On the Switch 2 the answers
come from the game's own sdk module where NVN2's driver switch has a constant case (`set_sdk_image`, parsing
`mov w0, #v; b` cases), else a table read from Donkey Kong Bananza's SDK 20.5.19 and SDK 21.4.4: 32 vertex bindings,
1,024 compute threads, 16 texture levels, items up to 0xA9. API and GPU code versions move with the SDK and games
check them. Ray-tracing items above 0x60 answer 0 with a red line.

Handles (`nvnDeviceGetTextureHandle` and friends): texture ID in bits 0-19, sampler ID from bit 20; image and texel
fetch handles are the ID. Counter timestamps: 614.4 MHz ticks on NVN, nanoseconds on NVN2
(`device::timestamp_from_ns`).

## Memory pools

`pool.rs` keeps the pool object; `src/graphics/src/vk/mem.rs` gives every pool a placement on the discrete GPU:

| Placement | When | Behaviour |
|---|---|---|
| Mirror | CPU-visible (default) | device-local copy; CPU writes found by the write watch (or a snapshot compare) and uploaded at the start of each submission; GPU writes copied back when the submission's fence completes, before any waiter sees it |
| Vram | `CPU_NO_ACCESS` | device-local, filled from the storage at first GPU use, then GPU-only |
| Host | `NXGFX_POOL=host` | guest pages imported (VK_EXT_external_memory_host): zero-copy, read over PCIe; falls back to Mirror |
| CpuOnly | `GPU_NO_ACCESS` | no GPU buffer |

Pages the GPU keeps writing (`STREAK` frames) are left on the device and made inaccessible; the first CPU access
faults them back (`src/graphics/src/vk/lazy.rs`, `NXGFX_WB_LAZY=0` off). Other switches: `NXGFX_POOL_COLD=0`
(check every pool at every submission), `NXGFX_WB_GUARD=0`, `NXGFX_COPY_SYNC=0`, `NXGFX_LAZY_CPU_CHECK=0`,
`NXGFX_POOL_LOG=1` (a line per pool with its placement).

### Virtual pools

`vpool.rs` (NVN `VIRTUAL`, flags 0x200). Each gets a reserved address range at Initialize. The first
`nvnMemoryPoolMapVirtual` request (0x28 bytes: physical pool, physical offset, virtual offset, size, storage class)
decides its kind:

- **Alias**: the whole pool from offset 0 onto one physical pool. The reservation is released and the pool's storage
  is that physical memory; later requests must keep the one base.
- **Sparse**: anything else, or a pool larger than the application pool. Requests go into a 4 KB page table; the CPU
  cannot address the range (Windows cannot map private pages twice), the GPU reads through the table, and a texture is
  uploaded from what the table points at (unmapped pages read 0).

Sparse textures in a sparse pool become Vulkan sparse images (`src/graphics/src/vk/sparse.rs`); see
[rendering.md](rendering.md#textures-and-formats).

### Descriptor pools

Texture and sampler pools register descriptors at once into the bindless pool set (`src/graphics/src/vk/tables.rs`,
update-after-bind); each pool gets a base in the host table (`dpool_base_of`), pushed to shaders as push constants.
As on the console, an ID the GPU still reads must not be rewritten by the game.

## Command memory

`cmd.rs`. Command memory holds only records; a recording's end, the links between its pieces and calls of other
recordings are entries in its control memory, so nothing is written past what `GetCommandMemoryUsed` reported (games
size their command memory by it). Compact records (`obj::opc`) keep common commands as small as the driver's; the
queue's flatten (`for_each_record`) expands them into canonical records. NVN2's device command buffers and message
buffers (shader printf records read back on the CPU) are `devbuf.rs`.

## Queues and syncs

- `nvnQueueSubmitCommands` hands recorded command handles to the back end; every NVN submission becomes one Vulkan
  submission whose fence is a timeline value (`src/graphics/src/vk/queue.rs`). By default the replay runs on its own
  thread (`NXGFX_ASYNC=0`: on the game's thread); fences taken before it ran are receipts that resolve once it did.
- A fenced sync names a point on that timeline; `nvnSyncWait` returns only once the GPU completed it **and** its
  CPU-visible effects landed (event values, counter reports, pool write-backs). A sync never fenced since Initialize
  is signalled (`sync.rs`).
- An event wait nothing satisfies parks that NVN queue (the rest of its stream is kept and replayed when a signal
  arrives); other queues go on.
- The first `nvnQueueSubmitCommands` is logged with the time since boot (`src/os/src/nvn/mod.rs`).

## CPU-written textures

A texture's storage is its content until the GPU writes it. CPU writes to the storage of a texture the GPU has not
written make it upload again (the pool's written pages tell `vk::image`). `nvnTextureWriteTexels` writes into the
storage, swizzled, so every image over that storage sees it (MK8D's preview panel, [movies.md](movies.md)). Render
targets, copy destinations, cleared and storage images are GPU-owned; their storage no longer counts. Linear render
targets are read back into guest memory at the fence (`NXGFX_RT_READBACK=0` off).

## Windows and present

`window.rs`: the swap chain textures the game renders into, acquire with present pacing, and present. Vsync blocks in
the acquire, not in present (a game often presents while holding a lock). The present interval and forced intervals
are in [frame-rate.md](frame-rate.md#present-interval-and-display-rate).

## NVN entry points

`nvnBootstrapLoader` and `nvnDeviceGetProcAddress` hand out host functions by name (`src/os/src/nvn/mod.rs`,
`bind_graphics`). A name graphics does not implement gets a logging stub that answers 0, records `UNIMPL Nvn <name>`
on its first call and counts calls; `report` prints the table. `NXOS_NVN_BT=<proc>` logs the arguments and caller of
that proc's first 64 calls. Native-image entry points are imported by name and bound separately (`native.rs`).
Debug labels, ZCull, tiled-cache hints and fast-clear registration do nothing (`nops.rs`).

## Problems found on games

### A texture released while queued work reads it

- **Symptom:** Mario Kart 8 Deluxe shows black stretches with the asynchronous replay.
- **Cause:** `nvnTextureFinalize` released the scene target on the game's thread while that frame's queued work still
  read it through pool slots.
- **Fix:** the release runs in queue order (`NXGFX_RELEASE_ORDER=0` restores the old behaviour; PORT_PLAN G-lane, G5).

### Window acquire waits

- **Symptom:** Mario Kart World stalls 31-40 s into a run.
- **Fix:** the window acquire's sync is satisfied at once, as mkw-recomp does (PORT_PLAN M7).

## Known limits

- NVN2 ray-tracing command entry points (`nvnCommandBufferBuildAccelerationStructure`, `...DispatchRays`) are not
  implemented: their argument layouts are unknown (PORT_PLAN 5A.4 "Ray tracing", Blocked).
- `SetConservativeRasterEnable` and a few NVX procs are tracked as Partial in PORT_PLAN 5A.4 ("NVN procs").
- Recording on several threads is partial (`NXGFX_DEFER=1`, off by default; PORT_PLAN 5A.4).
