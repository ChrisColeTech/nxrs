//! nn::audio: the renderer the game's SDK ships with, run in-process (nxnative's approach), and AudioOut with a
//! silent sink.
//!
//! * The renderer: SDKs link the renderer's server side and the CPU version of the DSP command processing, for
//!   the "CPU rendering + manual execution" mode (AudioRendererParameter +0x2c rendering device = 1, +0x30
//!   execution mode = 1). `OpenAudioRenderer` runs the SDK's own open in that mode, so `RequestUpdate` is parsed
//!   in-process, and a render thread the host starts (a guest thread made with the SDK's `nn::os::CreateThread`)
//!   calls the SDK's `ExecuteAudioRendererRendering` every 5 ms, which renders a frame on the CPU and signals the
//!   game's renderer event. `GetAudioRendererWorkBufferSize` keeps the DSP service's budget (what the game sizes
//!   its heap for); when the local renderer needs more, the host supplies that storage.
//! * The output: the rendered frame goes nowhere yet (the sink belongs to the audio part of the engine); AudioOut
//!   accepts buffers and hands them back every 5 ms, so a game that streams through it keeps going.
//! * Devices: one output device, the built-in speaker, two channels.

use crate::guest::{r32, r64, w32, Args};
use crate::hle::{Registry, RES_FAIL, RES_OK};
use nxrt::cpu::Cpu;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

const P_DEVICE: u64 = 0x2c;
const P_MODE: u64 = 0x30;
const P_SIZE: u64 = 0x40;
const WORK_PAGE: u64 = 0x1000;
const AR_MAX: usize = 4;

fn align(n: u64, a: u64) -> u64 {
    (n + a - 1) & !(a - 1)
}

/// Horizon's renderer service budget for a parameter (renderer revisions 1-15): nxnative's sizing, derived from
/// the service's own; 0 for an unknown revision.
pub fn service_work_size(p: &[u8], revision: u32) -> u64 {
    if revision & 0xff_ffff != 0x56_4552 || !(0x3156_4552..=0x3f56_4552).contains(&revision) {
        return 0;
    }
    let r = (revision >> 24) as u64 - 0x30;
    let mut n = [0u64; 11];
    for (i, v) in n.iter_mut().enumerate() {
        if i == 8 {
            *v = p[0x20] as u64;
            continue;
        }
        let x = i32::from_le_bytes(p[i * 4..i * 4 + 4].try_into().unwrap());
        if !(0..=0x10_0000).contains(&x) {
            return 0;
        }
        *v = x as u64;
    }
    let (mix, sub, voices, sinks, effects, perf, splitters, dest) = (n[2], n[3], n[4], n[5], n[6], n[7], n[9], n[10]);
    let mixes = sub + 1;
    let mut size = align(((sinks + sub) * 0x3c0 + n[1] * 4) * (mix + 6), 64);
    size += align(mix * 4, 64) + sub * 0x400 + mixes * 0x940 + align(mixes * 8, 16);
    size += voices * 0x1d0 + align(voices * if r >= 15 { 0x238 } else { 0x220 }, 16) + align(voices * 8, 16);
    if r >= 2 {
        let graph = mixes * mixes * 4 + mixes * 12 + 2 * align(mixes, 64) / 8 + align(mixes * mixes, 64) / 8;
        size += align(graph, 16);
        size += splitters * 0x20 + align(dest * if r >= 12 { if r >= 15 { 0x130 } else { 8 } } else { 0xe0 }, 16);
        if r < 5 {
            size += align(dest * 4, 16);
        }
    }
    size += (effects + voices * 4) * 0x20 + 0x50;
    if r >= 12 && splitters != 0 && dest != 0 {
        size += align(dest * if r >= 14 { 0xe0 } else { 0x80 }, 64);
    }
    size = align(size + if r >= 9 { effects * 0x80 } else { 0 }, 64);
    size += (sinks + sub) * 0x280 + effects * if r >= 9 { 0x530 } else { 0x4b0 } + sinks * 0x170 + voices * 0x100 + 0x40;
    if perf != 0 {
        let frame = (sub + voices + sinks + effects + 1) * if r >= 5 { 0x18 } else { 0x10 } + if r >= 5 { 0x990 } else { 0x658 };
        size += align(frame * (perf + 1), 64) + 0xc0;
    }
    let mut commands = 0x18000;
    if r >= 5 {
        commands = effects * 0x540 + sub * 0x3f24 + sinks * 0x130 + dest * 0x360 + voices * (splitters * 0x50 + if r >= 8 { 0x284 } else { 0x250 }) + (voices + sub + effects + sinks + 0x65) * 0x28 + 0x2c4;
    }
    align(size + commands + 0x7e, WORK_PAGE)
}

