//! The frame step: the host side of the frame-step patches (`nxrs-build` places nxnative's FRAME_STEP /
//! MATCH_FRAME_STEP stubs in a patch area after main, `<main>_stubs`, whose first page is this state page), and its
//! present tick (nxnative `hle_nvn.c` fstep_tick; the controller is `nvn_fstep.h`'s, ported):
//!
//! * The state page: +0 the target fps (f32; 0 = off: the stubs keep the game's own timing), +0x10 the render
//!   scale (u32), +0x20 the step handed to the game for its next frame in vsyncs (1/60 s), +0x24 a step the game fixes
//!   itself (the stubs write it; 0 = none), +0x30 vsyncs stepped (the stubs count them), +0x40 dynamic resolution
//!   allowed (u32).
//! * The present tick (`present`, once per present): `shown` is the time since the previous frame's start (the first
//!   acquire after a present, graphics `window::frame_start_take`), `cost` the frame's work: max(CPU = shown less the
//!   pacing wait in that acquire, GPU = the GPU time of the submissions finished since the last tick). The controller
//!   (`Fstep::next`) gives the hold `k` (the vsyncs the frame stays on screen: graphics' present interval) and the step
//!   for the game's next frame. At a steady rate the step is the time on screen; real time the game falls behind is
//!   a debt paid back one vsync a frame, so game time = real time.
//! * `k` follows the cost: three late frames in a row step up to the cheapest of them, when the frames at `k` also
//!   fall short of it on average (`LATE` over the last `RUN` frames, the longest left out: one stall is not a rate,
//!   nxnative issue #1823, Mario Kart World's near-misses); `RUN` frames on time whose cost fits `k − 1` try it, a
//!   trial that fails waits twice as long before the next (1 s → 8 s). A GPU whose clock falls with a longer hold
//!   (`Fgpu`) starts a probe straight back to `kmin`, with a grace while its clock rises.
//! * The target fps sets the shortest hold: 60 → 1, 30 → 2 (`fps_target`, `NXOS_FPS_TARGET`; 0 turns it off). The
//!   profile's `fps_hold` off keeps the hold at the target's vsyncs (late frames never raise it).
//! * `<data>\frame_step.txt` (written by `nxrs-build`): `step_max`, `render_scale`, `capabilities`.

use std::sync::Mutex;

/// The largest step: the games' own interval logic uses 1–3.
pub const MAX: u32 = 3;
/// A frame is late when it or its work took this many vsyncs (1.7 ms) more than k.
pub const LATE: f64 = 0.1;
/// A frame counts at most this many vsyncs past its hold toward the average.
pub const SHORT_MAX: f64 = 2.0;
/// A frame would fit k − 1 when its cost is at most this share of it.
pub const FIT: f64 = 0.95;
/// Frames in a row that fit k − 1 before a trial, and frames a trial must hold.
pub const RUN: usize = 30;
/// Debt kept at most (0.25 s): the rest of a long stall is dropped, not replayed.
pub const DEBT_MAX: f64 = 15.0;
/// Vsyncs since the last step up before a trial: 1 s, doubled after each failed trial up to 8 s.
pub const HOLD0: f64 = 60.0;
pub const HOLD_MAX: f64 = 480.0;
/// A probe's first frames, whose lateness neither fails it nor counts toward a step up.
pub const GRACE: i32 = 30;
/// The cost at the longer hold against the cost that held kmin: at most this much is the same scene.
pub const SAME: f64 = 1.1;
/// The fall in cost from one half of a grace window to the next that shows the clock still rising.
pub const RAMP: f64 = 0.05;
/// Grace frames a probe may take in all.
pub const PROBE_MAX: i32 = 180;

/// The controller (nxnative `nvn_fstep`).
#[derive(Clone, Debug, Default)]
pub struct Fstep {
    /// vsyncs each frame stays on screen; the step handed to the game for the next frame
    pub k: u32,
    pub step: u32,
    /// the last call's kmin (a lower one takes effect at once)
    pub kmin: u32,
    /// real time the game is behind, in vsyncs
    pub debt: f64,
    late_run: u32,
    fit_run: u32,
    /// frames left of a trial at k − 1 (0 = none)
    pub trial: u32,
    /// 1: the trial in progress is a probe; 2: a probe held and its run at kmin has not ended yet
    pub probe: u32,
    /// frames left of the probe's grace
    pub grace: i32,
    /// a probe's grace is renewed while its frames keep getting cheaper
    pub ramp: bool,
    pa: f64,
    pb: f64,
    plate: i32,
    ptotal: i32,
    /// vsyncs a trial waits for after a step up (the hold-off); vsyncs since the last step up
    pub holdoff: f64,
    since: f64,
    tpend: bool,
    tsince: f64,
    /// the ticks a 1/60 s vsync is (0 = 1)
    pub per_vsync: u32,
    run: f64,
    crun: f64,
    cref: f64,
    ccur: f64,
    ck: u32,
    cn: u32,
    ckmin: u32,
    cref_at: [f64; MAX as usize + 1],
    over: [f64; RUN],
    over_sum: f64,
    over_i: usize,
    over_k: u32,
    last_cost: [f64; 3],
    /// the largest step the game takes (its stub's STEP_MAX; 0 = MAX)
    pub smax: u32,
    /// the game does not take the step (it steps by its target): three late frames raise the hold whatever the average
    pub no_payback: bool,
    pub late: u64,
    pub paid: u64,
    pub ups: u64,
    pub downs: u64,
    pub failed: u64,
    pub probes: u64,
    pub dropped: f64,
}

