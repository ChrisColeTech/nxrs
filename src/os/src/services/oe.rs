//! nn::oe (and the bits of nn::applet / am an application reaches through it): the operation mode, focus, the
//! notification queue, performance configuration, language, launch parameter and the switches the system keeps.
//! The host runs one application in the foreground. The operation mode is the `docked` setting (Turbo d5b36a1: TV
//! mode by default, so games draw for a television; `NXOS_OPMODE` overrides), changed live with the console's
//! operation-mode notification; the language is the `language` setting (Turbo's numbers; `NXOS_LANG` overrides).

use crate::guest::{w32, w64, Args};
use crate::hle::{Registry, RES_OK};
use nxrt::cpu::Cpu;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

pub const MSG_FOCUS: u32 = 15;
pub const MSG_OPMODE: u32 = 30;
pub const MSG_PERFMODE: u32 = 31;

static OPMODE: AtomicI32 = AtomicI32::new(-1);

pub fn opmode() -> i32 {
    let m = OPMODE.load(Ordering::Relaxed);
    if m >= 0 {
        return m;
    }
    let v = std::env::var("NXOS_OPMODE").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| if crate::options::get("docked").trim() == "0" { 0 } else { 1 });
    OPMODE.store(v, Ordering::Relaxed);
    crate::log!("oe: {} (the docked setting), language {}", if v == 1 { "docked, TV mode" } else { "handheld mode" }, language());
    v
}

/// The console docked (1) or taken out (0) while the game runs: the game gets the operation-mode and
/// performance-mode notifications, as on the console.
pub fn set_opmode(m: i32) {
    if opmode() == m {
        return;
    }
    OPMODE.store(m, Ordering::Relaxed);
    crate::log!("oe: operation mode {} (the docked setting changed)", if m == 1 { "docked" } else { "handheld" });
    push(MSG_OPMODE);
    push(MSG_PERFMODE);
}

