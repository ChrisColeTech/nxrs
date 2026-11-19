//! nn::oe (and the bits of nn::applet / am an application reaches through it): the operation mode, focus, the
//! notification queue, performance configuration, language, launch parameter and the switches the system keeps.
//! The host runs one application in the foreground; `NXOS_OPMODE` (0 handheld, 1 docked; default docked) and
//! `NXOS_LANG` (default en-US) set what it reports.

use crate::guest::{w32, w64, Args};
use crate::hle::{Registry, RES_OK};
use nxrt::cpu::Cpu;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

pub const MSG_FOCUS: u32 = 15;
pub const MSG_OPMODE: u32 = 30;
pub const MSG_PERFMODE: u32 = 31;

pub fn opmode() -> i32 {
    static M: OnceLock<i32> = OnceLock::new();
    *M.get_or_init(|| std::env::var("NXOS_OPMODE").ok().and_then(|v| v.parse().ok()).unwrap_or(1))
}

pub fn language() -> String {
    std::env::var("NXOS_LANG").unwrap_or_else(|_| "en-US".into())
}

/// The language code as nn::settings::LanguageCode holds it (8 bytes, NUL padded).
pub fn language_code() -> u64 {
    let mut b = [0u8; 8];
    let l = language();
    let n = l.len().min(7);
    b[..n].copy_from_slice(&l.as_bytes()[..n]);
    u64::from_le_bytes(b)
}

static QUEUE: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());
static QUEUE_CV: Condvar = Condvar::new();
/// The guest SystemEventType of the notification event (0 until the game asks for it).
static MSG_EVENT: AtomicU64 = AtomicU64::new(0);
static FOCUS: AtomicI32 = AtomicI32::new(1);
static FOCUS_MODE: AtomicI32 = AtomicI32::new(-1);
static INACTIVITY: AtomicI32 = AtomicI32::new(0);

fn event_sync() {
    let ev = MSG_EVENT.load(Ordering::Relaxed);
    if ev == 0 {
        return;
    }
    if QUEUE.lock().unwrap().is_empty() {
        crate::services::os::clear_system_event(ev);
    } else {
        crate::services::os::signal_bound(ev);
    }
}

/// Queues a notification for the game (focus and mode messages are not queued twice).
pub fn push(m: u32) {
    {
        let mut q = QUEUE.lock().unwrap();
        if (m == MSG_FOCUS || m == MSG_OPMODE || m == MSG_PERFMODE) && q.contains(&m) {
            return;
        }
        q.push_back(m);
        QUEUE_CV.notify_all();
    }
    event_sync();
}

fn try_pop() -> Option<u32> {
    let m = QUEUE.lock().unwrap().pop_front();
    if let Some(m) = m {
        crate::log!("oe: notification {m} popped by the game");
    }
    event_sync();
    m
}

