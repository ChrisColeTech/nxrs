//! Write watches for debugging (the control pipe's `wpage` / `wtrace`): the watched guest pages are made read-only;
//! each write to them is logged with the guest function that made it (`nxrt::fault::describe_host`), the guest
//! thread's registers and frames, and the 16 bytes after the write; the page opens for that one instruction (single
//! step) and closes again. At most `NXOS_WWATCH_LINES` (600) lines a watch; `NXOS_WWATCH_CHANGED=1` logs only writes
//! that change the bytes. A write by another thread while a page is open for one write's step is not seen.
//! (Windows used guard pages before MKW-ATLAS: they fault on reads too, and a page read every frame let writes through.)
//!
//! The write fault logs, opens the page and sets the trap flag; the trap after the instruction closes it again.
//! A system call writing a watched page (a read() into it) fails meanwhile. AArch64 Linux has no user-mode single
//! step, so there a page logs its first write and then stays open.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_SINGLE_STEP, NTSTATUS,
};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS,
};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_READONLY, PAGE_READWRITE,
};

const PAGE: u64 = 0x1000;
const MAX_LINES: u32 = 600;

/// Watched ranges (page aligned): (start, end).
static RANGES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
/// The ranges as asked (byte exact): only writes inside them are logged and counted; a write elsewhere on a watched
/// page is let through silently (a table that shares its first page with other, busier data).
static EXACT: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
static LINES: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// The page to guard again after this thread's single step.
    static REARM: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// A write in a watched range, decided after its single step: (16-byte line it went to, those bytes before,
    /// the line naming the writer). It is logged with the bytes after, and with `NXOS_WWATCH_CHANGED=1` only when
    /// they changed (a clear of memory already clear, a rewrite of the same value, is not counted).
    static PENDING: std::cell::RefCell<Option<(u64, [u8; 16], String)>> = const { std::cell::RefCell::new(None) };
}

fn watched(a: u64) -> bool {
    RANGES
        .lock()
        .map(|r| r.iter().any(|&(s, e)| a >= s && a < e))
        .unwrap_or(false)
}

fn exact(a: u64) -> bool {
    EXACT
        .lock()
        .map(|r| r.iter().any(|&(s, e)| a >= s && a < e))
        .unwrap_or(false)
}

/// The most lines a watch logs: `NXOS_WWATCH_LINES` (default 600).
fn max_lines() -> u32 {
    std::env::var("NXOS_WWATCH_LINES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MAX_LINES)
}

/// Who wrote: the host thread's name and, when it runs a guest thread, the guest link register and the return
/// addresses of its frame chain (lock-free: read from the thread's own guest state).
fn writer() -> String {
    let name = std::thread::current()
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| "?".into());
    let Some(c) = nxrt::kernel::current_cpu() else {
        return format!("host thread {name} (no guest thread)");
    };
    let c = unsafe { &*c };
    // x0-x2 as the guest state holds them (a lifted function keeps its live registers in locals, so these are the
    // values at its entry: a memcpy's destination, source and size)
    let mut s = format!(
        "thread {name} x0 {:#x} x1 {:#x} x2 {:#x} lr {:#x} frames",
        c.x[0], c.x[1], c.x[2], c.x[30]
    );
    let mut fp = c.x[29];
    for _ in 0..10 {
        if fp == 0 || fp & 7 != 0 || !nxrt::mem::readable(fp, 16) {
            break;
        }
        let ret = unsafe { *((fp + 8) as *const u64) };
        s.push_str(&format!(" {ret:#x}"));
        fp = unsafe { *(fp as *const u64) };
    }
    s
}

/// Only writes that change the bytes are logged and counted: `NXOS_WWATCH_CHANGED=1`.
fn changed_only() -> bool {
    std::env::var("NXOS_WWATCH_CHANGED").is_ok_and(|v| v == "1")
}

/// The 16 bytes of the aligned line at `a` (zeros when not readable).
fn line16(a: u64) -> [u8; 16] {
    let mut b = [0u8; 16];
    if nxrt::mem::readable(a, 16) {
        unsafe { std::ptr::copy_nonoverlapping(a as *const u8, b.as_mut_ptr(), 16) };
    }
    b
}

/// At a write fault in a watched range: remembers who wrote and the bytes before, for [`log_value`].
fn note_write(a: u64, host: String) {
    if LINES.load(Ordering::Relaxed) > max_lines() {
        return;
    }
    let at = a & !15;
    let line = format!("wwatch: write to {a:#x} by {host}; {}", writer());
    PENDING.with(|p| *p.borrow_mut() = Some((at, line16(at), line)));
}

