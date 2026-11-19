//! The WinMM joystick fallback (DirectInput pads XInput and the native HID paths do not cover): joyGetPosEx. Buttons by
//! position: WinMM numbers a PlayStation pad's face buttons square, cross, circle, triangle, and generic DirectInput
//! pads also put the bottom button at 1 and the right one at 2; Sony only swaps the Options / Create order. Sony's
//! DirectInput layout has the right stick on Z / R. POV hats give the D-pad, diagonals both directions.

use windows_sys::Win32::Media::Multimedia::*;

const JOY_RETURNALL: u32 = 0xff;
const JOY_POVCENTERED: u32 = 0xffff;

#[derive(Clone, Default)]
pub struct Joy {
    pub id: u32,
    pub vid: u16,
    pub pid: u16,
    pub axes: u32,
    pub buttons: u32,
    caps: (u32, u32, u32, u32, u32, u32, u32, u32),
}

/// The first joystick that answers, at or after `from`.
pub fn open(from: u32) -> Option<Joy> {
    unsafe {
        let n = joyGetNumDevs();
        for id in from..n {
            let mut st: JOYINFOEX = std::mem::zeroed();
            st.dwSize = std::mem::size_of::<JOYINFOEX>() as u32;
            st.dwFlags = JOY_RETURNALL;
            if joyGetPosEx(id, &mut st) != JOYERR_NOERROR {
                continue;
            }
            let mut c: JOYCAPSW = std::mem::zeroed();
            if joyGetDevCapsW(id as usize, &mut c, std::mem::size_of::<JOYCAPSW>() as u32) != JOYERR_NOERROR {
                continue;
            }
            return Some(Joy {
                id,
                vid: c.wMid,
                pid: c.wPid,
                axes: c.wNumAxes,
                buttons: c.wNumButtons,
                caps: (c.wXmin, c.wXmax, c.wYmin, c.wYmax, c.wZmin, c.wZmax, c.wRmin, c.wRmax),
            });
        }
    }
    None
}

fn axis(v: u32, min: u32, max: u32, invert: bool) -> i32 {
    if max <= min {
        return 0;
    }
    let mid = (min as i64 + max as i64) / 2;
    let span = (max as i64 - min as i64) / 2;
    let o = ((v as i64 - mid) * 32767 / span.max(1)).clamp(-32767, 32767) as i32;
    if invert {
        -o
    } else {
        o
    }
}

pub fn buttons(x: u32, sony: bool) -> u64 {
    let mut b = 0u64;
    let mut m = |c: bool, bit: u32| {
        if c {
            b |= 1 << bit;
        }
    };
    m(x & 1 << 2 != 0, 0);
    m(x & 1 << 1 != 0, 1);
    m(x & 1 << 3 != 0, 2);
    m(x & 1 != 0, 3);
    m(x & 1 << 10 != 0, 4);
    m(x & 1 << 11 != 0, 5);
    m(x & 1 << 4 != 0, 6);
    m(x & 1 << 5 != 0, 7);
    m(x & 1 << 6 != 0, 8);
    m(x & 1 << 7 != 0, 9);
    m(x & 1 << if sony { 9 } else { 8 } != 0, 10);
    m(x & 1 << if sony { 8 } else { 9 } != 0, 11);
    b
}

/// The pad's positional buttons and sticks, or None when it went away.
pub fn poll(j: &Joy) -> Option<(u64, [i32; 4])> {
    unsafe {
        let mut st: JOYINFOEX = std::mem::zeroed();
        st.dwSize = std::mem::size_of::<JOYINFOEX>() as u32;
        st.dwFlags = JOY_RETURNALL;
        if joyGetPosEx(j.id, &mut st) != JOYERR_NOERROR {
            return None;
        }
        let mut b = buttons(st.dwButtons, j.vid == 0x054c);
        if st.dwPOV != JOY_POVCENTERED {
            let p = st.dwPOV;
            if p >= 31500 || p <= 4500 {
                b |= 1 << 13;
            }
            if (4500..=13500).contains(&p) {
                b |= 1 << 14;
            }
            if (13500..=22500).contains(&p) {
                b |= 1 << 15;
            }
            if (22500..=31500).contains(&p) {
                b |= 1 << 12;
            }
        }
        let c = j.caps;
        let dz = super::xinput::dz;
        Some((
            b,
            [
                dz(axis(st.dwXpos, c.0, c.1, false)),
                dz(axis(st.dwYpos, c.2, c.3, true)),
                dz(axis(st.dwZpos, c.4, c.5, false)),
                dz(axis(st.dwRpos, c.6, c.7, true)),
            ],
        ))
    }
}
