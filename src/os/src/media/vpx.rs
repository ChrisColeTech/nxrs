//! libvpx's VP8 and VP9 decoders (third_party/vpx: libvpx 1.14.0, decoders only, single-threaded build).

use super::yuv::Picture;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

#[repr(C)]
struct Ctx {
    name: *const c_char,
    iface: *mut c_void,
    err: c_int,
    err_detail: *const c_char,
    init_flags: i64,
    config: *const c_void,
    priv_: *mut c_void,
}

#[repr(C)]
struct Cfg {
    threads: c_uint,
    w: c_uint,
    h: c_uint,
}

#[repr(C)]
struct Image {
    fmt: c_int,
    cs: c_int,
    range: c_int,
    w: c_uint,
    h: c_uint,
    bit_depth: c_uint,
    d_w: c_uint,
    d_h: c_uint,
    r_w: c_uint,
    r_h: c_uint,
    x_chroma_shift: c_uint,
    y_chroma_shift: c_uint,
    planes: [*const u8; 4],
    stride: [c_int; 4],
    bps: c_int,
    user_priv: *mut c_void,
    img_data: *mut u8,
    img_data_owner: c_int,
    self_allocd: c_int,
    fb_priv: *mut c_void,
}

const ABI: c_int = 3 + 4 + 5; // VPX_DECODER_ABI_VERSION
const FMT_I420: c_int = 0x100 | 2;
const FMT_HBD: c_int = 0x800;
const CS_UNKNOWN: c_int = 0;
const CS_BT601: c_int = 1;
const CS_BT709: c_int = 2;
const CS_SMPTE170: c_int = 3;

extern "C" {
    fn vpx_codec_vp8_dx() -> *mut c_void;
    fn vpx_codec_vp9_dx() -> *mut c_void;
    fn vpx_codec_dec_init_ver(ctx: *mut Ctx, iface: *mut c_void, cfg: *const Cfg, flags: i64, ver: c_int) -> c_int;
    fn vpx_codec_decode(ctx: *mut Ctx, data: *const u8, n: c_uint, user: *mut c_void, deadline: i64) -> c_int;
    fn vpx_codec_get_frame(ctx: *mut Ctx, iter: *mut *const c_void) -> *const Image;
    fn vpx_codec_destroy(ctx: *mut Ctx) -> c_int;
    fn vpx_codec_version_str() -> *const c_char;
    fn vpx_codec_err_to_string(e: c_int) -> *const c_char;
    fn vpx_codec_error_detail(ctx: *const Ctx) -> *const c_char;
}

