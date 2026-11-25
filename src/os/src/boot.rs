//! The game process: the os layer's entry `nxos_main`, which the executable's `main` calls with the generated module
//! table (the runtime's `nxrt_main` contract), and the boot of the SDK on the main guest thread.
//!
//! The process is started the way nxnative starts it, without rtld: after loading, the main thread runs the steps
//! of `nn::init::Start` in their order: `_init_libc0`, `nnosInitialize(main thread handle, 0)`, `_init_libc1`,
//! `nninitInitializeSdkModule`, the host arguments, `nninitStartup`, `_init_libc2`, every module's DT_INIT, then
//! `nnMain`. The SDK owns DT_INIT_ARRAY. The host's services, takeovers and stubs are bound before relocation.
//!
//! Arguments: `<game>.exe <data dir>` (or `NXOS_DATA`): the build's data folder, holding `exefs/` and the RomFS
//! marker (`romfs.nsp`, `romfs.nup`, `romfs.dir` or `romfs/`). Saves go to `<data dir>/save` (or `NXOS_SAVE`).

use crate::guest;
use nxrt::cpu::Cpu;
use nxrt::dispatch::{Module, RtModule, Table};
use nxrt::mem::{Range, Space, KERNEL_BASE};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct Process {
    pub data_dir: PathBuf,
    pub save_dir: PathBuf,
    /// The application's display version (control.nacp +0x3060).
    pub display_version: String,
    /// Modules in load order: (name, base, image size, DT_INIT offset).
    pub modules: Vec<(String, u64, u64, Option<u64>)>,
}

static PROCESS: OnceLock<Process> = OnceLock::new();
static INITIALIZERS: OnceLock<Vec<(String, Vec<u64>)>> = OnceLock::new();

pub fn process() -> &'static Process {
    PROCESS.get().expect("os: process not started")
}

pub fn try_process() -> Option<&'static Process> {
    PROCESS.get()
}

/// Something the executable registers before the boot (the graphics layer's NVN procs: `nvn::register`).
pub type InitHook = unsafe extern "C" fn();

/// `int nxos_main(int argc, char **argv, const RtModule *mods, int count)`
#[no_mangle]
pub unsafe extern "C" fn nxos_main(
    argc: i32,
    argv: *const *const core::ffi::c_char,
    mods: *const RtModule,
    count: i32,
) -> i32 {
    nxos_main_with(argc, argv, mods, count, None)
}

