//! The gesture recognizer behind nn::hid::GetGestureStates (nxnative hle_touch.c `gesture_sample`, ported; yuzu
//! hid_core's rules): the fingers on the touch screen (the mouse, the two-finger mouse, or the control pipe's
//! `touch` / `drag` / `pinch` / `rotate`), at most four. A new touch is Touch (IsNewTouch, unless the entry before is
//! Cancel); held still 0.5 s it becomes Press; moved, Pan (the mid point's delta from the last entry, velocity over
//! the time since it); two or more fingers whose spread changes by more than 0.5 px Pinch (scale = new / old
//! spread), or whose angle turns more than 0.015 rad Rotate (degrees); a change in the finger count Cancel. On
//! release Touch becomes Tap (IsDoubleTap within 0.35 s of the last tap), Pan becomes Swipe above 400 px/s
//! (direction by the larger delta) or Complete, the others Complete; Idle follows Complete or Cancel. An entry is
//! written only when the fingers moved, after a forced step or when a Touch is due to become Press; a gesture stays
//! put until a finger moved 8 px from where it went down (a click wobbles). GestureState is 0x60 bytes.

use std::sync::Mutex;

const IDLE: i32 = 0;
const COMPLETE: i32 = 1;
const CANCEL: i32 = 2;
const TOUCH: i32 = 3;
const PRESS: i32 = 4;
const TAP: i32 = 5;
const PAN: i32 = 6;
const SWIPE: i32 = 7;
const PINCH: i32 = 8;
const ROTATE: i32 = 9;
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
    pub scale: f32,
    pub rot: f32,
    pub npt: i32,
    pub pt: [[i32; 2]; 4],
}

impl Entry {
    /// GestureState: +0 sampling number, +8 context number, +0x10 type, +0x14 direction, +0x18 x, +0x1c y (the
    /// fingers' mid point), +0x20 delta x, +0x24 delta y, +0x28 velocity x, +0x2c velocity y, +0x30 attributes,
    /// +0x34 scale, +0x38 rotation angle, +0x3c point count, +0x40 four points (x, y).
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
        b[0x34..0x38].copy_from_slice(&self.scale.to_le_bytes());
        b[0x38..0x3c].copy_from_slice(&self.rot.to_le_bytes());
        b[0x3c..0x40].copy_from_slice(&self.npt.to_le_bytes());
        for (i, p) in self.pt.iter().enumerate() {
            b[0x40 + 8 * i..0x44 + 8 * i].copy_from_slice(&p[0].to_le_bytes());
            b[0x44 + 8 * i..0x48 + 8 * i].copy_from_slice(&p[1].to_le_bytes());
        }
        b
    }
}

/// The fingers of one sample: their points (in order, at most 4), mid point, mean spread from it and the angle of
/// the first finger from it (nxnative `gesture_props`).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Props {
    n: usize,
    pt: [[i32; 2]; 4],
    mx: i32,
    my: i32,
    dist: f32,
    angle: f32,
}

fn props(fingers: &[(u32, u32)]) -> Props {
    let mut g = Props::default();
    for &(x, y) in fingers.iter().take(4) {
        g.pt[g.n] = [x.min(1279) as i32, y.min(719) as i32];
        g.n += 1;
    }
    let n = g.n as i32;
    for i in 0..g.n {
        g.mx += g.pt[i][0] / n;
        g.my += g.pt[i][1] / n;
    }
    for i in 0..g.n {
        g.dist += ((g.mx - g.pt[i][0]) as f32).hypot((g.my - g.pt[i][1]) as f32) / g.n as f32;
    }
    if g.n > 0 {
        g.angle = ((g.my - g.pt[0][1]) as f32).atan2((g.mx - g.pt[0][0]) as f32);
    }
    g
}

#[derive(Default)]
pub struct Recognizer {
    hist: Vec<Entry>,
    last: Props,
    down: Props,
    ctx: i64,
    force: bool,
    tap_ok: bool,
    slop: bool,
    upd_ns: u64,
    tap_ns: u64,
    pan_dt: f32,
}

