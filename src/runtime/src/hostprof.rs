//! The host sampling profiler (Turbo's `SWR_HOST_PROF`, runtime/nvn/host/nvn_host.c, and `tools/host_prof.py`):
//! `SWR_HOST_PROF=<start s>,<seconds>[,<file>]` samples one host thread about 2000 times a second for that window
//! and writes where it was. The thread is found by its name (`SWR_HOST_PROF_THREAD`, default `vk replay`: graphics'
//! command-buffer translation thread; Turbo's `completion` names the submit-completion thread, any other value is
//! matched against the start of the thread names, e.g. a guest thread's).
//!
//! The file keeps Turbo's sample lines, so its `host_prof.py` reads it: `<address> <return address or 0> <module>`,
//! the address relative to this image (module `-`) or to the module it is in, the return address the first stack
//! word pointing into this image's code. After the samples come the tables host_prof.py prints, as `#` lines (that
//! script skips them): by function (lifted guest functions by name, other code by the nearest export below the
//! address), the time in other modules and the function of this image that called into them. Costs nothing unless set.

use std::collections::HashMap;

/// `SWR_HOST_PROF=<start s>,<seconds>[,<file>]` (Turbo's sscanf "%lf,%lf,%[^\n]"; a missing number keeps 0 / 10).
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub start: f64,
    pub seconds: f64,
    pub file: Option<String>,
}

pub fn parse(v: &str) -> Spec {
    let mut it = v.splitn(3, ',');
    let mut s = Spec {
        start: 0.0,
        seconds: 10.0,
        file: None,
    };
    if let Some(x) = it.next().and_then(|x| x.trim().parse().ok()) {
        s.start = x;
        if let Some(y) = it.next().and_then(|y| y.trim().parse().ok()) {
            s.seconds = y;
            s.file = it
                .next()
                .map(|f| f.trim_end_matches(['\r', '\n']).to_string())
                .filter(|f| !f.is_empty());
        }
    }
    s
}

/// The thread name a `SWR_HOST_PROF_THREAD` value stands for.
pub fn thread_name(v: Option<&str>) -> String {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        None => "vk replay".into(),
        Some(v) if v.starts_with('c') && "completion".starts_with(v) => "nvn-completion".into(),
        Some(v) => v.into(),
    }
}

/// One sample: where the thread was, and the first return address into this image found on its stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub ip: u64,
    pub ret: u64,
}

/// What an address is: `own` = in this image (with its name), else the module's path and the name in it.
pub struct Names<'a> {
    pub own: &'a dyn Fn(u64) -> Option<String>,
    pub module: &'a dyn Fn(u64) -> Option<(String, u64, String)>,
}

/// The sample lines and the tables (host_prof.py's three), `image` = this image's base.
pub fn report(head: &str, samples: &[Sample], image: u64, names: &Names, top: usize) -> String {
    let mut s = format!("{head}\n");
    let (mut own, mut outside, mut via, mut outfn): (
        HashMap<String, u64>,
        HashMap<String, u64>,
        HashMap<String, u64>,
        HashMap<String, u64>,
    ) = Default::default();
    let mut n_own = 0u64;
    for sm in samples {
        match (names.own)(sm.ip) {
            Some(name) => {
                s.push_str(&format!(
                    "{:x} {:x} -\n",
                    sm.ip - image,
                    if sm.ret != 0 { sm.ret - image } else { 0 }
                ));
                *own.entry(name).or_default() += 1;
                n_own += 1;
            }
            None => {
                let (path, rel, fname) =
                    (names.module)(sm.ip).unwrap_or_else(|| ("?".into(), sm.ip, "?".into()));
                s.push_str(&format!(
                    "{:x} {:x} {path}\n",
                    rel,
                    if sm.ret != 0 { sm.ret - image } else { 0 }
                ));
                let base = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
                let f = format!("{base}!{fname}");
                *outside.entry(base).or_default() += 1;
                *outfn.entry(f.clone()).or_default() += 1;
                let caller = if sm.ret != 0 {
                    (names.own)(sm.ret).unwrap_or_else(|| "?".into())
                } else {
                    "(no caller found)".into()
                };
                *via.entry(format!("{caller} -> {f}")).or_default() += 1;
            }
        }
    }
    let n = samples.len() as u64;
    let pct = |c: u64| 100.0 * c as f64 / n.max(1) as f64;
    s.push_str(&format!(
        "# {n} samples; in this image {n_own} ({:.1} %), in other modules {}\n",
        pct(n_own),
        n - n_own
    ));
    let mut all = own.clone();
    for (m, c) in &outside {
        *all.entry(format!("[{m}]")).or_default() += c;
    }
    let table = |s: &mut String, title: &str, m: &HashMap<String, u64>, k: usize| {
        s.push_str(&format!("# -- {title}\n"));
        let mut v: Vec<(&String, &u64)> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (name, c) in v.into_iter().take(k) {
            s.push_str(&format!("# {:6.2} %  {c:6}  {name}\n", pct(*c)));
        }
    };
    table(
        &mut s,
        "by function (self time; other modules by name)",
        &all,
        top,
    );
    table(
        &mut s,
        "time in other modules, by the nearest export below the address",
        &outfn,
        top / 2,
    );
    table(
        &mut s,
        "time in other modules, by the function of this image that called",
        &via,
        top / 2,
    );
    s
}

