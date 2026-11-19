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
}
static CLOCK: Mutex<Clock> = Mutex::new(Clock { have: false, off: 0, last: 0 });

/// A submission ran on the GPU from `t0` to `t1` (GPU clock, ns); called at its completion.
pub fn part(_t0: u64, t1: u64) {
    let mut c = CLOCK.lock().unwrap();
    let cand = guest_ns() as i64 - t1 as i64;
    c.off = if !c.have || cand < c.off + 2000 { cand } else { c.off + 2000 };
    c.have = true;
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
                let x = s.clamp(t0, t1) as i64 + c.off;
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

/// Writes NVN's counter record {counter, time} at `addr`.
pub fn write_record(addr: u64, counter: u64, t: u64) {
    // SAFETY: the game's report memory (16 bytes, possibly unaligned)
    unsafe { core::ptr::write_unaligned(addr as *mut [u64; 2], [counter, t]) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn report_times_follow_the_gpu_and_never_go_back() {
        super::part(1_000, 2_000);
        let a = super::report_time(Some(1_500), 1_000, 2_000);
        let b = super::report_time(Some(1_200), 1_000, 2_000);
        assert!(b >= a);
    }
}
