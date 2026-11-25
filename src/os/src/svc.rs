//! The os layer's SVCs on the runtime's registry (`nxrt::svc::set_handler`, installed by `install` before any
//! guest code runs): the memory, IPC, information and counter SVCs are answered here; the kernel's (threads,
//! synchronization) go to `nxrt::kernel::svc`, which tells the scheduler (`sched.rs`) about blocking waits; ExitThread,
//! ExitProcess and Break are left to the runtime. After every SVC answered here comes the scheduler's checkpoint. An
//! SVC nobody handles (the runtime's fallback) answers NotImplemented (0xF601) with an `UNIMPL Svc` gap line
//! (audit 2026-11-20: answering success was a fake).
//!
//! The counter (CNTPCT/CNTVCT and GetSystemTick, the runtime's `rt_ticks`) runs at the title's rate (19.2 MHz on
//! Switch 1, 31.25 MHz on Switch 2: the runtime's `rt_tick_hz`, also what CNTFRQ reads) on the guest clock
//! (`clock.rs`, through `nxrt::rt::set_clock`), starting at 0.

use crate::guest::{r32, w32, w64};
use crate::log::Gap;
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static T0: AtomicU64 = AtomicU64::new(0);

/// The counter rate (CNTFRQ): set from the profile before any guest code runs (after `kernel::init`, whose start-up
/// sets the runtime's default).
pub fn set_tick_hz(hz: u32) {
    nxrt::rt::set_tick_hz(hz);
}

pub fn tick_hz() -> u64 {
    nxrt::rt::tick_hz() as u64
}

/// The counter's time base: the guest clock since the first reading, in ns.
fn counter_ns() -> u64 {
    let ns = crate::clock::now_ns();
    let t0 = T0.load(Ordering::Relaxed);
    if t0 == 0 {
        let _ = T0.compare_exchange(0, ns, Ordering::Relaxed, Ordering::Relaxed);
        0
    } else {
        ns - t0.min(ns)
    }
}

/// The guest's system counter.
pub fn ticks() -> u64 {
    nxrt::rt::rt_ticks()
}

/// Nanoseconds of the guest clock (the counter's time base).
pub fn guest_ns() -> u64 {
    (ticks() as u128 * 1_000_000_000 / tick_hz() as u128) as u64
}

static COUNTS: [AtomicU64; 0x80] = [const { AtomicU64::new(0) }; 0x80];

pub fn counts() -> Vec<(u32, u64)> {
    COUNTS
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let n = c.load(Ordering::Relaxed);
            (n > 0).then_some((i as u32, n))
        })
        .collect()
}

/// The SVCs os registers: its own and the kernel's (0x07 ExitProcess and 0x0A ExitThread are counted, then left to
/// the runtime).
const SERVED: &[u8] = &[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0A, 0x09, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x21,
    0x22, 0x24, 0x25, 0x26, 0x27, 0x29, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x34, 0x35, 0x36, 0x37,
    0x45, 0x6F, 0x80,
];

/// Registers os's SVCs, the fallback and the counter's clock with the runtime (before any guest code runs).
pub fn install() {
    for &imm in SERVED {
        nxrt::svc::set_handler(imm, handle);
    }
    nxrt::svc::set_fallback(unhandled);
    nxrt::rt::set_clock(counter_ns);
}

pub fn name(imm: u32) -> &'static str {
    match imm {
        0x01 => "SetHeapSize",
        0x02 => "SetMemoryPermission",
        0x03 => "SetMemoryAttribute",
        0x04 => "MapMemory",
        0x05 => "UnmapMemory",
        0x06 => "QueryMemory",
        0x13 => "MapSharedMemory",
        0x14 => "UnmapSharedMemory",
        0x15 => "CreateTransferMemory",
        0x1E => "GetSystemTick",
        0x1F => "ConnectToNamedPort",
        0x20 => "SendSyncRequestLight",
        0x21 => "SendSyncRequest",
        0x22 => "SendSyncRequestWithUserBuffer",
        0x24 => "GetProcessId",
        0x26 => "Break",
        0x28 => "ReturnFromException",
        0x29 => "GetInfo",
        0x2C => "MapPhysicalMemory",
        0x2D => "UnmapPhysicalMemory",
        0x30 => "GetResourceLimitLimitValue",
        0x31 => "GetResourceLimitCurrentValue",
        0x2E => "GetDebugFutureThreadInfo",
        0x2F => "GetLastThreadInfo",
        0x37 => "GetResourceLimitPeakValue",
        0x6F => "GetSystemInfo",
        0x80 => "SetGpuMemoryAttribute",
        0x32 => "SetThreadActivity",
        0x33 => "GetThreadContext3",
        0x36 => "SynchronizePreemptionState",
        0x40 => "CreateSession",
        0x43 => "ReplyAndReceive",
        0x50 => "CreateSharedMemory",
        0x51 => "MapTransferMemory",
        0x52 => "UnmapTransferMemory",
        _ => "?",
    }
}

