//! svc dispatch: the handler registry and the gap fallback (nxrt's svc.rs discipline, over the
//! `Host::svc` callback), plus the kernel table this wave implements. Named services answer
//! through `SendSyncRequest` (0x32) and `ConnectToPort` (0x2D): both gap-refuse here with the
//! kernel's stubbed-svc result and one logged line per site — os3ds's service lanes (W3) take
//! over from the same registry.

use crate::context::{self, Context};
use crate::hostfast;
use crate::mem::{Kind, Perm, State};
use crate::thread::{ThreadState, Waiting};
use crate::Process3ds;
use std::sync::atomic::{AtomicUsize, Ordering};

/// what the real kernel returns for the stubbed svcs (3dbrew `SVC`, "stubbed")
pub const R_STUB: u32 = 0xF8C0_07F4;
/// the kernel's timeout result (3dbrew `SVC` §Kernel error-codes)
pub const R_TIMEOUT: u32 = 0x0940_1BFE;
/// invalid combination (3dbrew, BindInterrupt notes) — the generic argument refusal
pub const R_INVALID: u32 = 0xD8E0_07EE;
/// the success result
pub const R_OK: u32 = 0;

/// a handler an os layer installs ahead of the kernel table (os3ds, W3)
pub type SvcHandler = fn(&Process3ds, &mut Context, u32) -> bool;

static HANDLERS: [AtomicUsize; 256] = [const { AtomicUsize::new(0) }; 256];
static FALLBACK: AtomicUsize = AtomicUsize::new(0);

/// routes svc `number` to `f` before the kernel table; a later call replaces it
pub fn set_handler(number: u8, f: SvcHandler) {
    HANDLERS[number as usize].store(f as usize, Ordering::Release);
}

/// runs for every svc neither a handler nor the kernel table answers
pub fn set_fallback(f: SvcHandler) {
    FALLBACK.store(f as usize, Ordering::Release);
}

fn handler_of(slot: &AtomicUsize) -> Option<SvcHandler> {
    match slot.load(Ordering::Acquire) {
        0 => None,
        // safe: only set_handler / set_fallback store here, always an SvcHandler
        p => Some(unsafe { core::mem::transmute::<usize, SvcHandler>(p) }),
    }
}

/// the Host::svc callback the generated code and the interpreter both reach
pub(crate) unsafe extern "C" fn svc_callback(ctx: *mut Context, number: u32) {
    let state = unsafe { ThreadState::of(ctx) };
    let process_ptr = state.process;
    // safe: the process outlives the boot, the pointer is a copy
    let process: &Process3ds = unsafe { &*process_ptr };
    process.svc_exec(state, number);
}

/// the tick window a slice opens lives in run.rs; the claim itself is the process's

/// the svc entry: registry first, then the kernel table, then the gap line
pub fn dispatch(process: &Process3ds, state: &mut ThreadState, number: u32) {
    process.stats.svcs.fetch_add(1, Ordering::Relaxed);
    let slot = HANDLERS.get((number & 0xFF) as usize).map(handler_of).flatten();
    if let Some(f) = slot {
        if f(process, &mut state.ctx, number) {
            return;
        }
    }
    kernel(process, state, number);
}

