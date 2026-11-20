//! The game window: a plain Win32 window on a thread of its own (nothing the game does can stall its messages),
//! handed to graphics as the present surface. Windowed or fullscreen (borderless on the window's monitor) from the
//! ini's `fullscreen`; F11 and Alt+Enter switch and keep the choice in the ini. The title shows the game, then the
//! state (FPS limit, recording, frame generation).
//!
//! Keys: F1 help and controls, F3 screenshot (captures\), F4 FPS limit 60 / unlimited, F5 diagnostic dump, F7 record
//! video, F8 full-resolution capture, F11 fullscreen, Esc quit. A held key's auto-repeat never repeats these.

use crate::keys;
use crate::WideZ;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Input::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub static HWND_: AtomicIsize = AtomicIsize::new(0);
static FULLSCREEN: AtomicBool = AtomicBool::new(false);
pub static HELP_OPEN: AtomicBool = AtomicBool::new(false);
static PLACEMENT: Mutex<Option<WINDOWPLACEMENT>> = Mutex::new(None);
static TITLE: Mutex<String> = Mutex::new(String::new());
static STATE: Mutex<[String; 3]> = Mutex::new([String::new(), String::new(), String::new()]);

pub const WM_APP_TITLE: u32 = WM_APP + 1;
pub const WM_APP_FULLSCREEN: u32 = WM_APP + 2;

pub fn hwnd() -> HWND {
    HWND_.load(Ordering::Acquire) as HWND
}

/// A state shown after the title: slot 0 the FPS limit, 1 recording, 2 frame generation ("" clears it).
pub fn set_state(slot: usize, text: &str) {
    STATE.lock().unwrap()[slot] = text.to_string();
    let h = hwnd();
    if !h.is_null() {
        unsafe { PostMessageW(h, WM_APP_TITLE, 0, 0) };
    }
}

fn title_text() -> String {
    let mut t = TITLE.lock().unwrap().clone();
    for s in STATE.lock().unwrap().iter().filter(|s| !s.is_empty()) {
        t.push_str("  |  ");
        t.push_str(s);
    }
    t
}

fn monitor_rect(h: HWND) -> RECT {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTOPRIMARY), &mut mi);
        mi.rcMonitor
    }
}

pub fn set_fullscreen(on: bool) {
    let h = hwnd();
    if h.is_null() || FULLSCREEN.load(Ordering::Acquire) == on {
        return;
    }
    unsafe {
        let st = GetWindowLongW(h, GWL_STYLE) as u32;
        if on {
            let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
            wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
            GetWindowPlacement(h, &mut wp);
            *PLACEMENT.lock().unwrap() = Some(wp);
            let r = monitor_rect(h);
            SetWindowLongW(h, GWL_STYLE, (st & !WS_OVERLAPPEDWINDOW) as i32);
            SetWindowPos(h, HWND_TOP, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOOWNERZORDER | SWP_FRAMECHANGED);
        } else {
            SetWindowLongW(h, GWL_STYLE, (st | WS_OVERLAPPEDWINDOW) as i32);
            if let Some(wp) = PLACEMENT.lock().unwrap().take() {
                SetWindowPlacement(h, &wp);
            }
            SetWindowPos(h, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED);
        }
    }
    FULLSCREEN.store(on, Ordering::Release);
    crate::flog!("window: {}", if on { "fullscreen" } else { "windowed" });
}

pub fn fullscreen() -> bool {
    FULLSCREEN.load(Ordering::Acquire)
}

/// F11 / Alt+Enter: the player's choice is kept in the ini, so the next start opens the same way.
fn toggle_fullscreen() {
    let on = !fullscreen();
    set_fullscreen(on);
    nxconfig::set("fullscreen", if on { "1" } else { "0" });
}

fn mouse_mode() -> bool {
    nxconfig::flag("mouse_right_stick")
}

/// Clips the cursor to the window while the mouse is the right stick and the game is in front.
pub fn mouse_capture() {
    let h = hwnd();
    unsafe {
        let active = mouse_mode() && !HELP_OPEN.load(Ordering::Acquire) && GetForegroundWindow() == h && IsIconic(h) == 0;
        if active {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(h, &mut r);
            MapWindowPoints(h, std::ptr::null_mut(), &mut r as *mut RECT as *mut POINT, 2);
            ClipCursor(&r);
        } else {
            ClipCursor(std::ptr::null());
        }
    }
    keys::MOUSE_DX.store(0, Ordering::Relaxed);
    keys::MOUSE_DY.store(0, Ordering::Relaxed);
}

