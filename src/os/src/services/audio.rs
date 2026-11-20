//! nn::audio's outputs and inputs (nxnative `hle_audio.c`, ported onto the engine's sound card): AudioOut on the
//! real output device, the audio device's names, volume and notifications, and AudioIn on the real capture device.
//! The renderer is `audren.rs`.
//!
//! * AudioOut: the game appends PCM16 buffers (`AudioOutBuffer`, the SDK's own struct: data +0x08, buffer size
//!   +0x10, data size +0x18) and takes them back once played; the release event is signalled as each one ends.
//!   The front of the guest AudioOut is the service's answer (rate, channels, format, state at +0/+4/+8/+0xc),
//!   which middleware reads directly. Six-channel output is folded to stereo with the system's downmix.
//! * AudioIn: the default capture device (WASAPI), 48 kHz PCM16 in the game's channel count. With no capture
//!   device the list is empty and an open fails, as the service answers on a console without a microphone.
//! * SDK 21's OutputStream on the one virtual output device (`AudioVirtualOutput`): StreamFormat {sample format
//!   (2 PCM16, 5 float), channels, rate} and a capacity in frames; each write is one block on a sound card stream
//!   of its own (interleaved, converted to 16 bits and folded to stereo as AudioOut's), so the free count is the
//!   capacity less the frames still queued and the stream's event fires as blocks play. States as games read them:
//!   0 not open, 1 open (stopped), 2 started. The device's properties give a 10 ms shortest stream.
//! * SDK 21's InputStream: no virtual input device is listed and an open fails, so a game never reads one; the
//!   calls that need an open stream still answer (closed, nothing captured) without touching the guest object.

use crate::guest::{r16, r32, r64, w32, Args};
use crate::hle::{Registry, RES_FAIL, RES_OK};
use crate::media::{mic, sink};
use nxrt::cpu::Cpu;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const RATE: u32 = 48000;
const FMT_PCM16: u32 = 2;
const STATE_STARTED: u32 = 0;
const STATE_STOPPED: u32 = 1;
/// audout's ResultInvalidSampleRate.
const RES_BAD_RATE: u64 = 0x699;
const QMAX: usize = 64;

macro_rules! afn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

// ---- AudioOut

struct AudioOut {
    id: u32,
    ch: u32,
    state: AtomicU32,
    volume: Mutex<f32>,
    stream: Arc<sink::Stream>,
}

static OUTS: Mutex<Option<HashMap<u64, Arc<AudioOut>>>> = Mutex::new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(0);

fn out_of(obj: u64) -> Option<Arc<AudioOut>> {
    OUTS.lock().unwrap().as_ref()?.get(&obj).cloned()
}

fn au_info(obj: u64, ch: u32, state: u32) {
    w32(obj, RATE);
    w32(obj + 4, ch);
    w32(obj + 8, FMT_PCM16);
    w32(obj + 0xc, state);
}

unsafe fn ao_open(c: *mut Cpu, obj: u64, ev: u64, name: u64, param: u64) {
    let rate = if param != 0 { r32(param) } else { 0 };
    let mut ch = if param != 0 { r16(param + 4) as u32 } else { 0 };
    if ch == 0 {
        ch = 2;
    }
    if rate != 0 && rate != RATE {
        crate::log!("[RED] AudioOut sample rate {rate}: audout takes 48 kHz only, the open fails with 0x699");
        (*c).x[0] = RES_BAD_RATE;
        return;
    }
    if ev != 0 {
        crate::services::os::bind_system_event(c, ev, true);
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let stream = sink::open(&format!("AudioOut #{id}"));
    if ev != 0 {
        stream.on_release(move || {
            crate::services::os::signal_bound(ev);
        });
    }
    let ao = Arc::new(AudioOut { id, ch, state: AtomicU32::new(STATE_STOPPED), volume: Mutex::new(1.0), stream });
    OUTS.lock().unwrap().get_or_insert_with(HashMap::new).insert(obj, ao);
    au_info(obj, ch, STATE_STOPPED);
    let nm = if name != 0 { crate::guest::cstr(name, 256) } else { "(default)".into() };
    crate::log!("audio: AudioOut #{id} open {nm}, {ch} channels, on {}{}", sink::device(), if ev != 0 { "" } else { ", no release event (the game polls)" });
    (*c).x[0] = RES_OK;
}
unsafe fn ao_open_ev_name(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], a.x[1], a.x[2], a.x[3]);
}
unsafe fn ao_open_name(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], 0, a.x[1], a.x[2]);
}
unsafe fn ao_open_default_ev(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], a.x[1], 0, a.x[2]);
}
unsafe fn ao_open_default(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], 0, 0, a.x[1]);
}
afn!(ao_init_param, |a| {
    crate::guest::zero(a.x(0), 8);
    0
});
afn!(ao_start, |a| {
    if let Some(o) = out_of(a.x(0)) {
        o.state.store(STATE_STARTED, Ordering::Relaxed);
        w32(a.x(0) + 0xc, STATE_STARTED);
        o.stream.start();
    }
    RES_OK
});
afn!(ao_stop, |a| {
    if let Some(o) = out_of(a.x(0)) {
        if o.state.swap(STATE_STOPPED, Ordering::Relaxed) == STATE_STARTED {
            o.stream.stop();
        }
        w32(a.x(0) + 0xc, STATE_STOPPED);
    }
    0
});
afn!(ao_close, |a| {
    let o = OUTS.lock().unwrap().as_mut().and_then(|m| m.remove(&a.x(0)));
    if let Some(o) = o {
        o.stream.stop();
        o.stream.close();
        au_info(a.x(0), o.ch, STATE_STOPPED);
        crate::log!("audio: AudioOut #{} closed", o.id);
    }
    0
});
afn!(ao_state, |a| { out_of(a.x(0)).map_or(STATE_STOPPED as u64, |o| o.state.load(Ordering::Relaxed) as u64) });
afn!(ao_count, |a| { out_of(a.x(0)).map_or(0, |o| o.stream.blocks() as u64) });
afn!(ao_rate, |a| { RATE as u64 });
afn!(ao_channels, |a| { out_of(a.x(0)).map_or(2, |o| o.ch as u64) });
afn!(ao_format, |a| { FMT_PCM16 as u64 });

