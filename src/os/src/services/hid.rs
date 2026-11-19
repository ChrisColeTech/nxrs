//! nn::hid: one player's controller, taken from the host's input state (`set_input`, for the front end), as a Pro
//! Controller on npad No1 docked or the handheld pair on npad Handheld in handheld mode (`NXOS_OPMODE`). Other
//! npads report nothing connected. Touch, gestures, keyboard, mouse and the debug pad read empty; six-axis
//! sensors report a controller at rest; vibration is accepted and dropped. State reads fill every requested
//! history entry with consecutive sampling numbers, as nxnative found games need.

use crate::guest::{r32, w32, w64, w8, Args};
use crate::hle::{Registry, RES_OK};
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

pub const NPAD_NO1: u32 = 0;
pub const NPAD_HANDHELD: u32 = 0x20;
pub const STYLE_FULLKEY: u32 = 1;
pub const STYLE_HANDHELD: u32 = 2;
pub const STYLE_JOYDUAL: u32 = 4;
const ATTR_CONNECTED: u32 = 1;
const ATTR_WIRED: u32 = 2;
const ATTR_LEFT: u32 = 4;
const ATTR_LEFT_WIRED: u32 = 8;
const ATTR_RIGHT: u32 = 0x10;
const ATTR_RIGHT_WIRED: u32 = 0x20;
const STICK: i32 = 0x4000;

static SAMPLING: AtomicU64 = AtomicU64::new(0);
static STYLES: AtomicU32 = AtomicU32::new(0xFFFF_FFFF);
static IDS: AtomicU32 = AtomicU32::new(0);
static HOLD: AtomicU32 = AtomicU32::new(0);
/// The host's input: buttons (NpadButton bits) and the sticks (lx, ly, rx, ry as i16).
static BUTTONS: AtomicU64 = AtomicU64::new(0);
static AXES: AtomicU64 = AtomicU64::new(0);
static STYLE_EVENTS: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());

/// The front end's input for player 1.
pub fn set_input(buttons: u64, lx: i16, ly: i16, rx: i16, ry: i16) {
    BUTTONS.store(buttons, Ordering::Relaxed);
    AXES.store(lx as u16 as u64 | (ly as u16 as u64) << 16 | (rx as u16 as u64) << 32 | (ry as u16 as u64) << 48, Ordering::Relaxed);
}

/// The npad the player's controller is on.
pub fn connected_id() -> u32 {
    if crate::services::oe::opmode() == 0 {
        NPAD_HANDHELD
    } else {
        NPAD_NO1
    }
}

/// The style set npad `id` reports.
pub fn style_of(id: u32) -> u32 {
    if id != connected_id() {
        return 0;
    }
    let supported = STYLES.load(Ordering::Relaxed);
    if id == NPAD_HANDHELD {
        return STYLE_HANDHELD & supported;
    }
    if supported & STYLE_FULLKEY != 0 {
        STYLE_FULLKEY
    } else if supported & STYLE_JOYDUAL != 0 {
        STYLE_JOYDUAL
    } else {
        0
    }
}

/// One NpadState (0x28 bytes: sampling, buttons, left and right stick, attributes).
fn fill(out: u64, id: u32, style: u32, sampling: u64) {
    crate::guest::zero(out, 0x28);
    w64(out, sampling);
    if style_of(id) & style == 0 {
        return;
    }
    let v = AXES.load(Ordering::Relaxed);
    let ax = |i: u32| (v >> (16 * i)) as i16 as i32;
    let (lx, ly, rx, ry) = (ax(0), ax(1), ax(2), ax(3));
    let mut b = BUTTONS.load(Ordering::Relaxed) & 0xFFFF;
    for (cond, bit) in [(lx < -STICK, 16), (ly > STICK, 17), (lx > STICK, 18), (ly < -STICK, 19), (rx < -STICK, 20), (ry > STICK, 21), (rx > STICK, 22), (ry < -STICK, 23)] {
        if cond {
            b |= 1 << bit;
        }
    }
    w64(out + 8, b);
    w32(out + 0x10, lx as u32);
    w32(out + 0x14, ly as u32);
    w32(out + 0x18, rx as u32);
    w32(out + 0x1C, ry as u32);
    let attr = if id == NPAD_HANDHELD {
        ATTR_CONNECTED | ATTR_WIRED | ATTR_LEFT | ATTR_LEFT_WIRED | ATTR_RIGHT | ATTR_RIGHT_WIRED
    } else {
        ATTR_CONNECTED | ATTR_LEFT | ATTR_RIGHT
    };
    w32(out + 0x20, attr);
}