pub fn focused() -> bool {
    let h = hwnd();
    !h.is_null() && unsafe { GetForegroundWindow() } == h && !HELP_OPEN.load(Ordering::Acquire)
}

/// From the front-end loop: player 1's keyboard and mouse, and the focus.
/// The power request while the game is in front (Turbo plat.c: the display and the system kept awake while playing):
/// ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED, and ES_CONTINUOUS alone (released) behind other windows.
pub fn awake_state(focused: bool) -> u32 {
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
    if focused {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
    } else {
        ES_CONTINUOUS
    }
}

/// Keeps the display awake while the window has the focus (set from the front-end loop's thread, which lives as long
/// as the game: the request is that thread's).
fn keep_awake(focused: bool) {
    static LAST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
    if LAST.swap(focused as u8, Ordering::Relaxed) != focused as u8 {
        unsafe { windows_sys::Win32::System::Power::SetThreadExecutionState(awake_state(focused)) };
    }
}

pub fn poll_input() {
    static MOUSE: Mutex<keys::Mouse> = Mutex::new(keys::Mouse::new());
    let f = focused();
    keep_awake(f);
    nxos::input::set_focus(f);
    // the game's focus state (oe message 15): in front 1, behind another window 2, minimised 3
    let h = hwnd();
    let st = if !h.is_null() && unsafe { IsIconic(h) } != 0 { 3 } else if f { 1 } else { 2 };
    nxos::services::oe::set_focus(st);
    // NXRS_CONTROLLER_BACKGROUND=1: the keyboard counts out of focus too (keys posted by a test script)
    static BG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let f = f || *BG.get_or_init(|| nxos::input::env_flag("NXRS_CONTROLLER_BACKGROUND"));
    nxos::input::set_suspended(HELP_OPEN.load(Ordering::Acquire));
    let held = if f { keys::HELD.load(Ordering::Acquire) } else { 0 };
    let m = if mouse_mode() { Some(MOUSE.lock().unwrap().stick(f)) } else { None };
    let (b, a) = keys::state(held, m);
    nxos::input::set_keyboard(b | crate::script_now(), a);
    home_quit();
}

/// Home held 2 s on a controller: "Quit the game?" over it (nxnative home_quit.h); A / Return quits, B / Escape /
/// Home closes it. The ini's `home_quit=0` (or `NXOS_HOME_QUIT=0`) turns it off.
fn home_quit() {
    use nxos::ui::{HomeQuit, HqPhase};
    static Q: Mutex<Option<(HomeQuit, std::time::Instant)>> = Mutex::new(None);
    if !nxconfig::flag("home_quit") || std::env::var("NXOS_HOME_QUIT").as_deref() == Ok("0") {
        return;
    }
    let mut g = Q.lock().unwrap();
    let (q, t0) = g.get_or_insert_with(|| (HomeQuit::default(), std::time::Instant::now()));
    let before = q.phase;
    let (pa, pb) = nxos::ui::pads_ab();
    let (confirm, cancel) = if before == HqPhase::Prompt { nxos::ui::keys() } else { (false, false) };
    let now = q.step(t0.elapsed().as_secs_f64(), nxos::input::home_held(), pa, pb, confirm, cancel);
    if now == before {
        return;
    }
    match now {
        HqPhase::Prompt => {
            crate::flog!("Home held: the quit prompt");
            nxos::input::set_prompt(true);
            let (title, hint) = nxos::ui_text::texts(nxos::ui_text::Prompt::HomeQuit, 0, 0);
            nxos::ui::show(&title, &hint);
        }
        HqPhase::Quit => {
            nxos::ui::hide();
            crate::flog!("Home: quit from the prompt");
            std::thread::spawn(|| crate::quit("Home"));
        }
        HqPhase::Settle => nxos::ui::hide(),
        HqPhase::Idle if before == HqPhase::Settle => nxos::input::set_prompt(false),
        _ => {}
    }
}

