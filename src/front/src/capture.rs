//! Screenshots. F3: the window as the player sees it (the swap chain's picture through DWM, PrintWindow with
//! PW_RENDERFULLCONTENT) as `captures\screenshot_<date>_<time>.png` beside the game. F8: the game's own picture at its
//! render resolution, read back by graphics (`nxgfx::vk::shot`), as `captures\fullres_<date>_<time>.png`. The window
//! grab also feeds the F7 recorder (`grab`).

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::Xps::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Frame {
    pub w: u32,
    pub h: u32,
    /// BGRA, top-down, w * 4 a row.
    pub bgra: Vec<u8>,
}

/// The window's client area now: from the screen (a fast copy of what DWM shows) while the window is in front and
/// visible, else through PrintWindow (slow, but works covered or minimised).
pub fn grab() -> Option<Frame> {
    let h = crate::window::hwnd();
    if !h.is_null() && unsafe { GetForegroundWindow() == h && IsIconic(h) == 0 } {
        if let Some(f) = grab_screen(h) {
            return Some(f);
        }
    }
    grab_print()
}

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
                let mut v = std::slice::from_raw_parts(bits as *const u8, (w * ht * 4) as usize).to_vec();
                for px in v.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                out = Some(Frame { w: w as u32, h: ht as u32, bgra: v });
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
                out = Some(Frame { w: w as u32, h: ht as u32, bgra: v });
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

pub fn stamp() -> String {
    unsafe {
        let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        format!("{:04}{:02}{:02}_{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
}

pub fn captures_dir() -> std::path::PathBuf {
    let d = crate::game().exe_dir.join("captures");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The window to `path` (PNG).
pub fn screenshot_to(path: &std::path::Path) -> bool {
    let t = std::time::Instant::now();
    match grab() {
        Some(f) => match write_png(path, &f) {
            Ok(()) => {
                crate::flog!("screenshot: {} ({}x{}, {:.0} ms)", path.display(), f.w, f.h, t.elapsed().as_secs_f64() * 1000.0);
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
    crate::flog!("F3: the game's picture at the next present -> {}", p.display());
    nxgfx::vk::shot::request(&p.display().to_string());
}

/// F8: the game's picture at its own resolution, from graphics at the next present.
pub fn full_res() {
    let p = captures_dir().join(format!("fullres_{}.png", stamp()));
    crate::flog!("F8: full-resolution capture of the next present -> {}", p.display());
    nxgfx::vk::shot::request(&p.display().to_string());
}

