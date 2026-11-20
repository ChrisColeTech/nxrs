//! nn::time: the clocks come from the host (the user and network clocks are the host's real time, the steady clock
//! its monotonic clock), and calendar conversions use the host's time zone, as nxnative does. `NXOS_FIXED_TIME`
//! (POSIX seconds) pins the user clock for comparable runs.
//!
//! The same clocks are served over IPC (`time:u`, `time:a`, `time:s`) for an SDK whose time client is not taken
//! over by name, with the service's shared memory (GetSharedMemoryNativeHandle, command 20): the SDK reads the
//! clocks from it without a request. Its layout (0x1000 bytes; each value a lock-free pair: a u32 counter, then
//! two copies, the counter's low bit picking the current one; a writer fills the other copy, then bumps it):
//!
//! | Offset | Value (each copy) |
//! |---|---|
//! | 0x00 (copies at 0x08, 0x20) | steady clock {i64 base time ns, clock source id[16]}: steady time = system counter ns + base |
//! | 0x38 (copies at 0x40, 0x60) | local system clock context {i64 offset s, i64 steady time point s, id[16]} |
//! | 0x80 (copies at 0x88, 0xA8) | network system clock context, the same as the local one |
//! | 0xC8 (bools at 0xCC, 0xCD) | automatic correction: on |
//! | 0xD0 (copies at 0xD8, 0x110, 0x38 bytes) | ContinuousAdjustmentTimePoint {i64 counter ns at setup, i64 multiplier 0, i64 shift 0, i64 lower = base, i64 upper = base, id[16] at +0x28} |
//!
//! The base makes the page's steady clock this module's (`crate::clock::now_ns()`), the contexts are the ones the
//! takeovers report, and every clock source id is `STEADY_ID`, so a context and the steady clock always match
//! (SDK 22's CalculateSystemClockCurrentTimespec compares the adjustment's id at +0x28 with the context's; a
//! mismatch fails clock_gettime(CLOCK_REALTIME)). A multiplier of 0 adds no drift: the adjusted steady clock is
//! the counter plus `lower`. A host thread writes the page into every view the game maps (each view is fresh
//! memory) and rewrites the contexts when the host clock is set.

use crate::guest::{r64, w32, w64, w8, Args};
use crate::hle::{Registry, RES_FAIL, RES_OK};
use nxrt::cpu::Cpu;
use std::sync::Mutex;
use std::sync::OnceLock;

const STEADY_ID: &[u8; 16] = b"nxos-steady-0001";

fn fixed_time() -> Option<i64> {
    static F: OnceLock<Option<i64>> = OnceLock::new();
    *F.get_or_init(|| std::env::var("NXOS_FIXED_TIME").ok().and_then(|v| v.parse().ok()))
}

pub fn now() -> i64 {
    fixed_time().unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64))
}

fn steady_secs() -> i64 {
    (crate::clock::now_ns() / 1_000_000_000) as i64
}

/// The host's offset from UTC now, in seconds (daylight saving included), and its zone abbreviation.
pub fn local_offset() -> (i64, String) {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut tz: TIME_ZONE_INFORMATION = unsafe { core::mem::zeroed() };
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    let mut bias = tz.Bias as i64;
    if r == 2 {
        bias += tz.DaylightBias as i64;
    } else {
        bias += tz.StandardBias as i64;
    }
    let off = -bias * 60;
    let h = off / 3600;
    let name = if off == 0 { "UTC".to_string() } else { format!("UTC{h:+}") };
    (off, name)
}

/// Days since 1970-01-01 → (year, month, day) (proleptic Gregorian).
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day) → days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// CalendarTime {i16 year, i8 month, day, hour, minute, second, pad} and (weekday, day of year).
pub fn calendar(t: i64) -> ([u8; 8], u32, u32) {
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    let mut c = [0u8; 8];
    c[0..2].copy_from_slice(&(y as i16).to_le_bytes());
    c[2] = m as u8;
    c[3] = d as u8;
    c[4] = (secs / 3600) as u8;
    c[5] = (secs / 60 % 60) as u8;
    c[6] = (secs % 60) as u8;
    let wday = (days + 4).rem_euclid(7) as u32;
    let yday = (days - days_from_civil(y, 1, 1)) as u32;
    (c, wday, yday)
}

