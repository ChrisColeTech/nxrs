//! The crash reporter: a vectored exception handler that turns a host fault inside lifted code (or the runtime) into a
//! guest report: the faulting host address mapped back to the guest function it was lifted from, the host stack
//! walked with the image's unwind tables (each return address mapped back the same way), the guest registers and the
//! guest frame-pointer chain. Faults in other modules (drivers) are left alone.

use std::io::Write;
use windows_sys::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_ILLEGAL_INSTRUCTION, EXCEPTION_INT_DIVIDE_BY_ZERO, EXCEPTION_PRIV_INSTRUCTION,
    EXCEPTION_STACK_OVERFLOW, HMODULE,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, RtlLookupFunctionEntry, RtlVirtualUnwind, CONTEXT, EXCEPTION_POINTERS, UNW_FLAG_NHANDLER,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;

/// The guest function a host address belongs to: exact through the PE function table when the address has unwind
/// info, else the nearest lifted function below it.
pub fn describe_host(rip: u64) -> String {
    let Some(t) = crate::dispatch::table() else { return format!("{rip:#x}") };
    let mut base = 0u64;
    let f = unsafe { RtlLookupFunctionEntry(rip, &mut base, core::ptr::null_mut()) };
    if !f.is_null() {
        let start = base + unsafe { (*f).BeginAddress } as u64;
        if let Some(g) = t.guest_of_fn(start) {
            return format!("{rip:#x} = lifted {} +{:#x}", crate::dispatch::guest_name(g), rip - start);
        }
        return format!("{rip:#x} (host code)");
    }
    match t.guest_of_host(rip) {
        Some((g, d)) if d < 0x10_0000 => format!("{rip:#x} = near lifted {} +{d:#x}", crate::dispatch::guest_name(g)),
        _ => format!("{rip:#x}"),
    }
}

/// "name.dll+0x1234" for an address in a loaded module (the foreign-fault lines).
fn module_name(a: u64) -> String {
    let Some(m) = module_of(a) else { return format!("{a:#x}") };
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(m, buf.as_mut_ptr(), 260) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
    format!("{name}+{:#x}", a - m as u64)
}

fn module_of(a: u64) -> Option<HMODULE> {
    let mut m: HMODULE = core::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    (unsafe { GetModuleHandleExW(flags, a as *const u16, &mut m) } != 0).then_some(m)
}

/// A fault in the image the runtime is linked into (the game's code DLL, or a test executable), or at no module at
/// all (a wild jump).
fn ours(rip: u64) -> bool {
    match module_of(rip) {
        None => true,
        Some(m) => Some(m) == module_of(install as *const () as u64),
    }
}

unsafe extern "system" fn handler(ep: *mut EXCEPTION_POINTERS) -> i32 {
    crate::boundary::guard("the crash reporter", || report(ep))
}

unsafe fn report(ep: *mut EXCEPTION_POINTERS) -> i32 {
    let er = &*(*ep).ExceptionRecord;
    let what = match er.ExceptionCode {
        EXCEPTION_ACCESS_VIOLATION => "access violation",
        EXCEPTION_ILLEGAL_INSTRUCTION => "illegal instruction",
        EXCEPTION_INT_DIVIDE_BY_ZERO => "integer division by zero",
        EXCEPTION_PRIV_INSTRUCTION => "privileged instruction",
        EXCEPTION_STACK_OVERFLOW => "stack overflow",
        _ => return EXCEPTION_CONTINUE_SEARCH,
    };
    if er.ExceptionCode == EXCEPTION_ACCESS_VIOLATION && er.NumberParameters >= 2 {
        // an untouched page of a lazily committed heap: commit it and retry, from any code (drivers too)
        if let Some(k) = crate::kernel::try_get() {
            if k.space.lazy_fault(er.ExceptionInformation[1] as u64) {
                return EXCEPTION_CONTINUE_EXECUTION;
            }
        }
    }
    let ctx = &*(*ep).ContextRecord;
    let rip = ctx.Rip;
    if !ours(rip) {
        // `NXRT_FOREIGN_FAULTS=1` (debugging): a fault in another module (a system DLL's memcpy under host code, a
        // driver) is left to its handlers, but its first lines are logged with the caller chain, since the process may
        // then die with no report (0xc0000005)
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if std::env::var("NXRT_FOREIGN_FAULTS").is_ok_and(|v| v == "1") && N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
            let mut s = format!("fault in another module ({what}) at {}", module_name(rip));
            if er.NumberParameters >= 2 {
                s.push_str(&format!(" (access {} of {:#x})", er.ExceptionInformation[0], er.ExceptionInformation[1]));
            }
            let mut c: CONTEXT = *ctx;
            for n in 0..16 {
                let mut base = 0u64;
                let f = RtlLookupFunctionEntry(c.Rip, &mut base, core::ptr::null_mut());
                if f.is_null() {
                    if c.Rsp == 0 || !crate::mem::readable(c.Rsp, 8) {
                        break;
                    }
                    c.Rip = *(c.Rsp as *const u64);
                    c.Rsp += 8;
                } else {
                    let mut hd = core::ptr::null_mut();
                    let mut est = 0u64;
                    RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, c.Rip, f, &mut c, &mut hd, &mut est, core::ptr::null_mut());
                }
                if c.Rip == 0 {
                    break;
                }
                s.push_str(&format!("
  frame {n:2}: {} {}", module_name(c.Rip), describe_host(c.Rip)));
            }
            crate::rt::log(&s);
        }
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let _ = std::io::stdout().flush();
    let mut s = format!("FATAL: host {what} at {}", describe_host(rip));
    if er.ExceptionCode == EXCEPTION_ACCESS_VIOLATION && er.NumberParameters >= 2 {
        let kind = ["read", "write", "?", "?", "?", "?", "?", "?", "execute"].get(er.ExceptionInformation[0]).copied().unwrap_or("?");
        s.push_str(&format!(" ({kind} of {:#x})", er.ExceptionInformation[1]));
    }
    s.push('\n');
    if er.ExceptionCode != EXCEPTION_STACK_OVERFLOW {
        let mut c: CONTEXT = *ctx;
        for n in 0..24 {
            let mut base = 0u64;
            let f = RtlLookupFunctionEntry(c.Rip, &mut base, core::ptr::null_mut());
            if f.is_null() {
                if c.Rsp == 0 || !crate::mem::readable(c.Rsp, 8) {
                    break;
                }
                c.Rip = *(c.Rsp as *const u64); // a leaf: the return address is at rsp
                c.Rsp += 8;
            } else {
                let mut hd = core::ptr::null_mut();
                let mut est = 0u64;
                RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, c.Rip, f, &mut c, &mut hd, &mut est, core::ptr::null_mut());
            }
            if c.Rip == 0 {
                break;
            }
            s.push_str(&format!("  host frame {n:2}: {}\n", describe_host(c.Rip)));
        }
    }
    if let Some(c) = crate::kernel::current_cpu() {
        s.push_str("guest state of this thread:\n");
        s.push_str(&crate::rt::guest_report(&*c));
    }
    crate::rt::log(&s);
    let _ = std::io::stderr().flush();
    std::process::exit(70);
}

pub fn install() {
    unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
}
