//! The gesture recognizer behind nn::hid::GetGestureStates (nxnative hle_touch.c, yuzu hid_core's rules, one
//! finger: the host's touch screen is the mouse or the control pipe). A new touch is Touch (IsNewTouch, unless the
//! entry before is Cancel); held still 0.5 s it becomes Press; moved, Pan (delta from the last entry, velocity over
//! the time since it). On release Touch becomes Tap (IsDoubleTap within 0.35 s of the last tap), Pan becomes Swipe
//! above 400 px/s (direction by the larger delta) or Complete, the others Complete; Idle follows Complete or Cancel.
//! An entry is written only when the finger moved, after a forced step or when a Touch is due to become Press; the
//! finger stays put until it moved 8 px from where it went down (a click wobbles). GestureState is 0x60 bytes.

use std::sync::Mutex;

const IDLE: i32 = 0;
const COMPLETE: i32 = 1;
const CANCEL: i32 = 2;
const TOUCH: i32 = 3;
const PRESS: i32 = 4;
const TAP: i32 = 5;
const PAN: i32 = 6;
const SWIPE: i32 = 7;
const SLOP: i32 = 8;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Entry {
    pub samp: i64,
    pub ctx: i64,
    pub ty: i32,
    pub dir: i32,
    pub x: i32,
    pub y: i32,
    pub dx: i32,
    pub dy: i32,
    pub vx: f32,
    pub vy: f32,
    pub attr: u32,
    pub npt: i32,
}

impl Entry {
    pub fn bytes(&self) -> [u8; 0x60] {
        let mut b = [0u8; 0x60];
        b[0..8].copy_from_slice(&self.samp.to_le_bytes());
        b[8..16].copy_from_slice(&self.ctx.to_le_bytes());
        for (o, v) in [(0x10, self.ty), (0x14, self.dir), (0x18, self.x), (0x1c, self.y), (0x20, self.dx), (0x24, self.dy)] {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[0x28..0x2c].copy_from_slice(&self.vx.to_le_bytes());
        b[0x2c..0x30].copy_from_slice(&self.vy.to_le_bytes());
        b[0x30..0x34].copy_from_slice(&self.attr.to_le_bytes());
        b[0x3c..0x40].copy_from_slice(&self.npt.to_le_bytes());
        if self.npt > 0 {
            b[0x40..0x44].copy_from_slice(&self.x.to_le_bytes());
            b[0x44..0x48].copy_from_slice(&self.y.to_le_bytes());
        }
        b
    }
}

#[derive(Default)]
pub struct Recognizer {
    hist: Vec<Entry>,
    last: Option<(i32, i32)>,
    down_at: (i32, i32),
    ctx: i64,
    force: bool,
    tap_ok: bool,
    slop: bool,
    upd_ns: u64,
    tap_ns: u64,
    pan_dt: f32,
}

impl Recognizer {
    fn newest(&self) -> Option<&Entry> {
        self.hist.last()
    }