/// Starts the sampler thread when `SWR_HOST_PROF` is set (once).
pub fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(v) = std::env::var("SWR_HOST_PROF")
            .ok()
            .filter(|v| !v.is_empty())
        else {
            return;
        };
        let spec = parse(&v);
        let name = thread_name(std::env::var("SWR_HOST_PROF_THREAD").ok().as_deref());
        let path = spec
            .file
            .clone()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(default_path);
        let _ = std::thread::Builder::new()
            .name("host profiler".into())
            .spawn(move || {
                let t0 = std::time::Instant::now();
                std::thread::sleep(std::time::Duration::from_secs_f64(spec.start.max(0.0)));
                match run(&name, spec.seconds, t0) {
                    Ok(text) => match std::fs::write(&path, text.as_bytes()) {
                        Ok(()) => crate::rt::log(&format!(
                            "host profile: written to {} ({})",
                            path.display(),
                            text.lines().next().unwrap_or("")
                        )),
                        Err(e) => crate::rt::log(&format!(
                            "host profile: cannot write {}: {e}",
                            path.display()
                        )),
                    },
                    Err(e) => crate::rt::log(&format!("host profile: {e}")),
                }
            });
    });
}

fn default_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| "game.exe".into());
    exe.with_extension("hostprof.txt")
}