/// Interleaved PCM16 of `ch` channels as stereo floats (six channels folded with the system's downmix).
pub fn to_stereo(src: &[i16], ch: usize, gain: f32) -> Vec<f32> {
    let n = src.len() / ch.max(1);
    let mut v = Vec::with_capacity(n * 2);
    let s = |x: i16| x as f32 / 32768.0 * gain;
    for f in src.chunks_exact(ch.max(1)) {
        match ch {
            1 => {
                v.push(s(f[0]));
                v.push(s(f[0]));
            }
            6 => {
                let c = s(f[2]) * 0.7071 + s(f[3]) * 0.2510;
                v.push(s(f[0]) + c + s(f[4]) * 0.7071);
                v.push(s(f[1]) + c + s(f[5]) * 0.7071);
            }
            _ => {
                v.push(s(f[0]));
                v.push(s(f[1]));
            }
        }
    }
    v
}

afn!(ao_append, |a| {
    let (obj, buf) = (a.x(0), a.x(1));
    let Some(o) = out_of(obj) else { return 0 };
    if o.stream.blocks() >= QMAX {
        return 0;
    }
    let (ptr, size) = (r64(buf + 8), r64(buf + 0x18));
    let pcm: Vec<i16> = if ptr != 0 && size >= 2 {
        let b = crate::guest::bytes(ptr, size & !1);
        b.chunks_exact(2).map(|x| i16::from_le_bytes([x[0], x[1]])).collect()
    } else {
        vec![]
    };
    let gain = *o.volume.lock().unwrap();
    if crate::trace::on("audio") {
        static N: AtomicU32 = AtomicU32::new(0);
        if N.fetch_add(1, Ordering::Relaxed) < 200 {
            crate::log!("audio: AudioOut #{} append {} frames ({} blocks, {} frames queued before)", o.id, pcm.len() / o.ch.max(1) as usize, o.stream.blocks(), o.stream.queued());
        }
    }
    o.stream.append(buf, to_stereo(&pcm, o.ch as usize, gain));
    1
});
afn!(ao_released, |a| { out_of(a.x(0)).and_then(|o| o.stream.released()).unwrap_or(0) });
afn!(ao_contains, |a| { out_of(a.x(0)).is_some_and(|o| o.stream.contains(a.x(1))) as u64 });
unsafe fn ao_set_volume(c: *mut Cpu) {
    let a = Args(&mut *c);
    let v = a.f(0);
    if let Some(o) = out_of(a.x(0)) {
        *o.volume.lock().unwrap() = if v >= 0.0 { v } else { 0.0 };
    }
}
unsafe fn ao_get_volume(c: *mut Cpu) {
    let obj = (*c).x[0];
    let v = out_of(obj).map_or(1.0, |o| *o.volume.lock().unwrap());
    (*c).ret_s(v);
}
afn!(ao_list, |a| {
    if a.i(1) < 1 {
        return 0;
    }
    crate::guest::zero(a.x(0), 256);
    crate::guest::put_cstr(a.x(0), 256, "DeviceOut");
    1
});
afn!(ao_name, |a| {
    static NAME: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *NAME.get_or_init(|| {
        let p = nxrt::kernel::get().space.kernel_alloc(16, 16);
        crate::guest::put_cstr(p, 16, "DeviceOut");
        p
    })
});

// ---- the audio device

