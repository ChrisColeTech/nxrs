//! NVN counter reports: timestamps with the GPU's own time and samples passed from occlusion queries.
//!
//! A game times its GPU work as the difference of two timestamp reports; that difference feeds its dynamic resolution
//! and frame-rate controller. Each report is stamped where it was recorded (`vkCmdWriteTimestamp2` at the top of the
//! pipeline for TIMESTAMP_TOP, after all commands for the rest), and the GPU's clock is brought to the guest clock by
//! the smallest difference seen between a submission's end stamp and the guest clock at its completion (it rises 2 µs a
//! submission to follow the two clocks drifting apart). NVN2 reports hold nanoseconds, NVN 614.4 MHz ticks
//! (`device::timestamp_from_ns`). With `game_dynamic_resolution=0` the reports carry a clock 256 times slower: the game
//! sees its GPU work take no time and stays at its largest size.
//!
//! SAMPLES_PASSED: ResetCounter starts counting on the queue (an occlusion query around the draws of each pass from
//! then on); ReportCounter writes the sum of the queries completed so far. The sum is the completion thread's, in
//! submission order.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

static GUEST_CLOCK: OnceLock<fn() -> u64> = OnceLock::new();

/// The guest clock in nanoseconds (the OS layer's system counter); graphics' own clock until it is set.
pub fn set_guest_clock(f: fn() -> u64) {
    let _ = GUEST_CLOCK.set(f);
}

pub fn guest_ns() -> u64 {
    match GUEST_CLOCK.get() {
        Some(f) => f(),
        None => crate::nvn::device::now_ns(),
    }
}

/// The game's own dynamic resolution (`game_dynamic_resolution`: auto = on).
pub fn drs() -> bool {
    nxconfig::tristate("game_dynamic_resolution").unwrap_or(true)
}

struct Clock {
    have: bool,
    off: i64,
    last: u64,
    /// the part (frame, or submission) the idle time is counted in, the GPU's idle time in it so far and where the
    /// last submission of it ended (GPU clock, ns)
    key: u64,
    idle: u64,
    prev_end: u64,
}
static CLOCK: Mutex<Clock> = Mutex::new(Clock {
    have: false,
    off: 0,
    last: 0,
    key: 0,
    idle: 0,
    prev_end: 0,
});

/// Which idle time reports leave out: the time the GPU sat idle between the submissions of
/// one frame (2, the default: present to present), or only inside one submission (1, nxnative's rule:
/// `NXGFX_REP_GAPS=1`). A game that spans its frame with two reports read the host's translation between its
/// submissions as GPU time with the second (Donkey Kong Bananza, 8 submissions a frame, sat at its smallest size).
pub fn rep_gaps() -> u32 {
    static V: OnceLock<u32> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_REP_GAPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2)
    })
}

/// The key of the part a submission's idle time is counted in: its frame (`rep_gaps` 2) or itself.
pub fn part_key(frame: u64, serial: u64) -> u64 {
    if rep_gaps() >= 2 {
        frame
    } else {
        serial | 1 << 63
    }
}

/// A submission of part `key` ran on the GPU from `t0` to `t1` (GPU clock, ns); called at its completion, in order.
pub fn part(key: u64, t0: u64, t1: u64) {
    let mut c = CLOCK.lock().unwrap();
    if !c.have || c.key != key {
        c.idle = 0;
    } else if t0 > c.prev_end {
        c.idle += t0 - c.prev_end;
    }
    let cand = guest_ns() as i64 - t1 as i64;
    c.off = if !c.have || cand < c.off + 2000 {
        cand
    } else {
        c.off + 2000
    };
    c.have = true;
    c.key = key;
    c.prev_end = t1;
}

/// A timestamp report's time in NVN units: `stamp` (GPU clock, ns) clamped to its submission's [t0, t1], on the
/// guest clock; the guest clock at completion when the GPU gave no stamp.
pub fn report_time(stamp: Option<u64>, t0: u64, t1: u64) -> u64 {
    let mut c = CLOCK.lock().unwrap();
    let ns = if !drs() {
        guest_ns() / 256
    } else {
        match stamp {
            Some(s) if c.have && t1 >= t0 && t1 > 0 => {
                // the GPU's idle time earlier in this frame is left out
                let x = s.clamp(t0, t1) as i64 + c.off - c.idle as i64;
                x.max(0) as u64
            }
            _ => guest_ns(),
        }
    };
    let ns = ns.max(c.last);
    c.last = ns;
    crate::nvn::device::timestamp_from_ns(ns)
}

static VIS: AtomicU64 = AtomicU64::new(0);

/// An occlusion query's samples, added at its submission's completion.
pub fn vis_add(n: u64) {
    VIS.fetch_add(n, Ordering::Relaxed);
}
pub fn vis_reset() {
    VIS.store(0, Ordering::Relaxed);
}
pub fn vis() -> u64 {
    VIS.load(Ordering::Relaxed)
}

