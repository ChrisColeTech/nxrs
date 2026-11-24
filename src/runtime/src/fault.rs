//! The crash reporter: a vectored exception handler that turns a host fault inside lifted code (or the runtime) into a
//! guest report: the faulting host address mapped back to the guest function it was lifted from, the host stack
//! walked with the image's unwind tables (each return address mapped back the same way), the guest registers and the
//! guest frame-pointer chain. Faults in other modules (drivers) are left alone.

use std::io::Write;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_ILLEGAL_INSTRUCTION, EXCEPTION_INT_DIVIDE_BY_ZERO,
    EXCEPTION_PRIV_INSTRUCTION, EXCEPTION_STACK_OVERFLOW, HMODULE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, RtlLookupFunctionEntry, RtlVirtualUnwind, CONTEXT,
    EXCEPTION_POINTERS, UNW_FLAG_NHANDLER,
};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};

#[cfg(windows)]
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
#[cfg(windows)]
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;

/// The guest function a host address belongs to: exact through the PE function table when the address has unwind
/// info, else the nearest lifted function below it.
#[cfg(windows)]
pub fn describe_host(rip: u64) -> String {
    let Some(t) = crate::dispatch::table() else {
        return format!("{rip:#x}");
    };
    let mut base = 0u64;
    let f = unsafe { RtlLookupFunctionEntry(rip, &mut base, core::ptr::null_mut()) };
    if !f.is_null() {
        let start = base + unsafe { (*f).BeginAddress } as u64;
        if let Some(g) = t.guest_of_fn(start) {
            return format!(
                "{rip:#x} = lifted {} +{:#x}",
                crate::dispatch::guest_name(g),
                rip - start
            );
        }
        return format!("{rip:#x} (host code)");
    }
    match t.guest_of_host(rip) {
        Some((g, d)) if d < 0x10_0000 => format!(
            "{rip:#x} = near lifted {} +{d:#x}",
            crate::dispatch::guest_name(g)
        ),
        _ => format!("{rip:#x}"),
    }
}

/// "name.dll+0x1234" for an address in a loaded module (the foreign-fault lines).
#[cfg(windows)]
fn module_name(a: u64) -> String {
    let Some(m) = module_of(a) else {
        return format!("{a:#x}");
    };
    let mut buf = [0u16; 260];
    let n = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(m, buf.as_mut_ptr(), 260)
    } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
    format!("{name}+{:#x}", a - m as u64)
}

#[cfg(windows)]
fn module_of(a: u64) -> Option<HMODULE> {
    let mut m: HMODULE = core::ptr::null_mut();
    let flags =
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    (unsafe { GetModuleHandleExW(flags, a as *const u16, &mut m) } != 0).then_some(m)
}

/// A fault in the image the runtime is linked into (the game's code DLL, or a test executable), or at no module at
/// all (a wild jump).
#[cfg(windows)]
fn ours(rip: u64) -> bool {
    match module_of(rip) {
        None => true,
        Some(m) => Some(m) == module_of(install as *const () as u64),
    }
}

#[cfg(windows)]
unsafe extern "system" fn handler(ep: *mut EXCEPTION_POINTERS) -> i32 {
    crate::boundary::guard("the crash reporter", || report(ep))
}

#[cfg(windows)]
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
        if std::env::var("NXRT_FOREIGN_FAULTS").is_ok_and(|v| v == "1")
            && N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8
        {
            let mut s = format!("fault in another module ({what}) at {}", module_name(rip));
            if er.NumberParameters >= 2 {
                s.push_str(&format!(
                    " (access {} of {:#x})",
                    er.ExceptionInformation[0], er.ExceptionInformation[1]
                ));
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
                    RtlVirtualUnwind(
                        UNW_FLAG_NHANDLER,
                        base,
                        c.Rip,
                        f,
                        &mut c,
                        &mut hd,
                        &mut est,
                        core::ptr::null_mut(),
                    );
                }
                if c.Rip == 0 {
                    break;
                }
                s.push_str(&format!(
                    "
  frame {n:2}: {} {}",
                    module_name(c.Rip),
                    describe_host(c.Rip)
                ));
            }
            crate::rt::log(&s);
        }
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let _ = std::io::stdout().flush();
    let mut s = format!("FATAL: host {what} at {}", describe_host(rip));
    if er.ExceptionCode == EXCEPTION_ACCESS_VIOLATION && er.NumberParameters >= 2 {
        let kind = ["read", "write", "?", "?", "?", "?", "?", "?", "execute"]
            .get(er.ExceptionInformation[0])
            .copied()
            .unwrap_or("?");
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
                RtlVirtualUnwind(
                    UNW_FLAG_NHANDLER,
                    base,
                    c.Rip,
                    f,
                    &mut c,
                    &mut hd,
                    &mut est,
                    core::ptr::null_mut(),
                );
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
    write_minidump(ep);
    let _ = std::io::stderr().flush();
    std::process::exit(70);
}