fn devname(out: u64) {
    crate::guest::zero(out, 0x100);
    crate::guest::put_cstr(out, 0x100, "AudioBuiltInSpeakerOutput");
}
static DEV_VOLUME: Mutex<f32> = Mutex::new(1.0);
static AUTOTUNE: AtomicU32 = AtomicU32::new(0);
static NOTIFY_OUT: AtomicU32 = AtomicU32::new(0);
static NOTIFY_IN: AtomicU32 = AtomicU32::new(0);
afn!(dev_list, |a| {
    if a.i(1) <= 0 || a.x(0) == 0 {
        return 0;
    }
    devname(a.x(0));
    1
});
afn!(dev_active, |a| {
    devname(a.x(0));
    0
});
afn!(dev_channels, |a| { 2 });
afn!(dev_set_volume, |a| {
    *DEV_VOLUME.lock().unwrap() = a.f(0);
    0
});
unsafe fn dev_get_volume(c: *mut Cpu) {
    (*c).ret_s(*DEV_VOLUME.lock().unwrap());
}
afn!(dev_set_autotune, |a| {
    AUTOTUNE.store(a.x(0) as u32 & 1, Ordering::Relaxed);
    0
});
afn!(dev_autotune, |a| { AUTOTUNE.load(Ordering::Relaxed) as u64 });
/// The output change notification: 0x499 when already acquired (the SDK keeps that flag for the process).
unsafe fn dev_notify(c: *mut Cpu) {
    let ev = (*c).x[0];
    if NOTIFY_OUT.swap(1, Ordering::Relaxed) != 0 {
        (*c).x[0] = 0x499;
        return;
    }
    crate::services::os::bind_system_event(c, ev, false);
    (*c).x[0] = RES_OK;
}
unsafe fn dev_notify_release(c: *mut Cpu) {
    let ev = (*c).x[0];
    if NOTIFY_OUT.swap(0, Ordering::Relaxed) == 0 {
        return;
    }
    if let Some(d) = crate::hle::sym("_ZN2nn2os18DestroySystemEventEPNS0_15SystemEventTypeE") {
        let _ = crate::guest::call(c, d, &[ev]);
    }
}
unsafe fn dev_notify_in(c: *mut Cpu) {
    let ev = (*c).x[0];
    if NOTIFY_IN.swap(1, Ordering::Relaxed) != 0 {
        (*c).x[0] = 0x499;
        return;
    }
    crate::services::os::bind_system_event(c, ev, false);
    (*c).x[0] = RES_OK;
}
unsafe fn dev_switch_notify(c: *mut Cpu) {
    let ev = (*c).x[0];
    crate::services::os::bind_system_event(c, ev, false);
}

// ---- AudioIn on the capture device

struct AudioIn {
    ch: u32,
    state: AtomicU32,
    mic: Arc<mic::Capture>,
}

static INS: Mutex<Option<HashMap<u64, Arc<AudioIn>>>> = Mutex::new(None);

fn in_of(obj: u64) -> Option<Arc<AudioIn>> {
    INS.lock().unwrap().as_ref()?.get(&obj).cloned()
}

afn!(ai_list, |a| {
    if a.i(1) < 1 || a.x(0) == 0 || mic::device().is_none() {
        return 0;
    }
    crate::guest::zero(a.x(0), 256);
    crate::guest::put_cstr(a.x(0), 256, "BuiltInHeadset");
    1
});

unsafe fn ai_open(c: *mut Cpu, obj: u64, ev: u64, param: u64) {
    let rate = if param != 0 { r32(param) } else { 0 };
    let mut ch = if param != 0 { r16(param + 4) as u32 } else { 0 };
    if ch == 0 {
        ch = 2;
    }
    if rate != 0 && rate != RATE {
        crate::log!("[RED] AudioIn sample rate {rate}: audin takes 48 kHz only");
        (*c).x[0] = RES_BAD_RATE;
        return;
    }
    let Some(dev) = mic::device() else {
        crate::log_once!("audin none", "audio: AudioIn: this PC has no capture device, the open fails (as audin answers without a microphone)");
        (*c).x[0] = RES_FAIL;
        return;
    };
    if ev != 0 {
        crate::services::os::bind_system_event(c, ev, true);
    }
    let cap = match mic::open(ch) {
        Ok(m) => m,
        Err(e) => {
            crate::log!("[RED] audio: AudioIn: the capture device {dev} does not open ({e})");
            (*c).x[0] = RES_FAIL;
            return;
        }
    };
    if ev != 0 {
        cap.on_release(move || {
            crate::services::os::signal_bound(ev);
        });
    }
    w32(obj, RATE);
    w32(obj + 4, ch);
    w32(obj + 8, FMT_PCM16);
    w32(obj + 0xc, STATE_STOPPED);
    crate::log!("audio: AudioIn open on {dev}, {ch} channels");
    INS.lock().unwrap().get_or_insert_with(HashMap::new).insert(obj, Arc::new(AudioIn { ch, state: AtomicU32::new(STATE_STOPPED), mic: cap }));
    (*c).x[0] = RES_OK;
}
unsafe fn ai_open_ev(c: *mut Cpu) {
    let a = &*c;
    ai_open(c, a.x[0], a.x[1], a.x[3]);
}
unsafe fn ai_open_noev(c: *mut Cpu) {
    let a = &*c;
    ai_open(c, a.x[0], 0, a.x[2]);
}
unsafe fn ai_open_default(c: *mut Cpu) {
    let a = &*c;
    ai_open(c, a.x[0], a.x[1], a.x[2]);
}
afn!(ai_start, |a| {
    let Some(i) = in_of(a.x(0)) else { return RES_FAIL };
    i.state.store(STATE_STARTED, Ordering::Relaxed);
    w32(a.x(0) + 0xc, STATE_STARTED);
    match i.mic.start() {
        Ok(()) => RES_OK,
        Err(e) => {
            crate::log!("[RED] audio: AudioIn start: {e}");
            RES_FAIL
        }
    }
});
afn!(ai_stop, |a| {
    if let Some(i) = in_of(a.x(0)) {
        i.state.store(STATE_STOPPED, Ordering::Relaxed);
        i.mic.stop();
        w32(a.x(0) + 0xc, STATE_STOPPED);
    }
    0
});
afn!(ai_close, |a| {
    let i = INS.lock().unwrap().as_mut().and_then(|m| m.remove(&a.x(0)));
    if let Some(i) = i {
        i.mic.stop();
        i.mic.close();
    }
    0
});
afn!(ai_append, |a| {
    let Some(i) = in_of(a.x(0)) else { return 0 };
    let buf = a.x(1);
    let (ptr, cap, size) = (r64(buf + 8), r64(buf + 0x10), r64(buf + 0x18));
    let n = if size != 0 { size } else { cap };
    if ptr == 0 || n < 2 {
        return 0;
    }
    crate::mem::prepare(ptr, n);
    i.mic.append(buf, ptr, n & !1, i.ch);
    1
});
afn!(ai_released, |a| { in_of(a.x(0)).and_then(|i| i.mic.released()).unwrap_or(0) });
afn!(ai_state, |a| { in_of(a.x(0)).map_or(STATE_STOPPED as u64, |i| i.state.load(Ordering::Relaxed) as u64) });
afn!(ai_count, |a| { in_of(a.x(0)).map_or(0, |i| i.mic.pending() as u64) });
afn!(ai_rate, |a| { RATE as u64 });
afn!(ai_channels, |a| { in_of(a.x(0)).map_or(2, |i| i.ch as u64) });
afn!(ai_format, |a| { FMT_PCM16 as u64 });

