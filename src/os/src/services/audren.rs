//! nn::audio's renderer, run in-process: the complete renderer the game's SDK ships with (its server side and the
//! CPU version of the DSP command processing), no mixer of the host's own (nxnative `hle_audren.c`, ported).
//!
//! * `GetAudioRendererWorkBufferSize` keeps the DSP service's budget (what the game sizes its heap for);
//!   `OpenAudioRenderer` opens the SDK's renderer in "CPU rendering + manual execution" mode (parameter +0x2c = 1,
//!   +0x30 = 1), backing the local renderer's larger storage itself when the game's buffer is too small.
//! * A render thread (a guest thread the SDK's CreateThread makes, running the host's loop) renders one frame each
//!   time the sound card hands a 5 ms block back, so the sound card's clock paces the renderer.
//! * `AudioRendererImpl::ExecuteAudioRendererRendering` (and SDK 20's `AudioRenderSystem` one) is taken over:
//!   the SDK generates the command list under the renderer's update lock (read off the SDK's own `Update`), the
//!   SDK's `CommandListProcessor` runs it on the CPU, then the host finds the DeviceSink command in the list (a
//!   no-op for the CPU processor) and folds its input mix buffers to stereo for the sound card. Command headers in
//!   the SDK 4/6, 8-20 and 21 layouts; DeviceSink of SDK 4 (0x130 bytes, s8 inputs), 5 (0x138), 8-20 (0x148)
//!   and 21 (0x140); 32 kHz renderers upsampled 3:2 by the host (the SDK makes no upsampler for CPU rendering).
//! * `AudioRenderSystem::Update` renders a catch-up frame first when nothing was rendered since the last update
//!   (an effect's initialization must reach the DSP between two updates); catch-up frames queue for the render
//!   thread, which keeps them in real time.

use crate::guest::{r16, r32, r64, r8, w32};
use crate::hle::{Registry, RES_OK};
use crate::media::sink::{self, Stream};
use crate::sched::without_core;
use nxrt::cpu::Cpu;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

const P_DEVICE: u64 = 0x2c;
const P_MODE: u64 = 0x30;
const P_SIZE: u64 = 0x40;
const WORK_PAGE: u64 = 0x1000;
const AR_MAX: usize = 4;
const FRAME_MAX: usize = 1024;
/// Blocks in flight between the render thread and the sound card (8 x 5 ms).
const NBUF: usize = 8;
/// Catch-up frames queued for the render thread.
const CQ: usize = 4;

fn align(n: u64, a: u64) -> u64 {
    (n + a - 1) & !(a - 1)
}

/// Horizon's renderer service budget for a parameter (renderer revisions 1-15); 0 for an unknown revision.
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
    thread_local! { static BUF: Cell<u64> = const { Cell::new(0) }; }
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
    crate::hle::sym(name).unwrap_or_else(|| nxrt::rt::fatal(&format!("audren: the SDK has no {name}")))
}

const WORK_SIZE: &str = "_ZN2nn5audio30GetAudioRendererWorkBufferSizeERKNS0_22AudioRendererParameterE";
const OPEN_EV: &str = "_ZN2nn5audio17OpenAudioRendererEPNS0_19AudioRendererHandleEPNS_2os11SystemEventERKNS0_22AudioRendererParameterEPvm";
const OPEN: &str = "_ZN2nn5audio17OpenAudioRendererEPNS0_19AudioRendererHandleERKNS0_22AudioRendererParameterEPvm";
const CLOSE: &str = "_ZN2nn5audio18CloseAudioRendererENS0_19AudioRendererHandleE";
const EXEC: &str = "_ZN2nn5audio29ExecuteAudioRendererRenderingENS0_19AudioRendererHandleE";
const IMPL_EXEC: &str = "_ZN2nn5audio6server24AudioRendererManagerImpl17AudioRendererImpl29ExecuteAudioRendererRenderingEv";
const SYS_EXEC: &str = "_ZN2nn5audio6server17AudioRenderSystem29ExecuteAudioRendererRenderingEv";
const SYS_UPDATE: &str = "_ZN2nn5audio6server17AudioRenderSystem6UpdateEPvmS3_mPKvm";
const SYS_UPDATE_IMPL: &str = "_ZN2nn5audio6server17AudioRenderSystem10UpdateImplEPvmS3_mPKvm";
const UPSAMPLER_ALLOC: &str = "_ZN2nn5audio6server16UpsamplerManager8AllocateEv";

/// An SDK without the renderer in the process (SDK 1.x) only has the client of audren:u.
fn in_process() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| {
        let v = crate::hle::sym(EXEC).is_some();
        if !v {
            crate::log!("audren: the SDK has no renderer of its own (SDK 1.x): its client talks to audren:u, mixed by the host (audren_ipc)");
        }
        v
    })
}

unsafe fn work_size(c: *mut Cpu) {
    let param = (*c).x[0];
    if !in_process() {
        if let Some(r) = crate::guest::call(c, sdk(WORK_SIZE), &[param]) {
            (*c).x[0] = r;
        }
        return;
    }
    let p = cpu_manual(param);
    let Some(local) = crate::guest::call(c, sdk(WORK_SIZE), &[p]) else { return };
    let bytes = crate::guest::bytes(param, P_SIZE).to_vec();
    let service = if r32(param + P_DEVICE) == 0 { param_service_size(&bytes) } else { 0 };
    let n = if service != 0 { service } else { local };
    crate::log_once!("audren work", "audren: work buffer {n:#x} for the game (the local CPU renderer needs {local:#x})");
    (*c).x[0] = n;
}

