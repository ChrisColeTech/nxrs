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
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn downmix() {
        let v = super::to_stereo(&[16384, -16384, 0, 0, 0, 0], 6, 1.0);
        assert_eq!(v, vec![0.5, -0.5]);
        assert_eq!(super::to_stereo(&[100], 1, 1.0).len(), 2);
    }
}
