//! The loader-to-run path: claim the guest address space, load the ExeFS modules at the translator's bases, bind
//! imports, install the dispatch table and the crash reporter, then start the main thread at `main`'s entry with its
//! stack and TLS, and run until the guest exits.
//!
//! The executable that links the translated code calls `nxrt_main` from its `main` with the generated module table
//! (see `build.rs` in `bin/nxrt_dev.rs`: `modules.c`).

use crate::cpu::Cpu;
use crate::dispatch::{Module, RtModule, Table};
use crate::loader::Loaded;
use crate::mem::{Range, Space, KERNEL_BASE};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---- start-up: what the process is decides how the runtime behaves

/// The program a process runs, as start-up needs it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Program {
    /// From main.npdm's ACI0 (0 when unknown).
    pub program_id: u64,
}

impl Program {
    /// A Switch 2 program (title ids 04...).
    pub fn switch2(&self) -> bool {
        self.program_id >> 56 == 0x04
    }
}

static PROGRAM: OnceLock<Program> = OnceLock::new();

/// The program id in an NPDM (META: ACI0 offset at +0x70; ACI0: program id at +0x10).
pub fn npdm_program_id(npdm: &[u8]) -> Option<u64> {
    if npdm.len() < 0x80 || &npdm[..4] != b"META" {
        return None;
    }
    let aci = u32::from_le_bytes(npdm[0x70..0x74].try_into().unwrap()) as usize;
    let b = npdm.get(aci..aci + 0x18)?;
    (&b[..4] == b"ACI0").then(|| u64::from_le_bytes(b[0x10..0x18].try_into().unwrap()))
}

/// Tells start-up which program runs (call before `kernel::init`; else start-up reads `main.npdm` from
/// `NXRT_EXEFS` or `NXRS_DATA\exefs`, which the game's loader sets).
pub fn set_program(p: Program) {
    let _ = PROGRAM.set(p);
}

fn program() -> Program {
    *PROGRAM.get_or_init(|| {
        let exefs = std::env::var_os("NXRT_EXEFS")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("NXRS_DATA").map(|d| Path::new(&d).join("exefs")));
        let id = exefs.and_then(|e| std::fs::read(e.join("main.npdm")).ok()).and_then(|n| npdm_program_id(&n));
        Program { program_id: id.unwrap_or(0) }
    })
}

fn env_flag(name: &str) -> Option<bool> {
    std::env::var(name).ok().filter(|v| !v.is_empty()).map(|v| v.trim().parse::<i64>().map_or(true, |n| n > 0))
}