// ---- one renderer

struct Up {
    buf: Vec<i32>,
    pos: u32,
}

pub struct Ar {
    idx: usize,
    handle: [u64; 2],
    sys: AtomicU64,
    open: AtomicBool,
    thread: AtomicU64,
    frame: Mutex<Vec<i16>>, // the latest frame, stereo
    cq: Mutex<VecDeque<Vec<i16>>>,
    cq_cv: Condvar,
    frames: AtomicU64,
    nonsilent: AtomicU64,
    updates: AtomicU64,
    renders: AtomicU64,
    catchups: AtomicU64,
    last_r: AtomicU64,
    updated: AtomicBool,
    sink_logged: AtomicBool,
    up: Mutex<Up>,
    out: Mutex<Option<Arc<Stream>>>,
}

static ARS: Mutex<Vec<&'static Ar>> = Mutex::new(Vec::new());

thread_local! { static T_AR: Cell<usize> = const { Cell::new(usize::MAX) }; }

fn ar_of_sys(sys: u64) -> Option<&'static Ar> {
    let g = ARS.lock().unwrap();
    if let Some(a) = g.iter().find(|a| a.open.load(Ordering::Relaxed) && a.sys.load(Ordering::Relaxed) == sys) {
        return Some(a);
    }
    let unknown: Vec<&&Ar> = g.iter().filter(|a| a.open.load(Ordering::Relaxed) && a.sys.load(Ordering::Relaxed) == 0).collect();
    if unknown.len() == 1 {
        unknown[0].sys.store(sys, Ordering::Relaxed);
        return Some(unknown[0]);
    }
    None
}

static IN_OPEN: AtomicBool = AtomicBool::new(false);

unsafe fn open_common(c: *mut Cpu, with_event: bool) {
    let a = &*c;
    let (handle, ev, param, mut work, mut size) =
        if with_event { (a.x[0], a.x[1], a.x[2], a.x[3], a.x[4]) } else { (a.x[0], 0, a.x[1], a.x[2], a.x[3]) };
    if IN_OPEN.load(Ordering::Relaxed) || !in_process() {
        // the SDK's event form calls the form without one: straight to the SDK
        let r = if with_event { crate::guest::call(c, sdk(OPEN_EV), &[handle, ev, param, work, size]) } else { crate::guest::call(c, sdk(OPEN), &[handle, param, work, size]) };
        if let Some(r) = r {
            (*c).x[0] = r;
        }
        return;
    }
    let p = cpu_manual(param);
    crate::log!(
        "audren: OpenAudioRenderer{} rate {} samples {} mix buffers {} submixes {} voices {} sinks {} effects {} splitters {} (device {} mode {} -> CPU, manual), work {work:#x}+{size:#x}",
        if with_event { "" } else { " (no event)" },
        r32(param) as i32,
        r32(param + 4),
        r32(param + 8),
        r32(param + 0xc),
        r32(param + 0x10),
        r32(param + 0x14),
        r32(param + 0x18),
        r32(param + 0x24),
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
    let idx = {
        let g = ARS.lock().unwrap();
        g.len()
    };
    if idx >= AR_MAX {
        crate::log!("[RED] audren: more than {AR_MAX} renderers: this one is not rendered");
        (*c).x[0] = r;
        return;
    }
    let ar: &'static Ar = Box::leak(Box::new(Ar {
        idx,
        handle: [r64(handle), r64(handle + 8)],
        sys: AtomicU64::new(0),
        open: AtomicBool::new(true),
        thread: AtomicU64::new(0),
        frame: Mutex::new(Vec::new()),
        cq: Mutex::new(VecDeque::new()),
        cq_cv: Condvar::new(),
        frames: AtomicU64::new(0),
        nonsilent: AtomicU64::new(0),
        updates: AtomicU64::new(0),
        renders: AtomicU64::new(0),
        catchups: AtomicU64::new(0),
        last_r: AtomicU64::new(0),
        updated: AtomicBool::new(false),
        sink_logged: AtomicBool::new(false),
        up: Mutex::new(Up { buf: vec![0; 2 * (RS_T - 1 + FRAME_MAX)], pos: 0 }),
        out: Mutex::new(None),
    }));
    ARS.lock().unwrap().push(ar);
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
        ar.thread.store(tt, Ordering::Relaxed);
        let _ = crate::guest::call(c, start, &[tt]);
        crate::log!("audren: renderer {idx} open; its render thread renders a frame for each 5 ms block the sound card hands back ({NBUF} in flight)");
    }
    (*c).x[0] = RES_OK;
}

unsafe fn open_ev(c: *mut Cpu) {
    open_common(c, true)
}
unsafe fn open_noev(c: *mut Cpu) {
    open_common(c, false)
}

