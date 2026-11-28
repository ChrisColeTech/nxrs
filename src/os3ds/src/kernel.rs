//! the kernel svcs this side owns, on top of runtime3ds's kernel table: the named-service hook
//! surface (`ConnectToPort` 0x2D, `SendSyncRequest` 0x32), the shared-memory block map/unmap the
//! HID/GSP services need (0x1F/0x20), and service-owned handles in `CloseHandle` (0x23). Nothing
//! runtime3ds already answers is re-implemented; a handler that does not own a handle returns
//! `false` so the kernel table's own answer runs.

use crate::hle::{self, Host};
use runtime3ds::context::Context;
use runtime3ds::{svc, Process3ds};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

static HOST: OnceLock<Arc<Host>> = OnceLock::new();
/// the diagnostic wait/mutex lines are bounded so a polling loop cannot flood the log
static PROBE_LINES: AtomicU32 = AtomicU32::new(0);
const PROBE_LIMIT: u32 = 400;

fn host() -> Option<&'static Arc<Host>> {
    HOST.get()
}

fn probe(host: &Host, line: String) {
    if PROBE_LINES.fetch_add(1, Ordering::Relaxed) < PROBE_LIMIT {
        host.say(line);
    }
}

/// installs the os3ds svc handlers into runtime3ds's registry
pub fn install(host: Arc<Host>) {
    HOST.set(host).ok();
    svc::set_handler(0x2D, connect_handler);
    svc::set_handler(0x32, send_sync_handler);
    svc::set_handler(0x1F, map_block_handler);
    svc::set_handler(0x20, unmap_block_handler);
    svc::set_handler(0x23, close_handler);
    // runtime3ds's kernel table does not answer GetProcessId; the title asks before its srv init
    svc::set_handler(0x35, get_process_id_handler);
    // diagnostics: name the mutex handle/thread of every ReleaseMutex, then let the kernel answer
    svc::set_handler(0x14, release_mutex_probe);
    svc::set_handler(0x24, wait_one_probe);
    svc::set_handler(0x25, wait_n_probe);
    svc::set_handler(0x3C, break_probe);
    svc::set_handler(0x18, signal_probe);
}

/// diagnostics: name the event and thread of every SignalEvent, then let the kernel answer
fn signal_probe(_process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    if let Some(host) = host() {
        let thread = unsafe { runtime3ds::thread::ThreadState::of(ctx) }.thread;
        probe(host, format!("svc SignalEvent handle {:#x} thread {thread} at {:#x}", ctx.r[0], ctx.r[15]));
    }
    false
}

fn break_probe(_process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    if let Some(host) = host() {
        let thread = unsafe { runtime3ds::thread::ThreadState::of(ctx) }.thread;
        host.say(format!(
            "svcBreak thread {thread} reason {} r1 {:#x} r2 {:#x} lr {:#x} at {:#x}",
            ctx.r[0], ctx.r[1], ctx.r[2], ctx.r[14], ctx.r[15]
        ));
    }
    false
}

fn wait_one_probe(process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    if let Some(host) = host() {
        if host.is_apt_lock(ctx.r[0]) {
            ctx.r[0] = hle::R_OK;
            return true;
        }
        let thread = unsafe { runtime3ds::thread::ThreadState::of(ctx) }.thread;
        let ns = (ctx.r[2] as u64) | ((ctx.r[3] as u64) << 32);
        probe(host, format!(
            "svc WaitSynchronization1 handle {:#x} thread {thread} ns {ns} at {:#x}",
            ctx.r[0], ctx.r[15]
        ));
    }
    let _ = process;
    false
}

fn wait_n_probe(process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    if let Some(host) = host() {
        let count = ctx.r[2].min(8);
        if count == 1 {
            let handle = process.memory.read32(ctx.r[1]);
            if host.is_apt_lock(handle) {
                ctx.r[0] = hle::R_OK;
                ctx.r[1] = 0;
                return true;
            }
        }
        let thread = unsafe { runtime3ds::thread::ThreadState::of(ctx) }.thread;
        let mut handles = Vec::new();
        for i in 0..count {
            handles.push(process.memory.read32(ctx.r[1].wrapping_add(4 * i)));
        }
        probe(host, format!(
            "svc WaitSynchronizationN thread {thread} handles {handles:08x?} count {count} waitAll {} at {:#x}",
            ctx.r[3], ctx.r[15]
        ));
    }
    false
}

fn release_mutex_probe(_process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    let Some(host) = host() else { return false };
    if host.is_apt_lock(ctx.r[0]) {
        // the APT lock: a service-owned sync object, released from whichever applet thread holds it
        ctx.r[0] = hle::R_OK;
        return true;
    }
    let thread = unsafe { runtime3ds::thread::ThreadState::of(ctx) }.thread;
    probe(host, format!(
        "svc ReleaseMutex handle {:#x} thread {thread} at {:#x}",
        ctx.r[0], ctx.r[15]
    ));
    // false: the kernel table's own ReleaseMutex runs
    false
}

/// SVC 0x35 GetProcessId: r1 = process id (the wrapper at 0x29FB28 reads r1)
fn get_process_id_handler(_process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    ctx.r[0] = hle::R_OK;
    ctx.r[1] = 1;
    true
}

/// SVC 0x2D: r0 = out pointer, r1 = name; r0 = result, r1 = session handle (the Kernel ABI the
/// title's wrapper at 0x45C828 stores r1 through r0)
fn connect_handler(process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    let Some(host) = host() else { return false };
    let name = hle::read_cstr(process, ctx.r[1], 16);
    match host.connect(&name) {
        Some(handle) => {
            ctx.r[0] = hle::R_OK;
            ctx.r[1] = handle;
        }
        None => {
            host.gap(format!("ConnectToPort({name:?}) is not a registered port"));
            ctx.r[0] = hle::R_NOT_FOUND;
            ctx.r[1] = 0;
        }
    }
    true
}

/// SVC 0x32: the session in r0; the IPC buffer is at TLS+0x80
fn send_sync_handler(process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    let Some(host) = host() else { return false };
    // safe: ctx is one this host handed out, its user naming a live ThreadState
    let thread = unsafe { runtime3ds::thread::ThreadState::of(ctx) }.thread;
    if host.send_sync(process, ctx.tls, thread, ctx.r[15], ctx.r[0]) {
        ctx.r[0] = hle::R_OK;
    } else {
        ctx.r[0] = hle::R_STUB;
    }
    true
}

/// SVC 0x1F: MapMemoryBlock(handle, addr, myperm, otherperm); only a service block is ours
fn map_block_handler(process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    let Some(host) = host() else { return false };
    let (handle, address) = (ctx.r[0], ctx.r[1]);
    if host.map_block(process, handle, address) {
        ctx.r[0] = hle::R_OK;
        true
    } else {
        false
    }
}

/// SVC 0x20: UnmapMemoryBlock(handle, addr)
fn unmap_block_handler(_process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    let Some(host) = host() else { return false };
    if host.unmap_block(ctx.r[0]) {
        ctx.r[0] = hle::R_OK;
        true
    } else {
        false
    }
}

/// SVC 0x23: CloseHandle, when the handle is a service session or a file/dir session
fn close_handler(_process: &Process3ds, ctx: &mut Context, _number: u32) -> bool {
    let Some(host) = host() else { return false };
    if host.close_session(ctx.r[0]) {
        ctx.r[0] = hle::R_OK;
        true
    } else {
        false
    }
}
