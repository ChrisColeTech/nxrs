//! Nintendo's Switch 1 controllers over raw HID: the Pro Controller (057e:2009, USB and Bluetooth), Joy-Con (L) 2006
//! and (R) 2007 over Bluetooth, and the Online classic pads that speak the same protocol (2017 SNES, 2019 N64, 201e
//! Mega Drive). The protocol is the community's (dekuNukem's Nintendo_Switch_Reverse_Engineering notes); the design
//! follows nxnative's `pad_pro.h` / `pad_jc1.h`: the host asks for the full report 0x30 (subcommand 0x03), the IMU
//! (0x40), vibration (0x48) and the player lights (0x30), reads each stick's calibration from SPI flash (the user's
//! at 0x8010 when present, else the factory's at 0x603D), and decodes report 0x30: buttons, sticks, three IMU samples
//! 5 ms apart. Rumble is HD rumble: each side's two (frequency, amplitude) pairs, the game's own values.

pub const VID: u16 = 0x057e;
pub const PRO: u16 = 0x2009;
pub const JOY_L: u16 = 0x2006;
pub const JOY_R: u16 = 0x2007;

/// What kind of pad a product is: 0 not ours, 1 Pro-like, 2 left Joy-Con, 3 right Joy-Con.
pub fn kind(vid: u16, pid: u16) -> u8 {
    if vid != VID {
        return 0;
    }
    match pid {
        PRO | 0x2017 | 0x2019 | 0x201e => 1,
        JOY_L => 2,
        JOY_R => 3,
        _ => 0,
    }
}

pub const NEUTRAL_RUMBLE: [u8; 8] = [0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40];

/// Output report 0x01: a subcommand with the rumble data alongside.
pub fn subcommand(counter: u8, rumble: &[u8; 8], id: u8, args: &[u8]) -> Vec<u8> {
    let mut o = vec![0x01, counter & 0x0f];
    o.extend_from_slice(rumble);
    o.push(id);
    o.extend_from_slice(args);
    o
}

/// Output report 0x10: rumble only.
pub fn rumble_report(counter: u8, rumble: &[u8; 8]) -> Vec<u8> {
    let mut o = vec![0x10, counter & 0x0f];
    o.extend_from_slice(rumble);
    o
}