/// The render thread: one frame per block the sound card hands back.
unsafe fn render_main(c: *mut Cpu) {
    let idx = (*c).x[0] as usize;
    let Some(a) = ARS.lock().unwrap().get(idx).copied() else { return };
    T_AR.with(|t| t.set(idx));
    let exec = sdk(EXEC);
    let out = sink::open(&format!("renderer {idx}"));
    *a.out.lock().unwrap() = Some(out.clone());
    let mut free: Vec<u64> = (0..NBUF as u64).collect();
    out.start();
    while a.open.load(Ordering::Relaxed) {
        while free.is_empty() {
            if let Some(b) = out.released() {
                free.push(b);
                break;
            }
            without_core(|| out.wait(10));
            if !a.open.load(Ordering::Relaxed) {
                break;
            }
        }
        let Some(tag) = free.pop() else { break };
        let queued = a.cq.lock().unwrap().pop_front();
        let pcm = match queued {
            Some(f) => {
                a.cq_cv.notify_all();
                f
            }
            None => {
                a.frame.lock().unwrap().clear();
                if crate::guest::call(c, exec, &[a.handle[0], a.handle[1]]).is_none() {
                    return;
                }
                std::mem::take(&mut *a.frame.lock().unwrap())
            }
        };
        if pcm.is_empty() {
            out.append_i16(tag, &[0i16; 480]);
        } else {
            out.append_i16(tag, &pcm);
        }
    }
    out.close();
    a.cq_cv.notify_all();
}

unsafe fn close(c: *mut Cpu) {
    let (h0, h1) = ((*c).x[0], (*c).x[1]);
    if in_process() {
        let ar = ARS.lock().unwrap().iter().find(|r| r.handle[0] == h0 && r.open.load(Ordering::Relaxed)).copied();
        if let Some(r) = ar {
            r.open.store(false, Ordering::Relaxed);
            let tt = r.thread.load(Ordering::Relaxed);
            if tt != 0 {
                let _ = crate::guest::call(c, sdk("_ZN2nn2os10WaitThreadEPNS0_10ThreadTypeE"), &[tt]);
            }
            crate::log!(
                "audren: renderer {} closed: {} frames rendered, {} not silent, {} catch-up frames",
                r.idx,
                r.frames.load(Ordering::Relaxed),
                r.nonsilent.load(Ordering::Relaxed),
                r.catchups.load(Ordering::Relaxed)
            );
        }
    }
    let _ = crate::guest::call(c, sdk(CLOSE), &[h0, h1]);
}

// ---- the command list and its DeviceSink

#[derive(Clone, Copy)]
struct Layout {
    done: bool,
    hdr6: bool,
    hdr21: bool,
    sink4: bool,
    sink5: bool,
    sink_id: i32,
    hist_logged: bool,
    malformed_logged: bool,
    skipped_logged: bool,
}

static LAYOUT: Mutex<Layout> = Mutex::new(Layout {
    done: false,
    hdr6: false,
    hdr21: false,
    sink4: false,
    sink5: false,
    sink_id: 0x13,
    hist_logged: false,
    malformed_logged: false,
    skipped_logged: false,
});

fn cmd_size(l: &Layout, q: u64) -> i32 {
    r16(q + if l.hdr21 { 2 } else if l.hdr6 { 4 } else { 6 }) as i16 as i32
}
fn cmd_id(l: &Layout, q: u64) -> i32 {
    if l.hdr21 {
        r8(q + 1) as i32
    } else if l.hdr6 {
        r16(q + 6) as i16 as i32
    } else {
        r8(q + 5) as i32
    }
}
fn cmd_on(l: &Layout, q: u64) -> bool {
    if l.hdr21 {
        r8(q) & 1 != 0
    } else if l.hdr6 {
        r8(q + 0x10) != 0
    } else {
        r8(q + 4) != 0
    }
}
fn cmd_ok(l: &Layout, q: u64, end: u64) -> bool {
    let header = if l.hdr21 { 0x10 } else if l.hdr6 { 0x11 } else { 8 };
    if q > end || end - q < header {
        return false;
    }
    if !l.hdr21 && r32(q) != 0xcafe_babe {
        return false;
    }
    let n = cmd_size(l, q);
    n >= header as i32 && (n as u64) <= end - q
}
fn sink_body(l: &Layout) -> i64 {
    if l.hdr21 {
        -8
    } else {
        0
    }
}
fn cmd_walks(l: &Layout, cl: u64, ncmd: u32, end: u64) -> bool {
    let mut q = cl + 0x20;
    for _ in 0..ncmd {
        if !cmd_ok(l, q, end) {
            return false;
        }
        q += cmd_size(l, q) as u64;
    }
    true
}

const RS_T: usize = 16;

fn up_taps() -> &'static [[f32; RS_T]; 3] {
    static H: OnceLock<[[f32; RS_T]; 3]> = OnceLock::new();
    H.get_or_init(|| {
        let mut h = [[0f32; RS_T]; 3];
        for (p, row) in h.iter_mut().enumerate() {
            let mut w = [0f64; RS_T];
            let mut sum = 0.0;
            for (k, wk) in w.iter_mut().enumerate() {
                let x = p as f64 / 3.0 + (RS_T / 2) as f64 - 1.0 - k as f64;
                let fc = 0.9;
                let s = if x == 0.0 { fc } else { (std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x) };
                let t = (x + (RS_T / 2) as f64) / RS_T as f64;
                let win = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * t).cos() + 0.08 * (4.0 * std::f64::consts::PI * t).cos();
                *wk = s * win;
                sum += *wk;
            }
            for k in 0..RS_T {
                row[k] = (w[k] / sum) as f32;
            }
        }
        h
    })
}

