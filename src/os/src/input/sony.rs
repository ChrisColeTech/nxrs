//! DualSense, DualSense Edge and DualShock 4 over raw HID: the report decoder and the output packets (rumble, player
//! LED, lightbar), from the owner's `controller_sony.h` / `controller_sony_win32.h` in the Turbo fork, with the
//! sensors (gyro and accelerometer) added. Report layouts follow Linux's hid-playstation.
//!
//! | Model | Products | USB in / out | Bluetooth in / out |
//! |---|---|---|---|
//! | DualSense / Edge | 0ce6, 0df2 | 01, 64 / 02, 48 (63 zero padded) | 01 simple or 31, 78 / 31, 78 |
//! | DualShock 4 | 05c4, 09cc | 01, 64 / 05, 32 | 01 simple or 11, 78 / 11, 78 |
//!
//! Buttons come out by position in the Switch bit order (right = bit 0 where a Switch has A, bottom = 1, top = 2,
//! left = 3); `layout.rs` maps them to the guest.

pub const DS4: u8 = 4;
pub const DS5: u8 = 5;
pub const USB: u8 = 1;
pub const BT: u8 = 2;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SonyState {
    pub buttons: u64,
    pub lx: i32,
    pub ly: i32,
    pub rx: i32,
    pub ry: i32,
    /// Accelerometer in G (specific force) and gyro in degrees / s, in the controller's own axes; `imu` when present.
    pub acc: [f32; 3],
    pub gyro: [f32; 3],
    pub imu: bool,
    /// The PS button (Home).
    pub home: bool,
}

pub fn model(vid: u16, pid: u16) -> u8 {
    if vid != 0x054c {
        return 0;
    }
    match pid {
        0x0ce6 | 0x0df2 => DS5,
        0x05c4 | 0x09cc => DS4,
        _ => 0,
    }
}

pub fn transport(model: u8, input: u16, output: u16) -> u8 {
    if model != DS4 && model != DS5 {
        return 0;
    }
    if input == 78 && output == 78 {
        return BT;
    }
    if input == 64
        && (if model == DS5 {
            output == 48 || output == 63
        } else {
            output == 32
        })
    {
        return USB;
    }
    0
}

/// CRC-32 (reflected, 0xEDB88320) over `seed` then `p`, as Sony's Bluetooth reports carry it.
pub fn crc(seed: u8, p: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in std::iter::once(&seed).chain(p.iter()) {
        c ^= b as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ (0u32.wrapping_sub(c & 1) & 0xedb8_8320);
        }
    }
    !c
}

fn sign(p: &mut [u8]) {
    let c = crc(0xa2, &p[..74]);
    p[74..78].copy_from_slice(&c.to_le_bytes());
}

/// DualSense firmware feature report 0x20: the update version at byte 44. The "compatible vibration 2" emulation is
/// on from update 2.21 (0x0215), always on Edge, and for firmware the report does not give (as SDL does on 2.24+).
pub fn vibration_v2(pid: u16, feature: Option<&[u8]>) -> bool {
    if pid == 0x0df2 {
        return true;
    }
    match feature {
        Some(f) if f.len() >= 46 && f[0] == 0x20 => u16::from_le_bytes([f[44], f[45]]) >= 0x0215,
        _ => true,
    }
}

/// One output packet: motors (0..255), the player LED and lightbar on or off, or (`reset`) the DualSense's lightbar
/// release. Returns its length (0 for an unsupported combination).
#[allow(clippy::too_many_arguments)]
pub fn output(
    model: u8,
    transport: u8,
    seq: u32,
    low: u32,
    high: u32,
    lit: bool,
    reset: bool,
    v2: bool,
    out: &mut [u8; 78],
) -> usize {
    if (model != DS4 && model != DS5) || (transport != USB && transport != BT) {
        return 0;
    }
    let n = if transport == BT {
        78
    } else if model == DS5 {
        48
    } else {
        32
    };
    out.fill(0);
    let (low, high) = (low.min(255) as u8, high.min(255) as u8);
    let o = if transport == BT { 3 } else { 1 };
    if model == DS5 {
        out[0] = if transport == BT { 0x31 } else { 0x02 };
        if transport == BT {
            out[1] = ((seq & 15) << 4) as u8;
            out[2] = 0x10;
        }
        let p = &mut out[o..];
        if reset {
            p[38] = 2;
            p[41] = 2;
        } else {
            p[0] = 3;
            p[1] = 0x14;
            p[2] = high;
            p[3] = low;
            if v2 {
                p[38] = 4;
            }
            p[43] = if lit { 4 } else { 0 };
            p[44] = 0;
            p[45] = if lit { 40 } else { 0 };
            p[46] = if lit { 128 } else { 0 };
        }
    } else {
        out[0] = if transport == BT { 0x11 } else { 0x05 };
        if transport == BT {
            out[1] = 0xc4;
        }
        let p = &mut out[o..];
        p[0] = 7;
        p[3] = high;
        p[4] = low;
        p[6] = if lit { 40 } else { 0 };
        p[7] = if lit { 128 } else { 0 };
    }
    if transport == BT {
        sign(out);
    }
    n
}