impl Recognizer {
    /// One sample with the fingers where they are now (none: up), at `now` ns.
    pub fn sample(&mut self, fingers: &[(u32, u32)], now: u64) {
        let mut g = props(fingers);
        let le = self.hist.last().copied();
        let ltype = le.map_or(IDLE, |e| e.ty);
        let dt = if self.upd_ns != 0 { now.saturating_sub(self.upd_ns) as f32 / 1e9 } else { 0.0 };
        if g.n > 0 && g.n == self.last.n && !self.slop {
            // within the slop the fingers stay where they went down
            let out = (0..g.n).any(|i| (g.pt[i][0] - self.down.pt[i][0]).abs() > SLOP || (g.pt[i][1] - self.down.pt[i][1]).abs() > SLOP);
            if out {
                self.slop = true;
            } else {
                g = self.last;
            }
        }
        let mut upd = self.force || g.n != self.last.n || g.pt != self.last.pt;
        if !upd && ltype == TOUCH && le.is_some_and(|e| e.npt == 1) && dt > 0.5 {
            upd = self.tap_ok; // due to become Press
        }
        if !upd {
            return;
        }
        self.force = false;
        let mut s = Entry { samp: le.map_or(1, |e| e.samp + 1), ..Default::default() };
        let mut ty = IDLE;
        if g.n > 0 {
            if self.last.n == 0 {
                // a new gesture
                self.ctx += 1;
                ty = TOUCH;
                self.down = g;
                self.slop = false;
                if ltype != CANCEL {
                    s.attr |= 1 << 4;
                    self.tap_ok = true;
                }
            } else if g.n != self.last.n {
                // the finger count changed: cancel and start over at the next sample
                ty = CANCEL;
                self.tap_ok = false;
                g = Props::default();
            } else if g.pt != self.last.pt {
                ty = PAN;
                let (lx, ly) = le.map_or((g.mx, g.my), |e| (e.x, e.y));
                s.dx = g.mx - lx;
                s.dy = g.my - ly;
                if dt > 0.0 {
                    s.vx = s.dx as f32 / dt;
                    s.vy = s.dy as f32 / dt;
                }
                self.pan_dt = dt;
                if (g.dist - self.last.dist).abs() > 0.5 {
                    ty = PINCH;
                    s.scale = if self.last.dist > 0.0 { g.dist / self.last.dist } else { 1.0 };
                }
                let turn = ((g.angle - self.last.angle) / (1.0 + g.angle * self.last.angle)).atan();
                if turn.abs() > 0.015 {
                    ty = ROTATE;
                    s.scale = 0.0;
                    s.rot = turn.to_degrees();
                }
            } else if ltype == TOUCH {
                ty = PRESS;
            }
        } else if self.last.n > 0 {
            // released
            match ltype {
                TOUCH => {
                    self.force = true;
                    if !self.tap_ok {
                        ty = CANCEL;
                    } else {
                        ty = TAP;
                        g = self.last;
                        if self.tap_ns != 0 && now - self.tap_ns < 350_000_000 {
                            s.attr |= 1 << 8;
                        }
                        self.tap_ns = now;
                    }
                }
                PRESS | TAP | SWIPE | PINCH | ROTATE => {
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
                        g = self.last;
                        s.vx = vx;
                        s.vy = vy;
                        s.dx = e.dx;
                        s.dy = e.dy;
                        s.dir = if s.dx.abs() > s.dy.abs() { if s.dx > 0 { 3 } else { 1 } } else if s.dy > 0 { 4 } else { 2 };
                    }
                }
                _ => {}
            }
        } else if ltype == COMPLETE || ltype == CANCEL {
            self.ctx += 1;
        }
        s.ctx = self.ctx;
        s.ty = ty;
        s.x = g.mx;
        s.y = g.my;
        s.npt = g.n as i32;
        s.pt = g.pt;
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

/// A read of GetGestureStates: one sample of the fingers down, then the newest `n` entries.
pub fn fill(n: usize, fingers: &[(u32, u32)], now: u64) -> Vec<[u8; 0x60]> {
    let mut g = R.lock().unwrap();
    let r = g.get_or_insert_with(Recognizer::default);
    r.sample(fingers, now);
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
        r.sample(&[(100, 100)], 10 * ms);
        r.sample(&[(102, 101)], 20 * ms); // within the slop: no entry
        r.sample(&[], 30 * ms);
        r.sample(&[], 40 * ms);
        r.sample(&[], 50 * ms);
        assert_eq!(types(&r), vec![TOUCH, TAP, COMPLETE, IDLE], "a click is a tap");
        assert_eq!(r.hist[0].attr & 1 << 4, 1 << 4, "IsNewTouch");
        // a second tap within 0.35 s is a double tap
        r.sample(&[(100, 100)], 200 * ms);
        r.sample(&[], 210 * ms);
        assert_eq!(r.hist.last().unwrap().ty, TAP);
        assert_eq!(r.hist.last().unwrap().attr & 1 << 8, 1 << 8, "IsDoubleTap");
        let mut r = Recognizer::default();
        r.sample(&[(500, 300)], ms);
        r.sample(&[(500, 300)], 600 * ms);
        assert_eq!(types(&r), vec![TOUCH, PRESS], "held 0.5 s");
        let mut r = Recognizer::default();
        r.sample(&[(100, 300)], 0);
        r.sample(&[(200, 300)], 50 * ms);
        r.sample(&[(300, 300)], 100 * ms);
        r.sample(&[], 110 * ms);
        let e = *r.hist.last().unwrap();
        assert_eq!((e.ty, e.dir), (SWIPE, 3), "a fast drag right is a swipe right: {:?}", types(&r));
        let b = e.bytes();
        assert_eq!(i32::from_le_bytes(b[0x10..0x14].try_into().unwrap()), SWIPE);
    }