/// 32 kHz to 48 kHz, stereo interleaved, the last RS_T - 1 input frames kept across calls.
fn up_3_2(up: &mut Up, input: &[i32]) -> Vec<i32> {
    let n = input.len() / 2;
    let h = up_taps();
    let nch = 2;
    up.buf.truncate(nch * (RS_T - 1));
    up.buf.resize(nch * (RS_T - 1), 0);
    up.buf.extend_from_slice(input);
    let mut out = Vec::with_capacity(n * 3);
    let mut pos = up.pos;
    while (pos as usize) < 3 * n {
        let x = nch * (pos as usize / 3);
        let c = &h[pos as usize % 3];
        for j in 0..nch {
            let mut v = 0f32;
            for (k, ck) in c.iter().enumerate() {
                v += ck * up.buf[x + nch * k + j] as f32;
            }
            out.push(v.round() as i32);
        }
        pos += 2;
    }
    up.pos = pos - 3 * n as u32;
    let tail = up.buf[nch * n..].to_vec();
    up.buf = tail;
    out
}

/// The DeviceSink's inputs folded to stereo 16-bit (48 kHz), or nothing when the list has no usable sink.
fn capture(a: &Ar, cl: u64, len: u64) -> Vec<i16> {
    if len < 0x20 {
        return vec![];
    }
    let ncmd = r32(cl + 4);
    let samples = r32(cl + 0x14) as usize;
    let rate = r32(cl + 0x18);
    let mix = r64(cl + 8);
    let end = cl + len;
    let mut l = LAYOUT.lock().unwrap();
    if !l.done && ncmd != 0 {
        // the header layout, from the first list that walks in exactly one (a list generated before the game's first
        // update can end in a zeroed command and walk in none)
        let mut t = *l;
        t.hdr6 = false;
        t.hdr21 = false;
        let w16 = cmd_walks(&t, cl, ncmd, end);
        t.hdr6 = true;
        let w6 = cmd_walks(&t, cl, ncmd, end);
        t.hdr6 = false;
        t.hdr21 = true;
        let w21 = cmd_walks(&t, cl, ncmd, end);
        if w16 as u32 + w6 as u32 + w21 as u32 == 1 {
            l.done = true;
            if w6 {
                l.hdr6 = true;
                l.sink_id = 0x10;
                crate::log!("audren: command headers in the SDK 6 layout (+4 size, +6 command ID, +0x10 enabled), DeviceSink is ID 0x10");
            }
            if w21 {
                l.hdr21 = true;
                crate::log!("audren: command headers in the SDK 21 layout (no magic: +0 enabled, +1 command ID, +2 size), DeviceSink 0x140 bytes");
            }
        } else {
            if !l.skipped_logged {
                l.skipped_logged = true;
                crate::log!(
                    "audren: a command list {ncmd} commands long walks in {} header layout (generated before the first update?): a silent frame, the layout comes from a later list",
                    if w16 || w6 || w21 { "more than one" } else { "no" }
                );
            }
            return vec![];
        }
    }
    if !l.done {
        return vec![];
    }
    let mut q = cl + 0x20;
    for i in 0..ncmd {
        if !cmd_ok(&l, q, end) {
            if !l.malformed_logged {
                l.malformed_logged = true;
                crate::log!("[RED] audren: command {i} of the list is malformed (offset {:#x}, list {len:#x} bytes)", q - cl);
            }
            return vec![];
        }
        q += cmd_size(&l, q) as u64;
    }
    if !l.hist_logged && ncmd > 4 {
        l.hist_logged = true;
        let mut hist = [0u32; 64];
        let (mut cand, mut ncand, mut cand4, mut ncand4) = (-1, 0, -1, 0);
        let sink_sz = 0x148 + sink_body(&l) as i32;
        let mut q = cl + 0x20;
        for _ in 0..ncmd {
            let sz = cmd_size(&l, q);
            hist[(cmd_id(&l, q) & 63) as usize] += 1;
            if sz == sink_sz && r32((q as i64 + 0x130 + sink_body(&l)) as u64).wrapping_sub(1) < 6 {
                cand = cmd_id(&l, q);
                ncand += 1;
            }
            if l.hdr6 && (sz == 0x130 || sz == 0x138) && r32(q + 0x120).wrapping_sub(1) < 6 {
                cand4 = cmd_id(&l, q);
                ncand4 += 1;
                l.sink5 = sz == 0x138;
            }
            q += sz as u64;
        }
        let line: String = hist.iter().enumerate().filter(|(_, n)| **n != 0).map(|(k, n)| format!(" {k}x{n}")).collect();
        crate::log!("audren: first full command list: {ncmd} commands, by command ID:{line}");
        let have = hist[(l.sink_id & 63) as usize] != 0;
        if !have && ncand == 1 {
            crate::log!("audren: no command with ID {:#x}; the only 0x148-byte command is ID {cand:#x}, taken as DeviceSink", l.sink_id);
            l.sink_id = cand;
        } else if !have && ncand == 0 && ncand4 == 1 {
            crate::log!(
                "audren: no command with ID {:#x}; the only {:#x}-byte command is ID {cand4:#x}, taken as the SDK {} DeviceSink",
                l.sink_id,
                if l.sink5 { 0x138 } else { 0x130 },
                if l.sink5 { 5 } else { 4 }
            );
            l.sink_id = cand4;
            l.sink4 = true;
        } else if !have {
            crate::log!("[RED] audren: no DeviceSink (ID {:#x}) in the command list, {ncand} commands of {sink_sz:#x} bytes: the game will have no sound", l.sink_id);
        }
    }
    let l = *l;
    let mut q = cl + 0x20;
    for _ in 0..ncmd {
        let size = cmd_size(&l, q);
        if cmd_id(&l, q) == l.sink_id && cmd_on(&l, q) {
            let b = (q as i64 + sink_body(&l)) as u64;
            let nin = r32(b + if l.sink4 { 0x120 } else { 0x130 });
            let mut inp = [0i64; 6];
            for (j, v) in inp.iter_mut().enumerate() {
                *v = if l.sink4 && !l.sink5 { r8(b + 0x124 + j as u64) as i8 as i64 } else { r16(b + if l.sink4 { 0x124 } else { 0x134 } + 2 * j as u64) as i16 as i64 };
            }
            if !a.sink_logged.swap(true, Ordering::Relaxed) {
                crate::log!(
                    "audren: renderer {}: DeviceSink inputs {nin} ({} {} ...), rate {rate}, {samples} samples a frame, mix buffers {mix:#x}{}",
                    a.idx,
                    inp[0],
                    inp[1],
                    if rate == 32000 { " (32 kHz: upsampled to 48 kHz by the host)" } else { "" }
                );
            }
            if nin == 0 || samples > if rate == 48000 { FRAME_MAX } else { FRAME_MAX * 2 / 3 - 1 } {
                return vec![];
            }
            if rate != 48000 && rate != 32000 {
                crate::log_once!("audren rate", "[RED] audren: renderer sample rate {rate} (the SDK takes 32000 or 48000): no sound");
                return vec![];
            }
            let k = (nin as usize).min(6);
            let ch: Vec<u64> = (0..k).map(|j| mix + (inp[j] as u64).wrapping_mul(samples as u64 * 4)).collect();
            let s32 = |c: u64, s: usize| r32(c + 4 * s as u64) as i32 as i64;
            // six channels to stereo with the system's default downmix (front 1.0, center 0.707, LFE 0.251, back 0.707, Q15)
            const CF: i64 = 32768;
            const CC: i64 = 23167;
            const CL: i64 = 8224;
            const CB: i64 = 23167;
            let mut st = Vec::with_capacity(samples * 2);
            for s in 0..samples {
                if k >= 6 {
                    let cm = s32(ch[2], s) * CC + s32(ch[3], s) * CL;
                    st.push(((s32(ch[0], s) * CF + cm + s32(ch[4], s) * CB) >> 15) as i32);
                    st.push(((s32(ch[1], s) * CF + cm + s32(ch[5], s) * CB) >> 15) as i32);
                } else {
                    st.push(s32(ch[0], s) as i32);
                    st.push(if k > 1 { s32(ch[1], s) } else { s32(ch[0], s) } as i32);
                }
            }
            let st = if rate == 48000 { st } else { up_3_2(&mut a.up.lock().unwrap(), &st) };
            let mut loud = false;
            let pcm: Vec<i16> = st
                .iter()
                .map(|&v| {
                    loud |= v != 0;
                    v.clamp(-32768, 32767) as i16
                })
                .collect();
            let f = a.frames.fetch_add(1, Ordering::Relaxed) + 1;
            if loud {
                a.nonsilent.fetch_add(1, Ordering::Relaxed);
            }
            if f == 200 || f % 12000 == 0 {
                crate::log!("audren: renderer {} rendered {f} frames, {} of them not silent", a.idx, a.nonsilent.load(Ordering::Relaxed));
            }
            return pcm;
        }
        q += size as u64;
    }
    vec![]
}

