//! The client-drawn title bar on Windows (`titlebar.cpp`). The window keeps its native frame styles;
//! the caption strip is given back to the client area (WM_NCCALCSIZE), so snapping, the shadow, the
//! minimize / restore animation, Alt+Space and edge resizing stay native. Hit testing answers
//! HTCAPTION for the bar's empty space and HTMAXBUTTON for the maximize button (Windows 11 shows Snap
//! Layouts on that code). The UI publishes where its buttons and menus are each frame.

mod messages;
mod regions;

pub use regions::{PxRect, Regions, hit_code};

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use windows_sys::Win32::Foundation::{HWND, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::UI::Shell::SetWindowSubclass;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsZoomed, PostMessageW, SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WM_SYSCOMMAND,
};

static REGIONS: Mutex<Option<Regions>> = Mutex::new(None);
static HWND_VALUE: AtomicIsize = AtomicIsize::new(0);
static MAX_HOVER: AtomicBool = AtomicBool::new(false);
static MAX_DOWN: AtomicBool = AtomicBool::new(false);
/// Physical pixels of the frame hanging off-screen while maximized.
static INSET: AtomicU32 = AtomicU32::new(0);
static REPAINT: OnceLock<Arc<dyn Fn() + Send + Sync>> = OnceLock::new();

pub fn installed() -> bool {
    HWND_VALUE.load(Ordering::Relaxed) != 0
}
fn hwnd() -> HWND {
    HWND_VALUE.load(Ordering::Relaxed) as HWND
}
pub fn publish(regions: Regions) {
    if let Ok(mut r) = REGIONS.lock() {
        *r = Some(regions);
    }
}
fn regions() -> Option<Regions> {
    REGIONS.lock().ok().and_then(|r| r.clone())
}
/// The maximize button's hover and press, which arrive as non-client messages.
pub fn max_state() -> (bool, bool) {
    (
        MAX_HOVER.load(Ordering::Relaxed),
        MAX_DOWN.load(Ordering::Relaxed),
    )
}
pub fn top_inset_px() -> u32 {
    INSET.load(Ordering::Relaxed)
}
pub fn is_maximized() -> bool {
    // SAFETY: IsZoomed only reads the state of our own window handle.
    installed() && unsafe { IsZoomed(hwnd()) != 0 }
}
fn repaint() {
    if let Some(r) = REPAINT.get() {
        r();
    }
}
fn syscommand(command: u32) {
    // SAFETY: posts a system command to our own window.
    unsafe {
        PostMessageW(hwnd(), WM_SYSCOMMAND, command as WPARAM, 0);
    }
}
pub fn toggle_maximized() {
    syscommand(if is_maximized() {
        SC_RESTORE
    } else {
        SC_MAXIMIZE
    });
}
pub fn minimize() {
    syscommand(SC_MINIMIZE);
}

/// Takes over the caption of `raw` (the main window's HWND).
pub fn install(raw: isize, repaint: Arc<dyn Fn() + Send + Sync>, dark: bool) {
    let _ = REPAINT.set(repaint);
    HWND_VALUE.store(raw, Ordering::Relaxed);
    let flags = SWP_FRAMECHANGED
        | SWP_NOMOVE
        | SWP_NOSIZE
        | SWP_NOZORDER
        | SWP_NOACTIVATE
        | SWP_NOOWNERZORDER;
    // SAFETY: subclasses our own window (the proc lives for the program) and asks it to recompute its
    // frame.
    unsafe {
        SetWindowSubclass(hwnd(), Some(messages::subclass), 1, 0);
        SetWindowPos(hwnd(), std::ptr::null_mut(), 0, 0, 0, 0, flags);
    }
    set_dark(dark);
}

/// The dark native frame (DWMWA_USE_IMMERSIVE_DARK_MODE).
pub fn set_dark(dark: bool) {
    if !installed() {
        return;
    }
    let value: i32 = dark.into();
    // SAFETY: a documented DWM attribute on our own window, with a 4-byte value.
    unsafe {
        DwmSetWindowAttribute(hwnd(), 20, (&value as *const i32).cast(), 4);
    }
}