fn states(out: u64, n: i32, id: u32, style: u32) -> u64 {
    if n <= 0 {
        return 0;
    }
    let s = SAMPLING.fetch_add(1, Ordering::Relaxed) + 1 + n as u64;
    for i in 0..n as u64 {
        fill(out + 0x28 * i, id, style, s - i);
    }
    n as u64
}

macro_rules! hidfn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

macro_rules! style_fns {
    ($one:ident, $many:ident, $style:expr) => {
        hidfn!($one, |a| {
            let s = SAMPLING.fetch_add(1, Ordering::Relaxed) + 1;
            fill(a.x(0), r32(a.x(1)), $style, s);
            0
        });
        hidfn!($many, |a| { states(a.x(0), a.i(1), r32(a.x(2)), $style) });
    };
}
style_fns!(state_fullkey, states_fullkey, STYLE_FULLKEY);
style_fns!(state_handheld, states_handheld, STYLE_HANDHELD);
style_fns!(state_joydual, states_joydual, STYLE_JOYDUAL);
style_fns!(state_joyleft, states_joyleft, 8);
style_fns!(state_joyright, states_joyright, 0x10);

hidfn!(nothing, |a| { 0 });
hidfn!(ok, |a| { RES_OK });
hidfn!(set_ids, |a| {
    let (ids, n) = (a.x(0), a.x(1));
    let mut m = 0u32;
    for i in 0..n.min(16) {
        let id = r32(ids + 4 * i);
        m |= if id < 8 { 1 << id } else if id == NPAD_HANDHELD { 1 << 8 } else { 1 << 9 };
    }
    if IDS.swap(m, Ordering::Relaxed) != m {
        crate::log!("hid: supported npad ids {m:#x}");
    }
    0
});
hidfn!(set_styles, |a| {
    let s = a.w(0);
    if STYLES.swap(s, Ordering::Relaxed) != s {
        crate::log!("hid: supported npad styles {s:#x}; player 1 reports {:#x} on npad {:#x}", style_of(connected_id()), connected_id());
    }
    0
});
hidfn!(get_styles, |a| { STYLES.load(Ordering::Relaxed) as u64 });
hidfn!(get_style, |a| { style_of(r32(a.x(0))) as u64 });
hidfn!(hold_type_set, |a| {
    HOLD.store(a.w(0), Ordering::Relaxed);
    0
});
hidfn!(hold_type, |a| { HOLD.load(Ordering::Relaxed) as u64 });
hidfn!(zero, |a| { 0 });
hidfn!(one, |a| { 1 });
hidfn!(led, |a| {
    let id = r32(a.x(0));
    (match id {
        0 => 0b0001,
        1 => 0b0011,
        2 => 0b0111,
        3 => 0b1111,
        4 => 0b1001,
        5 => 0b0101,
        6 => 0b1101,
        7 => 0b0110,
        _ => 0,
    }) as u64
});
hidfn!(color, |a| {
    w32(a.x(0), 0xFF32_3232);
    w32(a.x(0) + 4, 0xFFFF_FFFF);
    RES_OK
});
hidfn!(color2, |a| {
    for o in [a.x(0), a.x(1)] {
        w32(o, 0xFF32_3232);
        w32(o + 4, 0xFFFF_FFFF);
    }
    RES_OK
});
hidfn!(controller_support, |a| {
    // (ControllerSupportResultInfo* out, const ControllerSupportArg&): one player, on the connected npad
    let out = a.x(0);
    if out != 0 {
        w8(out, 1);
        w32(out + 4, connected_id());
    }
    crate::log_once!("ctrl applet", "hid: controller applet answered with one player on npad {:#x}", connected_id());
    RES_OK
});
unsafe fn bind_style_event(c: *mut Cpu) {
    let (id, ev, mode) = (r32((*c).x[0]), (*c).x[1], (*c).x[2] as u32);
    crate::services::os::bind_system_event(c, ev, mode == 1);
    STYLE_EVENTS.lock().unwrap().push((id, ev));
    if style_of(id) != 0 {
        crate::services::os::signal_bound(ev);
    }
}
hidfn!(six_handles, |a| {
    // (handles*, n, const NpadId&, style): one sensor, two for a handheld or dual pair
    let (h, n, id, style) = (a.x(0), a.i(1), r32(a.x(2)), a.w(3));
    if n <= 0 {
        return 0;
    }
    let idx = match style {
        s if s & STYLE_FULLKEY != 0 => 3,
        s if s & STYLE_HANDHELD != 0 => 4,
        s if s & STYLE_JOYDUAL != 0 => 5,
        _ => 3,
    };
    let k = if style & (STYLE_HANDHELD | STYLE_JOYDUAL) != 0 { 2.min(n) } else { 1 };
    for i in 0..k as u64 {
        w32(h + 4 * i, idx | (id & 0xFF) << 8 | (i as u32) << 16);
    }
    k as u64
});
fn six_fill(out: u64, sampling: u64) {
    // SixAxisSensorState: delta time, sampling, acceleration, angular velocity, angle, direction (3x3), attributes
    crate::guest::zero(out, 0x60);
    w64(out, 5_000_000);
    w64(out + 8, sampling);
    crate::guest::wr::<f32>(out + 0x18, -1.0); // acceleration z: at rest, screen up
    for (i, off) in [0x34u64, 0x44, 0x54].iter().enumerate() {
        let _ = i;
        crate::guest::wr::<f32>(out + off, 1.0); // the direction matrix's diagonal
    }
    w32(out + 0x58, 1);
}
hidfn!(six_state, |a| {
    let s = SAMPLING.fetch_add(1, Ordering::Relaxed) + 1;
    six_fill(a.x(0), s);
    0
});
hidfn!(six_states, |a| {
    let (out, n) = (a.x(0), a.i(1));
    if n <= 0 {
        return 0;
    }
    let s = SAMPLING.fetch_add(1, Ordering::Relaxed) + 1 + n as u64;
    for i in 0..n as u64 {
        six_fill(out + 0x60 * i, s - i);
    }
    n as u64
});
hidfn!(fusion_params, |a| {
    crate::guest::out(a.x(0), 0.03f32);
    crate::guest::out(a.x(1), 0.4f32);
    0
});
hidfn!(vib_handles, |a| {
    // (handles*, n, const NpadId&, style): two devices (left, right) of the connected controller
    let (h, n, id) = (a.x(0), a.i(1), r32(a.x(2)));
    let k = n.clamp(0, 2) as u64;
    let ty = if id == NPAD_HANDHELD { 2 } else { 3 };
    for i in 0..k {
        w32(h + 4 * i, ty | (id & 0xFF) << 8 | (i as u32) << 16);
    }
    k
});
hidfn!(vib_info, |a| {
    // VibrationDeviceInfo {u32 type (1 linear resonant actuator), u32 position (1 left, 2 right)}
    let h = r32(a.x(1));
    w32(a.x(0), 1);
    w32(a.x(0) + 4, 1 + (h >> 16 & 1));
    0
});
hidfn!(vib_actual, |a| {
    let o = a.x(0);
    crate::guest::zero(o, 16);
    crate::guest::wr::<f32>(o + 4, 160.0);
    crate::guest::wr::<f32>(o + 12, 320.0);
    0
});
hidfn!(touch_states, |a| {
    // GetTouchScreenStates<16>(states*, n): one sample with no touch points
    let (out, n) = (a.x(0), a.i(1));
    if n <= 0 {
        return 0;
    }
    crate::guest::zero(out, 0x10 + 0x28 * 16);
    w64(out, SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    1
});
hidfn!(touch_state, |a| {
    crate::guest::zero(a.x(0), 0x10 + 0x28 * 16);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    0
});
hidfn!(empty_state, |a| {
    // a single state of a device that reports nothing: sampling only
    crate::guest::zero(a.x(0), 0x30);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    0
});
hidfn!(empty_states, |a| {
    let n = a.i(1);
    if n <= 0 {
        return 0;
    }
    crate::guest::zero(a.x(0), 0x30);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    1
});
hidfn!(debugpad_state, |a| {
    crate::guest::zero(a.x(0), 0x20);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    0
});
hidfn!(debugpad_states, |a| {
    let n = a.i(1);
    if n <= 0 {
        return 0;
    }
    crate::guest::zero(a.x(0), 0x20);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    1
});
hidfn!(keyboard_state, |a| {
    crate::guest::zero(a.x(0), 0x38);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    0
});
hidfn!(keyboard_states, |a| {
    let n = a.i(1);
    if n <= 0 {
        return 0;
    }
    crate::guest::zero(a.x(0), 0x38);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    1
});
hidfn!(mouse_state, |a| {
    crate::guest::zero(a.x(0), 0x28);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    0
});
hidfn!(mouse_states, |a| {
    let n = a.i(1);
    if n <= 0 {
        return 0;
    }
    crate::guest::zero(a.x(0), 0x28);
    w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) + 1);
    1
});
hidfn!(gestures, |a| { 0 });
hidfn!(handheld_mode, |a| { 0 });
hidfn!(comm_mode, |a| { 0 });
hidfn!(joy_assign, |a| { 0 });

