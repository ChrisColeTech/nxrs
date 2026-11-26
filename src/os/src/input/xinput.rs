//! XInput pads (Xbox controllers and anything a driver presents as one), loaded at run time from xinput1_4 /
//! xinput1_3 / xinput9_1_0; on Linux the same surface over SDL2's game controllers (`linux` below). Buttons by position like a Switch pad: right (Xbox B) = A position, bottom (Xbox A) = B,
//! top (Y) = X, left (X) = Y; the triggers press ZL / ZR past 60 of 255. Rumble through XInputSetState (left = low
//! frequency motor, right = high).

#[cfg(windows)]
use std::sync::OnceLock;
#[cfg(windows)]
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
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct State {
    packet: u32,
    pad: Gamepad,
}
#[cfg(windows)]
#[repr(C)]
struct Vibration {
    left: u16,
    right: u16,
}

#[cfg(windows)]
type GetState = unsafe extern "system" fn(u32, *mut State) -> u32;
#[cfg(windows)]
type SetState = unsafe extern "system" fn(u32, *mut Vibration) -> u32;

#[cfg(windows)]
struct Api {
    get: GetState,
    set: Option<SetState>,
    dll: &'static str,
}

#[cfg(windows)]
fn api() -> Option<&'static Api> {
    static A: OnceLock<Option<Api>> = OnceLock::new();
    A.get_or_init(|| unsafe {
        for (dll, z) in [
            ("xinput1_4.dll", "xinput1_4.dll\0"),
            ("xinput1_3.dll", "xinput1_3.dll\0"),
            ("xinput9_1_0.dll", "xinput9_1_0.dll\0"),
        ] {
            let h = LoadLibraryA(z.as_ptr());
            if h.is_null() {
                continue;
            }
            let Some(get) = GetProcAddress(h, c"XInputGetState".as_ptr() as *const u8) else {
                continue;
            };
            let set = GetProcAddress(h, c"XInputSetState".as_ptr() as *const u8);
            return Some(Api {
                get: std::mem::transmute::<_, GetState>(get),
                set: set.map(|f| std::mem::transmute::<_, SetState>(f)),
                dll,
            });
        }
        None
    })
    .as_ref()
}

#[cfg(windows)]
pub fn dll() -> Option<&'static str> {
    api().map(|a| a.dll)
}

#[cfg(windows)]
/// User `i`'s pad (0..3), or None when not connected.
pub fn state(i: u32) -> Option<Gamepad> {
    let a = api()?;
    let mut s = State::default();
    (unsafe { (a.get)(i, &mut s) } == 0).then_some(s.pad)
}

