# Threads

Guest threads, their synchronization, and how the engine schedules them. The kernel is
`src/runtime/src/kernel.rs` (threads, locks, condition variables, address arbitration, events, waits); the virtual
cores that decide which guest thread may run are `src/os/src/sched.rs`; fibers are `src/runtime/src/fiber.rs`.

## Thread creation and cores

Every guest thread is one host thread running lifted code on the guest stack the caller gave
(`Kernel::create_thread`, `start_thread`; the main thread `create_main`). Its `Cpu` holds the guest registers; its
TLS region (TPIDRRO, 0x200 bytes) is allocated in the kernel region, and the thread id is also written at TLS+0x110
for SDK 22.

| SVC | Meaning here |
|---|---|
| 0x08 / 0x09 / 0x0A | CreateThread, StartThread, ExitThread |
| 0x0B | SleepThread (a positive time sleeps, anything else yields the host thread) |
| 0x0C / 0x0D, 0x0E / 0x0F | priority, core mask; changes reach the scheduler |
| 0x10 | GetCurrentProcessorNumber: the virtual core the thread holds |
| 0x25 | GetThreadId |

Horizon runs at most one thread per core, the best ready one, and games rely on it: a higher-priority thread started
on the caller's core runs before the caller goes on, and per-core data is indexed by core number without locks. The
virtual cores (`sched::Cores`, an `nxrt::kernel::Scheduler`) reproduce that on free host threads:

- The core count comes from the NPDM's core mask (3 on Switch 1; a Switch 2 program has 6; at most 8). MKW crashed
  at 15.5 s while the mask was assumed 0x7 instead of the NPDM's 0x3f (PORT_PLAN M7).
- A thread takes a core when it starts and gives it back where Horizon would switch: blocking SVCs (waits, locks,
  condition variables, address waits, sleep), yields, ExitThread, and host functions that block
  (`nxos::sched::without_core`).
- A free core goes to the best waiting thread allowed there (priority, then first come); a thread prefers its ideal
  core and migrates within its affinity mask.
- Preemption at checkpoints: after every SVC the OS layer answers (`src/os/src/svc.rs`) and at every pass of a marked
  spin-wait loop (`rt_spin_gate` -> `Scheduler::spin_pass`). A holder a better thread waits for hands its core over.
- A waiter that has waited 2 ms for a core held by a worse thread, or 20 ms for one held by an equal or better thread
  (a holder spinning in code with no checkpoint), takes the core; the holder gives up its claim at its next
  checkpoint. Counters `STEALS`, `STEALS_ANY`, `STEALS_GUEST` record how often.

Log line at start: `scheduler: <n> virtual cores, the best ready thread on each; checkpoints at SVCs and spin waits`.

Switches:

| Switch | Effect |
|---|---|
| `NXOS_FREE_CORES=1` | no gate: every thread runs whenever it is ready |
| `NXOS_CORES=<n>` | fewer virtual cores than the mask (1 = every guest thread in turn) |
| `NXOS_STEAL_WORSE_MS`, `NXOS_STEAL_ANY_MS` | the two steal delays; `NXOS_CORES=1` with `NXOS_STEAL_ANY_MS=0` means no two guest threads ever run at once |
| `NXOS_HOST_CPUS=<n>` | limits the process to n host CPUs (`src/os/src/boot.rs`) |

### Core count from the NPDM

- **Symptom:** Mario Kart World crashed 15.5 s into a run.
- **Cause:** the core mask was assumed 0x7 (three cores) instead of the NPDM's 0x3f.
- **Fix:** the virtual cores follow the NPDM's mask (`src/os/src/profile.rs` `npdm_core_mask`).

## Synchronization and address arbitration

Semantics follow the public Horizon kernel (KConditionVariable, KAddressArbiter, KSynchronization). All waits hang on
one kernel lock with one condition variable per thread; waiters are picked in kernel order (numerically lower
priority first, then first come).

| SVC | Notes |
|---|---|
| 0x1A / 0x1B ArbitrateLock / Unlock | tag word = owner handle, `0x40000000` = has waiters |
| 0x1C / 0x1D WaitProcessWideKeyAtomic / SignalProcessWideKey | condition variables |
| 0x34 / 0x35 WaitForAddress / SignalToAddress | wait if less than, decrement and wait if less than, wait if equal, and WaitIfEqual64 (type 3, SDK 22) |
| 0x18 WaitSynchronization, 0x19 CancelSynchronization | over events and threads |

