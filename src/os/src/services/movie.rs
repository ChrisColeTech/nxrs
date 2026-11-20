//! The SDK's movie library (`movie::Decoder`, NintendoSDK_movie in a subsdk: Mario Kart 8 Deluxe's menu previews,
//! Unity's and other engines' movies), taken over and decoded on the host (nxnative `hle_movie.c`, ported). On a
//! console it wraps NVIDIA's hardware decoder (a MediaCodec-like protocol over nvdrv), which the host does not have.
//!
//! The protocol, as the SDK's Decoder has it: the client registers 4 nn::os events {input available, output ready,
//! format changed, error}; inputs: AcquireInputBufferIndex -> GetInputBuffer -> SendInputBufferForDecode(index,
//! offset, size, time in µs, flags); outputs: AcquireOutputBufferIndex -> GetOutputBufferFormat / GetOutputBuffer
//! (NV12 into the client's buffer, RGBA into its NVN texture in the native-texture mode) -> ReleaseOutputBufferIndex.
//! Packets queue in arrival order; whenever an output slot is free the head packet is decoded and its input index
//! returned; while the client holds every slot packets stay queued (MediaCodec's back-pressure).
//!
//! Codecs: VP8 and VP9 with libvpx, H.264 with Windows' Media Foundation decoder (pictures in presentation order),
//! Vorbis with lewton, AAC with Media Foundation; PCM16 out for audio. Each decoder decodes on a host thread of its own; video results
//! are signalled by a guest thread of the host's (SignalEvent is guest code), audio ones by the caller.

use crate::guest::{r64, w32, w64, Args};
use crate::hle::Registry;
use crate::media::{mf, vpx, yuv::Picture};
use crate::sched::without_core;
use nxrt::cpu::Cpu;
use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const NIN: usize = 4;
const INCAP: usize = 4 << 20;
const NOUT: usize = 64;
const NOUT_AAC: usize = 32;
const NOUT_VIDEO: usize = 16;
const NO_INPUT: u64 = 0x1011;
const NO_OUTPUT: u64 = 0x100e;
const BAD_ARG: u64 = 0x100b;
const NO_IMPL: u64 = 0x1000;
const STALE: Duration = Duration::from_millis(250);
const FMT_NV12: i32 = 4;
const FMT_ABGR: i32 = 3;
const FMT_PCM16: i32 = 1;
const MODE_TEX: i32 = 2;
const T_AVC: i32 = 1;
const T_VP8: i32 = 2;
const T_VP9: i32 = 3;
const T_AAC: i32 = 4;
const T_VORBIS: i32 = 5;
const PCM_CAP: usize = 8192 * 8 * 2;
const EOS: u32 = 4;

// ---- the SDK's helpers (guest code)

struct G {
    set_data: u64,
    set_range: u64,
    data: u64,
    cap: u64,
    size: u64,
    set_i32: u64,
    set_rect: u64,
    find_i32: u64,
    find_buf: u64,
    signal: u64,
    nvn_sync: Option<u64>,
}

fn g() -> &'static G {
    static S: OnceLock<G> = OnceLock::new();
    S.get_or_init(|| {
        let need = |n: &str| crate::hle::sym(n).unwrap_or_else(|| nxrt::rt::fatal(&format!("movie: the SDK has no {n}")));
        let either = |a: &str, b: &str| crate::hle::sym(a).or_else(|| crate::hle::sym(b)).unwrap_or_else(|| nxrt::rt::fatal(&format!("movie: the SDK has no {a}")));
        G {
            set_data: need("_ZN5movie6Buffer18SetDataAndCapacityEPvm"),
            set_range: need("_ZN5movie6Buffer8SetRangeEii"),
            data: either("_ZNK5movie6Buffer4DataEv", "_ZN5movie6Buffer4DataEv"),
            cap: either("_ZNK5movie6Buffer8CapacityEv", "_ZN5movie6Buffer8CapacityEv"),
            size: either("_ZNK5movie6Buffer4SizeEv", "_ZN5movie6Buffer4SizeEv"),
            set_i32: need("_ZN5movie9MediaData8SetInt32EPKci"),
            set_rect: need("_ZN5movie9MediaData7SetRectEPKciiii"),
            find_i32: need("_ZNK5movie9MediaData9FindInt32EPKcPi"),
            find_buf: need("_ZNK5movie9MediaData10FindBufferEPKcPPNS_6BufferE"),
            signal: need("_ZN2nn2os11SignalEventEPNS0_9EventTypeE"),
            nvn_sync: crate::hle::sym("_ZNK5movie6Buffer10GetNvnSyncEv"),
        }
    })
}

/// A NUL-terminated key the guest can read.
fn key(k: &str) -> u64 {
    static KEYS: Mutex<Vec<CString>> = Mutex::new(Vec::new());
    let mut g = KEYS.lock().unwrap();
    if let Some(c) = g.iter().find(|c| c.to_bytes() == k.as_bytes()) {
        return c.as_ptr() as u64;
    }
    let c = CString::new(k).unwrap();
    let p = c.as_ptr() as u64;
    g.push(c);
    p
}