fn cs(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

pub fn version() -> String {
    cs(unsafe { vpx_codec_version_str() })
}

/// A VP8 or VP9 decoder.
pub struct Vpx {
    ctx: Box<Ctx>,
    pub name: &'static str,
}

// libvpx's decoder state is used by one thread at a time (its owner's).
unsafe impl Send for Vpx {}

impl Vpx {
    pub fn vp8(w: u32, h: u32) -> Result<Vpx, String> {
        Vpx::new(unsafe { vpx_codec_vp8_dx() }, "VP8", w, h)
    }
    pub fn vp9(w: u32, h: u32) -> Result<Vpx, String> {
        Vpx::new(unsafe { vpx_codec_vp9_dx() }, "VP9", w, h)
    }
    fn new(iface: *mut c_void, name: &'static str, w: u32, h: u32) -> Result<Vpx, String> {
        let mut ctx = Box::new(Ctx { name: std::ptr::null(), iface: std::ptr::null_mut(), err: 0, err_detail: std::ptr::null(), init_flags: 0, config: std::ptr::null(), priv_: std::ptr::null_mut() });
        let cfg = Cfg { threads: 1, w, h };
        let e = unsafe { vpx_codec_dec_init_ver(&mut *ctx, iface, &cfg, 0, ABI) };
        if e != 0 {
            return Err(cs(unsafe { vpx_codec_err_to_string(e) }));
        }
        Ok(Vpx { ctx, name })
    }
    /// One frame's packet (a VP9 superframe included); the last picture it shows, if any.
    pub fn decode(&mut self, data: &[u8], pts: i64) -> Result<Option<Picture>, String> {
        let e = unsafe { vpx_codec_decode(&mut *self.ctx, data.as_ptr(), data.len() as c_uint, std::ptr::null_mut(), 0) };
        if e != 0 {
            return Err(format!("{} {}", cs(unsafe { vpx_codec_err_to_string(e) }), cs(unsafe { vpx_codec_error_detail(&*self.ctx) })));
        }
        let mut it: *const c_void = std::ptr::null();
        let mut last: *const Image = std::ptr::null();
        loop {
            let im = unsafe { vpx_codec_get_frame(&mut *self.ctx, &mut it) };
            if im.is_null() {
                break;
            }
            last = im;
        }
        if last.is_null() {
            return Ok(None);
        }
        let im = unsafe { &*last };
        if im.fmt & FMT_HBD != 0 || im.fmt != FMT_I420 {
            return Err(format!("picture format {:#x} is not 8-bit 4:2:0", im.fmt));
        }
        let (w, h) = (im.d_w as usize, im.d_h as usize);
        let (ys, us, vs) = (im.stride[0] as usize, im.stride[1] as usize, im.stride[2] as usize);
        let ch = h.div_ceil(2);
        let y = unsafe { std::slice::from_raw_parts(im.planes[0], ys * (h - 1) + w) };
        let u = unsafe { std::slice::from_raw_parts(im.planes[1], us * (ch - 1) + w.div_ceil(2)) };
        let v = unsafe { std::slice::from_raw_parts(im.planes[2], vs * (ch - 1) + w.div_ceil(2)) };
        let mut p = Picture::from_i420(w, h, y, ys, u, us, v, vs);
        p.bt709 = match im.cs {
            CS_BT709 => Some(true),
            CS_BT601 | CS_SMPTE170 => Some(false),
            CS_UNKNOWN => None,
            _ => None,
        };
        p.full_range = im.range == 1;
        p.pts = pts;
        Ok(Some(p))
    }
}

impl Drop for Vpx {
    fn drop(&mut self) {
        unsafe { vpx_codec_destroy(&mut *self.ctx) };
    }
}

/// A VP9 key frame's size from its uncompressed header (VP9 spec 6.2).
pub fn key_frame_size(p: &[u8]) -> Option<(u32, u32)> {
    let mut bit = 0usize;
    let mut rb = |k: usize| -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..k {
            let b = *p.get(bit >> 3)?;
            v = v << 1 | ((b >> (7 - (bit & 7))) & 1) as u32;
            bit += 1;
        }
        Some(v)
    };
    if rb(2)? != 2 {
        return None;
    }
    let lo = rb(1)?;
    let hi = rb(1)?;
    let prof = hi << 1 | lo;
    if prof == 3 {
        rb(1)?;
    }
    if rb(1)? != 0 || rb(1)? != 0 {
        return None; // show_existing_frame, or not a key frame
    }
    rb(2)?;
    if rb(24)? != 0x49_8342 {
        return None;
    }
    if prof >= 2 {
        rb(1)?;
    }
    if rb(3)? != 7 {
        rb(1)?;
        if prof == 1 || prof == 3 {
            rb(3)?;
        }
    } else if prof == 1 || prof == 3 {
        rb(1)?;
    }
    let w = rb(16)? + 1;
    let h = rb(16)? + 1;
    Some((w, h))
}

/// A VP8 key frame's size (RFC 6386 9.1): the 3-byte frame tag (bit 0 clear on a key frame), the start code
/// 9d 01 2a, then 14-bit width and height (little-endian, the top 2 bits are the scale).
pub fn vp8_key_frame_size(p: &[u8]) -> Option<(u32, u32)> {
    if p.len() < 10 || p[0] & 1 != 0 || p[3..6] != [0x9d, 0x01, 0x2a] {
        return None;
    }
    let w = u16::from_le_bytes([p[6], p[7]]) as u32 & 0x3fff;
    let h = u16::from_le_bytes([p[8], p[9]]) as u32 & 0x3fff;
    (w > 0 && h > 0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    #[test]
    fn opens() {
        // Windows links the bundled libvpx 1.14; Linux uses the system's (1.14 or newer)
        #[cfg(windows)]
        assert!(super::version().starts_with("v1.14"), "{}", super::version());
        #[cfg(not(windows))]
        assert!(super::version().starts_with("v1."), "{}", super::version());
        let mut d = super::Vpx::vp9(64, 64).unwrap();
        assert!(d.decode(&[0u8; 4], 0).is_err(), "garbage is refused, not a black picture");
    }

    #[test]
    fn opens_vp8() {
        let mut d = super::Vpx::vp8(64, 64).unwrap();
        assert_eq!(d.name, "VP8");
        assert!(d.decode(&[0u8; 4], 0).is_err(), "garbage is refused, not a black picture");
        // a key frame header claiming 1280x720 (scale bits set) with no partition behind it
        let kf = [0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a, 0x00, 0x45, 0xd0, 0x42];
        assert_eq!(super::vp8_key_frame_size(&kf), Some((1280, 720)));
        assert!(d.decode(&kf, 0).is_err(), "a truncated key frame is refused");
        assert_eq!(super::vp8_key_frame_size(&[0x11, 0x02, 0x00, 0x9d, 0x01, 0x2a, 0x00, 0x05, 0xd0, 0x02]), None, "inter frame");
    }
}
