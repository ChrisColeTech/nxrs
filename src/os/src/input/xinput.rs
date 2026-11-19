//! XInput pads (Xbox controllers and anything a driver presents as one), loaded at run time from xinput1_4 /
//! xinput1_3 / xinput9_1_0. Buttons by position like a Switch pad: right (Xbox B) = A position, bottom (Xbox A) = B,
//! top (Y) = X, left (X) = Y; the triggers press ZL / ZR past 60 of 255. Rumble through XInputSetState (left = low
//! frequency motor, right = high).

use std::sync::OnceLock;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Gamepad {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct State {
    packet: u32,
    pad: Gamepad,
}
#[repr(C)]
struct Vibration {
    left: u16,
    right: u16,
}

type GetState = unsafe extern "system" fn(u32, *mut State) -> u32;
type SetState = unsafe extern "system" fn(u32, *mut Vibration) -> u32;

struct Api {
    get: GetState,
    set: Option<SetState>,
    dll: &'static str,
}

fn api() -> Option<&'static Api> {
    static A: OnceLock<Option<Api>> = OnceLock::new();
    A.get_or_init(|| unsafe {
        for (dll, z) in [("xinput1_4.dll", "xinput1_4.dll\0"), ("xinput1_3.dll", "xinput1_3.dll\0"), ("xinput9_1_0.dll", "xinput9_1_0.dll\0")] {
            let h = LoadLibraryA(z.as_ptr());
            if h.is_null() {
                continue;
            }
            let Some(get) = GetProcAddress(h, c"XInputGetState".as_ptr() as *const u8) else { continue };
            let set = GetProcAddress(h, c"XInputSetState".as_ptr() as *const u8);
            return Some(Api { get: std::mem::transmute::<_, GetState>(get), set: set.map(|f| std::mem::transmute::<_, SetState>(f)), dll });
        }
        None
    })
    .as_ref()
}

pub fn dll() -> Option<&'static str> {
    api().map(|a| a.dll)
}

/// User `i`'s pad (0..3), or None when not connected.
pub fn state(i: u32) -> Option<Gamepad> {
    let a = api()?;
    let mut s = State::default();
    (unsafe { (a.get)(i, &mut s) } == 0).then_some(s.pad)
}

/// Sets user `i`'s motors (0..65535); returns the XInput status (0 = done).
pub fn rumble(i: u32, low: u16, high: u16) -> u32 {
    let Some(set) = api().and_then(|a| a.set) else { return 1 };
    let mut v = Vibration { left: low, right: high };
    unsafe { set(i, &mut v) }
}

/// The positional mask (layout.rs order).
pub fn buttons(g: &Gamepad) -> u64 {
    let x = g.buttons;
    let mut b = 0u64;
    for (mask, bit) in [
        (0x2000u16, 0u32),
        (0x1000, 1),
        (0x8000, 2),
        (0x4000, 3),
        (0x0040, 4),
        (0x0080, 5),
        (0x0100, 6),
        (0x0200, 7),
        (0x0010, 10),
        (0x0020, 11),
        (0x0004, 12),
        (0x0001, 13),
        (0x0008, 14),
        (0x0002, 15),
    ] {
        if x & mask != 0 {
            b |= 1 << bit;
        }
    }
    if g.lt > 60 {
        b |= 1 << 8;
    }
    if g.rt > 60 {
        b |= 1 << 9;
    }
    b
}

/// Turbo's dead zone: ±7000 reads 0.
pub fn dz(v: i32) -> i32 {
    if v > -7000 && v < 7000 {
        0
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn positional() {
        let g = super::Gamepad { buttons: 0x1000 | 0x0001, lt: 200, ..Default::default() };
        assert_eq!(super::buttons(&g), 1 << 1 | 1 << 13 | 1 << 8, "Xbox A is the bottom (B) position");
    }
}
