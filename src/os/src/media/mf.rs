//! Windows' Media Foundation decoders: H.264 (`CLSID_MSH264DecoderMFT`, part of every Windows 10/11) and AAC
//! (`CLSID_MSAACDecMFT`). Each decoder belongs to one thread (COM, multithreaded apartment, initialized here).
//! H.264 comes out in presentation order (the decoder reorders), NV12, cropped to the stream's display size.

use super::yuv::Picture;
use windows::core::GUID;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

fn startup() -> Result<(), String> {
    thread_local! { static COM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    COM.with(|c| {
        if !c.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            c.set(true);
        }
    });
    static MF: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    MF.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string()) }).clone()
}

fn sample(data: &[u8], time_100ns: i64) -> windows::core::Result<IMFSample> {
    unsafe {
        let buf = MFCreateMemoryBuffer(data.len().max(1) as u32)?;
        let mut p = std::ptr::null_mut();
        buf.Lock(&mut p, None, None)?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        buf.Unlock()?;
        buf.SetCurrentLength(data.len() as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(time_100ns)?;
        Ok(s)
    }
}

/// One output sample's bytes and time.
fn take_output(t: &IMFTransform) -> windows::core::Result<Option<(Vec<u8>, i64)>> {
    unsafe {
        let info = t.GetOutputStreamInfo(0)?;
        let provides = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
        let own = if provides {
            None
        } else {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(info.cbSize.max(1 << 16))?)?;
            Some(s)
        };
        let mut out = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, pSample: std::mem::ManuallyDrop::new(own), dwStatus: 0, pEvents: std::mem::ManuallyDrop::new(None) }];
        let mut status = 0u32;
        let r = t.ProcessOutput(0, &mut out, &mut status);
        let s = std::mem::ManuallyDrop::take(&mut out[0].pSample);
        let _ = std::mem::ManuallyDrop::take(&mut out[0].pEvents);
        r?;
        let Some(s) = s else { return Ok(None) };
        let time = s.GetSampleTime().unwrap_or(0);
        let b = s.ConvertToContiguousBuffer()?;
        let mut p = std::ptr::null_mut();
        let mut n = 0u32;
        b.Lock(&mut p, None, Some(&mut n))?;
        let v = std::slice::from_raw_parts(p, n as usize).to_vec();
        b.Unlock()?;
        Ok(Some((v, time)))
    }
}

pub struct H264 {
    t: IMFTransform,
    /// Coded size, stride, display size of the current output type.
    coded: (u32, u32),
    stride: u32,
    display: (u32, u32),
    bt709: Option<bool>,
    full: bool,
    pub out: std::collections::VecDeque<Picture>,
    pub errors: u32,
}

// The transform is used by its owner's thread only (the multithreaded apartment).
unsafe impl Send for H264 {}

fn frame_size(t: &IMFMediaType, key: &GUID) -> Option<(u32, u32)> {
    let v = unsafe { t.GetUINT64(key) }.ok()?;
    Some(((v >> 32) as u32, v as u32))
}

impl H264 {
    pub fn new() -> Result<H264, String> {
        startup()?;
        unsafe {
            let t: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("the H.264 decoder: {e}"))?;
            let it = MFCreateMediaType().map_err(|e| e.to_string())?;
            it.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            it.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| e.to_string())?;
            t.SetInputType(0, &it, 0).map_err(|e| format!("H.264 input type: {e}"))?;
            let mut d = H264 { t, coded: (0, 0), stride: 0, display: (0, 0), bt709: None, full: false, out: Default::default(), errors: 0 };
            d.pick_output()?;
            let _ = d.t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            let _ = d.t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            Ok(d)
        }
    }

    fn pick_output(&mut self) -> Result<(), String> {
        unsafe {
            for i in 0..32 {
                let Ok(ty) = self.t.GetOutputAvailableType(0, i) else { break };
                if ty.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                    self.t.SetOutputType(0, &ty, 0).map_err(|e| format!("H.264 output type: {e}"))?;
                    self.coded = frame_size(&ty, &MF_MT_FRAME_SIZE).unwrap_or((0, 0));
                    self.stride = ty.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(0).unsigned_abs();
                    if self.stride == 0 {
                        self.stride = self.coded.0;
                    }
                    self.display = self.coded;
                    let mut ap = [0u8; 16];
                    let mut n = 0u32;
                    if ty.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut ap, Some(&mut n)).is_ok() && n >= 16 {
                        // MFVideoArea: OffsetX (fixed 16.16), OffsetY, SIZE {cx, cy}
                        let cx = u32::from_le_bytes(ap[8..12].try_into().unwrap());
                        let cy = u32::from_le_bytes(ap[12..16].try_into().unwrap());
                        if cx > 0 && cy > 0 {
                            self.display = (cx, cy);
                        }
                    }
                    self.bt709 = match ty.GetUINT32(&MF_MT_YUV_MATRIX) {
                        Ok(1) => Some(true),
                        Ok(2) => Some(false),
                        _ => None,
                    };
                    self.full = ty.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE) == Ok(1);
                    return Ok(());
                }
            }
        }
        Err("the H.264 decoder offers no NV12 output".into())
    }

    fn collect(&mut self) {
        loop {
            match take_output(&self.t) {
                Ok(Some((b, t))) => {
                    let (cw, ch) = self.coded;
                    let (w, h) = (self.display.0.min(cw) as usize, self.display.1.min(ch) as usize);
                    let stride = self.stride as usize;
                    if w == 0 || h == 0 || b.len() < stride * ch as usize * 3 / 2 {
                        self.errors += 1;
                        continue;
                    }
                    let mut p = Picture::from_nv12(w, h, &b, &b[stride * ch as usize..], stride);
                    p.bt709 = self.bt709;
                    p.full_range = self.full;
                    p.pts = t / 10;
                    self.out.push_back(p);
                }
                Ok(None) => continue,
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    if self.pick_output().is_err() {
                        self.errors += 1;
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    }

    /// One access unit (Annex B), its time in µs; the pictures it lets out queue in `out`.
    pub fn decode(&mut self, au: &[u8], pts_us: i64) -> Result<(), String> {
        let s = sample(au, pts_us * 10).map_err(|e| e.to_string())?;
        for _ in 0..4 {
            match unsafe { self.t.ProcessInput(0, &s, 0) } {
                Ok(()) => {
                    self.collect();
                    return Ok(());
                }
                Err(e) if e.code() == MF_E_NOTACCEPTING => self.collect(),
                Err(e) => {
                    self.errors += 1;
                    return Err(e.to_string());
                }
            }
        }
        Err("the H.264 decoder takes no more input".into())
    }

    /// End of stream: every picture still held comes out.
    pub fn drain(&mut self) {
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        }
        self.collect();
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
        }
    }

    /// A seek: pictures held are dropped.
    pub fn flush(&mut self) {
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
        self.out.clear();
    }
}