// ---- rendering

struct Fns {
    gen20: Option<u64>,
    mode: u64,
    device: u64,
    cbuf: u64,
    cbsz: u64,
    gen: u64,
    mu_off: u64,
    lock: u64,
    unlock: u64,
    tmax: u64,
    setup: u64,
    process: u64,
    proc_obj: u64,
}

/// adrp x16, page; ldr x17, [x16, #off]; ...; br x17 -> the function the GOT slot holds; else the address itself.
fn plt_target(pc: u64) -> u64 {
    let (a, l) = (r32(pc), r32(pc + 4));
    if a & 0x9f00_001f != 0x9000_0010 || l & 0xffc0_03ff != 0xf940_0211 {
        return pc;
    }
    let imm = (((((a as u64 >> 5) & 0x7ffff) << 2) | ((a as u64 >> 29) & 3)) << 43) as i64 >> 31;
    let page = (pc & !0xfff).wrapping_add(imm as u64);
    r64(page + ((l as u64 >> 10) & 0xfff) * 8)
}

/// The update lock a function takes first: `this` + imm moved to x0 before the first bl, to LockMutex or
/// SdkMutexType::Lock. (offset, is nn::os::MutexType)
fn lock_in(f: Option<u64>) -> Option<(u64, bool)> {
    let f = f?;
    let lock_mutex = crate::hle::sym("_ZN2nn2os9LockMutexEPNS0_9MutexTypeE");
    let sdk_lock = crate::hle::sym("_ZN2nn2os12SdkMutexType4LockEv");
    let mut this = 1u32;
    let mut add = [-1i64; 32];
    let mut x0 = -1i64;
    for i in 0..24u64 {
        let w = r32(f + 4 * i);
        let (rd, rn, rm) = ((w & 31) as usize, ((w >> 5) & 31) as usize, ((w >> 16) & 31) as usize);
        if w & 0xffe0_ffe0 == 0xaa00_03e0 {
            if this >> rm & 1 != 0 {
                this |= 1 << rd;
            }
            if rd == 0 {
                x0 = add[rm];
            } else {
                add[rd] = add[rm];
            }
        } else if w & 0xffc0_0000 == 0x9100_0000 && rn != 31 && this >> rn & 1 != 0 {
            add[rd] = ((w >> 10) & 0xfff) as i64;
            if rd == 0 {
                x0 = add[rd];
            }
        } else if w & 0xfc00_0000 == 0x9400_0000 {
            let off = ((w << 6) as i32 >> 6) as i64 * 4;
            let t = plt_target((f + 4 * i).wrapping_add(off as u64));
            let t = Some(t);
            if x0 < 0 || (t != lock_mutex && t != sdk_lock) {
                return None;
            }
            return Some((x0 as u64, t == lock_mutex));
        }
    }
    None
}

