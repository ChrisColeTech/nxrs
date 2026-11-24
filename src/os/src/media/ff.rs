//! H.264 and AAC on Linux: FFmpeg's libavcodec through `ffdec.c` (a C shim compiled against the installed headers, so
//! no FFmpeg struct layout is written down here). The same API as Windows' Media Foundation decoders (`mf.rs`), which
//! `media::mf` names on Linux: H.264 access units (Annex B) in, pictures out in presentation order, cropped to the
//! stream's display size, with its colour description; raw AAC frames in, interleaved PCM16 out.

use super::yuv::Picture;
use std::ffi::{c_char, c_int, c_void, CStr};

#[repr(C)]
struct FfPicture {
    w: c_int,
    h: c_int,
    layout: c_int,
    plane: [*const u8; 3],
    stride: [c_int; 3],
    pts: i64,
    matrix: c_int,
    full_range: c_int,
}

impl Default for FfPicture {
    fn default() -> Self {
        // SAFETY: integers and null pointers
        unsafe { std::mem::zeroed() }
    }
}

extern "C" {
    fn nxff_open(
        codec: c_int,
        extra: *const u8,
        extra_len: c_int,
        rate: c_int,
        channels: c_int,
        err: *mut c_char,
        errlen: c_int,
    ) -> *mut c_void;
    fn nxff_close(d: *mut c_void);
    fn nxff_send(d: *mut c_void, data: *const u8, len: c_int, pts: i64) -> c_int;
    fn nxff_picture(d: *mut c_void, out: *mut FfPicture) -> c_int;
    fn nxff_audio(d: *mut c_void, out: *mut i16, cap: c_int, channels: c_int) -> c_int;
    fn nxff_flush(d: *mut c_void);
    fn nxff_version() -> *const c_char;
    fn nxff_error(e: c_int, buf: *mut c_char, n: c_int);
}

/// FFmpeg's version ("6.1.1-3ubuntu5"), for the logs.
pub fn version() -> String {
    unsafe { CStr::from_ptr(nxff_version()) }
        .to_string_lossy()
        .into_owned()
}