impl Fstep {
    /// Whether the cost at the longer hold is the cost that held kmin.
    fn same(&self) -> bool {
        self.cref > 0.0 && self.cn >= RUN as u32 && self.ccur <= SAME * self.cref
    }

    /// Once per frame. `shown`: real time since the previous frame's start, in vsyncs; `cost`: the frame's work,
    /// max(CPU, GPU), in vsyncs; `kmin`: 1 at a 60 fps target, 2 at 30; `kmax`: the longest hold; `fixed`: a step the
    /// game has set itself (0 = none); `slow`: the GPU (or CPU) runs below its speed for the longer hold, so a cost
    /// above k − 1 is no evidence against kmin (probes). Returns the step for the next frame; `self.k` is how long
    /// that frame stays on screen.
    pub fn next(&mut self, shown: f64, cost: f64, kmin: u32, kmax: u32, fixed: u32, slow: bool) -> u32 {
        let pv = if self.per_vsync > 0 { self.per_vsync as f64 } else { 1.0 };
        let (hold0, hold_max) = (HOLD0 * pv, HOLD_MAX * pv);
        let smax = if self.smax > 0 && self.smax < MAX { self.smax } else { MAX };
        let mut kmin = kmin.max(1);
        let mut kmax = kmax.min(smax);
        if kmin > smax {
            kmin = smax;
        }
        if kmax < kmin {
            kmax = kmin;
        }
        if self.holdoff == 0.0 {
            self.holdoff = hold0;
        }
        if fixed > 0 {
            // the game's own step: speed is right by construction, nothing to pay back
            self.k = fixed.min(smax);
            self.step = self.k;
            self.debt = 0.0;
            self.late_run = 0;
            self.fit_run = 0;
            self.trial = 0;
            self.probe = 0;
            self.grace = 0;
            self.tpend = false;
            self.run = 0.0;
            self.kmin = MAX + 1; // when it ends, k starts again from kmin
            return self.step;
        }
        if self.k == 0 {
            // first frame: nothing measured yet
            self.k = kmin;
            self.step = kmin;
            self.kmin = kmin;
            return self.step;
        }
        let held = self.k;
        if held != self.over_k {
            self.over_k = held;
            self.over_sum = 0.0;
            self.over_i = 0;
            self.over = [0.0; RUN];
        }
        let over = (shown - held as f64).clamp(-1.0, SHORT_MAX);
        self.over_sum += over - self.over[self.over_i];
        self.over[self.over_i] = over;
        self.over_i = (self.over_i + 1) % RUN;
        self.last_cost = [self.last_cost[1], self.last_cost[2], cost];
        self.debt += shown - self.step as f64;
        if self.debt > DEBT_MAX {
            self.dropped += self.debt - DEBT_MAX;
            self.debt = DEBT_MAX;
        }
        if self.debt < -1.0 {
            self.debt = -1.0;
        }
        self.since += shown;
        self.tsince += shown;
        if self.tpend && self.tsince >= hold_max {
            self.tpend = false;
            self.holdoff = hold0;
        }
        let cap_probe = kmin < self.kmin && self.kmin <= MAX && self.k > kmin;
        if cap_probe {
            // the cap rose: a probe at once, whose grace covers the clocks still set for the longer hold
            self.k = kmin;
            self.probes += 1;
            self.start_probe();
        } else if kmin < self.kmin || self.k < kmin {
            self.k = kmin;
        }
        if self.k > kmax {
            self.k = kmax;
        }
        self.kmin = kmin;
        let late = shown > held as f64 + LATE || cost > held as f64 + LATE;
        let grace = self.grace > 0;
        if grace && !cap_probe {
            if self.grace > GRACE / 2 {
                self.pa += cost;
            } else {
                self.pb += cost;
            }
            self.plate += late as i32;
            self.ptotal += 1;
            self.grace -= 1;
            if self.grace == 0 && self.ramp && self.plate > 0 && self.ptotal + GRACE <= PROBE_MAX && self.pb <= (1.0 - RAMP) * self.pa {
                // still getting cheaper: another window
                self.grace = GRACE;
                self.pa = 0.0;
                self.pb = 0.0;
                self.plate = 0;
            }
        }
        if late {
            self.late += 1;
            self.late_run = if grace { 0 } else { self.late_run + 1 };
            self.fit_run = 0;
        } else {
            self.late_run = 0;
        }
        // the cost of the frames on time: at kmin, and at the longer hold
        if kmin != self.ckmin {
            if (1..=MAX).contains(&self.ckmin) {
                if self.run >= hold_max && self.crun > 0.0 {
                    self.cref = self.crun;
                }
                self.cref_at[self.ckmin as usize] = self.cref;
            }
            self.ckmin = kmin;
            self.run = 0.0;
            self.crun = 0.0;
            self.cref = if kmin <= MAX { self.cref_at[kmin as usize] } else { 0.0 };
        }
        if held <= kmin {
            self.run += shown;
            self.cn = 0;
            if !late && self.probe != 1 {
                self.crun = if self.crun > 0.0 { self.crun + (cost - self.crun) / 32.0 } else { cost };
            }
        } else {
            self.run = 0.0;
            if held != self.ck {
                self.ck = held;
                self.cn = 0;
            }
            if !late {
                self.ccur = if self.cn > 0 { self.ccur + (cost - self.ccur) / 8.0 } else { cost };
                self.cn += 1;
            }
        }
        // a rate below k over the last RUN frames, the longest left out
        let omax = self.over.iter().cloned().fold(0.0f64, f64::max);
        let short_of_k = self.no_payback || self.over_sum - omax > LATE * RUN as f64;
        if self.late_run >= 3 && self.k < kmax && short_of_k {
            // step up to the cost of the cheapest of the three late frames, at least one vsync
            let c3 = self.last_cost.iter().cloned().fold(f64::MAX, f64::min);
            let want = ((c3 - 0.05).ceil().max(0.0) as u32).max(self.k + 1).min(kmax);
            if self.trial > 0 || self.tpend || (self.probe == 2 && self.since < hold_max) {
                self.failed += 1;
                self.trial = 0;
                self.holdoff = (self.holdoff * 2.0).min(hold_max);
            } else if self.probe == 2 {
                self.holdoff = hold0; // the probe's run at kmin lasted
            }
            if held == kmin && self.run >= hold_max {
                self.cref = self.crun;
            }
            self.k = want;
            self.ups += 1;
            self.late_run = 0;
            self.fit_run = 0;
            self.since = 0.0;
            self.probe = 0;
            self.tpend = false;
        } else if self.trial > 0 && !grace {
            self.trial -= 1;
            if self.trial == 0 {
                // the trial held: its run is judged by how long it lasts
                if self.probe > 0 {
                    self.probe = 2;
                    self.since = 0.0;
                } else {
                    self.tpend = true;
                    self.tsince = 0.0;
                }
            }
        }
        // a frame also fits k − 1 while the running mean of this hold's frames on time does
        let fitc = if self.ck == self.k && self.cn > 0 && self.ccur < cost { self.ccur } else { cost };
        if !late {
            self.fit_run = if self.k > kmin && fitc <= FIT * (self.k - 1) as f64 { self.fit_run + 1 } else { 0 };
        }
        if self.fit_run >= RUN as u32 && self.since >= self.holdoff && self.trial == 0 {
            self.k -= 1;
            self.downs += 1;
            self.trial = RUN as u32;
            self.fit_run = 0;
        } else if (slow || self.same()) && self.k > kmin && self.since >= self.holdoff && self.trial == 0 {
            // probe: straight to kmin, so the clocks see the whole deadline
            self.k = kmin;
            self.downs += 1;
            self.probes += 1;
            self.start_probe();
        }
        self.step = self.k + (self.debt >= 1.0 && self.k < smax) as u32;
        self.paid += (self.step > self.k) as u64;
        self.step
    }

