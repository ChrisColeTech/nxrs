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
    let t =
        TRACE.get_or_init(|| trace_spec(std::env::var("NXRT_TRACE").ok(), nxconfig::int("log")));
    !t.is_empty() && (t == "all" || t.split(',').any(|w| w == what))
}

/// The traces on: `NXRT_TRACE` when set, else everything for the ini's `log=2`, else none.
pub fn trace_spec(env: Option<String>, log: i64) -> String {
    env.filter(|v| !v.is_empty()).unwrap_or_else(|| {
        if log >= 2 {
            "all".into()
        } else {
            String::new()
        }
    })
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

/// The printable text a guest pointer points to (at most 64 bytes, readable pages only), for a probe line.
pub fn guest_cstr(p: u64) -> Option<String> {
    if p < 0x1_0000 || !crate::mem::readable(p, 1) {
        return None;
    }
    let mut s = String::new();
    for i in 0..64u64 {
        if !crate::mem::readable(p + i, 1) {
            break;
        }
        // SAFETY: checked readable
        let b = unsafe { *((p + i) as *const u8) };
        if b == 0 {
            break;
        }
        if !(0x20..0x7f).contains(&b) {
            return None;
        }
        s.push(b as char);
    }
    (s.len() >= 2).then_some(s)
}

/// Which hits of a probe print (nxnative DM_PROBE_LIMIT / DM_PROBE_EVERY): every `every`-th, at most `limit`
/// printed (0: no limit). `hit` counts from 1.
pub fn probe_prints(hit: u64, every: u64, limit: u64) -> bool {
    let every = every.max(1);
    hit % every == 0 && (limit == 0 || hit / every <= limit)
}

/// A probe point (nxrs-build `NXRS_PROBE=<module>:<offset>,...`, nxnative probe.c's DM_PROBE, here placed by the
/// translator): the thread, x0..x3 (`NXRT_PROBE_ALL=1`: x0..x18) with the strings they point to, and the guest
/// registers' frame chain; then the instruction runs on. `NXRT_PROBE_EVERY=N` prints every N-th hit (its `#` the
/// true count), `NXRT_PROBE_LIMIT=N` at most N printed per probe (20 by default; 0 no limit).
/// Does not modify the CPU state or retain its pointer; updates probe counters and may log diagnostics.
///
/// # Safety
/// `c` must be non-null, properly aligned and point to a fully initialized `Cpu` that remains live and
/// readable for the entire call. No alias may mutate that state during the call; shared read-only aliases
/// are permitted. These requirements apply even when this hit is not printed.
/// Any guest memory read for strings, frame chains or `NXRT_FATAL_PEEK` must remain valid and initialized
/// for those reads, including alignment for `u64` loads, without concurrent writes or unmapping.
/// Readable-page checks do not synchronize access or keep allocations alive.
///
/// Rust callers must explicitly uphold this contract, even for a valid CPU:
/// ```compile_fail,E0133
/// let mut cpu = nxrt::cpu::Cpu::new_boxed();
/// nxrt::rt::rt_probe(&mut *cpu);
/// ```
#[no_mangle]
pub unsafe extern "C" fn rt_probe(c: *mut Cpu) {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static HITS: Mutex<Option<HashMap<u64, u64>>> = Mutex::new(None);
    static CFG: OnceLock<(u64, u64, bool)> = OnceLock::new();
    let env = |k: &str, d: u64| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(d)
    };
    let &(every, limit, all) = CFG.get_or_init(|| {
        (
            env("NXRT_PROBE_EVERY", 1),
            env("NXRT_PROBE_LIMIT", 20),
            env("NXRT_PROBE_ALL", 0) != 0,
        )
    });
    // SAFETY: the caller guarantees a live, aligned, initialized CPU with no mutation during this call.
    let c = unsafe { &*c };
    let hit = {
        let mut g = HITS.lock().unwrap_or_else(|p| p.into_inner());
        let n = g.get_or_insert_with(HashMap::new).entry(c.pc).or_insert(0);
        *n += 1;
        *n
    };
    if !probe_prints(hit, every, limit) {
        return;
    }
    let thread = crate::kernel::current_handle().map_or("host".to_string(), |h| format!("{h:#x}"));
    let mut s = format!(
        "probe {} #{hit} thread {thread}\n",
        crate::dispatch::guest_name(c.pc)
    );
    for r in 0..if all { 19 } else { 4 } {
        s.push_str(&format!("  x{r:<2} {:#018x}", c.x[r]));
        if let Some(t) = guest_cstr(c.x[r]) {
            s.push_str(&format!("  \"{t}\""));
        }
        s.push('\n');
    }
    s.push_str(&guest_report(c));
    log(&s);
}