pub fn language() -> String {
    std::env::var("NXOS_LANG").unwrap_or_else(|_| crate::options::language_code(&crate::options::get("language")))
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
// TryPopLaunchParameter(size_t* out, void* buffer, size_t size) -> bool: the parameter the program that started
// this one passed to ExecuteProgram (`NXRS_LAUNCH_PARAM`, a file), once
oefn!(launch_param, |a| {
    static POPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let data = std::env::var_os("NXRS_LAUNCH_PARAM").and_then(|p| std::fs::read(p).ok());
    match data {
        Some(d) if !POPPED.swap(true, Ordering::Relaxed) => {
            let n = d.len().min(a.x(2) as usize);
            crate::guest::write_bytes(a.x(1), &d[..n]);
            crate::guest::out(a.x(0), n as u64);
            crate::log!("oe: launch parameter of {} bytes from program {}", d.len(), previous_program());
            1
        }
        _ => {
            crate::guest::out(a.x(0), 0u64);
            0
        }
    }
});
oefn!(prev_program, |a| { previous_program() as i64 as u64 });

/// The program that started this one with ExecuteProgram (`NXRS_PREV_PROGRAM`), else -1.
fn previous_program() -> i32 {
    std::env::var("NXRS_PREV_PROGRAM").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(-1)
}

/// This build's program index and the executable of program `index`: program 0 is `<name>.exe`, program N
/// `<name>_p<N>.exe` in the same folder (`nxrs-build build --program N -o <name>_p<N>`).
fn program_exe(index: i32) -> Option<(i32, std::path::PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_string_lossy().into_owned();
    let (base, cur) = match stem.rsplit_once("_p") {
        Some((b, n)) if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => (b.to_string(), n.parse().unwrap_or(0)),
        _ => (stem.clone(), 0),
    };
    let name = if index == 0 { base } else { format!("{base}_p{index}") };
    Some((cur, exe.with_file_name(format!("{name}.exe"))))
}
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

// ---- the Switch 2's (and newer SDKs') am / apm / gs calls the host answers itself (nxnative hle_oe.c)

static NOSLEEP: AtomicI32 = AtomicI32::new(0);
static CPU_OC: AtomicI32 = AtomicI32::new(0);
oefn!(zero_fn, |a| { 0 });
oefn!(gfx_scale, |a| { std::env::var("NXOS_GFX_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(0) });
oefn!(nosleep_begin, |a| {
    NOSLEEP.fetch_add(1, Ordering::Relaxed);
    0
});
oefn!(nosleep_end, |a| {
    let _ = NOSLEEP.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| Some((v - 1).max(0)));
    0
});
oefn!(nosleep_q, |a| { (NOSLEEP.load(Ordering::Relaxed) > 0) as u64 });
oefn!(cpu_oc, |a| { CPU_OC.load(Ordering::Relaxed) as u64 });
oefn!(set_cpu_oc, |a| {
    CPU_OC.store(a.x(0) as u8 as i32, Ordering::Relaxed);
    0
});
oefn!(startup_logo_msg, |a| {
    push(MSG_STARTUP_LOGO);
    0
});
oefn!(frame_length, |a| {
    crate::log_once!("gs frame length", "gs::SetGraphicsFrameLengthMode {} (the host paces by the present interval)", a.x(0));
    0
});
oefn!(demo_exit, |a| {
    crate::log_once!("demo exit", "[RED] oe: ExitApplicationAndGoBackToInStoreDemoMenu: ignored (no in-store demo menu)");
    0
});
// ExecuteProgram(int index, const void* param, size_t size): this program ends and program `index` of the same
// application starts with the parameter (a successor process, `program_exe`; its log goes to the same output, and the
// harness follows it by the `exec:` line)
oefn!(execute_program, |a| {
    let (index, param, size) = (a.i(0), a.x(1), a.x(2));
    crate::log::report_gaps("ExecuteProgram");
    match program_exe(index) {
        Some((cur, exe)) if exe.is_file() => {
            let pfile = std::env::temp_dir().join(format!("nxrs_launch_{}_{index}.bin", std::process::id()));
            let bytes = if param != 0 && size != 0 { crate::guest::bytes(param, size).to_vec() } else { vec![] };
            let _ = std::fs::write(&pfile, &bytes);
            let mut cmd = std::process::Command::new(&exe);
            cmd.args(std::env::args_os().skip(1))
                .current_dir(exe.parent().unwrap())
                .env_remove("NXRS_DATA")
                .env_remove("NXRT_EXEFS")
                .env("NXRS_LAUNCH_PARAM", &pfile)
                .env("NXRS_PREV_PROGRAM", cur.to_string());
            match cmd.spawn() {
                Ok(child) => {
                    crate::log!("exec: program {cur} -> program {index} ({} bytes of parameters): {} started as process {}", bytes.len(), exe.display(), child.id());
                }
                Err(e) => crate::log!("[RED] oe::ExecuteProgram {index}: {} did not start: {e}", exe.display()),
            }
        }
        Some((_, exe)) => crate::log!("[RED] oe::ExecuteProgram {index} ({size} bytes of parameters): {} is not built (nxrs-build build --program {index}); the run ends here", exe.display()),
        None => crate::log!("[RED] oe::ExecuteProgram {index}: no executable path; the run ends here"),
    }
    nxrt::process::exit(0);
});
pub const MSG_STARTUP_LOGO: u32 = 95;

pub fn register_switch2(r: &mut Registry) {
    const W: &str = "am / apm / gs (Switch 2 SDKs): answered by the host";
    for (n, f) in [
        ("_ZN2nn2oe23GetCurrentGraphicsScaleEv", gfx_scale as unsafe fn(*mut Cpu)),
        ("_ZN2nn2oe42SetGraphicsScaleChangedNotificationEnabledEb", zero_fn),
        ("_ZN2nn2gs3sdk26SetGraphicsFrameLengthModeENS1_23GraphicsFrameLengthModeE", frame_length),
        ("_ZN2nn2oe13MuteRecordingEv", zero_fn),
        ("_ZN2nn2oe15UnmuteRecordingEv", zero_fn),
        ("_ZN2nn2oe17SetCopyrightImageEPKvmiiiiNS0_16WindowOriginModeE", zero_fn),
        ("_ZN2nn2oe22SetCopyrightVisibilityEb", zero_fn),
        ("_ZN2nn2oe30InitializeCopyrightFrameBufferEPvm", zero_fn),
        ("_ZN2nn2oe32BeginPresentPhotographedContentsEv", zero_fn),
        ("_ZN2nn2oe30EndPresentPhotographedContentsEv", zero_fn),
        ("_ZN2nn2oe17FinishStartupLogoEv", zero_fn),
        ("_ZN2nn2oe35EnableStartupLogoDisappearedMessageEv", startup_logo_msg),
        ("_ZN2nn2oe29BeginAutoSleepDisabledSectionEv", nosleep_begin),
        ("_ZN2nn2oe27EndAutoSleepDisabledSectionEv", nosleep_end),
        ("_ZN2nn2oe19IsAutoSleepDisabledEv", nosleep_q),
        ("_ZN2nn2oe21IsCpuOverclockEnabledEv", cpu_oc),
        ("_ZN2nn2oe22SetCpuOverclockEnabledEb", set_cpu_oc),
        ("_ZN2nn2oe24IsInStoreDemoModeEnabledEv", zero_fn),
        ("_ZN2nn2oe40SendServerMaintenanceOverlayNotificationENS_4time9PosixTimeES2_", zero_fn),
        ("_ZN2nn2oe41ExitApplicationAndGoBackToInStoreDemoMenuEv", demo_exit),
        ("_ZN2nn2oe14ExecuteProgramEiPKvm", execute_program),
    ] {
        r.force(n, W, f);
    }
}

pub fn register(r: &mut Registry) {
    register_switch2(r);
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