fn from_calendar(ct: u64) -> i64 {
    let y = crate::guest::r16(ct) as i16 as i64;
    let (mo, d, h, mi, s) = (crate::guest::r8(ct + 2), crate::guest::r8(ct + 3), crate::guest::r8(ct + 4), crate::guest::r8(ct + 5), crate::guest::r8(ct + 6));
    days_from_civil(y, mo as u32, d as u32) * 86_400 + h as i64 * 3600 + mi as i64 * 60 + s as i64
}

/// CalendarTime and CalendarAdditionalInfo {u32 weekday, u32 day of year, name[8], u32 dst, i32 offset} of `t` in
/// the host's zone.
fn calendar_local(t: i64) -> ([u8; 8], [u8; 0x18]) {
    let (off, name) = local_offset();
    let (c, wday, yday) = calendar(t + off);
    let mut ai = [0u8; 0x18];
    ai[0..4].copy_from_slice(&wday.to_le_bytes());
    ai[4..8].copy_from_slice(&yday.to_le_bytes());
    let n = name.as_bytes();
    ai[8..8 + n.len().min(7)].copy_from_slice(&n[..n.len().min(7)]);
    ai[0x14..0x18].copy_from_slice(&(off as i32).to_le_bytes());
    (c, ai)
}

/// ToCalendarTime into (CalendarTime*, CalendarAdditionalInfo*), in the host's zone.
fn to_calendar(ct: u64, ai: u64, t: i64) {
    let (c, info) = calendar_local(t);
    if ct != 0 {
        crate::guest::write_bytes(ct, &c);
    }
    if ai != 0 {
        crate::guest::write_bytes(ai, &info);
    }
}

fn steady_point(out: u64) {
    w64(out, steady_secs() as u64);
    crate::guest::write_bytes(out + 8, STEADY_ID);
}

static CONTEXT: Mutex<Option<([u8; 0x20], i64)>> = Mutex::new(None);

/// The user clock's SystemClockContext {i64 offset, SteadyClockTimePoint}: one context until the host clock moves.
fn context_bytes() -> [u8; 0x20] {
    let now = now();
    let steady = steady_secs();
    let mut g = CONTEXT.lock().unwrap();
    let d = now - steady;
    if g.as_ref().is_none_or(|(_, drift)| fixed_time().is_none() && (d - drift).abs() > 1) {
        let mut ctx = [0u8; 0x20];
        ctx[0..8].copy_from_slice(&(now - steady).to_le_bytes());
        ctx[8..16].copy_from_slice(&steady.to_le_bytes());
        ctx[16..32].copy_from_slice(STEADY_ID);
        *g = Some((ctx, d));
    }
    g.as_ref().unwrap().0
}
fn context(out: u64) {
    crate::guest::write_bytes(out, &context_bytes());
}

macro_rules! tfn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

