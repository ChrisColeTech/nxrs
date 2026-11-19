//! Nintendo Switch 2 controllers (Pro Controller 2, Joy-Con 2 L / R) over Bluetooth LE: the protocol, after
//! nxnative's `switch2_proto.h` design (itself written from the community's reverse engineering). Pure, so the unit
//! tests check it without a controller; `switch2_ble.rs` is the Windows transport (WinRT GATT).
//!
//! - Advertisements: Nintendo's company id 0x0553; vendor 057e, product 0x2069 Pro, 0x2067 Joy-Con L, 0x2066 Joy-Con R;
//!   six zero host bytes = pairing mode (Sync held), else the host the controller wakes toward.
//! - GATT: input notifications (63-byte reports), command writes, command-reply notifications, a vibration channel.
//! - Commands: `cmd, 0x91, 0x01, sub, 0, len, 0, 0, payload`: memory read (0x02/0x04), player LEDs (0x09/0x07),
//!   features set / enable (0x0C/0x02, 0x0C/0x04), bonding (0x15).
//! - Report: clock (u32), buttons (u32 at 4), sticks (two packed 12-bit pairs at 10 and 13), battery, IMU at 0x30.
//! - Rumble: a zero byte, then per actuator 0x40 | samples << 4 | sequence and five-byte samples (low band then high,
//!   10-bit log frequency code and 10-bit amplitude each).

pub const COMPANY: u16 = 0x0553;
pub const VENDOR: u16 = 0x057e;
pub const PID_JOY_R: u16 = 0x2066;
pub const PID_JOY_L: u16 = 0x2067;
pub const PID_PRO: u16 = 0x2069;

pub const UUID_INPUT: u128 = 0xAB7DE9BE_89FE_49AD_828F_118F09DF7FD2;
pub const UUID_CMD: u128 = 0x649D4AC9_8EB7_4E6C_AF44_1EA54FE5F005;
pub const UUID_RSP: u128 = 0xC765A961_D9D8_4D36_A20A_5315B111836A;
pub const UUID_VIB_R: u128 = 0xFA19B0FB_CD1F_46A7_84A1_BBB09E00C149;
pub const UUID_VIB_L: u128 = 0x289326CB_A471_485D_A8F4_240C14F18241;
pub const UUID_VIB_PRO: u128 = 0xCC483F51_9258_427D_A939_630C31F72B05;

pub const CMD_MEMORY: u8 = 0x02;
pub const CMD_LEDS: u8 = 0x09;
pub const CMD_FEATURE: u8 = 0x0C;
pub const CMD_PAIR: u8 = 0x15;
pub const SUB_MEMORY_READ: u8 = 0x04;
pub const SUB_LEDS_SET: u8 = 0x07;
pub const SUB_FEATURE_SET: u8 = 0x02;
pub const SUB_FEATURE_ENABLE: u8 = 0x04;
pub const SUB_PAIR_HOST: u8 = 0x01;
pub const SUB_PAIR_KEY1: u8 = 0x04;
pub const SUB_PAIR_KEY2: u8 = 0x02;
pub const SUB_PAIR_DONE: u8 = 0x03;
/// Buttons, sticks, motion, battery current, magnetometer (the console's mask).
pub const FEATURES: u8 = 0x01 | 0x02 | 0x04 | 0x20 | 0x80;
/// The Joy-Con 2's mouse sensor (on its rail side); 0x16 below `MOUSE_SURFACE_MAX`: on a surface.
pub const FEATURE_MOUSE: u8 = 0x10;
pub const MOUSE_SURFACE_MAX: u16 = 1000;
pub const ADDR_INFO: u32 = 0x13000;
pub const ADDR_STICK1: u32 = 0x13080;
pub const ADDR_STICK2: u32 = 0x130C0;
pub const ADDR_USER_STICK1: u32 = 0x1FC042;
pub const ADDR_USER_STICK2: u32 = 0x1FC062;
pub const STICK_PARAMS: usize = 0x28;
pub const STICK_BLOCK: usize = STICK_PARAMS + 9;
pub const REPLY_DATA: usize = 16;

// raw buttons (u32 at report offset 4)
const B_Y: u32 = 0x1;
const B_X: u32 = 0x2;
const B_B: u32 = 0x4;
const B_A: u32 = 0x8;
const B_SR_R: u32 = 0x10;
const B_SL_R: u32 = 0x20;
const B_R: u32 = 0x40;
const B_ZR: u32 = 0x80;
const B_MINUS: u32 = 0x100;
const B_PLUS: u32 = 0x200;
const B_RSTICK: u32 = 0x400;
const B_LSTICK: u32 = 0x800;
pub const B_HOME: u32 = 0x1000;
const B_DOWN: u32 = 0x10000;
const B_UP: u32 = 0x20000;
const B_RIGHT: u32 = 0x40000;
const B_LEFT: u32 = 0x80000;
const B_SR_L: u32 = 0x100000;
const B_SL_L: u32 = 0x200000;
const B_L: u32 = 0x400000;
const B_ZL: u32 = 0x800000;

