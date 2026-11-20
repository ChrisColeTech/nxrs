//! Write watches for debugging (Turbo vk_ctl.c `wpage` / `wtrace`): the watched guest pages are guard pages; each
//! write to them is logged with the guest function that made it (`nxrt::fault::describe_host`) and the value range,
//! then the page is guarded again after that one instruction (single step). At most 600 lines a watch.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{EXCEPTION_SINGLE_STEP, NTSTATUS, STATUS_GUARD_PAGE_VIOLATION};
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows_sys::Win32::System::Memory::{VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_GUARD};

const PAGE: u64 = 0x1000;
const MAX_LINES: u32 = 600;

/// Watched ranges (page aligned): (start, end).
static RANGES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
static LINES: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// The page to guard again after this thread's single step.
    static REARM: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn watched(a: u64) -> bool {
    RANGES.lock().map(|r| r.iter().any(|&(s, e)| a >= s && a < e)).unwrap_or(false)
}

fn guard(page: u64) -> bool {
    unsafe {
        let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
        if VirtualQuery(page as *const _, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) == 0 || mbi.State != 0x1000 {
            return false; // not committed
        }
        let mut old = 0;
        VirtualProtect(page as *mut _, PAGE as usize, (mbi.Protect & !PAGE_GUARD) | PAGE_GUARD, &mut old) != 0
    }
}

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
        if rec.ExceptionInformation[0] == 1 {
            let n = LINES.fetch_add(1, Ordering::Relaxed);
            if n < MAX_LINES {
                crate::log!("wwatch: write to {a:#x} by {}", nxrt::fault::describe_host(ctx.Rip));
            } else if n == MAX_LINES {
                crate::log!("wwatch: {MAX_LINES} writes logged: the watch stops");
            }
        }
        if LINES.load(Ordering::Relaxed) <= MAX_LINES {
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
        guard(p);
        return CONTINUE_EXECUTION;
    }
    CONTINUE_SEARCH
}

/// Watches the guest range [lo, hi) for writes (its committed pages; a page committed later is not watched).
pub fn arm(lo: u64, hi: u64) -> String {
    static HANDLER: std::sync::Once = std::sync::Once::new();
    HANDLER.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(veh));
    });
    let (lo, hi) = (lo & !(PAGE - 1), (hi + PAGE - 1) & !(PAGE - 1));
    if hi <= lo || hi - lo > 64 << 20 {
        return "wwatch: a range of 1 page to 64 MB\n".into();
    }
    LINES.store(0, Ordering::Relaxed);
    RANGES.lock().unwrap().push((lo, hi));
    let mut n = 0;
    let mut p = lo;
    while p < hi {
        n += guard(p) as u32;
        p += PAGE;
    }
    crate::log!("wwatch: {lo:#x}..{hi:#x} armed ({n} committed pages); each write goes in the log, up to {MAX_LINES}");
    format!("armed {n} pages: each write goes in the log, up to {MAX_LINES}\n")
}

/// Stops every watch: the pages lose their guard.
pub fn disarm() -> String {
    let ranges = std::mem::take(&mut *RANGES.lock().unwrap());
    for (lo, hi) in ranges {
        let mut p = lo;
        while p < hi {
            unsafe {
                let mut mbi: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                if VirtualQuery(p as *const _, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) != 0 && mbi.Protect & PAGE_GUARD != 0 {
                    let mut old = 0;
                    VirtualProtect(p as *mut _, PAGE as usize, mbi.Protect & !PAGE_GUARD, &mut old);
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
        use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};
        // a page of its own (a heap page would be shared with other tests' allocations)
        let base = unsafe { VirtualAlloc(std::ptr::null(), 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) } as u64;
        super::arm(base, base + 0x1000);
        let before = super::LINES.load(super::Ordering::Relaxed);
        unsafe {
            std::ptr::write_volatile(base as *mut u64, 7);
            std::ptr::write_volatile((base + 8) as *mut u64, 9);
        }
        let after = super::LINES.load(super::Ordering::Relaxed);
        assert_eq!(unsafe { std::ptr::read_volatile(base as *const u64) }, 7);
        super::disarm();
        assert!(after >= before + 2, "two writes logged: {before} -> {after}");
    }
}