tfn!(ok, |a| { RES_OK });
tfn!(one, |a| { 1 });
tfn!(zero, |a| { 0 });
tfn!(fail, |a| { RES_FAIL });
tfn!(steady_now, |a| {
    steady_point(a.x(0));
    RES_OK
});
tfn!(steady_chrono, |a| { steady_secs() as u64 });
tfn!(user_now, |a| {
    w64(a.x(0), now() as u64);
    RES_OK
});
tfn!(chrono_now, |a| { now() as u64 });
tfn!(to_time_t, |a| { r64(a.x(0)) });
tfn!(get_context, |a| {
    context(a.x(0));
    RES_OK
});
tfn!(timespec, |a| {
    let ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    w64(a.x(0), now() as u64);
    w64(a.x(0) + 8, if fixed_time().is_some() { 0 } else { ns as u64 });
    0
});
tfn!(mono_timespec, |a| {
    let ns = crate::clock::now_ns();
    w64(a.x(0), ns / 1_000_000_000);
    w64(a.x(0) + 8, ns % 1_000_000_000);
    0
});
tfn!(to_calendar_fn, |a| {
    to_calendar(a.x(0), a.x(1), r64(a.x(2)) as i64);
    RES_OK
});
tfn!(to_calendar_utc, |a| {
    let (c, _, _) = calendar(r64(a.x(0)) as i64);
    u64::from_le_bytes(c)
});
tfn!(to_posix, |a| {
    // (int *count, PosixTime *out, int n, const CalendarTime *ct): local time to UTC seconds
    let (count, out, n, ct) = (a.x(0), a.x(1), a.i(2), a.x(3));
    let t = from_calendar(ct) - local_offset().0;
    crate::guest::out(count, (n > 0) as u32);
    if n > 0 && out != 0 {
        w64(out, t as u64);
    }
    RES_OK
});
tfn!(to_posix_utc, |a| { from_calendar(a.x(0)) as u64 });
tfn!(location, |a| {
    crate::guest::zero(a.x(0), 0x24);
    crate::guest::put_cstr(a.x(0), 0x24, &local_offset().1);
    RES_OK
});
tfn!(load_rule, |a| {
    crate::log_once!("tzrule", "time::LoadTimeZoneRule {} (the host's zone is used)", crate::guest::cstr(a.x(1), 0x24));
    RES_OK
});
tfn!(snapshot, |a| {
    snapshot_fill(a.x(0), 1);
    RES_OK
});
tfn!(adj_snapshot, |a| {
    snapshot_fill(a.x(0), 2);
    RES_OK
});
/// The span (ns) between two ClockSnapshots: their steady time points when one clock, else their network times.
fn span_of(x: u64, y: u64) -> Option<i64> {
    let same = crate::guest::bytes(x + 0x98, 16) == crate::guest::bytes(y + 0x98, 16);
    let s = if same {
        r64(y + 0x90) as i64 - r64(x + 0x90) as i64
    } else if r64(x + 0x48) != 0 && r64(y + 0x48) != 0 {
        r64(y + 0x48) as i64 - r64(x + 0x48) as i64
    } else {
        return None;
    };
    Some(s * 1_000_000_000)
}
/// How far the user moved the user clock between two snapshots (ns): the change of their offsets.
fn user_diff_of(x: u64, y: u64) -> i64 {
    if crate::guest::bytes(x + 0x10, 16) != crate::guest::bytes(y + 0x10, 16) {
        return 0;
    }
    (r64(y) as i64 - r64(x) as i64) * 1_000_000_000
}
tfn!(span_between, |a| {
    let (out, x, y) = (a.x(0), a.x(1), a.x(2));
    match span_of(x, y) {
        Some(s) => {
            w64(out, s as u64);
            1
        }
        None => 0,
    }
});
tfn!(user_diff, |a| { user_diff_of(a.x(0), a.x(1)) as u64 });
/// ClaimPeriodicBenefitWithUserSystemClock: the public entry only checks the time service session (which the
/// host never opens: it would abort) and goes on to the SDK's detail:: implementation, which compares the
/// snapshots and asks the two questions below.
unsafe fn claim_benefit(c: *mut Cpu) {
    const DETAIL: &str = "_ZN2nn4time6detail39ClaimPeriodicBenefitWithUserSystemClockEPNS0_27PeriodicBenefitClaimContextERKNS0_13ClockSnapshotERKNS_8TimeSpanES9_PKNS0_25CheckCalendarTimeCallbackE";
    let a = Args(&mut *c);
    let args = [a.x(0), a.x(1), a.x(2), a.x(3), a.x(4)];
    let Some(f) = crate::hle::sym(DETAIL) else {
        nxrt::rt::guest_fatal(c, "time: the SDK has ClaimPeriodicBenefitWithUserSystemClock but not its detail:: implementation");
    };
    if let Some(r) = crate::guest::call(c, f, &args) {
        crate::log!("time: periodic benefit claim -> {}", r as u32);
        (*c).x[0] = r;
    }
}
// When automatic correction was last switched: not known on the host, a time point of no clock (its id matches
// no snapshot's, so the claim uses only the snapshots).
tfn!(auto_corr_updated, |a| {
    crate::guest::zero(a.x(0), 0x18);
    RES_OK
});
// The device's location and when it was set: the host's zone, and a time point of no clock.
tfn!(location_updated, |a| {
    crate::guest::zero(a.x(0), 0x24);
    crate::guest::put_cstr(a.x(0), 0x24, &local_offset().1);
    crate::guest::zero(a.x(1), 0x18);
    RES_OK
});
unsafe fn netclock_event(c: *mut Cpu) {
    static EV: OnceLock<u64> = OnceLock::new();
    let ev = *EV.get_or_init(|| nxrt::kernel::get().space.kernel_alloc(0x100, 16));
    crate::services::os::signal_system_event(c, ev);
    (*c).x[0] = ev;
}
unsafe fn netclock_start(c: *mut Cpu) {
    netclock_event(c);
    (*c).x[0] = RES_OK;
}

