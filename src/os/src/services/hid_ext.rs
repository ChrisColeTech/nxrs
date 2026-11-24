//! nn::hid's newer and rarer devices, after nxnative's `hle_hid.c` (reimplemented): the Joy-Con 2 mouse sensor (Jcms)
//! and the pointer on top of it (Nmouse), the npad IMU the SDK's own sensor fusion reads (Switch 2 SDKs), the six-axis
//! IC information and the fusion's calibration store, the console's own six-axis sensor ("seven"), the Poké Ball
//! Plus (Palma), the private application's scene id, and the two-argument touch / gesture initializers. Without these
//! the SDK opens the hid service, which the host does not serve, and aborts on its error.
//!
//! The Jcms ring is fed by `feed_mouse` (a Joy-Con 2 whose mouse sensor reports: `input/switch2_ble.rs`); until one
//! does, the sensor reads as none, as on a console without one.

use crate::guest::{r32, w32, w64, Args};
use crate::hle::{Registry, RES_OK};
use crate::input::{self, PLAYERS};
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Mutex;

const RES_NPAD_IMU_ENABLED: u64 = 0x2908ca;
const RES_INVALID_NPAD_ID: u64 = 0x58aca;
const RES_INVALID_SIX_DEVICE: u64 = 0xd6ca;
const RES_SIX_CAL_NONE: u64 = 202 | 108 << 9;
const RES_PALMA_INVALID_HANDLE: u64 = 202 | 3302 << 9;
const NMOUSE_TAG: u64 = 0x4e4d_0000;
const JCMS_RING: usize = 16;
/// Pointer pixels per sensor count (a slow 10 cm slide is about 6,000 counts).
const NMOUSE_SCALE: f32 = 0.25;
pub const MOUSE_CONNECTED: u32 = 1;
pub const MOUSE_ON_SURFACE: u32 = 2;

/// Bit 0: InitializeJcms, bit 1: InitializeNmouse (until their Finalize): a front end turns sensors on then.
pub static MOUSE_WANTED: AtomicU32 = AtomicU32::new(0);
static SCENE: AtomicI32 = AtomicI32::new(0);

#[derive(Clone, Copy, Default)]
struct JcmsState {
    sampling: i64,
    dx: i32,
    dy: i32,
    attributes: u32,
}

struct Mice {
    ring: [[[JcmsState; JCMS_RING]; 2]; PLAYERS],
    n: [[i64; 2]; PLAYERS],
    pos: [[[f32; 2]; 2]; PLAYERS],
    d: [[[i32; 2]; 2]; PLAYERS],
    state: [[u32; 2]; PLAYERS],
    last: [i32; PLAYERS],
    range: [i32; 2],
}
static MICE: Mutex<Mice> = Mutex::new(Mice {
    ring: [[[JcmsState {
        sampling: 0,
        dx: 0,
        dy: 0,
        attributes: 0,
    }; JCMS_RING]; 2]; PLAYERS],
    n: [[0; 2]; PLAYERS],
    pos: [[[0.0; 2]; 2]; PLAYERS],
    d: [[[0; 2]; 2]; PLAYERS],
    state: [[0; 2]; PLAYERS],
    last: [-1; PLAYERS],
    range: [0; 2],
});

/// A Joy-Con's mouse sensor: movement since its last report (counts, x right, y toward the player) and its state.
pub fn feed_mouse(p: usize, side: usize, dx: i32, dy: i32, state: u32) {
    if p >= PLAYERS || side > 1 {
        return;
    }
    let mut m = MICE.lock().unwrap();
    m.n[p][side] += 1;
    let n = m.n[p][side];
    m.ring[p][side][((n - 1) as usize) % JCMS_RING] = JcmsState {
        sampling: n,
        dx,
        dy,
        attributes: state & 3,
    };
    let w = if m.range[0] > 0 { m.range[0] } else { 1920 } as f32;
    let h = if m.range[1] > 0 { m.range[1] } else { 1080 } as f32;
    if n == 1 {
        m.pos[p][side] = [w / 2.0, h / 2.0];
    }
    let xy = m.pos[p][side];
    m.pos[p][side] = [
        (xy[0] + dx as f32 * NMOUSE_SCALE).clamp(0.0, w - 1.0),
        (xy[1] + dy as f32 * NMOUSE_SCALE).clamp(0.0, h - 1.0),
    ];
    m.d[p][side] = [
        (dx as f32 * NMOUSE_SCALE).round() as i32,
        (dy as f32 * NMOUSE_SCALE).round() as i32,
    ];
    m.state[p][side] = state;
    if state & MOUSE_ON_SURFACE != 0 {
        m.last[p] = side as i32;
    }
}