/// Horizon's kernel results for an SVC the host does not serve (NotImplemented / NotFound).
pub const R_NOT_IMPLEMENTED: u64 = 0xF601;
pub const R_NOT_FOUND: u64 = 0xF201;

/// A resource limit's limit (and peak) value: 0 physical memory (the pool), 1 threads 800,
/// 2 events 800, 3 transfer memories 256, the rest (sessions) 64.
pub fn resource_limit(which: u32) -> u64 {
    match which {
        0 => crate::mem::try_get().and_then(|m| m.info(6)).unwrap_or(0),
        1 | 2 => 800,
        3 => 256,
        _ => 64,
    }
}

/// GetSystemInfo(type, subtype): type 0 a pool's physical size (the application's, then the
/// applet, system and unsafe pools), 1 its use, 2 the initial process id range.
pub fn system_info(ty: u32, sub: u64, pool: u64, used: u64) -> u64 {
    match (ty, sub) {
        (0, 0) => pool,
        (0, 1) => 0x1FB0_0000,
        (0, 2) => 0x29D0_0000,
        (0, _) => 0x600_0000,
        (1, 0) => used,
        (1, _) => 0x100_0000,
        (2, 0) => 1,
        (2, _) => 0x50,
        _ => 0,
    }
}

/// The process id the guest sees (GetProcessId, the pid of service requests).
pub const PID: u64 = 0x51;

static TRANSFER: AtomicU32 = AtomicU32::new(0);

/// Shared memory a host service owns: its handle, size and a host writer that fills a fresh view.
pub struct SharedMem {
    pub handle: u32,
    pub size: u64,
    pub views: std::sync::Mutex<Vec<u64>>,
}

static SHARED: std::sync::Mutex<Vec<&'static SharedMem>> = std::sync::Mutex::new(Vec::new());
static NEXT_SHARED: AtomicU32 = AtomicU32::new(0);

/// A shared memory object for a service: the guest maps it with MapSharedMemory, the service writes its views.
pub fn new_shared(size: u64) -> &'static SharedMem {
    let h = crate::hle::MEMORY_BASE | 0x8000 | NEXT_SHARED.fetch_add(1, Ordering::Relaxed);
    let s: &'static SharedMem = Box::leak(Box::new(SharedMem {
        handle: h,
        size,
        views: std::sync::Mutex::new(vec![]),
    }));
    SHARED.lock().unwrap().push(s);
    s
}

fn shared_of(h: u32) -> Option<&'static SharedMem> {
    SHARED
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.handle == h)
        .copied()
}

/// os's handler for every SVC in `SERVED`: true = answered (the checkpoint follows), false = the runtime's.
fn handle(cpu: &mut Cpu, imm: u32) -> bool {
    let pc = nxrt::svc::caller_pc();
    // SAFETY: the calling guest thread's own state, as the runtime hands it.
    let done = unsafe { svc(cpu, imm, pc) };
    if done {
        // the scheduler's checkpoint: a better thread waiting for this core runs first
        crate::sched::checkpoint(false);
    }
    done
}

/// An SVC neither os nor the runtime handles: success, with a gap line.
fn unhandled(cpu: &mut Cpu, imm: u32) -> bool {
    if let Some(n) = COUNTS.get(imm as usize) {
        n.fetch_add(1, Ordering::Relaxed);
    }
    let pc = nxrt::svc::caller_pc();
    crate::log::gap(
        Gap::Svc,
        &format!("svc {imm:#x} {}", name(imm)),
        &format!(
            "at {}, answered NotImplemented",
            nxrt::dispatch::guest_name(pc)
        ),
    );
    cpu.x[0] = R_NOT_IMPLEMENTED;
    crate::sched::checkpoint(false);
    true
}