/// the kernel table (3dbrew SVC + Kernel ABI, the pinned forms in docs/waves/3DS-W2.md)
fn kernel(process: &Process3ds, state: &mut ThreadState, number: u32) {
    let ctx = &mut state.ctx;
    let me = state.thread;
    match number {
        // ControlMemory: r0 op, r1 addr0, r2 addr1, r3 size, r4 perms -> r0 result, r1 addr
        0x01 => {
            let (op, addr0, _addr1, size, _perms) = (ctx.r[0], ctx.r[1], ctx.r[2], ctx.r[3], ctx.r[4]);
            let operation = op & 0xFF;
            match operation {
                // COMMIT / COMMIT|LINEAR / FREE
                3 if op & 0x10000 == 0 => {
                    ctx.r[1] = addr0;
                    ctx.r[0] = map_result(process, addr0, size, Perm::Rw, State::Private, Kind::Heap);
                }
                3 => {
                    // LINEAR: the 0x14000000 window, phys = vaddr - 0x0C000000 on this host
                    ctx.r[1] = addr0;
                    ctx.r[0] = map_result(process, addr0, size, Perm::Rw, State::Continuous, Kind::Linear);
                }
                1 => {
                    let freed = process.memory.unmap(addr0, size).is_ok();
                    ctx.r[1] = addr0;
                    ctx.r[0] = if freed { R_OK } else { R_INVALID };
                }
                _ => {
                    process.gap(&format!("svc {number:#04x} ControlMemory op {op:#x} at site {:#x}", ctx.r[15]));
                    ctx.r[0] = R_STUB;
                }
            }
        }
        // QueryMemory: r0 MemoryInfo*, r1 PageInfo*, r2 addr
        0x02 => {
            let info = process.memory.query(ctx.r[2]);
            let (info_at, page_at) = (ctx.r[0], ctx.r[1]);
            process.memory.write32(info_at, info.base);
            process.memory.write32(info_at.wrapping_add(4), info.size);
            process.memory.write32(info_at.wrapping_add(8), match info.perm {
                Perm::None => 0,
                Perm::R => 1,
                Perm::Rw => 3,
            });
            process.memory.write32(info_at.wrapping_add(12), state_of(info.state));
            process.memory.write32(page_at, 0);
            ctx.r[0] = R_OK;
        }
        // ExitProcess: the boot stops cleanly
        0x03 => {
            process.stop_boot(format!("the guest called ExitProcess at {:#x}", ctx.r[15]));
            ctx.exit = context::EXIT_UNWIND;
        }
        // CreateThread: r0 priority, r1 entry, r2 arg, r3 stack_top, r4 processor -> r1 handle
        0x08 => {
            match process.spawn_guest(ctx.r[1], ctx.r[2], ctx.r[3], ctx.r[0] as i32) {
                Some(handle) => {
                    ctx.r[1] = handle;
                    ctx.r[0] = R_OK;
                }
                None => ctx.r[0] = R_STUB,
            }
        }
        // ExitThread
        0x09 => {
            process.exit_thread(me, ctx.r[15]);
            ctx.exit = context::EXIT_UNWIND;
        }
        // SleepThread: r0-r1 nanoseconds
        0x0A => {
            let ns = (ctx.r[0] as i64) | ((ctx.r[1] as i64) << 32);
            if ns < 0 {
                // a negative sleep waits until something signals the thread (yield forever)
                process.park_thread(me, Waiting::Sleep { until_signaled: true }, None);
            } else {
                process.park_thread(me, Waiting::Sleep { until_signaled: false }, Some(ns as u64));
            }
            ctx.exit = context::EXIT_UNWIND;
        }
        // GetThreadPriority: r1 handle -> r1 priority
        0x0B => {
            let handle = ctx.r[1];
            match process.priority_of(handle) {
                Some(p) => {
                    ctx.r[1] = p as u32;
                    ctx.r[0] = R_OK;
                }
                None => ctx.r[0] = R_STUB,
            }
        }
        // SetThreadPriority: r0 handle, r1 priority
        0x0C => {
            let (handle, priority) = (ctx.r[0], ctx.r[1] as i32);
            ctx.r[0] = if process.set_priority(handle, priority) { R_OK } else { R_STUB };
        }
        // Get/SetThreadIdealProcessor: answered with core 0
        0x0F => {
            ctx.r[1] = 0;
            ctx.r[0] = R_OK;
        }
        0x10 => ctx.r[0] = R_OK,
        // GetCurrentProcessorNumber
        0x11 => ctx.r[0] = 0,
        // CreateMutex: r0 Handle* out, r1 initialLocked -> r0 result
        0x13 => {
            let out = ctx.r[0];
            let initial = ctx.r[1] != 0;
            let handle = process.create_mutex(initial, me);
            process.memory.write32(out, handle);
            ctx.r[0] = R_OK;
        }
        // ReleaseMutex: r0 handle -> r0 result (the donor's in-place release site)
        0x14 => {
            let handle = ctx.r[0];
            let released = process.release_mutex(handle, me);
            if !released {
                // the code answered it itself under the loan; the settle already applied holding
                let lent = ctx.held == handle && ctx.holding > 0;
                if lent {
                    ctx.r[0] = R_OK;
                } else {
                    ctx.r[0] = R_INVALID;
                }
            } else {
                ctx.r[0] = R_OK;
            }
            learn_release_site(ctx, handle);
        }
        // CreateSemaphore: r0 Handle*, r1 initial, r2 max
        0x15 => {
            let out = ctx.r[0];
            let handle = process.create_semaphore(ctx.r[1], ctx.r[2]);
            process.memory.write32(out, handle);
            ctx.r[0] = R_OK;
        }
        // ReleaseSemaphore: r0 count*, r1 handle, r2 releaseCount -> r1 old count
        0x16 => {
            let (out, handle, count) = (ctx.r[0], ctx.r[1], ctx.r[2]);
            let old = process.release_semaphore(handle, count);
            if let Some(old) = old {
                process.memory.write32(out, old);
                ctx.r[1] = old;
                ctx.r[0] = R_OK;
            } else {
                ctx.r[0] = R_STUB;
            }
        }
        // CreateEvent: r0 Handle*, r1 resettype
        0x17 => {
            let out = ctx.r[0];
            let handle = process.create_event(ctx.r[1]);
            process.memory.write32(out, handle);
            ctx.r[0] = R_OK;
        }
        // SignalEvent / ClearEvent: r0 handle
        0x18 => {
            ctx.r[0] = if process.signal_event(ctx.r[0]) { R_OK } else { R_STUB };
        }
        0x19 => {
            ctx.r[0] = if process.clear_event(ctx.r[0]) { R_OK } else { R_STUB };
        }
        // timers: the table answers them as never-due objects with a clock that sleeps
        0x1A => {
            let out = ctx.r[0];
            let handle = process.create_timer(ctx.r[1]);
            process.memory.write32(out, handle);
            ctx.r[0] = R_OK;
        }
        0x1B | 0x1C | 0x1D => {
            process.gap(&format!("svc {number:#04x} timer op at {:#x}", ctx.r[15]));
            ctx.r[0] = R_STUB;
        }
        // CreateMemoryBlock / MapMemoryBlock / UnmapMemoryBlock: shared memory blocks
        0x1E => {
            let out = ctx.r[0];
            let handle = process.create_memory_block(ctx.r[1], ctx.r[2]);
            process.memory.write32(out, handle);
            ctx.r[0] = R_OK;
        }
        0x1F | 0x20 => {
            process.gap(&format!("svc {number:#04x} MapMemoryBlock at {:#x}", ctx.r[15]));
            ctx.r[0] = R_STUB;
        }
        // CreateAddressArbiter: r0 Handle*
        0x21 => {
            let out = ctx.r[0];
            let handle = process.create_arbiter();
            process.memory.write32(out, handle);
            ctx.r[0] = R_OK;
        }
        // ArbitrateAddress: r0 handle, r1 addr, r2 type, r3 value, r4-r5 ns
        0x22 => {
            let (handle, address, kind, value) = (ctx.r[0], ctx.r[1], ctx.r[2], ctx.r[3]);
            let ns = (ctx.r[4] as u64) | ((ctx.r[5] as u64) << 32);
            match kind {
                // SIGNAL
                0 => {
                    process.arbitrate_signal(handle, address, value as u32);
                    ctx.r[0] = R_OK;
                }
                // WAIT_IF_LESS_THAN / DECREMENT_AND_WAIT / with-timeout forms
                1..=5 => {
                    let timeout = if kind & 4 != 0 { Some(ns) } else { None };
                    let decrement = kind & 1 != 0 && kind & 4 == 0 || kind == 5;
                    process.park_arbiter(me, handle, address, value, decrement, timeout);
                    ctx.exit = context::EXIT_UNWIND;
                }
                _ => {
                    ctx.r[0] = R_INVALID;
                }
            }
        }
        // CloseHandle
        0x23 => {
            ctx.r[0] = if process.close_handle(ctx.r[0]) { R_OK } else { R_STUB };
        }
        // WaitSynchronization1: r0 handle, r2-r3 timeout
        0x24 => {
            let ns = (ctx.r[2] as u64) | ((ctx.r[3] as u64) << 32);
            let parked = process.park_wait(me, &[ctx.r[0]], ctx, ns);
            if parked {
                ctx.exit = context::EXIT_UNWIND;
            }
        }
        // WaitSynchronizationN: r0 s32* out, r1 handles*, r2 count, r3 waitAll, r4-r5 timeout
        0x25 => {
            let (handles_at, count, _wait_all) = (ctx.r[1], ctx.r[2], ctx.r[3]);
            let ns = (ctx.r[4] as u64) | ((ctx.r[5] as u64) << 32);
            let mut handles = [0u32; 8];
            let take = count.min(8) as usize;
            for (i, handle) in handles.iter_mut().take(take).enumerate() {
                *handle = process.memory.read32(handles_at.wrapping_add(4 * i as u32));
            }
            // the lent mutex alone: the code answers it itself once the site is known
            if take == 1 && ctx.wait_at != 0 && ctx.held != 0 && handles[0] == ctx.held {
                ctx.holding += 1;
                ctx.r[0] = R_OK;
                ctx.r[1] = 0;
                return;
            }
            let parked = process.park_wait(me, &handles[..take], ctx, ns);
            if parked {
                ctx.exit = context::EXIT_UNWIND;
            }
        }
        // DuplicateHandle: r0 out*, r1 handle
        0x27 => {
            let out = ctx.r[0];
            let copy = process.duplicate_handle(ctx.r[1]);
            if let Some(copy) = copy {
                process.memory.write32(out, copy);
                ctx.r[0] = R_OK;
            } else {
                ctx.r[0] = R_STUB;
            }
        }
        // GetSystemTick: r0-r1 tick (the donor's in-place tick site)
        0x28 => {
            let tick = hostfast::tick_now(ctx);
            ctx.r[0] = tick as u32;
            ctx.r[1] = (tick >> 32) as u32;
            ctx.ticks += 1;
            hostfast::learn_tick_site(ctx, ctx.r[15], ctx.thumb != 0);
        }
        // GetSystemInfo: r0 s64* out, r1 type, r2 param
        0x2A => {
            let (out, kind, param) = (ctx.r[0], ctx.r[1], ctx.r[2]);
            let value = match (kind, param) {
                // 0 param: used memory of the region (0: all, 1: application, ...)
                (0, 1) => process.used_application_bytes() as u64,
                (0, _) => 0,
                _ => {
                    process.gap(&format!("svc 0x2A GetSystemInfo type {kind} param {param}"));
                    ctx.r[0] = R_STUB;
                    return;
                }
            };
            process.memory.write32(out, value as u32);
            process.memory.write32(out.wrapping_add(4), (value >> 32) as u32);
            ctx.r[0] = R_OK;
        }
        // GetProcessInfo / GetThreadInfo / GetHandleInfo: answered with zeros
        0x29 | 0x2B | 0x2C => {
            let out = ctx.r[0];
            process.memory.write32(out, 0);
            process.memory.write32(out.wrapping_add(4), 0);
            ctx.r[0] = R_OK;
        }
        // ConnectToPort: named services are W3's
        0x2D => {
            let name_at = ctx.r[1];
            let mut name = [0u8; 12];
            for (i, byte) in name.iter_mut().enumerate() {
                *byte = process.memory.read8(name_at.wrapping_add(i as u32));
                if *byte == 0 {
                    break;
                }
            }
            let name = core::str::from_utf8(&name).unwrap_or("?").trim_end_matches('\0');
            process.gap(&format!("svc 0x2D ConnectToPort({name:?}) at {:#x}", ctx.r[15]));
            ctx.r[0] = R_STUB;
        }
        // SendSyncRequest: the named-service lane
        0x32 => {
            let session = ctx.r[0];
            process.gap(&format!("svc 0x32 SendSyncRequest(session {session:#x}) at {:#x}", ctx.r[15]));
            ctx.r[0] = R_STUB;
        }
        // GetThreadId: r0 out*, r1 handle
        0x37 => {
            let out = ctx.r[0];
            let id = process.thread_id_of(ctx.r[1]).unwrap_or(0);
            process.memory.write32(out, id);
            ctx.r[0] = R_OK;
        }
        // Break: the guest's assert; the boot stops cleanly with the reason
        0x3C => {
            let reason = ctx.r[0];
            let (r1, r2) = (ctx.r[1], ctx.r[2]);
            process.stop_boot(format!(
                "svcBreak reason {reason} r1 {r1:#x} r2 {r2:#x} at {:#x}",
                ctx.r[15]
            ));
            ctx.exit = context::EXIT_UNWIND;
        }
        // OutputDebugString: the guest's print, into the boot log
        0x3D => {
            let (at, len) = (ctx.r[0], ctx.r[1].min(4096));
            let mut text = Vec::with_capacity(len as usize);
            for i in 0..len {
                text.push(process.memory.read8(at.wrapping_add(i)));
            }
            let line = String::from_utf8_lossy(&text).trim_end_matches('\0').to_owned();
            process.say(format!("guest: {line}"));
            ctx.r[0] = R_OK;
        }
        // the cache maintenance svcs: nothing diverges on this host yet
        0x52 | 0x53 | 0x54 => {
            process.cache_ops.fetch_add(1, Ordering::Relaxed);
            ctx.r[0] = R_OK;
        }
        // KernelSetState: no-op types succeed, the rest gap
        0x7C => {
            ctx.r[0] = R_OK;
        }
        _ => {
            // the registry's fallback first, then the gap line
            if let Some(f) = handler_of(&FALLBACK) {
                if f(process, ctx, number) {
                    return;
                }
            }
            gap_refuse(process, ctx, number);
        }
    }
}