#[cfg(windows)]
/// Sets user `i`'s motors (0..65535); returns the XInput status (0 = done).
pub fn rumble(i: u32, low: u16, high: u16) -> u32 {
    let Some(set) = api().and_then(|a| a.set) else {
        return 1;
    };
    let mut v = Vibration {
        left: low,
        right: high,
    };
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

/// The dead zone: ±7000 reads 0.
pub fn dz(v: i32) -> i32 {
    if v > -7000 && v < 7000 {
        0
    } else {
        v
    }
}

// ---------------------------------------------------------------- Linux: SDL2 game controllers

/// SDL2, loaded at run time (libSDL2-2.0.so.0, else libSDL2.so), initialised once
/// for game controllers (and with them joysticks: `winmm.rs`). SDL's own HID drivers for PlayStation and Nintendo
/// pads are turned off, so those stay with the native HID path (`hidio`) unless NXRS_NO_SONY_HID /
/// NXRS_NO_NINTENDO_HID hands them to SDL. SDL installs no signal handlers and queues no events: everything is
/// polled from the input thread.
#[cfg(target_os = "linux")]
#[allow(non_snake_case)]
pub(crate) mod sdl {
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::sync::OnceLock;

    pub type Ptr = *mut c_void;

    pub struct Sdl {
        pub lib: &'static str,
        pub NumJoysticks: unsafe extern "C" fn() -> c_int,
        pub IsGameController: unsafe extern "C" fn(c_int) -> c_int,
        pub JoystickGetDeviceInstanceID: unsafe extern "C" fn(c_int) -> i32,
        pub JoystickGetDeviceVendor: unsafe extern "C" fn(c_int) -> u16,
        pub GameControllerOpen: unsafe extern "C" fn(c_int) -> Ptr,
        pub GameControllerGetAttached: unsafe extern "C" fn(Ptr) -> c_int,
        pub GameControllerClose: unsafe extern "C" fn(Ptr),
        pub GameControllerUpdate: unsafe extern "C" fn(),
        pub GameControllerGetButton: unsafe extern "C" fn(Ptr, c_int) -> u8,
        pub GameControllerGetAxis: unsafe extern "C" fn(Ptr, c_int) -> i16,
        pub GameControllerName: unsafe extern "C" fn(Ptr) -> *const c_char,
        pub GameControllerRumble: Option<unsafe extern "C" fn(Ptr, u16, u16, u32) -> c_int>,
        pub JoystickOpen: unsafe extern "C" fn(c_int) -> Ptr,
        pub JoystickClose: unsafe extern "C" fn(Ptr),
        pub JoystickGetAttached: unsafe extern "C" fn(Ptr) -> c_int,
        pub JoystickUpdate: unsafe extern "C" fn(),
        pub JoystickNumAxes: unsafe extern "C" fn(Ptr) -> c_int,
        pub JoystickNumButtons: unsafe extern "C" fn(Ptr) -> c_int,
        pub JoystickGetAxis: unsafe extern "C" fn(Ptr, c_int) -> i16,
        pub JoystickGetButton: unsafe extern "C" fn(Ptr, c_int) -> u8,
        pub JoystickNumHats: unsafe extern "C" fn(Ptr) -> c_int,
        pub JoystickGetHat: unsafe extern "C" fn(Ptr, c_int) -> u8,
        pub JoystickGetVendor: unsafe extern "C" fn(Ptr) -> u16,
        pub JoystickGetProduct: unsafe extern "C" fn(Ptr) -> u16,
    }
    // SAFETY: function pointers into a library that stays loaded; callers serialise on their own locks
    unsafe impl Sync for Sdl {}
    unsafe impl Send for Sdl {}

    unsafe fn sym<T: Copy>(h: *mut c_void, name: &CStr) -> Option<T> {
        let p = libc::dlsym(h, name.as_ptr());
        (!p.is_null()).then(|| core::mem::transmute_copy::<*mut c_void, T>(&p))
    }

    /// SDL2, loaded and initialised once (None: not installed or its init failed, said once in the log).
    pub fn get() -> Option<&'static Sdl> {
        static S: OnceLock<Option<Sdl>> = OnceLock::new();
        S.get_or_init(|| unsafe { load() }).as_ref()
    }

    unsafe fn load() -> Option<Sdl> {
        let mut lib = "libSDL2-2.0.so.0";
        let mut h = libc::dlopen(
            c"libSDL2-2.0.so.0".as_ptr(),
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        );
        if h.is_null() {
            lib = "libSDL2.so";
            h = libc::dlopen(c"libSDL2.so".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        }
        if h.is_null() {
            super::super::ilog!("SDL2 not found (install libsdl2-2.0-0): no XInput-style pads or joysticks, native HID pads and the keyboard only");
            return None;
        }
        macro_rules! need {
            ($n:literal) => {
                match sym(h, $n) {
                    Some(f) => f,
                    None => {
                        super::super::ilog!(
                            "SDL2 ({lib}) lacks {:?}: too old (2.0.9 or newer is needed)",
                            $n
                        );
                        return None;
                    }
                }
            };
        }
        let init: unsafe extern "C" fn(u32) -> c_int = need!(c"SDL_Init");
        let hint: unsafe extern "C" fn(*const c_char, *const c_char) -> c_int =
            need!(c"SDL_SetHint");
        let gc_events: unsafe extern "C" fn(c_int) -> c_int =
            need!(c"SDL_GameControllerEventState");
        let joy_events: unsafe extern "C" fn(c_int) -> c_int = need!(c"SDL_JoystickEventState");
        let err: unsafe extern "C" fn() -> *const c_char = need!(c"SDL_GetError");
        let s = Sdl {
            lib,
            NumJoysticks: need!(c"SDL_NumJoysticks"),
            IsGameController: need!(c"SDL_IsGameController"),
            JoystickGetDeviceInstanceID: need!(c"SDL_JoystickGetDeviceInstanceID"),
            JoystickGetDeviceVendor: need!(c"SDL_JoystickGetDeviceVendor"),
            GameControllerOpen: need!(c"SDL_GameControllerOpen"),
            GameControllerGetAttached: need!(c"SDL_GameControllerGetAttached"),
            GameControllerClose: need!(c"SDL_GameControllerClose"),
            GameControllerUpdate: need!(c"SDL_GameControllerUpdate"),
            GameControllerGetButton: need!(c"SDL_GameControllerGetButton"),
            GameControllerGetAxis: need!(c"SDL_GameControllerGetAxis"),
            GameControllerName: need!(c"SDL_GameControllerName"),
            GameControllerRumble: sym(h, c"SDL_GameControllerRumble"),
            JoystickOpen: need!(c"SDL_JoystickOpen"),
            JoystickClose: need!(c"SDL_JoystickClose"),
            JoystickGetAttached: need!(c"SDL_JoystickGetAttached"),
            JoystickUpdate: need!(c"SDL_JoystickUpdate"),
            JoystickNumAxes: need!(c"SDL_JoystickNumAxes"),
            JoystickNumButtons: need!(c"SDL_JoystickNumButtons"),
            JoystickGetAxis: need!(c"SDL_JoystickGetAxis"),
            JoystickGetButton: need!(c"SDL_JoystickGetButton"),
            JoystickNumHats: need!(c"SDL_JoystickNumHats"),
            JoystickGetHat: need!(c"SDL_JoystickGetHat"),
            JoystickGetVendor: need!(c"SDL_JoystickGetVendor"),
            JoystickGetProduct: need!(c"SDL_JoystickGetProduct"),
        };
        let set = |k: &CStr, v: &CStr| {
            hint(k.as_ptr(), v.as_ptr());
        };
        // buttons by position like on Windows, also for Nintendo pads that SDL would map by label
        set(c"SDL_GAMECONTROLLER_USE_BUTTON_LABELS", c"0");
        set(c"SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", c"1");
        // the process's own SIGINT / SIGTERM handling stays the engine's
        set(c"SDL_NO_SIGNAL_HANDLERS", c"1");
        if !super::super::env_flag("NXRS_NO_SONY_HID") {
            set(c"SDL_JOYSTICK_HIDAPI_PS4", c"0");
            set(c"SDL_JOYSTICK_HIDAPI_PS5", c"0");
        }
        if !super::super::env_flag("NXRS_NO_NINTENDO_HID") {
            set(c"SDL_JOYSTICK_HIDAPI_SWITCH", c"0");
            set(c"SDL_JOYSTICK_HIDAPI_JOY_CONS", c"0");
            set(c"SDL_JOYSTICK_HIDAPI_NINTENDO_CLASSIC", c"0");
        }
        // SDL_INIT_GAMECONTROLLER (with it joysticks and events)
        if init(0x2000) != 0 {
            let e = CStr::from_ptr(err()).to_string_lossy();
            super::super::ilog!("SDL2 ({lib}) game controllers did not start: {e}");
            return None;
        }
        // polled state only: nothing piles up in SDL's event queue (SDL_IGNORE)
        gc_events(0);
        joy_events(0);
        super::super::ilog!("SDL2 ({lib}) game controllers and joysticks ready");
        Some(s)
    }

    /// The Sony / Nintendo vendor ids the native HID path keeps (unless turned off).
    pub fn native(vendor: u16) -> bool {
        (vendor == 0x054c && !super::super::env_flag("NXRS_NO_SONY_HID"))
            || (vendor == 0x057e && !super::super::env_flag("NXRS_NO_NINTENDO_HID"))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::sdl::{self, Ptr};
    use super::Gamepad;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// One open game controller in an XInput-style user slot (stable while it stays connected, as XInput's are).
    struct Slot {
        gc: Ptr,
        inst: i32,
    }
    struct Pads {
        slots: [Option<Slot>; 4],
        updated: Option<Instant>,
        scanned: Option<Instant>,
        count: i32,
    }
    // SAFETY: the controller pointers are only used under the mutex
    unsafe impl Send for Pads {}
    static PADS: Mutex<Pads> = Mutex::new(Pads {
        slots: [None, None, None, None],
        updated: None,
        scanned: None,
        count: -1,
    });

    /// SDL's state, read at most every 2 ms; detached controllers closed, new ones opened into free slots (a count
    /// change, or once a second).
    fn refresh(s: &sdl::Sdl, p: &mut Pads) {
        let now = Instant::now();
        if p.updated
            .is_some_and(|t| now - t < Duration::from_millis(2))
        {
            return;
        }
        p.updated = Some(now);
        // SAFETY: SDL calls on the controllers we opened, serialised by PADS
        unsafe {
            (s.GameControllerUpdate)();
            for slot in p.slots.iter_mut() {
                if let Some(sl) = slot {
                    if (s.GameControllerGetAttached)(sl.gc) == 0 {
                        (s.GameControllerClose)(sl.gc);
                        *slot = None;
                    }
                }
            }
            let n = (s.NumJoysticks)();
            if n == p.count && p.scanned.is_some_and(|t| now - t < Duration::from_secs(1)) {
                return;
            }
            p.count = n;
            p.scanned = Some(now);
            for i in 0..n {
                if (s.IsGameController)(i) == 0 || sdl::native((s.JoystickGetDeviceVendor)(i)) {
                    continue;
                }
                let inst = (s.JoystickGetDeviceInstanceID)(i);
                if p.slots.iter().flatten().any(|sl| sl.inst == inst) {
                    continue;
                }
                let Some(free) = p.slots.iter().position(|sl| sl.is_none()) else {
                    break;
                };
                let gc = (s.GameControllerOpen)(i);
                if gc.is_null() {
                    continue;
                }
                p.slots[free] = Some(Slot { gc, inst });
                let name = (s.GameControllerName)(gc);
                let name = if name.is_null() {
                    "game controller".into()
                } else {
                    std::ffi::CStr::from_ptr(name)
                        .to_string_lossy()
                        .into_owned()
                };
                super::super::ilog!("SDL2 game controller {name:?} in XInput user {}", free + 1);
            }
        }
    }

    /// XInput's button bits from SDL's buttons (SDL names them by Xbox position: A bottom, B right, X left, Y top).
    pub fn xinput_buttons(down: impl Fn(i32) -> bool) -> u16 {
        // (SDL button, XInput bit): A B X Y, Back, Start, stick clicks, shoulders, D-pad up down left right
        const MAP: [(i32, u16); 14] = [
            (0, 0x1000),
            (1, 0x2000),
            (2, 0x4000),
            (3, 0x8000),
            (4, 0x0020),
            (6, 0x0010),
            (7, 0x0040),
            (8, 0x0080),
            (9, 0x0100),
            (10, 0x0200),
            (11, 0x0001),
            (12, 0x0002),
            (13, 0x0004),
            (14, 0x0008),
        ];
        MAP.iter()
            .filter(|(b, _)| down(*b))
            .fold(0, |m, (_, x)| m | x)
    }

    /// SDL's Y axes point down, XInput's (and the Switch's) up.
    pub fn flip(y: i16) -> i16 {
        (-(y as i32)).min(32767) as i16
    }

    /// An SDL trigger (0..32767) as XInput's 0..255.
    pub fn trigger(v: i16) -> u8 {
        (v.max(0) as i32 * 255 / 32767) as u8
    }

    pub fn dll() -> Option<&'static str> {
        sdl::get().map(|s| s.lib)
    }

    pub fn state(i: u32) -> Option<Gamepad> {
        let s = sdl::get()?;
        let mut p = PADS.lock().unwrap_or_else(|e| e.into_inner());
        refresh(s, &mut p);
        let gc = p.slots.get(i as usize)?.as_ref()?.gc;
        // SAFETY: an open controller, under PADS
        unsafe {
            let ax = |a: i32| (s.GameControllerGetAxis)(gc, a);
            Some(Gamepad {
                buttons: xinput_buttons(|b| (s.GameControllerGetButton)(gc, b) != 0),
                lt: trigger(ax(4)),
                rt: trigger(ax(5)),
                lx: ax(0),
                ly: flip(ax(1)),
                rx: ax(2),
                ry: flip(ax(3)),
            })
        }
    }

    pub fn rumble(i: u32, low: u16, high: u16) -> u32 {
        let Some(s) = sdl::get() else { return 1 };
        let Some(f) = s.GameControllerRumble else {
            return 1;
        };
        let p = PADS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Some(sl)) = p.slots.get(i as usize) else {
            return 1;
        };
        // SAFETY: an open controller, under PADS. XInput holds a level until the next call: SDL its longest duration
        (unsafe { f(sl.gc, low, high, 0xFFFF) } != 0) as u32
    }
}