/// A Jcms handle (npad id | type << 8) as (player, side).
fn jcms_target(h: u32) -> Option<(usize, usize)> {
    let (id, ty) = (h & 0xff, h >> 8 & 0xff);
    if ty > 1 || id as usize >= PLAYERS {
        return None;
    }
    let p = super::hid::player_on(id)?;
    Some((p, ty as usize))
}

fn jcms_copy(out: u64, n: i32, h: u32) -> u64 {
    let Some((p, side)) = jcms_target(h) else {
        return 0;
    };
    if out == 0 || n <= 0 {
        return 0;
    }
    let m = MICE.lock().unwrap();
    let have = m.n[p][side];
    let mut k = 0i64;
    while k < n as i64 && (k as usize) < JCMS_RING && have - k > 0 {
        let s = m.ring[p][side][((have - 1 - k) as usize) % JCMS_RING];
        let o = out + 32 * k as u64;
        crate::guest::zero(o, 32);
        w64(o + 8, s.sampling as u64);
        w32(o + 0x10, s.dx as u32);
        w32(o + 0x14, s.dy as u32);
        w32(o + 0x18, s.attributes);
        k += 1;
    }
    k as u64
}

fn nmouse_of(h: u64) -> Option<(usize, usize)> {
    if h & !0xffff != NMOUSE_TAG {
        return None;
    }
    let (p, side) = jcms_target((h >> 8 & 0xff) as u32 | ((h & 1) as u32) << 8)?;
    (MICE.lock().unwrap().n[p][side] > 0).then_some((p, side))
}