/// A scratch guest word (an out parameter for the SDK's getters).
fn scratch() -> u64 {
    thread_local! { static S: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
    S.with(|s| {
        if s.get() == 0 {
            s.set(nxrt::kernel::get().space.kernel_alloc(16, 16));
        }
        s.get()
    })
}

unsafe fn md_i32(c: *mut Cpu, md: u64, k: &str) -> Option<i32> {
    let o = scratch();
    w32(o, 0);
    let r = crate::guest::call(c, g().find_i32, &[md, key(k), o])?;
    (r as u8 != 0).then(|| crate::guest::r32(o) as i32)
}
unsafe fn md_buf(c: *mut Cpu, md: u64, k: &str) -> Option<Vec<u8>> {
    let o = scratch();
    w64(o, 0);
    let r = crate::guest::call(c, g().find_buf, &[md, key(k), o])?;
    let b = r64(o);
    if r as u8 == 0 || b == 0 {
        return None;
    }
    let data = crate::guest::call(c, g().data, &[b])?;
    let n = crate::guest::call(c, g().size, &[b])?;
    (data != 0 && n != 0).then(|| crate::guest::bytes(data, n).to_vec())
}
unsafe fn md_set(c: *mut Cpu, md: u64, k: &str, v: i32) {
    let _ = crate::guest::call(c, g().set_i32, &[md, key(k), v as u32 as u64]);
}

// ---- one decoder

#[derive(Default, Clone)]
struct Slot {
    used: bool,
    pic: bool,
    cli: bool,
    rnd: bool,
    pts: i64,
    flags: u32,
    frm: Vec<u8>,
    bytes: usize,
    lit: bool,
}

#[derive(Clone, Copy)]
struct Pkt {
    inp: usize,
    off: usize,
    size: usize,
    pts: i64,
    flags: u32,
}

enum Codec {
    None,
    Vpx(vpx::Vpx),
    Avc(Box<mf::H264>),
    Vorbis(Box<VorbisDec>),
    Aac(mf::Aac),
}

struct VorbisDec {
    ident: lewton::header::IdentHeader,
    setup: lewton::header::SetupHeader,
    pwr: lewton::audio::PreviousWindowRight,
}

#[derive(Default)]
struct St {
    evc: u64,
    ev: [u64; 4],
    w: i32,
    h: i32,
    fmt_sent: bool,
    started: bool,
    in_eos: bool,
    infree: Vec<usize>,
    inown: [bool; NIN],
    instale: [Option<Instant>; NIN],
    out: Vec<Slot>,
    outq: VecDeque<usize>,
    pend: VecDeque<Pkt>,
    avc_drain: bool,
    drain_pts: i64,
    tex_rel: bool,
    csd: [Vec<u8>; 2],
    avcc: Vec<u8>,
    ch: i32,
    rate: i32,
    hold: Vec<i16>,
    bt709: Option<bool>,
    full: bool,
    /// events to signal: (event index), video: by the signaller thread; audio: by the caller
    events: Vec<usize>,
    // the decoder thread's work
    work: bool,
    busy: bool,
    reset: Option<bool>,
    quit: bool,
    // statistics
    packets: u64,
    frames: u64,
    lit: u64,
    dec_n: u64,
    dec_ns: u64,
    dec_max: u64,
    red_dec: u32,
}

pub struct Dec {
    id: u64,
    obj: u64,
    ty: i32,
    fmt: i32,
    mode: i32,
    nout: usize,
    out_align: i32,
    audio: bool,
    inbuf: Vec<Box<[u8]>>,
    st: Mutex<St>,
    cv: Condvar,
    calls: [AtomicU64; 16],
}

impl Dec {
    fn trace(&self, slot: usize, msg: impl FnOnce() -> String) {
        let n = self.calls[slot].fetch_add(1, Ordering::Relaxed) + 1;
        if crate::trace::on("movie") || n <= 12 || n.is_power_of_two() {
            crate::log!("[movie] decoder {:#x}: {} (#{n})", self.obj, msg());
        }
    }
    fn input_addr(&self, i: usize) -> u64 {
        self.inbuf[i].as_ptr() as u64
    }
}

static DECS: Mutex<Option<HashMap<u64, Arc<Dec>>>> = Mutex::new(None);

/// Whether a movie decoder is open (a movie plays): game fixes keep a movie's own picture (os `gamefix`).
pub fn playing() -> bool {
    DECS.lock().unwrap().as_ref().is_some_and(|m| !m.is_empty())
}
static NEXT: AtomicU64 = AtomicU64::new(1);

fn dec_of(self_: u64) -> Option<Arc<Dec>> {
    if self_ == 0 {
        return None;
    }
    let id = r64(self_ + 8);
    DECS.lock().unwrap().as_ref()?.get(&id).cloned()
}

// ---- events: signalled by a guest thread of the host's (video), or by the caller (audio)

struct Signal {
    dec: u64,
    ev: usize,
    due: Instant,
}
static SIGQ: Mutex<Vec<Signal>> = Mutex::new(Vec::new());
static SIGCV: Condvar = Condvar::new();
static SIG_THREAD: AtomicBool = AtomicBool::new(false);
/// Held while a signal is delivered: Stop takes it, so no signal reaches a stopped decoder.
static SIG_MU: Mutex<()> = Mutex::new(());

fn ev_ok(p: u64) -> bool {
    p >= 0x1_0000_0000 && p & 7 == 0 && crate::guest::readable(p, 8)
}

/// Signals event `i` of `d`: the client's entry read now (it may have filled its struct after registering), else
/// the one registered.
unsafe fn md_event(c: *mut Cpu, d: &Dec, i: usize) {
    let (evc, reg) = {
        let st = d.st.lock().unwrap();
        (st.evc, st.ev[i])
    };
    let live = if evc != 0 && crate::guest::readable(evc + 8 * i as u64, 8) { r64(evc + 8 * i as u64) } else { 0 };
    let p = if live != 0 && ev_ok(live) { live } else { reg };
    if p != 0 {
        let _ = crate::guest::call(c, g().signal, &[p]);
    } else if live != 0 {
        crate::log_once!(format!("movie ev {:#x} {i}", d.obj), "[RED] movie: decoder {:#x} event {i} not delivered: the client's entry {live:#x} is not an event", d.obj);
    }
}

fn defer(d: &Dec, ev: usize, due: Instant) {
    let mut q = SIGQ.lock().unwrap();
    if q.iter().any(|s| s.dec == d.id && s.ev == ev && s.due <= due) {
        return;
    }
    q.push(Signal { dec: d.id, ev, due });
    SIGCV.notify_all();
}
fn defer_drop(d: &Dec) {
    SIGQ.lock().unwrap().retain(|s| s.dec != d.id);
}

unsafe fn signaller(c: *mut Cpu) {
    loop {
        let next = without_core(|| {
            let mut q = SIGQ.lock().unwrap();
            loop {
                let now = Instant::now();
                if let Some(k) = q.iter().position(|s| s.due <= now) {
                    return q.remove(k);
                }
                let wait = q.iter().map(|s| s.due - now).min().unwrap_or(Duration::from_millis(100));
                q = SIGCV.wait_timeout(q, wait).unwrap().0;
            }
        });
        let Some(d) = DECS.lock().unwrap().as_ref().and_then(|m| m.get(&next.dec).cloned()) else { continue };
        let _g = without_core(|| SIG_MU.lock().unwrap());
        if d.st.lock().unwrap().started {
            md_event(c, &d, next.ev);
        }
    }
}

unsafe fn ensure_signaller(c: *mut Cpu) {
    if !SIG_THREAD.swap(true, Ordering::SeqCst) && crate::guest::spawn(c, "movie signaller", signaller, 0, 16, 0x10000).is_none() {
        SIG_THREAD.store(false, Ordering::SeqCst);
    }
}

/// The events a decoder pass raised: audio ones now on the caller, video ones by the signaller.
unsafe fn deliver(c: *mut Cpu, d: &Dec) {
    let evs = std::mem::take(&mut d.st.lock().unwrap().events);
    for e in evs {
        if d.audio {
            md_event(c, d, e);
        } else {
            defer(d, e, Instant::now());
        }
    }
}

// ---- decoding (the decoder's own thread)

fn open_codec(d: &Dec, st: &St) -> Result<Codec, String> {
    match d.ty {
        T_VP9 => vpx::Vpx::vp9(st.w.max(0) as u32, st.h.max(0) as u32).map(Codec::Vpx),
        T_VP8 => vpx::Vpx::vp8(st.w.max(0) as u32, st.h.max(0) as u32).map(Codec::Vpx),
        T_AVC => {
            let mut a = mf::H264::new()?;
            let mut ps = vec![];
            for c in &st.csd {
                ps.extend_from_slice(c);
            }
            ps.extend(avcc_annexb(&st.avcc));
            if !ps.is_empty() {
                let _ = a.decode(&ps, 0);
            }
            Ok(Codec::Avc(Box::new(a)))
        }
        T_VORBIS => {
            let ident = lewton::header::read_header_ident(&st.csd[0]).map_err(|e| format!("Vorbis identification header: {e:?}"))?;
            let setup = lewton::header::read_header_setup(&st.csd[1], ident.audio_channels, (ident.blocksize_0, ident.blocksize_1))
                .map_err(|e| format!("Vorbis setup header: {e:?}"))?;
            crate::log!("[movie] Vorbis: {} channels {} Hz (lewton)", ident.audio_channels, ident.audio_sample_rate);
            Ok(Codec::Vorbis(Box::new(VorbisDec { ident, setup, pwr: lewton::audio::PreviousWindowRight::new() })))
        }
        T_AAC => {
            let asc = if st.csd[0].len() >= 2 { st.csd[0].clone() } else { mf::aac_lc_config(st.rate.max(8000) as u32, st.ch.max(1) as u32).to_vec() };
            let (rate, ch) = aac_shape(&asc, st.rate, st.ch);
            crate::log!("[movie] AAC: {ch} channels {rate} Hz, AudioSpecificConfig {} bytes (object type {})", asc.len(), asc[0] >> 3);
            mf::Aac::new(rate as u32, ch as u32, &asc).map(Codec::Aac)
        }
        _ => Err(format!("decoder type {}", d.ty)),
    }
}

/// The rate and channels an AudioSpecificConfig names (what Configure did not).
fn aac_shape(asc: &[u8], rate: i32, ch: i32) -> (i32, i32) {
    const RATES: [i32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
    let (mut r, mut c) = (rate, ch);
    if asc.len() >= 2 {
        let fi = (((asc[0] & 7) << 1) | (asc[1] >> 7)) as usize;
        let cc = (asc[1] >> 3) & 15;
        if r <= 0 && fi < 13 {
            r = RATES[fi];
        }
        if c <= 0 && cc > 0 && cc < 8 {
            c = if cc == 7 { 8 } else { cc as i32 };
        }
    }
    (if r > 0 { r } else { 48000 }, if c > 0 { c } else { 2 })
}

/// An AVCDecoderConfigurationRecord's parameter sets with Annex B start codes.
fn avcc_annexb(p: &[u8]) -> Vec<u8> {
    let mut v = vec![];
    if p.len() < 7 {
        return v;
    }
    let mut i = 6;
    let mut sets = vec![(p[5] & 31) as usize];
    let mut k = 0;
    while k < sets.len() {
        for _ in 0..sets[k] {
            if i + 2 > p.len() {
                return v;
            }
            let n = u16::from_be_bytes([p[i], p[i + 1]]) as usize;
            i += 2;
            if i + n > p.len() {
                return v;
            }
            v.extend_from_slice(&[0, 0, 0, 1]);
            v.extend_from_slice(&p[i..i + n]);
            i += n;
        }
        if k == 0 && i < p.len() {
            sets.push(p[i] as usize);
            i += 1;
        }
        k += 1;
    }
    v
}

/// A picture into a slot (NV12 stride w, or RGBA).
fn put_picture(d: &Dec, st: &mut St, slot: usize, p: &Picture) {
    let (w, h) = (st.w.max(1) as usize, st.h.max(1) as usize);
    let rgba = d.fmt == FMT_ABGR;
    let need = if rgba { w * h * 4 } else { w * h * 3 / 2 };
    let s = &mut st.out[slot];
    s.frm.resize(need, 0);
    if rgba {
        p.rgba_into(&mut s.frm, w, h);
    } else {
        p.nv12_into(&mut s.frm, w, h);
    }
    s.lit = p.lit();
    st.bt709 = p.bt709;
    st.full = p.full_range;
}

/// Decodes one packet (or, with `take_only`, takes the next queued H.264 picture) into `slot`; true when the
/// slot got output. `pts_out` is the picture's own time (H.264 reorders).
fn decode_one(d: &Dec, codec: &mut Codec, st: &mut St, data: &[u8], pts: i64, slot: usize, flush: bool, take_only: bool, pts_out: &mut i64) -> bool {
    match codec {
        Codec::None => false,
        Codec::Vpx(v) => match v.decode(data, pts) {
            Ok(Some(p)) => {
                if (p.w as i32, p.h as i32) != (st.w, st.h) {
                    crate::log_once!(format!("movie size {:#x}", d.obj), "[RED] movie: {} picture {}x{} does not match the output {}x{}: cropped / padded", v.name, p.w, p.h, st.w, st.h);
                }
                put_picture(d, st, slot, &p);
                *pts_out = pts;
                true
            }
            Ok(None) => false,
            Err(e) => {
                st.red_dec += 1;
                if st.red_dec == 1 {
                    crate::log!("[RED] movie: VP9 decode error ({} bytes): {e}", data.len());
                }
                false
            }
        },
        Codec::Avc(a) => {
            if !take_only && !data.is_empty() {
                if let Err(e) = a.decode(data, pts) {
                    st.red_dec += 1;
                    if st.red_dec == 1 {
                        crate::log!("[RED] movie: H.264 decode error ({} bytes): {e}", data.len());
                    }
                }
            }
            if flush {
                a.drain();
            }
            match a.out.pop_front() {
                Some(p) => {
                    if st.w <= 0 {
                        st.w = p.w as i32;
                        st.h = p.h as i32;
                    }
                    put_picture(d, st, slot, &p);
                    *pts_out = p.pts;
                    true
                }
                None => false,
            }
        }
        Codec::Vorbis(v) => {
            let prev_n = st.hold.len();
            match lewton::audio::read_audio_packet_generic::<lewton::samples::InterleavedSamples<i16>>(&v.ident, &v.setup, data, &mut v.pwr) {
                Ok(s) => {
                    st.hold.extend_from_slice(&s.samples);
                }
                Err(e) => {
                    st.red_dec += 1;
                    if st.red_dec == 1 {
                        crate::log!("[RED] movie: Vorbis packet of {} bytes does not decode ({e:?})", data.len());
                    }
                }
            }
            let _ = prev_n;
            if st.hold.is_empty() {
                return false;
            }
            let pcm = std::mem::take(&mut st.hold);
            let s = &mut st.out[slot];
            s.frm = pcm.iter().flat_map(|x| x.to_le_bytes()).collect();
            s.bytes = s.frm.len();
            *pts_out = pts;
            true
        }
        Codec::Aac(a) => match a.decode(data) {
            Ok(pcm) if !pcm.is_empty() => {
                let s = &mut st.out[slot];
                s.bytes = pcm.len();
                s.frm = pcm;
                *pts_out = pts;
                true
            }
            Ok(_) => false,
            Err(e) => {
                st.red_dec += 1;
                if st.red_dec == 1 {
                    crate::log!("[RED] movie: AAC decode error: {e}");
                }
                false
            }
        },
    }
}

fn avc_queued(codec: &Codec) -> bool {
    matches!(codec, Codec::Avc(a) if !a.out.is_empty())
}

/// While packets are queued and an output slot is free: decode the head packet; the input and output events go
/// into st.events (nxnative md_pump).
fn pump(d: &Dec, codec: &mut Codec, st: &mut St) {
    loop {
        let free = (0..d.nout).find(|&k| !st.out[k].used);
        if st.avc_drain {
            let Some(s) = free else { break };
            st.out[s].used = true;
            let mut t = 0;
            let got = decode_one(d, codec, st, &[], 0, s, false, true, &mut t);
            st.out[s].pic = got;
            st.out[s].pts = if got { t } else { st.drain_pts };
            st.out[s].flags = if got { 0 } else { EOS };
            st.outq.push_back(s);
            if !got {
                st.avc_drain = false;
            }
            st.events.push(1);
            continue;
        }
        // an H.264 picture that is due without a packet leaves in a free slot of its own
        if !st.in_eos && avc_queued(codec) {
            let Some(s) = free else { break };
            st.out[s].used = true;
            let mut t = 0;
            if decode_one(d, codec, st, &[], 0, s, false, true, &mut t) {
                st.out[s].pic = true;
                st.out[s].pts = t;
                st.out[s].flags = 0;
                st.outq.push_back(s);
                st.events.push(1);
                continue;
            }
            st.out[s].used = false;
        }
        if st.pend.is_empty() {
            break;
        }
        let Some(slot) = free else { break };
        let mut pk = st.pend.pop_front().unwrap();
        st.out[slot].used = true;
        let avc = d.ty == T_AVC;
        let eos = pk.flags & EOS != 0;
        if eos && (pk.pts < 0 || pk.size >= INCAP) {
            pk.size = 0;
        }
        let mut got = false;
        let mut t = pk.pts;
        if pk.size > 0 || (avc && eos) {
            let data = d.inbuf[pk.inp][pk.off..pk.off + pk.size].to_vec();
            let t0 = Instant::now();
            got = decode_one(d, codec, st, &data, pk.pts, slot, avc && eos, false, &mut t);
            let dt = t0.elapsed().as_nanos() as u64;
            st.dec_n += 1;
            st.dec_ns += dt;
            st.dec_max = st.dec_max.max(dt);
        }
        let more = avc && eos && got;
        st.infree.push(pk.inp);
        let ready = got || eos;
        if ready {
            let s = &mut st.out[slot];
            s.pic = got;
            s.pts = if got && avc { t } else { pk.pts };
            s.flags = if more { pk.flags & !EOS } else { pk.flags };
            st.outq.push_back(slot);
            if got && !d.audio {
                st.frames += 1;
                if st.out[slot].lit {
                    st.lit += 1;
                }
            }
        } else {
            st.out[slot].used = false;
        }
        if more {
            st.avc_drain = true;
            st.drain_pts = pk.pts;
        }
        if !st.in_eos {
            st.events.push(0);
        }
        if ready {
            st.events.push(1);
        }
    }
}

fn worker(d: Arc<Dec>) {
    let mut codec = Codec::None;
    let mut opened = false;
    loop {
        let mut st = d.st.lock().unwrap();
        while !st.work && st.reset.is_none() && !st.quit {
            st = d.cv.wait(st).unwrap();
        }
        if st.quit {
            st.busy = false;
            d.cv.notify_all();
            break;
        }
        if let Some(close) = st.reset.take() {
            match &mut codec {
                Codec::Avc(a) if !close => a.flush(),
                Codec::Aac(a) if !close => a.flush(),
                Codec::Vorbis(v) if !close => v.pwr = lewton::audio::PreviousWindowRight::new(),
                _ => {}
            }
            if close {
                codec = Codec::None;
                opened = false;
            }
            d.cv.notify_all();
            continue;
        }
        st.work = false;
        st.busy = true;
        if !opened && (!st.pend.is_empty()) {
            opened = true;
            match open_codec(&d, &st) {
                Ok(c) => {
                    codec = c;
                    crate::log!(
                        "[movie] decoder {:#x}: {} with {}",
                        d.obj,
                        ["?", "H.264", "VP8", "VP9", "AAC", "Vorbis"].get(d.ty as usize).unwrap_or(&"?"),
                        match d.ty {
                            T_VP9 | T_VP8 => format!("libvpx {}", vpx::version()),
                            T_AVC | T_AAC => "Media Foundation".into(),
                            _ => "lewton".into(),
                        }
                    );
                }
                Err(e) => crate::log!("[RED] movie: decoder {:#x} cannot decode: {e}", d.obj),
            }
        }
        pump(&d, &mut codec, &mut st);
        st.busy = false;
        drop(st);
        d.cv.notify_all();
        if !d.audio {
            deliver_now(&d);
        }
    }
}

/// From the decoder thread: video events go to the signaller.
fn deliver_now(d: &Dec) {
    let evs = std::mem::take(&mut d.st.lock().unwrap().events);
    for e in evs {
        defer(d, e, Instant::now());
    }
}

/// Wakes the decoder thread; audio decoders wait for its pass (their players feed one packet per signal) and
/// signal its events on the caller.
unsafe fn kick(c: *mut Cpu, d: &Dec) {
    if !d.audio {
        let mut st = d.st.lock().unwrap();
        st.work = true;
        d.cv.notify_all();
        return;
    }
    without_core(|| {
        let mut st = d.st.lock().unwrap();
        st.work = true;
        d.cv.notify_all();
        while st.work || st.busy {
            st = d.cv.wait(st).unwrap();
        }
    });
    deliver(c, d);
}

/// Clears queues and slots; `close` drops the codec too (rebuilt at the next packet).
fn reset(d: &Dec, close: bool) {
    let mut st = d.st.lock().unwrap();
    while st.busy {
        st = d.cv.wait(st).unwrap();
    }
    st.reset = Some(close);
    d.cv.notify_all();
    while st.reset.is_some() {
        st = d.cv.wait(st).unwrap();
    }
    let now = Instant::now();
    st.infree = (0..NIN).collect();
    for i in 0..NIN {
        st.instale[i] = if st.inown[i] { Some(now) } else { None };
        st.inown[i] = false;
    }
    st.outq.clear();
    st.pend.clear();
    st.avc_drain = false;
    st.in_eos = false;
    st.hold.clear();
    for s in st.out.iter_mut() {
        s.used = false;
        s.cli = false;
        s.rnd = false;
        if close {
            s.pic = false;
        }
    }
}

// ---- the takeovers

const VT: &str = "_ZTVN5movie7DecoderE";

unsafe fn construct(c: *mut Cpu, self_: u64, ty: i32, fmt: i32, mode: i32, prio: i32, out_align: i32) {
    let vt = crate::hle::sym(VT).unwrap_or_else(|| nxrt::rt::fatal("movie: the SDK has no movie::Decoder vtable")) + 0x10;
    let _ = g();
    w64(self_, vt);
    w64(self_ + 8, 0);
    let audio = (ty == T_VORBIS || ty == T_AAC) && fmt == FMT_PCM16;
    if !audio && (!(1..=3).contains(&ty) || (fmt != FMT_NV12 && !(fmt == FMT_ABGR && mode == MODE_TEX))) {
        crate::log!("[RED] movie: decoder type {ty} output {fmt} mode {mode} is not one the host decodes: every call fails");
        return;
    }
    ensure_signaller(c);
    make(self_, ty, fmt, mode, prio, out_align);
}

/// The host's decoder behind client object `self_` (its id at +8), with its decoding thread.
fn make(self_: u64, ty: i32, fmt: i32, mode: i32, prio: i32, out_align: i32) -> Arc<Dec> {
    let audio = (ty == T_VORBIS || ty == T_AAC) && fmt == FMT_PCM16;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let nout = if !audio { NOUT_VIDEO } else if ty == T_AAC { NOUT_AAC } else { NOUT };
    let d = Arc::new(Dec {
        id,
        obj: self_,
        ty,
        fmt,
        mode,
        nout,
        out_align,
        audio,
        inbuf: (0..NIN).map(|_| vec![0u8; INCAP].into_boxed_slice()).collect(),
        st: Mutex::new(St { infree: (0..NIN).collect(), out: vec![Slot::default(); NOUT], ..Default::default() }),
        cv: Condvar::new(),
        calls: Default::default(),
    });
    DECS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id, d.clone());
    w64(self_ + 8, id);
    let dw = d.clone();
    std::thread::Builder::new().name("nxos movie decoder".into()).stack_size(4 << 20).spawn(move || worker(dw)).expect("movie decoder thread");
    crate::log!("[movie] decoder {self_:#x}: type {ty} output {fmt} mode {mode} priority {prio}");
    d
}

unsafe fn ctor(c: *mut Cpu) {
    let a = Args(&mut *c);
    construct(c, a.x(0), a.i(1), a.i(2), a.i(3), a.i(4), 0);
}
unsafe fn ctor4(c: *mut Cpu) {
    let a = Args(&mut *c);
    construct(c, a.x(0), a.i(1), a.i(2), a.i(3), 0, 0);
}
/// SDK 3: no mode or priority; its formats are numbered one lower (3 NV12, 2 RGBA) and CPU mode.
unsafe fn ctor3(c: *mut Cpu) {
    let a = Args(&mut *c);
    let f = a.i(2);
    construct(c, a.x(0), a.i(1), if f == 3 { FMT_NV12 } else if f == 2 { FMT_ABGR } else { f }, 1, 0, 0x20);
}

fn free(self_: u64) {
    let Some(d) = dec_of(self_) else { return };
    defer_drop(&d);
    {
        let mut st = d.st.lock().unwrap();
        st.quit = true;
        d.cv.notify_all();
    }
    w64(self_ + 8, 0);
    DECS.lock().unwrap().as_mut().map(|m| m.remove(&d.id));
    let st = d.st.lock().unwrap();
    crate::log!(
        "[movie] decoder {self_:#x} destroyed: {} packets in, {} pictures out ({} not black); {} decoded, {:.2} ms a packet, longest {:.2} ms",
        st.packets,
        st.frames,
        st.lit,
        st.dec_n,
        if st.dec_n > 0 { st.dec_ns as f64 / 1e6 / st.dec_n as f64 } else { 0.0 },
        st.dec_max as f64 / 1e6
    );
}
unsafe fn dtor(c: *mut Cpu) {
    free((*c).x[0]);
}
unsafe fn dtor0(c: *mut Cpu) {
    let s = (*c).x[0];
    free(s);
    match crate::hle::sym("_ZdlPv") {
        Some(del) => {
            let _ = crate::guest::call(c, del, &[s]);
        }
        None => crate::log!("[RED] movie: no operator delete in the game; the decoder object is not freed"),
    }
}

macro_rules! mfn {
    ($name:ident, |$c:ident, $a:ident, $d:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name($c: *mut Cpu) {
            let $a: [u64; 8] = std::array::from_fn(|i| (*$c).x[i]);
            let Some($d) = dec_of($a[0]) else {
                (*$c).x[0] = NO_IMPL;
                return;
            };
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*$c).x[0] = r;
        }
    };
}

