//! The host sampling profiler:
//! `NXRS_HOST_PROF=<start s>,<seconds>[,<file>]` samples one host thread about 2000 times a second for that window
//! and writes where it was. The thread is found by its name (`NXRS_HOST_PROF_THREAD`, default `vk replay`: graphics'
//! command-buffer translation thread; `completion` names the submit-completion thread, any other value is
//! matched against the start of the thread names, e.g. a guest thread's). Several names, comma-separated, and every
//! thread whose name starts with one of them is sampled (`vk replay,vk translate`: the replay thread and its
//! translation lanes).
//!
//! The file holds one sample a line, as `host_prof.py` reads it: `<address> <return address or 0> <module>`,
//! the address relative to this image (module `-`) or to the module it is in, the return address the first stack
//! word pointing into this image's code. With `NXRS_HOST_PROF_STACK=1` the line goes on with ` | ` and the return
//! addresses into this image of the unwound stack (relative to it, innermost first, up to 32 frames).
//! A sampled thread is stopped only to read its context and copy its stack's top (4 KB, 64 KB with stacks) into a
//! buffer made before; the unwinding and every allocation happen after it runs again (it may hold the heap's or the
//! loader's lock while stopped). Threads that start later are taken in (the list is read again every 250 ms); the
//! sampler's own thread is never one of them. The head names each thread sampled and its sample count.
//! After the samples come the tables host_prof.py prints, as `#` lines (that
//! script skips them): by function (lifted guest functions by name, other code by the nearest export below the
//! address), the time in other modules and the function of this image that called into them. Costs nothing unless set.

use std::collections::HashMap;

/// `NXRS_HOST_PROF=<start s>,<seconds>[,<file>]` (as sscanf "%lf,%lf,%[^\n]"; a missing number keeps 0 / 10).
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

/// The thread name a `NXRS_HOST_PROF_THREAD` value stands for.
pub fn thread_name(v: Option<&str>) -> String {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        None => "vk replay".into(),
        Some(v) if v.starts_with('c') && "completion".starts_with(v) => "nvn-completion".into(),
        Some(v) => v.into(),
    }
}

/// The most return addresses kept for one sample's stack.
pub const MAX_DEPTH: usize = 32;

/// One sample: where the thread was, and the first return address into this image found on its stack. Its unwound
/// stack (`NXRS_HOST_PROF_STACK=1`), if any, is `stacks[first..first + depth]` of the side list the sampler keeps, so
/// a sample without one costs 24 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub ip: u64,
    pub ret: u64,
    pub first: u32,
    pub depth: u8,
}

impl Sample {
    pub fn at(ip: u64, ret: u64) -> Sample {
        Sample {
            ip,
            ret,
            first: 0,
            depth: 0,
        }
    }
}

/// ` | a b c` for a sample with a stack (relative to `image`), else nothing.
fn stack_text(sm: &Sample, stacks: &[u64], image: u64) -> String {
    if sm.depth == 0 {
        return String::new();
    }
    let mut s = String::from(" |");
    let f = sm.first as usize;
    for a in &stacks[f..f + sm.depth as usize] {
        s.push_str(&format!(" {:x}", a - image));
    }
    s
}

/// What an address is: `own` = in this image (with its name), else the module's path and the name in it.
pub struct Names<'a> {
    pub own: &'a dyn Fn(u64) -> Option<String>,
    pub module: &'a dyn Fn(u64) -> Option<(String, u64, String)>,
}

