//! audout:u, audin:u and aud:u over IPC, for SDK 1.x clients (later SDKs are taken over by name, `audio.rs`): the
//! same sound card stream and capture device the by-name AudioOut / AudioIn use.
//! - audout:u: ListAudioOuts ("DeviceOut"), OpenAudioOut (48 kHz only, a rate of 0 meaning 48 kHz; 1-2 channels
//!   play as stereo, more as six folded to stereo); IAudioOut: state, Start / Stop (Stop gives every queued buffer
//!   back), Append (the client's AudioOutBuffer: data +0x08, data size +0x18, data offset +0x20; tagged by the
//!   client's tag), the buffer event (signalled as buffers are released), GetReleased, Contains, the buffer count,
//!   the played sample count, Flush, the volume. The Auto forms are the same commands with X / C buffers.
//! - audin:u: ListAudioIns names the capture device when the PC has one ("BuiltInHeadset"), else none and an open
//!   fails, as audin answers on a console without a microphone; IAudioIn: state, Start / Stop, Append (captured
//!   into the client's buffer), the buffer event, GetReleased, Contains, the count.
//! - aud:u: command 2 hands out the audio renderer manager (`audren_ipc::Manager`).

use crate::ipc::{self, Ctx, Service};
use crate::kobj::Event;
use crate::media::{mic, sink};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const RATE: u32 = 48000;
const FMT_PCM16: u32 = 2;
const STARTED: u32 = 0;
const STOPPED: u32 = 1;
/// audout ResultInvalidSampleRate / ResultOperationFailed / ResultBufferCountReached.
const R_BAD_RATE: u32 = 0x699;
const R_NO_DEVICE: u32 = 153 | 1 << 9;
const R_TOO_MANY: u32 = 153 | 8 << 9;
const QMAX: usize = 32;

/// The (rate, channels) an open asks for, checked: the rate given back and the channel count it plays.
fn open_params(rate: u32, ch: u16) -> Result<(u32, u32), u32> {
    if rate != 0 && rate != RATE {
        return Err(R_BAD_RATE);
    }
    Ok((RATE, if ch <= 2 { 2 } else { 6 }))
}

/// An AudioOutBuffer / AudioInBuffer the client passes: (data address, data size) from +0x08, +0x18, +0x20.
fn buffer_span(b: &[u8]) -> Option<(u64, u64)> {
    if b.len() < 0x28 {
        return None;
    }
    let r = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
    let (ptr, size, off) = (r(8), r(0x18), r(0x20));
    (ptr != 0 && size >= off).then(|| (ptr + off, size - off))
}

// ---- audout:u

struct AudOut;