fn fns() -> &'static Fns {
    static F: OnceLock<Fns> = OnceLock::new();
    F.get_or_init(|| {
        let s = crate::hle::sym;
        let gen20 = s("_ZN2nn5audio6server17AudioRenderSystem15GenerateCommandENS_4SpanISt4byteEE");
        let mut mu_off = if gen20.is_some() { 0x2d0 } else { 0x1f0 };
        let found = lock_in(s(SYS_UPDATE)).or_else(|| lock_in(s(SYS_UPDATE_IMPL)));
        let os_mutex = match found {
            Some((o, m)) => {
                mu_off = o;
                m
            }
            None => {
                crate::log!("[RED] audren: no update lock read off AudioRenderSystem::Update or UpdateImpl: keeping +{mu_off:#x}, where command generation may race the game's RequestUpdate");
                false
            }
        };
        let (lock, unlock) = if os_mutex {
            (sdk("_ZN2nn2os9LockMutexEPNS0_9MutexTypeE"), sdk("_ZN2nn2os11UnlockMutexEPNS0_9MutexTypeE"))
        } else {
            (sdk("_ZN2nn2os12SdkMutexType4LockEv"), sdk("_ZN2nn2os12SdkMutexType6UnlockEv"))
        };
        let (mode, device, cbuf, cbsz, gen) = if gen20.is_some() {
            (0, 0, 0, 0, 0)
        } else {
            (
                sdk("_ZNK2nn5audio6server17AudioRenderSystem16GetExecutionModeEv"),
                sdk("_ZNK2nn5audio6server17AudioRenderSystem18GetRenderingDeviceEv"),
                sdk("_ZNK2nn5audio6server17AudioRenderSystem26GetCommandBufferWorkBufferEv"),
                sdk("_ZNK2nn5audio6server17AudioRenderSystem30GetCommandBufferWorkBufferSizeEv"),
                sdk("_ZN2nn5audio6server17AudioRenderSystem15GenerateCommandEPvm"),
            )
        };
        let mut l = LAYOUT.lock().unwrap();
        l.sink_id = if gen20.is_some() { 0x14 } else { 0x13 };
        drop(l);
        crate::log!(
            "audren: AudioRenderSystem in the SDK {} layout (update lock {} at +{mu_off:#x})",
            if gen20.is_some() { "20" } else { "16" },
            if os_mutex { "nn::os::MutexType" } else { "SdkMutexType" }
        );
        Fns {
            gen20,
            mode,
            device,
            cbuf,
            cbsz,
            gen,
            mu_off,
            lock,
            unlock,
            tmax: sdk("_ZN2nn5audio3dsp20CommandListProcessor17SetProcessTimeMaxEj"),
            setup: sdk("_ZN2nn5audio3dsp20CommandListProcessor5SetupEPKvm"),
            process: sdk("_ZN2nn5audio3dsp20CommandListProcessor7ProcessEv"),
            proc_obj: nxrt::kernel::get().space.kernel_alloc(0x100, 16),
        }
    })
}

static RENDER_MU: Mutex<()> = Mutex::new(());
static RENDER_TIME: Mutex<(u64, u64, u64)> = Mutex::new((0, 0, 0));

