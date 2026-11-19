//! `rt_svc`, the SVC entry of lifted code: memory, process and debug SVCs here, threads and synchronization in
//! `kernel.rs`. An SVC nothing handles stops with its name, the caller and the guest backtrace.

use crate::cpu::Cpu;
use crate::kernel::{self, R_INVALID_ENUM, R_OK};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

static SVCS: AtomicU64 = AtomicU64::new(0);

/// An SVC handled outside the runtime (the os layer's IPC, memory and info SVCs): it reads its arguments from the
/// guest registers and writes its results there. Returns false to let the runtime handle it after all.
pub type SvcHandler = fn(&mut Cpu, u32) -> bool;

static HANDLERS: [std::sync::atomic::AtomicUsize; 256] = [const { std::sync::atomic::AtomicUsize::new(0) }; 256];
static FALLBACK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Routes `SVC #imm` to `f` before the runtime's own handling (set before guest code runs; a later call replaces it).
pub fn set_handler(imm: u8, f: SvcHandler) {
    HANDLERS[imm as usize].store(f as usize, Ordering::Release);
}

/// Called for every SVC neither a handler nor the runtime handles, instead of the fatal "not implemented".
pub fn set_fallback(f: SvcHandler) {
    FALLBACK.store(f as usize, Ordering::Release);
}

fn handler_of(slot: &std::sync::atomic::AtomicUsize) -> Option<SvcHandler> {
    match slot.load(Ordering::Acquire) {
        0 => None,
        // SAFETY: only `set_handler` / `set_fallback` store here, always an `SvcHandler`.
        p => Some(unsafe { core::mem::transmute::<usize, SvcHandler>(p) }),
    }
}

thread_local! {
    static CALLER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The guest address of the SVC instruction the calling thread is in (for a handler's logs).
pub fn caller_pc() -> u64 {
    CALLER.get()
}

pub fn count() -> u64 {
    SVCS.load(Ordering::Relaxed)
}

fn name(imm: u32) -> &'static str {
    match imm {
        0x01 => "SetHeapSize",
        0x02 => "SetMemoryPermission",
        0x03 => "SetMemoryAttribute",
        0x04 => "MapMemory",
        0x05 => "UnmapMemory",
        0x06 => "QueryMemory",
        0x07 => "ExitProcess",
        0x13 => "MapSharedMemory",
        0x1F => "ConnectToNamedPort",
        0x21 => "SendSyncRequest",
        0x26 => "Break",
        0x27 => "OutputDebugString",
        0x28 => "ReturnFromException",
        0x29 => "GetInfo",
        0x2C => "MapPhysicalMemory",
        0x2D => "UnmapPhysicalMemory",
        0x32 => "SetThreadActivity",
        0x33 => "GetThreadContext3",
        0x40 => "CreateSession",
        0x43 => "ReplyAndReceive",
        0x50 => "CreateSharedMemory",
        0x51 => "MapTransferMemory",
        _ => "unknown",
    }
}

/// Called by `rt_svc` (asm.rs) for `void rt_svc(Cpu *c, uint32_t imm, uint64_t pc)`; returns a shim status
/// (`asm::ST_*`): ExitThread asks for the jump to the thread's next barrier.
#[no_mangle]
pub unsafe extern "C" fn nxrt_svc_impl(c: *mut Cpu, imm: u32, pc: u64) -> u32 {
    crate::boundary::guard("rt_svc", || svc(c, imm, pc))
}

unsafe fn svc(c: *mut Cpu, imm: u32, pc: u64) -> u32 {
    SVCS.fetch_add(1, Ordering::Relaxed);
    let c = &mut *c;
    CALLER.set(pc);
    if crate::rt::trace("svc") && imm != 0x27 {
        crate::rt::log(&format!("svc {imm:#04x} at {} x0 {:#x} x1 {:#x} x2 {:#x} x3 {:#x}", crate::dispatch::guest_name(pc), c.x[0], c.x[1], c.x[2], c.x[3]));
    }
    if let Some(f) = handler_of(&HANDLERS[(imm & 0xFF) as usize]) {
        if f(c, imm) {
            return crate::asm::ST_RETURN;
        }
    }
    let space = kernel::get().space;
    match imm {
        0x01 => match space.set_heap_size(c.x[1]) {
            Ok(a) => {
                c.x[0] = R_OK;
                c.x[1] = a;
            }
            Err(r) => c.x[0] = r as u64,
        },
        0x06 => {
            // QueryMemory(x0 MemoryInfo*, x1 page info*, x2 address) -> w1 page info
            let info = space.query(c.x[2]);
            core::ptr::write_unaligned(c.x[0] as *mut crate::mem::MemoryInfo, info);
            c.x[0] = R_OK;
            c.x[1] = 0;
        }
        0x07 => crate::process::exit(0),
        0x0A => {
            if crate::fiber::on_fiber() {
                crate::rt::guest_fatal(c, "ExitThread on a guest fiber is not supported yet");
            }
            kernel::get().exit_current();
            return crate::boundary::exit_jump(c);
        }
        0x26 => {
            // Break(w0 reason, x1 buffer, x2 size)
            let (reason, p, n) = (c.x[0], c.x[1], c.x[2]);
            if reason & 0x8000_0000 != 0 {
                c.x[0] = R_OK; // a notification only (e.g. a module load)
            } else {
                c.pc = pc;
                crate::rt::guest_fatal(c, &format!("guest svcBreak reason {reason:#x} buffer {p:#x}+{n:#x} at {}", crate::dispatch::guest_name(pc)));
            }
        }
        0x27 => {
            // OutputDebugString(x0 string, x1 size): the guest's output, to stdout.
            let s = core::slice::from_raw_parts(c.x[0] as *const u8, c.x[1] as usize);
            let mut o = std::io::stdout().lock();
            let _ = o.write_all(s);
            let _ = o.flush();
            c.x[0] = R_OK;
        }
        0x29 => {
            // GetInfo(-> x1; w1 id0, w2 handle, x3 id1)
            let (id0, id1) = (c.x[1] as u32, c.x[3]);
            let v = match id0 {
                0 => Some(0xF),  // core mask
                1 => Some(0xFFFF_FFFF_FFFF_FFF0), // priority mask
                8 => Some(0),    // debugger attached
                11 if id1 < 4 => Some(crate::rt::rt_ticks().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ id1.wrapping_mul(0xC2B2_AE3D_27D4_EB4F)), // random entropy
                18 => Some(0x0100_0000_0000_1000), // program id
                20 => Some(0), // user exception context
                23 => Some(1), // is application
                _ => space.info(id0),
            };
            match v {
                Some(v) => {
                    c.x[0] = R_OK;
                    c.x[1] = v;
                }
                None => c.x[0] = R_INVALID_ENUM,
            }
        }
        0x1E => c.x[0] = crate::rt::rt_ticks(),
        0x2C | 0x2D => c.x[0] = space.map_physical(c.x[0], c.x[1], imm == 0x2C) as u64,
        _ => {
            if !kernel::svc(c, imm) && !handler_of(&FALLBACK).is_some_and(|f| f(c, imm)) {
                c.pc = pc;
                crate::rt::guest_fatal(c, &format!("svc #{imm:#x} {} at {} is not implemented", name(imm), crate::dispatch::guest_name(pc)));
            }
        }
    }
    crate::asm::ST_RETURN
}