#[cfg(windows)]
pub use win::{own_name, run};

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HMODULE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetThreadContext, CONTEXT, IMAGE_NT_HEADERS64,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    use windows_sys::Win32::System::SystemServices::IMAGE_DOS_HEADER;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetThreadDescription, GetThreadTimes, OpenThread, ResumeThread,
        SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
        THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
    };

    const CONTEXT_CONTROL: u32 = 0x0010_0001;

    /// The base and code section of the module holding `addr`.
    fn module_of(addr: u64) -> Option<(HMODULE, u64, u64, u64, u64)> {
        let mut m: HMODULE = core::ptr::null_mut();
        // SAFETY: a lookup by address; the module stays loaded (unchanged refcount: the engine's own or a system DLL)
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                addr as *const u16,
                &mut m,
            )
        };
        if ok == 0 {
            return None;
        }
        let base = m as u64;
        // SAFETY: a loaded image's headers
        unsafe {
            let d = &*(base as *const IMAGE_DOS_HEADER);
            let nt = &*((base + d.e_lfanew as u64) as *const IMAGE_NT_HEADERS64);
            let size = nt.OptionalHeader.SizeOfImage as u64;
            let code = base + nt.OptionalHeader.BaseOfCode as u64;
            Some((
                m,
                base,
                base + size,
                code,
                code + nt.OptionalHeader.SizeOfCode as u64,
            ))
        }
    }

    /// The export at or below `addr` in the image at `base` (an approximation, as host_prof.py: internal functions
    /// count for the export before them).
    fn export_below(base: u64, addr: u64) -> String {
        // SAFETY: a loaded image's export directory, bounds from its own headers
        unsafe {
            let d = &*(base as *const IMAGE_DOS_HEADER);
            let nt = &*((base + d.e_lfanew as u64) as *const IMAGE_NT_HEADERS64);
            let dir = nt.OptionalHeader.DataDirectory[0];
            if dir.VirtualAddress == 0 || dir.Size == 0 {
                return format!("+{:x}", addr - base);
            }
            let e = (base + dir.VirtualAddress as u64) as *const u32;
            let (nfun, nname) = (*e.add(5), *e.add(6));
            let (afun, aname, aord) = (
                base + *e.add(7) as u64,
                base + *e.add(8) as u64,
                base + *e.add(9) as u64,
            );
            let rva = addr - base;
            let mut best: Option<(u32, u32)> = None;
            for i in 0..nname as usize {
                let ord = *(aord as *const u16).add(i) as u32;
                if ord >= nfun {
                    continue;
                }
                let f = *(afun as *const u32).add(ord as usize);
                if (f as u64) <= rva && best.is_none_or(|(b, _)| f > b) {
                    best = Some((f, *(aname as *const u32).add(i)));
                }
            }
            match best {
                Some((f, nm)) => {
                    let p = (base + nm as u64) as *const core::ffi::c_char;
                    format!(
                        "{}+{:x}",
                        core::ffi::CStr::from_ptr(p).to_string_lossy(),
                        rva - f as u64
                    )
                }
                None => format!("+{rva:x}"),
            }
        }
    }

    fn own_range() -> (u64, u64, u64, u64) {
        let (_, lo, hi, tlo, thi) = module_of(own_range as *const () as usize as u64).unwrap_or((
            core::ptr::null_mut(),
            0,
            0,
            0,
            0,
        ));
        (lo, hi, tlo, thi)
    }

    /// A name for an address in this image: the lifted guest function it belongs to, else the export below it.
    pub fn own_name(addr: u64) -> Option<String> {
        let (lo, hi, _, _) = own_range();
        if addr < lo || addr >= hi {
            return None;
        }
        if let Some((g, _)) = crate::dispatch::table()
            .and_then(|t| t.guest_of_host(addr))
            .filter(|&(_, d)| d < 0x10_0000)
        {
            return Some(crate::dispatch::guest_name(g));
        }
        Some(export_below(lo, addr))
    }

    fn module_name(addr: u64) -> Option<(String, u64, String)> {
        let (m, base, ..) = module_of(addr)?;
        let mut buf = [0u16; 520];
        // SAFETY: our buffer
        let n = unsafe { GetModuleFileNameW(m, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        Some((
            String::from_utf16_lossy(&buf[..n]),
            addr - base,
            export_below(base, addr),
        ))
    }

    /// The first thread of this process whose name starts with `name`.
    pub fn find_thread(name: &str) -> Option<u32> {
        // SAFETY: a thread snapshot of this process, our own buffers and handles
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap as isize == -1 {
                return None;
            }
            let me = GetCurrentProcessId();
            let mut te: THREADENTRY32 = core::mem::zeroed();
            te.dwSize = core::mem::size_of::<THREADENTRY32>() as u32;
            let mut found = None;
            let mut ok = Thread32First(snap, &mut te) != 0;
            while ok && found.is_none() {
                if te.th32OwnerProcessID == me {
                    let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, te.th32ThreadID);
                    if !h.is_null() {
                        let mut p: *mut u16 = core::ptr::null_mut();
                        if GetThreadDescription(h, &mut p) >= 0 && !p.is_null() {
                            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
                            if String::from_utf16_lossy(core::slice::from_raw_parts(p, len))
                                .starts_with(name)
                            {
                                found = Some(te.th32ThreadID);
                            }
                            windows_sys::Win32::Foundation::LocalFree(p as _);
                        }
                        CloseHandle(h);
                    }
                }
                ok = Thread32Next(snap, &mut te) != 0;
            }
            CloseHandle(snap);
            found
        }
    }

    fn cpu_seconds(h: HANDLE) -> f64 {
        let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) =
            unsafe { core::mem::zeroed() };
        // SAFETY: our thread handle and FILETIMEs
        unsafe { GetThreadTimes(h, &mut c, &mut e, &mut k, &mut u) };
        let ft =
            |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
        ft(k) + ft(u)
    }

    /// Samples thread `name` for `seconds` (waiting up to 10 minutes for it to exist) and returns the file's text.
    pub fn run(name: &str, seconds: f64, t0: std::time::Instant) -> Result<String, String> {
        let tid = loop {
            if let Some(t) = find_thread(name) {
                break t;
            }
            if t0.elapsed().as_secs() > 600 {
                return Err(format!("no thread named {name:?} (SWR_HOST_PROF_THREAD)"));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        // SAFETY: opening a thread of this process
        let th = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                0,
                tid,
            )
        };
        if th.is_null() {
            return Err(format!("cannot open the thread {name:?}"));
        }
        let (lo, hi, tlo, thi) = own_range();
        let mut samples = Vec::with_capacity(1 << 14);
        let cpu0 = cpu_seconds(th);
        let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds.max(0.0));
        while std::time::Instant::now() < end && samples.len() < 1 << 16 {
            // ~0.5 ms, without the 1 ms sleep granularity
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_micros(480) {
                core::hint::spin_loop();
            }
            // SAFETY: suspended, its context read, resumed; nothing else is done while it is stopped (it may hold any lock)
            unsafe {
                if SuspendThread(th) == u32::MAX {
                    break;
                }
                let mut cx: CONTEXT = core::mem::zeroed();
                cx.ContextFlags = CONTEXT_CONTROL;
                if GetThreadContext(th, &mut cx) != 0 {
                    let mut s = Sample { ip: cx.Rip, ret: 0 };
                    if cx.Rip < lo || cx.Rip >= hi {
                        // the first stack word that points into this image's code
                        let sp = cx.Rsp as *const u64;
                        let mut mb: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
                        if VirtualQuery(
                            sp as _,
                            &mut mb,
                            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                        ) != 0
                            && mb.State == MEM_COMMIT
                        {
                            let rend = mb.BaseAddress as u64 + mb.RegionSize as u64;
                            for k in 0..512u64 {
                                if cx.Rsp + (k + 1) * 8 > rend {
                                    break;
                                }
                                let w = *sp.add(k as usize);
                                if w >= tlo && w < thi {
                                    s.ret = w;
                                    break;
                                }
                            }
                        }
                    }
                    samples.push(s);
                }
                ResumeThread(th);
            }
        }
        let cpu = cpu_seconds(th) - cpu0;
        // SAFETY: our handle
        unsafe { CloseHandle(th) };
        let head = format!("# host profile of thread {name:?}: {} samples in {seconds:.1} s, thread CPU {cpu:.2} s, image {lo:x}..{hi:x}", samples.len());
        Ok(report(
            &head,
            &samples,
            lo,
            &Names {
                own: &own_name,
                module: &module_name,
            },
            40,
        ))
    }
}