mfn!(configure, |c, a, d| {
    let md = a[1];
    if d.audio {
        let mut csd = [vec![], vec![]];
        for (k, cs) in csd.iter_mut().enumerate() {
            if md != 0 {
                if let Some(b) = md_buf(c, md, &format!("csd-{k}")) {
                    *cs = b;
                }
            }
        }
        let ch = if md != 0 { md_i32(c, md, "channel-count").unwrap_or(0) } else { 0 };
        let rate = if md != 0 { md_i32(c, md, "sample-rate").unwrap_or(0) } else { 0 };
        let mut st = d.st.lock().unwrap();
        for (k, cs) in csd.into_iter().enumerate() {
            if !cs.is_empty() {
                st.csd[k] = cs;
            }
        }
        if ch > 0 {
            st.ch = ch;
        }
        if rate > 0 {
            st.rate = rate;
        }
        let (n0, n1) = (st.csd[0].len(), st.csd[1].len());
        drop(st);
        d.trace(0, || format!("Configure ({}): {ch} channels {rate} Hz, headers {n0} + {n1} bytes", if d.ty == T_AAC { "AAC" } else { "Vorbis" }));
        return 0;
    }
    if d.ty == T_AVC && md != 0 {
        let c0 = md_buf(c, md, "csd-0");
        let c1 = md_buf(c, md, "csd-1");
        let av = md_buf(c, md, "avcc");
        let mut st = d.st.lock().unwrap();
        if let Some(b) = c0 {
            st.csd[0] = b;
        }
        if let Some(b) = c1 {
            st.csd[1] = b;
        }
        if let Some(b) = av {
            st.avcc = b;
        }
    }
    let (w, h) = if md != 0 { (md_i32(c, md, "width").unwrap_or(0), md_i32(c, md, "height").unwrap_or(0)) } else { (0, 0) };
    let mut st = d.st.lock().unwrap();
    if w > 0 && h > 0 && !st.fmt_sent {
        st.w = w;
        st.h = h;
    }
    drop(st);
    d.trace(0, || format!("Configure: MediaData {w}x{h}"));
    0
});