    /// nxnative hle_touch.c: two fingers spreading are a Pinch with scale new / old spread; two fingers turning
    /// about their mid point a Rotate in degrees; their release Complete; a third finger joining Cancel.
    #[test]
    fn pinch_rotate_and_finger_count() {
        let ms = 1_000_000u64;
        let mut r = Recognizer::default();
        r.sample(&[(600, 360), (680, 360)], ms);
        assert_eq!((r.hist[0].ty, r.hist[0].npt, r.hist[0].x), (TOUCH, 2, 640));
        r.sample(&[(580, 360), (700, 360)], 20 * ms); // spread 40 -> 60
        let e = *r.hist.last().unwrap();
        assert_eq!(e.ty, PINCH);
        assert!((e.scale - 1.5).abs() < 1e-6, "{}", e.scale);
        let b = e.bytes();
        assert_eq!(f32::from_le_bytes(b[0x34..0x38].try_into().unwrap()), e.scale);
        assert_eq!(i32::from_le_bytes(b[0x3c..0x40].try_into().unwrap()), 2);
        assert_eq!(i32::from_le_bytes(b[0x48..0x4c].try_into().unwrap()), 700, "the second point's x");
        r.sample(&[], 40 * ms);
        assert_eq!(r.hist.last().unwrap().ty, COMPLETE);

        // a quarter turn about (640, 360), radius 60, in steps
        let mut r = Recognizer::default();
        let at = |deg: f64| crate::input::two_fingers(640.0, 360.0, 60.0, deg);
        r.sample(&at(0.0), ms);
        r.sample(&at(20.0), 20 * ms);
        let e = *r.hist.last().unwrap();
        assert_eq!(e.ty, ROTATE, "{:?}", types(&r));
        assert!((e.rot - 20.0).abs() < 1.5, "{} degrees", e.rot);
        assert_eq!(e.scale, 0.0);

        // the finger count changes: Cancel, and the next touch has no IsNewTouch
        let mut r = Recognizer::default();
        r.sample(&[(100, 100), (200, 100)], ms);
        r.sample(&[(100, 100), (200, 100), (300, 100)], 20 * ms);
        assert_eq!(r.hist.last().unwrap().ty, CANCEL);
        r.sample(&[(100, 100)], 40 * ms);
        assert_eq!(r.hist.last().unwrap().ty, TOUCH);
        assert_eq!(r.hist.last().unwrap().attr & 1 << 4, 0);
    }
}