fn param_service_size(p: &[u8]) -> u64 {
    let mut rev = u32::from_le_bytes(p[0x34..0x38].try_into().unwrap());
    if rev & 0xff_ffff != 0x56_4552 {
        rev = u32::from_le_bytes(p[0x38..0x3c].try_into().unwrap());
    }
    service_work_size(p, rev)
}

/// A copy of the parameter in guest memory, set to CPU rendering + manual execution.
fn cpu_manual(param: u64) -> u64 {
    thread_local! { static BUF: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
    let b = BUF.with(|b| {
        if b.get() == 0 {
            b.set(nxrt::kernel::get().space.kernel_alloc(P_SIZE, 16));
        }
        b.get()
    });
    let src = crate::guest::bytes(param, P_SIZE).to_vec();
    crate::guest::write_bytes(b, &src);
    w32(b + P_DEVICE, 1);
    w32(b + P_MODE, 1);
    b
}

fn sdk(name: &str) -> u64 {
    crate::hle::sym(name).unwrap_or_else(|| nxrt::rt::fatal(&format!("audio: the SDK has no {name}")))
}

const WORK_SIZE: &str = "_ZN2nn5audio30GetAudioRendererWorkBufferSizeERKNS0_22AudioRendererParameterE";
const OPEN_EV: &str = "_ZN2nn5audio17OpenAudioRendererEPNS0_19AudioRendererHandleEPNS_2os11SystemEventERKNS0_22AudioRendererParameterEPvm";
const OPEN: &str = "_ZN2nn5audio17OpenAudioRendererEPNS0_19AudioRendererHandleERKNS0_22AudioRendererParameterEPvm";
const CLOSE: &str = "_ZN2nn5audio18CloseAudioRendererENS0_19AudioRendererHandleE";
const EXEC: &str = "_ZN2nn5audio29ExecuteAudioRendererRenderingENS0_19AudioRendererHandleE";

fn in_process() -> bool {
    crate::hle::sym(EXEC).is_some()
}

unsafe fn work_size(c: *mut Cpu) {
    let param = (*c).x[0];
    let p = cpu_manual(param);
    let Some(local) = crate::guest::call(c, sdk(WORK_SIZE), &[p]) else { return };
    let bytes = crate::guest::bytes(param, P_SIZE).to_vec();
    let service = if r32(param + P_DEVICE) == 0 { param_service_size(&bytes) } else { 0 };
    let n = if service != 0 { service } else { local };
    crate::log_once!("audren work", "audren: work buffer {n:#x} for the game (the local CPU renderer needs {local:#x})");
    (*c).x[0] = n;
}

struct Renderer {
    handle: [u64; 2],
    open: AtomicBool,
    thread: u64,
}

static RENDERERS: Mutex<Vec<&'static Renderer>> = Mutex::new(Vec::new());
static IN_OPEN: AtomicBool = AtomicBool::new(false);

unsafe fn open_common(c: *mut Cpu, with_event: bool) {
    let a = &*c;
    let (handle, ev, param, mut work, mut size) =
        if with_event { (a.x[0], a.x[1], a.x[2], a.x[3], a.x[4]) } else { (a.x[0], 0, a.x[1], a.x[2], a.x[3]) };
    if IN_OPEN.load(Ordering::Relaxed) {
        // the SDK's event form calls the form without one: straight to the SDK
        let r = crate::guest::call(c, sdk(OPEN), &[handle, param, work, size]);
        if let Some(r) = r {
            (*c).x[0] = r;
        }
        return;
    }
    let p = cpu_manual(param);
    crate::log!(
        "audren: OpenAudioRenderer rate {} samples {} mix buffers {} submixes {} voices {} sinks {} effects {} (device {} mode {} -> CPU, manual), work {work:#x}+{size:#x}",
        r32(param) as i32,
        r32(param + 4),
        r32(param + 8),
        r32(param + 0xc),
        r32(param + 0x10),
        r32(param + 0x14),
        r32(param + 0x18),
        r32(param + P_DEVICE),
        r32(param + P_MODE)
    );
    let Some(need) = crate::guest::call(c, sdk(WORK_SIZE), &[p]) else { return };
    if size < need {
        // the local renderer's own storage, beyond the service budget the game allocated
        let layout = std::alloc::Layout::from_size_align(need as usize, WORK_PAGE as usize).unwrap();
        let owned = std::alloc::alloc_zeroed(layout) as u64;
        crate::log!("audren: the host backs the local renderer's work buffer {owned:#x}+{need:#x}");
        work = owned;
        size = need;
    }
    IN_OPEN.store(true, Ordering::Relaxed);
    let r = if with_event { crate::guest::call(c, sdk(OPEN_EV), &[handle, ev, p, work, size]) } else { crate::guest::call(c, sdk(OPEN), &[handle, p, work, size]) };
    IN_OPEN.store(false, Ordering::Relaxed);
    let Some(r) = r else { return };
    if r as u32 != 0 {
        crate::log!("[RED] audren: the SDK's OpenAudioRenderer failed {r:#x}");
        (*c).x[0] = r;
        return;
    }
    let ren: &'static Renderer = Box::leak(Box::new(Renderer { handle: [r64(handle), r64(handle + 8)], open: AtomicBool::new(true), thread: 0 }));
    let idx = {
        let mut g = RENDERERS.lock().unwrap();
        if g.len() >= AR_MAX {
            crate::log!("[RED] audren: more than {AR_MAX} renderers: this one is not rendered");
            (*c).x[0] = 0;
            return;
        }
        g.push(ren);
        g.len() - 1
    };
    // the render thread: a guest thread the SDK creates, running the host's loop
    let malloc = sdk("malloc");
    let create = sdk("_ZN2nn2os12CreateThreadEPNS0_10ThreadTypeEPFvPvES3_S3_mi");
    let start = sdk("_ZN2nn2os11StartThreadEPNS0_10ThreadTypeE");
    let Some(tt) = crate::guest::call(c, malloc, &[0x200]) else { return };
    crate::guest::zero(tt, 0x200);
    let stack_size = 0x10000;
    let Some(sa) = crate::guest::call(c, malloc, &[stack_size + 0x1000]) else { return };
    let stack = align(sa, 0x1000);
    let entry = crate::hle::addr("audren render thread", render_main);
    let Some(rc) = crate::guest::call(c, create, &[tt, entry, idx as u64, stack, stack_size, 9]) else { return };
    if rc as u32 != 0 {
        crate::log!("[RED] audren: CreateThread for the render thread failed {rc:#x}");
    } else {
        let p = ren as *const Renderer as *mut Renderer;
        (*p).thread = tt;
        let _ = crate::guest::call(c, start, &[tt]);
        crate::log!("audren: renderer {idx} open, its render thread runs every 5 ms (the output is silent)");
    }
    (*c).x[0] = RES_OK;
}

unsafe fn open_ev(c: *mut Cpu) {
    if in_process() {
        open_common(c, true)
    } else {
        let a = &*c;
        let r = crate::guest::call(c, sdk(OPEN_EV), &[a.x[0], a.x[1], a.x[2], a.x[3], a.x[4]]);
        if let Some(r) = r {
            (*c).x[0] = r;
        }
    }
}
unsafe fn open_noev(c: *mut Cpu) {
    if in_process() {
        open_common(c, false)
    } else {
        let a = &*c;
        let r = crate::guest::call(c, sdk(OPEN), &[a.x[0], a.x[1], a.x[2], a.x[3]]);
        if let Some(r) = r {
            (*c).x[0] = r;
        }
    }
}

/// The render thread: one frame every 5 ms, through the SDK's own ExecuteAudioRendererRendering.
unsafe fn render_main(c: *mut Cpu) {
    let idx = (*c).x[0] as usize;
    let Some(ren) = RENDERERS.lock().unwrap().get(idx).copied() else { return };
    let exec = sdk(EXEC);
    let period = std::time::Duration::from_micros(5000);
    let mut next = std::time::Instant::now();
    let mut frames = 0u64;
    while ren.open.load(Ordering::Relaxed) {
        next += period;
        let now = std::time::Instant::now();
        if next > now {
            crate::sched::without_core(|| std::thread::sleep(next - now));
        } else if now - next > period * 20 {
            next = now;
        }
        if crate::guest::call(c, exec, &[ren.handle[0], ren.handle[1]]).is_none() {
            return;
        }
        frames += 1;
        if frames == 1 {
            crate::log!("audren: renderer {idx} rendered its first frame");
        }
    }
}

unsafe fn close(c: *mut Cpu) {
    let (h0, h1) = ((*c).x[0], (*c).x[1]);
    let ren = RENDERERS.lock().unwrap().iter().find(|r| r.handle[0] == h0).copied();
    if let Some(r) = ren {
        r.open.store(false, Ordering::Relaxed);
        if r.thread != 0 {
            let _ = crate::guest::call(c, sdk("_ZN2nn2os10WaitThreadEPNS0_10ThreadTypeE"), &[r.thread]);
        }
    }
    let _ = crate::guest::call(c, sdk(CLOSE), &[h0, h1]);
}

// ---- AudioOut: a silent sink that hands every buffer back after its playing time

struct AudioOut {
    state: AtomicU32,
    queue: Mutex<VecDeque<(u64, std::time::Instant)>>,
    channels: u32,
    event: u64,
}

static OUTS: Mutex<Vec<(u64, &'static AudioOut)>> = Mutex::new(Vec::new());

fn out_of(obj: u64) -> Option<&'static AudioOut> {
    OUTS.lock().unwrap().iter().find(|(o, _)| *o == obj).map(|(_, a)| *a)
}

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

unsafe fn ao_open(c: *mut Cpu, obj: u64, ev: u64, param: u64) {
    if ev != 0 {
        crate::services::os::bind_system_event(c, ev, true);
    }
    let ch = if param != 0 { r32(param + 4).clamp(1, 6) } else { 2 };
    let ao: &'static AudioOut = Box::leak(Box::new(AudioOut { state: AtomicU32::new(1), queue: Mutex::new(VecDeque::new()), channels: if ch > 2 { 6 } else { 2 }, event: ev }));
    OUTS.lock().unwrap().push((obj, ao));
    crate::log!("audout: opened ({} channels, silent sink)", ao.channels);
    (*c).x[0] = RES_OK;
}
unsafe fn ao_open_ev_name(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], a.x[1], a.x[3]);
}
unsafe fn ao_open_name(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], 0, a.x[2]);
}
unsafe fn ao_open_default_ev(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], a.x[1], a.x[2]);
}
unsafe fn ao_open_default(c: *mut Cpu) {
    let a = &*c;
    ao_open(c, a.x[0], 0, a.x[1]);
}
afn!(ao_start, |a| {
    if let Some(o) = out_of(a.x(0)) {
        o.state.store(0, Ordering::Relaxed);
    }
    RES_OK
});
afn!(ao_stop, |a| {
    if let Some(o) = out_of(a.x(0)) {
        o.state.store(1, Ordering::Relaxed);
    }
    0
});
afn!(ao_close, |a| {
    OUTS.lock().unwrap().retain(|(o, _)| *o != a.x(0));
    0
});
afn!(ao_state, |a| { out_of(a.x(0)).map_or(1, |o| o.state.load(Ordering::Relaxed) as u64) });
afn!(ao_count, |a| { out_of(a.x(0)).map_or(0, |o| o.queue.lock().unwrap().len() as u64) });
afn!(ao_rate, |a| { 48000 });
afn!(ao_channels, |a| { out_of(a.x(0)).map_or(2, |o| o.channels as u64) });
afn!(ao_format, |a| { 2 });
afn!(ao_append, |a| {
    // AudioOutBuffer: +0x18 data size: plays for size / (channels * 2) samples at 48 kHz
    let (obj, buf) = (a.x(0), a.x(1));
    let Some(o) = out_of(obj) else { return 0 };
    let samples = r64(buf + 0x18) / (o.channels as u64 * 2).max(1);
    let mut q = o.queue.lock().unwrap();
    let start = q.back().map_or(std::time::Instant::now(), |b| b.1).max(std::time::Instant::now());
    q.push_back((buf, start + std::time::Duration::from_micros(samples * 1_000_000 / 48000)));
    drop(q);
    if o.event != 0 {
        crate::services::os::signal_bound(o.event);
    }
    1
});
afn!(ao_released, |a| {
    let Some(o) = out_of(a.x(0)) else { return 0 };
    let mut q = o.queue.lock().unwrap();
    match q.front() {
        Some(&(b, done)) if done <= std::time::Instant::now() => {
            q.pop_front();
            b
        }
        _ => 0,
    }
});
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
unsafe fn dev_notify(c: *mut Cpu) {
    let ev = (*c).x[0];
    crate::services::os::bind_system_event(c, ev, false);
    (*c).x[0] = RES_OK;
}
afn!(fail, |a| { RES_FAIL });