fn fps_limit_toggle(h: HWND) {
    let now = nxconfig::fps_limit_hz();
    let v = if now == 0 { "1" } else { "0" };
    nxconfig::set("fps_limit", v);
    let text = if v == "0" { "FPS limit: unlimited".to_string() } else { format!("FPS limit: {}", nxconfig::fps_limit_hz()) };
    crate::flog!("F4: {text}");
    let _ = h;
    set_state(0, &text);
}

pub fn diag_dump() {
    crate::flog!("F5: diagnostic dump (presents {}, the os gaps so far below)", nxos::nvn::presents());
    nxos::log::report_gaps("F5");
    let dir = crate::game().exe_dir.join("captures");
    let _ = std::fs::create_dir_all(&dir);
    let mut s = format!("presents {}\n", nxos::nvn::presents());
    for (k, w, n) in nxos::log::gaps() {
        s.push_str(&format!("{k:?}\t{w}\t{n}\n"));
    }
    s.push_str(&nxconfig::ctl::run("pads"));
    // every guest thread: what it waits for, the host function it is in, its guest frames (Turbo's F5)
    let threads = nxrt::kernel::try_get().map(|_| nxrt::kernel::dump_threads()).unwrap_or_default();
    crate::flog!("F5: guest threads
{threads}");
    s.push_str("
# guest threads
");
    s.push_str(&threads);
    // the render targets, and a pass dump of the next frame (Turbo's F5 gpu_gl_dump_rts)
    let rts = nxgfx::diag_dump();
    crate::flog!("F5: {}", rts.lines().next().unwrap_or("no graphics"));
    s.push('\n');
    s.push_str(&rts);
    let _ = std::fs::write(dir.join("threads.txt"), s);
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            if nxos::nvn::presents() == 0 {
                FillRect(dc, &ps.rcPaint, GetStockObject(BLACK_BRUSH) as HBRUSH);
            }
            EndPaint(h, &ps);
            return 0;
        }
        WM_CLOSE => {
            // NXRS_CLOSE_QUIT=0 (scripted runs): a single close is logged and ignored; a second one within 10 s
            // closes the game (whoever insists gets the window closed)
            if std::env::var("NXRS_CLOSE_QUIT").is_ok_and(|v| v.trim() == "0") {
                static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
                let mut last = LAST.lock().unwrap();
                let again = last.is_some_and(|t| t.elapsed().as_secs_f64() < 10.0);
                if !again {
                    *last = Some(std::time::Instant::now());
                    let fg = GetForegroundWindow() == h;
                    crate::flog!("window: close ignored (NXRS_CLOSE_QUIT=0; window in the foreground: {fg}); a second close within 10 s quits");
                    return 0;
                }
            }
            crate::flog!("window: closed by the user or the system");
            std::thread::spawn(|| crate::quit("the window was closed"));
            return 0;
        }
        WM_ENDSESSION => {
            if w != 0 {
                nxos::input::shutdown();
            }
        }
        WM_QUERYENDSESSION => crate::flog!("window: Windows is logging off / shutting down"),
        WM_POWERBROADCAST => {
            if w == PBT_APMSUSPEND as usize {
                crate::flog!("window: the PC is going to sleep");
            } else if w == PBT_APMRESUMEAUTOMATIC as usize {
                crate::flog!("window: the PC resumed from sleep");
            }
        }
        WM_DEVICECHANGE => nxos::input::device_changed(),
        // the mouse's left button as the touch screen (not while the mouse is the right stick)
        WM_LBUTTONDOWN | WM_MOUSEMOVE | WM_LBUTTONUP if !mouse_mode() => {
            let down = m == WM_LBUTTONDOWN || (m == WM_MOUSEMOVE && w & 1 != 0); // MK_LBUTTON
            if m == WM_LBUTTONDOWN {
                SetCapture(h);
            } else if m == WM_LBUTTONUP {
                ReleaseCapture();
            }
            if m != WM_MOUSEMOVE || down {
                let (x, y) = ((l & 0xffff) as i16 as i32, ((l >> 16) & 0xffff) as i16 as i32);
                let mut cr: RECT = std::mem::zeroed();
                GetClientRect(h, &mut cr);
                let sx = (x.max(0) as i64 * 1280 / cr.right.max(1) as i64) as i32;
                let sy = (y.max(0) as i64 * 720 / cr.bottom.max(1) as i64) as i32;
                // the two-finger mouse: Ctrl + drag pinches (a second finger mirrored about the press point),
                // Shift + drag rotates (two fingers turned about it), chosen when the button goes down
                use nxos::input::MouseTouch;
                static TWO: Mutex<(MouseTouch, (i32, i32))> = Mutex::new((MouseTouch::One, (0, 0)));
                let mut two = TWO.lock().unwrap();
                if m == WM_LBUTTONDOWN {
                    let (ctrl, shift) = (GetKeyState(VK_CONTROL as i32) < 0, GetKeyState(VK_SHIFT as i32) < 0);
                    *two = (if ctrl { MouseTouch::Pinch } else if shift { MouseTouch::Rotate } else { MouseTouch::One }, (sx, sy));
                }
                for (k, (fx, fy)) in nxos::input::mouse_fingers(two.0, two.1, (sx, sy)).into_iter().enumerate() {
                    nxos::input::set_finger(k, down, fx, fy);
                }
            }
            return 0;
        }
        WM_MOVE | WM_SETFOCUS | WM_KILLFOCUS | WM_ACTIVATEAPP => {
            if m == WM_KILLFOCUS {
                keys::HELD.store(0, Ordering::Release);
            }
            mouse_capture();
        }
        WM_SIZE => {
            mouse_capture();
            nxgfx::vk::present::window_resized();
            return 0;
        }
        WM_APP_TITLE => {
            let t = title_text().encode_wide_z();
            SetWindowTextW(h, t.as_ptr());
            return 0;
        }
        WM_APP_FULLSCREEN => {
            set_fullscreen(w != 0);
            return 0;
        }
        WM_SYSKEYDOWN | WM_KEYDOWN => {
            let repeat = l & (1 << 30) != 0;
            let vk = w as u32;
            if m == WM_SYSKEYDOWN {
                if vk == VK_RETURN as u32 {
                    if !repeat {
                        toggle_fullscreen();
                    }
                    return 0;
                }
                if vk == VK_F4 as u32 {
                    return DefWindowProcW(h, m, w, l); // Alt+F4
                }
            }
            let hot = [VK_ESCAPE, VK_F1, VK_F3, VK_F4, VK_F5, VK_F7, VK_F8, VK_F11].iter().any(|&k| k as u32 == vk);
            if hot {
                if repeat {
                    return 0;
                }
                match vk as u16 {
                    VK_ESCAPE => {
                        // NXRS_ESC_QUIT=0 (scripted runs): Esc is logged and ignored; the foreground state says
                        // whether a key press reached the window or a message was posted to it from outside
                        let fg = windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == h;
                        if std::env::var("NXRS_ESC_QUIT").is_ok_and(|v| v.trim() == "0") {
                            crate::flog!("Esc ignored (NXRS_ESC_QUIT=0; window in the foreground: {fg})");
                        } else {
                            crate::flog!("Esc: quit (window in the foreground: {fg})");
                            std::thread::spawn(|| crate::quit("Esc"));
                        }
                    }
                    VK_F11 => toggle_fullscreen(),
                    VK_F1 => crate::help::show(h),
                    VK_F3 => {
                        std::thread::spawn(crate::capture::screenshot);
                    }
                    VK_F4 => fps_limit_toggle(h),
                    VK_F5 => diag_dump(),
                    VK_F7 => crate::rec::toggle(),
                    VK_F8 => crate::capture::full_res(),
                    _ => {}
                }
                return 0;
            }
            keys::HELD.fetch_or(keys::keymap(vk), Ordering::AcqRel);
            return 0;
        }
        WM_SYSKEYUP | WM_KEYUP => {
            keys::HELD.fetch_and(!keys::keymap(w as u32), Ordering::AcqRel);
            return 0;
        }
        WM_INPUT => {
            if mouse_mode() && focused() {
                let mut raw: RAWINPUT = std::mem::zeroed();
                let mut size = std::mem::size_of::<RAWINPUT>() as u32;
                if GetRawInputData(l as HRAWINPUT, RID_INPUT, &mut raw as *mut _ as *mut _, &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32) != u32::MAX
                    && raw.header.dwType == RIM_TYPEMOUSE
                    && raw.data.mouse.usFlags as u32 & MOUSE_MOVE_ABSOLUTE as u32 == 0
                {
                    keys::MOUSE_DX.fetch_add(raw.data.mouse.lLastX, Ordering::Relaxed);
                    keys::MOUSE_DY.fetch_add(raw.data.mouse.lLastY, Ordering::Relaxed);
                }
            }
        }
        WM_SETCURSOR => {
            if (fullscreen() || (mouse_mode() && focused())) && (l & 0xffff) as u32 == HTCLIENT {
                SetCursor(std::ptr::null_mut());
                return 1;
            }
        }
        _ => {}
    }
    DefWindowProcW(h, m, w, l)
}

