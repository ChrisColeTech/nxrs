//! nn::time: the clocks come from the host (the user and network clocks are the host's real time, the steady clock
//! its monotonic clock), and calendar conversions use the host's time zone, as nxnative does. `NXOS_FIXED_TIME`
//! (POSIX seconds) pins the user clock for comparable runs.

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

/// ToCalendarTime into (CalendarTime*, CalendarAdditionalInfo*), in the host's zone.
fn to_calendar(ct: u64, ai: u64, t: i64) {
    let (off, name) = local_offset();
    let (c, wday, yday) = calendar(t + off);
    if ct != 0 {
        crate::guest::write_bytes(ct, &c);
    }
    if ai != 0 {
        crate::guest::zero(ai, 0x18);
        w32(ai, wday);
        w32(ai + 4, yday);
        crate::guest::put_cstr(ai + 8, 8, &name);
        w32(ai + 0x10, 0);
        w32(ai + 0x14, off as i32 as u32);
    }
}

fn steady_point(out: u64) {
    w64(out, steady_secs() as u64);
    crate::guest::write_bytes(out + 8, STEADY_ID);
}

static CONTEXT: Mutex<Option<([u8; 0x20], i64)>> = Mutex::new(None);

/// The user clock's SystemClockContext {i64 offset, SteadyClockTimePoint}: one context until the host clock moves.
fn context(out: u64) {
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
    crate::guest::write_bytes(out, &g.as_ref().unwrap().0);
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
tfn!(span_between, |a| {
    let (out, x, y) = (a.x(0), a.x(1), a.x(2));
    let same = crate::guest::bytes(x + 0x98, 16) == crate::guest::bytes(y + 0x98, 16);
    let s = if same {
        r64(y + 0x90) as i64 - r64(x + 0x90) as i64
    } else if r64(x + 0x48) != 0 && r64(y + 0x48) != 0 {
        r64(y + 0x48) as i64 - r64(x + 0x48) as i64
    } else {
        return 0;
    };
    w64(out, (s * 1_000_000_000) as u64);
    1
});
tfn!(user_diff, |a| {
    let (x, y) = (a.x(0), a.x(1));
    if crate::guest::bytes(x + 0x10, 16) != crate::guest::bytes(y + 0x10, 16) {
        return 0;
    }
    ((r64(y) as i64 - r64(x) as i64) * 1_000_000_000) as u64
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

pub fn register(r: &mut Registry) {
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
}