    fn start_probe(&mut self) {
        self.trial = RUN as u32;
        self.probe = 1;
        self.grace = GRACE;
        self.fit_run = 0;
        self.late_run = 0;
        self.pa = 0.0;
        self.pb = 0.0;
        self.plate = 0;
        self.ptotal = 0;
    }
}

/// GPU time at the longer hold against its time at kmin before the GPU counts as slowed.
pub const GPU_SLOW: f64 = 1.3;
/// Frames at the longer hold before the comparison counts.
pub const GPU_N: u32 = 30;
/// A probe's frames at kmin after the first half of its grace before it gives a ratio.
pub const GPU_PN: u32 = 3;
/// Failed probes whose ratio a hold needs before the ratio decides.
pub const GPU_FAILS: u32 = 2;
/// The smallest ratio a probe gives.
pub const GPU_RMIN: f64 = 0.75;

/// Whether the GPU's clock fell with the longer hold (nxnative `nvn_fgpu`): its time at the hold against its time at
/// kmin, or, once failed probes measured it, the ratio between the two.
#[derive(Clone, Debug, Default)]
pub struct Fgpu {
    pub reference: f64,
    pub cur: f64,
    avg: f64,
    n: u32,
    k: u32,
    kmin: u32,
    ref_at: [f64; MAX as usize + 1],
    pre: f64,
    post: f64,
    pre_k: u32,
    pn: u32,
    was: u32,
    fail: bool,
    pub ratio_at: [[f64; MAX as usize + 1]; MAX as usize + 1],
    ratio_n: [[u32; MAX as usize + 1]; MAX as usize + 1],
}