macro_rules! f {
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

f!(nothing, |a| { 0 });
f!(ok, |a| { RES_OK });
f!(zero, |a| { 0 });
f!(one, |a| { 1 });
f!(jcms_init, |a| {
    MOUSE_WANTED.fetch_or(1, Ordering::AcqRel);
    0
});
f!(jcms_fini, |a| {
    MOUSE_WANTED.fetch_and(!1, Ordering::AcqRel);
    0
});
f!(jcms_handle, |a| {
    ((r32(a.x(0)) & 0xff) | (r32(a.x(1)) & 0xff) << 8) as u64
});
f!(jcms_states, |a| {
    if a.x(2) != 0 {
        jcms_copy(a.x(0), a.i(1), r32(a.x(2)))
    } else {
        0
    }
});
f!(jcms_state, |a| {
    let out = a.x(0);
    if out != 0 {
        crate::guest::zero(out, 32);
        if a.x(1) != 0 {
            jcms_copy(out, 1, r32(a.x(1)));
        }
    }
    0
});
f!(nmouse_init, |a| {
    let mut m = MICE.lock().unwrap();
    m.range = [a.i(2), a.i(3)];
    MOUSE_WANTED.fetch_or(2, Ordering::AcqRel);
    0
});
f!(nmouse_fini, |a| {
    MOUSE_WANTED.fetch_and(!2, Ordering::AcqRel);
    0
});
f!(nmouse_merged, |a| {
    w64(a.x(0), 0);
    1
});
f!(nmouse_types, |a| {
    w32(a.x(0), 0);
    1
});
f!(nmouse_states, |a| {
    // (int* count, NmouseState* out, int n, NmouseHandle h): the merged handle has no device
    w32(a.x(0), 0);
    1
});
f!(nmouse_handles, |a| {
    // one handle per Joy-Con of the npad whose sensor reports; the one last on a surface first
    let (out, n, id) = (a.x(0), a.i(1), a.w(2));
    let mut k = 0;
    for pass in 0..2 {
        for side in 0..2u64 {
            if out == 0 || k >= n {
                break;
            }
            let h = NMOUSE_TAG | ((id & 0xff) as u64) << 8 | side;
            let Some((p, sd)) = nmouse_of(h) else {
                continue;
            };
            let last = MICE.lock().unwrap().last[p];
            let first = if last < 0 { sd == 1 } else { last == sd as i32 };
            if first == (pass == 0) {
                w64(out + 8 * k as u64, h);
                k += 1;
            }
        }
    }
    k as u64
});
f!(nmouse_set_range, |a| {
    MICE.lock().unwrap().range = [a.i(0), a.i(1)];
    0
});
f!(nmouse_get_range, |a| {
    let r = MICE.lock().unwrap().range;
    w32(a.x(0), r[0] as u32);
    w32(a.x(1), r[1] as u32);
    0
});
f!(nmouse_std, |a| { 0 });
f!(nmouse_enable, |a| {
    (a.x(0) == 0 || nmouse_of(a.x(0)).is_some()) as u64
});
f!(nmouse_pos, |a| {
    let h = a.x(0);
    match nmouse_of(h) {
        Some((p, s)) => {
            MICE.lock().unwrap().pos[p][s] = [a.i(1) as f32, a.i(2) as f32];
            1
        }
        None => (h == 0) as u64,
    }
});
f!(nmouse_state, |a| {
    // NmouseState (48 bytes): 8 bytes, sampling, x, y, dx, dy, wheel x / y, buttons (1 left, 2 right), attributes
    let (out, h) = (a.x(0), a.x(1));
    let Some((p, side)) = nmouse_of(h) else {
        return 0;
    };
    if out == 0 {
        return 0;
    }
    let (b, _) = input::JOY[p][side].peek();
    let buttons = if side == 1 {
        (b >> 7 & 1) as u32 | ((b >> 9 & 1) as u32) << 1
    } else {
        (b >> 6 & 1) as u32 | ((b >> 8 & 1) as u32) << 1
    };
    let m = MICE.lock().unwrap();
    crate::guest::zero(out, 48);
    w64(out + 8, m.n[p][side] as u64);
    w32(out + 0x10, m.pos[p][side][0].round() as i32 as u32);
    w32(out + 0x14, m.pos[p][side][1].round() as i32 as u32);
    w32(out + 0x18, m.d[p][side][0] as u32);
    w32(out + 0x1c, m.d[p][side][1] as u32);
    w32(out + 0x28, buttons);
    w32(
        out + 0x2c,
        2 | (m.state[p][side] & MOUSE_ON_SURFACE != 0) as u32,
    );
    1
});
f!(nmouse_npad, |a| { 0 });
f!(scene_set, |a| {
    SCENE.store(a.i(0), Ordering::Release);
    0
});
f!(scene_get, |a| {
    SCENE.load(Ordering::Acquire) as u32 as u64
});
unsafe fn mouse_speed(c: *mut Cpu) {
    (*c).v[0] = 1.0f32.to_bits() as u128;
}

// ---------------------------------------------------------------- the npad IMU (the SDK's sensor fusion)

static IMU_ON: Mutex<bool> = Mutex::new(false);
f!(imu_enable, |a| {
    let mut on = IMU_ON.lock().unwrap();
    if *on {
        return RES_NPAD_IMU_ENABLED;
    }
    *on = true;
    0
});
f!(imu_disable, |a| {
    *IMU_ON.lock().unwrap() = false;
    0
});
f!(imu_is, |a| { *IMU_ON.lock().unwrap() as u64 });

fn npad_slot(id: u32) -> Option<usize> {
    match id {
        0..=7 => Some(id as usize),
        0x10 => Some(8),
        0x20 => Some(9),
        _ => None,
    }
}
fn six_handle(h: u32) -> Result<(usize, usize), u64> {
    let slot = npad_slot(h >> 8 & 0xff).ok_or(RES_INVALID_NPAD_ID)?;
    let dev = (h >> 16 & 0xff) as usize;
    if dev >= 3 {
        return Err(RES_INVALID_SIX_DEVICE);
    }
    Ok((slot, dev))
}

/// NpadImuSample (0x28): sampling, ns since the previous, acceleration in m/s^2 (the gravity direction: face up reads
/// (0, 0, -9.8)), angular velocity in rad/s; the newest 5 ms ticks, oldest first.
pub fn imu_samples(out: u64, n: i32, h: u32) -> u64 {
    if out == 0 || n <= 0 || six_handle(h).is_err() || !*IMU_ON.lock().unwrap() {
        return 0;
    }
    let t = input::motion::now_s();
    let tick = (t / 0.005) as i64;
    let k = (n as i64).min(16).min(tick + 1);
    for i in 0..k {
        let tk = tick - k + 1 + i;
        let s = super::hid::six_sample_at(h, tk as f64 * 0.005);
        let o = out + 0x28 * i as u64;
        w64(o, (tk + 1) as u64);
        w64(o + 8, 5_000_000);
        for j in 0..3u64 {
            crate::guest::wr::<f32>(o + 0x10 + 4 * j, s.acc[j as usize] * 9.80665);
            crate::guest::wr::<f32>(o + 0x1c + 4 * j, s.gyro[j as usize] * std::f32::consts::TAU);
        }
    }
    k as u64
}
f!(imu_samples_fn, |a| { imu_samples(a.x(0), a.i(1), a.w(2)) });

// SixAxisSensorIcInformation (0xc8): gyro then accelerometer, each range, offset bounds, sensitivity bounds; 2000 deg/s
// and 8 G (the Switch's IC), offsets within 10 deg/s and 0.0612 G, sensitivity 0.95..1.05, cross-axis 0.003.
f!(six_ic, |a| {
    let (out, h) = (a.x(0), r32(a.x(1)));
    if let Err(e) = six_handle(h) {
        crate::log!("[RED] hid GetSixAxisSensorIcInformation: handle {h:#x} refused ({e:#x})");
        return e;
    }
    let smin = [
        0.95f32, -0.003, -0.003, -0.003, 0.95, -0.003, -0.003, -0.003, 0.95,
    ];
    let smax = [
        1.05f32, 0.003, 0.003, 0.003, 1.05, 0.003, 0.003, 0.003, 1.05,
    ];
    for (half, (range, off)) in [(2000.0f32, 10.0f32), (8.0, 0.0612)].iter().enumerate() {
        let b = out + 0x64 * half as u64;
        let mut v = vec![*range, -off, -off, -off, *off, *off, *off];
        v.extend(smin);
        v.extend(smax);
        for (i, x) in v.iter().enumerate() {
            crate::guest::wr::<f32>(b + 4 * i as u64, *x);
        }
    }
    RES_OK
});

const SIX_CAL_SIZE: usize = 0x744;
static SIX_CAL: Mutex<Vec<(usize, usize, Vec<u8>)>> = Mutex::new(Vec::new());
fn cal_dev(d: usize) -> usize {
    if d == 2 {
        0
    } else {
        d
    }
}
f!(six_cal_load, |a| {
    let (out, h) = (a.x(0), r32(a.x(1)));
    let (slot, dev) = match six_handle(h) {
        Ok(x) => x,
        Err(e) => return e,
    };
    crate::guest::zero(out, SIX_CAL_SIZE as u64);
    let c = SIX_CAL.lock().unwrap();
    if let Some((_, _, d)) = c
        .iter()
        .find(|(s, dv, _)| *s == slot && *dv == cal_dev(dev))
    {
        crate::guest::write_bytes(out, d);
        return RES_OK;
    }
    // the host's controllers come with their rest offset taken off (Sony, Nintendo): a zero bias; a pad without motion
    // has nothing stored
    if super::hid::six_sample_live(h) {
        RES_OK
    } else {
        RES_SIX_CAL_NONE
    }
});
f!(six_cal_store, |a| {
    let (h, inp) = (r32(a.x(0)), a.x(1));
    let (slot, dev) = match six_handle(h) {
        Ok(x) => x,
        Err(e) => return e,
    };
    let d = crate::guest::bytes_mut(inp, SIX_CAL_SIZE as u64).to_vec();
    let mut c = SIX_CAL.lock().unwrap();
    c.retain(|(s, dv, _)| !(*s == slot && *dv == cal_dev(dev)));
    c.push((slot, cal_dev(dev), d));
    RES_OK
});

f!(palma_handle, |a| {
    if a.x(0) != 0 {
        w64(a.x(0), 0);
    }
    RES_PALMA_INVALID_HANDLE
});

// every call on a Palma handle: none is ever connected (the operation complete event is never bound)
f!(palma_fail, |a| {
    crate::log_once!(
        "palma fail",
        "hid: a Poké Ball Plus call on a handle: none connected (ResultPalmaInvalidHandle)"
    );
    RES_PALMA_INVALID_HANDLE
});

pub fn register(r: &mut Registry) {
    const W: &str = "hid: Joy-Con mouse, pointer, npad IMU, sensor IC and calibration, console six-axis, Palma (hid_ext)";
    let p = "_ZN2nn3hid";
    for (n, f) in [
        ("14InitializeJcmsEPvm", jcms_init as unsafe fn(*mut Cpu)),
        ("12FinalizeJcmsEv", jcms_fini),
        ("13GetJcmsHandleERKjRKNS0_8JcmsTypeE", jcms_handle),
        ("9StartJcmsERKNS0_10JcmsHandleE", nothing),
        ("8StopJcmsERKNS0_10JcmsHandleE", nothing),
        ("13GetJcmsStatesEPNS0_9JcmsStateEiRKNS0_10JcmsHandleE", jcms_states),
        ("12GetJcmsStateEPNS0_9JcmsStateERKNS0_10JcmsHandleE", jcms_state),
        ("16InitializeNmouseEPvmii", nmouse_init),
        ("16InitializeNmouseEPvmiiNS0_17NmousePointerTypeE", nmouse_init),
        ("14FinalizeNmouseEv", nmouse_fini),
        ("26SetMergedNmouseSourceTypesENS_4util10BitFlagSetILi32ENS0_10NmouseTypeEEE", nothing),
        ("21GetMergedNmouseHandleEPNS0_12NmouseHandleE", nmouse_merged),
        ("16GetNmouseTypeSetEPNS_4util10BitFlagSetILi32ENS0_10NmouseTypeEEENS0_12NmouseHandleE", nmouse_types),
        ("15GetNmouseStatesEPiPNS0_11NmouseStateEiNS0_12NmouseHandleE", nmouse_states),
        ("16GetNmouseHandlesEPNS0_12NmouseHandleEij", nmouse_handles),
        ("32SetNmouseButtonConfigJoyLeftJcmsERKNS0_29NmouseButtonConfigJoyLeftJcmsE", nothing),
        ("33SetNmouseButtonConfigJoyRightJcmsERKNS0_30NmouseButtonConfigJoyRightJcmsE", nothing),
        ("34SetNmouseButtonConfigStandardMouseERKNS0_31NmouseButtonConfigStandardMouseE", nothing),
        ("22SetNmousePointerRangesEii", nmouse_set_range),
        ("22GetNmousePointerRangesEPiS1_", nmouse_get_range),
        ("28SetNmousePointerMovementAreaEiiii", nothing),
        ("21RestrictNmousePointerEffff", nothing),
        ("28GetStandardMouseNmouseHandleEPNS0_12NmouseHandleE", nmouse_std),
        ("19EnableNmousePointerENS0_12NmouseHandleEb", nmouse_enable),
        ("24SetNmousePointerPositionENS0_12NmouseHandleEii", nmouse_pos),
        ("14GetNmouseStateEPNS0_11NmouseStateENS0_12NmouseHandleE", nmouse_state),
        ("13GetNpadIdTypeEPjNS0_12NmouseHandleE", nmouse_npad),
        ("23SetSceneIdForPrivateAppEi", scene_set),
        ("23GetSceneIdForPrivateAppEv", scene_get),
        ("25GetMousePointerSpeedScaleEv", mouse_speed),
        ("13EnableNpadImuEPvm", imu_enable),
        ("14DisableNpadImuEv", imu_disable),
        ("16IsNpadImuEnabledEv", imu_is),
        ("17GetNpadImuSamplesEPNS0_13NpadImuSampleEiNS0_13NpadImuHandleE", imu_samples_fn),
        ("29GetSixAxisSensorIcInformationEPNS0_26SixAxisSensorIcInformationERKNS0_19SixAxisSensorHandleE", six_ic),
        ("37LoadSixAxisSensorCalibrationParameterEPNS0_33SixAxisSensorCalibrationParameterERKNS0_19SixAxisSensorHandleE", six_cal_load),
        ("38StoreSixAxisSensorCalibrationParameterERKNS0_19SixAxisSensorHandleERKNS0_33SixAxisSensorCalibrationParameterE", six_cal_store),
        ("41IsFirmwareUpdateAvailableForSixAxisSensorERKNS0_19SixAxisSensorHandleE", zero),
        ("20FinalizeSensorFusionEv", nothing),
        ("28InitializeSevenSixAxisSensorEPvm", ok),
        ("26FinalizeSevenSixAxisSensorEv", ok),
        ("23StartSevenSixAxisSensorEv", ok),
        ("22StopSevenSixAxisSensorEv", ok),
        ("32ResetSevenSixAxisSensorTimestampEv", nothing),
        ("27GetSevenSixAxisSensorStatesEPNS0_23SevenSixAxisSensorStateEi", zero),
        ("26IsSevenSixAxisSensorAtRestEv", one),
        ("17SetPalmaBoostModeEb", nothing),
        ("24EnableAnyPalmaConnectionEv", ok),
        ("25DisableAnyPalmaConnectionEv", ok),
        ("24GetPalmaConnectionHandleEPNS0_21PalmaConnectionHandleERKj", palma_handle),
        ("15InitializePalmaERKNS0_21PalmaConnectionHandleE", palma_fail),
        ("31BindPalmaOperationCompleteEventERKNS0_21PalmaConnectionHandleEPNS_2os15SystemEventTypeENS4_14EventClearModeE", palma_fail),
        ("21GetPalmaOperationInfoEPNS0_18PalmaOperationInfoERKNS0_21PalmaConnectionHandleE", palma_fail),
        ("17PlayPalmaActivityERKNS0_21PalmaConnectionHandleERKt", palma_fail),
        ("18SetPalmaFrModeTypeERKNS0_21PalmaConnectionHandleERKNS0_15PalmaFrModeTypeE", palma_fail),
        ("13ReadPalmaStepERKNS0_21PalmaConnectionHandleE", palma_fail),
        ("15EnablePalmaStepERKNS0_21PalmaConnectionHandleEb", palma_fail),
        ("14ResetPalmaStepERKNS0_21PalmaConnectionHandleE", palma_fail),
        ("27ReadPalmaApplicationSectionERKNS0_21PalmaConnectionHandleEim", palma_fail),
        ("28WritePalmaApplicationSectionERKNS0_21PalmaConnectionHandleEimRKNS0_35PalmaApplicationSectionAccessBufferE", palma_fail),
        ("19ReadPalmaUniqueCodeERKNS0_21PalmaConnectionHandleE", palma_fail),
        ("25SetPalmaUniqueCodeInvalidERKNS0_21PalmaConnectionHandleE", palma_fail),
        ("23WritePalmaActivityEntryERKNS0_21PalmaConnectionHandleERKtPKNS0_18PalmaActivityEntryE", palma_fail),
        ("28WritePalmaRgbLedPatternEntryERKNS0_21PalmaConnectionHandleERKtPKvm", palma_fail),
        ("19WritePalmaWaveEntryERKNS0_21PalmaConnectionHandleERKNS0_12PalmaWaveSetERKtPKvmm", palma_fail),
        ("37GetPalmaDataBaseIdentificationVersionERKNS0_21PalmaConnectionHandleE", palma_fail),
        ("37SetPalmaDataBaseIdentificationVersionERKNS0_21PalmaConnectionHandleEi", palma_fail),
        ("19SuspendPalmaFeatureERKNS0_21PalmaConnectionHandleERKNS_4util10BitFlagSetILi32ENS0_12PalmaFeatureEEE", palma_fail),
        ("16ReadPalmaPlayLogERKNS0_21PalmaConnectionHandleERKt", palma_fail),
        ("17ResetPalmaPlayLogERKNS0_21PalmaConnectionHandleERKt", palma_fail),
        ("21InitializeTouchScreenEii", nothing),
        ("17InitializeGestureEii", nothing),
        ("24ShowControllerStrapGuideERKNS0_13StrapGuideArgE", ok),
    ] {
        r.force(&format!("{p}{n}"), W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jcms_ring_and_pointer() {
        // player 1 is npad No1 (docked by default)
        feed_mouse(0, 1, 100, -40, MOUSE_CONNECTED | MOUSE_ON_SURFACE);
        feed_mouse(0, 1, 8, 4, MOUSE_CONNECTED | MOUSE_ON_SURFACE);
        let mut buf = vec![0u8; 32 * 4];
        let a = buf.as_mut_ptr() as u64;
        let h = super::super::hid::connected_id() | 1 << 8;
        assert_eq!(jcms_copy(a, 4, h), 2, "two samples, newest first");
        assert_eq!(crate::guest::r64(a + 8), 2);
        assert_eq!(r32(a + 0x10), 8);
        assert_eq!(r32(a + 32 + 0x14) as i32, -40);
        assert!(
            nmouse_of(NMOUSE_TAG | (super::super::hid::connected_id() as u64) << 8 | 1).is_some()
        );
        assert!(
            nmouse_of(NMOUSE_TAG | 5 << 8).is_none(),
            "no sensor on that npad"
        );
    }

    /// Palma (Poké Ball Plus) as nxnative answers it: all 24 calls taken over; no handle, every call on one fails
    /// with ResultPalmaInvalidHandle (202-3302).
    #[test]
    fn palma_calls_fail_as_none_connected() {
        let mut r = crate::hle::Registry::default();
        register(&mut r);
        let n = r
            .names()
            .filter(|n| n.starts_with("_ZN2nn3hid") && n.contains("Palma"))
            .count();
        assert_eq!(n, 24);
        let mut c = nxrt::cpu::Cpu::new_boxed();
        unsafe { palma_fail(&mut *c) };
        assert_eq!(c.x[0], 202 | 3302 << 9);
    }

    #[test]
    fn imu_and_handles() {
        assert!(six_handle(0x0300).is_ok());
        assert_eq!(six_handle(0x0f00), Err(RES_INVALID_NPAD_ID));
        assert_eq!(six_handle(0x30000), Err(RES_INVALID_SIX_DEVICE));
        *IMU_ON.lock().unwrap() = true;
        input::motion::now_s();
        std::thread::sleep(std::time::Duration::from_millis(30)); // ticks since the clock's start
        let mut buf = vec![0u8; 0x28 * 4];
        let a = buf.as_mut_ptr() as u64;
        assert_eq!(imu_samples(a, 4, 3), 4);
        let s0 = crate::guest::r64(a);
        assert_eq!(crate::guest::r64(a + 0x28), s0 + 1, "oldest first");
        let z: f32 = crate::guest::rd(a + 0x18);
        assert!((z + 9.80665).abs() < 1e-3, "at rest, face up: {z}");
        *IMU_ON.lock().unwrap() = false;
    }
}
