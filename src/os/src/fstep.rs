//! The frame step: the host side of the frame-step patches (`nxrs-build` places nxnative's FRAME_STEP /
//! MATCH_FRAME_STEP stubs in a patch area after main, `<main>_stubs`, whose first page is this state page).
//! nxnative's design (`hle_nvn.c` fstep_tick, `nvn_fstep.h`), reimplemented:
//!
//! * The state page: +0 the target fps (f32; 0 = off: the stubs keep the game's own timing), +0x10 the render
//!   scale (u32), +0x20 the step handed to the game for its next frame in vsyncs (1/60 s), +0x24 the game's fixed
//!   step (the stubs write it), +0x30 vsyncs stepped (the stubs count them), +0x40 dynamic resolution allowed (u32).
//! * The step controller: each frame stays on screen `k` vsyncs (the present interval, held by graphics); a
//!   frame's cost in vsyncs is the time since the last present. Three late frames in a row (each over `k` by
//!   `LATE`) step `k` up to the cheapest of them; `RUN` frames in a row that would fit `k − 1` (at `FIT` of it) try
//!   `k − 1`, a trial that fails within `RUN` frames waits twice as long before the next (1 s up to 8 s). Real time
//!   the game falls behind (a late frame) is a debt paid back one vsync a frame (the step is `k + 1` while it
//!   lasts), at most `DEBT_MAX`; `k` and the step stay within the stubs' largest step (`step_max`, 3 when 0).
//!   The target fps sets the shortest hold: 60 → 1, 30 → 2 (`fps_target`, `NXOS_FPS_TARGET`; 0 turns it off).
//!   Left out from nxnative's: the probes and grace windows for clocks a phone or a Mac lowers under a long hold.
//! * `<data>\frame_step.txt` (written by `nxrs-build`): `step_max`, `render_scale`, `capabilities`.

use std::sync::Mutex;

pub const LATE: f64 = 0.1;
pub const FIT: f64 = 0.95;
pub const RUN: u32 = 30;
pub const DEBT_MAX: f64 = 15.0;
const HOLD0: u32 = 60;
const HOLD_MAX: u32 = 480;

/// The controller's state (pure: `frame` takes a frame's cost and gives the hold and the step).
#[derive(Clone, Debug)]
pub struct Ctl {
    pub k: u32,
    pub step: u32,
    pub kmin: u32,
    pub smax: u32,
    pub debt: f64,
    late: Vec<f64>,
    fit_run: u32,
    /// frames left of a trial at a shorter hold (0: none), and the hold it came from
    trial: u32,
    trial_from: u32,
    /// frames since the last step up, and how many must pass before a trial
    since_up: u32,
    holdoff: u32,
}

impl Ctl {
    pub fn new(kmin: u32, smax: u32) -> Ctl {
        let smax = if smax == 0 { 3 } else { smax };
        let kmin = kmin.clamp(1, smax);
        Ctl { k: kmin, step: kmin, kmin, smax, debt: 0.0, late: vec![], fit_run: 0, trial: 0, trial_from: 0, since_up: 0, holdoff: HOLD0 }
    }

    /// A new shortest hold (the target changed): a lower one takes effect at once.
    pub fn set_kmin(&mut self, kmin: u32) {
        self.kmin = kmin.clamp(1, self.smax);
        self.k = self.kmin;
        self.trial = 0;
    }

    /// One frame that took `v` vsyncs (start to start): the hold and the step for the next frame.
    pub fn frame(&mut self, v: f64) -> (u32, u32) {
        // the game moved `step` vsyncs for this frame; real time moved `v`
        self.debt = (self.debt + v - self.step as f64).clamp(0.0, DEBT_MAX);
        self.since_up = self.since_up.saturating_add(1);
        let k = self.k as f64;
        if v > k + LATE {
            self.late.push(v);
        } else {
            self.late.clear();
        }
        if self.trial > 0 {
            if self.late.len() >= 3 {
                // the trial failed: back, and wait twice as long before the next
                self.k = self.trial_from;
                self.trial = 0;
                self.late.clear();
                self.holdoff = (self.holdoff * 2).min(HOLD_MAX);
                self.since_up = 0;
            } else {
                self.trial -= 1;
                if self.trial == 0 {
                    self.holdoff = HOLD0; // held
                }
            }
        } else if self.late.len() >= 3 && self.k < self.smax {
            let cheapest = self.late.iter().cloned().fold(f64::MAX, f64::min);
            self.k = (cheapest.ceil() as u32).clamp(self.k + 1, self.smax);
            self.late.clear();
            self.fit_run = 0;
            self.since_up = 0;
        } else if self.k > self.kmin {
            if v <= (k - 1.0) * FIT {
                self.fit_run += 1;
            } else {
                self.fit_run = 0;
            }
            if self.fit_run >= RUN && self.since_up >= self.holdoff {
                self.trial_from = self.k;
                self.k -= 1;
                self.trial = RUN;
                self.fit_run = 0;
            }
        }
        // a debt of a vsync or more is paid back one vsync a frame, within the largest step
        self.step = if self.debt >= 1.0 && self.k < self.smax { self.k + 1 } else { self.k };
        (self.k, self.step)
    }
}