// ---- SDK 21's OutputStream / InputStream

const OS_STOPPED: u32 = 1;
const OS_STARTED: u32 = 2;
const OS_PCM16: i32 = 2;
const OS_FLOAT: i32 = 5;
const VDEV_OUT: &str = "AudioVirtualOutput";
const VDEV_NAME: u64 = 0x100;
/// The shortest stream the device takes, in ns (VirtualDeviceProperties +0x100): one 10 ms period.
const OS_MIN_NS: u64 = 10_000_000;

struct OStream {
    id: u32,
    ch: usize,
    fmt: i32,
    cap: i64,
    queued: i64,
    frames: HashMap<u64, i64>,
    next: u64,
    state: u32,
    gain: f32,
    stream: Arc<sink::Stream>,
}

impl OStream {
    /// The blocks the output played: their frames leave the queue.
    fn reclaim(&mut self) {
        while let Some(t) = self.stream.released() {
            self.queued -= self.frames.remove(&t).unwrap_or(0);
        }
    }
}

static OSTREAMS: Mutex<Option<HashMap<u64, Arc<Mutex<OStream>>>>> = Mutex::new(None);

fn ostream(g: u64) -> Option<Arc<Mutex<OStream>>> {
    OSTREAMS.lock().unwrap().as_ref()?.get(&g).cloned()
}

afn!(os_default_dev, |a| {
    crate::guest::zero(a.x(0), VDEV_NAME);
    crate::guest::put_cstr(a.x(0), VDEV_NAME, VDEV_OUT);
    0
});
afn!(os_dev_props, |a| {
    let (p, name) = (a.x(0), a.x(1));
    let n = crate::guest::cstr(name, VDEV_NAME as usize);
    if n != VDEV_OUT {
        crate::log!("[RED] audio: virtual output device {n}: the host has only {VDEV_OUT}, no properties");
        return RES_FAIL;
    }
    crate::guest::zero(p, 0x110);
    crate::guest::write_bytes(p, crate::guest::bytes(name, VDEV_NAME));
    crate::guest::w64(p + 0x100, OS_MIN_NS);
    RES_OK
});

