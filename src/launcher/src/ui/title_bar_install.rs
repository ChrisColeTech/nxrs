//! Takes over the main window's caption on Windows (the native hit test keeps snapping, the shadow,
//! Snap Layouts on Windows 11); elsewhere the window is frameless and egui moves and resizes it.

use crate::services::jobs::Notify;

pub fn install(cc: &eframe::CreationContext<'_>, repaint: Notify, dark: bool) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(w) = handle.as_raw()
        {
            crate::services::platform::windows_caption::install(w.hwnd.get(), repaint, dark);
        }
    }
    #[cfg(not(windows))]
    let _ = (cc, repaint, dark);
}
