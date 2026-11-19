//! The small runtime services generated code calls (cpu.h): `rt_ticks`, `rt_spin_gate`, `rt_brk`, `rt_udf`,
//! `fnprof_hit`; plus logging, fatal errors and the guest backtrace the reports print.

use crate::cpu::Cpu;
use std::io::Write;
use std::sync::OnceLock;

/// The Switch 2's system counter: 31.25 MHz (32 ns a tick). MKW's sdk returns 31,250,000 from
/// nn::os::GetSystemTickFrequency and converts with it.
pub const TICK_HZ_SWITCH2: u32 = 31_250_000;
/// A Switch program's: 19.2 MHz.
pub const TICK_HZ_SWITCH1: u32 = 19_200_000;
/// The rate before start-up chose one (`process::startup`).
pub const TICK_HZ: u64 = TICK_HZ_SWITCH2 as u64;

/// `RT_TICK_HZ` of the generated code (cpu.h: CNTFRQ_EL0) and the rate of `rt_ticks`: set once at start-up, before
/// any guest code runs, by the program's console (`set_tick_hz`).
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static mut rt_tick_hz: u32 = TICK_HZ_SWITCH2;

/// The counter rate (CNTFRQ_EL0, svcGetSystemTick).
pub fn tick_hz() -> u32 {
    // SAFETY: written only before guest threads start
    unsafe { core::ptr::addr_of!(rt_tick_hz).read_volatile() }
}

/// Sets the counter rate; call before any guest code runs.
pub fn set_tick_hz(hz: u32) {
    // SAFETY: start-up, one thread
    unsafe { core::ptr::addr_of_mut!(rt_tick_hz).write_volatile(hz) }
}

static TRACE: OnceLock<String> = OnceLock::new();

/// `NXRT_TRACE=svc,host,sjlj,thread,load` (or `all`).
pub fn trace(what: &str) -> bool {
    // NXRT_TRACE, else the ini's log=2 ("everything", slow): every trace
    let t = TRACE.get_or_init(|| {
        std::env::var("NXRT_TRACE").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| if nxconfig::int("log") >= 2 { "all".into() } else { String::new() })
    });
    !t.is_empty() && (t == "all" || t.split(',').any(|w| w == what))
}

/// "[nxrt] msg" on stderr, in one write.
pub fn log(msg: &str) {
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(format!("[nxrt] {msg}\n").as_bytes());
}

/// A fatal runtime error: the message, then exit code 70.
pub fn fatal(msg: &str) -> ! {
    let _ = std::io::stdout().flush();
    log(&format!("FATAL: {msg}"));
    let _ = std::io::stderr().flush();
    std::process::exit(70);
}

/// A fatal error on a guest thread: the message, the guest registers and backtrace, then exit.
// `c` is null or the calling thread's own guest state (every caller passes one of those).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn guest_fatal(c: *const Cpu, msg: &str) -> ! {
    let _ = std::io::stdout().flush();
    log(&format!("FATAL: {msg}"));
    if !c.is_null() {
        // SAFETY: the caller's own guest state.
        log(&guest_report(unsafe { &*c }));
    }
    let _ = std::io::stderr().flush();
    std::process::exit(70);
}

/// Registers and the frame-pointer chain of a guest state.
pub fn guest_report(c: &Cpu) -> String {
    let mut s = String::new();
    for i in (0..31).step_by(4) {
        let row: Vec<String> = (i..(i + 4).min(31)).map(|r| format!("x{r:<2} {:#018x}", c.x[r])).collect();
        s.push_str(&format!("  {}\n", row.join("  ")));
    }
    s.push_str(&format!("  sp  {:#018x}  pc {}  tpidr {:#x}  tpidrro {:#x}\n", c.sp, crate::dispatch::guest_name(c.pc), c.tpidr, c.tpidrro));
    s.push_str(&format!("  lr  {}\n", crate::dispatch::guest_name(c.x[30])));
    let mut fp = c.x[29];
    for n in 0..32 {
        if fp == 0 || fp & 7 != 0 || !crate::mem::readable(fp, 16) {
            break;
        }
        // SAFETY: checked readable.
        let (next, lr) = unsafe { (*(fp as *const u64), *((fp + 8) as *const u64)) };
        s.push_str(&format!("  guest frame {n:2}: {}\n", crate::dispatch::guest_name(lr)));
        if next <= fp || next - fp > (64 << 20) {
            break;
        }
        fp = next;
    }
    s
}