/// The sample lines and the tables (host_prof.py's three), `image` = this image's base, `stacks` the samples' stacks.
pub fn report(
    head: &str,
    samples: &[Sample],
    stacks: &[u64],
    image: u64,
    names: &Names,
    top: usize,
) -> String {
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
                    "{:x} {:x} -{}\n",
                    sm.ip - image,
                    if sm.ret != 0 { sm.ret - image } else { 0 },
                    stack_text(sm, stacks, image)
                ));
                *own.entry(name).or_default() += 1;
                n_own += 1;
            }
            None => {
                let (path, rel, fname) =
                    (names.module)(sm.ip).unwrap_or_else(|| ("?".into(), sm.ip, "?".into()));
                s.push_str(&format!(
                    "{:x} {:x} {path}{}\n",
                    rel,
                    if sm.ret != 0 { sm.ret - image } else { 0 },
                    stack_text(sm, stacks, image)
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

/// Starts the sampler thread when `NXRS_HOST_PROF` is set (once).
pub fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(v) = std::env::var("NXRS_HOST_PROF")
            .ok()
            .filter(|v| !v.is_empty())
        else {
            return;
        };
        let spec = parse(&v);
        let name = thread_name(std::env::var("NXRS_HOST_PROF_THREAD").ok().as_deref());
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
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HMODULE, STILL_ACTIVE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind, CONTEXT, IMAGE_NT_HEADERS64,
        UNW_FLAG_NHANDLER,
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
        GetCurrentProcessId, GetCurrentThreadId, GetExitCodeThread, GetThreadDescription,
        GetThreadTimes, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
        THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
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

    /// Every thread of this process whose name starts with one of `names` (id, name), never the calling thread (the
    /// sampler must not suspend itself).
    pub fn find_threads(names: &[&str]) -> Vec<(u32, String)> {
        let mut found = Vec::new();
        // SAFETY: a thread snapshot of this process, our own buffers and handles
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap as isize == -1 {
                return found;
            }
            let me = GetCurrentProcessId();
            let self_tid = GetCurrentThreadId();
            let mut te: THREADENTRY32 = core::mem::zeroed();
            te.dwSize = core::mem::size_of::<THREADENTRY32>() as u32;
            let mut ok = Thread32First(snap, &mut te) != 0;
            while ok {
                if te.th32OwnerProcessID == me && te.th32ThreadID != self_tid {
                    let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, te.th32ThreadID);
                    if !h.is_null() {
                        let mut p: *mut u16 = core::ptr::null_mut();
                        if GetThreadDescription(h, &mut p) >= 0 && !p.is_null() {
                            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
                            let n = String::from_utf16_lossy(core::slice::from_raw_parts(p, len));
                            if names.iter().any(|x| n.starts_with(x)) {
                                found.push((te.th32ThreadID, n));
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

    /// Bytes of a sampled thread's stack copied (from its rsp up) while it is suspended: with stacks, enough for the
    /// frames that matter; without, the 512 words searched for the first return address into this image.
    const COPY_STACK: usize = 64 << 10;
    const COPY_RET: usize = 4 << 10;
    /// Zeros after the copy: an unwind step reads a frame's saved registers above its rsp, possibly past the copied
    /// bytes; it reads zeros there (and stops at a zero return address) instead of past our buffer.
    const COPY_PAD: usize = 64 << 10;

    /// The general registers of a context, for moving stack addresses between the thread's stack and the copy.
    fn gprs(c: &mut CONTEXT) -> [&mut u64; 16] {
        [
            &mut c.Rax, &mut c.Rcx, &mut c.Rdx, &mut c.Rbx, &mut c.Rsp, &mut c.Rbp, &mut c.Rsi,
            &mut c.Rdi, &mut c.R8, &mut c.R9, &mut c.R10, &mut c.R11, &mut c.R12, &mut c.R13,
            &mut c.R14, &mut c.R15,
        ]
    }

    /// The return addresses into this image's code [tlo, thi), innermost first, of a stack copied while its thread
    /// was suspended: `copy` holds the thread's bytes [real, real + len) (then zeros, `COPY_PAD` of them), `cx` its
    /// context at that moment. Runs after the thread was resumed: the unwind tables' lookups may take the loader's
    /// locks, which a suspended thread could hold. Every register that points into the copied range is moved into
    /// the copy (again after each step, for the frame pointers restored from it); the walk ends when rsp leaves the
    /// copied bytes, so nothing is read from the live stack or past the buffer.
    ///
    /// SAFETY: `copy` is `len + COPY_PAD` bytes long at least, zero past `len`.
    unsafe fn unwind_copy(
        cx: &CONTEXT,
        copy: &[u64],
        real: u64,
        len: u64,
        tlo: u64,
        thi: u64,
        out: &mut Vec<u64>,
    ) -> u8 {
        debug_assert!(copy.len() * 8 >= len as usize + COPY_PAD);
        let lo = copy.as_ptr() as u64;
        let relocate = |c: &mut CONTEXT| {
            for r in gprs(c) {
                if *r >= real && *r < real + len {
                    *r = *r - real + lo;
                }
            }
        };
        let mut c: CONTEXT = *cx;
        relocate(&mut c);
        let mut n = 0u8;
        for _ in 0..64 {
            if c.Rsp < lo || c.Rsp + 8 > lo + len {
                break;
            }
            let mut base = 0u64;
            let f = RtlLookupFunctionEntry(c.Rip, &mut base, core::ptr::null_mut());
            if f.is_null() {
                // a leaf: the return address is at rsp
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
                relocate(&mut c);
            }
            if c.Rip == 0 || c.Rsp < lo || c.Rsp > lo + len {
                break;
            }
            if c.Rip >= tlo && c.Rip < thi {
                out.push(c.Rip);
                n += 1;
                if n as usize == MAX_DEPTH {
                    break;
                }
            }
        }
        n
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

    /// How often the thread list is read again for threads started after the sampler.
    const RESCAN: std::time::Duration = std::time::Duration::from_millis(250);

    /// A sampled thread.
    struct Target {
        tid: u32,
        name: String,
        h: HANDLE,
        cpu0: f64,
        from: std::time::Instant,
        n: u64,
        alive: bool,
    }

    /// Opens the threads of `found` not sampled yet.
    fn add_new(ths: &mut Vec<Target>, found: Vec<(u32, String)>) {
        for (tid, name) in found {
            if ths.iter().any(|t| t.tid == tid && t.alive) {
                continue;
            }
            // SAFETY: opening a thread of this process
            let h = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION,
                    0,
                    tid,
                )
            };
            if !h.is_null() {
                ths.push(Target {
                    tid,
                    name,
                    h,
                    cpu0: cpu_seconds(h),
                    from: std::time::Instant::now(),
                    n: 0,
                    alive: true,
                });
            }
        }
    }

    /// Samples the threads `name` names (comma-separated name prefixes; waiting up to 10 minutes for one to exist,
    /// and taking in the ones that start later) for `seconds` and returns the file's text.
    pub fn run(name: &str, seconds: f64, t0: std::time::Instant) -> Result<String, String> {
        let stack = std::env::var("NXRS_HOST_PROF_STACK").as_deref() == Ok("1");
        run_ex(name, seconds, t0, stack)
    }

    /// As `run`; `stack`: each sample with its unwound stack.
    pub fn run_ex(
        name: &str,
        seconds: f64,
        t0: std::time::Instant,
        stack: bool,
    ) -> Result<String, String> {
        let names: Vec<&str> = name
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .collect();
        let mut ths: Vec<Target> = Vec::new();
        loop {
            add_new(&mut ths, find_threads(&names));
            if !ths.is_empty() {
                break;
            }
            if t0.elapsed().as_secs() > 600 {
                return Err(format!(
                    "no thread named {name:?} (NXRS_HOST_PROF_THREAD) that can be opened"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let (lo, hi, tlo, thi) = own_range();
        // made before any thread is stopped: nothing below allocates while one is (it may hold the heap's lock)
        let copy_len = if stack { COPY_STACK } else { COPY_RET };
        let mut copy = vec![0u64; (copy_len + COPY_PAD) / 8];
        let mut copied = 0u64; // bytes of `copy` the last sample filled (the rest is zero)
        let mut samples: Vec<Sample> = Vec::with_capacity(1 << 16);
        let mut stacks: Vec<u64> = Vec::with_capacity(if stack { 1 << 20 } else { 0 });
        let start = std::time::Instant::now();
        let end = start + std::time::Duration::from_secs_f64(seconds.max(0.0));
        let mut rescan = start + RESCAN;
        while std::time::Instant::now() < end && samples.len() < 1 << 20 {
            // ~0.5 ms, without the 1 ms sleep granularity
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_micros(480) {
                core::hint::spin_loop();
            }
            if t >= rescan {
                // lanes start lazily, after the sampler: take them in as they appear; drop the ones that ended
                for tg in ths.iter_mut().filter(|t| t.alive) {
                    let mut code = 0u32;
                    // SAFETY: our handle (opened with THREAD_QUERY_INFORMATION)
                    if unsafe { GetExitCodeThread(tg.h, &mut code) } == 0
                        || code != STILL_ACTIVE as u32
                    {
                        tg.alive = false;
                    }
                }
                add_new(&mut ths, find_threads(&names));
                rescan = t + RESCAN;
            }
            for tg in ths.iter_mut().filter(|t| t.alive) {
                let mut cx: CONTEXT = unsafe { core::mem::zeroed() };
                // CONTEXT_FULL with a stack (the unwind needs the integer registers)
                cx.ContextFlags = if stack { 0x0010_000b } else { CONTEXT_CONTROL };
                let mut len = 0u64;
                // SAFETY: while the thread is stopped (it may hold any lock: the heap's, the loader's) only its
                // context is read, the kernel asked where its stack region ends, and its stack bytes copied into the
                // buffer made above; no allocation, no lookup in the unwind tables
                let got = unsafe {
                    if SuspendThread(tg.h) == u32::MAX {
                        // gone (a lane that ended): it is dropped from the sampling
                        tg.alive = false;
                        continue;
                    }
                    let ok = GetThreadContext(tg.h, &mut cx) != 0;
                    if ok {
                        let mut mb: MEMORY_BASIC_INFORMATION = core::mem::zeroed();
                        if VirtualQuery(
                            cx.Rsp as _,
                            &mut mb,
                            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                        ) != 0
                            && mb.State == MEM_COMMIT
                        {
                            let rend = mb.BaseAddress as u64 + mb.RegionSize as u64;
                            len = rend.saturating_sub(cx.Rsp).min(copy_len as u64) & !7;
                            core::ptr::copy_nonoverlapping(
                                cx.Rsp as *const u8,
                                copy.as_mut_ptr() as *mut u8,
                                len as usize,
                            );
                        }
                    }
                    ResumeThread(tg.h);
                    ok
                };
                if !got {
                    continue;
                }
                // the thread runs again: everything below works on the copy (zero past len: what the last, longer
                // copy left there)
                if len < copied {
                    copy[len as usize / 8..copied as usize / 8].fill(0);
                }
                copied = len;
                let mut s = Sample::at(cx.Rip, 0);
                if cx.Rip < lo || cx.Rip >= hi {
                    // the first stack word that points into this image's code
                    s.ret = copy[..len as usize / 8]
                        .iter()
                        .take(512)
                        .copied()
                        .find(|&w| w >= tlo && w < thi)
                        .unwrap_or(0);
                }
                if stack && len > 0 {
                    s.first = stacks.len() as u32;
                    // SAFETY: `copy` is len + COPY_PAD bytes or more, zero past len
                    s.depth =
                        unsafe { unwind_copy(&cx, &copy, cx.Rsp, len, tlo, thi, &mut stacks) };
                }
                samples.push(s);
                tg.n += 1;
            }
        }
        let cpu: f64 = ths.iter().map(|t| cpu_seconds(t.h) - t.cpu0).sum();
        let mut head = format!("# host profile of thread {name:?} ({} threads): {} samples in {seconds:.1} s, thread CPU {cpu:.2} s, image {lo:x}..{hi:x}", ths.len(), samples.len());
        for t in &ths {
            head.push_str(&format!(
                "\n# thread {} {:?}: {} samples, from {:.2} s{}",
                t.tid,
                t.name,
                t.n,
                t.from.saturating_duration_since(start).as_secs_f64(),
                if t.alive { "" } else { ", ended" }
            ));
            // SAFETY: our handle
            unsafe { CloseHandle(t.h) };
        }
        Ok(report(
            &head,
            &samples,
            &stacks,
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
    Err("NXRS_HOST_PROF samples a host thread on Windows only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spec as sscanf("%lf,%lf,%[^\n]") reads it, and NXRS_HOST_PROF_THREAD.
    #[test]
    fn spec_and_thread_name() {
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

    /// host_prof.py's three tables from samples.
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
        let mut s = [
            Sample::at(0x1100, 0),
            Sample::at(0x1200, 0),
            Sample::at(0x1900, 0),
            Sample::at(0x9010, 0x1110),
        ];
        // a stack: its return addresses after ` | `, relative to the image (from the side list)
        let stacks = [0x1f00, 0x1300, 0x1400];
        s[1].first = 1;
        s[1].depth = 2;
        let r = report("# head", &s, &stacks, 0x1000, &names, 40);
        let l: Vec<&str> = r.lines().collect();
        assert_eq!(
            &l[..5],
            [
                "# head",
                "100 0 -",
                "200 0 - | 300 400",
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
            head.starts_with("# host profile of thread \"hostprof test spinner\" (1 threads): "),
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

    /// With stacks: the spinner's samples carry return addresses into this image (the test harness's frames).
    #[cfg(windows)]
    #[test]
    fn samples_carry_the_unwound_stack() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        #[inline(never)]
        fn spin(s: &std::sync::atomic::AtomicBool) -> u64 {
            let mut x = 0u64;
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
            }
            x
        }
        let h = std::thread::Builder::new()
            .name("hostprof stack spinner".into())
            .spawn(move || spin(&s2))
            .unwrap();
        let text = win::run_ex(
            "hostprof stack spinner",
            0.2,
            std::time::Instant::now(),
            true,
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        h.join().unwrap();
        let text = text.unwrap();
        let with = text
            .lines()
            .filter(|l| !l.starts_with('#') && l.contains(" | "))
            .count();
        let all = text.lines().filter(|l| !l.starts_with('#')).count();
        assert!(
            all > 50 && with > all / 2,
            "{with} of {all} samples with a stack"
        );
    }

    /// A spinner named `name` until `stop`.
    #[cfg(windows)]
    fn spinner(
        name: &str,
        stop: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::thread::JoinHandle<u64> {
        let s2 = stop.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut x = 0u64;
                while !s2.load(std::sync::atomic::Ordering::Relaxed) {
                    x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
                }
                x
            })
            .unwrap()
    }

    /// The sampler never stops itself, though its own thread's name matches (before: it suspended itself and hung).
    #[cfg(windows)]
    #[test]
    fn the_sampler_skips_its_own_thread() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let lane = spinner("hostprof self lane", &stop);
        let me = std::thread::Builder::new()
            .name("hostprof self sampler".into())
            .spawn(|| win::run_ex("hostprof self", 0.2, std::time::Instant::now(), true))
            .unwrap();
        let text = me.join().unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        lane.join().unwrap();
        let text = text.unwrap();
        assert!(
            text.contains("(1 threads)"),
            "{}",
            text.lines().next().unwrap()
        );
        assert!(text.contains("\"hostprof self lane\""));
        assert!(!text.contains("\"hostprof self sampler\""));
    }

    /// A thread started after the sampler is taken in at the next rescan and named in the head (before: never).
    #[cfg(windows)]
    #[test]
    fn threads_started_later_are_sampled() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let first = spinner("hostprof late 0", &stop);
        let st = stop.clone();
        let later = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            spinner("hostprof late 1", &st)
        });
        let text = win::run_ex("hostprof late", 0.8, std::time::Instant::now(), false);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        first.join().unwrap();
        later.join().unwrap().join().unwrap();
        let text = text.unwrap();
        assert!(
            text.contains("(2 threads)"),
            "{}",
            text.lines().next().unwrap()
        );
        let late = text
            .lines()
            .find(|l| l.contains("\"hostprof late 1\""))
            .expect("the late thread named");
        let n: u64 = late
            .split(": ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(n > 20, "{late}");
    }
}