impl Fgpu {
    /// The hold's ratio when it decides (0: the reference does).
    pub fn ratio(&self) -> f64 {
        let ok = (1..=MAX).contains(&self.kmin) && (1..=MAX).contains(&self.k);
        if ok && self.ratio_n[self.kmin as usize][self.k as usize] >= GPU_FAILS {
            self.ratio_at[self.kmin as usize][self.k as usize]
        } else {
            0.0
        }
    }
    fn slow(&self) -> bool {
        if self.n < GPU_N || self.cur <= FIT * self.kmin as f64 {
            return false;
        }
        let r = self.ratio();
        if r > 0.0 {
            self.cur / r <= self.kmin as f64
        } else {
            self.reference > 0.0 && self.cur >= GPU_SLOW * self.reference
        }
    }
    fn learn(&mut self, kmin: u32, k: u32, t: f64) {
        let r = t / self.post;
        if r < GPU_RMIN {
            return;
        }
        let at = &mut self.ratio_at[kmin as usize][k as usize];
        if r > *at {
            *at = r;
        }
        self.ratio_n[kmin as usize][k as usize] += 1;
    }

    /// Once per frame, with the frame's GPU time (vsyncs; 0 = none finished) and the controller's state the frame ran
    /// under. True: the GPU counts as slowed (a probe may start).
    pub fn slowed(&mut self, gpu: f64, s: &Fstep, kmin: u32) -> bool {
        if !(1..=MAX).contains(&kmin) {
            return false;
        }
        let k = if s.k > 0 { s.k } else { kmin };
        let probe = s.probe;
        if kmin != self.kmin {
            self.fail = false;
        }
        if probe == 1 && self.was != 1 {
            self.pre_k = if self.kmin == kmin && self.k > kmin && self.k <= MAX && self.n >= GPU_N { self.k } else { 0 };
            self.pre = self.cur.min(self.avg);
            self.post = 0.0;
            self.pn = 0;
            self.fail = false;
        } else if probe != 1 && self.was == 1 {
            self.fail = probe == 0 && k > kmin && self.pn >= GPU_PN;
        }
        self.was = probe;
        if probe == 1 && k <= kmin && gpu > 0.0 && s.grace <= GRACE / 2 {
            self.post = if self.pn > 0 { self.post + (gpu - self.post) / 4.0 } else { gpu };
            self.pn += 1;
        }
        self.reference = self.ref_at[kmin as usize];
        self.kmin = kmin;
        if gpu <= 0.0 {
            return self.slow();
        }
        if k <= kmin {
            if probe != 1 {
                self.reference = if self.reference > 0.0 { self.reference + (gpu - self.reference) / 32.0 } else { gpu };
                self.ref_at[kmin as usize] = self.reference;
            }
            self.n = 0;
            self.k = k;
            self.fail = false;
            return false;
        }
        if k != self.k {
            self.k = k;
            self.n = 0;
        }
        self.n += 1;
        self.cur = if self.n > 1 { self.cur + (gpu - self.cur) / 8.0 } else { gpu };
        self.avg = if self.n > 1 { self.avg + (gpu - self.avg) / self.n.min(32) as f64 } else { gpu };
        if self.fail && self.n >= GPU_N && k <= MAX {
            let t = self.cur.min(self.avg);
            if k == self.pre_k {
                self.learn(kmin, k, self.pre.min(t));
            } else {
                self.learn(kmin, k, t);
                if self.pre_k > 0 {
                    let (pk, pre) = (self.pre_k, self.pre);
                    self.learn(kmin, pk, pre);
                }
            }
            self.fail = false;
        }
        self.slow()
    }
}