fn qpc() -> (u64, u64) {
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    static FREQ: OnceLock<u64> = OnceLock::new();
    let f = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe { QueryPerformanceFrequency(&mut f) };
        f as u64
    });
    let mut n = 0i64;
    unsafe { QueryPerformanceCounter(&mut n) };
    (n as u64, f)
}

/// Host nanoseconds since an arbitrary start.
pub fn now_ns() -> u64 {
    let (n, f) = qpc();
    (n as u128 * 1_000_000_000 / f as u128) as u64
}

static CLOCK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The counter's time base: a function returning the guest clock in ns (the os layer's clock, whose rate the front
/// end may lower). Unset, the counter follows the host's monotonic clock. Set before guest code runs.
pub fn set_clock(f: fn() -> u64) {
    CLOCK.store(f as usize, std::sync::atomic::Ordering::Release);
}

/// CNTPCT_EL0 / CNTVCT_EL0 and svcGetSystemTick: ticks at [`tick_hz`].
#[no_mangle]
pub extern "C" fn rt_ticks() -> u64 {
    crate::boundary::guard("rt_ticks", || match CLOCK.load(std::sync::atomic::Ordering::Acquire) {
        0 => {
            let (n, f) = qpc();
            (n as u128 * tick_hz() as u128 / f as u128) as u64
        }
        // SAFETY: only `set_clock` stores here, always a `fn() -> u64`.
        p => {
            let ns = unsafe { core::mem::transmute::<usize, fn() -> u64>(p) }();
            (ns as u128 * tick_hz() as u128 / 1_000_000_000) as u64
        }
    })
}

/// One pass of a spin-wait loop the generator marked (`recomp/patches.py` `spin`): the scheduler's checkpoint
/// (`Scheduler::spin_pass`), then give the host core away.
#[no_mangle]
pub extern "C" fn rt_spin_gate(_c: *mut Cpu, _head: u64) {
    crate::boundary::guard("rt_spin_gate", || {
        crate::kernel::sched().spin_pass();
        std::thread::yield_now();
    });
}

/// BRK #imm: the guest's abort / assertion.
#[no_mangle]
pub unsafe extern "C" fn rt_brk(c: *mut Cpu, imm: u32, pc: u64) -> ! {
    crate::boundary::guard("rt_brk", || {
        (*c).pc = pc;
        guest_fatal(c, &format!("guest BRK #{imm:#x} at {}", crate::dispatch::guest_name(pc)))
    })
}

/// An instruction the translator could not lift (or fell off the end of the code).
#[no_mangle]
pub unsafe extern "C" fn rt_udf(c: *mut Cpu, pc: u64) -> ! {
    crate::boundary::guard("rt_udf", || {
        (*c).pc = pc;
        let w = if crate::mem::readable(pc, 4) { format!("{:08x}", *(pc as *const u32)) } else { "????????".into() };
        guest_fatal(c, &format!("undefined or unlifted instruction {w} at {}", crate::dispatch::guest_name(pc)))
    })
}

/// The function-entry profiler hook (`FN_ENTER`); reached only while `g_fnprof` is set, which nothing sets yet.
#[no_mangle]
pub extern "C" fn fnprof_hit(_addr: u64, _lr: u64, _x0: u64) {}

#[cfg(test)]
mod tests {
    #[test]
    fn ticks_advance_at_the_rate() {
        let (t0, n0) = (super::rt_ticks(), super::now_ns());
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (t1, n1) = (super::rt_ticks(), super::now_ns());
        let hz = (t1 - t0) as f64 / ((n1 - n0) as f64 / 1e9);
        assert!((hz / super::tick_hz() as f64 - 1.0).abs() < 0.01, "{hz}");
    }
}
