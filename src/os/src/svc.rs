//! The SVC entry of the generated code as the os layer builds it: the translated C calls `nxos_svc` (the os
//! layer's `cpu.h` overlay renames `rt_svc`), which answers the memory, IPC, information and counter SVCs here and
//! hands the rest (threads, synchronization, ExitThread, ExitProcess, debug output) to the runtime's
//! `nxrt_svc_impl`, whose shim status it passes on. An SVC neither side handles answers success with an
//! `UNIMPL Svc` gap line, as on nxnative.
//!
//! The counter (`nxos_ticks`, CNTPCT/CNTVCT and GetSystemTick) runs at the title's rate: 19.2 MHz on Switch 1,
//! 31.25 MHz on Switch 2 (`nxos_tick_hz`, also what CNTFRQ reads).

use crate::guest::{r32, w32, w64};
use crate::log::Gap;
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

core::arch::global_asm!(
    ".text",
    ".globl nxos_svc",
    ".p2align 4",
    ".seh_proc nxos_svc",
    "nxos_svc:",
    "push rcx",
    ".seh_pushreg rcx",
    "sub rsp, 32",
    ".seh_stackalloc 32",
    ".seh_endprologue",
    "call nxos_svc_impl",
    "add rsp, 32",
    "pop rcx",
    "test eax, eax",
    "jnz 2f",
    "ret",
    "2:",
    "cmp eax, 1",
    "je rt_dispatch",
    "jmp nxrt_jump_pending",
    ".seh_endproc",
);

/// The counter rate (CNTFRQ): set from the profile before any guest code runs.
#[no_mangle]
pub static mut nxos_tick_hz: u32 = 19_200_000;

static T0: AtomicU64 = AtomicU64::new(0);

pub fn set_tick_hz(hz: u32) {
    unsafe { nxos_tick_hz = hz };
}

pub fn tick_hz() -> u64 {
    unsafe { nxos_tick_hz as u64 }
}

/// The guest's system counter.
#[no_mangle]
pub extern "C" fn nxos_ticks() -> u64 {
    let ns = nxrt::rt::now_ns();
    let t0 = T0.load(Ordering::Relaxed);
    let ns = if t0 == 0 {
        let _ = T0.compare_exchange(0, ns, Ordering::Relaxed, Ordering::Relaxed);
        0
    } else {
        ns - t0.min(ns)
    };
    (ns as u128 * tick_hz() as u128 / 1_000_000_000) as u64
}

/// Nanoseconds of the guest clock (the counter's time base).
pub fn guest_ns() -> u64 {
    (nxos_ticks() as u128 * 1_000_000_000 / tick_hz() as u128) as u64
}

static COUNTS: [AtomicU64; 0x80] = [const { AtomicU64::new(0) }; 0x80];

pub fn counts() -> Vec<(u32, u64)> {
    COUNTS.iter().enumerate().filter_map(|(i, c)| {
        let n = c.load(Ordering::Relaxed);
        (n > 0).then_some((i as u32, n))
    }).collect()
}

/// SVCs the runtime answers (memory and debug ones it has are answered here instead).
fn runtime_handles(imm: u32) -> bool {
    matches!(
        imm,
        0x07 | 0x08 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D | 0x0E | 0x0F | 0x10 | 0x11 | 0x12 | 0x16 | 0x17 | 0x18 | 0x19 | 0x1A | 0x1B | 0x1C | 0x1D | 0x25 | 0x26 | 0x27 | 0x34 | 0x35 | 0x45
    )
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
    let s: &'static SharedMem = Box::leak(Box::new(SharedMem { handle: h, size, views: std::sync::Mutex::new(vec![]) }));
    SHARED.lock().unwrap().push(s);
    s
}

fn shared_of(h: u32) -> Option<&'static SharedMem> {
    SHARED.lock().unwrap().iter().find(|s| s.handle == h).copied()
}

#[no_mangle]
pub unsafe extern "C" fn nxos_svc_impl(c: *mut Cpu, imm: u32, pc: u64) -> u32 {
    nxrt::boundary::guard("nxos_svc", || {
        let st = svc(c, imm, pc);
        // the scheduler's checkpoint: a better thread waiting for this core runs first
        if st == nxrt::asm::ST_RETURN && imm != 0x0A {
            crate::sched::checkpoint(false);
        }
        st
    })
}