/// the gap refusal: one logged line per site, the kernel's stubbed result in r0
fn gap_refuse(process: &Process3ds, ctx: &mut Context, number: u32) {
    process.gap(&format!(
        "UNIMPL: svc {number:#04x} ({}) at {:#x}",
        svc_name(number),
        ctx.r[15]
    ));
    ctx.r[0] = R_STUB;
}

/// the release site the code just answered in place (under the loan the settle will close)
fn learn_release_site(ctx: &mut Context, handle: u32) {
    if ctx.held == handle && ctx.holding > 0 {
        ctx.release_at = ctx.r[15] | ctx.thumb as u32;
    }
}

/// the MemoryState enum value for the query answer
fn state_of(state: State) -> u32 {
    match state {
        State::Free => 0,
        State::Reserved => 1,
        State::Io => 2,
        State::Static => 3,
        State::Code => 4,
        State::Private => 5,
        State::Shared => 6,
        State::Continuous => 7,
        State::Aliased => 8,
        State::Alias => 9,
        State::AliasCode => 10,
        State::Locked => 11,
    }
}

fn map_result(process: &Process3ds, base: u32, size: u32, perm: Perm, state: State, kind: Kind) -> u32 {
    match process.memory.map(base, size, perm, state, kind) {
        Ok(()) => R_OK,
        Err(_) => R_INVALID,
    }
}