/// Conditional rendering: SetRenderEnableConditional compares the 64-bit counters
/// at addr and addr + 16. agl wraps its clears in it: the CPU sets the record to 1 when a target needs a clear, each
/// clear is followed by ReportCounter(TIMESTAMP_TOP) there, and a timestamp's counter field is 0, so a target already
/// cleared is not cleared again. A timestamp report replayed but not landed yet (it lands when its submission
/// completes) counts with its known counter 0; an address a samples-passed report writes is known only on the GPU, and
/// a condition on it keeps drawing. Without this (addr + 8, the timestamp, compared; timestamps' counter the time)
/// the second clear ran and wiped the lit scene: Mario Kart 8 Deluxe's black world with only its lights' glow.
static RC: Mutex<Option<Rc>> = Mutex::new(None);
#[derive(Default)]
struct Rc {
    pending: std::collections::HashMap<u64, u32>,
    occl: std::collections::HashSet<u64>,
}

/// A report replayed at `addr` (timestamp: counter 0 once it lands; samples passed: unknown at replay).
pub fn rc_note(addr: u64, timestamp: bool) {
    let mut g = RC.lock().unwrap();
    let r = g.get_or_insert_with(Rc::default);
    if timestamp {
        *r.pending.entry(addr).or_insert(0) += 1;
    } else if r.occl.len() < 1 << 20 {
        r.occl.insert(addr);
    }
}

/// A timestamp report at `addr` landed.
pub fn rc_landed(addr: u64) {
    let mut g = RC.lock().unwrap();
    if let Some(r) = g.as_mut() {
        if let Some(n) = r.pending.get_mut(&addr) {
            *n -= 1;
            if *n == 0 {
                r.pending.remove(&addr);
            }
        }
    }
}

/// The counter at `addr` as the GPU will see it when it reaches the condition; None: not known before the GPU runs.
pub fn rc_value(addr: u64) -> Option<u64> {
    {
        let g = RC.lock().unwrap();
        if let Some(r) = g.as_ref() {
            if r.occl.contains(&addr) {
                return None;
            }
            if r.pending.contains_key(&addr) {
                return Some(0);
            }
        }
    }
    if addr == 0 {
        return None;
    }
    // SAFETY: the game's report memory (identity mapped guest memory, as the replay read it before)
    Some(unsafe { core::ptr::read_unaligned(addr as *const u64) })
}

/// Writes NVN's counter record {counter, time} at `addr`.
pub fn write_record(addr: u64, counter: u64, t: u64) {
    // SAFETY: the game's report memory (16 bytes, possibly unaligned)
    unsafe { core::ptr::write_unaligned(addr as *mut [u64; 2], [counter, t]) };
}

#[cfg(test)]
mod tests {
    static T: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Two submissions of one frame with the GPU idle 5 ms between them: a report at the end of the second is 5 ms
    /// earlier than its stamp says (the host's translation between them is not GPU time); a new frame counts again.
    #[test]
    fn idle_time_between_a_frames_submissions_is_left_out() {
        let _l = T.lock().unwrap_or_else(|e| e.into_inner());
        if !super::drs() {
            return;
        }
        let now = || crate::nvn::device::now_ns() + 1_000_000_000;
        let k = super::part_key(7, 100);
        let t1 = now();
        super::part(k, t1 - 2_000_000, t1);
        let a = super::report_time(Some(t1), t1 - 2_000_000, t1);
        let s = now();
        while now() - s < 7_000_000 {
            std::hint::spin_loop();
        }
        let t2 = now();
        super::part(k, t2 - 2_000_000, t2);
        let b = super::report_time(Some(t2), t2 - 2_000_000, t2);
        let idle = super::CLOCK.lock().unwrap().idle;
        assert!(idle >= 5_000_000 && idle < 6_000_000, "idle {idle}");
        let d = crate::nvn::device::timestamp_to_ns(b) as i64
            - crate::nvn::device::timestamp_to_ns(a) as i64;
        let want = (t2 - t1) as i64 - idle as i64;
        assert!(
            (d - want).abs() < 200_000,
            "the frame's GPU time without its idle gap: {d} ns, want {want}"
        );
        super::part(super::part_key(8, 101), now() - 1_000, now());
        assert_eq!(super::CLOCK.lock().unwrap().idle, 0, "a new frame");
    }

    /// nvnDeviceGetCurrentTimestampInNanoseconds is on the clock the reports carry (the guest clock), not graphics'
    /// own epoch: a controller comparing the two (Donkey Kong Bananza in nxnative) reads a true GPU time.
    #[test]
    fn the_current_timestamp_is_the_reports_clock() {
        let _l = T.lock().unwrap_or_else(|e| e.into_inner());
        fn guest() -> u64 {
            crate::nvn::device::now_ns() + 5_000_000_000_000
        }
        super::set_guest_clock(guest);
        let scale = if super::drs() { 1 } else { 256 };
        let a = super::guest_ns() / scale;
        let t = crate::nvn::device::current_timestamp_ns();
        let b = super::guest_ns() / scale;
        assert!(a <= t && t <= b, "{a} <= {t} <= {b}");
        assert!(t > crate::nvn::device::now_ns() / scale + 4_000_000_000_000 / scale, "not graphics' own clock");
    }

    #[test]
    fn report_times_follow_the_gpu_and_never_go_back() {
        let _l = T.lock().unwrap_or_else(|e| e.into_inner());
        super::part(super::part_key(1, 1), 1_000, 2_000);
        let a = super::report_time(Some(1_500), 1_000, 2_000);
        let b = super::report_time(Some(1_200), 1_000, 2_000);
        assert!(b >= a);
    }
}