pub fn register(r: &mut Registry) {
    const W: &str = "audio: the SDK's renderer in-process, a silent output";
    for (n, f) in [
        (WORK_SIZE, work_size as unsafe fn(*mut Cpu)),
        (OPEN_EV, open_ev),
        (OPEN, open_noev),
        (CLOSE, close),
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
        ("_ZN2nn5audio13ListAudioOutsEPNS0_12AudioOutInfoEi", ao_list),
        ("_ZN2nn5audio19ListAudioDeviceNameEPNS0_15AudioDeviceNameEi", dev_list),
        ("_ZN2nn5audio25ListAudioOutputDeviceNameEPNS0_15AudioDeviceNameEi", dev_list),
        ("_ZN2nn5audio24GetActiveAudioDeviceNameEPNS0_15AudioDeviceNameE", dev_active),
        ("_ZN2nn5audio30GetActiveAudioOutputDeviceNameEPNS0_15AudioDeviceNameE", dev_active),
        ("_ZN2nn5audio41GetActiveAudioDeviceChannelCountForOutputEv", dev_channels),
        ("_ZN2nn5audio21GetActiveChannelCountEv", dev_channels),
        ("_ZN2nn5audio26SetAudioDeviceOutputVolumeEPKNS0_15AudioDeviceNameEf", dev_set_volume),
        ("_ZN2nn5audio26GetAudioDeviceOutputVolumeEPKNS0_15AudioDeviceNameE", dev_get_volume),
        ("_ZN2nn5audio39AcquireAudioDeviceNotificationForOutputEPNS_2os11SystemEventE", dev_notify),
        ("_ZN2nn5audio38AcquireAudioDeviceNotificationForInputEPNS_2os11SystemEventE", dev_notify),
        ("_ZN2nn5audio36AcquireAudioDeviceSwitchNotificationEPNS_2os11SystemEventE", dev_notify),
        ("_ZN2nn5audio11OpenAudioInEPNS0_7AudioInEPNS_2os11SystemEventEPKcRKNS0_16AudioInParameterE", fail),
        ("_ZN2nn5audio11OpenAudioInEPNS0_7AudioInEPKcRKNS0_16AudioInParameterE", fail),
        ("_ZN2nn5audio18OpenDefaultAudioInEPNS0_7AudioInEPNS_2os11SystemEventERKNS0_16AudioInParameterE", fail),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn service_budget() {
        // 48 kHz, 240 samples, 24 mix buffers, 1 submix, 96 voices, 1 sink, 4 effects, revision 5
        let mut p = vec![0u8; 0x40];
        for (i, v) in [48000i32, 240, 24, 1, 96, 1, 4, 0].iter().enumerate() {
            p[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        p[0x34..0x38].copy_from_slice(&0x3556_4552u32.to_le_bytes());
        let n = super::param_service_size(&p);
        assert!(n > 0x18000 && n % 0x1000 == 0, "{n:#x}");
        p[0x34..0x38].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(super::param_service_size(&p), 0, "unknown revision");
    }
}