/// Which minidump a crash writes (`NXRT_MINIDUMP`): `0` none, `full` with the memory the threads' stacks point to,
/// anything else (the default) the threads, their stacks and the modules: small enough to send.
pub fn minidump_flags(v: Option<&str>) -> Option<u32> {
    // MiniDumpNormal 0, WithUnloadedModules 0x20, WithIndirectlyReferencedMemory 0x40, WithThreadInfo 0x1000
    match v.map(str::trim) {
        Some("0") | Some("off") => None,
        Some("full") => Some(0x20 | 0x40 | 0x1000),
        _ => Some(0x20 | 0x1000),
    }
}

/// The crash's minidump beside the executable (`<name>.dmp`, replaced by the next crash), for a debugger with the
/// game's code DLL.
#[cfg(windows)]
unsafe fn write_minidump(ep: *mut EXCEPTION_POINTERS) {
    let Some(flags) = minidump_flags(std::env::var("NXRT_MINIDUMP").ok().as_deref()) else {
        return;
    };
    let path = std::env::current_exe()
        .map(|e| e.with_extension("dmp"))
        .unwrap_or_else(|_| "crash.dmp".into());
    if dump_to(&path, flags, ep) {
        crate::rt::log(&format!(
            "minidump written: {} (NXRT_MINIDUMP=0 turns it off, =full adds referenced memory)",
            path.display()
        ));
    }
}

/// A minidump of this process into `path` (`ep`: the exception, or null); whether it was written.
#[cfg(windows)]
pub unsafe fn dump_to(path: &std::path::Path, flags: u32, ep: *mut EXCEPTION_POINTERS) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        MiniDumpWriteDump, MINIDUMP_EXCEPTION_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
    };
    let Ok(f) = std::fs::File::create(path) else {
        return false;
    };
    let info = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: GetCurrentThreadId(),
        ExceptionPointers: ep,
        ClientPointers: 0,
    };
    let pinfo = if ep.is_null() {
        core::ptr::null()
    } else {
        &info as *const _
    };
    let ok = MiniDumpWriteDump(
        GetCurrentProcess(),
        GetCurrentProcessId(),
        f.as_raw_handle() as _,
        flags as _,
        pinfo,
        core::ptr::null(),
        core::ptr::null(),
    );
    drop(f);
    if ok == 0 {
        let _ = std::fs::remove_file(path);
    }
    ok != 0
}

#[cfg(windows)]
pub fn install() {
    unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
}

/// The guest function a host address belongs to: the nearest lifted function below it (the image's symbols are
/// hidden, and generated code keeps no frame pointers, so the dispatch table is the map).
#[cfg(target_os = "linux")]
pub fn describe_host(rip: u64) -> String {
    let Some(t) = crate::dispatch::table() else {
        return format!("{rip:#x}");
    };
    if let Some(g) = t.guest_of_fn(rip) {
        return format!("{rip:#x} = lifted {}", crate::dispatch::guest_name(g));
    }
    match t.guest_of_host(rip) {
        Some((g, d)) if d < 0x10_0000 => format!(
            "{rip:#x} = near lifted {} +{d:#x}",
            crate::dispatch::guest_name(g)
        ),
        _ => match crate::linux::module_of(rip) {
            Some((m, base)) => format!("{rip:#x} ({m}+{:#x})", rip - base),
            None => format!("{rip:#x}"),
        },
    }
}