fn u16le(p: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([p[o], p[o + 1]])
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    pub clock_ms: u32,
    pub buttons: u32,
    pub lx: u16,
    pub ly: u16,
    pub rx: u16,
    pub ry: u16,
    /// accelerometer xyz, gyro xyz (counts)
    pub raw: [i16; 6],
    pub battery_mv: u16,
    /// The Joy-Con 2 mouse sensor (feature 0x10): position counters (wrap at 16 bits), quality, surface distance.
    pub mouse: [i16; 2],
    pub mouse_q: u16,
    pub mouse_s: u16,
}

pub fn parse_report(d: &[u8]) -> Option<Report> {
    if d.len() < 0x3c || d.len() > 0x50 {
        return None;
    }
    let a = d[10] as u32 | (d[11] as u32) << 8 | (d[12] as u32) << 16;
    let b = d[13] as u32 | (d[14] as u32) << 8 | (d[15] as u32) << 16;
    Some(Report {
        clock_ms: u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
        buttons: u32::from_le_bytes([d[4], d[5], d[6], d[7]]),
        lx: (a & 0xfff) as u16,
        ly: (a >> 12) as u16,
        rx: (b & 0xfff) as u16,
        ry: (b >> 12) as u16,
        raw: std::array::from_fn(|i| u16le(d, 0x30 + 2 * i) as i16),
        battery_mv: u16le(d, 0x1f),
        mouse: [u16le(d, 0x10) as i16, u16le(d, 0x12) as i16],
        mouse_q: u16le(d, 0x14),
        mouse_s: u16le(d, 0x16),
    })
}

pub fn command(cmd: u8, sub: u8, payload: &[u8]) -> Vec<u8> {
    let mut o = vec![cmd, 0x91, 0x01, sub, 0, payload.len() as u8, 0, 0];
    o.extend_from_slice(payload);
    o
}

pub fn memory_payload(len: u8, addr: u32) -> [u8; 8] {
    let a = addr.to_le_bytes();
    [len, 0x7e, 0, 0, a[0], a[1], a[2], a[3]]
}