/// the svc's name for the gap lines
pub fn svc_name(number: u32) -> &'static str {
    match number {
        0x01 => "ControlMemory",
        0x02 => "QueryMemory",
        0x03 => "ExitProcess",
        0x04 => "GetProcessAffinityMask",
        0x05 => "SetProcessAffinityMask",
        0x06 => "GetProcessIdealProcessor",
        0x07 => "SetProcessIdealProcessor",
        0x08 => "CreateThread",
        0x09 => "ExitThread",
        0x0A => "SleepThread",
        0x0B => "GetThreadPriority",
        0x0C => "SetThreadPriority",
        0x0D => "GetThreadAffinityMask",
        0x0E => "SetThreadAffinityMask",
        0x0F => "GetThreadIdealProcessor",
        0x10 => "SetThreadIdealProcessor",
        0x11 => "GetCurrentProcessorNumber",
        0x12 => "Run",
        0x13 => "CreateMutex",
        0x14 => "ReleaseMutex",
        0x15 => "CreateSemaphore",
        0x16 => "ReleaseSemaphore",
        0x17 => "CreateEvent",
        0x18 => "SignalEvent",
        0x19 => "ClearEvent",
        0x1A => "CreateTimer",
        0x1B => "SetTimer",
        0x1C => "CancelTimer",
        0x1D => "ClearTimer",
        0x1E => "CreateMemoryBlock",
        0x1F => "MapMemoryBlock",
        0x20 => "UnmapMemoryBlock",
        0x21 => "CreateAddressArbiter",
        0x22 => "ArbitrateAddress",
        0x23 => "CloseHandle",
        0x24 => "WaitSynchronization1",
        0x25 => "WaitSynchronizationN",
        0x26 => "SignalAndWait",
        0x27 => "DuplicateHandle",
        0x28 => "GetSystemTick",
        0x29 => "GetHandleInfo",
        0x2A => "GetSystemInfo",
        0x2B => "GetProcessInfo",
        0x2C => "GetThreadInfo",
        0x2D => "ConnectToPort",
        0x32 => "SendSyncRequest",
        0x35 => "GetProcessId",
        0x37 => "GetThreadId",
        0x38 => "GetResourceLimit",
        0x3C => "Break",
        0x3D => "OutputDebugString",
        0x4F => "ReplyAndReceive",
        0x52 => "InvalidateProcessDataCache",
        0x53 => "StoreProcessDataCache",
        0x54 => "FlushProcessDataCache",
        0x70 => "ControlProcessMemory",
        0x7C => "KernelSetState",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the gap refusal answers the kernel's stubbed result and logs once per site
    #[test]
    fn names_and_results_line_up() {
        // the three in-place numbers are the donor's, verbatim
        assert_eq!(svc_name(0x28), "GetSystemTick");
        assert_eq!(svc_name(0x25), "WaitSynchronizationN");
        assert_eq!(svc_name(0x14), "ReleaseMutex");
        assert_eq!(R_STUB, 0xF8C0_07F4, "3dbrew's stubbed-svc result");
        assert_eq!(R_TIMEOUT, 0x0940_1BFE, "3dbrew's WaitSynchronization timeout");
    }
}