struct State {
    page: u64,
    fs: Fstep,
    gpu: Fgpu,
    /// the previous frame's start (graphics' clock, ns; 0 = none yet)
    prev: u64,
    target: u32,
    render_scale: u32,
    dynres: bool,
    /// late frames may raise the hold (profile `fps_hold`)
    hold: bool,
    applied_k: u32,
    presents: u64,
    fixed_was: u32,
    held: [u64; MAX as usize + 1],
    fixed_sum: u64,
    t0: u64,
    v0: u64,
    nlog: u32,
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
    let hold = crate::options::game_flag("fps_hold", true);
    if !hold {
        crate::log!("frame step: frames are not held longer than the target's vsyncs (game profile fps_hold)");
    }
    let fs = Fstep { smax, ..Default::default() };
    crate::log!(
        "frame step: state page {page:#x}, target {target} fps (0 = off: the game's own timing), largest step {}, render scale {render_scale}{}",
        if smax > 0 && smax < MAX { smax } else { MAX },
        if dynres { ", dynamic resolution" } else { "" }
    );
    *STATE.lock().unwrap() = Some(State {
        page,
        fs,
        gpu: Fgpu::default(),
        prev: 0,
        target,
        render_scale,
        dynres,
        hold,
        applied_k: 0,
        presents: 0,
        fixed_was: 0,
        held: [0; MAX as usize + 1],
        fixed_sum: 0,
        t0: 0,
        v0: 0,
        nlog: 0,
    });
}

/// The shortest hold of a target (60 → 1, 30 → 2, 20 → 3).
pub fn kmin_of(target: u32) -> u32 {
    match target {
        0 => 1,
        t => (60 / t.min(60)).max(1),
    }
}

/// The longest hold: the target's own with `fps_hold` off, 2 at a 30 fps target, else `MAX`.
pub fn kmax_of(kmin: u32, hold: bool) -> u32 {
    if !hold {
        kmin
    } else if kmin >= 2 {
        2
    } else {
        MAX
    }
}

fn put32(a: u64, v: u32) {
    unsafe { (*(a as *const std::sync::atomic::AtomicU32)).store(v, std::sync::atomic::Ordering::Release) }
}
fn get32(a: u64) -> u32 {
    unsafe { (*(a as *const std::sync::atomic::AtomicU32)).load(std::sync::atomic::Ordering::Acquire) }
}

/// The present tick: the frame's time and cost, the controller, the state page for the game's next frame.
pub fn present() {
    let mut g = STATE.lock().unwrap();
    let Some(st) = g.as_mut() else { return };
    st.presents += 1;
    let target = crate::fpslock::target();
    if target != st.target {
        st.target = target;
        crate::log!("frame step: target {target} fps");
    }
    let (start, wait) = nxgfx::nvn::window::frame_start_take();
    let gpu_ns = nxgfx::vk::perf::frame_gpu_take();
    let now = if start > 0 { start } else { nxgfx::nvn::device::now_ns() };
    const V: f64 = 1e9 / 60.0;
    let shown = if st.prev > 0 && now > st.prev { (now - st.prev) as f64 / V } else { 0.0 };
    let first = st.prev == 0;
    st.prev = now;
    let gpu = gpu_ns as f64 / V;
    let cost = (shown - wait as f64 / V).max(gpu);
    put32(st.page, if target > 0 { (target as f32).to_bits() } else { 0 });
    put32(st.page + 0x10, st.render_scale);
    put32(st.page + 0x40, st.dynres as u32);
    if target == 0 {
        put32(st.page + 0x20, 0);
        apply_hold(st, 0);
        return;
    }
    let kmin = kmin_of(target);
    let kmax = kmax_of(kmin, st.hold);
    let fixed = get32(st.page + 0x24);
    let k0 = st.fs.k;
    let slow = !first && st.gpu.slowed(gpu, &st.fs, kmin);
    let step = if first { st.fs.next(0.0, 0.0, kmin, kmax, fixed, false) } else { st.fs.next(shown, cost, kmin, kmax, fixed, slow) };
    put32(st.page + 0x20, step);
    st.held[st.fs.k.min(MAX) as usize] += 1;
    if fixed > 0 {
        st.fixed_sum += step as u64;
    }
    if fixed != st.fixed_was && st.nlog < 40 {
        st.nlog += 1;
        if fixed > 0 {
            crate::log!("frame step: the game fixes its own step at {fixed} vsyncs at present {}", st.presents);
        } else {
            crate::log!("frame step: the game hands the step back to the host at present {}", st.presents);
        }
    }
    st.fixed_was = fixed;
    if k0 > 0 && st.fs.k != k0 && st.nlog < 40 {
        st.nlog += 1;
        crate::log!("frame step: hold {k0} -> {} vsyncs at present {} (cost {cost:.2}, GPU {gpu:.2}{})", st.fs.k, st.presents, if slow { ", GPU slowed: probe" } else { "" });
    }
    if st.presents % 1800 == 0 {
        // game time against real time (1.000 = normal speed)
        let vs = unsafe { core::ptr::read_volatile((st.page + 0x30) as *const u64) } + st.fixed_sum;
        let t = nxgfx::nvn::device::now_ns();
        if st.t0 > 0 {
            let (gt, rt) = ((vs - st.v0) as f64 / 60.0, (t - st.t0) as f64 / 1e9);
            crate::log!(
                "frame step: game time {gt:.2} s in {rt:.2} s ({:.3} of real time); held 1 / 2 / 3 vsyncs {} / {} / {} frames, {} late, {} vsyncs paid back, {:.0} dropped, {} up / {} down ({} probes, {} failed trials)",
                gt / rt.max(1e-9),
                st.held[1],
                st.held[2],
                st.held[3],
                st.fs.late,
                st.fs.paid,
                st.fs.dropped,
                st.fs.ups,
                st.fs.downs,
                st.fs.probes,
                st.fs.failed
            );
        }
        st.t0 = t;
        st.v0 = vs;
        st.held = [0; MAX as usize + 1];
    }
    let k = st.fs.k;
    apply_hold(st, k);
}