    /// One sample with the finger where it is now (None: up), at `now` ns.
    pub fn sample(&mut self, finger: Option<(u32, u32)>, now: u64) {
        let mut g = finger.map(|(x, y)| (x.min(1279) as i32, y.min(719) as i32));
        let le = self.newest().copied();
        let ltype = le.map_or(IDLE, |e| e.ty);
        let dt = if self.upd_ns != 0 { now.saturating_sub(self.upd_ns) as f32 / 1e9 } else { 0.0 };
        if let (Some(p), Some(_), false) = (g, self.last, self.slop) {
            if (p.0 - self.down_at.0).abs() > SLOP || (p.1 - self.down_at.1).abs() > SLOP {
                self.slop = true;
            } else {
                g = self.last;
            }
        }
        let mut upd = self.force || g.is_some() != self.last.is_some() || g != self.last;
        if !upd && ltype == TOUCH && dt > 0.5 {
            upd = self.tap_ok;
        }
        if !upd {
            return;
        }
        self.force = false;
        let mut s = Entry { samp: le.map_or(1, |e| e.samp + 1), ..Default::default() };
        let mut ty = IDLE;
        match (g, self.last) {
            (Some(p), None) => {
                self.ctx += 1;
                ty = TOUCH;
                self.down_at = p;
                self.slop = false;
                if ltype != CANCEL {
                    s.attr |= 1 << 4;
                    self.tap_ok = true;
                }
            }
            (Some(p), Some(l)) if p != l => {
                ty = PAN;
                let (lx, ly) = le.map_or(p, |e| (e.x, e.y));
                s.dx = p.0 - lx;
                s.dy = p.1 - ly;
                if dt > 0.0 {
                    s.vx = s.dx as f32 / dt;
                    s.vy = s.dy as f32 / dt;
                }
                self.pan_dt = dt;
            }
            (Some(_), Some(_)) => {
                if ltype == TOUCH {
                    ty = PRESS;
                }
            }
            (None, Some(l)) => match ltype {
                TOUCH => {
                    self.force = true;
                    if !self.tap_ok {
                        ty = CANCEL;
                    } else {
                        ty = TAP;
                        g = Some(l);
                        if self.tap_ns != 0 && now - self.tap_ns < 350_000_000 {
                            s.attr |= 1 << 8;
                        }
                        self.tap_ns = now;
                    }
                }
                PRESS | TAP | SWIPE => {
                    ty = COMPLETE;
                    self.force = true;
                }
                PAN => {
                    let e = le.unwrap();
                    let t = self.pan_dt + dt;
                    let (vx, vy) = if t > 0.0 { (e.dx as f32 / t, e.dy as f32 / t) } else { (0.0, 0.0) };
                    self.force = true;
                    if vx.hypot(vy) <= 400.0 {
                        ty = COMPLETE;
                    } else {
                        ty = SWIPE;
                        g = Some(l);
                        s.vx = vx;
                        s.vy = vy;
                        s.dx = e.dx;
                        s.dy = e.dy;
                        s.dir = if s.dx.abs() > s.dy.abs() { if s.dx > 0 { 3 } else { 1 } } else if s.dy > 0 { 4 } else { 2 };
                    }
                }
                _ => {}
            },
            (None, None) => {
                if ltype == COMPLETE || ltype == CANCEL {
                    self.ctx += 1;
                }
            }
        }
        s.ctx = self.ctx;
        s.ty = ty;
        if let Some(p) = g {
            s.x = p.0;
            s.y = p.1;
            s.npt = 1;
        }
        self.last = g;
        self.upd_ns = now;
        self.hist.push(s);
        if self.hist.len() > 16 {
            self.hist.remove(0);
        }
    }

    /// The newest `n` entries, newest first.
    pub fn newest_first(&self, n: usize) -> Vec<[u8; 0x60]> {
        self.hist.iter().rev().take(n).map(|e| e.bytes()).collect()
    }
}

static R: Mutex<Option<Recognizer>> = Mutex::new(None);

/// A read of GetGestureStates: one sample, then the newest `n` entries.
pub fn fill(n: usize, finger: Option<(u32, u32)>, now: u64) -> Vec<[u8; 0x60]> {
    let mut g = R.lock().unwrap();
    let r = g.get_or_insert_with(Recognizer::default);
    r.sample(finger, now);
    r.newest_first(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(r: &Recognizer) -> Vec<i32> {
        r.hist.iter().map(|e| e.ty).collect()
    }

    #[test]
    fn tap_press_pan_swipe() {
        let ms = 1_000_000u64;
        let mut r = Recognizer::default();
        r.sample(Some((100, 100)), 10 * ms);
        r.sample(Some((102, 101)), 20 * ms); // within the slop: no entry
        r.sample(None, 30 * ms);
        r.sample(None, 40 * ms);
        r.sample(None, 50 * ms);
        assert_eq!(types(&r), vec![TOUCH, TAP, COMPLETE, IDLE], "a click is a tap");
        assert_eq!(r.hist[0].attr & 1 << 4, 1 << 4, "IsNewTouch");
        // a second tap within 0.35 s is a double tap
        r.sample(Some((100, 100)), 200 * ms);
        r.sample(None, 210 * ms);
        assert_eq!(r.hist.last().unwrap().ty, TAP);
        assert_eq!(r.hist.last().unwrap().attr & 1 << 8, 1 << 8, "IsDoubleTap");
        let mut r = Recognizer::default();
        r.sample(Some((500, 300)), ms);
        r.sample(Some((500, 300)), 600 * ms);
        assert_eq!(types(&r), vec![TOUCH, PRESS], "held 0.5 s");
        let mut r = Recognizer::default();
        r.sample(Some((100, 300)), 0);
        r.sample(Some((200, 300)), 50 * ms);
        r.sample(Some((300, 300)), 100 * ms);
        r.sample(None, 110 * ms);
        let e = *r.hist.last().unwrap();
        assert_eq!((e.ty, e.dir), (SWIPE, 3), "a fast drag right is a swipe right: {:?}", types(&r));
        let b = e.bytes();
        assert_eq!(i32::from_le_bytes(b[0x10..0x14].try_into().unwrap()), SWIPE);
    }
}