/// The kernel's handling of a thread / synchronization SVC.
fn kernel(cpu: &mut Cpu, imm: u32) -> bool {
    nxrt::kernel::svc(cpu, imm)
}

unsafe fn svc(cpu: &mut Cpu, imm: u32, pc: u64) -> bool {
    if let Some(n) = COUNTS.get(imm as usize) {
        n.fetch_add(1, Ordering::Relaxed);
    }
    if crate::trace::on("svc") && imm != 0x27 && imm != 0x1E {
        crate::log!(
            "svc {imm:#04x} {} at {} x0 {:#x} x1 {:#x} x2 {:#x} x3 {:#x}",
            name(imm),
            nxrt::dispatch::guest_name(pc),
            cpu.x[0],
            cpu.x[1],
            cpu.x[2],
            cpu.x[3]
        );
    }
    let m = crate::mem::get();
    match imm {
        0x01 => match m.set_heap_size(cpu.x[1]) {
            Ok(a) => {
                cpu.x[0] = 0;
                cpu.x[1] = a;
            }
            Err(r) => cpu.x[0] = r,
        },
        0x02 => cpu.x[0] = m.set_permission(cpu.x[0], cpu.x[1], cpu.x[2] as u32),
        0x03 => {
            cpu.x[0] = m.set_attribute(cpu.x[0], cpu.x[1], cpu.x[2] as u32, cpu.x[3] as u32, false)
        }
        0x80 => {
            let r = m.set_attribute(cpu.x[0], cpu.x[1], cpu.x[2] as u32, cpu.x[3] as u32, true);
            crate::log_once!(
                "svc 0x80",
                "svc SetGpuMemoryAttribute {:#x}+{:#x} {:#x}/{:#x}: recorded (result {r:#x})",
                cpu.x[0],
                cpu.x[1],
                cpu.x[3],
                cpu.x[2]
            );
            cpu.x[0] = r;
        }
        0x30 | 0x37 => {
            // GetResourceLimitLimitValue / PeakValue(handle, which)
            cpu.x[1] = resource_limit(cpu.x[2] as u32);
            cpu.x[0] = 0;
        }
        0x31 => {
            // GetResourceLimitCurrentValue: memory in use, live threads, 16 of the rest
            cpu.x[1] = match cpu.x[2] as u32 {
                0 => m.info(7).unwrap_or(0),
                1 => nxrt::kernel::get().thread_count() as u64,
                _ => 16,
            };
            cpu.x[0] = 0;
        }
        0x6F => {
            // GetSystemInfo(type, handle, subtype): newer SDKs abort at boot on 0
            cpu.x[1] = system_info(
                cpu.x[1] as u32,
                cpu.x[3],
                m.info(6).unwrap_or(0),
                m.info(7).unwrap_or(0),
            );
            cpu.x[0] = 0;
        }
        // GetDebugFutureThreadInfo / GetLastThreadInfo: no debugger
        0x2E | 0x2F => cpu.x[0] = R_NOT_FOUND,
        0x04 => cpu.x[0] = m.map_memory(cpu.x[0], cpu.x[1], cpu.x[2]),
        0x05 => cpu.x[0] = m.unmap_memory(cpu.x[0], cpu.x[1], cpu.x[2]),
        0x06 => {
            let info = m.query(cpu.x[2]);
            if cpu.x[0] != 0 {
                core::ptr::write_unaligned(cpu.x[0] as *mut nxrt::mem::MemoryInfo, info);
            }
            cpu.x[0] = 0;
            cpu.x[1] = 0;
        }
        0x13 => {
            // MapSharedMemory(w0 handle, x1 address, x2 size, w3 permission)
            let (h, a, n) = (cpu.x[0] as u32, cpu.x[1], cpu.x[2]);
            cpu.x[0] = match shared_of(h) {
                Some(s)
                    if n <= crate::mem::PAGE * (s.size.div_ceil(crate::mem::PAGE))
                        && m.map_shared(a, n) =>
                {
                    s.views.lock().unwrap().push(a);
                    crate::log_once!(
                        format!("shm {h:#x}"),
                        "MapSharedMemory {h:#x} at {a:#x}+{n:#x}"
                    );
                    0
                }
                Some(_) => crate::mem::R_INVALID_SIZE,
                None => {
                    crate::log!("[RED] MapSharedMemory of {h:#x}, not a host shared memory");
                    0xE401
                }
            };
        }
        0x14 => {
            let (h, a, n) = (cpu.x[0] as u32, cpu.x[1], cpu.x[2]);
            if let Some(s) = shared_of(h) {
                s.views.lock().unwrap().retain(|&v| v != a);
            }
            m.unmap_shared(a, n);
            cpu.x[0] = 0;
        }
        0x15 => {
            // CreateTransferMemory(-> w1 handle; x1 address, x2 size, w3 permission): stays where it is
            crate::log_once!(
                "transfer",
                "CreateTransferMemory {:#x}+{:#x}: host services read it in place",
                cpu.x[1],
                cpu.x[2]
            );
            cpu.x[0] = 0;
            cpu.x[1] = (0xD100_0000 | (TRANSFER.fetch_add(1, Ordering::Relaxed) & 0x7FFF)) as u64;
        }
        0x1E => {
            cpu.x[0] = ticks();
        }
        0x1F => {
            // ConnectToNamedPort(-> w1 handle; x1 name)
            let name = crate::guest::cstr(cpu.x[1], 12);
            if name == "sm:" {
                cpu.x[0] = 0;
                cpu.x[1] = crate::ipc::open(Box::new(crate::ipc::Sm)) as u64;
            } else {
                crate::log::gap(
                    Gap::Service,
                    &format!("port {name}"),
                    "ConnectToNamedPort refused",
                );
                cpu.x[0] = 0xF201; // NotFound
            }
        }
        0x21 => {
            // SendSyncRequest(w0 session): the message is the thread's TLS (0x100 bytes)
            let msg = core::slice::from_raw_parts_mut(cpu.tpidrro as *mut u8, 0x100);
            cpu.x[0] = crate::ipc::process(cpu.x[0] as u32, msg) as u64;
        }
        0x22 => {
            // SendSyncRequestWithUserBuffer(x0 buffer, x1 size, w2 session)
            let (a, n, h) = (cpu.x[0], cpu.x[1], cpu.x[2] as u32);
            let msg = crate::guest::bytes_mut(a, n);
            cpu.x[0] = crate::ipc::process(h, msg) as u64;
        }
        0x16 if crate::ipc::is_session(cpu.x[0] as u32) => {
            crate::ipc::close(cpu.x[0] as u32);
            cpu.x[0] = 0;
        }
        0x16 if cpu.x[0] as u32 & 0xFFF0_0000 == 0xD100_0000
            || shared_of(cpu.x[0] as u32).is_some() =>
        {
            cpu.x[0] = 0
        }
        0x24 => {
            cpu.x[0] = 0;
            cpu.x[1] = PID;
        }
        0x26 => {
            // Break(w0 reason, x1 buffer, x2 size): the SDK's abort. Log its Result before the runtime's report.
            let (reason, buf, n) = (cpu.x[0], cpu.x[1], cpu.x[2]);
            if reason & 0x8000_0000 == 0 {
                let res = if buf != 0 && n >= 4 && nxrt::mem::readable(buf, 4) {
                    r32(buf)
                } else {
                    0
                };
                crate::log!(
                    "svcBreak reason {reason:#x} result {res:#x} at {}",
                    nxrt::dispatch::guest_name(pc)
                );
                crate::log::report_gaps("svcBreak");
            }
            return false;
        }
        0x29 => {
            // GetInfo(-> x1; w1 id0, w2 handle, x3 id1)
            let (id0, id1) = (cpu.x[1] as u32, cpu.x[3]);
            let p = crate::profile::get();
            let v = match id0 {
                0 => Some(p.core_mask),
                1 => Some(0),
                8 => Some(0),
                11 if id1 < 4 => Some(entropy(id1)),
                18 => Some(p.program_id),
                19 => Some(0),
                20 => Some(user_exception_context()),
                23 => Some(1),
                24 => Some(96u64.saturating_sub(THREADS.load(Ordering::Relaxed))),
                // 29, 30: Switch 2 kernels' newer ids (0); 33: their page size
                25..=30 => Some(0),
                33 => Some(0x1000),
                _ => m.info(id0),
            };
            match v {
                Some(v) => {
                    cpu.x[0] = 0;
                    cpu.x[1] = v;
                }
                None => {
                    crate::log::gap(Gap::Svc, &format!("GetInfo type {id0}"), "answered 0");
                    cpu.x[0] = 0;
                    cpu.x[1] = 0;
                }
            }
        }
        0x2C => cpu.x[0] = m.physical(cpu.x[0], cpu.x[1], true),
        0x2D => cpu.x[0] = m.physical(cpu.x[0], cpu.x[1], false),
        0x36 => cpu.x[0] = 0,
        0x08 => {
            // CreateThread(-> w1 handle; x1 entry, x2 argument, x3 stack top, w4 priority, w5 core): the scheduler
            // learns the thread here, and gives it a core when it starts (`Scheduler::thread_start`)
            THREADS.fetch_add(1, Ordering::Relaxed);
            let (prio, core) = (cpu.x[4] as i32, cpu.x[5] as i32);
            let core = if core == -2 { 0 } else { core };
            cpu.x[5] = core as u32 as u64;
            if !kernel(cpu, imm) {
                return false;
            }
            if cpu.x[0] == 0 {
                crate::sched::created(cpu.x[1] as u32, prio, core);
            }
        }
        0x0B if crate::sched::active() && (cpu.x[0] as i64) <= 0 => {
            // SleepThread 0 / -1 / -2: yields (to the same priority, with migration, to anyone)
            let ns = cpu.x[0] as i64;
            crate::sched::checkpoint(ns == -2 || ns == -1);
            std::thread::yield_now();
            cpu.x[0] = 0;
        }
        // ExitProcess, ExitThread: the runtime's (the scheduler hears of the thread's end in `thread_exit`)
        0x07 | 0x0A => return false,
        0x16 => {
            crate::services::os::handle_closed(cpu.x[0] as u32);
            return kernel(cpu, imm);
        }
        0x27 => {
            // OutputDebugString(x0 string, x1 size): the guest's output, to stdout
            use std::io::Write;
            let s = core::slice::from_raw_parts(cpu.x[0] as *const u8, cpu.x[1] as usize);
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(s);
            let _ = o.flush();
            cpu.x[0] = 0;
        }
        // threads and synchronization (SetThreadPriority / SetThreadCoreMask / GetCurrentProcessorNumber reach the
        // scheduler through its hooks, the blocking waits through block_enter / block_leave)
        _ => return kernel(cpu, imm),
    }
    true
}

