//! The guest's clock: the host's monotonic time at a rate the front end may lower (Turbo's virtual time, for
//! `smooth_recording`: a video recorded while the PC cannot keep up stays smooth). The system counter, the steady
//! clock and vi's vsync follow it; 1.0 is real time.

use std::sync::Mutex;

struct State {
    real: u64,
    virt: u64,
    rate: f64,
}

static S: Mutex<State> = Mutex::new(State { real: 0, virt: 0, rate: 1.0 });

fn at(s: &State, real: u64) -> u64 {
    if s.rate == 1.0 {
        return s.virt + (real - s.real.min(real));
    }
    s.virt + ((real - s.real.min(real)) as f64 * s.rate) as u64
}

/// The guest clock in ns (the host's monotonic time scale; equal to it until a rate is set).
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
    let rate = if rate.is_finite() { rate.clamp(0.05, 4.0) } else { 1.0 };
    let real = nxrt::rt::now_ns();
    let mut s = S.lock().unwrap();
    let virt = if s.real == 0 { real } else { at(&s, real) };
    *s = State { real, virt, rate };
}

pub fn rate() -> f64 {
    S.lock().unwrap().rate
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
}
