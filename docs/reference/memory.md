# Memory and address space

Guest addresses are host addresses: the game's code, heap, stacks and TLS live at the same numbers in the engine's
process, all below 2^39, so lifted code loads and stores with no translation. The runtime owns the address space
(`src/runtime/src/mem.rs`, Linux `src/runtime/src/linux.rs`); the OS layer serves the memory SVCs on top of it
(`src/os/src/mem.rs`); the graphics layer places NVN memory pools on the discrete GPU (`src/graphics/src/vk/mem.rs`,
see [nvn-objects.md](nvn-objects.md)).

## Guest layout

Fixed part (`src/runtime/src/mem.rs`):

| Region | Where | Size | Notes |
|---|---|---|---|
| Modules | from `0x70_0000_0000` | per module, 2 MiB aligned | the translator's `layout.json` / `layout.h`; NROs after the ExeFS ([rewriter.md](rewriter.md)) |
| Kernel region | `KERNEL_BASE` = `0x7F_0000_0000` | 256 MiB | TLS regions (TPIDRRO) and host objects the guest reads |
| Host functions | `hostfn::HOSTFN_BASE` | 1 MiB | never accessible; a call there is a host function (`src/runtime/src/hostfn.rs`) |

Placed at start-up, searched from 4 GiB up:

| Region | Size | Notes |
|---|---|---|
| Stack + heap block | 12 GiB, write-watched | stack region at +256 MiB, heap after it; heap region `HEAP_CAP` = 10 GiB, room for a Switch 2 pool |
| Alias region | 64 GiB, 4 GiB aligned, write-watched | address space only; physical memory is mapped into it on request |

Pages are committed in 4 KiB units; decommitted pages read back as zero. The write watch (`MEM_WRITE_WATCH` +
`GetWriteWatch`, `mem::written_pages`) is what the GPU side uses to find CPU writes to pool memory. On Linux the same
map is `PROT_NONE` `MAP_NORESERVE` reservations at fixed addresses, a byte per guest page records committed state,
and the write watch is a userfaultfd in asynchronous write-protect mode with `PAGEMAP_SCAN` (Linux 6.7); without it
`written_pages` answers None and callers compare snapshots.

`mem::describe_taken` names whoever holds an address when a fixed reservation fails at start.

## Heap and application pool

- **Pool.** `nxrt::process::app_pool` (`src/runtime/src/process.rs`) decides what GetInfo TotalMemorySize (ids 6 / 21)
  reports. ini `memory_gb`: 0 = automatic (Switch 1: the console's 3285 MiB; Tears of the Kingdom 5.2 GB; Switch 2:
  9 GB), 4 = console, 6 / 8 = yuzu's extended layouts (5.2 / 7.2 GB). A value above 4 is logged with a warning.
- **SetHeapSize** (`os::mem::Memory::set_heap_size`): 2 MiB steps; larger than the heap region fails
  ResultOutOfMemory; larger than the pool less memory in use fails ResultLimitReached (`heap_cap`), unless the hidden
  ini key `heap_max_mb` allows more. Growing only moves the end: `heap <n> GB (lazily committed; ...)`.
- **Lazy commit.** A heap page is committed on its first touch, by the guest (a vectored exception handler that runs
  before the crash reporter, `install_fault_handler`) or by host code before an OS call writes guest memory
  (`os::mem::prepare`, runtime `Space::ensure_committed`). `NXRT_LAZY_HEAP=1` turns on the runtime's own variant for
  builds without the OS layer.
- **GetInfo** (`Memory::info`): alias base / size (2, 3), heap base / size (4, 5), pool (6, 21), used = heap +
  physical maps + memory beside the heap (7, 22), ASLR range (12, 13), stack region (14, 15), system resource (16, 17).
  The figures come from the title's profile (`src/os/src/profile.rs`: `<data>\profile.tsv`, else the built-in table,
  else Switch 1 defaults; the title is identified by the program id in `main.npdm`).

## Alias region and physical memory

`MapPhysicalMemory` / `UnmapPhysicalMemory` commit and release pages of the alias region, counted against the pool
with the heap (`Memory::physical`, runtime `Space::map_physical`). With the map limit on, a map past the pool fails
with ResultLimitReached as on Horizon. The limit is on for Pokémon Legends: Z-A and Ocarina of Time, and for every
profile with nxnative's `mem_map_limit on` (Scarlet / Violet probe all allocatable memory at start and give cached
pages back only after a map fails); `SWR_MAP_LIMIT=1/0` forces it. Without the limit those games take everything.

The start-up log line says what was chosen:
`program <id>: system tick 19.20 MHz; exclusive monitor on ...; maps past the pool fail (ResultLimitReached)`.

## Mapping SVCs

| SVC | Behaviour here |
|---|---|
| SetHeapSize | above |
| MapMemory / UnmapMemory | copy, as one address space allows; the source stays untouched while mapped (on Horizon it is inaccessible). The SDK uses it to move a thread's heap stack into the stack region |
| MapPhysicalMemory / Unmap | alias region commit / release, pool-counted |
| SetMemoryPermission / SetMemoryAttribute / SetGpuMemoryAttribute | `Memory::set_permission`, `set_attribute`: the bits the game names are recorded, nothing is refused (MKW and DKB abort on a refusal); SetGpuMemoryAttribute takes only bit 0x20 |
| Shared memory | a host service's shared memory is backed at the address the guest maps it to (`map_shared`), and the service writes there (time, hid, applet storage) |
| Transfer memory | stays where it is; the only receivers are host services |
| QueryMemory | the OS layer's regions over the runtime's module, stack and TLS regions, with Horizon's states (`ST_CODE`, `ST_NORMAL`, `ST_STACK`, ...) |