/// ClockSnapshot (0xd0 bytes): the user and network clock contexts, both times, their calendars, the steady time
/// point, the location name and the type at +0xcd.
fn snapshot_fill(s: u64, ty: u8) {
    crate::guest::zero(s, 0xd0);
    let now = now();
    context(s);
    w64(s + 0x40, now as u64);
    to_calendar(s + 0x50, s + 0x60, now);
    let ctx = crate::guest::bytes(s, 0x20).to_vec();
    crate::guest::write_bytes(s + 0x20, &ctx);
    w64(s + 0x48, now as u64);
    let cal = crate::guest::bytes(s + 0x50, 8).to_vec();
    crate::guest::write_bytes(s + 0x58, &cal);
    let info = crate::guest::bytes(s + 0x60, 0x18).to_vec();
    crate::guest::write_bytes(s + 0x78, &info);
    steady_point(s + 0x90);
    crate::guest::put_cstr(s + 0xa8, 0x24, &local_offset().1);
    w8(s + 0xcd, ty);
}

// ---- the time service over IPC, and its shared memory

const SHM_SIZE: usize = 0x1000;

/// The page as the views should hold it, and the views already written.
struct Page {
    shm: &'static crate::svc::SharedMem,
    img: Vec<u8>,
    filled: Vec<u64>,
    ctx: [u8; 0x20],
}

static PAGE: Mutex<Option<Page>> = Mutex::new(None);

impl Page {
    /// Writes `v` as the next copy of the lock-free pair at `off` (copies `align` past it, `size` apart), then
    /// bumps its counter: in the image and in every view.
    fn put(&mut self, views: &[u64], off: usize, align: usize, size: usize, v: &[u8]) {
        let cnt = u32::from_le_bytes(self.img[off..off + 4].try_into().unwrap()).wrapping_add(1);
        let at = off + align + (cnt as usize & 1) * size;
        self.img[at..at + v.len()].copy_from_slice(v);
        self.img[off..off + 4].copy_from_slice(&cnt.to_le_bytes());
        for &b in views.iter().filter(|b| self.filled.contains(b)) {
            crate::guest::write_bytes(b + at as u64, v);
            std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
            w32(b + off as u64, cnt);
        }
    }
}

/// The steady clock's base: steady time (ns) at system counter 0.
fn steady_base() -> i64 {
    static B: OnceLock<i64> = OnceLock::new();
    *B.get_or_init(|| crate::clock::now_ns() as i64 - crate::svc::guest_ns() as i64)
}

fn page_new() -> Page {
    let shm = crate::svc::new_shared(SHM_SIZE as u64);
    page_fill(shm, steady_base(), crate::svc::guest_ns() as i64)
}

/// The page's first contents: the steady clock from `base`, both system clock contexts, automatic correction on,
/// the ContinuousAdjustmentTimePoint set up at system counter `now_ns`.
fn page_fill(shm: &'static crate::svc::SharedMem, base: i64, now_ns: i64) -> Page {
    let mut p = Page { shm, img: vec![0; SHM_SIZE], filled: vec![], ctx: context_bytes() };
    let mut steady = [0u8; 0x18];
    steady[0..8].copy_from_slice(&base.to_le_bytes());
    steady[8..24].copy_from_slice(STEADY_ID);
    p.put(&[], 0x00, 8, 0x18, &steady);
    let ctx = p.ctx;
    p.put(&[], 0x38, 8, 0x20, &ctx);
    p.put(&[], 0x80, 8, 0x20, &ctx);
    p.put(&[], 0xC8, 4, 1, &[1]);
    let mut adj = [0u8; 0x38];
    adj[0..8].copy_from_slice(&now_ns.to_le_bytes());
    adj[0x18..0x20].copy_from_slice(&base.to_le_bytes());
    adj[0x20..0x28].copy_from_slice(&base.to_le_bytes());
    adj[0x28..0x38].copy_from_slice(STEADY_ID);
    p.put(&[], 0xD0, 8, 0x38, &adj);
    p
}