fn axis(v: u8, invert: bool) -> i32 {
    let a = v as i32 - 128;
    let a = if a < 0 {
        a * 32767 / 128
    } else {
        a * 32767 / 127
    };
    let a = if a > -7000 && a < 7000 { 0 } else { a };
    if invert {
        -a
    } else {
        a
    }
}

fn i16le(p: &[u8], o: usize) -> f32 {
    i16::from_le_bytes([p[o], p[o + 1]]) as f32
}

/// Decodes one input report into `s`; false for a report of another kind, length or CRC.
pub fn input(model: u8, transport: u8, r: &[u8], s: &mut SonyState) -> bool {
    if r.is_empty() || (model != DS4 && model != DS5) {
        return false;
    }
    let (p, bo, full): (&[u8], usize, bool) = match transport {
        USB => {
            if r.len() != 64 || r[0] != 1 {
                return false;
            }
            (&r[1..], if model == DS5 { 7 } else { 4 }, true)
        }
        BT => {
            if r[0] == 1 {
                if r.len() < 10 {
                    return false;
                }
                (&r[1..], 4, false) // the simple Bluetooth report: no sensors
            } else {
                if r.len() != 78 || r[0] != if model == DS5 { 0x31 } else { 0x11 } {
                    return false;
                }
                let c = u32::from_le_bytes([r[74], r[75], r[76], r[77]]);
                if crc(0xa1, &r[..74]) != c {
                    return false;
                }
                (
                    &r[if model == DS5 { 2 } else { 3 }..],
                    if model == DS5 { 7 } else { 4 },
                    true,
                )
            }
        }
        _ => return false,
    };
    let (f, x, hat) = (p[bo], p[bo + 1], p[bo] & 15);
    let mut b = 0u64;
    // by position like every pad path: Circle (right) = A position, Cross (bottom) = B, Triangle (top) = X, Square (left) = Y
    let mut set = |c: bool, bit: u32| {
        if c {
            b |= 1 << bit;
        }
    };
    set(f & 0x40 != 0, 0);
    set(f & 0x20 != 0, 1);
    set(f & 0x80 != 0, 2);
    set(f & 0x10 != 0, 3);
    set(x & 0x40 != 0, 4);
    set(x & 0x80 != 0, 5);
    set(x & 1 != 0, 6);
    set(x & 2 != 0, 7);
    set(x & 4 != 0, 8);
    set(x & 8 != 0, 9);
    set(x & 0x20 != 0, 10);
    set(x & 0x10 != 0, 11);
    set(matches!(hat, 5..=7), 12);
    set(matches!(hat, 7 | 0 | 1), 13);
    set(matches!(hat, 1..=3), 14);
    set(matches!(hat, 3..=5), 15);
    s.buttons = b;
    s.home = p.get(bo + 2).is_some_and(|v| v & 1 != 0);
    s.lx = axis(p[0], false);
    s.ly = axis(p[1], true);
    s.rx = axis(p[2], false);
    s.ry = axis(p[3], true);
    // sensors: gyro (pitch, yaw, roll) then accelerometer, 16-bit; 1024 a degree / s, 8192 a G (nominal)
    let (g, a) = if model == DS5 { (15, 21) } else { (12, 18) };
    s.imu = full && p.len() >= a + 6;
    if s.imu {
        for i in 0..3 {
            s.gyro[i] = i16le(p, g + 2 * i) / 1024.0;
            s.acc[i] = i16le(p, a + 2 * i) / 8192.0;
        }
    }
    true
}

