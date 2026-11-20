//! The WinMM joystick fallback (DirectInput pads XInput and the native HID paths do not cover): joyGetPosEx. Buttons by
//! position: WinMM numbers a PlayStation pad's face buttons square, cross, circle, triangle, and generic DirectInput
//! pads also put the bottom button at 1 and the right one at 2; Sony only swaps the Options / Create order. Sony's
//! DirectInput layout has the right stick on Z / R. POV hats give the D-pad, diagonals both directions. On Linux the same
//! fallback reads SDL2's joysticks (`linux` below): the kernel keeps the HID button order, the right stick is the
//! third and fourth axis (Z / Rz), the first hat is the POV.

#[cfg(windows)]
use windows_sys::Win32::Media::Multimedia::*;

#[cfg(windows)]
const JOY_RETURNALL: u32 = 0xff;
#[cfg(windows)]
const JOY_POVCENTERED: u32 = 0xffff;

#[cfg(windows)]
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
#[cfg(windows)]
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

#[cfg(windows)]
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
#[cfg(windows)]
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

/// A joystick SDL2 opened (Linux): `id` is its SDL instance id, stable while it stays connected.
#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
pub struct Joy {
    pub id: u32,
    pub vid: u16,
    pub pid: u16,
    pub axes: u32,
    pub buttons: u32,
}

#[cfg(target_os = "linux")]
struct Open(Vec<(i32, super::xinput::sdl::Ptr)>);
// SAFETY: the joystick pointers are only used under the mutex
#[cfg(target_os = "linux")]
unsafe impl Send for Open {}
#[cfg(target_os = "linux")]
static OPEN: std::sync::Mutex<Open> = std::sync::Mutex::new(Open(Vec::new()));

/// The first joystick that answers, at or after device index `from` (Linux: SDL2's joysticks).
#[cfg(target_os = "linux")]
pub fn open(from: u32) -> Option<Joy> {
    let s = super::xinput::sdl::get()?;
    let mut o = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: SDL joystick calls serialised by OPEN
    unsafe {
        (s.JoystickUpdate)();
        let n = (s.NumJoysticks)();
        for i in from as i32..n {
            let inst = (s.JoystickGetDeviceInstanceID)(i);
            let j = match o.0.iter().find(|(k, _)| *k == inst) {
                Some(&(_, j)) => j,
                None => {
                    let j = (s.JoystickOpen)(i);
                    if j.is_null() {
                        continue;
                    }
                    o.0.push((inst, j));
                    j
                }
            };
            return Some(Joy {
                id: inst as u32,
                vid: (s.JoystickGetVendor)(j),
                pid: (s.JoystickGetProduct)(j),
                axes: (s.JoystickNumAxes)(j).max(0) as u32,
                buttons: (s.JoystickNumButtons)(j).max(0) as u32,
            });
        }
    }
    None
}

/// SDL's first hat (up 1, right 2, down 4, left 8) as the D-pad bits, diagonals both directions.
#[cfg(target_os = "linux")]
pub fn hat_bits(h: u8) -> u64 {
    let mut b = 0u64;
    for (m, bit) in [(1u8, 13u32), (2, 14), (4, 15), (8, 12)] {
        if h & m != 0 {
            b |= 1 << bit;
        }
    }
    b
}

/// The pad's positional buttons and sticks, or None when it went away (it is closed then).
#[cfg(target_os = "linux")]
pub fn poll(j: &Joy) -> Option<(u64, [i32; 4])> {
    let s = super::xinput::sdl::get()?;
    let mut o = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let at = o.0.iter().position(|(k, _)| *k as u32 == j.id)?;
    let h = o.0[at].1;
    // SAFETY: an open joystick, under OPEN
    unsafe {
        (s.JoystickUpdate)();
        if (s.JoystickGetAttached)(h) == 0 {
            (s.JoystickClose)(h);
            o.0.remove(at);
            return None;
        }
        let nb = (s.JoystickNumButtons)(h).clamp(0, 32);
        let mask = (0..nb).filter(|&k| (s.JoystickGetButton)(h, k) != 0).fold(0u32, |m, k| m | 1 << k);
        let mut b = buttons(mask, j.vid == 0x054c);
        if (s.JoystickNumHats)(h) > 0 {
            b |= hat_bits((s.JoystickGetHat)(h, 0));
        }
        let na = (s.JoystickNumAxes)(h);
        let ax = |k: i32, invert: bool| -> i32 {
            if k >= na {
                return 0;
            }
            let v = (s.JoystickGetAxis)(h, k) as i32;
            super::xinput::dz(if invert { (-v).min(32767) } else { v.max(-32767) })
        };
        Some((b, [ax(0, false), ax(1, true), ax(2, false), ax(3, true)]))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn hat_is_the_dpad() {
        assert_eq!(super::hat_bits(0), 0);
        assert_eq!(super::hat_bits(1 | 2), 1 << 13 | 1 << 14, "up-right: both");
        assert_eq!(super::hat_bits(8), 1 << 12);
    }
}
