//! Motion fusion: a controller's gyro and accelerometer into what nn::hid's six-axis sensor reports (acceleration in
//! G, angular velocity in revolutions / s, the angle in revolutions, the direction as the controller's three axes in
//! the world). The design is nxnative's `motion_fusion.h`: the gyro integrated into the direction, the tilt pulled
//! toward the measured gravity with a small gain (a complementary filter), re-orthonormalised each step; samples are
//! kept in a short ring stamped on the host clock so a history read gets one sample per 5 ms tick, interpolated.

use std::f32::consts::TAU;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Seconds since the input layer started.
    pub t: f64,
    pub acc: [f32; 3],
    /// rev / s
    pub gyro: [f32; 3],
    /// revolutions
    pub angle: [f32; 3],
    /// rows: the controller's x, y, z axes in the world
    pub dir: [[f32; 3]; 3],
}

impl Default for Sample {
    fn default() -> Self {
        Sample {
            t: 0.0,
            acc: [0.0, 0.0, -1.0],
            gyro: [0.0; 3],
            angle: [0.0; 3],
            dir: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-6 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        a
    }
}

pub fn orthonormalise(d: &mut [[f32; 3]; 3]) {
    let x = norm(d[0]);
    let mut y = d[1];
    let k = dot(x, y);
    y = norm([y[0] - k * x[0], y[1] - k * x[1], y[2] - k * x[2]]);
    let z = cross(x, y);
    *d = [x, y, z];
}

pub const RING: usize = 128;
/// A stream with no sample for this long is stale (the sensor lies flat again).
pub const STALE_S: f64 = 0.3;

pub struct Stream {
    pub dir: [[f32; 3]; 3],
    pub angle: [f32; 3],
    pub last: f64,
    pub ring: Vec<Sample>,
    pub head: usize,
    pub count: usize,
}

impl Default for Stream {
    fn default() -> Self {
        Stream {
            dir: Sample::default().dir,
            angle: [0.0; 3],
            last: 0.0,
            ring: vec![Sample::default(); RING],
            head: 0,
            count: 0,
        }
    }
}

pub fn now_s() -> f64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

impl Stream {
    /// One sample at host time `t` (s): gravity direction in G, rotation in rad / s (Switch frame).
    pub fn add(&mut self, t: f64, acc: [f32; 3], rad: [f32; 3], gain: f32) {
        if self.count > 0 && t - self.last > STALE_S {
            *self = Stream::default();
        }
        if self.count > 0 && t <= self.last {
            return;
        }
        let dt = if self.count > 0 {
            (t - self.last) as f32
        } else {
            0.0
        };
        // the measured gravity in the controller's frame against the one the direction predicts (world down = -z)
        let g_meas = norm(acc);
        let g_pred = [-self.dir[0][2], -self.dir[1][2], -self.dir[2][2]];
        let la = dot(acc, acc).sqrt();
        let mut w = rad;
        if (0.5..1.5).contains(&la) {
            let e = cross(g_meas, g_pred);
            for i in 0..3 {
                w[i] += gain * e[i];
            }
        }
        if dt > 0.0 {
            // the world rotation rate, then each axis turned by it
            let ww = [
                self.dir[0][0] * w[0] + self.dir[1][0] * w[1] + self.dir[2][0] * w[2],
                self.dir[0][1] * w[0] + self.dir[1][1] * w[1] + self.dir[2][1] * w[2],
                self.dir[0][2] * w[0] + self.dir[1][2] * w[1] + self.dir[2][2] * w[2],
            ];
            for r in 0..3 {
                let c = cross(ww, self.dir[r]);
                for k in 0..3 {
                    self.dir[r][k] += c[k] * dt;
                }
            }
            orthonormalise(&mut self.dir);
            for i in 0..3 {
                self.angle[i] += rad[i] * dt / TAU;
            }
        }
        self.last = t;
        let s = Sample {
            t,
            acc,
            gyro: [rad[0] / TAU, rad[1] / TAU, rad[2] / TAU],
            angle: self.angle,
            dir: self.dir,
        };
        let at = (self.head + self.count) % RING;
        self.ring[at] = s;
        if self.count < RING {
            self.count += 1;
        } else {
            self.head = (self.head + 1) % RING;
        }
    }

    pub fn fresh(&self, now: f64) -> bool {
        self.count > 0 && now - self.last <= STALE_S
    }

    /// The sample at host time `t`: interpolated between the two around it, the newest held after it.
    pub fn at(&self, t: f64) -> Sample {
        if self.count == 0 {
            return Sample::default();
        }
        let get = |i: usize| &self.ring[(self.head + i) % RING];
        let (first, last) = (get(0), get(self.count - 1));
        if t <= first.t {
            return *first;
        }
        if t >= last.t {
            return *last;
        }
        let (mut lo, mut hi) = (0, self.count - 1);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if get(mid).t <= t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let (a, b) = (get(lo), get(hi));
        let f = ((t - a.t) / (b.t - a.t)) as f32;
        let mut o = if f < 0.5 { *a } else { *b };
        o.t = t;
        for i in 0..3 {
            o.acc[i] = a.acc[i] + (b.acc[i] - a.acc[i]) * f;
            o.gyro[i] = a.gyro[i] + (b.gyro[i] - a.gyro[i]) * f;
            o.angle[i] = a.angle[i] + (b.angle[i] - a.angle[i]) * f;
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn turning_and_tilt() {
        let mut s = Stream::default();
        // a quarter turn about z in one second, flat
        for i in 0..=200 {
            s.add(
                i as f64 * 0.005,
                [0.0, 0.0, -1.0],
                [0.0, 0.0, TAU / 4.0],
                0.0,
            );
        }
        let n = s.at(1.0);
        assert!((n.angle[2] - 0.25).abs() < 0.01, "{:?}", n.angle);
        assert!(
            (n.dir[0][1] - 1.0).abs() < 0.02,
            "x axis now points along world y: {:?}",
            n.dir
        );
        // tilt correction pulls a wrong attitude back to the measured gravity
        let mut s = Stream::default();
        s.dir = [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]]; // stood on end
        for i in 0..=2000 {
            s.add(i as f64 * 0.005, [0.0, 0.0, -1.0], [0.0; 3], 0.5);
        }
        assert!(
            (s.dir[2][2] - 1.0).abs() < 0.05,
            "back to flat: {:?}",
            s.dir
        );
    }
}