/// What start-up decides, once per process (from `kernel::init`, before any guest thread runs):
/// - the system counter: 19.2 MHz for a Switch program, 31.25 MHz for a Switch 2 one (`SWR_TICK_HZ=<Hz>` sets it);
/// - the global exclusive monitor: on for Switch programs, off for Switch 2 ones (`SWR_XMON=1` / `SWR_NO_XMON=1`);
/// - a map past the application pool fails (ResultLimitReached) for the titles that need it (Pokemon Legends: Z-A,
///   Ocarina of Time; `SWR_MAP_LIMIT=1/0` for any title);
/// - the lazy heap (`NXRT_LAZY_HEAP=1`);
/// - the process's memory cap (`memory_cap`).
pub fn startup(space: &Space) {
    static DONE: OnceLock<()> = OnceLock::new();
    if DONE.set(()).is_err() {
        return;
    }
    let p = program();
    let mut hz = if p.switch2() { crate::rt::TICK_HZ_SWITCH2 } else { crate::rt::TICK_HZ_SWITCH1 };
    if let Some(v) = std::env::var("SWR_TICK_HZ").ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v >= 1e6) {
        hz = v as u32;
    }
    crate::rt::set_tick_hz(hz);
    let xmon = env_flag("SWR_XMON").unwrap_or_else(|| env_flag("SWR_NO_XMON").map_or(!p.switch2(), |no| !no));
    crate::cpu::set_exclusive_monitor(xmon);
    let limit = env_flag("SWR_MAP_LIMIT").unwrap_or(matches!(p.program_id, 0x0100_F430_08C4_4000 | 0x0400_4860_24DE_C000));
    space.set_map_limit(limit);
    if env_flag("NXRT_LAZY_HEAP") == Some(true) {
        space.set_lazy_heap(true);
    }
    let pool = app_pool(p);
    space.set_pool(pool);
    let gb = nxconfig::int("memory_gb");
    if gb > 4 {
        crate::rt::log(&format!(
            "memory_gb={gb}: the game sees {} MB instead of the console's 3285. Only a few games need this (Tears of the Kingdom); if this one crashes while loading, set memory_gb=0",
            pool >> 20
        ));
    }
    if nxconfig::flag("null_trap") {
        crate::rt::log("null_trap: writes to page 0 crash with a log (always so here: Windows maps nothing below 64 KB)");
    }
    crate::profile::start(nxconfig::flag("profile"));
    crate::hostprof::start();
    crate::rt::log(&format!(
        "program {:016x}: system tick {:.2} MHz{}; exclusive monitor {} (SWR_XMON=1 / SWR_NO_XMON=1); maps past the pool {}",
        p.program_id,
        hz as f64 / 1e6,
        if p.switch2() { " (a Switch 2 program)" } else { "" },
        if xmon { "on: a STXR fails after another thread's store-exclusive to the same place" } else { "off: LDXR/STXR compare values only" },
        if limit { "fail (ResultLimitReached)" } else { "are granted" },
    ));
    memory_cap(pool);
}

/// The application pool the game sees (GetInfo TotalMemorySize; the heap and physical memory share it), from the ini's
/// `memory_gb`: 0 = automatic (a Switch 2 program 9 GB; Tears of the Kingdom 5.2 GB; else the console's 3285 MB), 4 =
/// the console's, 6 / 8 = extended (yuzu's extended layout: 5.2 / 7.2 GB).
pub fn app_pool(p: Program) -> u64 {
    let gb = nxconfig::int("memory_gb");
    match gb {
        g if p.switch2() && g <= 0 => 0x2_4000_0000,
        g if g >= 8 => 0x1_CD50_0000,
        g if g >= 6 => 0x1_4D50_0000,
        g if g >= 4 || g < 0 => 0xCD50_0000,
        _ if p.program_id == 0x0100_F2C0_115B_6000 => 0x1_4D50_0000,
        _ => 0xCD50_0000,
    }
}

/// The cap on this process's committed memory, once `memory_cap` set it (0: none).
pub fn memory_cap_bytes() -> u64 {
    MEMORY_CAP.get().copied().unwrap_or(0)
}

static MEMORY_CAP: OnceLock<u64> = OnceLock::new();