pub fn register(r: &mut Registry) {
    const W: &str = "hid: the host's controller state";
    let p = "_ZN2nn3hid";
    for (n, f) in [
        ("14InitializeNpadEv", nothing as unsafe fn(*mut Cpu)),
        ("22SetSupportedNpadIdTypeEPKjm", set_ids),
        ("24SetSupportedNpadStyleSetENS_4util10BitFlagSetILi32ENS0_12NpadStyleTagEEE", set_styles),
        ("24GetSupportedNpadStyleSetEv", get_styles),
        ("15GetNpadStyleSetERKj", get_style),
        ("12GetNpadStateEPNS0_16NpadFullKeyStateERKj", state_fullkey),
        ("12GetNpadStateEPNS0_17NpadHandheldStateERKj", state_handheld),
        ("12GetNpadStateEPNS0_16NpadJoyDualStateERKj", state_joydual),
        ("12GetNpadStateEPNS0_16NpadJoyLeftStateERKj", state_joyleft),
        ("12GetNpadStateEPNS0_17NpadJoyRightStateERKj", state_joyright),
        ("13GetNpadStatesEPNS0_16NpadFullKeyStateEiRKj", states_fullkey),
        ("13GetNpadStatesEPNS0_17NpadHandheldStateEiRKj", states_handheld),
        ("13GetNpadStatesEPNS0_16NpadJoyDualStateEiRKj", states_joydual),
        ("13GetNpadStatesEPNS0_16NpadJoyLeftStateEiRKj", states_joyleft),
        ("13GetNpadStatesEPNS0_17NpadJoyRightStateEiRKj", states_joyright),
        ("14DisconnectNpadERKj", nothing),
        ("18SetNpadJoyHoldTypeENS0_15NpadJoyHoldTypeE", hold_type_set),
        ("18GetNpadJoyHoldTypeEv", hold_type),
        ("20GetNpadJoyAssignmentERKj", joy_assign),
        ("28SetNpadJoyAssignmentModeDualERKj", nothing),
        ("30SetNpadJoyAssignmentModeSingleERKj", nothing),
        ("30SetNpadJoyAssignmentModeSingleERKjNS0_17NpadJoyDeviceTypeE", nothing),
        ("23MergeSingleJoyAsDualJoyERKjS2_", ok),
        ("18SwapNpadAssignmentERKjS2_", nothing),
        ("21StartLrAssignmentModeEv", nothing),
        ("20StopLrAssignmentModeEv", nothing),
        ("24SetNpadCommunicationModeENS0_21NpadCommunicationModeE", nothing),
        ("24GetNpadCommunicationModeEv", comm_mode),
        ("29SetNpadHandheldActivationModeENS0_26NpadHandheldActivationModeE", nothing),
        ("29GetNpadHandheldActivationModeEv", handheld_mode),
        ("19GetPlayerLedPatternERKj", led),
        ("22GetNpadControllerColorEPNS0_19NpadControllerColorERKj", color),
        ("22GetNpadControllerColorEPNS0_19NpadControllerColorES2_RKj", color2),
        ("21ShowControllerSupportEPNS0_27ControllerSupportResultInfoERKNS0_20ControllerSupportArgE", controller_support),
        ("21ShowControllerSupportERKNS0_20ControllerSupportArgE", ok),
        ("24ShowControllerStrapGuideEv", ok),
        ("28ShowControllerFirmwareUpdateERKNS0_27ControllerFirmwareUpdateArgE", ok),
        ("27BindNpadStyleSetUpdateEventERKjPNS_2os15SystemEventTypeENS3_14EventClearModeE", bind_style_event),
        ("41EnableUnintendedHomeButtonInputProtectionERKjb", nothing),
        ("44IsUnintendedHomeButtonInputProtectionEnabledERKj", zero),
        ("23GetSixAxisSensorHandlesEPNS0_19SixAxisSensorHandleEiRKjNS_4util10BitFlagSetILi32ENS0_12NpadStyleTagEEE", six_handles),
        ("18StartSixAxisSensorERKNS0_19SixAxisSensorHandleE", nothing),
        ("17StopSixAxisSensorERKNS0_19SixAxisSensorHandleE", nothing),
        ("21GetSixAxisSensorStateEPNS0_18SixAxisSensorStateERKNS0_19SixAxisSensorHandleE", six_state),
        ("22GetSixAxisSensorStatesEPNS0_18SixAxisSensorStateEiRKNS0_19SixAxisSensorHandleE", six_states),
        ("21IsSixAxisSensorAtRestERKNS0_19SixAxisSensorHandleE", one),
        ("25EnableSixAxisSensorFusionERKNS0_19SixAxisSensorHandleEb", nothing),
        ("28IsSixAxisSensorFusionEnabledERKNS0_19SixAxisSensorHandleE", one),
        ("32SetSixAxisSensorFusionParametersERKNS0_19SixAxisSensorHandleEff", nothing),
        ("32GetSixAxisSensorFusionParametersEPfS1_RKNS0_19SixAxisSensorHandleE", fusion_params),
        ("34ResetSixAxisSensorFusionParametersERKNS0_19SixAxisSensorHandleE", nothing),
        ("25SetGyroscopeZeroDriftModeERKNS0_19SixAxisSensorHandleERKNS0_22GyroscopeZeroDriftModeE", nothing),
        ("25GetGyroscopeZeroDriftModeERKNS0_19SixAxisSensorHandleE", one),
        ("25GetVibrationDeviceHandlesEPNS0_21VibrationDeviceHandleEiRKjNS_4util10BitFlagSetILi32ENS0_12NpadStyleTagEEE", vib_handles),
        ("25InitializeVibrationDeviceERKNS0_21VibrationDeviceHandleE", nothing),
        ("22GetVibrationDeviceInfoEPNS0_19VibrationDeviceInfoERKNS0_21VibrationDeviceHandleE", vib_info),
        ("24IsVibrationDeviceMountedERKNS0_21VibrationDeviceHandleE", one),
        ("18SendVibrationValueERKNS0_21VibrationDeviceHandleERKNS0_14VibrationValueE", nothing),
        ("6detail19SendVibrationValuesEPKNS0_21VibrationDeviceHandleEPKNS0_14VibrationValueEi", ok),
        ("23GetActualVibrationValueEPNS0_14VibrationValueERKNS0_21VibrationDeviceHandleE", vib_actual),
        ("20IsVibrationPermittedEv", one),
        ("27BeginForcedVibrationSessionEv", nothing),
        ("25EndForcedVibrationSessionEv", nothing),
        ("21InitializeTouchScreenEv", nothing),
        ("19GetTouchScreenStateILm16EEEvPNS0_16TouchScreenStateIXT_EEE", touch_state),
        ("20GetTouchScreenStatesILm16EEEiPNS0_16TouchScreenStateIXT_EEEi", touch_states),
        ("17InitializeGestureEv", nothing),
        ("16GetGestureStatesEPNS0_12GestureStateEi", gestures),
        ("18InitializeDebugPadEv", nothing),
        ("16GetDebugPadStateEPNS0_13DebugPadStateE", debugpad_state),
        ("17GetDebugPadStatesEPNS0_13DebugPadStateEi", debugpad_states),
        ("18InitializeKeyboardEv", nothing),
        ("16GetKeyboardStateEPNS0_13KeyboardStateE", keyboard_state),
        ("17GetKeyboardStatesEPNS0_13KeyboardStateEi", keyboard_states),
        ("15InitializeMouseEv", nothing),
        ("13GetMouseStateEPNS0_10MouseStateE", mouse_state),
        ("14GetMouseStatesEPNS0_10MouseStateEi", mouse_states),
    ] {
        r.force(&format!("{p}{n}"), W, f);
    }
    let _ = (empty_state, empty_states);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npad_states() {
        STYLES.store(STYLE_FULLKEY | STYLE_HANDHELD, Ordering::Relaxed);
        set_input(0b1, 0, 0x7000, 0, 0);
        let mut buf = vec![0u8; 0x28 * 3];
        let a = buf.as_mut_ptr() as u64;
        assert_eq!(states(a, 3, connected_id(), style_of(connected_id())), 3);
        let s0 = crate::guest::r64(a);
        assert_eq!(crate::guest::r64(a + 0x28), s0 - 1, "consecutive sampling numbers, newest first");
        assert_eq!(crate::guest::r64(a + 8), 0b1 | 1 << 17, "the A button and the left stick's up direction");
        assert_eq!(r32(a + 0x20) & ATTR_CONNECTED, 1);
        fill(a, 3, STYLE_FULLKEY, 1);
        assert_eq!(r32(a + 0x20), 0, "no controller on npad No4");
        set_input(0, 0, 0, 0, 0);
    }
}