/// Registers and the frame-pointer chain of a guest state.
pub fn guest_report(c: &Cpu) -> String {
    let mut s = String::new();
    for i in (0..31).step_by(4) {
        let row: Vec<String> = (i..(i + 4).min(31))
            .map(|r| format!("x{r:<2} {:#018x}", c.x[r]))
            .collect();
        s.push_str(&format!("  {}\n", row.join("  ")));
    }
    s.push_str(&format!(
        "  sp  {:#018x}  pc {}  tpidr {:#x}  tpidrro {:#x}\n",
        c.sp,
        crate::dispatch::guest_name(c.pc),
        c.tpidr,
        c.tpidrro
    ));
    s.push_str(&format!("  lr  {}\n", crate::dispatch::guest_name(c.x[30])));
    let mut fp = c.x[29];
    for n in 0..32 {
        if fp == 0 || fp & 7 != 0 || !crate::mem::readable(fp, 16) {
            break;
        }
        // SAFETY: checked readable.
        let (next, lr) = unsafe { (*(fp as *const u64), *((fp + 8) as *const u64)) };
        s.push_str(&format!(
            "  guest frame {n:2}: {}\n",
            crate::dispatch::guest_name(lr)
        ));
        if next <= fp || next - fp > (64 << 20) {
            break;
        }
        fp = next;
    }
    if let Ok(e) = std::env::var("NXRT_FATAL_PEEK") {
        for chain in e.split(';') {
            s.push_str(&peek(c, chain));
        }
    }
    s
}

/// `NXRT_FATAL_PEEK=<chain>[;<chain>...]` (a debugging aid for crash reports): a chain is `<reg or hex>[+-off]`
/// followed by `>off` steps (read the u64 there, then add off); 0x100 bytes at the end are dumped. Example:
/// `sp+0x90>0x2a78>0` = the u64 at [[sp+0x90]+0x2a78].
fn peek(c: &Cpu, chain: &str) -> String {
    fn num(t: &str) -> Option<i64> {
        let t = t.trim();
        let (neg, t) = t
            .strip_prefix('-')
            .map(|r| (true, r))
            .unwrap_or((false, t.strip_prefix('+').unwrap_or(t)));
        let v = if let Some(h) = t.strip_prefix("0x") {
            i64::from_str_radix(h, 16).ok()?
        } else {
            t.parse().ok()?
        };
        Some(if neg { -v } else { v })
    }
    let mut parts = chain.split('>');
    let head = parts.next().unwrap_or("");
    let (base, off) = match head.find(['+', '-']) {
        Some(i) => (&head[..i], num(&head[i..]).unwrap_or(0)),
        None => (head, 0),
    };
    let b = match base.trim() {
        "sp" => c.sp,
        "lr" => c.x[30],
        "fp" => c.x[29],
        r if r.starts_with('x') => r[1..]
            .parse::<usize>()
            .ok()
            .filter(|&i| i < 31)
            .map(|i| c.x[i])
            .unwrap_or(0),
        h => num(h).unwrap_or(0) as u64,
    };
    let mut a = b.wrapping_add(off as u64);
    let mut s = format!("  peek {chain}: {a:#x}");
    for st in parts {
        if !crate::mem::readable(a, 8) {
            s.push_str(" (unreadable)\n");
            return s;
        }
        // SAFETY: checked readable.
        let v = unsafe { *(a as *const u64) };
        a = v.wrapping_add(num(st).unwrap_or(0) as u64);
        s.push_str(&format!(" -> {v:#x} -> {a:#x}"));
    }
    s.push('\n');
    for row in 0..16u64 {
        let p = a + row * 16;
        if !crate::mem::readable(p, 16) {
            break;
        }
        // SAFETY: checked readable.
        let b = unsafe { core::slice::from_raw_parts(p as *const u8, 16) };
        let hex: Vec<String> = b
            .chunks(8)
            .map(|q| format!("{:016x}", u64::from_le_bytes(q.try_into().unwrap())))
            .collect();
        let asc: String = b
            .iter()
            .map(|&x| {
                if (0x20..0x7f).contains(&x) {
                    x as char
                } else {
                    '.'
                }
            })
            .collect();
        s.push_str(&format!("    {p:#014x}: {}  {asc}\n", hex.join(" ")));
    }
    s
}