unsafe fn svc(c: *mut Cpu, imm: u32, pc: u64) -> u32 {
    if let Some(n) = COUNTS.get(imm as usize) {
        n.fetch_add(1, Ordering::Relaxed);
    }
    let cpu = &mut *c;
    if crate::trace::on("svc") && imm != 0x27 && imm != 0x1E {
        crate::log!("svc {imm:#04x} {} at {} x0 {:#x} x1 {:#x} x2 {:#x} x3 {:#x}", name(imm), nxrt::dispatch::guest_name(pc), cpu.x[0], cpu.x[1], cpu.x[2], cpu.x[3]);
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
        0x02 | 0x03 => cpu.x[0] = 0,
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
                Some(s) if n <= crate::mem::PAGE * (s.size.div_ceil(crate::mem::PAGE)) && m.map_shared(a, n) => {
                    s.views.lock().unwrap().push(a);
                    crate::log_once!(format!("shm {h:#x}"), "MapSharedMemory {h:#x} at {a:#x}+{n:#x}");
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
            crate::log_once!("transfer", "CreateTransferMemory {:#x}+{:#x}: host services read it in place", cpu.x[1], cpu.x[2]);
            cpu.x[0] = 0;
            cpu.x[1] = (0xD100_0000 | (TRANSFER.fetch_add(1, Ordering::Relaxed) & 0x7FFF)) as u64;
        }
        0x1E => {
            cpu.x[0] = nxos_ticks();
        }
        0x1F => {
            // ConnectToNamedPort(-> w1 handle; x1 name)
            let name = crate::guest::cstr(cpu.x[1], 12);
            if name == "sm:" {
                cpu.x[0] = 0;
                cpu.x[1] = crate::ipc::open(Box::new(crate::ipc::Sm)) as u64;
            } else {
                crate::log::gap(Gap::Service, &format!("port {name}"), "ConnectToNamedPort refused");
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
        0x16 if cpu.x[0] as u32 & 0xFFF0_0000 == 0xD100_0000 || shared_of(cpu.x[0] as u32).is_some() => cpu.x[0] = 0,
        0x24 => {
            cpu.x[0] = 0;
            cpu.x[1] = PID;
        }
        0x26 => {
            // Break(w0 reason, x1 buffer, x2 size): the SDK's abort. Log its Result before the runtime's report.
            let (reason, buf, n) = (cpu.x[0], cpu.x[1], cpu.x[2]);
            if reason & 0x8000_0000 == 0 {
                let res = if buf != 0 && n >= 4 && nxrt::mem::readable(buf, 4) { r32(buf) } else { 0 };
                crate::log!("svcBreak reason {reason:#x} result {res:#x} at {}", nxrt::dispatch::guest_name(pc));
                crate::log::report_gaps("svcBreak");
            }
            return nxrt::svc::nxrt_svc_impl(c, imm, pc);
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
                25..=28 => Some(0),
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
            THREADS.fetch_add(1, Ordering::Relaxed);
            return crate::sched::create_thread(c, pc);
        }
        0x16 => {
            crate::services::os::handle_closed(cpu.x[0] as u32);
            return crate::sched::delegate(c, imm, pc);
        }
        0x0D => {
            let (h, p) = (cpu.x[0] as u32, cpu.x[1] as i32);
            let st = crate::sched::delegate(c, imm, pc);
            if (*c).x[0] == 0 {
                crate::sched::note_priority(h, p);
            }
            return st;
        }
        0x0F => {
            let (h, ideal, mask) = (cpu.x[0] as u32, cpu.x[1] as i32, cpu.x[2]);
            let st = crate::sched::delegate(c, imm, pc);
            if (*c).x[0] == 0 {
                crate::sched::note_core_mask(h, ideal, mask);
            }
            return st;
        }
        0x10 => match crate::sched::current_core() {
            Some(core) => cpu.x[0] = core as u64,
            None => return crate::sched::delegate(c, imm, pc),
        },
        _ if runtime_handles(imm) => return crate::sched::delegate(c, imm, pc),
        _ => {
            crate::log::gap(Gap::Svc, &format!("svc {imm:#x} {}", name(imm)), &format!("at {}, answered success", nxrt::dispatch::guest_name(pc)));
            cpu.x[0] = 0;
        }
    }
    nxrt::asm::ST_RETURN
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