/// One side's HD rumble (4 bytes) for a high band (frequency, amplitude) and a low band, as nn::hid::VibrationValue
/// gives them: amplitudes 0..1, frequencies in Hz.
pub fn encode_side(amp_low: f32, freq_low: f32, amp_high: f32, freq_high: f32) -> [u8; 4] {
    fn freq(f: f32, lo: f32, hi: f32) -> u8 {
        let f = if f.is_finite() { f.clamp(lo, hi) } else { lo };
        ((f / 10.0).log2() * 32.0).round().clamp(0.0, 255.0) as u8
    }
    fn amp(a: f32) -> u8 {
        let a = if a.is_finite() {
            a.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if a < 0.01 {
            return 0;
        }
        let v = if a > 0.23 {
            (a * 8.7).log2() * 32.0
        } else if a > 0.12 {
            (a * 17.0).log2() * 16.0
        } else {
            ((a.log2() * 32.0) - 96.0) / (4.0 - 2.0 * a)
        };
        v.round().clamp(0.0, 100.0) as u8
    }
    let hf = (freq(freq_high, 81.75, 1252.0) as u16).saturating_sub(0x60) * 4;
    let lf = freq(freq_low, 40.875, 626.0).saturating_sub(0x40);
    let ea_high = amp(amp_high);
    let ea_low = amp(amp_low);
    let hf_amp = (ea_high as u16) * 2;
    let lf_amp = (ea_low as u16) / 2 + 64;
    if ea_high == 0 && ea_low == 0 {
        return [0x00, 0x01, 0x40, 0x40];
    }
    [
        (hf & 0xff) as u8,
        (hf_amp + ((hf >> 8) & 0xff)) as u8,
        (lf as u16 + ((lf_amp >> 8) & 0xff)) as u8,
        (lf_amp & 0xff) as u8,
    ]
}

/// A stick's calibration state, per axis: rest and the reach below and above it (12-bit units).
#[derive(Clone, Copy, Debug)]
pub struct Sticks {
    pub seen: bool,
    pub cal: [u8; 2], // 0 measured, 1 factory, 2 user
    pub rest: [i32; 4],
    pub reach: [[i32; 2]; 4],
}

impl Default for Sticks {
    fn default() -> Self {
        Sticks {
            seen: false,
            cal: [0; 2],
            rest: [2048; 4],
            reach: [[1200; 2]; 4],
        }
    }
}

pub const SPI_FACTORY: u32 = 0x603d;
pub const SPI_USER: u32 = 0x8010;
pub const SPI_IMU_CAL: u32 = 0x6020;

/// One stick's 9 calibration bytes as (x, y) rest, below, above. The left stick stores above, centre, below; the right
/// centre, below, above.
fn cal_decode(d: &[u8], right: bool) -> [i32; 6] {
    let mut v = [0i32; 6];
    for i in 0..3 {
        v[2 * i] = ((d[3 * i + 1] as i32) << 8 & 0xf00) | d[3 * i] as i32;
        v[2 * i + 1] = (d[3 * i + 2] as i32) << 4 | (d[3 * i + 1] as i32) >> 4;
    }
    let order: [usize; 3] = if right { [0, 1, 2] } else { [1, 2, 0] };
    let mut c = [0i32; 6];
    for k in 0..3 {
        c[2 * k] = v[2 * order[k]];
        c[2 * k + 1] = v[2 * order[k] + 1];
    }
    c
}

fn cal_sane(c: &[i32; 6]) -> bool {
    (0..2)
        .all(|a| (c[a] - 2048).abs() <= 500 && (1..3).all(|k| (400..=2048).contains(&c[2 * k + a])))
}

impl Sticks {
    /// A reply (report 0x21) to an SPI read of a calibration area: sets the sticks it holds a sane calibration for
    /// (the user's wins over the factory's). Returns the sticks set as bits (1 left, 2 right).
    pub fn spi_reply(&mut self, r: &[u8]) -> u32 {
        if r.len() < 20 || r[0] != 0x21 || r[13] & 0x80 == 0 || r[14] != 0x10 {
            return 0;
        }
        let addr = u32::from_le_bytes([r[15], r[16], r[17], r[18]]);
        let n = r[19] as usize;
        let Some(d) = r.get(20..20 + n) else { return 0 };
        let mut set = 0;
        for stick in 0..2 {
            let (at, kind) = if addr == SPI_USER
                && n >= 22
                && d[11 * stick] == 0xb2
                && d[11 * stick + 1] == 0xa1
            {
                (&d[11 * stick + 2..], 2)
            } else if addr == SPI_FACTORY && n >= 18 && self.cal[stick] < 2 {
                (&d[9 * stick..], 1)
            } else {
                continue;
            };
            let c = cal_decode(at, stick == 1);
            if !cal_sane(&c) {
                continue;
            }
            for a in 0..2 {
                self.rest[2 * stick + a] = c[a];
                self.reach[2 * stick + a] = [c[2 + a], c[4 + a]];
            }
            self.cal[stick] = kind;
            set |= 1 << stick;
        }
        if set != 0 {
            self.seen = true;
        }
        set
    }

    fn axis(&mut self, a: usize, raw: i32) -> i32 {
        let v = raw - self.rest[a];
        let mag = v.abs();
        let reach = &mut self.reach[a][(v > 0) as usize];
        if mag > *reach && self.cal[a / 2] == 0 {
            *reach = mag;
        }
        let dead = *reach * 7 / 100;
        if mag <= dead {
            return 0;
        }
        let out = ((mag - dead) as i64 * 32767 / (*reach - dead).max(1) as i64).min(32767) as i32;
        if v < 0 {
            -out
        } else {
            out
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    /// Buttons by label in the Switch bit order (Nintendo pads: label = position), rail buttons at 24..27.
    pub buttons: u64,
    pub sticks: [i32; 4],
    pub home: bool,
    pub capture: bool,
    /// Three IMU samples, oldest first: accelerometer and gyro raw counts, in the chip's axes.
    pub imu: [[i16; 6]; 3],
    pub has_imu: bool,
}

/// Decodes report 0x30 (or 0x21, which carries the same input block). `side`: 1 Pro-like, 2 left, 3 right Joy-Con.
pub fn decode(st: &mut Sticks, r: &[u8], side: u8) -> Option<Report> {
    if r.len() < 12 || (r[0] != 0x30 && r[0] != 0x21) {
        return None;
    }
    let raw = [
        r[6] as i32 | ((r[7] as i32 & 0x0f) << 8),
        (r[7] as i32) >> 4 | (r[8] as i32) << 4,
        r[9] as i32 | ((r[10] as i32 & 0x0f) << 8),
        (r[10] as i32) >> 4 | (r[11] as i32) << 4,
    ];
    if !st.seen {
        st.seen = true;
        for a in 0..4 {
            st.rest[a] = if (raw[a] - 2048).abs() <= 500 {
                raw[a]
            } else {
                2048
            };
            st.reach[a] = [1200; 2];
        }
    }
    let mut rep = Report::default();
    const MAP: &[(usize, u8, u32)] = &[
        (3, 0x01, 3),  // Y
        (3, 0x02, 2),  // X
        (3, 0x04, 1),  // B
        (3, 0x08, 0),  // A
        (3, 0x40, 7),  // R
        (3, 0x80, 9),  // ZR
        (4, 0x01, 11), // Minus
        (4, 0x02, 10), // Plus
        (4, 0x04, 5),  // right stick
        (4, 0x08, 4),  // left stick
        (5, 0x01, 15), // Down
        (5, 0x02, 13), // Up
        (5, 0x04, 14), // Right
        (5, 0x08, 12), // Left
        (5, 0x40, 6),  // L
        (5, 0x80, 8),  // ZL
    ];
    for &(at, bit, b) in MAP {
        if r[at] & bit != 0 {
            rep.buttons |= 1 << b;
        }
    }
    // the rail buttons of a Joy-Con: SL / SR, NpadButton 24, 25 (left) and 26, 27 (right)
    if side == 2 {
        if r[5] & 0x20 != 0 {
            rep.buttons |= 1 << 24;
        }
        if r[5] & 0x10 != 0 {
            rep.buttons |= 1 << 25;
        }
    } else if side == 3 {
        if r[3] & 0x20 != 0 {
            rep.buttons |= 1 << 26;
        }
        if r[3] & 0x10 != 0 {
            rep.buttons |= 1 << 27;
        }
    }
    rep.home = r[4] & 0x10 != 0;
    rep.capture = r[4] & 0x20 != 0;
    for a in 0..4 {
        rep.sticks[a] = st.axis(a, raw[a]);
    }
    // a Joy-Con's absent stick reads 0: keep it centred
    if side == 2 {
        rep.sticks[2] = 0;
        rep.sticks[3] = 0;
    }
    if side == 3 {
        rep.sticks[0] = 0;
        rep.sticks[1] = 0;
    }
    if r[0] == 0x30 && r.len() >= 49 {
        rep.has_imu = true;
        for s in 0..3 {
            for k in 0..6 {
                let o = 13 + 12 * s + 2 * k;
                rep.imu[s][k] = i16::from_le_bytes([r[o], r[o + 1]]);
            }
        }
    }
    Some(rep)
}

/// One IMU sample in the Switch frame: gravity direction in G (flat: 0, 0, -1) and rotation in rad / s. The chip's
/// +X runs toward the controller's top, +Y along the short axis, +Z out of the face (Pro, left Joy-Con) or out of the
/// back (right Joy-Con); the Switch frame is x right, y toward the top, z out of the face.
pub fn imu_switch(s: &[i16; 6], side: u8) -> ([f32; 3], [f32; 3]) {
    const ACC_G: f32 = 0.000_244_14; // ±8 G
    const GYRO_RAD: f32 = 0.061_035 * std::f32::consts::PI / 180.0; // ±2000 deg/s
    let f = [
        s[0] as f32 * ACC_G,
        s[1] as f32 * ACC_G,
        s[2] as f32 * ACC_G,
    ];
    let g = [
        s[3] as f32 * GYRO_RAD,
        s[4] as f32 * GYRO_RAD,
        s[5] as f32 * GYRO_RAD,
    ];
    if side == 3 {
        ([-f[1], -f[0], f[2]], [g[1], g[0], -g[2]])
    } else {
        ([f[1], -f[0], -f[2]], [-g[1], g[0], g[2]])
    }
}

/// The player-light pattern for player p (0-based), as the console lights them.
pub fn lights(p: usize) -> u8 {
    [
        0b0001, 0b0011, 0b0111, 0b1111, 0b1001, 0b0101, 0b1101, 0b0110,
    ]
    .get(p)
    .copied()
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rumble_encoding() {
        assert_eq!(
            encode_side(0.0, 160.0, 0.0, 320.0),
            [0x00, 0x01, 0x40, 0x40],
            "silence is the neutral pattern"
        );
        let s = encode_side(1.0, 160.0, 1.0, 320.0);
        // 320 Hz: round(log2(32)*32) = 160 = 0xa0, (0xa0 - 0x60) * 4 = 0x100; amplitude 1.0: round(log2(8.7)*32) = 100
        assert_eq!(s[0], 0x00);
        assert_eq!(s[1], 200 + 1);
        // 160 Hz: round(log2(16)*32) = 128 = 0x80, - 0x40 = 0x40; low amplitude 100 / 2 + 64 = 114
        assert_eq!(s[2], 0x40);
        assert_eq!(s[3], 114);
    }

    #[test]
    fn report_30() {
        let mut r = [0u8; 49];
        r[0] = 0x30;
        r[3] = 0x08 | 0x80; // A, ZR
        r[4] = 0x02 | 0x10; // Plus, Home
        r[5] = 0x02; // Up
                     // sticks at rest: 0x800 each
        r[6] = 0x00;
        r[7] = 0x08;
        r[8] = 0x80;
        r[9] = 0x00;
        r[10] = 0x08;
        r[11] = 0x80;
        // IMU sample 0: accel z = 4096 (1 G, face up), gyro x = 16384
        r[13 + 4] = 0x00;
        r[13 + 5] = 0x10;
        let mut st = Sticks::default();
        let rep = decode(&mut st, &r, 1).unwrap();
        assert_eq!(rep.buttons, 1 | 1 << 9 | 1 << 10 | 1 << 13);
        assert!(rep.home);
        assert_eq!(rep.sticks, [0; 4]);
        assert!(rep.has_imu);
        let (acc, _) = imu_switch(&rep.imu[0], 1);
        assert!(
            (acc[2] + 1.0).abs() < 0.01,
            "face up reads gravity down: {acc:?}"
        );
        // left stick pushed right
        r[6] = 0xff;
        r[7] = 0x0f | 0x80;
        let rep = decode(&mut st, &r, 1).unwrap();
        assert!(rep.sticks[0] > 30000);
    }

    #[test]
    fn spi_calibration() {
        let mut st = Sticks::default();
        let mut r = vec![0u8; 20 + 18];
        r[0] = 0x21;
        r[13] = 0x90;
        r[14] = 0x10;
        r[15..19].copy_from_slice(&SPI_FACTORY.to_le_bytes());
        r[19] = 18;
        // left stick: above (0x600, 0x600), centre (0x7f0, 0x810), below (0x600, 0x600)
        let enc = |x: i32, y: i32| {
            [
                (x & 0xff) as u8,
                ((x >> 8) & 0x0f | (y & 0x0f) << 4) as u8,
                (y >> 4) as u8,
            ]
        };
        let mut d = vec![];
        d.extend(enc(0x600, 0x600));
        d.extend(enc(0x7f0, 0x810));
        d.extend(enc(0x600, 0x600));
        d.extend(enc(0x7f0, 0x810)); // right stick: centre, below, above
        d.extend(enc(0x600, 0x600));
        d.extend(enc(0x600, 0x600));
        r[20..38].copy_from_slice(&d);
        assert_eq!(st.spi_reply(&r), 3);
        assert_eq!(st.rest[0], 0x7f0);
        assert_eq!(st.rest[1], 0x810);
        assert_eq!(st.cal, [1, 1]);
    }
}
