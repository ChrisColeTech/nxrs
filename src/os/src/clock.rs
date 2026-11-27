//! The guest's clock: the host's monotonic time at a rate the front end may lower (virtual time, for
//! `smooth_recording`: a video recorded while the PC cannot keep up stays smooth). The system counter, the steady
//! clock and vi's vsync follow it; 1.0 is real time.
//!
//! A hold (`set_hold`, the web page's in-game menu, WEB-W9) stops it: no guest time passes while held, and the time
//! held is never seen by the game (it resumes where it stopped, no jump).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

struct State {
    real: u64,
    virt: u64,
    rate: f64,
    held: bool,
}

static S: Mutex<State> = Mutex::new(State {
    real: 0,
    virt: 0,
    rate: 1.0,
    held: false,
});
static HELD: AtomicBool = AtomicBool::new(false);

fn at(s: &State, real: u64) -> u64 {
    if s.held {
        return s.virt;
    }
    if s.rate == 1.0 {
        return s.virt + (real - s.real.min(real));
    }
    s.virt + ((real - s.real.min(real)) as f64 * s.rate) as u64
}

/// The guest clock in ns (the host's monotonic time scale; equal to it until a rate or a hold is set).
pub fn now_ns() -> u64 {
    let real = nxrt::rt::now_ns();
    let s = S.lock().unwrap();
    if s.real == 0 {
        return real;
    }
    at(&s, real)
}

/// The guest clock's rate (1.0 normal, lower = slow motion); time already passed is kept.
pub fn set_rate(rate: f64) {
    let rate = if rate.is_finite() {
        rate.clamp(0.05, 4.0)
    } else {
        1.0
    };
    let real = nxrt::rt::now_ns();
    let mut s = S.lock().unwrap();
    let virt = if s.real == 0 { real } else { at(&s, real) };
    let held = s.held;
    *s = State {
        real,
        virt,
        rate,
        held,
    };
}

pub fn rate() -> f64 {
    S.lock().unwrap().rate
}

/// Holds (true) or releases the guest clock. While held it stands still; on release it goes on from the held value.
pub fn set_hold(on: bool) {
    let real = nxrt::rt::now_ns();
    let mut s = S.lock().unwrap();
    if s.held == on {
        return;
    }
    let virt = if s.real == 0 { real } else { at(&s, real) };
    s.real = real;
    s.virt = virt;
    s.held = on;
    HELD.store(on, Ordering::Release);
}

/// Whether the clock is held (cheap: for loops that wait out a hold).
pub fn held() -> bool {
    HELD.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    #[test]
    fn slows_and_keeps_time() {
        let a = super::now_ns();
        super::set_rate(0.5);
        let b = super::now_ns();
        assert!(b >= a);
        super::set_rate(1.0);
        assert!(super::now_ns() >= b);
    }

    /// A hold stops the clock; on release it goes on from where it stopped, without the time held.
    #[test]
    fn hold_stops_time_and_hides_the_gap() {
        super::set_rate(1.0);
        super::set_hold(true);
        assert!(super::held());
        let a = super::now_ns();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(super::now_ns(), a, "no time passes while held");
        super::set_hold(false);
        assert!(!super::held());
        let b = super::now_ns();
        assert!(b >= a && b - a < 20_000_000, "the 30 ms held are not seen: {}", b - a);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(super::now_ns() > b, "it runs again");
    }
}