/// One pass of the page's writer: new views get the whole page, a moved host clock new contexts. True once a
/// view has been written.
fn page_tick() -> bool {
    let mut g = PAGE.lock().unwrap();
    let Some(p) = g.as_mut() else { return false };
    // the views lock is held while writing: an unmap takes its view out before the memory goes
    let views = p.shm.views.lock().unwrap();
    p.filled.retain(|v| views.contains(v));
    for &v in views.iter() {
        if !p.filled.contains(&v) {
            crate::guest::write_bytes(v, &p.img);
            p.filled.push(v);
            crate::log_once!(format!("time shm {v:#x}"), "time: shared memory written at {v:#x} (steady base {} ns)", steady_base());
        }
    }
    let ctx = context_bytes();
    if ctx != p.ctx {
        p.ctx = ctx;
        let vs: Vec<u64> = views.clone();
        p.put(&vs, 0x38, 8, 0x20, &ctx);
        p.put(&vs, 0x80, 8, 0x20, &ctx);
        crate::log!("time: the host clock moved: new system clock contexts in the shared memory");
    }
    !p.filled.is_empty()
}

/// The shared memory's handle, made (with its writer thread) on the first request.
fn page_handle() -> u32 {
    let mut g = PAGE.lock().unwrap();
    if let Some(p) = g.as_ref() {
        return p.shm.handle;
    }
    let p = page_new();
    let h = p.shm.handle;
    *g = Some(p);
    drop(g);
    std::thread::Builder::new()
        .name("time shm".into())
        .spawn(|| loop {
            // a view is written within a millisecond of its mapping until the first one is; then 10 times a second
            let ms = if page_tick() { 100 } else { 1 };
            std::thread::sleep(std::time::Duration::from_millis(ms));
        })
        .expect("time shm thread");
    h
}

struct TimeService(&'static str);
struct SystemClock(&'static str, Option<crate::kobj::Event>);
struct SteadyClock;
struct TimeZone;

impl crate::ipc::Service for TimeService {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut crate::ipc::Ctx) -> u32 {
        match ctx.cmd() {
            0 => ctx.object(SystemClock("ISystemClock (user)", None)),
            1 => ctx.object(SystemClock("ISystemClock (network)", None)),
            4 => ctx.object(SystemClock("ISystemClock (local)", None)),
            5 => ctx.object(SystemClock("ISystemClock (ephemeral network)", None)),
            2 => ctx.object(SteadyClock),
            3 => ctx.object(TimeZone),
            20 => {
                let h = page_handle();
                crate::log_once!("time shm", "time: shared memory {h:#x} handed out");
                ctx.copy_handle(h);
            }
            // IsStandardUserSystemClockAutomaticCorrectionEnabled (as the page says)
            100 => ctx.push_u8(1),
            101 => {}
            // IsStandardNetworkSystemClockAccuracySufficient (as the by-name answer)
            200 => ctx.push_u8(0),
            // GetClockSnapshot(type) -> buffer
            400 => {
                let b = ctx.buf_out(0);
                if b.addr != 0 && b.size >= 0xd0 {
                    snapshot_fill(b.addr, ctx.u8(0));
                }
            }
            // CalculateStandardUserSystemClockDifferenceByUser / CalculateSpanBetween (two snapshots in)
            500 | 501 => {
                let (x, y) = (ctx.buf_in(0), ctx.buf_in(1));
                if x.addr == 0 || y.addr == 0 || x.size < 0xd0 || y.size < 0xd0 {
                    return crate::hle::RES_FAIL as u32;
                }
                if ctx.cmd() == 500 {
                    ctx.push_u64(user_diff_of(x.addr, y.addr) as u64);
                } else {
                    match span_of(x.addr, y.addr) {
                        Some(s) => ctx.push_u64(s as u64),
                        None => return crate::hle::RES_FAIL as u32,
                    }
                }
            }
            _ => return ctx.unimplemented(),
        }
        0
    }
}

impl crate::ipc::Service for SystemClock {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut crate::ipc::Ctx) -> u32 {
        match ctx.cmd() {
            0 => ctx.push_u64(now() as u64),
            1 | 3 => {}
            2 => ctx.push_bytes(&context_bytes()),
            // GetOperationEventReadableHandle: the clock is never operated on
            4 => {
                let e = self.1.get_or_insert_with(crate::kobj::Event::new);
                ctx.copy_handle(e.readable);
            }
            _ => return ctx.unimplemented(),
        }
        0
    }
}

