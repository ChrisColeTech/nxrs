//! F1: the help and controls page, over the game (Turbo's design): four options changed with the arrows, Enter or a
//! click (right stick from the mouse or I J K L, mouse direction, mouse sensitivity 10%..500%, smooth audio slowdown),
//! saved into the ini, and the keyboard and hotkey tables. Esc, F1 or Backspace goes back; the game reads no pads
//! meanwhile and a fullscreen game is windowed while it is open.

use crate::WideZ;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const W: i32 = 760;
const H: i32 = 620;
const ROW: i32 = 46;
const LIST_Y: i32 = 84;
const NOPT: usize = 4;
static SEL: AtomicUsize = AtomicUsize::new(0);

const NAMES: [&str; NOPT] = ["Right stick", "Mouse direction", "Mouse sensitivity", "Smooth audio slowdown"];
const INVERSION: [&str; 4] = ["Normal", "Left / right inverted", "Up / down inverted", "Both inverted"];
const KEYS: [(&str, &str); 7] = [
    ("D-Pad", "Arrows"),
    ("Left stick", "W A S D"),
    ("Right stick", "I J K L (or the mouse)"),
    ("A  B  X  Y", "X/Space  Z  C  V"),
    ("L  R  ZL  ZR", "Q  E  1  3"),
    ("+  -", "Enter  Backspace"),
    ("Controller", "Xbox / PlayStation / Switch"),
];
const FKEYS: [(&str, &str); 8] = [
    ("F1", "This page"),
    ("F3", "Screenshot"),
    ("F4", "FPS limit on/off"),
    ("F5", "Diagnostic dump"),
    ("F7", "Record video"),
    ("F8", "Full resolution capture"),
    ("F11", "Fullscreen"),
    ("Esc", "Quit the game"),
];

fn value(i: usize) -> String {
    match i {
        0 => if nxconfig::flag("mouse_right_stick") { "Mouse" } else { "I J K L keys" }.into(),
        1 => INVERSION[(nxconfig::int("mouse_invert") & 3) as usize].into(),
        2 => {
            let p = (nxconfig::f32("mouse_sensitivity") * 100.0).round() as i32;
            format!("{}{p}%{}", if p > 10 { "<  " } else { "" }, if p < 500 { "  >" } else { "" })
        }
        _ => if nxconfig::flag("audio_slowdown") { "On" } else { "Off" }.into(),
    }
}

fn change(i: usize, dir: i32) {
    match i {
        0 => nxconfig::set("mouse_right_stick", if nxconfig::flag("mouse_right_stick") { "0" } else { "1" }),
        1 => nxconfig::set("mouse_invert", &((nxconfig::int("mouse_invert") + if dir < 0 { 3 } else { 1 }) & 3).to_string()),
        2 => {
            let p = (nxconfig::f32("mouse_sensitivity") * 100.0).round() as i32;
            let n = if dir == 0 { 100 } else { (p + dir * 10).clamp(10, 500) };
            nxconfig::set("mouse_sensitivity", &format!("{:.2}", n as f32 / 100.0));
        }
        _ => nxconfig::set("audio_slowdown", if nxconfig::flag("audio_slowdown") { "0" } else { "1" }),
    }
    crate::window::mouse_capture();
}

unsafe fn font(px: i32, weight: i32) -> HFONT {
    let face = "Segoe UI".encode_wide_z();
    CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, DEFAULT_PITCH as u32, face.as_ptr())
}
unsafe fn rect(dc: HDC, x: i32, y: i32, w: i32, h: i32, c: u32) {
    let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
    let b = CreateSolidBrush(c);
    FillRect(dc, &r, b);
    DeleteObject(b as _);
}
unsafe fn text(dc: HDC, f: HFONT, t: &str, x: i32, y: i32, w: i32, h: i32, fmt: u32, c: u32) {
    let mut r = RECT { left: x, top: y, right: x + w, bottom: y + h };
    SelectObject(dc, f as _);
    SetTextColor(dc, c);
    let mut s: Vec<u16> = t.encode_utf16().collect();
    DrawTextW(dc, s.as_mut_ptr(), s.len() as i32, &mut r, fmt | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
}
const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | g << 8 | b << 16
}