pub fn reply_is(d: &[u8], cmd: u8) -> bool {
    d.len() >= 8 && d[0] == cmd && (d[1] == 1 || d[1] == 2)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Advert {
    pub vid: u16,
    pub pid: u16,
    pub pairing: bool,
    pub host: [u8; 6],
}

/// Manufacturer data after the company id.
pub fn parse_advert(d: &[u8]) -> Option<Advert> {
    if d.len() < 16 {
        return None;
    }
    let mut a = Advert { vid: u16le(d, 3), pid: u16le(d, 5), pairing: true, host: [0; 6] };
    for i in 10..16 {
        a.host[i - 10] = d[i];
        if d[i] != 0 {
            a.pairing = false;
        }
    }
    (a.vid == VENDOR && matches!(a.pid, PID_PRO | PID_JOY_L | PID_JOY_R)).then_some(a)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Ignore,
    Wake,
    Pair,
    Take,
}

/// In Sync: bond to this host (or use unbonded when its address is unknown); a wake toward this host: connect;
/// toward another host (a nearby console): leave it alone.
pub fn advert_use(a: &Advert, host: Option<[u8; 6]>) -> Use {
    if a.pairing {
        return if host.is_some() { Use::Pair } else { Use::Take };
    }
    if host == Some(a.host) {
        Use::Wake
    } else {
        Use::Ignore
    }
}

pub fn pair_host_payload(host: &[u8; 6]) -> [u8; 14] {
    let mut o = [0u8; 14];
    o[1] = 0x02;
    o[2..8].copy_from_slice(host);
    o[8..14].copy_from_slice(host);
    o
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cal {
    pub cx: f32,
    pub cy: f32,
    pub maxx: f32,
    pub maxy: f32,
    pub minx: f32,
    pub miny: f32,
    pub valid: bool,
    pub dead: f32,
}
impl Default for Cal {
    fn default() -> Self {
        Cal { cx: 2048.0, cy: 2048.0, maxx: 1200.0, maxy: 1200.0, minx: 1200.0, miny: 1200.0, valid: false, dead: 0.0 }
    }
}

fn pair12(d: &[u8]) -> (f32, f32) {
    let p = d[0] as u32 | (d[1] as u32) << 8 | (d[2] as u32) << 16;
    ((p & 0xfff) as f32, (p >> 12) as f32)
}

pub fn parse_cal(d: &[u8]) -> Option<Cal> {
    if d.len() < 9 || (d[0] == 0xff && d[1] == 0xff && d[2] == 0xff) {
        return None;
    }
    let (cx, cy) = pair12(&d[0..3]);
    let (maxx, maxy) = pair12(&d[3..6]);
    let (minx, miny) = pair12(&d[6..9]);
    let valid = maxx > 100.0 && maxy > 100.0 && minx > 100.0 && miny > 100.0;
    valid.then_some(Cal { cx, cy, maxx, maxy, minx, miny, valid, dead: 0.0 })
}

/// The stick parameters' third pair: the model's dead zone in counts (0 when not of that shape).
pub fn params_dead(d: &[u8]) -> f32 {
    if d.len() < STICK_PARAMS || d[0] == 0xff {
        return 0.0;
    }
    let (x, y) = pair12(&d[7..10]);
    if x != y || x == 0.0 || x == 4095.0 {
        0.0
    } else {
        x
    }
}

/// A stick's calibration from its factory block and user block: the user's when set, else the factory's.
pub fn cal_from_flash(block: Option<&[u8]>, user: Option<&[u8]>) -> (Cal, u8) {
    let dead = block.map(params_dead).unwrap_or(0.0);
    if let Some(c) = user.and_then(parse_cal) {
        return (Cal { dead, ..c }, 2);
    }
    if let Some(c) = block.filter(|b| b.len() >= STICK_BLOCK).and_then(|b| parse_cal(&b[STICK_PARAMS..])) {
        return (Cal { dead, ..c }, 1);
    }
    (Cal { dead, ..Default::default() }, 0)
}

/// A stick in -32767..32767 (y up): each axis over its travel on that side, a round dead zone (the stick's own when
/// plausible, else 10%), scaled back to start at 0.
pub fn apply_cal(c: &Cal, rx: u16, ry: u16) -> (i32, i32) {
    let (sx, sy) = (rx as f32 - c.cx, ry as f32 - c.cy);
    let (tx, ty) = (if sx >= 0.0 { c.maxx } else { c.minx }, if sy >= 0.0 { c.maxy } else { c.miny });
    let (nx, ny) = (if tx > 0.0 { sx / tx } else { 0.0 }, if ty > 0.0 { sy / ty } else { 0.0 });
    let r = (nx * nx + ny * ny).sqrt();
    let travel = (c.maxx + c.minx + c.maxy + c.miny) / 4.0;
    let own = if travel > 0.0 { c.dead / travel } else { 0.0 };
    let dead = if (0.03..=0.25).contains(&own) { own } else { 0.10 };
    if r <= dead {
        return (0, 0);
    }
    let k = (r - dead) / (1.0 - dead) / r;
    ((nx * k).clamp(-1.0, 1.0) as f64 * 32767.0).round() as i32;
    (((nx * k).clamp(-1.0, 1.0) * 32767.0).round() as i32, ((ny * k).clamp(-1.0, 1.0) * 32767.0).round() as i32)
}

/// The device's buttons as NpadButtons (positional = labels on Nintendo pads); `rail` adds SL/SR at 24..27 for a
/// Joy-Con (left 24, 25; right 26, 27).
pub fn buttons(b: u32, joycon: Option<usize>) -> u64 {
    let mut m = 0u64;
    for (raw, bit) in [
        (B_A, 0),
        (B_B, 1),
        (B_X, 2),
        (B_Y, 3),
        (B_LSTICK, 4),
        (B_RSTICK, 5),
        (B_L, 6),
        (B_R, 7),
        (B_ZL, 8),
        (B_ZR, 9),
        (B_PLUS, 10),
        (B_MINUS, 11),
        (B_LEFT, 12),
        (B_UP, 13),
        (B_RIGHT, 14),
        (B_DOWN, 15),
    ] {
        if b & raw != 0 {
            m |= 1 << bit;
        }
    }
    match joycon {
        Some(0) => {
            if b & B_SL_L != 0 {
                m |= 1 << 24;
            }
            if b & B_SR_L != 0 {
                m |= 1 << 25;
            }
        }
        Some(_) => {
            if b & B_SL_R != 0 {
                m |= 1 << 26;
            }
            if b & B_SR_R != 0 {
                m |= 1 << 27;
            }
        }
        None => {}
    }
    m
}

pub const LED_PATTERN: [u8; 8] = [0x01, 0x03, 0x07, 0x0f, 0x09, 0x05, 0x0d, 0x06];

pub fn freq_hz(code: u16) -> f32 {
    110.0 * ((code as f32 - 333.0) / 96.0).exp2()
}
/// Hz to the 10-bit code as the console rounds it.
pub fn freq_code(hz: f32) -> u16 {
    let hz = if hz >= 9.935 { hz.min(16034.0) } else { 9.935 };
    let c = (96.0 * hz.log2() - 318.010_53) as i32;
    if c > 0x3fe {
        return 0x3ff.min(c) as u16;
    }
    let c = c.max(0) as u16;
    if (freq_hz(c + 1) - hz).abs() < (freq_hz(c) - hz).abs() {
        c + 1
    } else {
        c
    }
}
pub fn amp_code(a: f32) -> u16 {
    let a = if a > 0.0 { a.min(1.0) } else { 0.0 };
    let c = (a * 1023.0) as i32;
    if c > 0x3fe {
        return 0x3ff;
    }
    let c = c as u16;
    if ((c + 1) as f32 / 1023.0 - a).abs() < (c as f32 / 1023.0 - a).abs() {
        c + 1
    } else {
        c
    }
}

/// One five-byte sample: low band then high band, each frequency code and amplitude (10 bits each), with the
/// console's total-strength cap.
pub fn sample(v: &super::Vib) -> [u8; 5] {
    let (mut lo, mut hi) = (v.amp_low, v.amp_high);
    let sum = lo + hi;
    let k = if sum > 1.0 { 0.98 / sum } else { 0.98 };
    lo *= k;
    hi *= k;
    let x = freq_code(v.freq_low) as u64 & 0x3ff | (amp_code(lo) as u64 & 0x3ff) << 10 | (freq_code(v.freq_high) as u64 & 0x3ff) << 20 | (amp_code(hi) as u64 & 0x3ff) << 30;
    std::array::from_fn(|i| (x >> (8 * i)) as u8)
}

/// The rumble report: two actuator blocks of three samples (the same value: the game's value at the moment).
pub fn rumble_packet(a: &super::Vib, b: &super::Vib, seq: u8) -> Vec<u8> {
    let mut o = vec![0u8];
    for v in [a, b] {
        o.push(0x40 | 3 << 4 | (seq & 0x0f));
        let s = sample(v);
        for _ in 0..3 {
            o.extend_from_slice(&s);
        }
    }
    o
}

/// One IMU sample in the Switch frame (G, rad / s), the Switch 1 chip's orientation and scale.
pub fn imu_switch(raw: &[i16; 6], right: bool) -> ([f32; 3], [f32; 3]) {
    super::nintendo::imu_switch(raw, if right { 3 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol() {
        assert_eq!(command(CMD_LEDS, SUB_LEDS_SET, &[1, 0, 0, 0]), vec![9, 0x91, 1, 7, 0, 4, 0, 0, 1, 0, 0, 0]);
        let mut adv = vec![0u8; 16];
        adv[3..5].copy_from_slice(&VENDOR.to_le_bytes());
        adv[5..7].copy_from_slice(&PID_JOY_L.to_le_bytes());
        let a = parse_advert(&adv).unwrap();
        assert!(a.pairing);
        assert_eq!(advert_use(&a, None), Use::Take);
        adv[10] = 1;
        let a = parse_advert(&adv).unwrap();
        assert_eq!(advert_use(&a, Some([1, 0, 0, 0, 0, 0])), Use::Wake);
        assert_eq!(advert_use(&a, Some([2, 0, 0, 0, 0, 0])), Use::Ignore);
        // frequency code: 333 at 110 Hz
        assert_eq!(freq_code(110.0), 333);
        assert_eq!(amp_code(1.0), 1023);
        let p = rumble_packet(&super::super::Vib::default(), &super::super::Vib::default(), 5);
        assert_eq!((p.len(), p[1]), (33, 0x75));
    }
    #[test]
    fn report_and_sticks() {
        let mut d = vec![0u8; 63];
        d[4..8].copy_from_slice(&(B_A | B_ZL | B_SL_L).to_le_bytes());
        // left stick centre 2048 / 2048
        d[10] = 0x00;
        d[11] = 0x08;
        d[12] = 0x80;
        let r = parse_report(&d).unwrap();
        assert_eq!((r.lx, r.ly), (2048, 2048));
        assert_eq!(buttons(r.buttons, Some(0)), 1 | 1 << 8 | 1 << 24);
        assert_eq!(apply_cal(&Cal::default(), 2048, 2048), (0, 0));
        let (x, _) = apply_cal(&Cal::default(), 2048 + 1200, 2048);
        assert_eq!(x, 32767);
    }
}