impl crate::ipc::Service for SteadyClock {
    fn name(&self) -> String {
        "ISteadyClock".into()
    }
    fn call(&mut self, ctx: &mut crate::ipc::Ctx) -> u32 {
        match ctx.cmd() {
            0 => {
                ctx.push_u64(steady_secs() as u64);
                ctx.push_bytes(STEADY_ID);
            }
            // GetTestOffset, GetInternalOffset
            2 | 200 => ctx.push_u64(0),
            // GetRtcValue
            100 => ctx.push_u64(now() as u64),
            _ => return ctx.unimplemented(),
        }
        0
    }
}

impl crate::ipc::Service for TimeZone {
    fn name(&self) -> String {
        "ITimeZoneService".into()
    }
    fn call(&mut self, ctx: &mut crate::ipc::Ctx) -> u32 {
        match ctx.cmd() {
            0 => {
                let mut n = [0u8; 0x24];
                let z = local_offset().1;
                n[..z.len().min(0x23)].copy_from_slice(&z.as_bytes()[..z.len().min(0x23)]);
                ctx.push_bytes(&n);
            }
            1 => {}
            // GetTotalLocationNameCount
            2 => ctx.push_u32(1),
            // LoadTimeZoneRule: the host's zone is used, the rule stays unread
            4 => {}
            // ToCalendarTime(posix, rule) / ToCalendarTimeWithMyRule(posix)
            100 | 101 => {
                let (c, info) = calendar_local(ctx.u64(0) as i64);
                ctx.push_bytes(&c);
                ctx.push_bytes(&info);
            }
            // ToPosixTime(calendar, rule) / ToPosixTimeWithMyRule(calendar) -> count, posix times
            201 | 202 => {
                let raw = ctx.input().get(..8).map(|b| b.to_vec()).unwrap_or_default();
                if raw.len() < 8 {
                    return crate::hle::RES_FAIL as u32;
                }
                let y = i16::from_le_bytes([raw[0], raw[1]]) as i64;
                let t = days_from_civil(y, raw[2] as u32, raw[3] as u32) * 86_400 + raw[4] as i64 * 3600 + raw[5] as i64 * 60 + raw[6] as i64 - local_offset().0;
                let n = ctx.write_out(0, &t.to_le_bytes());
                ctx.push_u32((n >= 8) as u32);
            }
            _ => return ctx.unimplemented(),
        }
        0
    }
}