mfn!(register_events, |c, a, d| {
    let ev = a[1];
    let mut st = d.st.lock().unwrap();
    for i in 0..4 {
        let e = if ev != 0 { r64(ev + 8 * i as u64) } else { 0 };
        st.ev[i] = if ev_ok(e) { e } else { 0 };
    }
    st.evc = ev;
    let evs = st.ev;
    drop(st);
    d.trace(1, || format!("RegisterClientEvents {:#x} {:#x} {:#x} {:#x}", evs[0], evs[1], evs[2], evs[3]));
    0
});

mfn!(start, |c, a, d| {
    d.st.lock().unwrap().started = true;
    d.trace(2, || "Start".into());
    // "input available" a little later, from the signaller (the client may link its waiters after Start)
    defer(&d, 0, Instant::now() + Duration::from_millis(2));
    0
});

mfn!(stop, |c, a, d| {
    d.trace(3, || "Stop".into());
    {
        let _g = without_core(|| SIG_MU.lock().unwrap());
        d.st.lock().unwrap().started = false;
    }
    defer_drop(&d);
    without_core(|| reset(&d, true));
    0
});

mfn!(reset_fn, |c, a, d| {
    d.trace(4, || "Reset".into());
    without_core(|| reset(&d, true));
    d.st.lock().unwrap().fmt_sent = false;
    0
});