impl Service for AudOut {
    fn name(&self) -> String {
        "audout:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // ListAudioOuts(Auto)
            0 | 2 => {
                let mut b = vec![0u8; 0x100];
                b[..9].copy_from_slice(b"DeviceOut");
                let n = if ctx.buf_out(0).size >= 0x100 { ctx.write_out(0, &b) / 0x100 } else { 0 };
                ctx.push_u32(n as u32);
                0
            }
            // OpenAudioOut(Auto)(rate, channels, reserved, aruid; name in, name out)
            1 | 3 => {
                let (rate, ch) = match open_params(ctx.u32(0), ctx.arg::<u16>(4)) {
                    Ok(v) => v,
                    Err(r) => {
                        crate::log!("[RED] audout:u: OpenAudioOut at {} Hz: 48 kHz only", ctx.u32(0));
                        return r;
                    }
                };
                let name = ctx.read_in(0);
                if !name.is_empty() && !name.starts_with(b"DeviceOut") && name[0] != 0 {
                    return R_NO_DEVICE;
                }
                let mut b = vec![0u8; 0x100];
                b[..9].copy_from_slice(b"DeviceOut");
                ctx.write_out(0, &b);
                static N: AtomicU64 = AtomicU64::new(0);
                let id = N.fetch_add(1, Ordering::Relaxed);
                let stream = sink::open(&format!("audout:u #{id}"));
                let event = Event::new();
                let ev = event;
                stream.on_release(move || ev.signal());
                crate::log!("audout:u: AudioOut #{id} open, {ch} channels at {rate} Hz (IPC)");
                for v in [rate, ch, FMT_PCM16, STOPPED] {
                    ctx.push_u32(v);
                }
                ctx.object(IAudioOut { stream, ch, state: STOPPED, volume: 1.0, event, played: 0, sizes: vec![] });
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

struct IAudioOut {
    stream: Arc<sink::Stream>,
    ch: u32,
    state: u32,
    volume: f32,
    event: Event,
    /// Frames of the buffers released so far, and each queued buffer's (tag, frames).
    played: u64,
    sizes: Vec<(u64, u64)>,
}

impl IAudioOut {
    fn released(&mut self, room: usize) -> Vec<u64> {
        let mut v = vec![];
        while v.len() < room {
            let Some(t) = self.stream.released() else { break };
            if let Some(i) = self.sizes.iter().position(|s| s.0 == t) {
                self.played += self.sizes.remove(i).1;
            }
            v.push(t);
        }
        v
    }
}

impl Service for IAudioOut {
    fn name(&self) -> String {
        "IAudioOut".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            0 => ctx.push_u32(self.state),
            1 => {
                self.state = STARTED;
                self.stream.start();
            }
            2 => {
                if self.state == STARTED {
                    self.stream.stop();
                }
                self.state = STOPPED;
            }
            // AppendAudioOutBuffer(Auto)(tag; buffer)
            3 | 7 => {
                if self.stream.blocks() >= QMAX {
                    return R_TOO_MANY;
                }
                let tag = ctx.u64(0);
                let Some((ptr, size)) = buffer_span(&ctx.read_in(0)) else { return R_NO_DEVICE };
                let b = crate::guest::bytes(ptr, size & !1);
                let pcm: Vec<i16> = b.chunks_exact(2).map(|x| i16::from_le_bytes([x[0], x[1]])).collect();
                self.sizes.push((tag, (pcm.len() / self.ch as usize) as u64));
                self.stream.append(tag, crate::services::audio::to_stereo(&pcm, self.ch as usize, self.volume));
            }
            // RegisterBufferEvent
            4 => ctx.copy_handle(self.event.readable),
            // GetReleasedAudioOutBuffers(Auto) -> count; tags out
            5 | 8 => {
                let room = (ctx.buf_out(0).size / 8) as usize;
                let v = self.released(room);
                let b: Vec<u8> = v.iter().flat_map(|t| t.to_le_bytes()).collect();
                ctx.write_out(0, &b);
                ctx.push_u32(v.len() as u32);
                if self.stream.blocks() == 0 || v.is_empty() {
                    self.event.clear();
                }
            }
            6 => {
                let t = ctx.u64(0);
                ctx.push_u8(self.stream.contains(t) as u8)
            }
            9 => ctx.push_u32(self.stream.blocks() as u32),
            // GetAudioOutPlayedSampleCount
            10 => {
                let _ = self.released(0);
                ctx.push_u64(self.played)
            }
            // FlushAudioOutBuffers -> whether any was queued
            11 => {
                let any = self.stream.blocks() > 0;
                if any {
                    self.stream.stop();
                    if self.state == STARTED {
                        self.stream.start();
                    }
                }
                ctx.push_u8(any as u8)
            }
            12 => self.volume = f32::from_bits(ctx.u32(0)).max(0.0),
            13 => ctx.push_u32(self.volume.to_bits()),
            _ => return ctx.unimplemented(),
        }
        0
    }
    fn close(&mut self) {
        self.stream.stop();
        self.stream.close();
    }
}

// ---- audin:u

struct AudIn;

impl Service for AudIn {
    fn name(&self) -> String {
        "audin:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // ListAudioIns(Auto)
            0 | 2 => {
                let n = if mic::device().is_some() && ctx.buf_out(0).size >= 0x100 {
                    let mut b = vec![0u8; 0x100];
                    b[..14].copy_from_slice(b"BuiltInHeadset");
                    ctx.write_out(0, &b) / 0x100
                } else {
                    0
                };
                ctx.push_u32(n as u32);
                0
            }
            // OpenAudioIn(Auto)
            1 | 3 => {
                let (rate, _) = match open_params(ctx.u32(0), 2) {
                    Ok(v) => v,
                    Err(r) => return r,
                };
                let ch = match ctx.arg::<u16>(4) {
                    0 => 2,
                    c => c as u32,
                };
                let Some(dev) = mic::device() else {
                    crate::log_once!("audin ipc none", "audin:u: this PC has no capture device, the open fails (as audin answers without a microphone)");
                    return R_NO_DEVICE;
                };
                let cap = match mic::open(ch) {
                    Ok(m) => m,
                    Err(e) => {
                        crate::log!("[RED] audin:u: the capture device {dev} does not open ({e})");
                        return R_NO_DEVICE;
                    }
                };
                let event = Event::new();
                let ev = event;
                cap.on_release(move || ev.signal());
                let mut b = vec![0u8; 0x100];
                b[..14].copy_from_slice(b"BuiltInHeadset");
                ctx.write_out(0, &b);
                for v in [rate, ch, FMT_PCM16, STOPPED] {
                    ctx.push_u32(v);
                }
                crate::log!("audin:u: AudioIn open on {dev}, {ch} channels (IPC)");
                ctx.object(IAudioIn { cap, ch, state: STOPPED, event, tags: Mutex::new(vec![]) });
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

struct IAudioIn {
    cap: Arc<mic::Capture>,
    ch: u32,
    state: u32,
    event: Event,
    tags: Mutex<Vec<u64>>,
}

impl Service for IAudioIn {
    fn name(&self) -> String {
        "IAudioIn".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            0 => ctx.push_u32(self.state),
            1 => {
                if let Err(e) = self.cap.start() {
                    crate::log!("[RED] audin:u: Start: {e}");
                    return R_NO_DEVICE;
                }
                self.state = STARTED;
            }
            2 => {
                self.cap.stop();
                self.state = STOPPED;
            }
            // AppendAudioInBuffer(Auto)(tag; buffer): captured into the client's data
            3 | 8 => {
                let tag = ctx.u64(0);
                let Some((ptr, size)) = buffer_span(&ctx.read_in(0)) else { return R_NO_DEVICE };
                crate::mem::prepare(ptr, size);
                self.cap.append(tag, ptr, size & !1, self.ch);
                self.tags.lock().unwrap().push(tag);
            }
            4 => ctx.copy_handle(self.event.readable),
            // GetReleasedAudioInBuffers(Auto)
            5 | 9 => {
                let room = (ctx.buf_out(0).size / 8) as usize;
                let mut v = vec![];
                while v.len() < room {
                    let Some(t) = self.cap.released() else { break };
                    self.tags.lock().unwrap().retain(|x| *x != t);
                    v.push(t);
                }
                let b: Vec<u8> = v.iter().flat_map(|t| t.to_le_bytes()).collect();
                ctx.write_out(0, &b);
                ctx.push_u32(v.len() as u32);
                if v.is_empty() {
                    self.event.clear();
                }
            }
            6 => {
                let t = ctx.u64(0);
                ctx.push_u8(self.tags.lock().unwrap().contains(&t) as u8)
            }
            11 => ctx.push_u32(self.cap.pending() as u32),
            _ => return ctx.unimplemented(),
        }
        0
    }
    fn close(&mut self) {
        self.cap.stop();
        self.cap.close();
    }
}

// ---- aud:u

struct Aud;

impl Service for Aud {
    fn name(&self) -> String {
        "aud:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            2 => {
                ctx.object(crate::services::audren_ipc::Manager);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

pub fn register() {
    ipc::register("audout:u", || Box::new(AudOut));
    ipc::register("audin:u", || Box::new(AudIn));
    ipc::register("aud:u", || Box::new(Aud));
}

#[cfg(test)]
mod tests {
    use crate::ipc::hipc::{self, Buf, Builder, T_REQUEST};

    fn send(h: u32, cmd: u32, data: Vec<u8>, a: Vec<Buf>, b: Vec<Buf>) -> hipc::Response {
        let mut m = Builder { ty: T_REQUEST, cmd, data, a, b, ..Default::default() }.build();
        crate::ipc::process(h, &mut m);
        hipc::parse_response(&m, false).unwrap()
    }
    fn u32_at(r: &hipc::Response, o: usize) -> u32 {
        u32::from_le_bytes(r.data[o..o + 4].try_into().unwrap())
    }

    /// An SDK 1.x client's AudioOut over audout:u, on the sound card: list, open (48 kHz; 44.1 kHz refused with
    /// 0x699), the buffer event, two 10 ms buffers appended (AudioOutBuffer layout, client tags), started, both
    /// given back in real time through GetReleased with the event signalled, the played sample count, the
    /// volume, Stop.
    #[test]
    fn audout_u_plays_an_sdk1_client() {
        crate::mem::tests::test_kernel();
        super::register();
        let h = crate::ipc::connect("audout:u").unwrap();
        let mut names = [0u8; 0x100];
        let r = send(h, 0, vec![], vec![], vec![Buf { addr: names.as_mut_ptr() as u64, size: 0x100, tag: 0 }]);
        assert_eq!((u32_at(&r, 0), &names[..10]), (1, &b"DeviceOut\0"[..]));
        let r = send(h, 1, [44100u32.to_le_bytes(), 2u32.to_le_bytes()].concat(), vec![], vec![]);
        assert_eq!(r.result, 0x699);
        let r = send(h, 1, [0u32.to_le_bytes(), 2u32.to_le_bytes()].concat(), vec![], vec![Buf { addr: names.as_mut_ptr() as u64, size: 0x100, tag: 0 }]);
        assert_eq!(r.result, 0);
        assert_eq!((u32_at(&r, 0), u32_at(&r, 4), u32_at(&r, 8), u32_at(&r, 12)), (48000, 2, 2, 1));
        let o = r.moved[0];
        let ev = send(o, 4, vec![], vec![], vec![]).copy[0];
        let pcm: Vec<i16> = (0..960).map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16).collect();
        let mut descs = vec![];
        for _ in 0..2 {
            let mut d = vec![0u8; 0x28];
            d[8..16].copy_from_slice(&(pcm.as_ptr() as u64).to_le_bytes());
            d[0x10..0x18].copy_from_slice(&1920u64.to_le_bytes());
            d[0x18..0x20].copy_from_slice(&1920u64.to_le_bytes());
            descs.push(d);
        }
        for (i, d) in descs.iter().enumerate() {
            let r = send(o, 3, (0x100 + i as u64).to_le_bytes().to_vec(), vec![Buf { addr: d.as_ptr() as u64, size: 0x28, tag: 0 }], vec![]);
            assert_eq!(r.result, 0);
        }
        assert_eq!(u32_at(&send(o, 9, vec![], vec![], vec![]), 0), 2);
        assert_eq!(send(o, 6, 0x101u64.to_le_bytes().to_vec(), vec![], vec![]).data[0], 1);
        assert_eq!(send(o, 1, vec![], vec![], vec![]).result, 0);
        assert_eq!(u32_at(&send(o, 0, vec![], vec![], vec![]), 0), 0, "started");
        let mut tags = [0u64; 4];
        let mut got = vec![];
        let t0 = std::time::Instant::now();
        while got.len() < 2 && t0.elapsed().as_secs() < 3 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let r = send(o, 5, vec![], vec![], vec![Buf { addr: tags.as_mut_ptr() as u64, size: 32, tag: 0 }]);
            got.extend_from_slice(&tags[..u32_at(&r, 0) as usize]);
        }
        assert_eq!(got, vec![0x100, 0x101], "both back, in order, in real time ({:?})", t0.elapsed());
        assert!(t0.elapsed().as_millis() >= 15, "not before they played");
        assert!(nxrt::kernel::get().event_signaled(ev).is_some());
        let r = send(o, 10, vec![], vec![], vec![]);
        assert_eq!(u64::from_le_bytes(r.data[..8].try_into().unwrap()), 960);
        send(o, 12, 0.5f32.to_bits().to_le_bytes().to_vec(), vec![], vec![]);
        assert_eq!(f32::from_bits(u32_at(&send(o, 13, vec![], vec![], vec![]), 0)), 0.5);
        assert_eq!(send(o, 2, vec![], vec![], vec![]).result, 0);
        assert_eq!(u32_at(&send(o, 0, vec![], vec![], vec![]), 0), 1, "stopped");
        crate::ipc::close(o);
    }

    /// audin:u on this PC: with no capture device nothing is listed and an open fails (the console's answer
    /// without a microphone); with one, it is listed and opens at 48 kHz. aud:u 2 hands out the renderer manager.
    #[test]
    fn audin_u_and_aud_u() {
        crate::mem::tests::test_kernel();
        super::register();
        let h = crate::ipc::connect("audin:u").unwrap();
        let mut names = [0u8; 0x100];
        let r = send(h, 0, vec![], vec![], vec![Buf { addr: names.as_mut_ptr() as u64, size: 0x100, tag: 0 }]);
        let r2 = send(h, 1, [0u32.to_le_bytes(), 2u32.to_le_bytes()].concat(), vec![], vec![Buf { addr: names.as_mut_ptr() as u64, size: 0x100, tag: 0 }]);
        match crate::media::mic::device() {
            None => {
                assert_eq!(u32_at(&r, 0), 0);
                assert_eq!(r2.result, super::R_NO_DEVICE);
            }
            Some(_) => {
                assert_eq!(u32_at(&r, 0), 1);
                assert_eq!((r2.result, u32_at(&r2, 0)), (0, 48000));
                crate::ipc::close(r2.moved[0]);
            }
        }
        let a = crate::ipc::connect("aud:u").unwrap();
        let r = send(a, 2, vec![], vec![], vec![]);
        assert_eq!((r.result, r.moved.len()), (0, 1));
    }
}