/// After the write's single step: logs it with the 16 bytes now there (only when they changed, with
/// `NXOS_WWATCH_CHANGED=1`).
fn log_value() {
    let Some((at, before, line)) = PENDING.with(|p| p.borrow_mut().take()) else {
        return;
    };
    let now = line16(at);
    if changed_only() && now == before {
        return;
    }
    let max = max_lines();
    let n = LINES.fetch_add(1, Ordering::Relaxed);
    if n < max {
        let w: Vec<String> = now
            .chunks(4)
            .map(|c| format!("{:08x}", u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect();
        crate::log!("{line}");
        crate::log!("wwatch:   {at:#x} now {}", w.join(" "));
    } else if n == max {
        crate::log!("wwatch: {max} writes logged: the watch stops");
    }
}

// the web never arms a page (`arm` says so), so these only keep the shared code whole there
#[cfg(target_os = "emscripten")]
fn guard(_page: u64) -> bool {
    false
}
#[cfg(target_os = "emscripten")]
fn unguard(_page: u64) {}

#[cfg(target_os = "linux")]
fn guard(page: u64) -> bool {
    // committed read-write pages only (a read-only page faults on writes by itself)
    matches!(nxrt::linux::query(page), Some((true, 2, _, _)))
        && nxrt::mem::protect(page, PAGE, nxrt::mem::Prot::Read)
}

#[cfg(target_os = "linux")]
fn unguard(page: u64) {
    if matches!(nxrt::linux::query(page), Some((true, 1, _, _))) {
        nxrt::mem::protect(page, PAGE, nxrt::mem::Prot::ReadWrite);
    }
}

#[cfg(target_os = "linux")]
unsafe fn hook(f: &mut nxrt::linux::Fault) -> bool {
    if f.sig == libc::SIGSEGV && f.write {
        let a = f.addr;
        if !watched(a) {
            return false;
        }
        let max = max_lines();
        if exact(a) {
            note_write(a, nxrt::fault::describe_host(f.rip()));
            if !nxrt::linux::CAN_SINGLE_STEP {
                log_value(); // no step to wait for: logged now, with the bytes before the write
            }
        }
        let page = a & !(PAGE - 1);
        nxrt::mem::protect(page, PAGE, nxrt::mem::Prot::ReadWrite);
        if !nxrt::linux::CAN_SINGLE_STEP {
            // AArch64: no single step to close the page after this write, so it stays open: its first write is the
            // only one logged
            crate::log!(
                "wwatch: page {page:#x} is no longer watched (no single step on this host)"
            );
        } else if LINES.load(Ordering::Relaxed) <= max {
            REARM.with(|r| r.set(page));
            f.set_trap(true); // trap after this instruction
        }
        return true;
    }
    if f.sig == libc::SIGTRAP {
        let p = REARM.with(|r| r.replace(0));
        if p == 0 {
            return false;
        }
        f.set_trap(false);
        log_value();
        guard(p);
        return true;
    }
    false
}

#[cfg(windows)]
fn guard(page: u64) -> bool {
    unsafe {
        let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        if VirtualQuery(
            page as *const _,
            &mut mbi,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        ) == 0
            || mbi.State != 0x1000
        {
            return false; // not committed
        }
        // read-write pages only (a read-only page faults on writes by itself); made read-only, so only writes fault:
        // a guard page faulted on reads too, and a page opened for one reader's single step let other threads'
        // writes through unseen (a table read every frame hid its writes)
        if mbi.Protect != PAGE_READWRITE {
            return false;
        }
        let mut old = 0;
        VirtualProtect(page as *mut _, PAGE as usize, PAGE_READONLY, &mut old) != 0
    }
}

/// Opens a watched page for one write (its single step).
#[cfg(windows)]
fn open(page: u64) {
    let mut old = 0;
    unsafe { VirtualProtect(page as *mut _, PAGE as usize, PAGE_READWRITE, &mut old) };
}

#[cfg(windows)]
unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let rec = &*(*info).ExceptionRecord;
    let ctx = &mut *(*info).ContextRecord;
    if rec.ExceptionCode == EXCEPTION_ACCESS_VIOLATION as NTSTATUS
        && rec.NumberParameters >= 2
        && rec.ExceptionInformation[0] == 1
    {
        let a = rec.ExceptionInformation[1] as u64;
        if !watched(a) {
            return CONTINUE_SEARCH;
        }
        let max = max_lines();
        if exact(a) {
            note_write(a, nxrt::fault::describe_host(ctx.Rip));
        }
        open(a & !(PAGE - 1));
        if LINES.load(Ordering::Relaxed) <= max {
            REARM.with(|r| r.set(a & !(PAGE - 1)));
            ctx.EFlags |= 0x100; // trap after this instruction
        }
        return CONTINUE_EXECUTION;
    }
    if rec.ExceptionCode == EXCEPTION_SINGLE_STEP as NTSTATUS {
        let p = REARM.with(|r| r.replace(0));
        if p == 0 {
            return CONTINUE_SEARCH;
        }
        log_value();
        guard(p);
        return CONTINUE_EXECUTION;
    }
    CONTINUE_SEARCH
}

/// Watches the guest range [lo, hi) for writes (its committed pages; a page committed later is not watched).
pub fn arm(lo: u64, hi: u64) -> String {
    // the diagnostic guards pages and catches the fault of each write: neither exists in wasm
    #[cfg(target_os = "emscripten")]
    {
        let _ = (lo, hi);
        return "wwatch: not available in a web build (no page protection)\n".into();
    }
    #[cfg(not(target_os = "emscripten"))]
    static HANDLER: std::sync::Once = std::sync::Once::new();
    #[cfg(windows)]
    HANDLER.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(veh));
    });
    #[cfg(target_os = "linux")]
    HANDLER.call_once(|| nxrt::linux::add_fault_hook(hook));
    let (from, to) = (lo, hi.max(lo + 1));
    let (lo, hi) = (lo & !(PAGE - 1), (hi + PAGE - 1) & !(PAGE - 1));
    if hi <= lo || hi - lo > 64 << 20 {
        return "wwatch: a range of 1 page to 64 MB\n".into();
    }
    LINES.store(0, Ordering::Relaxed);
    EXACT.lock().unwrap().push((from, to));
    RANGES.lock().unwrap().push((lo, hi));
    let mut n = 0;
    let mut p = lo;
    while p < hi {
        n += guard(p) as u32;
        p += PAGE;
    }
    let max = max_lines();
    crate::log!("wwatch: {from:#x}..{to:#x} armed ({n} committed pages); each write goes in the log, up to {max}");
    format!("armed {n} pages: each write in {from:#x}..{to:#x} goes in the log, up to {max}\n")
}