mfn!(flush, |c, a, d| {
    d.trace(5, || "Flush".into());
    defer_drop(&d);
    without_core(|| reset(&d, false));
    0
});

mfn!(acquire_input, |c, a, d| {
    let (idx, left) = (a[1], a[2]);
    if idx == 0 || left == 0 {
        return BAD_ARG;
    }
    let mut st = d.st.lock().unwrap();
    w32(idx, u32::MAX);
    w32(left, 0);
    let now = Instant::now();
    let k = if st.in_eos { None } else { st.infree.iter().position(|&x| st.instale[x].is_none_or(|t| now - t >= STALE)) };
    let mut wake = None;
    let r = match k {
        Some(k) if st.started => {
            let i = st.infree.remove(k);
            st.inown[i] = true;
            st.instale[i] = None;
            w32(idx, i as u32);
            w32(left, st.infree.len() as u32);
            0
        }
        _ => {
            if !st.infree.is_empty() && st.started && !st.in_eos {
                wake = st.infree.iter().filter_map(|&x| st.instale[x]).map(|t| t + STALE).min();
            }
            NO_INPUT
        }
    };
    drop(st);
    if let Some(t) = wake {
        defer(&d, 0, t);
    }
    r
});

mfn!(get_input, |c, a, d| {
    let (i, buf) = (a[1] as i32, a[2]);
    if buf == 0 || !(0..NIN as i32).contains(&i) {
        return BAD_ARG;
    }
    let p = d.input_addr(i as usize);
    let _ = crate::guest::call(c, g().set_data, &[buf, p, INCAP as u64]);
    let _ = crate::guest::call(c, g().set_range, &[buf, 0, INCAP as u64]);
    0
});

mfn!(send_input, |c, a, d| {
    let (i, off, size, pts, flags) = (a[1] as i32, a[2] as i32, a[3] as i32, a[4] as i64, a[5] as u32);
    if !(0..NIN as i32).contains(&i) || off < 0 || size < 0 || off as usize + size as usize > INCAP {
        return BAD_ARG;
    }
    let i = i as usize;
    let mut st = d.st.lock().unwrap();
    if st.pend.iter().any(|p| p.inp == i) {
        return BAD_ARG;
    }
    if st.in_eos {
        if !st.inown[i] && st.instale[i].is_none() {
            return BAD_ARG;
        }
        if st.inown[i] {
            st.infree.push(i);
        }
        st.inown[i] = false;
        st.instale[i] = None;
        return 0;
    }
    if st.pend.len() == NIN {
        return BAD_ARG;
    }
    if let Some(k) = st.infree.iter().position(|&x| x == i) {
        st.infree.remove(k);
    }
    st.inown[i] = false;
    st.instale[i] = None;
    let mut fmt_now = false;
    if (d.ty == T_VP9 || d.ty == T_VP8) && st.w == 0 && size > 0 {
        let pkt = &d.inbuf[i][off as usize..off as usize + size as usize];
        let wh = if d.ty == T_VP8 { vpx::vp8_key_frame_size(pkt) } else { vpx::key_frame_size(pkt) };
        if let Some((w, h)) = wh {
            st.w = w as i32;
            st.h = h as i32;
            crate::log!("[movie] decoder {:#x}: {} key frame {w}x{h}", d.obj, if d.ty == T_VP8 { "VP8" } else { "VP9" });
        }
    }
    if !st.fmt_sent {
        st.fmt_sent = true;
        fmt_now = true;
        if st.w == 0 && !d.audio {
            st.w = 1920;
            st.h = 1080;
        }
    }
    st.pend.push_back(Pkt { inp: i, off: off as usize, size: size as usize, pts, flags });
    st.packets += 1;
    if flags & EOS != 0 {
        st.in_eos = true;
    }
    let q = st.pend.len();
    drop(st);
    d.trace(7, || format!("SendInputBufferForDecode index {i} offset {off} length {size} time {pts} us flags {flags:#x} ({q} queued)"));
    if fmt_now {
        md_event(c, &d, 2);
    }
    kick(c, &d);
    0
});