/// The library the pads come through (SDL2's file name on Linux).
#[cfg(target_os = "linux")]
pub fn dll() -> Option<&'static str> {
    linux::dll()
}

/// User `i`'s pad (0..3), or None when not connected: an SDL2 game controller in that slot, XInput-shaped.
#[cfg(target_os = "linux")]
pub fn state(i: u32) -> Option<Gamepad> {
    linux::state(i)
}

/// Sets user `i`'s motors (0..65535); 0 when done.
#[cfg(target_os = "linux")]
pub fn rumble(i: u32, low: u16, high: u16) -> u32 {
    linux::rumble(i, low, high)
}

// The web: the browser's gamepads reach the game through the page's input block (docs/35 W3), not this API; no pad
// is connected here.
#[cfg(target_os = "emscripten")]
pub fn dll() -> Option<&'static str> {
    None
}
#[cfg(target_os = "emscripten")]
pub fn state(_i: u32) -> Option<Gamepad> {
    None
}
/// ERROR_DEVICE_NOT_CONNECTED, as XInput answers for an empty slot.
#[cfg(target_os = "emscripten")]
pub fn rumble(_i: u32, _low: u16, _high: u16) -> u32 {
    1167
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::linux::{flip, trigger, xinput_buttons};

    #[test]
    fn sdl_to_xinput_shape() {
        // SDL A (bottom) and D-pad up: XInput A and DPAD_UP, then positionally B (bottom) and up
        let b = xinput_buttons(|x| x == 0 || x == 11);
        assert_eq!(b, 0x1000 | 0x0001);
        let g = super::Gamepad {
            buttons: b,
            lt: trigger(32767),
            ..Default::default()
        };
        assert_eq!(super::buttons(&g), 1 << 1 | 1 << 13 | 1 << 8);
        // SDL B (right) is the Switch A position
        assert_eq!(
            super::buttons(&super::Gamepad {
                buttons: xinput_buttons(|x| x == 1),
                ..Default::default()
            }),
            1
        );
        assert_eq!(flip(-32768), 32767, "SDL up is XInput up, clamped");
        assert_eq!(flip(1000), -1000);
        assert_eq!((trigger(0), trigger(-5), trigger(16384)), (0, 0, 127));
        assert!(
            trigger(7700) < 60 && trigger(7900) > 60,
            "a 7700 threshold is XInput's 60"
        );
    }

    /// SDL loads and starts here (libsdl2 installed) and sees no controller in a plain WSL.
    #[test]
    fn sdl_loads_and_polls() {
        if super::sdl::get().is_some() {
            for i in 0..4 {
                let _ = super::state(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn positional() {
        let g = super::Gamepad {
            buttons: 0x1000 | 0x0001,
            lt: 200,
            ..Default::default()
        };
        assert_eq!(
            super::buttons(&g),
            1 << 1 | 1 << 13 | 1 << 8,
            "Xbox A is the bottom (B) position"
        );
    }
}