Not modelled (`src/runtime/src/mem.rs` header): frozen regions, read traps for GPU-written pages (the graphics layer
has its own, below), real stack moves.

## Engine allocators

- **64 KB alignment.** Some engines cut their GPU pools at 64 KB alignment relative to the pool's GPU address, which is
  a big page on the console and a 4 KB page here. The profile setting `alloc_big_align` (`NXOS_ALLOC_BIG_ALIGN=1`)
  makes `nn::mem::StandardAllocator::Allocate` align blocks of 64 KB or more to 64 KB (`src/os/src/services/os.rs`).
- **Environment-sized runtimes.** Horizon gives a program no environment; a .NET GC sizes its region range from
  environment variables otherwise. The profile's `game_env` (`NXOS_GAME_ENV`, `KEY=VALUE` words) answers `getenv`
  for its names; other names go to the SDK's own getenv.
- **Mario Kart World's render-buffer pool** is resized by its game layer (`src/os/src/gamefix/mkw.rs`:
  `NXRS_MKW_RB_EXTRA_MIB`, `NXRS_MKW_SCENE_LINES`, `NXRS_MKW_POOL_MODE`).

## GPU-visible memory

NVN memory pools are guest memory; the GPU sees each through a placement chosen in `vk::mem`
([nvn-objects.md](nvn-objects.md#memory-pools)): a device-local mirror with CPU writes found by the write watch and
GPU writes copied back at the fence (default), device-only for `CPU_NO_ACCESS`, imported host pages with
`NXGFX_POOL=host`, or nothing for `GPU_NO_ACCESS`. GPU-written pages the CPU does not touch stay on the device and are
made `PAGE_NOACCESS`; the first CPU access faults and reads them back (`src/graphics/src/vk/lazy.rs`,
`NXGFX_WB_LAZY=0` off). Such a fault inside a host function is normal and not a crash.

## Running out of memory

- **Process cap.** `nxrt::process::memory_cap` puts the process into a job object limiting committed memory to half of
  RAM, between 12 and 32 GB (never below the pool + 8 GB); `SWR_MEMORY_CAP_GB` sets it, 0 turns it off. Past it,
  allocations fail in this process only and it stops with a reason in its log, rather than taking Windows down (Ocarina
  of Time grew 1 GB a second in driver allocations on 2026-11-18). Log: `process memory cap: <n> MB of committed
  memory ...`. On Linux: a cgroup v2 `memory.max` when delegated, else a watchdog on the resident size.
- **Vulkan host allocations.** `NXGFX_HOST_MB` caps host-memory allocations the graphics layer makes (0 = no cap);
  over it the allocation is refused and its caller's fallback applies.
- **VRAM.** Once a frame `vram_budget` (`src/graphics/src/vk/image.rs`) checks the device-local heaps against
  their budget (VK_EXT_memory_budget); over 90% every parked image (render targets and textures kept for a
  re-initialization the game has let go) is freed. `NXGFX_VRAM_EVICT=0` off, `NXGFX_VRAM_EVICT_PCT` the threshold.
- **Harness watchdog.** `scripts/play_scenario.py` and `scripts/library_play.py` kill a run by PID at a memory
  threshold; a "memory watchdog at 20 GB" in a run note is that, not the engine.

## Problems found on games

### Alias region too small

- **Symptom:** Pokémon Legends: Z-A aborts in the SDK (nn::diag) during `nninitStartup`.
- **Cause:** it puts a StandardAllocator over 12 GiB of the alias region; a 4 GiB region refused it.
- **Fix:** the alias region is Horizon's 64 GiB of address space (`ALIAS_SIZE`).

### Memory attributes refused

- **Symptom:** Mario Kart World aborts at main's DT_INIT with 0xE801; Donkey Kong Bananza's SDK aborts at 0.9 s.
- **Cause:** SetMemoryAttribute's PermissionLocked bit (the RELRO lock) and other bits were refused.
- **Fix:** every bit is recorded, nothing refused (`Memory::set_attribute`).

### Driver allocations take the machine down

- **Symptom:** Windows runs out of memory while a game compiles pipelines (Ocarina of Time, 1 GB a second).
- **Cause:** the video driver's own allocations in the game's process, with no limit.
- **Fix:** the process memory cap (above): the game stops with a reason instead.

## Debugging memory problems

- `null_trap=1` in the ini only logs: on Windows nothing is mapped below 64 KB, so a NULL write already faults with a
  guest report (`src/runtime/src/fault.rs`).
- Control-pipe commands (`src/os/src/watch.rs`): `memdump <addr> <bytes> <file>` writes guest memory to a file,
  `addr <mangled name>` gives a symbol's guest address, `wpage <page>` / `wtrace <from> <to>` log each write to a
  guarded page or range with the guest function that made it, at most 600 lines (`src/os/src/wwatch.rs`).
- `NXGFX_WATCH_ADDR=<hex>` logs images made or released over a guest address; `NXGFX_POOL_LOG=1` prints each pool's
  placement.
- Crash reports map host addresses back to the guest function and walk the guest frame chain; `NXRT_FATAL_PEEK`
  dumps memory along register chains, `NXRT_MINIDUMP=0|full` picks the minidump.

nxnative's ASan registration-table layouts and its Apple address-space limits (the reason for its observation gates)
have no counterpart here.