/// No game may take the machine's memory: the process puts itself into a job whose committed memory is limited, so
/// past the limit allocations fail in this process (it stops, with the reason in its log) instead of in every other
/// program. On 2026-11-18 Ocarina of Time's process grew by 1 GB a second (the NVIDIA driver's own allocations while
/// pipelines were made) and Windows went down with it. The limit: half of the machine's memory, at least 12 GB and
/// at most 32 GB; `SWR_MEMORY_CAP_GB` sets it, 0 turns it off.
#[cfg(windows)]
pub fn memory_cap(pool: u64) {
    use windows_sys::Win32::System::JobObjects::*;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    if MEMORY_CAP.get().is_some() {
        return;
    }
    let mut ms: MEMORYSTATUSEX = unsafe { core::mem::zeroed() };
    ms.dwLength = core::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a plain query into our struct
    unsafe { GlobalMemoryStatusEx(&mut ms) };
    // never below what the game is allowed to map (memory_gb) plus room for the driver and the engine
    let mut cap = (ms.ullTotalPhys / 2).clamp(12 << 30, 32 << 30).max((pool + (8 << 30)).min(ms.ullTotalPhys));
    if let Some(gb) = std::env::var("SWR_MEMORY_CAP_GB").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        cap = gb << 30;
    }
    let _ = MEMORY_CAP.set(cap);
    if cap == 0 {
        crate::rt::log("process memory cap: off (SWR_MEMORY_CAP_GB=0)");
        return;
    }
    // SAFETY: job object calls on handles we own; the job stays open for the life of the process
    let ok = unsafe {
        let job = CreateJobObjectW(core::ptr::null(), core::ptr::null());
        let mut li: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = core::mem::zeroed();
        li.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        li.ProcessMemoryLimit = cap as usize;
        !job.is_null()
            && SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &li as *const _ as *const core::ffi::c_void,
                core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
            && AssignProcessToJobObject(job, windows_sys::Win32::System::Threading::GetCurrentProcess()) != 0
    };
    if ok {
        crate::rt::log(&format!(
            "process memory cap: {} MB of committed memory (half of the machine's {} MB, between 12 and 32 GB; SWR_MEMORY_CAP_GB sets it, 0 = off). Past it this game stops instead of the machine",
            cap >> 20,
            ms.ullTotalPhys >> 20
        ));
    } else {
        let e = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        crate::rt::log(&format!("[RED] process memory cap: could not be set (error {e}): this game's memory is not limited"));
    }
}

/// The same cap on Linux (`linux::memory_cap`: a cgroup with memory.max, else a watchdog on the resident size).
#[cfg(target_os = "linux")]
pub fn memory_cap(pool: u64) {
    if MEMORY_CAP.get().is_some() {
        return;
    }
    let total = crate::linux::total_memory();
    let mut cap = (total / 2).clamp(12 << 30, 32 << 30).max((pool + (8 << 30)).min(total));
    if let Some(gb) = std::env::var("SWR_MEMORY_CAP_GB").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        cap = gb << 30;
    }
    let _ = MEMORY_CAP.set(cap);
    if cap == 0 {
        crate::rt::log("process memory cap: off (SWR_MEMORY_CAP_GB=0)");
        return;
    }
    match crate::linux::memory_cap(cap) {
        Ok(how) => crate::rt::log(&format!(
            "process memory cap: {} MB ({how}; half of the machine's {} MB, between 12 and 32 GB; SWR_MEMORY_CAP_GB sets it, 0 = off). Past it this game stops instead of the machine",
            cap >> 20,
            total >> 20
        )),
        Err(e) => crate::rt::log(&format!("[RED] process memory cap: could not be set ({e}): this game's memory is not limited")),
    }
}

/// Ends the process: flushes the guest's output, exits with `code`.
pub fn exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}

/// Host functions every process has: the fiber API, and two bridge probes the homebrew tests import.
fn register_builtins() {
    crate::fiber::register();
    crate::hostfn::add_override("nxrt_test_sum9", test_sum9);
    crate::hostfn::add_override("nxrt_test_scale", test_scale);
    crate::hostfn::add_override("nxrt_test_callback", test_callback);
    crate::hostfn::add_override("nxrt_test_drops", test_drops);
}