/// AAC (raw frames, the AudioSpecificConfig given) to interleaved PCM16.
pub struct Aac {
    t: IMFTransform,
    pub channels: u32,
    pub rate: u32,
}

unsafe impl Send for Aac {}

impl Aac {
    pub fn new(rate: u32, channels: u32, asc: &[u8]) -> Result<Aac, String> {
        startup()?;
        unsafe {
            let t: IMFTransform = CoCreateInstance(&CLSID_MSAACDecMFT, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("the AAC decoder: {e}"))?;
            let it = MFCreateMediaType().map_err(|e| e.to_string())?;
            let e = |r: windows::core::Result<()>| r.map_err(|e| e.to_string());
            e(it.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio))?;
            e(it.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC))?;
            e(it.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate))?;
            e(it.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels))?;
            e(it.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16))?;
            e(it.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0))?;
            // HEAACWAVEINFO's tail (payload type, profile level 0xFE, struct type, reserved) + AudioSpecificConfig
            let mut ud = vec![0u8; 12];
            ud[2..4].copy_from_slice(&0xfeu16.to_le_bytes());
            ud.extend_from_slice(asc);
            e(it.SetBlob(&MF_MT_USER_DATA, &ud))?;
            t.SetInputType(0, &it, 0).map_err(|e| format!("AAC input type: {e}"))?;
            let mut ok = false;
            for i in 0..16 {
                let Ok(ty) = t.GetOutputAvailableType(0, i) else { break };
                if ty.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFAudioFormat_PCM) && ty.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE) == Ok(16) {
                    t.SetOutputType(0, &ty, 0).map_err(|e| e.to_string())?;
                    ok = true;
                    break;
                }
            }
            if !ok {
                return Err("the AAC decoder offers no 16-bit PCM".into());
            }
            let _ = t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            Ok(Aac { t, channels, rate })
        }
    }

    /// One raw AAC frame; the PCM16 bytes it gives (may be none at the start).
    pub fn decode(&mut self, frame: &[u8]) -> Result<Vec<u8>, String> {
        let s = sample(frame, 0).map_err(|e| e.to_string())?;
        unsafe { self.t.ProcessInput(0, &s, 0) }.map_err(|e| e.to_string())?;
        let mut pcm = vec![];
        while let Ok(Some((b, _))) = take_output(&self.t) {
            pcm.extend_from_slice(&b);
        }
        Ok(pcm)
    }

    pub fn flush(&mut self) {
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}

/// The AudioSpecificConfig for AAC-LC of a rate and channel count (when a stream brings none).
pub fn aac_lc_config(rate: u32, channels: u32) -> [u8; 2] {
    const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
    let fi = RATES.iter().position(|&r| r == rate).unwrap_or(3) as u8;
    [(2 << 3) | (fi >> 1), ((fi & 1) << 7) | (((channels & 15) as u8) << 3)]
}

#[cfg(test)]
mod tests {
    #[test]
    fn decoders_open() {
        let mut h = super::H264::new().expect("Windows' H.264 decoder");
        // a lone access unit delimiter: no picture, no error
        let _ = h.decode(&[0, 0, 0, 1, 9, 0xf0], 0);
        assert!(h.out.is_empty());
        let asc = super::aac_lc_config(48000, 2);
        assert!(super::Aac::new(48000, 2, &asc).is_ok());
    }
}