pub fn register(r: &mut Registry) {
    crate::ipc::register("time:u", || Box::new(TimeService("time:u")));
    crate::ipc::register("time:a", || Box::new(TimeService("time:a")));
    crate::ipc::register("time:s", || Box::new(TimeService("time:s")));
    r.force(
        "_ZN2nn4time39ClaimPeriodicBenefitWithUserSystemClockEPNS0_27PeriodicBenefitClaimContextERKNS0_13ClockSnapshotERKNS_8TimeSpanES8_PKNS0_25CheckCalendarTimeCallbackE",
        "the session check of the time service; the SDK's detail:: compares the snapshots",
        claim_benefit,
    );
    const W: &str = "the time service: the host's clocks and time zone";
    for (n, f) in [
        ("_ZN2nn4time10InitializeEv", ok as unsafe fn(*mut Cpu)),
        ("_ZN2nn4time8FinalizeEv", ok),
        ("_ZN2nn4time13IsInitializedEv", one),
        ("_ZN2nn4time19StandardSteadyClock19GetCurrentTimePointEPNS0_20SteadyClockTimePointE", steady_now),
        ("_ZN2nn4time38GetStandardSteadyClockCurrentTimePointEPNS0_20SteadyClockTimePointE", steady_now),
        ("_ZN2nn4time19StandardSteadyClock3nowEv", steady_chrono),
        ("_ZN2nn4time23StandardUserSystemClock14GetCurrentTimeEPNS0_9PosixTimeE", user_now),
        ("_ZN2nn4time25AdjustableUserSystemClock14GetCurrentTimeEPNS0_9PosixTimeE", user_now),
        ("_ZN2nn4time26StandardNetworkSystemClock14GetCurrentTimeEPNS0_9PosixTimeE", user_now),
        ("_ZN2nn4time28AdjustableNetworkSystemClock14GetCurrentTimeEPNS0_9PosixTimeE", user_now),
        ("_ZN2nn4time41GetStandardUserSystemClockCurrentTimespecEP8timespec", timespec),
        ("_ZN2nn4time43GetAdjustableUserSystemClockCurrentTimespecEP8timespec", timespec),
        ("_ZN2nn4time44GetStandardNetworkSystemClockCurrentTimespecEP8timespec", timespec),
        ("_ZN2nn4time46GetAdjustableNetworkSystemClockCurrentTimespecEP8timespec", timespec),
        ("_ZN2nn4time37GetStandardSteadyClockCurrentTimespecEP8timespec", mono_timespec),
        ("_ZN2nn4time23StandardUserSystemClock21GetSystemClockContextEPNS0_18SystemClockContextE", get_context),
        ("_ZN2nn4time26StandardNetworkSystemClock21GetSystemClockContextEPNS0_18SystemClockContextE", get_context),
        ("_ZN2nn4time25AdjustableUserSystemClock21GetSystemClockContextEPNS0_18SystemClockContextE", get_context),
        ("_ZN2nn4time28AdjustableNetworkSystemClock21GetSystemClockContextEPNS0_18SystemClockContextE", get_context),
        ("_ZN2nn4time23StandardUserSystemClock3nowEv", chrono_now),
        ("_ZN2nn4time26StandardNetworkSystemClock3nowEv", chrono_now),
        ("_ZN2nn4time25AdjustableUserSystemClock3nowEv", chrono_now),
        ("_ZN2nn4time28AdjustableNetworkSystemClock3nowEv", chrono_now),
        ("_ZN2nn4time23StandardUserSystemClock9to_time_tERKNSt3__16chrono10time_pointINS0_17SystemClockTraitsENS3_8durationIlNS2_5ratioILl1ELl1EEEEEEE", to_time_t),
        ("_ZN2nn4time14ToCalendarTimeEPNS0_12CalendarTimeEPNS0_22CalendarAdditionalInfoERKNS0_9PosixTimeE", to_calendar_fn),
        ("_ZN2nn4time14ToCalendarTimeEPNS0_12CalendarTimeEPNS0_22CalendarAdditionalInfoERKNS0_9PosixTimeERKNS0_12TimeZoneRuleE", to_calendar_fn),
        ("_ZN2nn4time19ToCalendarTimeInUtcERKNS0_9PosixTimeE", to_calendar_utc),
        ("_ZN2nn4time11ToPosixTimeEPiPNS0_9PosixTimeEiRKNS0_12CalendarTimeE", to_posix),
        ("_ZN2nn4time11ToPosixTimeEPiPNS0_9PosixTimeEiRKNS0_12CalendarTimeERKNS0_12TimeZoneRuleE", to_posix),
        ("_ZN2nn4time18ToPosixTimeFromUtcERKNS0_12CalendarTimeE", to_posix_utc),
        ("_ZN2nn4time21GetDeviceLocationNameEPNS0_12LocationNameE", location),
        ("_ZN2nn4time16LoadTimeZoneRuleEPNS0_12TimeZoneRuleERKNS0_12LocationNameE", load_rule),
        ("_ZN2nn4time26StandardNetworkSystemClock20IsAccuracySufficientEv", zero),
        ("_ZN2nn4time13ClockSnapshot29CreateWithStandardSystemClockEPS1_", snapshot),
        ("_ZN2nn4time13ClockSnapshot31CreateWithAdjustableSystemClockEPS1_", adj_snapshot),
        ("_ZN2nn4time20CalculateSpanBetweenEPNS_8TimeSpanERKNS0_13ClockSnapshotES5_", span_between),
        ("_ZN2nn4time48CalculateStandardUserSystemClockDifferenceByUserERKNS0_13ClockSnapshotES3_", user_diff),
        ("_ZN2nn4time39AsyncEnsureNetworkClockAvailabilityTaskC1Ev", zero),
        ("_ZN2nn4time39AsyncEnsureNetworkClockAvailabilityTaskD1Ev", zero),
        ("_ZN2nn4time39AsyncEnsureNetworkClockAvailabilityTask10InitializeENS_2os14EventClearModeE", ok),
        ("_ZN2nn4time39AsyncEnsureNetworkClockAvailabilityTask9StartTaskEv", netclock_start),
        ("_ZN2nn4time39AsyncEnsureNetworkClockAvailabilityTask14GetFinishEventEv", netclock_event),
        ("_ZN2nn4time39AsyncEnsureNetworkClockAvailabilityTask9GetResultEv", fail),
        ("_ZN2nn4time6detail56GetStandardUserSystemClockAutomaticCorrectionUpdatedTimeEPNS0_20SteadyClockTimePointE", auto_corr_updated),
        ("_ZN2nn4time6detail35GetDeviceLocationNameAndUpdatedTimeEPNS0_12LocationNameEPNS0_20SteadyClockTimePointE", location_updated),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_math() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(days_from_civil(2026, 11, 19)), (2026, 11, 19));
        assert_eq!(days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 28), 2, "2000 is a leap year");
        let (c, wday, yday) = calendar(1_700_000_000); // 2023-11-14 22:13:20 UTC, a Tuesday
        assert_eq!(i16::from_le_bytes([c[0], c[1]]), 2023);
        assert_eq!(&c[2..7], &[11, 14, 22, 13, 20]);
        assert_eq!((wday, yday), (2, 317));
    }

    #[test]
    fn lock_free_pairs() {
        // the page writer's pairs: the counter picks the copy, a write fills the other one first
        let shm: &'static crate::svc::SharedMem = Box::leak(Box::new(crate::svc::SharedMem { handle: 0, size: 0x1000, views: Mutex::new(vec![]) }));
        let mut p = Page { shm, img: vec![0; SHM_SIZE], filled: vec![], ctx: [0; 0x20] };
        p.put(&[], 0x38, 8, 0x20, &[7; 0x20]);
        assert_eq!(u32::from_le_bytes(p.img[0x38..0x3c].try_into().unwrap()), 1);
        assert_eq!(&p.img[0x60..0x80], &[7; 0x20], "counter 1: the second copy");
        p.put(&[], 0x38, 8, 0x20, &[9; 0x20]);
        assert_eq!(&p.img[0x40..0x60], &[9; 0x20], "counter 2: the first copy");
        p.put(&[], 0xC8, 4, 1, &[1]);
        assert_eq!(p.img[0xCD], 1);
        let mut adj = [0u8; 0x38];
        adj[0x28..0x38].copy_from_slice(STEADY_ID);
        p.put(&[], 0xD0, 8, 0x38, &adj);
        assert_eq!(&p.img[0x110 + 0x28..0x110 + 0x38], STEADY_ID, "the adjustment's clock source id at +0x28");
    }

    /// The page as Turbo 1f12977 lays it out, read the way SDK 22's SharedMemoryClient does: the local context's
    /// current copy (counter & 1), the ContinuousAdjustmentTimePoint at 0xD0 with 0x38-byte entries at 0xD8 / 0x110,
    /// the current one picked by counter & 1, its clock source id at +0x28 equal to the context's (a mismatch is
    /// result 52340 and clock_gettime(CLOCK_REALTIME) fails), multiplier and shift 0; and the steady clock's id.
    #[test]
    fn shared_page_layout_as_turbo() {
        let shm: &'static crate::svc::SharedMem = Box::leak(Box::new(crate::svc::SharedMem { handle: 0, size: 0x1000, views: Mutex::new(vec![]) }));
        let p = page_fill(shm, 1_000_000_000, 123_456);
        let img = &p.img;
        let rd32 = |o: usize| u32::from_le_bytes(img[o..o + 4].try_into().unwrap());
        let rd64 = |o: usize| i64::from_le_bytes(img[o..o + 8].try_into().unwrap());
        let pick = |off: usize, align: usize, size: usize| off + align + (rd32(off) as usize & 1) * size;
        let steady = pick(0x00, 8, 0x18);
        assert_eq!((rd64(steady), &img[steady + 8..steady + 24]), (1_000_000_000, &STEADY_ID[..]));
        let local = pick(0x38, 8, 0x20);
        let ctx_id = &img[local + 0x10..local + 0x20];
        assert_eq!(ctx_id, STEADY_ID);
        assert_eq!(img[pick(0xC8, 4, 1)], 1, "automatic correction on");
        assert_eq!(rd32(0xD0), 1, "the adjustment's counter");
        let adj = pick(0xD0, 8, 0x38);
        assert_eq!(adj, 0xD8 + 0x38, "counter 1: the second 0x38-byte entry");
        assert_eq!(&img[adj + 0x28..adj + 0x38], ctx_id, "the entry's id at +0x28 is the context's");
        assert_eq!((rd64(adj), rd64(adj + 8), rd64(adj + 0x10)), (123_456, 0, 0), "set up now, multiplier 0, shift 0");
        assert!(img[0xD8..0xD8 + 0x38].iter().all(|&b| b == 0), "the other entry untouched");
    }
}
