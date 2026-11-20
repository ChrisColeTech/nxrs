//! The window procedure in front of winit's: the frame, the hit test, the maximize button's
//! non-client mouse traffic and the system menu on the caption.

use super::{INSET, MAX_DOWN, MAX_HOVER, hit_code, regions, repaint, toggle_maximized};
use std::sync::atomic::Ordering;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;

/// WM_MOUSELEAVE (in the controls' headers, not windows_sys's WindowsAndMessaging).
const MOUSE_LEAVE: u32 = 0x02A3;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Shell::DefSubclassProc;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn x_of(l: LPARAM) -> i32 {
    i32::from((l & 0xffff) as i16)
}
fn y_of(l: LPARAM) -> i32 {
    i32::from(((l >> 16) & 0xffff) as i16)
}

/// SAFETY (whole module): every call is a documented Win32 call on our own window handle.
fn frame_thickness(hwnd: HWND) -> i32 {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

fn clear_max() {
    if MAX_HOVER.swap(false, Ordering::Relaxed) | MAX_DOWN.swap(false, Ordering::Relaxed) {
        repaint();
    }
}

unsafe fn hit_test(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let standard = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    if standard != HTCLIENT as LRESULT {
        return standard; // left, right and bottom resize borders
    }
    let mut pt = POINT {
        x: x_of(lparam),
        y: y_of(lparam),
    };
    let mut client = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        ScreenToClient(hwnd, &mut pt);
        GetClientRect(hwnd, &mut client);
    }
    let border = frame_thickness(hwnd);
    if unsafe { IsZoomed(hwnd) } == 0 && pt.y < border {
        let code = if pt.x < border * 2 {
            HTTOPLEFT
        } else if pt.x >= client.right - border * 2 {
            HTTOPRIGHT
        } else {
            HTTOP
        };
        return code as LRESULT;
    }
    regions().map_or(HTCLIENT, |r| hit_code(&r, pt.x, pt.y)) as LRESULT
}

unsafe fn system_menu(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let menu = GetSystemMenu(hwnd, 0);
        if menu.is_null() {
            return;
        }
        let zoomed = IsZoomed(hwnd) != 0;
        for (item, on) in [
            (SC_RESTORE, zoomed),
            (SC_MOVE, !zoomed),
            (SC_SIZE, !zoomed),
            (SC_MAXIMIZE, !zoomed),
        ] {
            EnableMenuItem(menu, item, if on { MF_ENABLED } else { MF_GRAYED });
        }
        SetMenuDefaultItem(menu, if zoomed { SC_RESTORE } else { SC_MAXIMIZE }, 0);
        let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
            TPM_RIGHTALIGN
        } else {
            TPM_LEFTALIGN
        };
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | align,
            x,
            y,
            0,
            hwnd,
            std::ptr::null(),
        );
        if command != 0 {
            PostMessageW(hwnd, WM_SYSCOMMAND, command as WPARAM, 0);
        }
    }
}

pub unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let max = wparam == HTMAXBUTTON as WPARAM;
    unsafe {
        match msg {
            WM_NCCALCSIZE if wparam != 0 => {
                // the standard frame, then the caption strip back to the client area
                let params = &mut *(lparam as *mut NCCALCSIZE_PARAMS);
                let top = params.rgrc[0].top;
                let standard = DefWindowProcW(hwnd, msg, wparam, lparam);
                params.rgrc[0].top = top;
                let wanted = if IsZoomed(hwnd) != 0 {
                    frame_thickness(hwnd).unsigned_abs()
                } else {
                    0
                };
                if INSET.swap(wanted, Ordering::Relaxed) != wanted {
                    repaint();
                }
                standard
            }
            WM_NCHITTEST => hit_test(hwnd, msg, wparam, lparam),
            // the maximize button's traffic: handled here, so DefWindowProc never paints the classic one
            WM_NCMOUSEMOVE if max => {
                if !MAX_HOVER.swap(true, Ordering::Relaxed) {
                    repaint();
                }
                let size = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
                0
            }
            WM_NCMOUSEMOVE | WM_NCMOUSELEAVE | MOUSE_LEAVE => {
                clear_max();
                DefSubclassProc(hwnd, msg, wparam, lparam)
            }
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if max => {
                MAX_HOVER.store(true, Ordering::Relaxed);
                MAX_DOWN.store(true, Ordering::Relaxed);
                repaint();
                0
            }
            WM_NCLBUTTONUP if max => {
                if MAX_DOWN.swap(false, Ordering::Relaxed) {
                    toggle_maximized();
                }
                repaint();
                0
            }
            WM_NCRBUTTONUP if wparam == HTCAPTION as WPARAM => {
                system_menu(hwnd, x_of(lparam), y_of(lparam));
                0
            }
            _ => DefSubclassProc(hwnd, msg, wparam, lparam),
        }
    }
}