#[cfg(windows)]
fn qpc() -> (u64, u64) {
    use windows_sys::Win32::System::Performance::{
        QueryPerformanceCounter, QueryPerformanceFrequency,
    };
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
#[cfg(windows)]
pub fn now_ns() -> u64 {
    let (n, f) = qpc();
    (n as u128 * 1_000_000_000 / f as u128) as u64
}

/// Host nanoseconds since an arbitrary start (CLOCK_MONOTONIC).
#[cfg(target_os = "linux")]
pub fn now_ns() -> u64 {
    crate::linux::now_ns()
}

/// The monotonic clock as (count, rate): QueryPerformanceCounter's shape, in ns.
#[cfg(target_os = "linux")]
fn qpc() -> (u64, u64) {
    (crate::linux::now_ns(), 1_000_000_000)
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
    crate::boundary::guard("rt_ticks", || {
        match CLOCK.load(std::sync::atomic::Ordering::Acquire) {
            0 => {
                let (n, f) = qpc();
                (n as u128 * tick_hz() as u128 / f as u128) as u64
            }
            // SAFETY: only `set_clock` stores here, always a `fn() -> u64`.
            p => {
                let ns = unsafe { core::mem::transmute::<usize, fn() -> u64>(p) }();
                (ns as u128 * tick_hz() as u128 / 1_000_000_000) as u64
            }
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
        guest_fatal(
            c,
            &format!("guest BRK #{imm:#x} at {}", crate::dispatch::guest_name(pc)),
        )
    })
}

/// An instruction the translator could not lift (or fell off the end of the code).
#[no_mangle]
pub unsafe extern "C" fn rt_udf(c: *mut Cpu, pc: u64) -> ! {
    crate::boundary::guard("rt_udf", || {
        (*c).pc = pc;
        let w = if crate::mem::readable(pc, 4) {
            format!("{:08x}", *(pc as *const u32))
        } else {
            "????????".into()
        };
        guest_fatal(
            c,
            &format!(
                "undefined or unlifted instruction {w} at {}",
                crate::dispatch::guest_name(pc)
            ),
        )
    })
}

/// The function-entry profiler hook (`FN_ENTER`); reached only while `g_fnprof` is set, which nothing sets yet.
#[no_mangle]
pub extern "C" fn fnprof_hit(_addr: u64, _lr: u64, _x0: u64) {}

#[cfg(test)]
mod tests {
    /// nxnative DM_PROBE_EVERY / DM_PROBE_LIMIT: every N-th hit prints, at most LIMIT of them; probe strings are
    /// printable text the registers point to.
    #[test]
    fn probe_hits_and_strings() {
        let printed = |every, limit| {
            (1..=100u64)
                .filter(|&h| super::probe_prints(h, every, limit))
                .collect::<Vec<_>>()
        };
        assert_eq!(printed(1, 20).len(), 20);
        assert_eq!(printed(10, 0), (1..=10).map(|k| 10 * k).collect::<Vec<_>>());
        assert_eq!(printed(10, 3), vec![10, 20, 30]);
        let s = b"rom:/Layout/title.bflyt\0";
        assert_eq!(
            super::guest_cstr(s.as_ptr() as u64).as_deref(),
            Some("rom:/Layout/title.bflyt")
        );
        let bin = [1u8, 2, 3, 0];
        assert_eq!(super::guest_cstr(bin.as_ptr() as u64), None);
        assert_eq!(super::guest_cstr(0x10), None);
        let mut c = crate::cpu::Cpu::new_boxed();
        c.pc = 0x1234;
        c.x[0] = s.as_ptr() as u64;
        let probe: crate::cpu::GuestFn = super::rt_probe;
        // SAFETY: the box owns a live, initialized CPU, exclusively borrowed for this call; its string
        // points to the live immutable `s`, and the zero frame pointer ends the frame chain.
        unsafe { probe(&mut *c) }; // logs, changes nothing
        assert_eq!((c.pc, c.x[0]), (0x1234, s.as_ptr() as u64));
    }

    #[test]
    fn ticks_advance_at_the_rate() {
        let (t0, n0) = (super::rt_ticks(), super::now_ns());
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (t1, n1) = (super::rt_ticks(), super::now_ns());
        let hz = (t1 - t0) as f64 / ((n1 - n0) as f64 / 1e9);
        assert!((hz / super::tick_hz() as f64 - 1.0).abs() < 0.01, "{hz}");
    }
}