**Stuck locks.** A lock can be left with a stale owner, or with waiters whose waiters bit was cleared (the owner then
unlocks in user mode without calling the kernel). `Kernel::heal_mutexes` (Turbo's `kernel_heal_mutexes`) runs each
frame from the front end (`src/front/src/lib.rs`): a stale lock goes to its first waiter, a lost bit is set again.
Log: `kernel: mutex <addr> repaired (<what>, value <v>) [<n> times]` (the first 20, then every 1024th). A steady stream
of these points at a real ordering problem, not at the healer.

**Memory ordering.** x86 lets a later load pass an earlier store. Code that relies on a load-acquire to order a
preceding plain store can lose a wake-up; the known case (Game Freak's Trinity rendezvous) gets a fence from the build
([rewriter.md](rewriter.md#exclusives-and-atomics)). Exclusive pairs use the per-thread monitor, plus the global
bucket table on Switch 1 titles.

## Yields and spin waits

- `SleepThread` is a blocking SVC: the thread gives its virtual core up around it (`block_enter` / `block_leave`)
  and queues again for one afterwards; a time of 0 or less is a host yield.
- Spin-wait loops in game code have no SVC, so a spinning thread would hold its core forever. The profile's patch
  table marks their back edges (`spin <module> <head> <back-edge>`); the generated code calls `rt_spin_gate` there
  (`src/runtime/src/rt.rs`): the scheduler's checkpoint, then the host core is given away. MKW's table marks 97 spin
  waits (PORT_PLAN M7).
- A spin with no mark is caught only by the 20 ms steal above; a game that stutters in a busy wait is a candidate for a
  new `spin` row.

## Events and notifications

- Kernel events: `Kernel::{create_event, signal_event, clear_event, event_signaled, close_handle}`; SVCs 0x45
  CreateEvent, 0x11 SignalEvent, 0x12 ClearEvent, 0x17 ResetSignal, 0x16 CloseHandle.
- Host-signalled events the OS layer hands to the guest (a service's system event, the read end copied to the client):
  `src/os/src/kobj.rs`. Guests wait on them with WaitSynchronization like any other.
- An SDK `nn::os::SystemEventType` a host service signals (oe notifications, hid style-set events, async completions)
  is bound on first use to a kernel event the SDK creates itself, run as guest code on the calling thread
  (`nxos::services::os::bind_system_event`, `signal_system_event`); a CloseHandle of the write end ends the binding.
- vi's vsync event is signalled by a host thread at 60 Hz on the guest clock ([frame-rate.md](frame-rate.md)).

## Fibers

The SDK switches fibers by swapping register contexts, which breaks "RET is a C return" in lifted code. Each guest
fiber gets a host fiber (its own reserved stack) and its own `Cpu` (`src/runtime/src/fiber.rs`); a switch carries
TPIDR, TPIDRRO, FPCR / FPSR over and clears the exclusive monitor. A finished fiber's stack holds no Rust frame
(`nxrt_fiber_entry`), and paused fibers' stacks are freed (PORT_PLAN M2).

## Frame threads

The replay thread (`vk replay`) and the game thread that acquires window textures join the Multimedia Class
Scheduler's "Games" task, the Windows counterpart of nxnative's time-constraint policy; their measured need per frame
is logged when it moves by more than 15% (`src/graphics/src/vk/rtclass.rs`; `NXGFX_RT_THREAD=0` leaves them normal).

## Watching a run

- Heartbeat every `NXOS_HEARTBEAT` seconds (default 10, 0 none) with the guest's progress (`src/os/src/watch.rs`).
- Stall dumps: once presents stop after the first image, a thread dump after 6 s, then every 15 s, at most 4 in a run;
  a new image resets the count. Each dump is `nxrt::kernel::dump_threads`: handle, priority, core, what the thread
  waits for (lock, condition variable, address, handles) and its guest frames (x30, then the x29 chain) as of its last
  SVC.
- Control pipe `threads` prints the same any time; F5 writes it with the diagnostic dump.
- `NXRT_TRACE=thread` (or `svc`, `sjlj`, `all`) logs kernel activity; `NXOS_TRACE=svc` the OS layer's SVCs.
- `NXRS_DUMP_FRAMES=<ret>[,...]` logs the 16 words at the frame record of a frame returning to that address.
- `NXOS_EXIT_AFTER=<s>` / `NXOS_EXIT_AFTER_SUBMIT=<s>` end a measured run with the gap summary.
- `profile=1` (ini) samples guest threads ~1000 times a second into `profile.txt` every 10 s
  (`src/runtime/src/profile.rs`); `SWR_HOST_PROF=<start>,<seconds>[,<file>]` samples one host thread
  (`SWR_HOST_PROF_THREAD`, default `vk replay`) in Turbo's `host_prof.py` format (`src/runtime/src/hostprof.rs`).

## Known limits

- The kernel's own timeouts run on host time, not on the guest clock that `smooth_recording` slows
  (INTERFACES "os", lane I's request 2).
- nxnative's Apple scheduler-field publication and QoS classes have no counterpart; MMCSS above is the Windows
  equivalent. PORT_PLAN 5A.4 ("GPU deswizzle, built-in counter kernels, real-time thread class") tracks it as Partial.