mfn!(acquire_output, |c, a, d| {
    let (idx, pts, flags, left) = (a[1], a[2], a[3], a[4]);
    if idx == 0 || pts == 0 || flags == 0 || left == 0 {
        return BAD_ARG;
    }
    let mut st = d.st.lock().unwrap();
    w32(idx, u32::MAX);
    w32(left, 0);
    if !st.started {
        return NO_OUTPUT;
    }
    let Some(s) = st.outq.pop_front() else { return NO_OUTPUT };
    w32(idx, s as u32);
    w64(pts, st.out[s].pts as u64);
    w32(flags, st.out[s].flags);
    w32(left, st.outq.len() as u32);
    st.out[s].cli = true;
    let (t, f) = (st.out[s].pts, st.out[s].flags);
    drop(st);
    d.trace(8, || format!("AcquireOutputBufferIndex -> {s} time {t} flags {f:#x}"));
    0
});

unsafe fn format(c: *mut Cpu, d: &Dec, md: u64) {
    let (w, h, ch, rate, b709, full) = {
        let st = d.st.lock().unwrap();
        (if st.w > 0 { st.w } else { 1920 }, if st.h > 0 { st.h } else { 1080 }, st.ch, st.rate, st.bt709, st.full)
    };
    if d.audio {
        md_set(c, md, "channel-count", if ch > 0 { ch } else { 2 });
        md_set(c, md, "sample-rate", if rate > 0 { rate } else { 48000 });
        return;
    }
    md_set(c, md, "width", w);
    md_set(c, md, "height", h);
    let _ = crate::guest::call(c, g().set_rect, &[md, key("crop"), 0, 0, (w - 1) as u64, (h - 1) as u64]);
    for (k, v) in [("stride", w), ("slice-height", h), ("nv12-y-stride", w), ("nv12-y-offset", 0), ("nv12-uv-offset", w * h), ("nv12-uv-stride", w), ("nv12-width", w), ("nv12-height", h)] {
        md_set(c, md, k, v);
    }
    let cs = (if b709.unwrap_or(h >= 720) { 3 } else { 1 }) + full as i32;
    md_set(c, md, "nv12-colorspace", cs);
}

mfn!(output_format, |c, a, d| {
    if a[1] == 0 {
        return BAD_ARG;
    }
    format(c, &d, a[1]);
    d.trace(9, || "GetOutputFormat".into());
    0
});
mfn!(buffer_format, |c, a, d| {
    if a[2] == 0 {
        return BAD_ARG;
    }
    format(c, &d, a[2]);
    d.trace(10, || format!("GetOutputBufferFormat index {}", a[1] as i32));
    0
});
mfn!(output_size, |c, a, d| {
    let (size, mode) = (a[1], a[2]);
    if size == 0 || mode == 0 {
        return BAD_ARG;
    }
    let st = d.st.lock().unwrap();
    let m = if d.out_align != 0 { d.out_align } else { d.mode };
    if d.audio {
        let sbr = !st.csd[0].is_empty() && matches!(st.csd[0][0] >> 3, 5 | 29);
        let n = if d.ty == T_AAC && st.ch > 0 { 1024 * if sbr { 2 } else { 1 } * st.ch * 2 } else { PCM_CAP as i32 };
        w32(size, n as u32);
        w32(mode, m as u32);
        return 0;
    }
    let (w, h) = (if st.w > 0 { st.w } else { 1920 }, if st.h > 0 { st.h } else { 1080 });
    w32(size, (w * h * 3 / 2) as u32);
    w32(mode, m as u32);
    0
});