unsafe fn paint(h: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let wdc = BeginPaint(h, &mut ps);
    let dc = CreateCompatibleDC(wdc);
    let bm = CreateCompatibleBitmap(wdc, W, H);
    let obm = SelectObject(dc, bm as _);
    SetBkMode(dc, TRANSPARENT as i32);
    let (fh, fb, fs) = (font(30, 400), font(21, 400), font(17, 400));
    let (bg, fg, dim, accent, sep) = (rgb(235, 235, 235), rgb(45, 45, 45), rgb(130, 130, 130), rgb(8, 175, 200), rgb(205, 205, 205));
    rect(dc, 0, 0, W, H, bg);
    text(dc, fh, "Controls", 40, 18, 500, 48, DT_LEFT, fg);
    rect(dc, 20, 66, W - 40, 1, fg);
    for (i, n) in NAMES.iter().enumerate() {
        let y = LIST_Y + i as i32 * ROW;
        if i > 0 {
            rect(dc, 40, y, W - 80, 1, sep);
        }
        text(dc, fb, n, 64, y, 360, ROW, DT_LEFT, fg);
        text(dc, fb, &value(i), W - 380, y, 316, ROW, DT_RIGHT, accent);
    }
    let y = LIST_Y + SEL.load(Ordering::Relaxed) as i32 * ROW;
    let pen = CreatePen(PS_SOLID, 4, accent);
    SelectObject(dc, pen as _);
    SelectObject(dc, GetStockObject(NULL_BRUSH));
    RoundRect(dc, 38, y - 1, W - 38, y + ROW + 2, 10, 10);
    DeleteObject(pen as _);
    let ky = LIST_Y + NOPT as i32 * ROW + 22;
    text(dc, fs, "KEYBOARD", 64, ky, 300, 24, DT_LEFT, dim);
    text(dc, fs, "KEYS", 410, ky, 300, 24, DT_LEFT, dim);
    for (i, (a, b)) in KEYS.iter().enumerate() {
        let y = ky + 28 + i as i32 * 28;
        text(dc, fs, a, 64, y, 120, 28, DT_LEFT, fg);
        text(dc, fs, b, 170, y, 220, 28, DT_LEFT, accent);
    }
    for (i, (a, b)) in FKEYS.iter().enumerate() {
        let y = ky + 28 + i as i32 * 28;
        text(dc, fs, a, 410, y, 50, 28, DT_LEFT, accent);
        text(dc, fs, b, 460, y, 260, 28, DT_LEFT, fg);
    }
    rect(dc, 30, H - 60, W - 60, 1, fg);
    text(dc, fs, "Arrows / Enter or a click: change    Esc or F1: back to the game", 40, H - 46, W - 80, 32, DT_LEFT, dim);
    BitBlt(wdc, 0, 0, W, H, dc, 0, 0, SRCCOPY);
    for f in [fh, fb, fs] {
        DeleteObject(f as _);
    }
    SelectObject(dc, obm);
    DeleteObject(bm as _);
    DeleteDC(dc);
    EndPaint(h, &ps);
}

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            paint(h);
            return 0;
        }
        WM_KEYDOWN => {
            let k = w as u16;
            let sel = SEL.load(Ordering::Relaxed);
            if k == VK_ESCAPE || k == VK_F1 || k == VK_BACK || k == b'Z' as u16 {
                DestroyWindow(h);
                return 0;
            }
            if k == VK_UP || k == b'W' as u16 {
                SEL.store(sel.saturating_sub(1), Ordering::Relaxed);
            } else if k == VK_DOWN || k == b'S' as u16 {
                SEL.store((sel + 1).min(NOPT - 1), Ordering::Relaxed);
            } else if k == VK_LEFT || k == b'A' as u16 {
                change(sel, -1);
            } else if k == VK_RIGHT || k == b'D' as u16 || k == VK_RETURN || k == VK_SPACE || k == b'X' as u16 {
                change(sel, 1);
            } else if k == b'R' as u16 && sel == 2 {
                change(2, 0);
            }
            InvalidateRect(h, std::ptr::null(), 0);
            return 0;
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let (x, y) = ((l & 0xffff) as i16 as i32, (l >> 16 & 0xffff) as i16 as i32);
            let i = (y - LIST_Y) / ROW;
            if y >= LIST_Y && (0..NOPT as i32).contains(&i) && x >= 38 && x < W - 38 {
                let i = i as usize;
                if i != SEL.load(Ordering::Relaxed) {
                    SEL.store(i, Ordering::Relaxed);
                    InvalidateRect(h, std::ptr::null(), 0);
                }
                if m != WM_MOUSEMOVE {
                    change(i, if m == WM_RBUTTONDOWN || (i == 2 && x < W / 2) { -1 } else { 1 });
                    InvalidateRect(h, std::ptr::null(), 0);
                }
            } else if m == WM_LBUTTONDOWN && y > H - 60 {
                DestroyWindow(h);
            }
            return 0;
        }
        WM_CLOSE => {
            DestroyWindow(h);
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(h, m, w, l)
}

/// Shows the page over `parent` and returns when it closes (runs on the window thread, its own message loop).
pub fn show(parent: HWND) {
    use crate::window::{fullscreen, set_fullscreen, HELP_OPEN};
    unsafe {
        let fs = fullscreen();
        HELP_OPEN.store(true, Ordering::Release);
        crate::keys::HELD.store(0, Ordering::Release);
        crate::window::mouse_capture();
        if fs {
            set_fullscreen(false);
        }
        let hi = GetModuleHandleW(std::ptr::null());
        let class = "NxrsControls".encode_wide_z();
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(proc_);
        wc.hInstance = hi;
        wc.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        let mut pr: RECT = std::mem::zeroed();
        GetWindowRect(parent, &mut pr);
        let mut r = RECT { left: 0, top: 0, right: W, bottom: H };
        AdjustWindowRect(&mut r, WS_CAPTION | WS_SYSMENU, 0);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let (x, y) = (((pr.left + pr.right - ww) / 2).max(0), ((pr.top + pr.bottom - wh) / 2).max(0));
        let title = "nxrs - Controls".encode_wide_z();
        let h = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_CAPTION | WS_SYSMENU, x, y, ww, wh, parent, std::ptr::null_mut(), hi, std::ptr::null());
        crate::flog!("F1: the controls page");
        EnableWindow(parent, 0);
        ShowWindow(h, SW_SHOW);
        SetFocus(h);
        let mut msg: MSG = std::mem::zeroed();
        while IsWindow(h) != 0 && GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        EnableWindow(parent, 1);
        SetForegroundWindow(parent);
        if fs {
            set_fullscreen(true);
        }
        HELP_OPEN.store(false, Ordering::Release);
        crate::keys::HELD.store(0, Ordering::Release);
        crate::window::mouse_capture();
    }
}