fn error(e: c_int) -> String {
    let mut b = [0 as c_char; 128];
    unsafe { nxff_error(e, b.as_mut_ptr(), b.len() as c_int) };
    unsafe { CStr::from_ptr(b.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn open(codec: c_int, extra: &[u8], rate: u32, channels: u32) -> Result<*mut c_void, String> {
    let mut err = [0 as c_char; 256];
    let d = unsafe {
        nxff_open(
            codec,
            extra.as_ptr(),
            extra.len() as c_int,
            rate as c_int,
            channels as c_int,
            err.as_mut_ptr(),
            err.len() as c_int,
        )
    };
    if d.is_null() {
        let what = if codec == 0 {
            "the H.264 decoder"
        } else {
            "the AAC decoder"
        };
        return Err(format!(
            "{what} (FFmpeg): {}",
            unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy()
        ));
    }
    Ok(d)
}

pub struct H264 {
    d: *mut c_void,
    pub out: std::collections::VecDeque<Picture>,
    pub errors: u32,
    /// Units with no slice (parameter sets, SEI) sent with the next unit that has one.
    held: Vec<u8>,
}

// The decoder is used by its owner's thread only.
unsafe impl Send for H264 {}

impl Drop for H264 {
    fn drop(&mut self) {
        unsafe { nxff_close(self.d) };
    }
}

impl H264 {
    pub fn new() -> Result<H264, String> {
        Ok(H264 {
            d: open(0, &[], 0, 0)?,
            out: Default::default(),
            errors: 0,
            held: Vec::new(),
        })
    }

    fn collect(&mut self) {
        loop {
            let mut p = FfPicture::default();
            match unsafe { nxff_picture(self.d, &mut p) } {
                1 => {}
                0 => return,
                _ => {
                    self.errors += 1;
                    return;
                }
            }
            let (w, h) = (p.w.max(0) as usize, p.h.max(0) as usize);
            if w == 0
                || h == 0
                || p.stride[0] <= 0
                || p.stride[1] <= 0
                || (p.layout == 0 && p.stride[2] <= 0)
            {
                self.errors += 1;
                continue;
            }
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            let plane = |i: usize, rows: usize| unsafe {
                std::slice::from_raw_parts(
                    p.plane[i],
                    p.stride[i] as usize * (rows - 1)
                        + if i == 0 {
                            w
                        } else {
                            cw * if p.layout == 1 { 2 } else { 1 }
                        },
                )
            };
            let mut pic = if p.layout == 1 {
                // one stride for both planes in Picture::from_nv12: libavcodec gives NV12 planes equal strides
                Picture::from_nv12(
                    w,
                    h,
                    plane(0, h),
                    plane(1, ch),
                    p.stride[0].min(p.stride[1]) as usize,
                )
            } else {
                Picture::from_i420(
                    w,
                    h,
                    plane(0, h),
                    p.stride[0] as usize,
                    plane(1, ch),
                    p.stride[1] as usize,
                    plane(2, ch),
                    p.stride[2] as usize,
                )
            };
            pic.bt709 = match p.matrix {
                1 => Some(true),
                5 | 6 => Some(false),
                _ => None,
            };
            pic.full_range = p.full_range != 0;
            pic.pts = p.pts;
            self.out.push_back(pic);
        }
    }

    /// One access unit (Annex B), its time in µs; the pictures it lets out queue in `out`. A unit with no slice (a lone
    /// delimiter, parameter sets) gives no picture and no error, as with Media Foundation (libavcodec refuses it).
    pub fn decode(&mut self, au: &[u8], pts_us: i64) -> Result<(), String> {
        if !has_slice(au) {
            if self.held.len() < 1 << 20 {
                self.held.extend_from_slice(au);
            }
            return Ok(());
        }
        let joined;
        let au = if self.held.is_empty() {
            au
        } else {
            self.held.extend_from_slice(au);
            joined = std::mem::take(&mut self.held);
            &joined[..]
        };
        for _ in 0..4 {
            match unsafe { nxff_send(self.d, au.as_ptr(), au.len() as c_int, pts_us) } {
                0 => {
                    self.collect();
                    return Ok(());
                }
                1 => self.collect(),
                e => {
                    self.errors += 1;
                    return Err(error(e));
                }
            }
        }
        Err("the H.264 decoder takes no more input".into())
    }

    /// End of stream: every picture still held comes out (and the decoder takes a new stream after it).
    pub fn drain(&mut self) {
        unsafe { nxff_send(self.d, std::ptr::null(), 0, 0) };
        self.collect();
        unsafe { nxff_flush(self.d) };
    }

    /// A seek: pictures held are dropped.
    pub fn flush(&mut self) {
        unsafe { nxff_flush(self.d) };
        self.out.clear();
        self.held.clear();
    }
}

/// Whether an Annex B unit holds a coded slice (NAL types 1 and 5), not only delimiters, parameter sets or SEI.
fn has_slice(au: &[u8]) -> bool {
    au.windows(4)
        .any(|w| w[..3] == [0, 0, 1] && matches!(w[3] & 0x1f, 1 | 5))
}

/// AAC (raw frames, the AudioSpecificConfig given) to interleaved PCM16 in `channels` channels.
pub struct Aac {
    d: *mut c_void,
    pub channels: u32,
    pub rate: u32,
}

unsafe impl Send for Aac {}

impl Drop for Aac {
    fn drop(&mut self) {
        unsafe { nxff_close(self.d) };
    }
}

impl Aac {
    pub fn new(rate: u32, channels: u32, asc: &[u8]) -> Result<Aac, String> {
        Ok(Aac {
            d: open(1, asc, rate, channels)?,
            channels,
            rate,
        })
    }

    /// One raw AAC frame; the PCM16 bytes it gives (may be none at the start).
    pub fn decode(&mut self, frame: &[u8]) -> Result<Vec<u8>, String> {
        let mut pcm = vec![];
        let mut buf = vec![0i16; 8192 * self.channels.max(1) as usize];
        let mut sent = false;
        for _ in 0..4 {
            let r = unsafe { nxff_send(self.d, frame.as_ptr(), frame.len() as c_int, 0) };
            if r < 0 {
                return Err(error(r));
            }
            loop {
                let n = unsafe {
                    nxff_audio(
                        self.d,
                        buf.as_mut_ptr(),
                        buf.len() as c_int,
                        self.channels as c_int,
                    )
                };
                if n == 0 {
                    break;
                }
                if n < 0 {
                    return Err(if n == -2 {
                        "an AAC frame larger than 8192 samples".into()
                    } else {
                        error(n)
                    });
                }
                for s in &buf[..n as usize] {
                    pcm.extend_from_slice(&s.to_le_bytes());
                }
            }
            if r == 0 {
                sent = true;
                break;
            }
        }
        if !sent {
            return Err("the AAC decoder takes no more input".into());
        }
        Ok(pcm)
    }

    pub fn flush(&mut self) {
        unsafe { nxff_flush(self.d) };
    }
}

/// The AudioSpecificConfig for AAC-LC of a rate and channel count (when a stream brings none).
pub fn aac_lc_config(rate: u32, channels: u32) -> [u8; 2] {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let fi = RATES.iter().position(|&r| r == rate).unwrap_or(3) as u8;
    [
        (2 << 3) | (fi >> 1),
        ((fi & 1) << 7) | (((channels & 15) as u8) << 3),
    ]
}

#[cfg(test)]
mod tests {
    use crate::services::codec_video::tests::{clip, fnv, h264_access_units, reference, H, W};

    /// The x264 test clip (B-frames: pictures leave in display order) decodes to ffmpeg's reference pictures.
    #[test]
    fn h264_clip_matches_the_reference() {
        let mut h = super::H264::new().expect("FFmpeg's H.264 decoder");
        let _ = h.decode(&[0, 0, 0, 1, 9, 0xf0], 0);
        assert!(
            h.out.is_empty(),
            "a lone access unit delimiter gives no picture"
        );
        let aus = h264_access_units(&clip("clip_h264.h264"));
        let mut got = vec![];
        for (i, au) in aus.iter().enumerate() {
            h.decode(au, i as i64 * 41_666).unwrap();
            got.extend(h.out.drain(..));
        }
        h.drain();
        got.extend(h.out.drain(..));
        assert_eq!(h.errors, 0);
        let want = reference("clip_h264.h264");
        assert_eq!(got.len(), want.len());
        for (i, p) in got.iter().enumerate() {
            assert_eq!((p.w as u32, p.h as u32), (W, H));
            let mut nv12 = p.y.clone();
            nv12.extend_from_slice(&p.uv);
            assert_eq!(fnv(&nv12), want[i], "picture {i}");
        }
        // each picture carries its own unit's time (units go in decode order, so the times are those of the units)
        let mut t: Vec<i64> = got.iter().map(|p| p.pts).collect();
        t.sort();
        assert_eq!(
            t,
            (0..want.len() as i64)
                .map(|i| i * 41_666)
                .collect::<Vec<_>>()
        );
    }

    /// A 0.2 s 440 Hz stereo tone (ffmpeg's AAC-LC encoder, ADTS): 1024 samples a frame, two channels, not silent.
    #[test]
    fn aac_tone_decodes() {
        let b = clip("tone_aac.adts");
        let asc = super::aac_lc_config(48000, 2);
        let mut a = super::Aac::new(48000, 2, &asc).expect("FFmpeg's AAC decoder");
        let (mut o, mut frames, mut pcm) = (0usize, 0, vec![]);
        while o + 7 <= b.len() {
            assert_eq!(b[o], 0xff);
            let len = ((b[o + 3] as usize & 3) << 11)
                | (b[o + 4] as usize) << 3
                | (b[o + 5] as usize) >> 5;
            let hdr = if b[o + 1] & 1 == 0 { 9 } else { 7 };
            pcm.extend(a.decode(&b[o + hdr..o + len]).unwrap());
            o += len;
            frames += 1;
        }
        assert!(frames >= 9);
        assert_eq!(
            pcm.len(),
            frames * 1024 * 2 * 2,
            "1024 stereo PCM16 samples a frame"
        );
        let peak = pcm
            .chunks(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs())
            .max()
            .unwrap();
        assert!(peak > 3000, "a tone, not silence: peak {peak}");
        assert!(!super::version().is_empty());
    }
}
