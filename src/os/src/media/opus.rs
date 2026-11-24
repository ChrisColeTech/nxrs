//! libopus (third_party/opus, 1.5.2): single- and multi-stream decoders, and the Switch's packet framing.

use std::ffi::c_int;

#[repr(C)]
struct OpusDecoder {
    _p: [u8; 0],
}
#[repr(C)]
struct OpusMSDecoder {
    _p: [u8; 0],
}

extern "C" {
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    fn opus_decode(
        st: *mut OpusDecoder,
        data: *const u8,
        len: i32,
        pcm: *mut i16,
        frame_size: c_int,
        decode_fec: c_int,
    ) -> c_int;
    fn opus_decoder_ctl(st: *mut OpusDecoder, request: c_int, ...) -> c_int;
    fn opus_decoder_destroy(st: *mut OpusDecoder);
    fn opus_decoder_get_size(channels: c_int) -> c_int;
    fn opus_multistream_decoder_create(
        fs: i32,
        channels: c_int,
        streams: c_int,
        coupled: c_int,
        mapping: *const u8,
        error: *mut c_int,
    ) -> *mut OpusMSDecoder;
    fn opus_multistream_decode(
        st: *mut OpusMSDecoder,
        data: *const u8,
        len: i32,
        pcm: *mut i16,
        frame_size: c_int,
        decode_fec: c_int,
    ) -> c_int;
    fn opus_multistream_decoder_ctl(st: *mut OpusMSDecoder, request: c_int, ...) -> c_int;
    fn opus_multistream_decoder_destroy(st: *mut OpusMSDecoder);
    fn opus_multistream_decoder_get_size(streams: c_int, coupled: c_int) -> c_int;
    fn opus_get_version_string() -> *const std::ffi::c_char;
}

const OPUS_RESET_STATE: c_int = 4028;

pub fn version() -> String {
    unsafe { std::ffi::CStr::from_ptr(opus_get_version_string()) }
        .to_string_lossy()
        .into_owned()
}

enum Inner {
    One(*mut OpusDecoder),
    Multi(*mut OpusMSDecoder),
}

/// A decoder; the PCM comes out interleaved, 16-bit.
pub struct Decoder {
    inner: Inner,
    pub rate: i32,
    pub channels: i32,
    pub streams: i32,
    pub stereo: i32,
}

// libopus decoder state is plain memory; each decoder is used under its owner's lock.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(rate: i32, channels: i32) -> Result<Decoder, i32> {
        let mut e = 0;
        let d = unsafe { opus_decoder_create(rate, channels, &mut e) };
        if d.is_null() {
            return Err(e);
        }
        Ok(Decoder {
            inner: Inner::One(d),
            rate,
            channels,
            streams: 1,
            stereo: (channels == 2) as i32,
        })
    }
    pub fn multi(
        rate: i32,
        channels: i32,
        streams: i32,
        stereo: i32,
        mapping: &[u8],
    ) -> Result<Decoder, i32> {
        let mut e = 0;
        let mut m = [0u8; 256];
        let n = mapping.len().min(256);
        m[..n].copy_from_slice(&mapping[..n]);
        let d = unsafe {
            opus_multistream_decoder_create(rate, channels, streams, stereo, m.as_ptr(), &mut e)
        };
        if d.is_null() {
            return Err(e);
        }
        Ok(Decoder {
            inner: Inner::Multi(d),
            rate,
            channels,
            streams,
            stereo,
        })
    }
    pub fn size(channels: i32) -> u64 {
        unsafe { opus_decoder_get_size(channels) }.max(0) as u64
    }
    pub fn multi_size(streams: i32, stereo: i32) -> u64 {
        unsafe { opus_multistream_decoder_get_size(streams, stereo) }.max(0) as u64
    }
    pub fn reset(&mut self) {
        unsafe {
            match self.inner {
                Inner::One(d) => opus_decoder_ctl(d, OPUS_RESET_STATE),
                Inner::Multi(d) => opus_multistream_decoder_ctl(d, OPUS_RESET_STATE),
            };
        }
    }
    /// One raw Opus packet into `pcm` (interleaved, room for `pcm.len() / channels` samples a channel); the samples
    /// a channel decoded, or libopus's error.
    pub fn decode(&mut self, packet: &[u8], pcm: &mut [i16]) -> Result<usize, i32> {
        let frames = (pcm.len() / self.channels.max(1) as usize) as c_int;
        let r = unsafe {
            match self.inner {
                Inner::One(d) => opus_decode(
                    d,
                    packet.as_ptr(),
                    packet.len() as i32,
                    pcm.as_mut_ptr(),
                    frames,
                    0,
                ),
                Inner::Multi(d) => opus_multistream_decode(
                    d,
                    packet.as_ptr(),
                    packet.len() as i32,
                    pcm.as_mut_ptr(),
                    frames,
                    0,
                ),
            }
        };
        if r < 0 {
            Err(r)
        } else {
            Ok(r as usize)
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            match self.inner {
                Inner::One(d) => opus_decoder_destroy(d),
                Inner::Multi(d) => opus_multistream_decoder_destroy(d),
            }
        }
    }
}

/// The Switch's Opus framing: an 8-byte header (the packet's size and its final range, both big-endian), then the
/// packet. Returns (packet, bytes consumed).
pub fn frame(input: &[u8]) -> Option<(&[u8], usize)> {
    if input.len() < 8 {
        return None;
    }
    let n = u32::from_be_bytes(input[0..4].try_into().unwrap()) as usize;
    if n == 0 || 8 + n > input.len() {
        return None;
    }
    Some((&input[8..8 + n], 8 + n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_packet() {
        // the vendored libopus 1.5 on Windows; the system's 1.x on Linux
        #[cfg(windows)]
        assert!(version().starts_with("libopus 1.5"), "{}", version());
        #[cfg(target_os = "linux")]
        assert!(version().starts_with("libopus 1."), "{}", version());
        let mut d = Decoder::new(48000, 2).unwrap();
        // TOC 0xfc: CELT fullband 20 ms, stereo, one frame; an empty frame decodes as silence of its length
        let mut pcm = vec![0i16; 960 * 2];
        let n = d.decode(&[0xfc], &mut pcm).unwrap();
        assert_eq!(n, 960);
        let framed = [0, 0, 0, 1, 0, 0, 0, 0, 0xfc];
        let (p, used) = frame(&framed).unwrap();
        assert_eq!((p, used), (&[0xfcu8][..], 9));
        assert!(Decoder::multi(48000, 6, 4, 2, &[0, 4, 1, 2, 3, 5]).is_ok());
    }
}