/// One frame: commands generated under the update lock, processed on the CPU, the DeviceSink taken.
unsafe fn render_once(c: *mut Cpu, a: &Ar, sys: u64) -> Option<(u64, Vec<i16>)> {
    let f = fns();
    let (md, dv) = match f.gen20 {
        Some(_) => (r32(sys + 0x4c), r32(sys + 0x48)),
        None => (crate::guest::call(c, f.mode, &[sys])? as u32, crate::guest::call(c, f.device, &[sys])? as u32),
    };
    if md != 1 || dv != 1 {
        return Some((0x40299, vec![]));
    }
    let _g = without_core(|| RENDER_MU.lock().unwrap());
    let t0 = std::time::Instant::now();
    crate::guest::call(c, f.lock, &[sys + f.mu_off])?;
    let (cl, n) = match f.gen20 {
        Some(g) => {
            let r = crate::guest::call(c, g, &[sys, r64(sys + 0xf8), r64(sys + 0x100)]);
            let n = (*c).x[1];
            (r?, n)
        }
        None => {
            let cb = crate::guest::call(c, f.cbuf, &[sys])?;
            let sz = crate::guest::call(c, f.cbsz, &[sys])?;
            (cb, crate::guest::call(c, f.gen, &[sys, cb, sz])?)
        }
    };
    crate::guest::call(c, f.unlock, &[sys + f.mu_off])?;
    a.renders.fetch_add(1, Ordering::Relaxed);
    crate::guest::zero(f.proc_obj, 0x100);
    crate::guest::call(c, f.tmax, &[f.proc_obj, 0xffff_ffff])?;
    crate::guest::call(c, f.setup, &[f.proc_obj, cl, n])?;
    crate::guest::call(c, f.process, &[f.proc_obj])?;
    let pcm = capture(a, cl, n);
    let d = t0.elapsed().as_nanos() as u64;
    let mut t = RENDER_TIME.lock().unwrap();
    t.0 += d;
    t.1 = t.1.max(d);
    t.2 += 1;
    if t.2 == 2000 {
        crate::log!(
            "audren: render time mean {:.2} ms / max {:.2} ms (5 ms budget a frame, last 2000 frames); catch-up frames so far {}",
            t.0 as f64 / 2000.0 / 1e6,
            t.1 as f64 / 1e6,
            a.catchups.load(Ordering::Relaxed)
        );
        *t = (0, 0, 0);
    }
    Some((RES_OK, pcm))
}

/// Where the AudioRenderSystem sits in AudioRendererImpl, read off the SDK's own ExecuteAudioRendererRendering.
fn impl_sys_off() -> u64 {
    static OFF: OnceLock<u64> = OnceLock::new();
    *OFF.get_or_init(|| {
        let mut off = 0x60;
        if let Some(f) = crate::hle::sym(IMPL_EXEC) {
            for i in 0..16 {
                let w = r32(f + 4 * i);
                if w & 0xffc0_03e0 == 0x9100_0000 {
                    off = ((w >> 10) & 0xfff) as u64;
                    break;
                }
                if w & 0x7c00_0000 == 0x1400_0000 {
                    break;
                }
            }
        }
        crate::log!("audren: AudioRenderSystem at +{off:#x} in AudioRendererImpl");
        off
    })
}

unsafe fn exec_sys(c: *mut Cpu, sys: u64) {
    let idx = T_AR.with(|t| t.get());
    let a = if idx != usize::MAX {
        let a = ARS.lock().unwrap().get(idx).copied();
        if let Some(a) = a {
            let _ = a.sys.compare_exchange(0, sys, Ordering::Relaxed, Ordering::Relaxed);
        }
        a
    } else {
        ar_of_sys(sys)
    };
    let Some(a) = a else {
        crate::log_once!("audren foreign", "[RED] audren: a render of a renderer the host did not open");
        (*c).x[0] = crate::hle::RES_FAIL;
        return;
    };
    let Some((r, pcm)) = render_once(c, a, sys) else { return };
    if idx == a.idx {
        *a.frame.lock().unwrap() = pcm;
    }
    (*c).x[0] = r;
}

unsafe fn impl_exec(c: *mut Cpu) {
    let sys = (*c).x[0] + impl_sys_off();
    exec_sys(c, sys)
}

unsafe fn sys_exec(c: *mut Cpu) {
    let sys = (*c).x[0];
    exec_sys(c, sys)
}

/// AudioRenderSystem::Update: a catch-up frame first when nothing was rendered since the last update.
unsafe fn sys_update(c: *mut Cpu) {
    let args: [u64; 7] = std::array::from_fn(|i| (*c).x[i]);
    let sys = args[0];
    if let Some(a) = ar_of_sys(sys) {
        let u = a.updates.fetch_add(1, Ordering::Relaxed) + 1;
        let r = a.renders.load(Ordering::Relaxed);
        if a.updated.load(Ordering::Relaxed) && r == a.last_r.load(Ordering::Relaxed) {
            let n = a.catchups.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 3 {
                crate::log!("audren: renderer {}: no frame rendered before update {u}, rendering a catch-up frame first ({n} so far)", a.idx);
            }
            // a full queue waits for the render thread to play one (every 5 ms), at most 50 ms
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
            without_core(|| {
                let mut q = a.cq.lock().unwrap();
                while a.open.load(Ordering::Relaxed) && q.len() >= CQ {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        break;
                    }
                    q = a.cq_cv.wait_timeout(q, deadline - now).unwrap().0;
                }
            });
            let Some((_, pcm)) = render_once(c, a, sys) else { return };
            let mut q = a.cq.lock().unwrap();
            if q.len() < CQ {
                q.push_back(pcm);
            }
        }
        let res = crate::guest::call(c, sdk(SYS_UPDATE), &args);
        a.last_r.store(a.renders.load(Ordering::Relaxed), Ordering::Relaxed);
        a.updated.store(true, Ordering::Relaxed);
        if let Some(r) = res {
            (*c).x[0] = r;
        }
        return;
    }
    if let Some(r) = crate::guest::call(c, sdk(SYS_UPDATE), &args) {
        (*c).x[0] = r;
    }
}

