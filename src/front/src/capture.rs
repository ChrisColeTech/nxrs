//! Screenshots. F3: the window as the player sees it (the swap chain's picture through DWM, PrintWindow with
//! PW_RENDERFULLCONTENT) as `captures\screenshot_<date>_<time>.png` beside the game. F8: the game's own picture at its
//! render resolution, read back by graphics (`nxgfx::vk::shot`), as `captures\fullres_<date>_<time>.png`. The window
//! grab also feeds the F7 recorder (`grab`).

//!
//! Linux: no system call reads another window's pixels on Wayland (and X11's XGetImage reads what covers it), so the
//! window grab is the presented picture read back by graphics at the next present, the picture the window shows.

#[cfg(windows)]
use windows_sys::Win32::Foundation::*;
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::*;
#[cfg(windows)]
use windows_sys::Win32::Storage::Xps::*;
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Frame {
    pub w: u32,
    pub h: u32,
    /// BGRA, top-down, w * 4 a row.
    pub bgra: Vec<u8>,
}

/// The window's client area now: from the screen (a fast copy of what DWM shows) while the window is in front and
/// visible, else through PrintWindow (slow, but works covered or minimised).
#[cfg(windows)]
pub fn grab() -> Option<Frame> {
    let h = crate::window::hwnd();
    // Never copy from the screen: anything over the window (another app, a notification) would be captured. Only the
    // window's own content, through PrintWindow, whatever is in front of it. NXRS_SCREEN_GRAB=1 restores the screen copy.
    if !h.is_null()
        && std::env::var("NXRS_SCREEN_GRAB").as_deref() == Ok("1")
        && unsafe { GetForegroundWindow() == h && IsIconic(h) == 0 }
    {
        if let Some(f) = grab_screen(h) {
            return Some(f);
        }
    }
    grab_print()
}

#[cfg(windows)]
fn grab_screen(h: HWND) -> Option<Frame> {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(h, &mut r);
        let mut tl = POINT { x: 0, y: 0 };
        ClientToScreen(h, &mut tl);
        let (w, ht) = (r.right, r.bottom);
        if w <= 0 || ht <= 0 {
            return None;
        }
        let sdc = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(sdc);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -ht;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bm = CreateDIBSection(sdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut out = None;
        if !bm.is_null() {
            let old = SelectObject(dc, bm as _);
            if BitBlt(dc, 0, 0, w, ht, sdc, tl.x, tl.y, SRCCOPY) != 0 {
                GdiFlush();
                let mut v =
                    std::slice::from_raw_parts(bits as *const u8, (w * ht * 4) as usize).to_vec();
                for px in v.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                out = Some(Frame {
                    w: w as u32,
                    h: ht as u32,
                    bgra: v,
                });
            }
            SelectObject(dc, old);
            DeleteObject(bm as _);
        }
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), sdc);
        out
    }
}

/// The window through PrintWindow (PW_CLIENTONLY | PW_RENDERFULLCONTENT).
#[cfg(windows)]
pub fn grab_print() -> Option<Frame> {
    let h = crate::window::hwnd();
    if h.is_null() {
        return None;
    }
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(h, &mut r);
        let (w, ht) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || ht <= 0 {
            return None;
        }
        let wdc = GetDC(h);
        let dc = CreateCompatibleDC(wdc);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -ht;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bm = CreateDIBSection(wdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut out = None;
        if !bm.is_null() {
            let old = SelectObject(dc, bm as _);
            // PW_CLIENTONLY | PW_RENDERFULLCONTENT: the composed picture, a flip-model swap chain included
            if PrintWindow(h, dc, 1 | 2) != 0 {
                GdiFlush();
                let n = (w * ht * 4) as usize;
                let mut v = std::slice::from_raw_parts(bits as *const u8, n).to_vec();
                for px in v.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                out = Some(Frame {
                    w: w as u32,
                    h: ht as u32,
                    bgra: v,
                });
            }
            SelectObject(dc, old);
            DeleteObject(bm as _);
        }
        DeleteDC(dc);
        ReleaseDC(h, wdc);
        out
    }
}

pub fn write_png(path: &std::path::Path, f: &Frame) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut e = png::Encoder::new(file, f.w, f.h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    let mut wr = e.write_header().map_err(std::io::Error::other)?;
    let mut rgb = Vec::with_capacity((f.w * f.h * 3) as usize);
    for px in f.bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    wr.write_image_data(&rgb).map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn stamp() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}{:02}{:02}_{:02}{:02}{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

#[cfg(windows)]
pub fn stamp() -> String {
    unsafe {
        let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        format!(
            "{:04}{:02}{:02}_{:02}{:02}{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        )
    }
}

pub fn captures_dir() -> std::path::PathBuf {
    let d = crate::game().exe_dir.join("captures");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The window to `path` (PNG): Linux reads the presented picture back at the next present and waits for it (2 s).
#[cfg(target_os = "linux")]
pub fn screenshot_to(path: &std::path::Path) -> bool {
    let t = std::time::Instant::now();
    let _ = std::fs::remove_file(path);
    let before = nxgfx::vk::shot::count();
    nxgfx::vk::shot::request(&path.display().to_string());
    while t.elapsed() < std::time::Duration::from_secs(2) {
        if nxgfx::vk::shot::count() > before && path.is_file() {
            crate::flog!(
                "screenshot: {} ({:.0} ms)",
                path.display(),
                t.elapsed().as_secs_f64() * 1000.0
            );
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    crate::flog!("[RED] screenshot: no present within 2 s to capture");
    false
}

/// The window to `path` (PNG).
#[cfg(windows)]
pub fn screenshot_to(path: &std::path::Path) -> bool {
    let t = std::time::Instant::now();
    match grab() {
        Some(f) => match write_png(path, &f) {
            Ok(()) => {
                crate::flog!(
                    "screenshot: {} ({}x{}, {:.0} ms)",
                    path.display(),
                    f.w,
                    f.h,
                    t.elapsed().as_secs_f64() * 1000.0
                );
                true
            }
            Err(e) => {
                crate::flog!("[RED] screenshot: {}: {e}", path.display());
                false
            }
        },
        None => {
            crate::flog!("[RED] screenshot: the window could not be captured");
            false
        }
    }
}

/// F3: the game's own picture at the next present (graphics reads the presented image back), as Turbo's F3; the
/// window grab stays for the control pipe's `shot`.
pub fn screenshot() {
    let p = captures_dir().join(format!("screenshot_{}.png", stamp()));
    crate::flog!(
        "F3: the game's picture at the next present -> {}",
        p.display()
    );
    nxgfx::vk::shot::request(&p.display().to_string());
}

/// F8: the game's picture at its own resolution, from graphics at the next present.
pub fn full_res() {
    let p = captures_dir().join(format!("fullres_{}.png", stamp()));
    crate::flog!(
        "F8: full-resolution capture of the next present -> {}",
        p.display()
    );
    nxgfx::vk::shot::request(&p.display().to_string());
}