/// The controller's frame to the Switch frame (x right, y toward the controller's top, z out of the face), giving
/// the gravity direction in G (flat and still: 0, 0, -1) and the rotation in rad / s. Sony's sensor: x right, y out
/// of the face, z toward the player.
pub fn to_switch(s: &SonyState) -> ([f32; 3], [f32; 3]) {
    let d = std::f32::consts::PI / 180.0;
    let acc = [-s.acc[0], s.acc[2], -s.acc[1]];
    let gyro = [s.gyro[0] * d, -s.gyro[2] * d, s.gyro[1] * d];
    (acc, gyro)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard() {
        assert_eq!(crc(b'1', b"23456789"), 0xCBF4_3926);
    }

    #[test]
    fn dualsense_usb_lights_packet() {
        let mut p = [0u8; 78];
        assert_eq!(output(DS5, USB, 0, 0, 0, true, false, true, &mut p), 48);
        assert_eq!(&p[..3], &[0x02, 0x03, 0x14]);
        assert_eq!(p[39], 4, "compatible vibration 2");
        assert_eq!(
            &p[44..48],
            &[0x04, 0x00, 0x28, 0x80],
            "centre player LED, blue lightbar"
        );
        assert_eq!(
            output(DS5, USB, 0, 255, 128, false, false, false, &mut p),
            48
        );
        assert_eq!(
            (p[3], p[4]),
            (128, 255),
            "high (right) motor then low (left)"
        );
        assert_eq!(output(DS5, USB, 0, 0, 0, false, true, true, &mut p), 48);
        assert_eq!((p[39], p[42]), (2, 2), "lightbar release");
    }

    #[test]
    fn bluetooth_packets_are_signed() {
        let mut p = [0u8; 78];
        assert_eq!(output(DS5, BT, 3, 10, 20, true, false, false, &mut p), 78);
        assert_eq!((p[0], p[1], p[2]), (0x31, 0x30, 0x10));
        assert_eq!(
            u32::from_le_bytes(p[74..78].try_into().unwrap()),
            crc(0xa2, &p[..74])
        );
        assert_eq!(output(DS4, BT, 0, 1, 2, true, false, false, &mut p), 78);
        assert_eq!((p[0], p[1], p[3 + 3], p[3 + 4]), (0x11, 0xc4, 2, 1));
        assert_eq!(output(DS4, USB, 0, 1, 2, false, false, false, &mut p), 32);
        assert_eq!((p[0], p[1], p[4], p[5]), (0x05, 7, 2, 1));
    }

    #[test]
    fn dualsense_usb_input() {
        let mut r = [0u8; 64];
        r[0] = 1;
        r[1] = 128;
        r[2] = 0; // left stick fully up
        r[3] = 255;
        r[4] = 128;
        r[8] = 0x40 | 8; // Circle, hat centred
        r[9] = 0x01 | 0x20; // L1, Options
        r[16] = 0x00;
        r[17] = 0x04; // gyro pitch 1024 = 1 deg/s
        r[24] = 0x00;
        r[25] = 0x20; // accel y 8192 = 1 G (face up)
        let mut s = SonyState::default();
        assert!(input(DS5, USB, &r, &mut s));
        assert_eq!(s.buttons, 1 | 1 << 6 | 1 << 10);
        assert_eq!((s.lx, s.ly, s.rx), (0, 32767, 32767));
        assert!(s.imu);
        assert_eq!(s.gyro[0], 1.0);
        assert_eq!(s.acc[1], 1.0);
        let (acc, _) = to_switch(&s);
        assert_eq!(acc, [-0.0, 0.0, -1.0], "flat and still reads gravity down");
        r[8] = 0x20 | 1; // Cross, hat up-right
        assert!(input(DS5, USB, &r, &mut s));
        assert_eq!(
            s.buttons & 0xffff,
            1 << 1 | 1 << 13 | 1 << 14 | 1 << 6 | 1 << 10
        );
        assert!(!input(DS5, USB, &r[..63], &mut s), "wrong length rejected");
    }

    #[test]
    fn firmware_gate() {
        let mut f = vec![0u8; 64];
        f[0] = 0x20;
        f[44] = 0x14;
        f[45] = 0x02;
        assert!(!vibration_v2(0x0ce6, Some(&f)));
        f[44] = 0x30;
        f[45] = 0x06;
        assert!(vibration_v2(0x0ce6, Some(&f)));
        assert!(vibration_v2(0x0df2, Some(&[0u8; 2])));
    }
}