/// The same with a hook the executable runs once the os layer is set up, before the modules are bound.
#[no_mangle]
pub unsafe extern "C" fn nxos_main_with(
    argc: i32,
    argv: *const *const core::ffi::c_char,
    mods: *const RtModule,
    count: i32,
    hook: Option<InitHook>,
) -> i32 {
    let args: Vec<String> = (0..argc as usize)
        .map(|i| {
            std::ffi::CStr::from_ptr(*argv.add(i))
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let data = match args.get(1) {
        Some(a) => PathBuf::from(a),
        None => match std::env::var("NXOS_DATA") {
            Ok(d) => PathBuf::from(d),
            Err(_) => {
                eprintln!("usage: <game>.exe <data dir>   (NXOS_DATA also names it; NXOS_TRACE=svc,ipc,hle,nvn; NXRT_TRACE for the runtime)");
                nxrt::process::exit(2);
            }
        },
    };
    let modules: Vec<Module> = (0..count as usize)
        .map(|i| Module::from_raw(&*mods.add(i)))
        .collect();
    match nxrt::boundary::guard("nxos_main", || run(&data, modules, hook)) {
        Ok(()) => loop {
            std::thread::park();
        },
        Err(e) => nxrt::rt::fatal(&e),
    }
}

/// The display version of a control.nacp (+0x3060, a C string of up to 16 bytes), when it is one (it starts
/// with a digit).
pub fn nacp_version(nacp: &[u8]) -> Option<String> {
    let v = nacp.get(0x3060..0x3070)?;
    let v = &v[..v.iter().position(|&c| c == 0).unwrap_or(v.len())];
    let s = String::from_utf8_lossy(v).trim_end_matches(' ').to_string();
    s.starts_with(|c: char| c.is_ascii_digit()).then_some(s)
}

fn run(data: &Path, modules: Vec<Module>, hook: Option<InitHook>) -> Result<(), String> {
    crate::log::secs();
    if let Some(n) = std::env::var("NXOS_HOST_CPUS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&n| n > 0 && n < 64)
    {
        #[cfg(windows)]
        let ok = {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, SetProcessAffinityMask,
            };
            let mask = (1usize << n) - 1;
            unsafe { SetProcessAffinityMask(GetCurrentProcess(), mask) != 0 }
        };
        #[cfg(target_os = "linux")]
        let ok = nxrt::linux::limit_cpus(n);
        crate::log!(
            "host CPUs limited to {n} (NXOS_HOST_CPUS){}",
            if ok { "" } else { ": failed" }
        );
    }
    if modules.is_empty() {
        return Err("no modules".into());
    }
    let exefs = data.join("exefs");
    let npdm = std::fs::read(exefs.join("main.npdm")).ok();
    let over = std::fs::read_to_string(data.join("profile.tsv")).ok();
    let prof = crate::profile::set(crate::profile::Profile::load(
        npdm.as_deref(),
        over.as_deref(),
    ));
    crate::svc::install();
    crate::log!(
        "{} ({:016X}): pool {:#x}, counter {} Hz, core mask {:#x}{}",
        prof.name,
        prof.program_id,
        prof.pool,
        prof.tick_hz,
        prof.core_mask,
        if prof.switch2 { ", Switch 2" } else { "" }
    );
    // HD rumble: a Switch 2 program's amplitudes take RenHDrum's Switch 2 laws (process-wide in the core)
    crate::input::rumble::set_program(prof.program_id);

    let lo = modules.iter().map(|m| m.base).min().unwrap();
    let hi = modules.iter().map(|m| m.base + m.size).max().unwrap();
    let mut space = Space::new(
        Range {
            base: lo,
            size: hi - lo,
        },
        KERNEL_BASE,
        true,
    )?;
    space.pool = prof.pool;
    let space: &'static Space = Box::leak(Box::new(space));
    crate::mem::init(
        space,
        crate::mem::Figures {
            pool: prof.pool,
            used_beside_heap: prof.used_beside_heap,
            sys_resource: prof.sys_resource,
        },
    );

    // NRO modules the game loads itself (nro.tsv): in the module window and the dispatch table, mapped at their load
    let nros = crate::services::ro::read_table(data);
    let mut files = vec![];
    // a stubs module (frame-step / stub patches, `<module>_stubs`) has no file: its code is translated, its first
    // page is the frame step's state page, mapped below
    let stubs: Vec<(u64, u64)> = modules
        .iter()
        .filter(|m| m.name.ends_with("_stubs"))
        .map(|m| (m.base, m.size))
        .collect();
    for m in modules
        .iter()
        .filter(|m| !nros.contains(&m.name) && !m.name.ends_with("_stubs"))
    {
        let p = exefs.join(&m.name);
        let raw = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        files.push((m.name.clone(), m.base, m.size, raw));
    }
    let syms = crate::hle::ModuleSyms::read(
        &files
            .iter()
            .map(|f| (f.0.clone(), f.3.clone()))
            .collect::<Vec<_>>(),
    )?;

    let save = std::env::var("NXOS_SAVE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data.join("save"));
    let mods_info = files
        .iter()
        .map(|(name, base, size, raw)| {
            let init = nxrt::loader::parse(raw)
                .ok()
                .and_then(|img| nxrt::loader::dynamic(&img.bytes).ok())
                .and_then(|dynamic| dynamic.init);
            (name.clone(), *base, *size, init)
        })
        .collect();
    // the title's version in the boot log (the build's control data, the update's when it came with one)
    let version = std::fs::read(data.join("control.nacp"))
        .ok()
        .and_then(|n| nacp_version(&n));
    match &version {
        Some(v) => crate::log!("title: version {v} (control.nacp)"),
        None => crate::log!("title: no version (no control.nacp in {})", data.display()),
    }
    let display_version = version.unwrap_or_else(|| "1.0.0".into());
    let _ = PROCESS.set(Process {
        data_dir: data.to_path_buf(),
        save_dir: save,
        display_version,
        modules: mods_info,
    });
    // code the build missed runs in the interpreter and is written to <data>\discovered.tsv for the next build
    nxrt::interp::set_data_dir(data);

    // the host's side, bound before relocation
    nxrt::fiber::register();
    let mut reg = crate::hle::Registry::default();
    crate::services::register(&mut reg);
    if let Some(h) = hook {
        unsafe { h() };
    }
    crate::nvn::finish_registration(&mut reg);
    let inst = crate::hle::install(&reg, &syms);
    crate::log!(
        "host functions: {} registered, {} take over SDK code the game references, {} for imports nothing defines, {} aliases; {} imports stubbed: {}",
        reg.len(),
        inst.forced,
        inst.hle,
        inst.aliased,
        inst.stubbed.len(),
        inst.stubbed.join(" ")
    );

    let (loaded, rep) = nxrt::loader::load(space, &files)?;
    // The SDK runs DT_INIT_ARRAY itself. Host execution double-runs Unity constructors, and OoT's whole array reaches
    // constructors before their prerequisites. Keep it only as an explicit initialization-order diagnostic.
    let init_array = std::env::var("NXOS_INIT_ARRAY").is_ok_and(|v| v != "0");
    if init_array {
        crate::log!("loader: host DT_INIT_ARRAY execution enabled for this title");
    }
    let initializers = loaded
        .iter()
        .map(|m| Ok((m.name.clone(), m.initializers(init_array)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let _ = INITIALIZERS.set(initializers);
    for (i, &(base, size)) in stubs.iter().enumerate() {
        space.map_module(base, size)?;
        space.protect_segment(base, size, nxrt::mem::ST_CODEDATA, nxrt::mem::PERM_RW)?;
        if i == 0 {
            crate::fstep::init(base, data);
        }
    }
    let table = nxrt::dispatch::install(Table::build(modules)?)?;
    nxrt::fault::install();
    crate::mem::install_fault_handler();
    crate::hle::set_symbols(&loaded);
    crate::services::ro::set_base(&loaded);
    crate::gamefix::install(&loaded);
    if let Some(m) = loaded.iter().find(|m| m.name == "main") {
        crate::fpslock::set_main(crate::fpslock::Main {
            base: m.base,
            size: m.size,
            text_end: m.image.text.0 + m.image.text.1,
            ro_off: m.image.rodata.0,
            data_off: m.image.data.0,
            build: m
                .image
                .build_id
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect(),
        });
    }
    let names: Vec<String> = loaded
        .iter()
        .map(|m| format!("{} {:#x}+{:#x}", m.name, m.base, m.size))
        .collect();
    crate::log!(
        "loaded {}; {} lifted functions; {} relocations; {} references bound to the host",
        names.join(", "),
        table.functions,
        rep.relocations,
        rep.host_bound.len()
    );
    if crate::mods::init(data) {
        crate::mods::apply_bytes(
            &loaded
                .iter()
                .map(|m| (m.name.clone(), m.base, m.size))
                .collect::<Vec<_>>(),
        );
    }
    if !rep.unresolved.is_empty() {
        crate::log!(
            "[RED] unresolved after binding: {}",
            rep.unresolved.join(" ")
        );
    }

    // the main thread at its host function (the runtime's start request 4); before it exists: the profile's counter
    // rate over the runtime start-up's, and the virtual cores
    nxrt::process::start_main(
        space,
        npdm.as_deref(),
        nxrt::process::Entry::Host(boot_main),
        |m| {
            crate::svc::set_tick_hz(prof.tick_hz);
            crate::sched::init(prof.core_mask);
            crate::sched::set_main(m.priority, m.core);
            crate::log!(
                "main thread: priority {}, core {}, stack {:#x}",
                m.priority,
                m.core,
                m.stack_size
            );
        },
    )?;
    crate::options::watch();
    std::thread::spawn(|| {
        let _ = std::panic::catch_unwind(crate::watch::run);
    });
    Ok(())
}

fn need(name: &str) -> u64 {
    crate::hle::sym(name)
        .unwrap_or_else(|| nxrt::rt::fatal(&format!("os boot: the modules do not define {name}")))
}

/// The main guest thread's first function (x1 = its handle; it holds a core already, `sched::start`): the SDK's
/// start, then nnMain.
unsafe fn boot_main(c: *mut Cpu) {
    let hmain = (*c).x[1];
    let step = |c: *mut Cpu, name: &str, args: &[u64]| -> bool {
        crate::log!(">>> {name}");
        guest::call(c, need(name), args).is_some()
    };
    if !step(c, "_init_libc0", &[])
        || !step(c, "nnosInitialize", &[hmain, 0])
        || !step(c, "_init_libc1", &[])
    {
        return;
    }
    if !step(c, "nninitInitializeSdkModule", &[]) {
        return;
    }
    // the game's arguments: argv[0] = "game"
    let space = nxrt::kernel::get().space;
    let s = space.kernel_alloc(16, 16);
    guest::write_bytes(s, b"game\0");
    let argv = space.kernel_alloc(16, 16);
    guest::w64(argv, s);
    guest::w64(argv + 8, 0);
    if !step(c, "_ZN2nn2os11SetHostArgcEi", &[1]) || !step(c, "_ZN2nn2os11SetHostArgvEPPc", &[argv])
    {
        return;
    }
    if !step(c, "nninitStartup", &[]) || !step(c, "_init_libc2", &[]) {
        return;
    }
    for (name, initializers) in INITIALIZERS
        .get()
        .expect("os: module initializers not loaded")
    {
        for &init in initializers {
            crate::log!("{name}: initializer {init:#x}");
            if guest::call(c, init, &[]).is_none() {
                return;
            }
        }
    }
    crate::log!(">>> nnMain");
    if guest::call(c, need("nnMain"), &[]).is_none() {
        return;
    }
    crate::log!("nnMain returned");
    crate::log::report_gaps("nnMain returned");
    nxrt::process::exit(0);
}

#[cfg(test)]
mod tests {
    /// control.nacp's display version at +0x3060, logged when it starts with a digit.
    #[test]
    fn title_version_from_control_nacp() {
        let mut n = vec![0u8; 0x4000];
        n[0x3060..0x3065].copy_from_slice(b"3.0.1");
        assert_eq!(super::nacp_version(&n).as_deref(), Some("3.0.1"));
        n[0x3060..0x3070].copy_from_slice(b"1.2.0-build.4567");
        assert_eq!(super::nacp_version(&n).as_deref(), Some("1.2.0-build.4567"));
        n[0x3060] = b'v';
        assert_eq!(super::nacp_version(&n), None);
        assert_eq!(super::nacp_version(&n[..0x3000]), None);
    }
}