/// Opens the window on its own thread and gives it to graphics; returns once it exists.
pub fn open(title: &str) {
    *TITLE.lock().unwrap() = format!("{title} - nxrs  (F1 help)");
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("window".into())
        .spawn(move || unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let hi = GetModuleHandleW(std::ptr::null());
            let class = "NxrsGameWindow".encode_wide_z();
            let mut wc: WNDCLASSEXW = std::mem::zeroed();
            wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
            wc.style = CS_OWNDC;
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = hi;
            wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            wc.hIcon = LoadIconW(hi, 1 as _);
            if wc.hIcon.is_null() {
                wc.hIcon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
            }
            wc.hbrBackground = GetStockObject(BLACK_BRUSH) as HBRUSH;
            wc.lpszClassName = class.as_ptr();
            RegisterClassExW(&wc);
            // the client area: the largest 16:9 that fits three quarters of the primary monitor's work area
            let mut wa: RECT = std::mem::zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0);
            let (aw, ah) = ((wa.right - wa.left) * 3 / 4, (wa.bottom - wa.top) * 3 / 4);
            let (cw, ch) = if aw * 9 > ah * 16 { (ah * 16 / 9, ah) } else { (aw, aw * 9 / 16) };
            let mut r = RECT { left: 0, top: 0, right: cw.max(640), bottom: ch.max(360) };
            AdjustWindowRectEx(&mut r, WS_OVERLAPPEDWINDOW, 0, 0);
            let t = title_text().encode_wide_z();
            // NXRS_NO_ACTIVATE=1 (scripted runs): the window shows without taking the focus from whoever works at the PC
            let no_activate = std::env::var("NXRS_NO_ACTIVATE").is_ok_and(|v| v.trim() == "1");
            let style = if no_activate { WS_OVERLAPPEDWINDOW } else { WS_OVERLAPPEDWINDOW | WS_VISIBLE };
            let h = CreateWindowExW(0, class.as_ptr(), t.as_ptr(), style, CW_USEDEFAULT, CW_USEDEFAULT, r.right - r.left, r.bottom - r.top, std::ptr::null_mut(), std::ptr::null_mut(), hi, std::ptr::null());
            if h.is_null() {
                crate::flog!("[RED] window: CreateWindowExW failed ({})", GetLastError());
                let _ = tx.send(());
                return;
            }
            HWND_.store(h as isize, Ordering::Release);
            if no_activate {
                ShowWindow(h, SW_SHOWNOACTIVATE);
                crate::flog!("window: shown without taking the focus (NXRS_NO_ACTIVATE=1)");
            }
            let rid = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: 0, hwndTarget: h };
            RegisterRawInputDevices(&rid, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
            if nxconfig::flag("fullscreen") {
                set_fullscreen(true);
            }
            UpdateWindow(h);
            let mut cr: RECT = std::mem::zeroed();
            GetClientRect(h, &mut cr);
            crate::flog!("window: {}x{} client ({})", cr.right, cr.bottom, if fullscreen() { "fullscreen" } else { "windowed" });
            nxgfx::set_window(h as isize, hi as isize);
            crate::lsfg::launch_app();
            mouse_capture();
            let _ = tx.send(());
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            crate::quit("the window's message loop ended");
        })
        .expect("window thread");
    let _ = rx.recv();
}

#[cfg(test)]
mod tests {
    /// Turbo plat.c: display and system kept awake while playing; released when the window is behind others.
    #[test]
    fn display_kept_awake_while_focused() {
        assert_eq!(super::awake_state(true), 0x8000_0003);
        assert_eq!(super::awake_state(false), 0x8000_0000);
        // the request is accepted by Windows (the previous state comes back), and released again
        let prev = unsafe { windows_sys::Win32::System::Power::SetThreadExecutionState(super::awake_state(true)) };
        assert_ne!(prev, 0);
        assert_ne!(unsafe { windows_sys::Win32::System::Power::SetThreadExecutionState(super::awake_state(false)) }, 0);
    }
}