struct State {
    page: u64,
    ctl: Ctl,
    last: Option<std::time::Instant>,
    target: u32,
    render_scale: u32,
    dynres: bool,
    applied_k: u32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Boot: the state page (the stubs module's first page) and the build's facts.
pub fn init(page: u64, data: &std::path::Path) {
    let facts = std::fs::read_to_string(data.join("frame_step.txt")).unwrap_or_default();
    let get = |k: &str| facts.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_default();
    let smax: u32 = get("step_max ").parse().unwrap_or(0);
    let render_scale: u32 = get("render_scale ").parse().unwrap_or(0);
    let dynres = get("capabilities ").split(',').any(|c| c == "DYNAMIC_RESOLUTION");
    let target = crate::fpslock::target();
    let ctl = Ctl::new(kmin_of(target), smax);
    crate::log!(
        "frame step: state page {page:#x}, target {target} fps (0 = off: the game's own timing), largest step {}, render scale {render_scale}{}",
        ctl.smax,
        if dynres { ", dynamic resolution" } else { "" }
    );
    *STATE.lock().unwrap() = Some(State { page, ctl, last: None, target, render_scale, dynres, applied_k: 0 });
}

fn kmin_of(target: u32) -> u32 {
    match target {
        0 => 1,
        t => (60 / t.min(60)).max(1),
    }
}

fn put32(a: u64, v: u32) {
    unsafe { (*(a as *const std::sync::atomic::AtomicU32)).store(v, std::sync::atomic::Ordering::Release) }
}

/// Each present: the frame's cost, the controller, the state page for the game's next frame.
pub fn present() {
    let mut g = STATE.lock().unwrap();
    let Some(st) = g.as_mut() else { return };
    let now = std::time::Instant::now();
    let target = crate::fpslock::target();
    if target != st.target {
        st.target = target;
        st.ctl.set_kmin(kmin_of(target));
        crate::log!("frame step: target {target} fps");
    }
    let (k, step) = match st.last {
        Some(t) if target > 0 => st.ctl.frame(now.duration_since(t).as_secs_f64() * 60.0),
        _ => (st.ctl.k, st.ctl.step),
    };
    st.last = Some(now);
    put32(st.page, if target > 0 { (target as f32).to_bits() } else { 0 });
    put32(st.page + 0x10, st.render_scale);
    put32(st.page + 0x20, if target > 0 { step } else { 0 });
    put32(st.page + 0x40, st.dynres as u32);
    // the hold: graphics keeps each frame k vsyncs (set when it changes; off, the profile's / the game's own)
    let want = if target > 0 { k } else { 0 };
    if want != st.applied_k {
        st.applied_k = want;
        nxgfx::nvn::window::set_profile_interval(if want > 0 { want as i32 } else { crate::profile::get().present_interval });
        crate::log!("frame step: frames held {want} vsync(s)");
    }
}

/// Whether the build has frame-step patches (the state page is set).
pub fn on() -> bool {
    STATE.lock().unwrap().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady 60: k 1, step 1; a late frame's time is paid back one vsync a frame; three late frames in a row step
    /// up to 2; a run that fits 1 again tries 1 after the hold-off and keeps it; a failed trial doubles the wait.
    #[test]
    fn controller() {
        let mut c = Ctl::new(1, 0);
        for _ in 0..10 {
            assert_eq!(c.frame(1.0), (1, 1));
        }
        // one hitch of 3 vsyncs: 2 vsyncs of debt, paid back over the next frames
        assert_eq!(c.frame(3.0), (1, 2));
        assert_eq!(c.frame(1.0), (1, 2));
        assert_eq!(c.frame(1.0), (1, 1));
        assert!(c.debt < 1.0);
        // three late in a row: hold 2
        c.frame(1.5);
        c.frame(1.6);
        let (k, _) = c.frame(1.7);
        assert_eq!(k, 2);
        // frames that fit 1: a trial after the 60-frame hold-off, and it holds
        let mut n = 0;
        while c.k == 2 && n < 200 {
            c.frame(0.8);
            n += 1;
        }
        assert_eq!(c.k, 1);
        assert!(n >= 60, "no trial before the hold-off ({n} frames)");
        for _ in 0..RUN {
            c.frame(1.0);
        }
        assert_eq!(c.k, 1, "the trial held");
        // a 30 fps target: hold 2 at least; the step stays within step_max 2
        let mut c = Ctl::new(2, 2);
        assert_eq!(c.frame(2.0), (2, 2));
        assert_eq!(c.frame(5.0), (2, 2), "no step past step_max");
        // a failed trial doubles the hold-off
        let mut c = Ctl::new(1, 0);
        for _ in 0..3 {
            c.frame(1.9);
        }
        assert_eq!(c.k, 2);
        for _ in 0..70 {
            c.frame(0.5);
        }
        assert_eq!(c.k, 1);
        for _ in 0..3 {
            c.frame(1.9);
        }
        assert_eq!((c.k, c.holdoff), (2, 120));
    }
}