/// No upsampler manager with CPU rendering: answer "none" (the generator then skips the upsample command and the
/// host upsamples the stereo fold itself).
unsafe fn upsampler_alloc(c: *mut Cpu) {
    let mgr = (*c).x[0];
    if mgr == 0 {
        (*c).x[0] = 0;
        return;
    }
    if let Some(r) = crate::guest::call(c, sdk(UPSAMPLER_ALLOC), &[mgr]) {
        (*c).x[0] = r;
    }
}

/// Each renderer's output statistics for the log.
pub fn report() -> Vec<String> {
    ARS.lock()
        .unwrap()
        .iter()
        .map(|a| {
            format!(
                "audren: renderer {}: {} frames rendered, {} not silent, {} updates, {} catch-up frames",
                a.idx,
                a.frames.load(Ordering::Relaxed),
                a.nonsilent.load(Ordering::Relaxed),
                a.updates.load(Ordering::Relaxed),
                a.catchups.load(Ordering::Relaxed)
            )
        })
        .collect()
}

pub fn register(r: &mut Registry) {
    const W: &str = "audren: the SDK's renderer in-process (CPU, manual), its DeviceSink to the sound card";
    for (n, f) in [
        (WORK_SIZE, work_size as unsafe fn(*mut Cpu)),
        (OPEN_EV, open_ev),
        (OPEN, open_noev),
        (CLOSE, close),
        (IMPL_EXEC, impl_exec),
        (SYS_EXEC, sys_exec),
        (SYS_UPDATE, sys_update),
        (UPSAMPLER_ALLOC, upsampler_alloc),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_budget() {
        // 48 kHz, 240 samples, 24 mix buffers, 1 submix, 96 voices, 1 sink, 4 effects, revision 5
        let mut p = vec![0u8; 0x40];
        for (i, v) in [48000i32, 240, 24, 1, 96, 1, 4, 0].iter().enumerate() {
            p[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        p[0x34..0x38].copy_from_slice(&0x3556_4552u32.to_le_bytes());
        let n = param_service_size(&p);
        assert!(n > 0x18000 && n % 0x1000 == 0, "{n:#x}");
        p[0x34..0x38].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(param_service_size(&p), 0, "unknown revision");
    }

    fn cmd6(id: i16, size: i16) -> Vec<u8> {
        let mut v = vec![0u8; size as usize];
        v[0..4].copy_from_slice(&0xcafe_babeu32.to_le_bytes());
        v[4..6].copy_from_slice(&size.to_le_bytes());
        v[6..8].copy_from_slice(&id.to_le_bytes());
        v[0x10] = 1;
        v
    }

    fn ar() -> Ar {
        Ar {
            idx: 0,
            handle: [0; 2],
            sys: AtomicU64::new(0),
            open: AtomicBool::new(true),
            thread: AtomicU64::new(0),
            frame: Mutex::new(vec![]),
            cq: Mutex::new(VecDeque::new()),
            cq_cv: Condvar::new(),
            frames: AtomicU64::new(0),
            nonsilent: AtomicU64::new(0),
            updates: AtomicU64::new(0),
            renders: AtomicU64::new(0),
            catchups: AtomicU64::new(0),
            last_r: AtomicU64::new(0),
            updated: AtomicBool::new(false),
            sink_logged: AtomicBool::new(false),
            up: Mutex::new(Up { buf: vec![], pos: 0 }),
            out: Mutex::new(None),
        }
    }

    /// SDK 4 (Mario Kart 8 Deluxe): SDK 6 headers, DeviceSink ID 0xf of 0x130 bytes with s8 inputs.
    #[test]
    fn sdk4_device_sink() {
        let samples = 240usize;
        let mut mix = vec![0i32; samples * 4];
        for s in 0..samples {
            mix[2 * samples + s] = 1000; // buffer 2: left
            mix[3 * samples + s] = -1000; // buffer 3: right
        }
        let mut list = vec![0u8; 0x20];
        let mut cmds = vec![];
        for id in [1i16, 2, 3, 4, 5] {
            cmds.extend(cmd6(id, 0x40));
        }
        let mut sinkc = cmd6(0xf, 0x130);
        sinkc[0x120..0x124].copy_from_slice(&2u32.to_le_bytes());
        sinkc[0x124] = 2;
        sinkc[0x125] = 3;
        cmds.extend(sinkc);
        list.extend(cmds);
        let ncmd = 6u32;
        list[4..8].copy_from_slice(&ncmd.to_le_bytes());
        list[8..16].copy_from_slice(&(mix.as_ptr() as u64).to_le_bytes());
        list[0x14..0x18].copy_from_slice(&(samples as u32).to_le_bytes());
        list[0x18..0x1c].copy_from_slice(&48000u32.to_le_bytes());
        let a = ar();
        let pcm = capture(&a, list.as_ptr() as u64, list.len() as u64);
        assert_eq!(pcm.len(), samples * 2);
        assert_eq!((pcm[0], pcm[1]), (1000, -1000));
        let l = LAYOUT.lock().unwrap();
        assert!(l.hdr6 && l.sink4 && l.sink_id == 0xf);
    }

    #[test]
    fn upsample_32k() {
        let mut up = Up { buf: vec![], pos: 0 };
        let mut n = 0;
        for _ in 0..10 {
            n += up_3_2(&mut up, &vec![1000i32; 160 * 2]).len() / 2;
        }
        assert_eq!(n, 2400, "160 frames at 32 kHz make 240 at 48 kHz");
    }
}