/// The hold: graphics keeps each frame k vsyncs (set when it changes; 0: the profile's / the game's own).
fn apply_hold(st: &mut State, want: u32) {
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
    //! nxnative's `tests/t_fstep.c`: frames are simulated in vsyncs. A frame whose work w fits its hold k stays on
    //! screen k vsyncs; a longer one takes w. Game time is the sum of the steps, real time the sum of the frame times.
    use super::*;

    #[derive(Default)]
    struct Sim {
        s: Fstep,
        real: f64,
        game: f64,
        next: u32,
        slow: bool,
    }
    impl Sim {
        fn run(&mut self, n: usize, w: f64, kmin: u32, kmax: u32, fixed: u32) {
            for _ in 0..n {
                let k = if self.s.k > 0 { self.s.k } else { kmin };
                let step = if self.next > 0 { self.next } else { kmin };
                let shown = w.max(k as f64);
                self.real += shown;
                self.game += step as f64;
                self.next = self.s.next(shown, w, kmin, kmax, fixed, self.slow);
            }
        }
        fn balanced(&self) -> bool {
            let d = self.real - self.game - self.s.debt;
            d > -1.01 && d < 1.01
        }
    }

    #[test]
    fn steady_60_and_steady_22_ms() {
        let mut m = Sim::default();
        m.run(600, 0.5, 1, 3, 0);
        assert!(m.s.k == 1 && m.s.ups == 0 && m.s.paid == 0 && m.game == m.real, "steady 60: hold 1, step 1, game time = real time");
        let mut m = Sim::default();
        m.run(120, 0.5, 1, 3, 0);
        m.run(600, 1.32, 1, 3, 0);
        assert!(m.s.k == 2 && m.s.ups == 1 && m.s.downs == 0, "22 ms frames: 30 fps, no flapping");
        assert!(m.balanced() && m.s.debt < 1.0);
        let mut m = Sim::default();
        m.run(120, 0.5, 1, 3, 0);
        m.run(300, 1.12, 1, 3, 0);
        assert!(m.s.k == 2 && m.s.ups == 1, "18.7 ms frames (53 fps): 30 fps");
    }

    /// Mario Kart World on an M4 Max (nxnative issue #1823): near-misses and lone stalls keep hold 1.
    #[test]
    fn near_misses_and_lone_stalls_keep_hold_1() {
        let mut m = Sim::default();
        m.run(120, 0.5, 1, 3, 0);
        for i in 0..3000 {
            m.run(1, if i % 20 < 3 { 1.25 } else { 0.9 }, 1, 3, 0);
        }
        assert!(m.s.k == 1 && m.s.ups == 0 && m.s.late >= 400, "3 of every 20 frames late by 4 ms: hold 1");
        assert!(m.balanced() && m.s.paid > 0);
        let mut h = Sim::default();
        h.run(120, 0.5, 1, 3, 0);
        h.run(600, 0.9, 1, 3, 0);
        h.run(3, 1.6, 1, 3, 0);
        h.run(60, 0.9, 1, 3, 0);
        assert!(h.s.k == 1 && h.s.ups == 0, "a hitch of three 27 ms frames among frames on time: hold 1");
        let stalls = [(1913, 67.7), (10267, 57.7), (10286, 43.1), (10454, 54.9), (11396, 65.4)];
        let mut s = Fstep { smax: 2, ..Default::default() };
        let (mut next, mut at2, mut real, mut game) = (1u32, 0, 0.0, 0.0);
        for p in 0..12000 {
            let k = if s.k > 0 { s.k } else { 1 };
            let mut shown = if k == 1 && (5..8).contains(&(p % 20)) { 1.3 } else { k as f64 };
            for &(at, ms) in &stalls {
                if p == at {
                    shown = ms / (1000.0 / 60.0);
                }
            }
            real += shown;
            game += next as f64;
            next = s.next(shown, 0.45, 1, 3, 0, false);
            at2 += (s.k > 1) as u32;
        }
        assert!(s.ups == 0 && at2 == 0, "lone stalls of 43-68 ms among near-misses: hold 1 (ups {}, {at2} frames at hold 2)", s.ups);
        let d = real - game - s.debt;
        assert!(d > -1.01 && d < 1.01);
    }

    #[test]
    fn step_goes_to_the_cheapest_late_frame_within_the_stub_cap() {
        let mut m = Sim::default();
        m.run(120, 0.5, 1, 3, 0);
        m.run(11, 1.3, 1, 3, 0);
        assert_eq!(m.s.k, 1, "eleven late frames are not yet short of hold 1");
        m.run(1, 3.0, 1, 3, 0);
        assert!(m.s.k == 2 && m.s.ups == 1, "a 3-vsync frame then: hold 2, not 3");
        let mut m = Sim { s: Fstep { smax: 2, ..Default::default() }, ..Default::default() };
        m.run(120, 0.5, 1, 3, 0);
        let mut over = 0;
        for i in 0..600 {
            m.run(1, if i < 300 { 2.6 } else { 1.3 }, 1, 3, 0);
            over += (m.next > 2) as u32;
        }
        assert!(m.s.k == 2 && over == 0, "step cap 2 (MKW): hold 2 at most, steps at most 2");
        let mut f = Sim { s: Fstep { smax: 2, ..Default::default() }, ..Default::default() };
        f.run(10, 0.5, 1, 3, 3);
        assert!(f.s.k == 2 && f.next == 2, "a step the game fixes at 3 is held at the cap");
    }

    #[test]
    fn hitches_are_paid_back_and_stalls_dropped() {
        let mut m = Sim::default();
        m.run(60, 0.5, 1, 3, 0);
        m.real += 6.0;
        m.game += m.next as f64;
        m.next = m.s.next(6.0, 6.0, 1, 3, 0, false);
        assert!(m.next == 2 && m.s.k == 1, "100 ms hitch: one vsync paid back, hold stays 1");
        let mut two = 0;
        for _ in 0..20 {
            let st = m.next;
            m.run(1, 0.5, 1, 3, 0);
            two += (st == 2) as u32;
        }
        assert!(two == 5 && m.game == m.real && m.s.k == 1);
        let mut m = Sim::default();
        m.run(60, 0.5, 1, 3, 0);
        m.real += 30.0;
        m.game += m.next as f64;
        m.next = m.s.next(30.0, 30.0, 1, 3, 0, false);
        m.run(60, 0.5, 1, 3, 0);
        assert!(m.s.dropped == 14.0 && m.s.paid == 15 && m.real - m.game == 14.0 && m.s.k == 1, "0.5 s stall: 15 paid back, 14 dropped");
    }

    #[test]
    fn trials_return_to_60_and_spread_out_when_they_fail() {
        let mut m = Sim::default();
        m.run(60, 0.5, 1, 3, 0);
        m.run(300, 1.3, 1, 3, 0);
        assert_eq!(m.s.k, 2);
        m.run(29, 0.6, 1, 3, 0);
        assert_eq!(m.s.k, 2, "29 frames that fit 1 are not enough");
        m.run(1, 0.6, 1, 3, 0);
        assert!(m.s.k == 1 && m.s.trial == RUN as u32, "the 30th starts a trial");
        m.run(RUN + 10, 0.6, 1, 3, 0);
        assert!(m.s.k == 1 && m.s.trial == 0 && m.s.holdoff == HOLD0 && m.s.failed == 0, "the trial holds");
        let mut m = Sim::default();
        m.run(60, 0.5, 1, 3, 0);
        m.run(30, 1.5, 1, 3, 0);
        assert_eq!(m.s.k, 2);
        let (mut last, mut gaps, mut downs) = (m.real, vec![], m.s.downs);
        for _ in 0..4000 {
            if gaps.len() >= 4 {
                break;
            }
            let w = if m.s.k == 1 { 1.5 } else { 0.6 };
            m.run(1, w, 1, 3, 0);
            if m.s.downs != downs {
                downs = m.s.downs;
                gaps.push(m.real - last);
                last = m.real;
            }
        }
        assert!(gaps.len() == 4 && gaps[0] >= 60.0 && gaps[1] >= 120.0 && gaps[2] >= 240.0 && gaps[3] >= 480.0 && gaps[3] < 560.0 && m.s.failed >= 3, "{gaps:?}");
    }

    #[test]
    fn caps_fixed_steps_and_fps_hold() {
        let mut m = Sim::default();
        m.run(300, 0.5, 2, 2, 0);
        assert!(m.s.k == 2 && m.next == 2 && m.game == m.real, "30 fps cap: hold 2, step 2");
        m.real += 6.0;
        m.game += m.next as f64;
        m.next = m.s.next(6.0, 6.0, 2, 2, 0, false);
        let mut three = 0;
        for _ in 0..10 {
            let st = m.next;
            m.run(1, 0.5, 2, 2, 0);
            three += (st == 3) as u32;
        }
        assert!(three == 4 && m.s.k == 2 && m.game == m.real, "a hitch at the 30 cap is paid back with steps of 3");
        let mut m = Sim::default();
        m.run(60, 0.5, 1, 3, 0);
        m.run(120, 0.5, 1, 3, 2);
        assert!(m.s.k == 2 && m.next == 2 && m.s.debt == 0.0, "the game fixes its step at 2");
        m.run(2, 0.5, 1, 3, 0);
        assert!(m.s.k == 1 && m.next == 1 && m.s.late == 0, "the override ends: hold 1, step 1");
        // fps_hold off: the longest hold is the target's; late frames are still paid back in the step
        let mut m = Sim::default();
        m.run(60, 0.5, 1, kmax_of(1, false), 0);
        m.run(300, 1.9, 1, kmax_of(1, false), 0);
        assert!(m.s.k == 1 && m.s.paid > 0, "fps_hold off: hold 1 whatever the frames take");
        assert_eq!((kmax_of(1, true), kmax_of(2, true), kmin_of(30), kmin_of(60), kmin_of(0)), (3, 2, 2, 1, 1));
    }

    /// The GPU clock follows the hold (Breath of the Wild on an iPhone; a desktop GPU's boost clock does the same):
    /// one probe back to hold 1 that holds, and a heavier scene at the same clock starts none.
    #[test]
    fn a_gpu_slowed_by_the_longer_hold_is_probed_back() {
        let (mut m, mut g, mut at1, mut stepped) = (Sim::default(), Fgpu::default(), 0, false);
        for i in 0..2000 {
            let w = if i < 600 {
                0.86
            } else if i < 603 {
                1.25
            } else if m.s.k >= 2 {
                1.5
            } else {
                at1 += 1;
                if at1 <= 20 {
                    1.5
                } else {
                    0.86
                }
            };
            stepped |= i >= 603 && m.s.k == 2;
            m.slow = g.slowed(w, &m.s, 1);
            m.run(1, w, 1, 3, 0);
        }
        assert!(stepped && m.s.k == 1 && m.s.probes == 1 && m.s.failed == 0, "k {} probes {} failed {}", m.s.k, m.s.probes, m.s.failed);
        assert!(m.balanced() && m.s.dropped == 0.0);
        let (mut m, mut g) = (Sim::default(), Fgpu::default());
        for i in 0..2000 {
            let w = if i < 600 { 0.86 } else { 1.12 };
            m.slow = g.slowed(w, &m.s, 1);
            m.run(1, w, 1, 3, 0);
        }
        assert!(m.s.k == 2 && m.s.probes == 0, "heavier scene, GPU 0.86 -> 1.12 at hold 2: not slowed");
        // a slowed report while hold 1 does not fit: the first probe lasts its grace + 3 late frames
        let mut m = Sim::default();
        m.run(60, 0.5, 1, 3, 0);
        m.run(300, 1.3, 1, 3, 0);
        m.slow = true;
        let (mut at1, mut first_len) = (0, -1);
        for _ in 0..600 {
            m.run(1, if m.s.k == 1 { 1.5 } else { 1.2 }, 1, 3, 0);
            if m.s.k == 1 {
                at1 += 1;
            } else if at1 > 0 && first_len < 0 {
                first_len = at1;
            }
        }
        assert_eq!(first_len, GRACE + 3);
    }
}