static THREADS: AtomicU64 = AtomicU64::new(1);

fn entropy(i: u64) -> u64 {
    let t = nxrt::rt::now_ns();
    let mut x = t ^ (i + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x
}

/// The user exception context area (GetInfo 20): newer SDKs write their dying message region there.
fn user_exception_context() -> u64 {
    static A: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *A.get_or_init(|| nxrt::kernel::get().space.kernel_alloc(0x1000, 0x1000))
}

/// Writes the guest's MemoryInfo for tests.
#[allow(dead_code)]
pub(crate) fn write_info(a: u64, base: u64, size: u64, state: u32) {
    w64(a, base);
    w64(a + 8, size);
    w32(a + 16, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answers: resource limits, GetSystemInfo, and NotImplemented / NotFound for what the host
    /// does not serve; the unhandled path no longer answers success.
    #[test]
    fn unserved_svcs_answer_not_implemented() {
        assert_eq!(
            (
                resource_limit(1),
                resource_limit(2),
                resource_limit(3),
                resource_limit(4)
            ),
            (800, 800, 256, 64)
        );
        assert_eq!(system_info(0, 0, 0xCD50_0000, 0), 0xCD50_0000);
        assert_eq!(
            (
                system_info(0, 1, 0, 0),
                system_info(0, 2, 0, 0),
                system_info(0, 3, 0, 0)
            ),
            (0x1FB0_0000, 0x29D0_0000, 0x600_0000)
        );
        assert_eq!(
            (system_info(1, 0, 0, 0x1234), system_info(1, 1, 0, 0)),
            (0x1234, 0x100_0000)
        );
        assert_eq!(
            (system_info(2, 0, 0, 0), system_info(2, 1, 0, 0)),
            (1, 0x50)
        );
        let mut cpu: Box<Cpu> = Box::new(unsafe { std::mem::zeroed() });
        assert!(unhandled(&mut cpu, 0x4B));
        assert_eq!(cpu.x[0], R_NOT_IMPLEMENTED);
        for imm in [0x02u8, 0x03, 0x2E, 0x2F, 0x30, 0x31, 0x37, 0x6F, 0x80] {
            assert!(
                SERVED.contains(&imm),
                "{imm:#x} is served, not answered by the fallback"
            );
        }
    }
}
