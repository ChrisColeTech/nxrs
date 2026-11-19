//! The loader-to-run path: claim the guest address space, load the ExeFS modules at the translator's bases, bind
//! imports, install the dispatch table and the crash reporter, then start the main thread at `main`'s entry with its
//! stack and TLS, and run until the guest exits.
//!
//! The executable that links the translated code calls `nxrt_main` from its `main` with the generated module table
//! (see `build.rs` in `bin/nxrt_dev.rs`: `modules.c`).

use crate::cpu::Cpu;
use crate::dispatch::{Module, RtModule, Table};
use crate::mem::{Range, Space, KERNEL_BASE};
use std::io::Write;
use std::path::PathBuf;

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

/// `int nxrt_main(int argc, char **argv, const RtModule *mods, int count)`
#[no_mangle]
pub unsafe extern "C" fn nxrt_main(argc: i32, argv: *const *const core::ffi::c_char, mods: *const RtModule, count: i32) -> i32 {
    let args: Vec<String> = (0..argc as usize).map(|i| std::ffi::CStr::from_ptr(*argv.add(i)).to_string_lossy().into_owned()).collect();
    let exefs = match args.get(1) {
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

fn run(exefs: &std::path::Path, modules: Vec<Module>) -> Result<(), String> {
    if modules.is_empty() {
        return Err("no modules".into());
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
    let npdm = std::fs::read(exefs.join("main.npdm")).ok();
    let (prio, core, stack_size) = crate::loader::npdm_main_thread(npdm.as_deref());
    let stack = space.alloc_stack(stack_size)?;
    let main = loaded.iter().find(|m| m.name == "main").or(loaded.first()).unwrap();
    let k = crate::kernel::init(space);
    k.create_main(main.entry(), stack + crate::mem::align_up(stack_size, crate::mem::PAGE), prio, core);
    Ok(())
}
