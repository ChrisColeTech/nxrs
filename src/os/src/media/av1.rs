//! AV1 with dav1d (third_party/dav1d, 1.5.5), driven so:
//! - frame threads: a temporal unit of a hierarchical stream holds several frames, so with max_frame_delay above 1
//!   they decode side by side and a picture comes out later than its unit went in; each unit's time rides with it;
//! - one dav1d_get_picture for each dav1d_send_data (a second in a row is dav1d's drain);
//! - a drain that has begun goes on to its end (EAGAIN) before the next unit;
//! - the picture as 8-bit 4:2:0: luma copied or shifted down from 10 / 12 bits, chroma subsampled from 4:2:2 and
//!   4:4:4 by the even rows and columns, grey for 4:0:0.

use super::yuv::Picture;
use std::ffi::{c_char, c_int, c_void, CStr};

const EAGAIN: c_int = -11;

// Dav1dSettings is filled by dav1d_default_settings; the host sets three of its fields.
const S_THREADS: usize = 0;
const S_FRAME_DELAY: usize = 4;
const S_ALL_LAYERS: usize = 16;
// Dav1dPicture: seq_hdr, frame_hdr, data[3], stride[2], p {w, h, layout, bpc}, m {timestamp, ...}
const P_SEQ: usize = 0;
const P_DATA: usize = 16;
const P_STRIDE: usize = 40;
const P_W: usize = 56;
const P_H: usize = 60;
const P_LAYOUT: usize = 64;
const P_BPC: usize = 68;
const P_TS: usize = 72;
// Dav1dData: data, sz, ref, m {timestamp, ...}
const D_SZ: usize = 8;
const D_TS: usize = 24;

extern "C" {
    fn dav1d_default_settings(s: *mut u8);
    fn dav1d_open(c: *mut *mut c_void, s: *const u8) -> c_int;
    fn dav1d_send_data(c: *mut c_void, d: *mut u8) -> c_int;
    fn dav1d_get_picture(c: *mut c_void, p: *mut u8) -> c_int;
    fn dav1d_picture_unref(p: *mut u8);
    fn dav1d_data_create(d: *mut u8, sz: usize) -> *mut u8;
    fn dav1d_data_unref(d: *mut u8);
    fn dav1d_flush(c: *mut c_void);
    fn dav1d_close(c: *mut *mut c_void);
    fn dav1d_version() -> *const c_char;
}

pub fn version() -> String {
    unsafe { CStr::from_ptr(dav1d_version()) }
        .to_string_lossy()
        .into_owned()
}

#[repr(C, align(16))]
struct Blob([u8; 512]);

fn rd<T: Copy>(b: &[u8], o: usize) -> T {
    unsafe { std::ptr::read_unaligned(b.as_ptr().add(o) as *const T) }
}

pub struct Av1 {
    c: *mut c_void,
    fed: bool,
    pub bad_units: u32,
    pub bad_frames: u32,
    pub last_error: i32,
    pub threads: i32,
}

// dav1d's context is used by its owner's thread only (dav1d's own worker threads are its business).
unsafe impl Send for Av1 {}

impl Av1 {
    pub fn new(threads: i32, frames: i32) -> Result<Av1, i32> {
        let threads = threads.max(1);
        let frames = frames.clamp(1, threads);
        let mut s = Blob([0; 512]);
        unsafe { dav1d_default_settings(s.0.as_mut_ptr()) };
        s.0[S_THREADS..S_THREADS + 4].copy_from_slice(&threads.to_le_bytes());
        s.0[S_FRAME_DELAY..S_FRAME_DELAY + 4].copy_from_slice(&frames.to_le_bytes());
        s.0[S_ALL_LAYERS..S_ALL_LAYERS + 4].copy_from_slice(&0i32.to_le_bytes());
        let mut c = std::ptr::null_mut();
        let e = unsafe { dav1d_open(&mut c, s.0.as_ptr()) };
        if e != 0 {
            return Err(e);
        }
        Ok(Av1 {
            c,
            fed: false,
            bad_units: 0,
            bad_frames: 0,
            last_error: 0,
            threads,
        })
    }

