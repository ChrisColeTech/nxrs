//! The screen's refresh rate matched to the game's frame rate in fullscreen (nxnative front_mac.m
//! `front_screen_match`, DM_SCREEN_MATCH): 60 fps cannot be paced evenly on a 144 or 165 Hz screen, so while the
//! game's window covers its monitor, a monitor whose rate is not a multiple of the game's is switched to the mode of
//! the same size and depth with the highest rate that is (144 -> 120 Hz for 60 fps). The switch is temporary
//! (`CDS_FULLSCREEN`: Windows puts the monitor back when the process ends, a crash included); leaving fullscreen puts
//! it back at once. A window keeps the screen's rate. `NXGFX_SCREEN_MATCH=0` keeps the rate in fullscreen too.

/// The refresh rate to switch to: the highest of `rates` (the modes of the monitor's current size and depth) that is a
/// multiple of `fps`, when `cur` is not one already. None: keep the current rate.
pub fn pick(cur: u32, rates: &[u32], fps: u32) -> Option<u32> {
    if fps == 0 || cur <= 1 || cur % fps == 0 {
        return None;
    }
    rates
        .iter()
        .copied()
        .filter(|&r| r > 1 && r % fps == 0)
        .max()
}

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_SCREEN_MATCH").as_deref() != Ok("0"))
}

/// The game's frame rate the screen is matched to: 60, or `fps_limit` above it (120, 144 ...).
fn game_fps() -> u32 {
    let l = nxconfig::int("fps_limit");
    if l > 60 {
        l as u32
    } else {
        60
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

    /// The monitor whose rate this process changed: (device name, the rate it had).
    static SWITCHED: std::sync::Mutex<Option<([u16; 32], u32)>> = std::sync::Mutex::new(None);

    fn mode(dev: &[u16; 32], i: u32) -> Option<DEVMODEW> {
        let mut m: DEVMODEW = unsafe { core::mem::zeroed() };
        m.dmSize = core::mem::size_of::<DEVMODEW>() as u16;
        (unsafe { EnumDisplaySettingsW(dev.as_ptr(), i, &mut m) } != 0).then_some(m)
    }

    fn set_rate(dev: &[u16; 32], hz: u32) -> bool {
        let Some(mut m) = mode(dev, ENUM_CURRENT_SETTINGS) else {
            return false;
        };
        m.dmDisplayFrequency = hz;
        m.dmFields = DM_DISPLAYFREQUENCY;
        unsafe {
            ChangeDisplaySettingsExW(
                dev.as_ptr(),
                &m,
                core::ptr::null_mut(),
                CDS_FULLSCREEN,
                core::ptr::null(),
            ) == DISP_CHANGE_SUCCESSFUL
        }
    }

    fn name(dev: &[u16; 32]) -> String {
        String::from_utf16_lossy(&dev[..dev.iter().position(|&c| c == 0).unwrap_or(32)])
    }

    /// Once in a while from present: the window's monitor matched while the window covers it, put back otherwise.
    pub fn tick(hwnd: isize) {
        let h = hwnd as HWND;
        let mon = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
        let mut mi: MONITORINFOEXW = unsafe { core::mem::zeroed() };
        mi.monitorInfo.cbSize = core::mem::size_of::<MONITORINFOEXW>() as u32;
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) } == 0
            || unsafe { GetWindowRect(h, &mut wr) } == 0
        {
            return;
        }
        let r = mi.monitorInfo.rcMonitor;
        let covers =
            wr.left <= r.left && wr.top <= r.top && wr.right >= r.right && wr.bottom >= r.bottom;
        let dev = mi.szDevice;
        let mut sw = SWITCHED.lock().unwrap();
        if let Some((d, old)) = *sw {
            if !covers || d != dev || !super::on() {
                set_rate(&d, old);
                crate::vklog!("screen {}: back to {old} Hz", name(&d));
                *sw = None;
            }
            return;
        }
        if !covers || !super::on() {
            return;
        }
        let Some(cur) = mode(&dev, ENUM_CURRENT_SETTINGS) else {
            return;
        };
        let mut rates = vec![];
        let mut i = 0;
        while let Some(m) = mode(&dev, i) {
            if m.dmPelsWidth == cur.dmPelsWidth
                && m.dmPelsHeight == cur.dmPelsHeight
                && m.dmBitsPerPel == cur.dmBitsPerPel
            {
                rates.push(m.dmDisplayFrequency);
            }
            i += 1;
        }
        let fps = super::game_fps();
        if let Some(hz) = super::pick(cur.dmDisplayFrequency, &rates, fps) {
            let ok = set_rate(&dev, hz);
            crate::vklog!(
                "screen {} at {} Hz {} {hz} Hz for {fps} fps in fullscreen (NXGFX_SCREEN_MATCH=0 keeps it)",
                name(&dev),
                cur.dmDisplayFrequency,
                if ok { "switched to" } else { "could not be switched to" }
            );
            if ok {
                *sw = Some((dev, cur.dmDisplayFrequency));
            }
        }
    }

    /// Puts a switched monitor back (window closing).
    pub fn restore() {
        if let Some((d, old)) = SWITCHED.lock().unwrap().take() {
            set_rate(&d, old);
        }
    }
}

#[cfg(windows)]
pub use imp::restore;
#[cfg(not(windows))]
pub fn restore() {}

/// From present: every 60 presents (and the first), the match is looked at again.
pub fn tick(hwnd: isize) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if hwnd == 0 || N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 60 != 0 {
        return;
    }
    #[cfg(windows)]
    imp::tick(hwnd);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_screen_takes_the_highest_rate_that_is_a_multiple() {
        let modes = [60, 100, 120, 144, 165];
        assert_eq!(
            super::pick(144, &modes, 60),
            Some(120),
            "144 Hz -> 120 for 60 fps"
        );
        assert_eq!(super::pick(165, &modes, 60), Some(120));
        assert_eq!(super::pick(120, &modes, 60), None, "already a multiple");
        assert_eq!(super::pick(60, &modes, 60), None);
        assert_eq!(super::pick(144, &[144, 75], 60), None, "no such mode: kept");
        assert_eq!(super::pick(165, &modes, 144), Some(144), "144 fps: 144 Hz");
        assert_eq!(
            super::pick(165, &[165, 60], 144),
            None,
            "no mode for 144 fps: kept"
        );
        assert_eq!(super::pick(144, &[144, 240], 120), Some(240));
    }
}