/// The native-texture mode: the picture into the client's NVN texture through the graphics layer, the buffer's
/// sync signalled. A rendered slot is returned at the client's release, or at its next GetOutputBuffer.
unsafe fn out_tex(c: *mut Cpu, d: &Dec, i: usize, buf: u64) -> u64 {
    let tex = crate::guest::call(c, g().data, &[buf]).unwrap_or(0);
    if tex == 0 {
        crate::log!("[RED] movie: the client's buffer in the native-texture mode has no texture object");
        return BAD_ARG;
    }
    let Some((tw, th, f)) = gfx::texture_info(tex) else {
        crate::log!("[RED] movie: buffer data {tex:#x} in the native-texture mode is not a texture object");
        return BAD_ARG;
    };
    let bgr = (0x99..=0x9c).contains(&f);
    let (pic, frm, w, h) = {
        let st = d.st.lock().unwrap();
        (st.out[i].pic, st.out[i].frm.clone(), st.w, st.h)
    };
    if pic && !frm.is_empty() {
        let mut src = frm;
        if bgr {
            for px in src.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        let (cw, ch) = (w.min(tw), h.min(th));
        gfx::write_texture(tex, &src, cw, ch, (w * 4) as usize);
    }
    if let Some(gs) = g().nvn_sync {
        let sy = crate::guest::call(c, gs, &[buf]).unwrap_or(0);
        if sy != 0 {
            gfx::signal_sync(sy);
        }
    }
    let mut st = d.st.lock().unwrap();
    let mut n = 0;
    if !st.tex_rel {
        for k in 0..d.nout {
            if k != i && st.out[k].rnd && st.out[k].cli {
                st.out[k] = Slot { frm: std::mem::take(&mut st.out[k].frm), ..Default::default() };
                n += 1;
            }
        }
    }
    if st.out[i].cli {
        st.out[i].rnd = true;
    }
    let more = !st.pend.is_empty() || st.avc_drain;
    drop(st);
    d.trace(12, || format!("GetOutputBuffer index {i} -> texture {tex:#x} ({tw}x{th} format {f:#x}){}", if pic { "" } else { " (no picture: the texture keeps its last frame)" }));
    if n > 0 && more {
        kick(c, d);
    }
    0
}

mfn!(get_output, |c, a, d| {
    let (i, buf) = (a[1] as i32, a[2]);
    if buf == 0 || i < 0 || i as usize >= d.nout {
        return BAD_ARG;
    }
    let i = i as usize;
    if d.mode == MODE_TEX {
        return out_tex(c, &d, i, buf);
    }
    let _ = crate::guest::call(c, g().set_range, &[buf, 0, 0]);
    let p = crate::guest::call(c, g().data, &[buf]).unwrap_or(0);
    let cap = crate::guest::call(c, g().cap, &[buf]).unwrap_or(0) as usize;
    let st = d.st.lock().unwrap();
    if d.audio {
        let n = if st.out[i].pic { st.out[i].bytes } else { 0 };
        if p == 0 || cap < n {
            drop(st);
            crate::log!("[RED] movie: the client's audio output buffer is {cap} bytes, this packet needs {n}");
            return BAD_ARG;
        }
        if n > 0 {
            crate::guest::write_bytes(p, &st.out[i].frm[..n]);
        }
        drop(st);
        let _ = crate::guest::call(c, g().set_range, &[buf, 0, n as u64]);
        return 0;
    }
    let (w, h) = (if st.w > 0 { st.w } else { 1920 } as usize, if st.h > 0 { st.h } else { 1080 } as usize);
    let need = w * h * 3 / 2;
    if p == 0 || cap < need {
        drop(st);
        crate::log!("[RED] movie: the client's output buffer is {cap} bytes, a frame needs {need}");
        return BAD_ARG;
    }
    let pic = st.out[i].pic && st.out[i].frm.len() >= need;
    // NXOS_MOVIE_DUMP=<dir>: the first pictures handed out, as PNG (a check of what the game was given)
    if pic {
        if let Ok(dir) = std::env::var("NXOS_MOVIE_DUMP") {
            static DUMPED: AtomicU64 = AtomicU64::new(0);
            let n = DUMPED.fetch_add(1, Ordering::Relaxed);
            if n < 8 || n % 100 == 0 {
                let f = &st.out[i].frm;
                let p = Picture { w, h, y: f[..w * h].to_vec(), uv: f[w * h..need].to_vec(), bt709: st.bt709, full_range: st.full, pts: 0 };
                let mut rgba = vec![0u8; w * h * 4];
                p.rgba_into(&mut rgba, w, h);
                let _ = crate::media::yuv::write_png(&std::path::Path::new(&dir).join(format!("movie-{:x}-{n:04}.png", d.obj)), &rgba, w, h);
            }
        }
    }
    nv12_out(&st, i, w, h, crate::guest::bytes_mut(p, need as u64));
    drop(st);
    let _ = crate::guest::call(c, g().set_range, &[buf, 0, need as u64]);
    d.trace(12, || format!("GetOutputBuffer index {i} -> {need} bytes{}", if pic { "" } else { " (no picture in this output: end of stream)" }));
    0
});

/// Output slot `i`'s NV12 picture (w x h, stride w) into the client's buffer; black when the slot has none (end of
/// stream). True when it had a picture.
fn nv12_out(st: &St, i: usize, w: usize, h: usize, out: &mut [u8]) -> bool {
    let need = w * h * 3 / 2;
    let pic = st.out[i].pic && st.out[i].frm.len() >= need;
    if pic {
        out[..need].copy_from_slice(&st.out[i].frm[..need]);
    } else {
        out[..w * h].fill(16);
        out[w * h..need].fill(128);
    }
    pic
}

unsafe fn get_output_fmt(c: *mut Cpu) {
    let md = (*c).x[3];
    if let Some(d) = dec_of((*c).x[0]) {
        if md != 0 {
            let saved: [u64; 8] = std::array::from_fn(|i| (*c).x[i]);
            format(c, &d, md);
            (&mut (*c).x)[..8].copy_from_slice(&saved);
        }
    }
    get_output(c)
}

mfn!(release_output, |c, a, d| {
    let i = a[1] as i32;
    if i < 0 || i as usize >= d.nout {
        return BAD_ARG;
    }
    let i = i as usize;
    let mut st = d.st.lock().unwrap();
    let held = st.out[i].cli;
    if held && st.out[i].rnd {
        st.tex_rel = true;
    }
    if held {
        let s = &mut st.out[i];
        s.used = false;
        s.pic = false;
        s.cli = false;
        s.rnd = false;
    }
    let more = !st.pend.is_empty() || st.avc_drain;
    drop(st);
    d.trace(13, || format!("release output index {i}{}", if held { "" } else { ": not held by the client" }));
    if held && (more || d.ty == T_AVC) {
        kick(c, &d);
    }
    0
});

unsafe fn last_error(c: *mut Cpu) {
    (*c).x[0] = 0;
}

/// The movie decoders' lines for the log.
pub fn report() -> Vec<String> {
    DECS.lock()
        .unwrap()
        .as_ref()
        .map(|m| {
            m.values()
                .map(|d| {
                    let st = d.st.lock().unwrap();
                    format!("[movie] decoder {:#x} (type {}): {} packets in, {} pictures out, {} of them not black", d.obj, d.ty, st.packets, st.frames, st.lit)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The graphics layer's NVN texture and sync calls, made from the host for the native-texture mode.
mod gfx {
    use nxrt::cpu::Cpu;

    fn proc_(name: &str) -> Option<nxrt::hostfn::HostFn> {
        nxgfx::nvn::procs().iter().find(|e| e.name == name).map(|e| e.f)
    }
    fn call(name: &str, args: &[u64]) -> Option<u64> {
        let f = proc_(name)?;
        let mut c = Cpu::new_boxed();
        for (i, &a) in args.iter().enumerate() {
            c.x[i] = a;
        }
        unsafe { f(&mut *c) };
        Some(c.x[0])
    }
    /// (width, height, NVN format) of a texture object.
    pub fn texture_info(tex: u64) -> Option<(i32, i32, i32)> {
        if !crate::guest::readable(tex, 0x40) {
            return None;
        }
        let w = call("nvnTextureGetWidth", &[tex])? as i32;
        let h = call("nvnTextureGetHeight", &[tex])? as i32;
        let f = call("nvnTextureGetFormat", &[tex])? as i32;
        (w > 0 && h > 0).then_some((w, h, f))
    }
    /// RGBA rows into the texture's top-left w x h (WriteTexelsStrided).
    pub fn write_texture(tex: u64, rgba: &[u8], w: i32, h: i32, row: usize) {
        let region: [i32; 6] = [0, 0, 0, w, h, 1];
        let _ = call("nvnTextureWriteTexelsStrided", &[tex, 0, region.as_ptr() as u64, rgba.as_ptr() as u64, row as u64, (row * h as usize) as u64]);
    }
    /// The buffer's sync as the SDK's consumer leaves it once its copy is done: signalled. A sync not fenced since
    /// its initialization is signalled in the graphics layer, so it is initialized again.
    pub fn signal_sync(sync: u64) {
        let _ = call("nvnSyncInitialize", &[sync, 0]);
    }
}

pub fn register(r: &mut Registry) {
    const W: &str = "movie::Decoder: decoded on the host (VP9 libvpx, H.264 / AAC Media Foundation, Vorbis lewton)";
    let p = "_ZN5movie7Decoder";
    for (n, f) in [
        (format!("{p}C1ENS_11DecoderTypeENS_19DecoderOutputFormatENS_11DecoderModeEi"), ctor as unsafe fn(*mut Cpu)),
        (format!("{p}C2ENS_11DecoderTypeENS_19DecoderOutputFormatENS_11DecoderModeEi"), ctor),
        (format!("{p}C1ENS_11DecoderTypeENS_19DecoderOutputFormatENS_11DecoderModeE"), ctor4),
        (format!("{p}C2ENS_11DecoderTypeENS_19DecoderOutputFormatENS_11DecoderModeE"), ctor4),
        (format!("{p}C1ENS_11DecoderTypeENS_19DecoderOutputFormatE"), ctor3),
        (format!("{p}C2ENS_11DecoderTypeENS_19DecoderOutputFormatE"), ctor3),
        (format!("{p}D2Ev"), dtor),
        (format!("{p}D1Ev"), dtor),
        (format!("{p}D0Ev"), dtor0),
        (format!("{p}9ConfigureEPNS_9MediaDataE"), configure),
        (format!("{p}20RegisterClientEventsEPNS_13DecoderEventsE"), register_events),
        (format!("{p}5StartEv"), start),
        (format!("{p}4StopEv"), stop),
        (format!("{p}5ResetEv"), reset_fn),
        (format!("{p}5FlushEv"), flush),
        (format!("{p}23AcquireInputBufferIndexEPiS1_"), acquire_input),
        (format!("{p}14GetInputBufferEiPNS_6BufferE"), get_input),
        (format!("{p}24SendInputBufferForDecodeEiiilj"), send_input),
        (format!("{p}24AcquireOutputBufferIndexEPiPlPjS1_"), acquire_output),
        (format!("{p}15GetOutputFormatEPNS_9MediaDataE"), output_format),
        (format!("{p}21GetOutputBufferFormatEiPNS_9MediaDataE"), buffer_format),
        (format!("{p}19GetOutputBufferSizeEPiS1_"), output_size),
        (format!("{p}15GetOutputBufferEiPNS_6BufferE"), get_output),
        (format!("{p}15GetOutputBufferEiPNS_6BufferEPNS_9MediaDataE"), get_output_fmt),
        (format!("{p}24ReleaseOutputBufferIndexEi"), release_output),
        (format!("{p}12GetLastErrorEv"), last_error),
    ] {
        r.force(&n, W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::codec_video::tests::{clip, fnv, h264_access_units, ivf_frames, reference, H, W};

    #[test]
    fn avcc_to_annexb() {
        let avcc = [1, 0x64, 0, 0x28, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xee];
        assert_eq!(super::avcc_annexb(&avcc), vec![0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68, 0xee]);
        assert_eq!(super::aac_shape(&[0x11, 0x90], 0, 0), (48000, 2));
    }

    unsafe fn call(f: unsafe fn(*mut Cpu), args: &[u64]) -> u64 {
        let mut cpu: Box<Cpu> = Box::new(std::mem::zeroed());
        cpu.x[..args.len()].copy_from_slice(args);
        f(&mut *cpu);
        cpu.x[0]
    }
    fn p<T>(v: &mut T) -> u64 {
        v as *mut T as u64
    }

    /// One clip through movie::Decoder as its client drives it (CPU mode, NV12): AcquireInputBufferIndex, the
    /// packet into that input buffer, SendInputBufferForDecode (end of stream on the last), AcquireOutputBufferIndex,
    /// the output buffer's NV12, ReleaseOutputBufferIndex. The SDK's MediaData / Buffer helpers are the game's code,
    /// so the test sets the size Configure would read and copies the input / output the way they hand them over.
    /// The client keeps its first 16 outputs (every slot) before it releases any: then packets stay queued, no
    /// input buffer is free and no output comes (MediaCodec's back-pressure) until it releases them.
    /// Returns the pictures' FNV in output order, whether end of stream was flagged, and whether back-pressure held.
    unsafe fn run(ty: i32, packets: &[Vec<u8>]) -> (Vec<u64>, bool, bool) {
        let mut obj = [0u64; 4];
        let o = obj.as_mut_ptr() as u64;
        let d = make(o, ty, FMT_NV12, 1, 0, 0);
        if ty == T_AVC {
            let mut st = d.st.lock().unwrap();
            st.w = W as i32;
            st.h = H as i32;
        }
        assert_eq!(call(start, &[o]), 0);
        let (mut next, mut sums, mut eos_seen) = (0usize, vec![], false);
        let (mut held, mut pressure) = (vec![], None);
        let (w, h) = (W as usize, H as usize);
        let mut frame = vec![0u8; w * h * 3 / 2];
        let t0 = Instant::now();
        while !eos_seen {
            assert!(t0.elapsed().as_secs() < 30, "type {ty}: {next} packets in, {} pictures out", sums.len());
            if next < packets.len() {
                let (mut idx, mut left) = (0u32, 0u32);
                match call(acquire_input, &[o, p(&mut idx), p(&mut left)]) {
                    0 => {
                        let pk = &packets[next];
                        crate::guest::write_bytes(d.input_addr(idx as usize), pk);
                        let flags = if next + 1 == packets.len() { EOS as u64 } else { 0 };
                        let pts_us = (next as u64 * 1_000_000) / 24;
                        assert_eq!(call(send_input, &[o, idx as u64, 0, pk.len() as u64, pts_us, flags]), 0);
                        next += 1;
                    }
                    NO_INPUT => {}
                    r => panic!("AcquireInputBufferIndex {r:#x}"),
                }
            }
            let (mut idx, mut pts, mut flags, mut left) = (0u32, 0u64, 0u32, 0u32);
            match call(acquire_output, &[o, p(&mut idx), p(&mut pts), p(&mut flags), p(&mut left)]) {
                0 => {
                    let pic = nv12_out(&d.st.lock().unwrap(), idx as usize, w, h, &mut frame);
                    if pic {
                        sums.push(fnv(&frame));
                    }
                    eos_seen = flags & EOS != 0;
                    held.push(idx);
                    if pressure.is_none() && held.len() < NOUT_VIDEO && !eos_seen {
                        continue;
                    }
                    if pressure.is_none() && !eos_seen {
                        // every slot held: no output, and the inputs run out, for as long as the client keeps them
                        let mut no_input = 0;
                        let t1 = Instant::now();
                        let mut outs = 0;
                        while t1.elapsed().as_millis() < 300 {
                            let (mut i2, mut l2) = (0u32, 0u32);
                            if call(acquire_input, &[o, p(&mut i2), p(&mut l2)]) == 0 && next < packets.len() {
                                let pk = &packets[next];
                                crate::guest::write_bytes(d.input_addr(i2 as usize), pk);
                                assert_eq!(call(send_input, &[o, i2 as u64, 0, pk.len() as u64, (next as u64 * 1_000_000) / 24, 0]), 0);
                                next += 1;
                            } else {
                                no_input += 1;
                            }
                            let (mut a, mut b, mut c2, mut e) = (0u32, 0u64, 0u32, 0u32);
                            if call(acquire_output, &[o, p(&mut a), p(&mut b), p(&mut c2), p(&mut e)]) == 0 {
                                outs += 1;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        pressure = Some(outs == 0 && no_input > 0 && d.st.lock().unwrap().pend.len() == NIN);
                    }
                    for i in held.drain(..) {
                        assert_eq!(call(release_output, &[o, i as u64]), 0);
                    }
                }
                NO_OUTPUT => std::thread::sleep(std::time::Duration::from_micros(200)),
                r => panic!("AcquireOutputBufferIndex {r:#x}"),
            }
        }
        assert_eq!(next, packets.len());
        free(o);
        (sums, eos_seen, pressure == Some(true))
    }

    fn check(ty: i32, name: &str, packets: Vec<Vec<u8>>) {
        let want = reference(name);
        let (got, eos, pressure) = unsafe { run(ty, &packets) };
        assert!(eos, "type {ty}: end of stream flagged");
        assert_eq!(got.len(), want.len(), "type {ty}: pictures out");
        let bad: Vec<usize> = (0..want.len()).filter(|&i| got[i] != want[i]).collect();
        assert!(bad.is_empty(), "type {ty}: pictures {bad:?} differ from ffmpeg's decode");
        assert!(pressure, "type {ty}: with every output slot held, packets stay queued and no output comes");
    }

    /// Real clips (ffmpeg, `testdata/media`) through the movie decoder's service path, every picture checked
    /// against ffmpeg's decode.
    #[test]
    fn movie_vp9_clip_decodes_as_ffmpeg() {
        check(T_VP9, "clip_vp9.ivf", ivf_frames(&clip("clip_vp9.ivf")));
    }

    #[test]
    fn movie_h264_clip_decodes_as_ffmpeg() {
        check(T_AVC, "clip_h264.h264", h264_access_units(&clip("clip_h264.h264")));
    }
}