    fn picture(p: &[u8]) -> Picture {
        let (w, h) = (rd::<i32>(p, P_W) as usize, rd::<i32>(p, P_H) as usize);
        let layout = rd::<i32>(p, P_LAYOUT);
        let sh = rd::<i32>(p, P_BPC) - 8;
        let data: [*const u8; 3] = [rd(p, P_DATA), rd(p, P_DATA + 8), rd(p, P_DATA + 16)];
        let stride: [isize; 2] = [rd(p, P_STRIDE), rd(p, P_STRIDE + 8)];
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut pic = Picture {
            w,
            h,
            y: vec![0; w * h],
            uv: vec![128; cw * 2 * ch],
            ..Default::default()
        };
        let sample = |plane: *const u8, row: usize, stride: isize, x: usize| -> u8 {
            unsafe {
                let r = plane.offset(row as isize * stride);
                if sh > 0 {
                    (std::ptr::read_unaligned(r.add(2 * x) as *const u16) >> sh) as u8
                } else {
                    *r.add(x)
                }
            }
        };
        for r in 0..h {
            for x in 0..w {
                pic.y[r * w + x] = sample(data[0], r, stride[0], x);
            }
        }
        if layout != 0 {
            let (sx, sy) = (layout != 3, layout == 1); // I420 1, I422 2, I444 3
            for r in 0..ch {
                let row = if sy { r } else { 2 * r };
                for x in 0..cw {
                    let c = if sx { x } else { 2 * x };
                    pic.uv[r * cw * 2 + 2 * x] = sample(data[1], row, stride[1], c);
                    pic.uv[r * cw * 2 + 2 * x + 1] = sample(data[2], row, stride[1], c);
                }
            }
        }
        let seq: *const u8 = rd(p, P_SEQ);
        if !seq.is_null() {
            let mtrx = unsafe { std::ptr::read_unaligned(seq.add(24) as *const i32) };
            pic.bt709 = match mtrx {
                1 => Some(true),
                5 | 6 => Some(false),
                _ => None,
            };
            pic.full_range = unsafe { *seq.add(33) } != 0;
        }
        pic.pts = rd(p, P_TS);
        pic
    }

    /// One dav1d_get_picture: Some(picture), or None (none now); an error counts a bad frame.
    fn take(&mut self, out: &mut Vec<Picture>) -> Result<bool, i32> {
        let mut p = Blob([0; 512]);
        let g = unsafe { dav1d_get_picture(self.c, p.0.as_mut_ptr()) };
        if g == 0 {
            out.push(Self::picture(&p.0));
            unsafe { dav1d_picture_unref(p.0.as_mut_ptr()) };
            return Ok(true);
        }
        if g == EAGAIN {
            return Ok(false);
        }
        self.bad_frames += 1;
        self.last_error = g;
        Err(g)
    }

    /// One temporal unit (OBUs in the low-overhead format) with its time; the pictures that came out.
    pub fn feed(&mut self, data: &[u8], pts: i64) -> Vec<Picture> {
        let mut got = vec![];
        if data.is_empty() {
            return got;
        }
        let mut d = Blob([0; 512]);
        let buf = unsafe { dav1d_data_create(d.0.as_mut_ptr(), data.len()) };
        if buf.is_null() {
            return got;
        }
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len()) };
        d.0[D_TS..D_TS + 8].copy_from_slice(&pts.to_le_bytes());
        self.fed = true;
        for _ in 0..64 {
            if rd::<usize>(&d.0, D_SZ) == 0 {
                break;
            }
            let r = unsafe { dav1d_send_data(self.c, d.0.as_mut_ptr()) };
            if r != 0 && r != EAGAIN {
                self.bad_units += 1;
                self.last_error = r;
                break;
            }
            let _ = self.take(&mut got);
        }
        if rd::<usize>(&d.0, D_SZ) != 0 {
            unsafe { dav1d_data_unref(d.0.as_mut_ptr()) };
        }
        got
    }

    /// The pictures dav1d still holds, to the drain's end.
    pub fn drain(&mut self) -> Vec<Picture> {
        let mut got = vec![];
        if !self.fed {
            return got;
        }
        for _ in 0..64 {
            match self.take(&mut got) {
                Ok(false) => {
                    self.fed = false;
                    break;
                }
                _ => {}
            }
        }
        got
    }

    pub fn flush(&mut self) {
        unsafe { dav1d_flush(self.c) };
        self.fed = false;
    }
}

impl Drop for Av1 {
    fn drop(&mut self) {
        unsafe { dav1d_close(&mut self.c) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn opens() {
        // the vendored dav1d 1.5 on Windows; the system's (1.x, the same API and structs) on Linux
        #[cfg(windows)]
        assert!(super::version().starts_with("1.5"), "{}", super::version());
        #[cfg(target_os = "linux")]
        assert!(super::version().starts_with("1."), "{}", super::version());
        let mut d = super::Av1::new(2, 2).unwrap();
        assert!(d.feed(&[0x12, 0x00, 0x0a, 0x0b], 0).is_empty());
        let _ = d.drain();
    }
}