macro_rules! oefn {
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

oefn!(nothing, |a| { 0 });
oefn!(get_opmode, |a| { opmode() as u64 });
oefn!(get_perfmode, |a| { opmode() as u64 });
oefn!(get_focus, |a| { FOCUS.load(Ordering::Relaxed) as u64 });
oefn!(set_focus_mode, |a| {
    FOCUS_MODE.store(a.i(0), Ordering::Relaxed);
    crate::log!("oe::SetFocusHandlingMode {}", a.i(0));
    0
});
oefn!(try_pop_msg, |a| {
    match try_pop() {
        Some(m) => {
            w32(a.x(0), m);
            1
        }
        None => 0,
    }
});
oefn!(pop_msg, |a| {
    loop {
        crate::sched::without_core(|| {
            let mut q = QUEUE.lock().unwrap();
            while q.is_empty() {
                q = QUEUE_CV.wait(q).unwrap();
            }
        });
        if let Some(m) = try_pop() {
            return m as u64;
        }
    }
});
unsafe fn get_msg_event(c: *mut Cpu) {
    let mut ev = MSG_EVENT.load(Ordering::Relaxed);
    if ev == 0 {
        ev = nxrt::kernel::get().space.kernel_alloc(0x80, 16);
        crate::services::os::bind_system_event(c, ev, true);
        MSG_EVENT.store(ev, Ordering::Relaxed);
        event_sync();
    }
    (*c).x[0] = ev;
}
oefn!(get_lang, |a| { language_code() });
oefn!(region, |a| {
    let l = language();
    (if l.starts_with("ja") {
        0
    } else if l.starts_with("ko") || l.starts_with("zh") {
        4
    } else if ["en-US", "fr-CA", "es-419", "pt-BR"].contains(&l.as_str()) {
        1
    } else {
        2
    }) as u64
});
oefn!(device_id, |a| {
    crate::guest::write_bytes(a.x(0), &[0x78, 0x62, 0x6c, 0x00, 0x53, 0x57, 0x4e, 0x41, 0x54, 0x49, 0x56, 0x45, 0x31, 0x30, 0x30, 0x31]);
    0
});
oefn!(launch_param, |a| {
    crate::guest::out(a.x(0), 0u64);
    0
});
oefn!(prev_program, |a| { u64::MAX });
oefn!(default_res, |a| {
    let docked = opmode() == 1;
    crate::guest::out(a.x(0), if docked { 1920u32 } else { 1280 });
    crate::guest::out(a.x(1), if docked { 1080u32 } else { 720 });
    0
});
unsafe fn default_res_event(c: *mut Cpu) {
    let ev = (*c).x[0];
    crate::services::os::bind_system_event(c, ev, false);
}
oefn!(perf_cfg, |a| { if a.i(0) == 1 { 0x0001_0001 } else { 0x0001_0000 } });
oefn!(set_perf_cfg, |a| {
    crate::log_once!("perfcfg", "oe::SetPerformanceConfiguration {:#x} {:#x} (the host does not scale clocks)", a.x(0), a.x(1));
    0
});
oefn!(inactivity_unsafe, |a| {
    INACTIVITY.store(if a.b(0) { 2 } else { 0 }, Ordering::Relaxed);
    0
});
oefn!(inactivity_safe, |a| {
    INACTIVITY.store(a.b(0) as i32, Ordering::Relaxed);
    0
});
oefn!(inactivity_mode, |a| { INACTIVITY.load(Ordering::Relaxed) as u64 });
oefn!(inactivity_q, |a| { (INACTIVITY.load(Ordering::Relaxed) != 0) as u64 });
oefn!(aruid, |a| { 0x80 });
oefn!(display_version, |a| {
    let v = crate::boot::try_process().map(|p| p.display_version.clone()).unwrap_or_else(|| "1.0.0".into());
    crate::guest::zero(a.x(0), 16);
    crate::guest::put_cstr(a.x(0), 16, &v);
    0
});
oefn!(volume_balance, |a| {
    crate::guest::out(a.x(0), 1.0f32);
    crate::guest::out(a.x(1), 1.0f32);
    0
});
oefn!(active_time, |a| { (crate::log::secs() * 1e9) as u64 });
oefn!(relaunch, |a| {
    crate::log!("oe::RequestToRelaunchApplication: the host ends the run");
    crate::log::report_gaps("relaunch requested");
    nxrt::process::exit(0)
});
oefn!(ok, |a| { RES_OK });
oefn!(zero, |a| { 0 });

pub fn register(r: &mut Registry) {
    const W: &str = "am/oe: the host runs one application in the foreground";
    for (n, f) in [
        ("_ZN2nn2oe20SwitchLcdBacklightOnEv", nothing as unsafe fn(*mut Cpu)),
        ("_ZN2nn2oe21SwitchLcdBacklightOffEv", nothing),
        ("_ZN2nn2oe17FinishStartupLogoEv", nothing),
        ("_ZN2nn2oe15EnableRecordingEv", nothing),
        ("_ZN2nn2oe16DisableRecordingEv", nothing),
        ("_ZN2nn2oe16EnableScreenShotEv", nothing),
        ("_ZN2nn2oe17DisableScreenShotEv", nothing),
        ("_ZN2nn2oe23BeginBlockingHomeButtonEv", nothing),
        ("_ZN2nn2oe21EndBlockingHomeButtonEv", nothing),
        ("_ZN2nn2oe29SetScreenShotImageOrientationENS_5album16ImageOrientationE", nothing),
        ("_ZN2nn2oe44SetUserInactivityDetectionTimeExtendedUnsafeEb", inactivity_unsafe),
        ("_ZN2nn2oe38SetUserInactivityDetectionTimeExtendedEb", inactivity_safe),
        ("_ZN2nn2oe43GetUserInactivityDetectionTimeExtensionModeEv", inactivity_mode),
        ("_ZN2nn2oe37IsUserInactivityDetectionTimeExtendedEv", inactivity_q),
        ("_ZN2nn6applet23GetAppletResourceUserIdEv", aruid),
        ("_ZN2nn2oe21TryPopLaunchParameterEPmPvm", launch_param),
        ("_ZN2nn2oe23GetPreviousProgramIndexEv", prev_program),
        ("_ZN2nn2oe28RequestToRelaunchApplicationEv", relaunch),
        ("_ZN2nn2oe16GetOperationModeEv", get_opmode),
        ("_ZN2nn2oe18GetPerformanceModeEv", get_perfmode),
        ("_ZN2nn2oe27GetDefaultDisplayResolutionEPiS1_", default_res),
        ("_ZN2nn2oe38GetDefaultDisplayResolutionChangeEventEPNS_2os11SystemEventE", default_res_event),
        ("_ZN2nn2oe27GetPerformanceConfigurationENS0_15PerformanceModeE", perf_cfg),
        ("_ZN2nn2oe27SetPerformanceConfigurationENS0_15PerformanceModeEi", set_perf_cfg),
        ("_ZN2nn2oe27SetPerformanceConfigurationEi", set_perf_cfg),
        ("_ZN2nn2oe15SetCpuBoostModeENS0_12CpuBoostModeE", nothing),
        ("_ZN2nn2oe20GetCurrentFocusStateEv", get_focus),
        ("_ZN2nn2oe20SetFocusHandlingModeENS0_17FocusHandlingModeE", set_focus_mode),
        ("_ZN2nn2oe25TryPopNotificationMessageEPj", try_pop_msg),
        ("_ZN2nn2oe22PopNotificationMessageEv", pop_msg),
        ("_ZN2nn2oe27GetNotificationMessageEventEv", get_msg_event),
        ("_ZN2nn2oe28SetResumeNotificationEnabledEb", nothing),
        ("_ZN2nn2oe42SetOperationModeChangedNotificationEnabledEb", nothing),
        ("_ZN2nn2oe44SetPerformanceModeChangedNotificationEnabledEb", nothing),
        ("_ZN2nn2oe18GetDesiredLanguageEv", get_lang),
        ("_ZN2nn2oe13GetRegionCodeEv", region),
        ("_ZN2nn2oe26IsLanguageSelectionLimitedEv", zero),
        ("_ZN2nn2oe17GetPseudoDeviceIdEPNS_4util4UuidE", device_id),
        ("_ZN2nn2oe17GetDisplayVersionEPNS0_14DisplayVersionE", display_version),
        ("_ZN2nn2oe24GetExpectedVolumeBalanceEPfS1_", volume_balance),
        ("_ZN2nn2oe24SetExpectedVolumeBalanceEff", nothing),
        ("_ZN2nn2oe25GetProgramTotalActiveTimeEv", active_time),
        ("_ZN2nn2oe31EnterExitRequestHandlingSectionEv", nothing),
        ("_ZN2nn2oe31LeaveExitRequestHandlingSectionEv", nothing),
        ("_ZN2nn2oe34DisableAlbumImageTakenNotificationEv", nothing),
        ("_ZN2nn2oe33EnableAlbumImageTakenNotificationEv", nothing),
        ("_ZN2nn2oe24IsInStoreDemoModeEnabledEv", zero),
        ("_ZN2nn2oe24SetAlbumImageOrientationENS_5album16ImageOrientationE", nothing),
        ("_ZN2nn2oe29BeginAutoSleepDisabledSectionEv", nothing),
        ("_ZN2nn2oe27EndAutoSleepDisabledSectionEv", nothing),
        ("_ZN2nn6applet24BeginUsingCameraResourceEv", ok),
        ("_ZN2nn6applet22EndUsingCameraResourceEv", ok),
    ] {
        r.force(n, W, f);
    }
    let _ = w64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_dedups_focus() {
        push(MSG_FOCUS);
        push(MSG_FOCUS);
        push(99);
        assert_eq!(try_pop(), Some(MSG_FOCUS));
        assert_eq!(try_pop(), Some(99));
        assert_eq!(try_pop(), None);
        assert_eq!(language_code().to_le_bytes()[..5], *b"en-US");
    }
}