/// A fault in the image the runtime is linked into (the game's code library, or a test executable), or at no module
/// at all (a wild jump).
#[cfg(target_os = "linux")]
fn ours(rip: u64) -> bool {
    match crate::linux::module_of(rip) {
        None => true,
        Some((_, base)) => {
            Some(base) == crate::linux::module_of(install as *const () as u64).map(|m| m.1)
        }
    }
}

#[cfg(target_os = "linux")]
unsafe fn hook(f: &mut crate::linux::Fault) -> bool {
    crate::boundary::guard("the crash reporter", || report(f))
}

#[cfg(target_os = "linux")]
unsafe fn report(f: &mut crate::linux::Fault) -> bool {
    if f.sig == libc::SIGTRAP {
        return false;
    }
    if f.sig == libc::SIGSEGV {
        // an untouched page of a lazily committed heap: commit it and retry, from any code (drivers too)
        if let Some(k) = crate::kernel::try_get() {
            if k.space.lazy_fault(f.addr) {
                return true;
            }
        }
    }
    let what = f.what();
    let rip = f.rip();
    if !ours(rip) {
        // `NXRT_FOREIGN_FAULTS=1` (debugging): a fault in another library (libc's memcpy under host code, a driver)
        // is left to the default action, but its first lines are logged with the caller chain
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if std::env::var("NXRT_FOREIGN_FAULTS").is_ok_and(|v| v == "1")
            && N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8
        {
            let m = crate::linux::module_of(rip)
                .map(|(m, b)| format!("{m}+{:#x}", rip - b))
                .unwrap_or_else(|| format!("{rip:#x}"));
            let mut s = format!("fault in another module ({what}) at {m}");
            if f.sig == libc::SIGSEGV || f.sig == libc::SIGBUS {
                s.push_str(&format!(
                    " ({} of {:#x})",
                    if f.write { "write" } else { "read" },
                    f.addr
                ));
            }
            for (n, a) in crate::linux::host_backtrace(24)
                .into_iter()
                .enumerate()
                .skip(2)
            {
                s.push_str(&format!("\n  frame {n:2}: {}", describe_host(a)));
            }
            crate::rt::log(&s);
        }
        return false;
    }
    let _ = std::io::stdout().flush();
    let mut s = format!("FATAL: host {what} at {}", describe_host(rip));
    if f.sig == libc::SIGSEGV || f.sig == libc::SIGBUS {
        let kind = if f.exec {
            "execute"
        } else if f.write {
            "write"
        } else {
            "read"
        };
        s.push_str(&format!(" ({kind} of {:#x})", f.addr));
    }
    s.push('\n');
    // the frames below the signal frame: glibc's unwinder (.eh_frame) through the handler into the faulting code
    let frames = crate::linux::host_backtrace(40);
    let at = frames
        .iter()
        .position(|&a| a == rip || a == rip + 1)
        .map(|i| i + 1)
        .unwrap_or(0);
    s.push_str(&format!("  host frame  0: {}\n", describe_host(rip)));
    for (n, a) in frames.into_iter().skip(at).take(24).enumerate() {
        s.push_str(&format!("  host frame {:2}: {}\n", n + 1, describe_host(a)));
    }
    if let Some(c) = crate::kernel::current_cpu() {
        s.push_str("guest state of this thread:\n");
        s.push_str(&crate::rt::guest_report(&*c));
    }
    crate::rt::log(&s);
    let _ = std::io::stderr().flush();
    std::process::exit(70);
}

#[cfg(target_os = "linux")]
pub fn install() {
    crate::linux::add_fault_hook(hook);
}

#[cfg(test)]
mod tests {
    /// NXRT_MINIDUMP: off, the default (threads and modules), full (with referenced memory).
    #[test]
    fn minidump_flags_by_setting() {
        assert_eq!(super::minidump_flags(Some("0")), None);
        assert_eq!(super::minidump_flags(None), Some(0x1020));
        assert_eq!(super::minidump_flags(Some("full")), Some(0x1060));
    }

    /// A minidump of the test process is written and starts with the MDMP signature.
    #[cfg(windows)]
    #[test]
    fn minidump_is_written() {
        let p = std::env::temp_dir().join(format!("nxrt-minidump-{}.dmp", std::process::id()));
        assert!(unsafe { super::dump_to(&p, 0x1020, core::ptr::null_mut()) });
        let head = std::fs::read(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(&head[..4], b"MDMP");
    }
}
