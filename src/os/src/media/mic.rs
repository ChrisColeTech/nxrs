//! The capture device for AudioIn: the system's default input (WASAPI, through cpal), resampled to 48 kHz and
//! written as PCM16 into the buffers the game appended, which go back (released) once full.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Tests: the PC's capture device hidden, to check the answer without one.
#[cfg(test)]
pub static HIDE_DEVICE: AtomicBool = AtomicBool::new(false);

/// The default capture device's name, or None when the PC has none.
pub fn device() -> Option<String> {
    use cpal::traits::HostTrait;
    #[cfg(test)]
    if HIDE_DEVICE.load(Ordering::SeqCst) {
        return None;
    }
    let host = cpal::default_host();
    let d = host.default_input_device()?;
    // Linux: ALSA names a "default" capture PCM even with no sound server or card behind it; a device is there only
    // when it can be opened (its default configuration answers)
    #[cfg(target_os = "linux")]
    {
        use cpal::traits::DeviceTrait;
        d.default_input_config().ok()?;
    }
    Some(d.to_string())
}

struct Buf {
    tag: u64,
    ptr: u64,
    bytes: u64,
    filled: u64,
    ch: u32,
}

type Hook = Box<dyn Fn() + Send + Sync>;

pub struct Capture {
    ch: u32,
    q: Mutex<VecDeque<Buf>>,
    released: Mutex<VecDeque<u64>>,
    hook: Mutex<Option<Hook>>,
    running: AtomicBool,
    stream: Mutex<Option<SendStream>>,
    // resampler state (device rate -> 48 kHz)
    rs: Mutex<(f64, [f32; 2], [f32; 2])>,
}

#[allow(dead_code)]
struct SendStream(cpal::Stream);
// The stream is only created, played and dropped under the Capture's lock.
unsafe impl Send for SendStream {}

pub fn open(ch: u32) -> Result<Arc<Capture>, String> {
    device().ok_or("no capture device")?;
    Ok(Arc::new(Capture {
        ch: ch.clamp(1, 2),
        q: Mutex::new(VecDeque::new()),
        released: Mutex::new(VecDeque::new()),
        hook: Mutex::new(None),
        running: AtomicBool::new(false),
        stream: Mutex::new(None),
        rs: Mutex::new((1.0, [0.0; 2], [0.0; 2])),
    }))
}

impl Capture {
    pub fn on_release(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }
    pub fn append(&self, tag: u64, ptr: u64, bytes: u64, ch: u32) {
        self.q.lock().unwrap().push_back(Buf { tag, ptr, bytes, filled: 0, ch: ch.clamp(1, 2) });
    }
    pub fn released(&self) -> Option<u64> {
        self.released.lock().unwrap().pop_front()
    }
    pub fn pending(&self) -> usize {
        self.q.lock().unwrap().len()
    }
    pub fn start(self: &Arc<Self>) -> Result<(), String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let host = cpal::default_host();
        let dev = host.default_input_device().ok_or("no capture device")?;
        let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
        let fmt = cfg.sample_format();
        let config: cpal::StreamConfig = cfg.into();
        let dch = config.channels as usize;
        let step = config.sample_rate as f64 / 48000.0;
        let me = self.clone();
        let err = |e: cpal::Error| crate::log!("audio: capture stream error: {e}");
        macro_rules! build {
            ($t:ty) => {
                dev.build_input_stream(
                    config.clone(),
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        let frames: Vec<[f32; 2]> = data
                            .chunks(dch)
                            .map(|f| {
                                let l = <f32 as cpal::FromSample<$t>>::from_sample_(f[0]);
                                let r = if dch > 1 { <f32 as cpal::FromSample<$t>>::from_sample_(f[1]) } else { l };
                                [l, r]
                            })
                            .collect();
                        me.feed(&frames, step);
                    },
                    err,
                    None,
                )
            };
        }
        let s = match fmt {
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::I32 => build!(i32),
            cpal::SampleFormat::U16 => build!(u16),
            f => return Err(format!("capture sample format {f:?}")),
        }
        .map_err(|e| e.to_string())?;
        s.play().map_err(|e| e.to_string())?;
        *self.stream.lock().unwrap() = Some(SendStream(s));
        Ok(())
    }
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        *self.stream.lock().unwrap() = None;
    }
    pub fn close(&self) {
        self.stop();
        self.q.lock().unwrap().clear();
    }

    /// Device frames in; 48 kHz frames out into the queued buffers.
    fn feed(&self, frames: &[[f32; 2]], step: f64) {
        let mut out48 = Vec::with_capacity((frames.len() as f64 / step) as usize + 2);
        {
            let mut rs = self.rs.lock().unwrap();
            for f in frames {
                rs.1 = rs.2;
                rs.2 = *f;
                // 48 kHz positions between the previous device frame and this one
                while rs.0 < 1.0 {
                    let p = rs.0 as f32;
                    out48.push([rs.1[0] + (rs.2[0] - rs.1[0]) * p, rs.1[1] + (rs.2[1] - rs.1[1]) * p]);
                    rs.0 += step;
                }
                rs.0 -= 1.0;
            }
        }
        let mut done = false;
        let mut q = self.q.lock().unwrap();
        let mut i = 0;
        while i < out48.len() {
            let Some(b) = q.front_mut() else { break };
            let fb = 2 * b.ch as u64;
            while i < out48.len() && b.filled + fb <= b.bytes {
                let f = out48[i];
                let s = |x: f32| (x.clamp(-1.0, 1.0) * 32767.0) as i16;
                if b.ch == 1 {
                    crate::guest::w16(b.ptr + b.filled, s((f[0] + f[1]) * 0.5) as u16);
                } else {
                    crate::guest::w16(b.ptr + b.filled, s(f[0]) as u16);
                    crate::guest::w16(b.ptr + b.filled + 2, s(f[1]) as u16);
                }
                b.filled += fb;
                i += 1;
            }
            if b.filled + fb > b.bytes {
                let b = q.pop_front().unwrap();
                crate::guest::w64(b.tag + 0x18, b.filled);
                self.released.lock().unwrap().push_back(b.tag);
                done = true;
            }
        }
        drop(q);
        if done {
            if let Some(h) = self.hook.lock().unwrap().as_ref() {
                h();
            }
        }
        let _ = self.ch;
    }
}