#[cfg(not(windows))]
pub fn run(_name: &str, _seconds: f64, _t0: std::time::Instant) -> Result<String, String> {
    Err("SWR_HOST_PROF samples a host thread on Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turbo's sscanf("%lf,%lf,%[^\n]") and SWR_HOST_PROF_THREAD.
    #[test]
    fn spec_as_turbo() {
        assert_eq!(
            parse("100,25"),
            Spec {
                start: 100.0,
                seconds: 25.0,
                file: None
            }
        );
        assert_eq!(
            parse("5,2.5,C:\\x\\p.txt\n"),
            Spec {
                start: 5.0,
                seconds: 2.5,
                file: Some("C:\\x\\p.txt".into())
            }
        );
        assert_eq!(
            parse("7"),
            Spec {
                start: 7.0,
                seconds: 10.0,
                file: None
            }
        );
        assert_eq!(
            parse("x"),
            Spec {
                start: 0.0,
                seconds: 10.0,
                file: None
            }
        );
        assert_eq!(thread_name(None), "vk replay");
        assert_eq!(thread_name(Some("completion")), "nvn-completion");
        assert_eq!(thread_name(Some("c")), "nvn-completion");
        assert_eq!(thread_name(Some("MainThread")), "MainThread");
    }

    /// host_prof.py's three tables from Turbo-format samples.
    #[test]
    fn report_tables_as_host_prof_py() {
        let own = |a: u64| {
            (0x1000..0x2000).contains(&a).then(|| {
                if a < 0x1800 {
                    "vk_replay_cmd".to_string()
                } else {
                    "vk_bind".to_string()
                }
            })
        };
        let module = |a: u64| {
            Some((
                "C:\\Windows\\System32\\nvoglv64.dll".to_string(),
                a - 0x9000,
                "vkQueueSubmit+10".to_string(),
            ))
        };
        let names = Names {
            own: &own,
            module: &module,
        };
        let s = [
            Sample { ip: 0x1100, ret: 0 },
            Sample { ip: 0x1200, ret: 0 },
            Sample { ip: 0x1900, ret: 0 },
            Sample {
                ip: 0x9010,
                ret: 0x1110,
            },
        ];
        let r = report("# head", &s, 0x1000, &names, 40);
        let l: Vec<&str> = r.lines().collect();
        assert_eq!(
            &l[..5],
            [
                "# head",
                "100 0 -",
                "200 0 -",
                "900 0 -",
                "10 110 C:\\Windows\\System32\\nvoglv64.dll"
            ]
        );
        assert!(r.contains("# 4 samples; in this image 3 (75.0 %), in other modules 1"));
        assert!(r.contains("#  50.00 %       2  vk_replay_cmd"));
        assert!(r.contains("#  25.00 %       1  [nvoglv64.dll]"));
        assert!(r.contains("#  25.00 %       1  vk_replay_cmd -> nvoglv64.dll!vkQueueSubmit+10"));
    }

    /// A named spinning thread is found and sampled; its samples land in this image.
    #[cfg(windows)]
    #[test]
    fn samples_a_named_thread() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        let h = std::thread::Builder::new()
            .name("hostprof test spinner".into())
            .spawn(move || {
                let mut x = 0u64;
                while !s2.load(std::sync::atomic::Ordering::Relaxed) {
                    x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
                }
                x
            })
            .unwrap();
        let text = run("hostprof test spinner", 0.2, std::time::Instant::now());
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        h.join().unwrap();
        let text = text.unwrap();
        let head = text.lines().next().unwrap();
        assert!(
            head.starts_with("# host profile of thread \"hostprof test spinner\": "),
            "{head}"
        );
        let n: usize = head
            .split(": ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(n > 50, "{head}");
        assert!(
            text.lines().filter(|l| l.ends_with(" -")).count() > n / 2,
            "most samples in this image"
        );
    }
}
