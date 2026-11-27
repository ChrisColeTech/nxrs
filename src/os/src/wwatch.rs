//! Write watches for debugging (the control pipe's `wpage` / `wtrace`): the watched guest pages are guard pages; each
//! write to them is logged with the guest function that made it (`nxrt::fault::describe_host`) and the value range,
//! then the page is guarded again after that one instruction (single step). At most 600 lines a watch.

//!
//! Linux: a watched page is made read-only instead of a guard page (only writes fault, which is all the watch logs);
//! the write fault logs, opens the page and sets the trap flag; the SIGTRAP after the instruction closes it again.
//! A system call writing a watched page (a read() into it) fails with EFAULT meanwhile. AArch64 Linux has no user-mode
//! single step, so there a page logs its first write and then stays open.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    EXCEPTION_SINGLE_STEP, NTSTATUS, STATUS_GUARD_PAGE_VIOLATION,
};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS,
};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_GUARD,
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
    /// The address a logged write went to: its 16 bytes are logged after the single step (the value written).
    static WROTE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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
    let mut s = format!("thread {name} lr {:#x} frames", c.x[30]);
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

/// After the write's single step: the 16 bytes at the address it wrote.
fn log_value() {
    let a = WROTE.with(|w| w.replace(0));
    if a == 0 {
        return;
    }
    let a = a & !15;
    if nxrt::mem::readable(a, 16) {
        let w: Vec<String> = (0..4u64)
            .map(|i| format!("{:08x}", unsafe { *((a + 4 * i) as *const u32) }))
            .collect();
        crate::log!("wwatch:   {a:#x} now {}", w.join(" "));
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
            let n = LINES.fetch_add(1, Ordering::Relaxed);
            if n < max {
                crate::log!(
                    "wwatch: write to {a:#x} by {}; {}",
                    nxrt::fault::describe_host(f.rip()),
                    writer()
                );
                WROTE.with(|w| w.set(a));
            } else if n == max {
                crate::log!("wwatch: {max} writes logged: the watch stops");
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
        let mut old = 0;
        VirtualProtect(
            page as *mut _,
            PAGE as usize,
            (mbi.Protect & !PAGE_GUARD) | PAGE_GUARD,
            &mut old,
        ) != 0
    }
}

#[cfg(windows)]
unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let rec = &*(*info).ExceptionRecord;
    let ctx = &mut *(*info).ContextRecord;
    if rec.ExceptionCode == STATUS_GUARD_PAGE_VIOLATION as NTSTATUS && rec.NumberParameters >= 2 {
        let a = rec.ExceptionInformation[1] as u64;
        if !watched(a) {
            return CONTINUE_SEARCH;
        }
        let max = max_lines();
        if rec.ExceptionInformation[0] == 1 && exact(a) {
            let n = LINES.fetch_add(1, Ordering::Relaxed);
            if n < max {
                crate::log!(
                    "wwatch: write to {a:#x} by {}; {}",
                    nxrt::fault::describe_host(ctx.Rip),
                    writer()
                );
                WROTE.with(|w| w.set(a));
            } else if n == max {
                crate::log!("wwatch: {max} writes logged: the watch stops");
            }
        }
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
                    && mbi.Protect & PAGE_GUARD != 0
                {
                    let mut old = 0;
                    VirtualProtect(
                        p as *mut _,
                        PAGE as usize,
                        mbi.Protect & !PAGE_GUARD,
                        &mut old,
                    );
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