/// Stops every watch: the pages lose their guard.
pub fn disarm() -> String {
    EXACT.lock().unwrap().clear();
    let ranges = std::mem::take(&mut *RANGES.lock().unwrap());
    for (lo, hi) in ranges {
        let mut p = lo;
        while p < hi {
            #[cfg(target_os = "linux")]
            unguard(p);
            #[cfg(windows)]
            unsafe {
                let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                if VirtualQuery(
                    p as *const _,
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                ) != 0
                    && mbi.Protect == PAGE_READONLY
                {
                    open(p);
                }
            }
            p += PAGE;
        }
    }
    "watches cleared\n".into()
}

#[cfg(test)]
mod tests {
    /// A watched page reports a write, keeps working and stays watched for the next one.
    #[test]
    fn writes_are_seen_and_the_page_rearmed() {
        // a page of its own (a heap page would be shared with other tests' allocations)
        #[cfg(windows)]
        let base = {
            use windows_sys::Win32::System::Memory::{
                VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE,
            };
            unsafe {
                VirtualAlloc(
                    std::ptr::null(),
                    0x1000,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                ) as u64
            }
        };
        #[cfg(target_os = "linux")]
        let base = {
            let a = nxrt::mem::reserve_anywhere(0x1000, false).unwrap();
            assert!(nxrt::mem::commit(a, 0x1000));
            a
        };
        super::arm(base, base + 0x1000);
        let before = super::LINES.load(super::Ordering::Relaxed);
        unsafe {
            std::ptr::write_volatile(base as *mut u64, 7);
            std::ptr::write_volatile((base + 8) as *mut u64, 9);
        }
        let after = super::LINES.load(super::Ordering::Relaxed);
        assert_eq!(unsafe { std::ptr::read_volatile(base as *const u64) }, 7);
        super::disarm();
        assert!(
            after >= before + 2,
            "two writes logged: {before} -> {after}"
        );
        // only the asked bytes are logged: a write elsewhere on the watched page goes through uncounted
        super::arm(base + 0x100, base + 0x110);
        let before = super::LINES.load(super::Ordering::Relaxed);
        unsafe {
            std::ptr::write_volatile(base as *mut u64, 1);
            std::ptr::write_volatile((base + 0x108) as *mut u64, 2);
        }
        let after = super::LINES.load(super::Ordering::Relaxed);
        super::disarm();
        assert_eq!(unsafe { std::ptr::read_volatile(base as *const u64) }, 1);
        assert_eq!(after, before + 1, "one write inside the range");
    }
}