unsafe fn os_open(c: *mut Cpu, g: u64, ev: u64, fmt: u64, frames: i32) -> u64 {
    let (f, ch, rate) = (r32(fmt) as i32, r32(fmt + 4) as i32, r32(fmt + 8) as i32);
    if f != OS_FLOAT && f != OS_PCM16 {
        crate::log!("[RED] audio: OutputStream sample format {f}: not supported, the open fails");
        return RES_FAIL;
    }
    if rate != 0 && rate != RATE as i32 {
        crate::log!("[RED] audio: OutputStream sample rate {rate}: not supported (48 kHz only), the open fails");
        return RES_FAIL;
    }
    if ch <= 0 || frames <= 0 {
        return RES_FAIL;
    }
    if ev != 0 {
        crate::services::os::bind_system_event(c, ev, true);
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let stream = sink::open(&format!("OutputStream #{id}"));
    if ev != 0 {
        stream.on_release(move || {
            crate::services::os::signal_bound(ev);
        });
    }
    let o = OStream { id, ch: ch as usize, fmt: f, cap: frames as i64, queued: 0, frames: HashMap::new(), next: 1, state: OS_STOPPED, gain: 1.0, stream };
    OSTREAMS.lock().unwrap().get_or_insert_with(HashMap::new).insert(g, Arc::new(Mutex::new(o)));
    crate::log!(
        "audio: OutputStream #{id} open, {ch} channels {}, {frames} frames, on {}{}",
        if f == OS_FLOAT { "float" } else { "PCM16" },
        sink::device(),
        if ev != 0 { "" } else { ", no event (the game polls)" }
    );
    RES_OK
}
// OpenOutputStream(g, SystemEvent *, buffer, size, device, const StreamFormat &, frames, threshold[, flags])
unsafe fn os_open_ev(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (g, ev, fmt, frames) = (a.x(0), a.x(1), a.x(5), a.i(6));
    (*c).x[0] = os_open(c, g, ev, fmt, frames);
}
// OpenOutputStream(g, buffer, size, device, const StreamFormat &, frames, flags): no event, the game polls
unsafe fn os_open_noev(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (g, fmt, frames) = (a.x(0), a.x(4), a.i(5));
    (*c).x[0] = os_open(c, g, 0, fmt, frames);
}
fn os_start_one(g: u64) -> u64 {
    let Some(o) = ostream(g) else {
        crate::log!("[RED] audio: StartOutputStream on {g:#x}, not opened by the host");
        return RES_FAIL;
    };
    let mut o = o.lock().unwrap();
    o.state = OS_STARTED;
    o.stream.start();
    RES_OK
}
afn!(os_start, |a| { os_start_one(a.x(0)) });
afn!(os_start_n, |a| {
    let (gs, n) = (a.x(0), a.i(1));
    for i in 0..n.max(0) as u64 {
        let r = os_start_one(r64(gs + 8 * i));
        if r != RES_OK {
            return r;
        }
    }
    RES_OK
});
afn!(os_stop, |a| {
    if let Some(o) = ostream(a.x(0)) {
        let mut o = o.lock().unwrap();
        o.stream.stop();
        o.reclaim();
        o.state = OS_STOPPED;
    }
    0
});
// PurgeOutputStream: the frames not yet played are dropped (they come back as released); a started stream plays on.
afn!(os_purge, |a| {
    if let Some(o) = ostream(a.x(0)) {
        let mut o = o.lock().unwrap();
        o.reclaim();
        o.stream.stop();
        if o.state == OS_STARTED {
            o.stream.start();
        }
        o.reclaim();
    }
    0
});
afn!(os_close, |a| {
    let o = OSTREAMS.lock().unwrap().as_mut().and_then(|m| m.remove(&a.x(0)));
    if let Some(o) = o {
        let o = o.lock().unwrap();
        o.stream.stop();
        o.stream.close();
        crate::log!("audio: OutputStream #{} closed", o.id);
    }
    0
});
afn!(os_state, |a| { ostream(a.x(0)).map_or(0, |o| o.lock().unwrap().state as u64) });
afn!(os_free, |a| {
    let Some(o) = ostream(a.x(0)) else { return 0 };
    let mut o = o.lock().unwrap();
    o.reclaim();
    if o.stream.blocks() >= QMAX - 1 {
        return 0;
    }
    (o.cap - o.queued).max(0) as u64
});
afn!(os_filled, |a| {
    let Some(o) = ostream(a.x(0)) else { return 0 };
    let mut o = o.lock().unwrap();
    o.reclaim();
    o.queued.max(0) as u64
});
// WriteOutputStream(g, const void *data, int frames): one block, at most what the free count allows.
afn!(os_write, |a| {
    let (g, data, frames) = (a.x(0), a.x(1), a.i(2));
    let Some(o) = ostream(g) else { return 0 };
    let mut o = o.lock().unwrap();
    if frames <= 0 || data == 0 {
        return 0;
    }
    o.reclaim();
    if o.stream.blocks() >= QMAX - 1 || frames as i64 > o.cap - o.queued {
        return 0;
    }
    let n = frames as usize * o.ch;
    let pcm: Vec<i16> = if o.fmt == OS_FLOAT {
        crate::guest::bytes(data, n as u64 * 4)
            .chunks_exact(4)
            .map(|b| (f32::from_le_bytes([b[0], b[1], b[2], b[3]]) * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect()
    } else {
        crate::guest::bytes(data, n as u64 * 2).chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
    };
    let tag = o.next;
    o.next += 1;
    let stereo = to_stereo(&pcm, o.ch, o.gain);
    o.stream.append(tag, stereo);
    o.frames.insert(tag, frames as i64);
    o.queued += frames as i64;
    frames as u64
});
// SetOutputStreamGain(g, float gain, TimeSpan fade): the volume, no fade.
unsafe fn os_gain(c: *mut Cpu) {
    let a = Args(&mut *c);
    let v = a.f(0);
    if let Some(o) = ostream(a.x(0)) {
        o.lock().unwrap().gain = if v >= 0.0 { v } else { 0.0 };
    }
}
/// Statistics, returned through x8 (24 bytes; a game reads +0x10 as its glitch count): none kept.
unsafe fn stream_stats(c: *mut Cpu) {
    crate::guest::zero((*c).x[8], 24);
}

afn!(is_list, |a| { 0 });
afn!(is_fail, |a| { RES_FAIL });
afn!(is_zero, |a| { 0 });
/// GetInputStreamFormat: StreamFormat {PCM16, 1 channel, 48 kHz}, 12 bytes in x0 / x1.
unsafe fn is_format(c: *mut Cpu) {
    (*c).x[0] = OS_PCM16 as u32 as u64 | 1 << 32;
    (*c).x[1] = RATE as u64;
}
unsafe fn is_gain(c: *mut Cpu) {
    (*c).ret_s(1.0);
}

/// The output and input lines for the log.
pub fn report() -> Vec<String> {
    let mut v = sink::report();
    v.extend(crate::services::audren::report());
    v.extend(crate::services::movie::report());
    v.extend(crate::services::codec_video::report());
    v
}

pub fn register(r: &mut Registry) {
    const W: &str = "audio: outputs on the PC's sound card, inputs from its capture device";
    for (n, f) in [
        ("_ZN2nn5audio27InitializeAudioOutParameterEPNS0_17AudioOutParameterE", ao_init_param as unsafe fn(*mut Cpu)),
        ("_ZN2nn5audio12OpenAudioOutEPNS0_8AudioOutEPNS_2os11SystemEventEPKcRKNS0_17AudioOutParameterE", ao_open_ev_name),
        ("_ZN2nn5audio12OpenAudioOutEPNS0_8AudioOutEPKcRKNS0_17AudioOutParameterE", ao_open_name),
        ("_ZN2nn5audio19OpenDefaultAudioOutEPNS0_8AudioOutEPNS_2os11SystemEventERKNS0_17AudioOutParameterE", ao_open_default_ev),
        ("_ZN2nn5audio19OpenDefaultAudioOutEPNS0_8AudioOutERKNS0_17AudioOutParameterE", ao_open_default),
        ("_ZN2nn5audio13StartAudioOutEPNS0_8AudioOutE", ao_start),
        ("_ZN2nn5audio12StopAudioOutEPNS0_8AudioOutE", ao_stop),
        ("_ZN2nn5audio13CloseAudioOutEPNS0_8AudioOutE", ao_close),
        ("_ZN2nn5audio16GetAudioOutStateEPKNS0_8AudioOutE", ao_state),
        ("_ZN2nn5audio22GetAudioOutBufferCountEPKNS0_8AudioOutE", ao_count),
        ("_ZN2nn5audio21GetAudioOutSampleRateEPKNS0_8AudioOutE", ao_rate),
        ("_ZN2nn5audio23GetAudioOutChannelCountEPKNS0_8AudioOutE", ao_channels),
        ("_ZN2nn5audio23GetAudioOutSampleFormatEPKNS0_8AudioOutE", ao_format),
        ("_ZN2nn5audio15GetAudioOutNameEPKNS0_8AudioOutE", ao_name),
        ("_ZN2nn5audio20AppendAudioOutBufferEPNS0_8AudioOutEPNS0_14AudioOutBufferE", ao_append),
        ("_ZN2nn5audio25GetReleasedAudioOutBufferEPNS0_8AudioOutE", ao_released),
        ("_ZN2nn5audio20ContainsAudioOutBufferEPKNS0_8AudioOutEPKNS0_14AudioOutBufferE", ao_contains),
        ("_ZN2nn5audio17SetAudioOutVolumeEPNS0_8AudioOutEf", ao_set_volume),
        ("_ZN2nn5audio17GetAudioOutVolumeEPKNS0_8AudioOutE", ao_get_volume),
        ("_ZN2nn5audio13ListAudioOutsEPNS0_12AudioOutInfoEi", ao_list),
        ("_ZN2nn5audio19ListAudioDeviceNameEPNS0_15AudioDeviceNameEi", dev_list),
        ("_ZN2nn5audio25ListAudioOutputDeviceNameEPNS0_15AudioDeviceNameEi", dev_list),
        ("_ZN2nn5audio24GetActiveAudioDeviceNameEPNS0_15AudioDeviceNameE", dev_active),
        ("_ZN2nn5audio30GetActiveAudioOutputDeviceNameEPNS0_15AudioDeviceNameE", dev_active),
        ("_ZN2nn5audio41GetActiveAudioDeviceChannelCountForOutputEv", dev_channels),
        ("_ZN2nn5audio21GetActiveChannelCountEv", dev_channels),
        ("_ZN2nn5audio26SetAudioDeviceOutputVolumeEPKNS0_15AudioDeviceNameEf", dev_set_volume),
        ("_ZN2nn5audio26GetAudioDeviceOutputVolumeEPKNS0_15AudioDeviceNameE", dev_get_volume),
        ("_ZN2nn5audio40SetDeviceOutputVolumeAutoTuneModeEnabledEb", dev_set_autotune),
        ("_ZN2nn5audio39IsDeviceOutputVolumeAutoTuneModeEnabledEv", dev_autotune),
        ("_ZN2nn5audio39AcquireAudioDeviceNotificationForOutputEPNS_2os11SystemEventE", dev_notify),
        ("_ZN2nn5audio36AcquireAudioOutputDeviceNotificationEPNS_2os11SystemEventE", dev_notify),
        ("_ZN2nn5audio36ReleaseAudioOutputDeviceNotificationEPNS_2os11SystemEventE", dev_notify_release),
        ("_ZN2nn5audio38AcquireAudioDeviceNotificationForInputEPNS_2os11SystemEventE", dev_notify_in),
        ("_ZN2nn5audio36AcquireAudioDeviceSwitchNotificationEPNS_2os11SystemEventE", dev_switch_notify),
        ("_ZN2nn5audio12ListAudioInsEPNS0_11AudioInInfoEi", ai_list),
        ("_ZN2nn5audio11OpenAudioInEPNS0_7AudioInEPNS_2os11SystemEventEPKcRKNS0_16AudioInParameterE", ai_open_ev),
        ("_ZN2nn5audio11OpenAudioInEPNS0_7AudioInEPKcRKNS0_16AudioInParameterE", ai_open_noev),
        ("_ZN2nn5audio18OpenDefaultAudioInEPNS0_7AudioInEPNS_2os11SystemEventERKNS0_16AudioInParameterE", ai_open_default),
        ("_ZN2nn5audio12StartAudioInEPNS0_7AudioInE", ai_start),
        ("_ZN2nn5audio11StopAudioInEPNS0_7AudioInE", ai_stop),
        ("_ZN2nn5audio12CloseAudioInEPNS0_7AudioInE", ai_close),
        ("_ZN2nn5audio19AppendAudioInBufferEPNS0_7AudioInEPNS0_13AudioInBufferE", ai_append),
        ("_ZN2nn5audio24GetReleasedAudioInBufferEPNS0_7AudioInE", ai_released),
        ("_ZN2nn5audio15GetAudioInStateEPKNS0_7AudioInE", ai_state),
        ("_ZN2nn5audio21GetAudioInBufferCountEPKNS0_7AudioInE", ai_count),
        ("_ZN2nn5audio20GetAudioInSampleRateEPKNS0_7AudioInE", ai_rate),
        ("_ZN2nn5audio22GetAudioInChannelCountEPKNS0_7AudioInE", ai_channels),
        ("_ZN2nn5audio22GetAudioInSampleFormatEPKNS0_7AudioInE", ai_format),
        ("_ZN2nn5audio29GetDefaultVirtualOutputDeviceEPNS0_15AudioDeviceNameE", os_default_dev),
        ("_ZN2nn5audio32GetVirtualOutputDevicePropertiesEPNS0_23VirtualDevicePropertiesERKNS0_15AudioDeviceNameE", os_dev_props),
        ("_ZN2nn5audio16OpenOutputStreamEPNS0_16OutputStreamTypeEPNS_2os11SystemEventEPvmRKNS0_15AudioDeviceNameERKNS0_12StreamFormatEiiNS_4util10BitFlagSetILi32ENS0_16OutputStreamFlagEEE", os_open_ev),
        ("_ZN2nn5audio16OpenOutputStreamEPNS0_16OutputStreamTypeEPNS_2os11SystemEventEPvmRKNS0_15AudioDeviceNameERKNS0_12StreamFormatEii", os_open_ev),
        ("_ZN2nn5audio16OpenOutputStreamEPNS0_16OutputStreamTypeEPvmRKNS0_15AudioDeviceNameERKNS0_12StreamFormatEiNS_4util10BitFlagSetILi32ENS0_16OutputStreamFlagEEE", os_open_noev),
        ("_ZN2nn5audio17StartOutputStreamEPNS0_16OutputStreamTypeE", os_start),
        ("_ZN2nn5audio18StartOutputStreamsEPKPNS0_16OutputStreamTypeEi", os_start_n),
        ("_ZN2nn5audio16StopOutputStreamEPNS0_16OutputStreamTypeE", os_stop),
        ("_ZN2nn5audio17PurgeOutputStreamEPNS0_16OutputStreamTypeE", os_purge),
        ("_ZN2nn5audio17CloseOutputStreamEPNS0_16OutputStreamTypeE", os_close),
        ("_ZN2nn5audio20GetOutputStreamStateEPKNS0_16OutputStreamTypeE", os_state),
        ("_ZN2nn5audio29GetOutputStreamFreeFrameCountEPKNS0_16OutputStreamTypeE", os_free),
        ("_ZN2nn5audio31GetOutputStreamFilledFrameCountEPKNS0_16OutputStreamTypeE", os_filled),
        ("_ZN2nn5audio17WriteOutputStreamEPNS0_16OutputStreamTypeEPKvi", os_write),
        ("_ZN2nn5audio19SetOutputStreamGainEPNS0_16OutputStreamTypeEfNS_8TimeSpanE", os_gain),
        ("_ZN2nn5audio25GetOutputStreamStatisticsEPNS0_16OutputStreamTypeEb", stream_stats),
        ("_ZN2nn5audio23ListVirtualInputDevicesEPNS0_15AudioDeviceNameEi", is_list),
        ("_ZN2nn5audio31GetVirtualInputDevicePropertiesEPNS0_23VirtualDevicePropertiesERKNS0_15AudioDeviceNameE", is_fail),
        ("_ZN2nn5audio15OpenInputStreamEPNS0_15InputStreamTypeEPvmRKNS0_15AudioDeviceNameERKNS0_12StreamFormatEi", is_fail),
        ("_ZN2nn5audio16CloseInputStreamEPNS0_15InputStreamTypeE", is_zero),
        ("_ZN2nn5audio16StartInputStreamEPNS0_15InputStreamTypeE", is_fail),
        ("_ZN2nn5audio15StopInputStreamEPNS0_15InputStreamTypeE", is_zero),
        ("_ZN2nn5audio15ReadInputStreamEPNS0_15InputStreamTypeEPvi", is_zero),
        ("_ZN2nn5audio30GetInputStreamFilledFrameCountEPKNS0_15InputStreamTypeE", is_zero),
        ("_ZN2nn5audio20GetInputStreamFormatEPKNS0_15InputStreamTypeE", is_format),
        ("_ZN2nn5audio19GetInputStreamStateEPKNS0_15InputStreamTypeE", is_zero),
        ("_ZN2nn5audio16PurgeInputStreamEPNS0_15InputStreamTypeE", is_zero),
        ("_ZN2nn5audio24GetInputStreamStatisticsEPNS0_15InputStreamTypeEb", stream_stats),
        ("_ZN2nn5audio28GetInputStreamFreeFrameCountEPKNS0_15InputStreamTypeE", is_zero),
        ("_ZN2nn5audio32GetInputStreamCapacityFrameCountEPKNS0_15InputStreamTypeE", is_zero),
        ("_ZN2nn5audio18GetInputStreamGainEPKNS0_15InputStreamTypeE", is_gain),
        ("_ZN2nn5audio18SetInputStreamGainEPNS0_15InputStreamTypeEfNS_8TimeSpanE", is_zero),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn call(f: unsafe fn(*mut Cpu), args: &[u64]) -> u64 {
        let mut cpu: Box<Cpu> = Box::new(std::mem::zeroed());
        cpu.x[..args.len()].copy_from_slice(args);
        f(&mut *cpu);
        cpu.x[0]
    }

    /// AudioIn through the service's takeovers on the PC's real capture device: listed, opened (48 kHz, 2
    /// channels), started, two 10 ms buffers appended and released full by the device, stopped, closed. Then with
    /// no device (the PC's own absence, or the device hidden): nothing listed and the open fails, as audin answers
    /// on a console without a microphone. A PC with no capture device runs only the second half (logged).
    #[test]
    fn audio_in_on_the_real_capture_device_and_without_one() {
        unsafe {
            let mut names = vec![0u8; 256];
            let mut param = [0u8; 8];
            param[..4].copy_from_slice(&RATE.to_le_bytes());
            param[4..6].copy_from_slice(&2u16.to_le_bytes());
            let dev = mic::device();
            if dev.is_none() {
                eprintln!("test: this PC has no capture device: only the no-device answer is checked");
            }
            if let Some(dev) = dev {
                assert_eq!(call(ai_list, &[names.as_mut_ptr() as u64, 1]), 1);
                assert!(names.starts_with(b"BuiltInHeadset\0"));
                let mut obj = vec![0u64; 8];
                let o = obj.as_mut_ptr() as u64;
                assert_eq!(call(ai_open_noev, &[o, 0, param.as_ptr() as u64]), RES_OK);
                assert_eq!((r32(o), r32(o + 4)), (RATE, 2));
                assert_eq!(call(ai_start, &[o]), RES_OK, "the capture stream starts");
                assert_eq!(call(ai_state, &[o]), STATE_STARTED as u64);
                // AudioInBuffer: +8 data, +0x10 capacity, +0x18 size (written back as the bytes filled)
                let mut pcm = vec![vec![0i16; 480 * 2]; 2];
                let mut bufs = vec![[0u64; 5]; 2];
                for (b, p) in bufs.iter_mut().zip(&mut pcm) {
                    b[1] = p.as_mut_ptr() as u64;
                    b[2] = (p.len() * 2) as u64;
                    b[3] = (p.len() * 2) as u64;
                    assert_eq!(call(ai_append, &[o, b.as_ptr() as u64]), 1);
                }
                let t0 = std::time::Instant::now();
                let mut got = vec![];
                while got.len() < 2 && t0.elapsed().as_secs() < 5 {
                    match call(ai_released, &[o]) {
                        0 => std::thread::sleep(std::time::Duration::from_millis(2)),
                        t => got.push(t),
                    }
                }
                assert_eq!(got, vec![bufs[0].as_ptr() as u64, bufs[1].as_ptr() as u64], "both buffers come back full, in order");
                assert_eq!(bufs[0][3], 480 * 4, "10 ms of 48 kHz stereo PCM16 captured");
                call(ai_stop, &[o]);
                assert_eq!(call(ai_state, &[o]), STATE_STOPPED as u64);
                call(ai_close, &[o]);
                crate::log!("test: AudioIn captured from {dev}");
            }

            mic::HIDE_DEVICE.store(true, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(call(ai_list, &[names.as_mut_ptr() as u64, 1]), 0, "no device listed");
            let mut obj2 = vec![0u64; 8];
            assert_eq!(call(ai_open_noev, &[obj2.as_mut_ptr() as u64, 0, param.as_ptr() as u64]), RES_FAIL, "the open fails");
            mic::HIDE_DEVICE.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn downmix() {
        let v = super::to_stereo(&[16384, -16384, 0, 0, 0, 0], 6, 1.0);
        assert_eq!(v, vec![0.5, -0.5]);
        assert_eq!(super::to_stereo(&[100], 1, 1.0).len(), 2);
    }
}