static DROPS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
struct Counted;
impl Drop for Counted {
    fn drop(&mut self) {
        DROPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// `void nxrt_test_callback(void (*fn)(void *), void *arg)`: calls the guest function with arg, holding a value with
/// a destructor across the call. When the guest longjmps out of the callback, the call reports it and this returns,
/// so the destructor runs before the jump continues (`nxrt_test_drops` counts it).
unsafe fn test_callback(c: *mut Cpu) {
    let _held = Counted;
    let (f, arg) = ((*c).x[0], (*c).x[1]);
    let saved = (*c).x;
    (*c).x[0] = arg;
    if crate::boundary::call_guest(c, f) != crate::boundary::GuestCall::Returned {
        return;
    }
    (&mut (*c).x)[19..30].copy_from_slice(&saved[19..30]);
}

/// `uint32_t nxrt_test_drops(void)`
unsafe fn test_drops(c: *mut Cpu) {
    (*c).x[0] = DROPS.load(std::sync::atomic::Ordering::SeqCst) as u64;
}

/// `int64_t nxrt_test_sum9(uint8_t a, int16_t b, int32_t c, int64_t d, int64_t e, int64_t f, int64_t g, int64_t h,
/// int64_t i)`: a + b + c + ... + 1000 * i. Narrow arguments are truncated as AAPCS64 says (the upper bits of a
/// w/x register holding a narrow argument are unspecified), the ninth comes from the guest stack.
unsafe fn test_sum9(c: *mut Cpu) {
    let c = &mut *c;
    let mut s = c.arg_x(0) as u8 as i64 + c.arg_x(1) as i16 as i64 + c.arg_x(2) as i32 as i64;
    for i in 3..8 {
        s += c.arg_x(i) as i64;
    }
    s += 1000 * c.arg_x(8) as i64;
    c.ret_x(s as u64);
}

/// `double nxrt_test_scale(double x, int32_t k)`: x * k + 0.25.
unsafe fn test_scale(c: *mut Cpu) {
    let c = &mut *c;
    let r = c.arg_d(0) * c.arg_x(0) as i32 as f64 + 0.25;
    c.ret_d(r);
}

fn usage() -> ! {
    eprintln!("usage: <game>.exe <exefs folder>   (NXRT_EXEFS also names it; NXRT_TRACE=svc,host,sjlj,thread,load)");
    exit(2);
}

/// Reserves the guest map's fixed regions for the modules the generated code lists (`mem::claim_fixed`): called first
/// thing by the entry points, before anything else can take that address space.
///
/// # Safety
/// `mods` points to `count` modules (the generated code's table).
pub unsafe fn claim_fixed_regions(mods: *const RtModule, count: i32) {
    if mods.is_null() || count <= 0 {
        return;
    }
    let ms = std::slice::from_raw_parts(mods, count as usize);
    let lo = ms.iter().map(|m| m.base).min().unwrap_or(0);
    let hi = ms.iter().map(|m| m.base + m.size).max().unwrap_or(0);
    if hi > lo {
        crate::mem::claim_fixed(Range { base: lo, size: hi - lo });
    }
}

/// `int nxrt_main(int argc, char **argv, const RtModule *mods, int count)`
#[no_mangle]
pub unsafe extern "C" fn nxrt_main(argc: i32, argv: *const *const core::ffi::c_char, mods: *const RtModule, count: i32) -> i32 {
    claim_fixed_regions(mods, count);
    let args: Vec<String> = (0..argc as usize).map(|i| std::ffi::CStr::from_ptr(*argv.add(i)).to_string_lossy().into_owned()).collect();
    // the first argument that is not a --key=value setting names the ExeFS folder
    let exefs = match args.iter().skip(1).find(|a| !(a.starts_with("--") && a.contains('='))) {
        Some(a) => PathBuf::from(a),
        None => std::env::var("NXRT_EXEFS").map(PathBuf::from).unwrap_or_else(|_| usage()),
    };
    let modules: Vec<Module> = (0..count as usize).map(|i| Module::from_raw(&*mods.add(i))).collect();
    match crate::boundary::guard("nxrt_main", || run(&exefs, modules)) {
        Ok(()) => loop {
            std::thread::park();
        },
        Err(e) => crate::rt::fatal(&e),
    }
}

/// Where the main thread starts.
#[derive(Clone, Debug)]
pub enum Entry {
    /// `main`'s first instruction (homebrew; nxrt_main's default).
    Main,
    /// Another module's first instruction (an SDK title starts in `rtld`).
    Module(String),
    /// A guest address.
    Address(u64),
    /// A host function (registered at a synthetic guest address): the os layer starts the main thread in its own
    /// code and runs nn::init's steps from there.
    Host(crate::hostfn::HostFn),
}

/// A loaded process: its address space and modules, the dispatch table installed.
pub struct Process {
    pub space: &'static Space,
    pub modules: Vec<Loaded>,
    pub npdm: Option<Vec<u8>>,
}

/// Claims the guest address space, loads the ExeFS modules at the translator's bases, binds their imports (host
/// overrides first: register them before), installs the dispatch table and the crash reporter.
pub fn load(exefs: &Path, modules: Vec<Module>) -> Result<Process, String> {
    if modules.is_empty() {
        return Err("no modules".into());
    }
    let npdm = std::fs::read(exefs.join("main.npdm")).ok();
    if let Some(id) = npdm.as_deref().and_then(npdm_program_id) {
        set_program(Program { program_id: id });
    }
    let lo = modules.iter().map(|m| m.base).min().unwrap();
    let hi = modules.iter().map(|m| m.base + m.size).max().unwrap();
    let space: &'static Space = Box::leak(Box::new(Space::new(Range { base: lo, size: hi - lo }, KERNEL_BASE, true)?));
    register_builtins();
    let mut files = vec![];
    for m in &modules {
        let p = exefs.join(&m.name);
        let raw = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        files.push((m.name.clone(), m.base, m.size, raw));
    }
    let (loaded, rep) = crate::loader::load(space, &files)?;
    let table = crate::dispatch::install(Table::build(modules)?)?;
    crate::fault::install();
    let names: Vec<String> = loaded.iter().map(|m| format!("{} {:#x}+{:#x}", m.name, m.base, m.size)).collect();
    crate::rt::log(&format!(
        "loaded {}; {} lifted functions; {} relocations; host-bound imports: {}{}",
        names.join(", "),
        table.functions,
        rep.relocations,
        if rep.host_bound.is_empty() { "none".into() } else { rep.host_bound.join(" ") },
        if rep.unresolved.is_empty() { String::new() } else { format!("; UNRESOLVED (stubbed): {}", rep.unresolved.join(" ")) },
    ));
    Ok(Process { space, modules: loaded, npdm })
}

/// Starts the kernel (and start-up's decisions) and the main thread at `entry`, with main.npdm's priority, core and
/// stack size. Returns the main thread's handle.
pub fn start(p: &Process, entry: Entry) -> Result<u32, String> {
    let first = |name: &str| p.modules.iter().find(|m| m.name == name).map(|m| m.entry());
    let entry = match entry {
        Entry::Main => Entry::Address(first("main").or(p.modules.first().map(|m| m.entry())).unwrap()),
        Entry::Module(n) => Entry::Address(first(&n).ok_or_else(|| format!("no module {n} to start in"))?),
        e => e,
    };
    start_main(p.space, p.npdm.as_deref(), entry, |_| {})
}

/// The main thread a host that loads the modules itself starts (the os layer): the kernel and start-up's decisions,
/// then `setup(MainThread)` (the host's scheduler, counter rate: before any guest thread exists), then the main
/// thread at `entry` (`Address` or `Host`) with main.npdm's priority, core and stack size. Returns its handle.
pub fn start_main(space: &'static Space, npdm: Option<&[u8]>, entry: Entry, setup: impl FnOnce(&MainThread)) -> Result<u32, String> {
    let (prio, core, stack_size) = crate::loader::npdm_main_thread(npdm);
    let stack = space.alloc_stack(stack_size)?;
    let pc = match entry {
        Entry::Address(a) => a,
        Entry::Host(f) => crate::hostfn::register("the main thread's start", f),
        Entry::Main | Entry::Module(_) => return Err("start_main: a module entry needs the loaded process (process::start)".into()),
    };
    let k = crate::kernel::init(space);
    setup(&MainThread { priority: prio, core, stack_size });
    Ok(k.create_main(pc, stack + crate::mem::align_up(stack_size, crate::mem::PAGE), prio, core))
}

/// The main thread `start_main` is about to create, as main.npdm describes it.
#[derive(Clone, Copy, Debug)]
pub struct MainThread {
    pub priority: i32,
    pub core: i32,
    pub stack_size: u64,
}

fn run(exefs: &Path, modules: Vec<Module>) -> Result<(), String> {
    let p = load(exefs, modules)?;
    // the interpreter's execution hints (discovered.tsv) beside the ExeFS folder
    crate::interp::set_data_dir(exefs.parent().unwrap_or(exefs));
    start(&p, Entry::Main)?;
    Ok(())
}

#[cfg(test)]
mod option_tests {
    //! The ini options the runtime honours at start-up (Turbo runtime/main.c's option table): one test each.
    use super::*;

    /// `memory_gb`: 0 automatic (the console's 3285 MB, Tears of the Kingdom 5.2 GB, a Switch 2 program 9 GB), 4 the
    /// console's, 6 / 8 the extended layouts; the pool is what GetInfo and the memory cap use.
    #[test]
    fn memory_gb_sets_the_application_pool() {
        let s1 = Program { program_id: 0x0100_1520_0002_2000 };
        let totk = Program { program_id: 0x0100_F2C0_115B_6000 };
        let s2 = Program { program_id: 0x0400_C3F0_0006_E000 };
        for (gb, want, want_totk, want_s2) in [
            ("0", 0xCD50_0000u64, 0x1_4D50_0000u64, 0x2_4000_0000u64),
            ("4", 0xCD50_0000, 0xCD50_0000, 0xCD50_0000),
            ("6", 0x1_4D50_0000, 0x1_4D50_0000, 0x1_4D50_0000),
            ("8", 0x1_CD50_0000, 0x1_CD50_0000, 0x1_CD50_0000),
        ] {
            assert!(nxconfig::apply_arg(&format!("--memory_gb={gb}")));
            assert_eq!((app_pool(s1), app_pool(totk), app_pool(s2)), (want, want_totk, want_s2), "memory_gb={gb}");
        }
        assert!(nxconfig::apply_arg("--memory_gb=0"));
    }

    /// `null_trap`: Turbo makes the guest's page 0 (its first 64 KB) read-only so a write through NULL stops the
    /// game with a log. Guest addresses are host addresses here and Windows maps nothing below 64 KB, so such a
    /// write always faults (the fault handler reports it): the option is on by construction, and start-up says so.
    #[test]
    fn null_trap_page_zero_never_maps() {
        assert!(nxconfig::apply_arg("--null_trap=1"));
        assert!(nxconfig::flag("null_trap"));
        for a in [0u64, 0x1000, 0x8000, 0xF000] {
            assert!(!crate::mem::readable(a, 8), "{a:#x} is mapped");
        }
    }

    /// `log`: 2 turns every trace on (Turbo's g_verbose), 1 none; `NXRT_TRACE` names its own set over it.
    #[test]
    fn log_two_traces_everything() {
        assert_eq!(crate::rt::trace_spec(None, 1), "");
        assert_eq!(crate::rt::trace_spec(None, 2), "all");
        assert_eq!(crate::rt::trace_spec(Some(String::new()), 2), "all");
        assert_eq!(crate::rt::trace_spec(Some("svc,host".into()), 2), "svc,host");
        assert!(nxconfig::apply_arg("--log=2"));
        assert_eq!(crate::rt::trace_spec(None, nxconfig::int("log")), "all");
    }

    /// `profile`: 1 starts the sampling profiler (profile.txt every 10 s).
    #[test]
    fn profile_starts_the_profiler() {
        assert!(nxconfig::apply_arg("--profile=1"));
        crate::profile::start(nxconfig::flag("profile"));
    crate::hostprof::start();
        assert!(crate::profile::on());
    }
}
