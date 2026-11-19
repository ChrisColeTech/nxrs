//! nn::codec's video decoders (the Switch 2's SDK: VP9, H.264, AV1, VP8 decoder types), taken over and decoded on
//! the host (nxnative `hle_codec_video.c`, ported; Turbo serves the same through the graphics supervisor's decoder,
//! f9c8469 / 6c4578c / 0ae160a). The SDK's protocol:
//! - the client makes one image pool per picture (MakeDecodeImagePreference -> CalculateDecoderImagePoolBuffer ->
//!   InitializeDecoderImagePool) and registers each under an index; registering makes it acquirable (a queue 8
//!   deep) with a sync handle already signalled;
//! - Acquire takes the oldest index and its sync handle (3604 when none); Release gives the buffer to the decoder
//!   and makes it acquirable again at once, with a new sync handle that signals when a picture is in it (the client
//!   polls nn::gs::TryWaitNativeSync, or waits on an nn::syncpt event), then reads the picture's properties (3604
//!   until filled) and the picture (ReadDecoderOutputImageRgba / Nv12), or draws it through a native image;
//! - Submit takes one packet with its time and an end-of-stream flag and returns, the decoder decoding on a thread
//!   of its own; 3603 (busy) when packets and pictures waiting fill CV_HOLD, the submission-ready event going down
//!   at CV_READY (back-pressure: the client retries after the event).
//! VP8 and VP9 with libvpx, H.264 with Media Foundation, AV1 with dav1d (frame threads).

use crate::guest::{r16, r32, r64, w16, w32, w64, w8, Args};
use crate::hle::Registry;
use crate::media::{av1, mf, vpx, yuv::Picture};
use nxrt::cpu::Cpu;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

const MAGIC: u32 = 0x5643_6d44; // 'DmCV'
const SYNC_MAGIC: u32 = 0x5343_6d44; // 'DmCS'
const BUFS: usize = 32;
const READY: usize = 4;
const HOLD: usize = 12;
const EVENT_OFF: u64 = 0x100;
const fn cv_r(desc: u32) -> u64 {
    (0x6f | (desc << 9)) as u64
}
const BUSY: u64 = cv_r(3603);
const NONE: u64 = cv_r(3604);
const BAD: u64 = cv_r(2004);
const NATIVE_ALIGN: u64 = 0x4000;
const TAG: u64 = 0x5643_0000_0000_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Vp8,
    Vp9,
    H264,
    Av1,
}

fn native_stride(w: u32) -> u32 {
    (w * 4 + 63) & !63
}
fn native_size(w: u32, h: u32) -> u64 {
    (native_stride(w) as u64 * h as u64 + NATIVE_ALIGN - 1) & !(NATIVE_ALIGN - 1)
}

// ---- image pools

struct Pool {
    w: u32,
    h: u32,
    fmt: u32,
    count: u32,
    img: Vec<Option<Picture>>,
    mem: u64,
    size: u64,
    native: bool,
    slot: u32,
}

static POOLS: Mutex<Option<HashMap<u64, Pool>>> = Mutex::new(None);
static POOL_SLOTS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);

fn pool_key(t: u64) -> Option<u64> {
    if t == 0 {
        return None;
    }
    let k = r64(t);
    (k & 0xffff_ff00_0000_0000 == TAG | 0x0100_0000_0000).then_some(k)
}

fn pool_native_write(p: &Pool, i: u32, pic: &Picture) {
    if !p.native || p.mem == 0 || (i as u64 + 1) * native_size(p.w, p.h) > p.size {
        return;
    }
    let (w, h) = (p.w as usize, p.h as usize);
    let stride = native_stride(p.w) as usize;
    let mut tmp = vec![0u8; w * h * 4];
    pic.rgba_into(&mut tmp, w, h);
    let dst = p.mem + i as u64 * native_size(p.w, p.h);
    for y in 0..h {
        crate::guest::write_bytes(dst + (y * stride) as u64, &tmp[y * w * 4..(y + 1) * w * 4]);
    }
}

/// A native image handle's picture for the graphics layer's nvnNativeImage: (memory, size, width, height, row
/// stride) of pool picture `handle & 0xff` of pool slot `handle >> 8`.
pub fn native_image(handle: u32) -> Option<(u64, u64, u32, u32, u32)> {
    let (slot, i) = (handle >> 8, handle & 0xff);
    let key = *POOL_SLOTS.lock().unwrap().get((slot as usize).checked_sub(1)?)?;
    let g = POOLS.lock().unwrap();
    let p = g.as_ref()?.get(&key)?;
    let ns = native_size(p.w, p.h);
    (p.mem != 0 && i < p.count && (i as u64 + 1) * ns <= p.size).then(|| (p.mem + i as u64 * ns, ns, p.w, p.h, native_stride(p.w)))
}

// ---- decoders

#[derive(Default, Clone)]
struct Buf {
    pool: u64,
    pidx: u32,
    reg: bool,
    filled: bool,
    gen: u32,
    filled_gen: u32,
    w: u16,
    h: u16,
    ts: i64,
    eos: bool,
}

struct Packet {
    data: Vec<u8>,
    ts: i64,
    eos: bool,
}

#[derive(Default)]
struct St {
    buf: Vec<Buf>,
    fifo: VecDeque<u32>,
    pend: VecDeque<u32>,
    held: VecDeque<(Picture, i64, bool)>,
    eos_pending: bool,
    q: VecDeque<Packet>,
    busy: bool,
    quit: bool,
    reopen: bool,
    epoch: u32,
    submits: u64,
    pictures: u64,
    lit: u64,
    // statistics
    dec_n: u64,
    dec_ns: u64,
    dec_max: u64,
    late: u64,
}

struct Dec {
    id: u32,
    kind: Kind,
    w: u32,
    h: u32,
    csd: Vec<u8>,
    event: u64,
    st: Mutex<St>,
    cv: Condvar,
}

static DECS: Mutex<Vec<Option<Arc<Dec>>>> = Mutex::new(Vec::new());

fn dec_of(t: u64) -> Option<Arc<Dec>> {
    if t == 0 {
        return None;
    }
    let k = r64(t);
    if k & 0xffff_ffff_0000_0000 != TAG {
        return None;
    }
    let id = (k & 0xffff_ffff) as usize;
    DECS.lock().unwrap().get(id.checked_sub(1)?)?.clone()
}

fn pending(st: &St) -> usize {
    st.held.len() + st.q.len() + st.busy as usize
}

/// The submission-ready event: down at READY waiting, up when a packet was taken or room came back.
fn ready(d: &Dec, before: usize, took: bool, st: &St) {
    if d.event == 0 {
        return;
    }
    if pending(st) >= READY {
        crate::services::os::clear_system_event(d.event);
    } else if took || before >= READY {
        crate::services::os::signal_bound(d.event);
    }
}

/// Pictures and end of stream into the released buffers, in order.
fn fill(d: &Dec, st: &mut St, decoded: bool) {
    while !st.pend.is_empty() && (!st.held.is_empty() || st.eos_pending) {
        let i = st.pend.pop_front().unwrap() as usize;
        if let Some((pic, ts, eos)) = st.held.pop_front() {
            let b = &mut st.buf[i];
            b.w = pic.w as u16;
            b.h = pic.h as u16;
            b.ts = ts;
            b.eos = eos;
            let lit = pic.lit();
            if let Some(p) = POOLS.lock().unwrap().as_mut().and_then(|m| m.get_mut(&b.pool)) {
                if b.pidx < p.count {
                    pool_native_write(p, b.pidx, &pic);
                    p.img[b.pidx as usize] = Some(pic);
                }
            }
            st.pictures += 1;
            if lit {
                st.lit += 1;
            }
            if st.pictures == 1 {
                crate::log!("[codec] {:?} decoder {}: first picture {}x{} into buffer {i}", d.kind, d.id, st.buf[i].w, st.buf[i].h);
            }
            if decoded {
                st.late += 1;
            }
        } else {
            let b = &mut st.buf[i];
            b.w = 0;
            b.h = 0;
            b.ts = 0;
            b.eos = true;
            st.eos_pending = false;
        }
        if st.buf[i].eos {
            crate::log!("[codec] {:?} decoder {}: end of stream after {} pictures ({} not black)", d.kind, d.id, st.pictures, st.lit);
        }
        st.buf[i].filled = true;
        st.buf[i].filled_gen = st.buf[i].gen;
        spt_fire(d.id, i as u32, st.buf[i].filled_gen);
    }
    d.cv.notify_all();
}

enum Codec {
    None,
    Vpx(vpx::Vpx),
    H264(Box<mf::H264>),
    Av1(av1::Av1),
}

fn open(d: &Dec) -> Codec {
    let r = match d.kind {
        Kind::Vp9 => vpx::Vpx::vp9(d.w, d.h).map(Codec::Vpx),
        Kind::Vp8 => vpx::Vpx::vp8(d.w, d.h).map(Codec::Vpx),
        Kind::H264 => mf::H264::new().map(|mut h| {
            if !d.csd.is_empty() {
                let _ = h.decode(&d.csd, 0);
            }
            Codec::H264(Box::new(h))
        }),
        Kind::Av1 => {
            let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8) as i32;
            av1::Av1::new(n, n.min(4)).map(Codec::Av1).map_err(|e| format!("dav1d_open {e}"))
        }
    };
    match r {
        Ok(c) => c,
        Err(e) => {
            crate::log!("[RED] nn::codec {:?} decoder {}: {e}", d.kind, d.id);
            Codec::None
        }
    }
}

/// One packet through the codec (on the decoder's thread): its pictures, in presentation order.
fn decode(d: &Dec, codec: &mut Codec, p: &Packet, idle: bool) -> Vec<(Picture, i64)> {
    match codec {
        Codec::None => vec![],
        Codec::Vpx(v) => {
            if p.data.is_empty() {
                return vec![];
            }
            match v.decode(&p.data, p.ts) {
                Ok(Some(pic)) => vec![(pic, p.ts)],
                Ok(None) => vec![],
                Err(e) => {
                    crate::log_once!(format!("cv vpx {}", d.id), "[RED] nn::codec {} decoder {}: {e}", v.name, d.id);
                    vec![]
                }
            }
        }
        Codec::H264(h) => {
            if !p.data.is_empty() {
                if let Err(e) = h.decode(&p.data, p.ts / 1000) {
                    crate::log_once!(format!("cv h264 {}", d.id), "[RED] nn::codec H.264 decoder {}: {e}", d.id);
                }
            }
            if p.eos {
                h.drain();
            }
            h.out.drain(..).map(|pic| {
                let t = pic.pts * 1000;
                (pic, t)
            }).collect()
        }
        Codec::Av1(a) => {
            let mut v: Vec<(Picture, i64)> = a.feed(&p.data, p.ts).into_iter().map(|pic| {
                let t = pic.pts;
                (pic, t)
            }).collect();
            if p.eos || idle {
                v.extend(a.drain().into_iter().map(|pic| {
                    let t = pic.pts;
                    (pic, t)
                }));
            }
            v
        }
    }
}

fn worker(d: Arc<Dec>) {
    let mut codec = open(&d);
    crate::log!(
        "[codec] {:?} decoder {}: {}x{}{} ({})",
        d.kind,
        d.id,
        d.w,
        d.h,
        if d.csd.is_empty() { "" } else { ", with parameter sets" },
        match d.kind {
            Kind::Av1 => format!("dav1d {}", av1::version()),
            Kind::H264 => "Media Foundation".into(),
            _ => format!("libvpx {}", vpx::version()),
        }
    );
    let mut st = d.st.lock().unwrap();
    loop {
        while !st.quit && (st.q.is_empty() || st.held.len() >= HOLD) {
            st = d.cv.wait(st).unwrap();
        }
        if st.quit {
            break;
        }
        let p = st.q.pop_front().unwrap();
        let epoch = st.epoch;
        let reopen = std::mem::take(&mut st.reopen);
        st.busy = true;
        let idle = st.q.is_empty();
        drop(st);
        if reopen {
            codec = open(&d);
        }
        let t0 = Instant::now();
        let pics = decode(&d, &mut codec, &p, idle);
        let dt = t0.elapsed().as_nanos() as u64;
        st = d.st.lock().unwrap();
        let n = pics.len();
        for (k, (pic, ts)) in pics.into_iter().enumerate() {
            while st.held.len() >= HOLD && !st.quit && st.epoch == epoch {
                st = d.cv.wait(st).unwrap();
            }
            if st.quit || st.epoch != epoch {
                break;
            }
            st.held.push_back((pic, ts, p.eos && k + 1 == n));
            fill(&d, &mut st, true);
        }
        let before = pending(&st);
        st.busy = false;
        if st.quit {
            break;
        }
        if st.epoch == epoch {
            if n == 0 && p.eos {
                st.eos_pending = true;
            }
            fill(&d, &mut st, true);
        }
        ready(&d, before, false, &st);
        st.dec_n += 1;
        st.dec_ns += dt;
        st.dec_max = st.dec_max.max(dt);
    }
    st.busy = false;
    let (n, ns, max, late) = (st.dec_n, st.dec_ns, st.dec_max, st.late);
    drop(st);
    crate::log!(
        "[codec] {:?} decoder {}: {n} packets decoded in {:.2} ms each, {:.2} ms the longest; {late} pictures came after their buffer was released",
        d.kind,
        d.id,
        if n > 0 { ns as f64 / 1e6 / n as f64 } else { 0.0 },
        max as f64 / 1e6
    );
}

unsafe fn dec_init(c: *mut Cpu, t: u64, kind: Kind, w: u32, h: u32, inf: u32, outf: u32, csd: u64, csd_n: u64) -> u64 {
    let csd = if kind == Kind::H264 && csd != 0 && csd_n > 0 && csd_n < 1 << 20 { crate::guest::bytes(csd, csd_n).to_vec() } else { vec![] };
    let event = t + EVENT_OFF;
    crate::services::os::bind_system_event(c, event, false);
    crate::services::os::signal_bound(event);
    let id = {
        let mut g = DECS.lock().unwrap();
        let i = g.iter().position(|x| x.is_none()).unwrap_or_else(|| {
            g.push(None);
            g.len() - 1
        });
        let d = Arc::new(Dec { id: i as u32 + 1, kind, w, h, csd, event, st: Mutex::new(St { buf: vec![Buf::default(); BUFS], ..Default::default() }), cv: Condvar::new() });
        g[i] = Some(d.clone());
        let dw = d.clone();
        std::thread::Builder::new().name("nxos codec video".into()).stack_size(4 << 20).spawn(move || worker(dw)).expect("codec video thread");
        i as u32 + 1
    };
    w64(t, TAG | id as u64);
    crate::log!("[codec] {kind:?} decoder {id}: {w}x{h}, input format {inf}, output format {outf}");
    0
}

unsafe fn dec_fin(c: *mut Cpu, t: u64) -> u64 {
    let Some(d) = dec_of(t) else { return 0 };
    {
        let mut st = d.st.lock().unwrap();
        st.quit = true;
        d.cv.notify_all();
    }
    DECS.lock().unwrap()[d.id as usize - 1] = None;
    if let Some(destroy) = crate::hle::sym("_ZN2nn2os18DestroySystemEventEPNS0_15SystemEventTypeE") {
        let _ = crate::guest::call(c, destroy, &[d.event]);
    }
    w64(t, 0);
    0
}

fn dec_reg(t: u64, i: u32, pool: u64, pidx: u32) -> u64 {
    let Some(d) = dec_of(t) else { return BAD };
    if i as usize >= BUFS {
        return BAD;
    }
    let mut st = d.st.lock().unwrap();
    st.buf[i as usize] = Buf { pool: pool_key(pool).unwrap_or(0), pidx, reg: true, ..Default::default() };
    if st.fifo.len() < 8 {
        st.fifo.push_back(i);
    }
    0
}
fn dec_unreg(t: u64, i: u32) -> u64 {
    let Some(d) = dec_of(t) else { return BAD };
    if i as usize >= BUFS {
        return BAD;
    }
    let mut st = d.st.lock().unwrap();
    let b = &mut st.buf[i as usize];
    b.reg = false;
    b.filled = false;
    b.pool = 0;
    b.gen += 1;
    st.fifo.retain(|&x| x != i);
    st.pend.retain(|&x| x != i);
    0
}
fn dec_reset(t: u64) -> u64 {
    let Some(d) = dec_of(t) else { return 0 };
    let mut st = d.st.lock().unwrap();
    st.q.clear();
    st.held.clear();
    st.eos_pending = false;
    st.epoch += 1;
    st.reopen = true;
    d.cv.notify_all();
    if d.event != 0 {
        crate::services::os::signal_bound(d.event);
    }
    0
}
fn dec_start(t: u64) -> u64 {
    let Some(d) = dec_of(t) else { return BAD };
    let st = d.st.lock().unwrap();
    if d.event != 0 && pending(&st) < READY {
        crate::services::os::signal_bound(d.event);
    }
    0
}
fn dec_submit(out: u64, t: u64, data: u64, size: u64, ts: i64, eos: bool) -> u64 {
    let Some(d) = dec_of(t) else { return BAD };
    let mut st = d.st.lock().unwrap();
    if pending(&st) >= HOLD {
        crate::log_once!(format!("cv busy {}", d.id), "nn::codec {:?} decoder {}: busy (3603), a packet refused with {} pictures waiting for a released buffer", d.kind, d.id, st.held.len());
        if d.event != 0 {
            crate::services::os::clear_system_event(d.event);
        }
        return BUSY;
    }
    st.q.push_back(Packet { data: crate::guest::bytes(data, size).to_vec(), ts, eos });
    st.submits += 1;
    crate::guest::out(out, st.submits);
    ready(&d, 0, true, &st);
    d.cv.notify_all();
    0
}
fn dec_acq(idx: u64, sync: u64, t: u64) -> u64 {
    let Some(d) = dec_of(t) else { return BAD };
    let mut st = d.st.lock().unwrap();
    let Some(i) = st.fifo.pop_front() else { return NONE };
    w32(idx, i);
    w32(sync, SYNC_MAGIC);
    w32(sync + 4, d.id);
    w32(sync + 8, i);
    w32(sync + 12, st.buf[i as usize].gen);
    0
}
fn dec_rel(t: u64, i: u32) {
    let Some(d) = dec_of(t) else { return };
    if i as usize >= BUFS {
        return;
    }
    let mut st = d.st.lock().unwrap();
    if !st.buf[i as usize].reg {
        crate::log_once!("cv rel unreg", "[RED] nn::codec: Release of buffer {i}, which is not registered");
        return;
    }
    st.buf[i as usize].gen += 1;
    st.buf[i as usize].filled = false;
    if st.pend.len() < BUFS {
        st.pend.push_back(i);
    }
    if st.fifo.len() < 8 {
        st.fifo.push_back(i);
    }
    let before = pending(&st);
    fill(&d, &mut st, false);
    ready(&d, before, false, &st);
}
fn dec_props(w: u64, h: u64, cw: u64, ch: u64, ts: u64, eos: u64, t: u64, i: u32) -> u64 {
    let Some(d) = dec_of(t) else { return BAD };
    if i as usize >= BUFS {
        return BAD;
    }
    let st = d.st.lock().unwrap();
    let b = &st.buf[i as usize];
    if !b.filled {
        return NONE;
    }
    crate::guest::out(w, b.w);
    crate::guest::out(cw, b.w);
    crate::guest::out(h, b.h);
    crate::guest::out(ch, b.h);
    crate::guest::out(ts, b.ts);
    if eos != 0 {
        w8(eos, b.eos as u8);
    }
    0
}

/// The host's sync handles signal once their buffer holds the picture of that release; a handle the host did
/// not make reads as signalled.
fn sync_reached(s: u64) -> bool {
    let (magic, dec, idx, gen) = (r32(s), r32(s + 4), r32(s + 8), r32(s + 12));
    if magic != SYNC_MAGIC {
        if magic != 0 || dec != 0 {
            crate::log_once!("cv foreign sync", "[RED] nn::codec: a sync handle the host did not make ({magic:#x}): signalled");
        }
        return true;
    }
    if gen == 0 {
        return true;
    }
    let d = DECS.lock().unwrap().get((dec as usize).wrapping_sub(1)).cloned().flatten();
    match d {
        Some(d) if (idx as usize) < BUFS => {
            let st = d.st.lock().unwrap();
            let b = &st.buf[idx as usize];
            b.filled && b.filled_gen >= gen
        }
        _ => true,
    }
}

// ---- nn::syncpt events: a SystemEvent at +0 the host binds, the awaited handle at +0x40

static SPT: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn spt_fire(dec: u32, idx: u32, gen: u32) {
    let mut g = SPT.lock().unwrap();
    g.retain(|&e| {
        let (d, i, want) = (r32(e + 0x4c), r32(e + 0x50), r32(e + 0x54));
        if d == dec && i == idx && gen >= want {
            w32(e + 0x44, 0);
            crate::services::os::signal_bound(e);
            false
        } else {
            true
        }
    });
}
fn spt_unarm(e: u64) {
    SPT.lock().unwrap().retain(|&x| x != e);
    w32(e + 0x44, 0);
}

// ---- the takeovers

macro_rules! cfn {
    ($name:ident, |$c:ident, $a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name($c: *mut Cpu) {
            let mut $a = Args(&mut *$c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*$c).x[0] = r;
        }
    };
}

cfn!(ok, |c, a| { 0 });
/// MakeDecodeImagePreference(w, h, format, mode): a 24-byte VideoDecoderImagePreference returned through x8.
unsafe fn pref(c: *mut Cpu) {
    let a = Args(&mut *c);
    let o = (*c).x[8];
    let (w, h, f, m) = (a.x(0) as u16, a.x(1) as u16, a.w(2), a.w(3));
    crate::guest::zero(o, 24);
    w16(o, w);
    w16(o + 2, h);
    w32(o + 4, f);
    w32(o + 8, m);
    w32(o + 12, MAGIC);
}
cfn!(pool_size, |c, a| {
    let (size, align, p, n) = (a.x(0), a.x(1), a.x(2), a.w(3));
    crate::guest::out(size, native_size(r16(p) as u32, r16(p + 2) as u32) * n.max(1) as u64);
    crate::guest::out(align, NATIVE_ALIGN);
    0
});
cfn!(pool_init, |c, a| {
    let (t, mem, size, p, n) = (a.x(0), a.x(1), a.x(2), a.x(3), a.w(4));
    let key = TAG | 0x0100_0000_0000 | NEXT.fetch_add(1, Ordering::Relaxed);
    let pool = Pool { w: r16(p) as u32, h: r16(p + 2) as u32, fmt: r32(p + 4), count: n.max(1), img: vec![None; n.max(1) as usize], mem, size, native: false, slot: 0 };
    crate::log_once!("cv pool", "[codec] image pool {}x{} format {}, {} picture(s): the host keeps the decoded pictures", pool.w, pool.h, pool.fmt, pool.count);
    POOLS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, pool);
    w64(t, key);
    0
});
cfn!(pool_fin, |c, a| {
    let t = a.x(0);
    if let Some(k) = pool_key(t) {
        if let Some(p) = POOLS.lock().unwrap().as_mut().and_then(|m| m.remove(&k)) {
            if p.slot != 0 {
                POOL_SLOTS.lock().unwrap()[p.slot as usize - 1] = 0;
            }
        }
        for d in DECS.lock().unwrap().iter().flatten() {
            for b in d.st.lock().unwrap().buf.iter_mut() {
                if b.pool == k {
                    b.pool = 0;
                }
            }
        }
        w64(t, 0);
    }
    0
});
cfn!(pool_native, |c, a| {
    let (t, i) = (a.x(0), a.w(1));
    let Some(k) = pool_key(t) else { return 0 };
    let mut g = POOLS.lock().unwrap();
    let Some(p) = g.as_mut().and_then(|m| m.get_mut(&k)) else { return 0 };
    if i >= p.count {
        return 0;
    }
    if p.slot == 0 && !claim_slot(p, k) {
        return 0;
    }
    p.native = true;
    ((p.slot << 8) | i) as u64
});

/// Gives pool `k` a native image slot (the handle's high bits); false when all 256 are taken.
fn claim_slot(p: &mut Pool, k: u64) -> bool {
    let mut s = POOL_SLOTS.lock().unwrap();
    let at = s.iter().position(|&x| x == 0).unwrap_or_else(|| {
        s.push(0);
        s.len() - 1
    });
    if at >= 256 {
        crate::log_once!("cv native", "[RED] nn::codec: more than 256 pools with native images");
        return false;
    }
    s[at] = k;
    p.slot = at as u32 + 1;
    true
}

/// Blank native images' memory, kept for reuse by size once freed (the kernel region has no free).
static BLANK_FREE: Mutex<Vec<(u32, u32, u64)>> = Mutex::new(Vec::new());

/// A blank (zeroed) native image of `w` x `h` the graphics layer can draw, for a picture source the host does not
/// have (GameChat's camera images): a one-picture pool of its own. 0 when no slot is left.
pub fn native_image_blank(w: u32, h: u32) -> u32 {
    let size = native_size(w, h);
    let mem = {
        let mut f = BLANK_FREE.lock().unwrap();
        match f.iter().position(|&(fw, fh, _)| fw == w && fh == h) {
            Some(i) => f.swap_remove(i).2,
            None => nxrt::kernel::get().space.kernel_alloc(size, NATIVE_ALIGN),
        }
    };
    crate::guest::zero(mem, size);
    let key = TAG | 0x0100_0000_0000 | NEXT.fetch_add(1, Ordering::Relaxed);
    let mut pool = Pool { w, h, fmt: 0, count: 1, img: vec![None], mem, size, native: true, slot: 0 };
    if !claim_slot(&mut pool, key) {
        BLANK_FREE.lock().unwrap().push((w, h, mem));
        return 0;
    }
    let handle = pool.slot << 8;
    POOLS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, pool);
    handle
}

/// Releases a blank native image `native_image_blank` made.
pub fn native_image_blank_free(handle: u32) {
    let slot = handle >> 8;
    if handle == 0 || slot == 0 {
        return;
    }
    let key = {
        let mut s = POOL_SLOTS.lock().unwrap();
        match s.get_mut(slot as usize - 1) {
            Some(k) if *k != 0 => std::mem::take(k),
            _ => return,
        }
    };
    if let Some(p) = POOLS.lock().unwrap().as_mut().and_then(|m| m.remove(&key)) {
        BLANK_FREE.lock().unwrap().push((p.w, p.h, p.mem));
    }
}
cfn!(read_hdr, |c, a| {
    let (dst, size) = (a.x(6), a.x(8));
    crate::log_once!("cv hdr", "[RED] nn::codec::ReadDecoderOutputImageRgbaHdr: HDR pictures are not served (black)");
    crate::guest::zero(dst, size);
    0
});
// ReadDecoderOutputImageRgba(pool, i, x, y, w, h, dst, stride, size)
cfn!(read_rgba, |c, a| {
    let (t, i, x, y, w, h, dst, stride, size) = (a.x(0), a.w(1), a.x(2) as u16, a.x(3) as u16, a.x(4) as u16 as usize, a.x(5) as u16 as usize, a.x(6), a.x(7), a.x(8));
    let g = POOLS.lock().unwrap();
    let pic = pool_key(t).and_then(|k| g.as_ref()?.get(&k)?.img.get(i as usize)?.clone());
    let Some(pic) = pic else {
        crate::guest::zero(dst, size);
        return 0;
    };
    if x != 0 || y != 0 || stride != (w * 4) as u64 || size < (w * h * 4) as u64 {
        crate::log_once!("cv rgba region", "[RED] nn::codec::ReadDecoderOutputImageRgba: region {x},{y} stride {stride}: only whole tight pictures are served");
        return 0;
    }
    pic.rgba_into(crate::guest::bytes_mut(dst, (w * h * 4) as u64), w, h);
    0
});
// ReadDecoderOutputImageNv12(pool, i, x, y, w, h, y dst, y stride, y size, uv dst, uv stride, uv size)
cfn!(read_nv12, |c, a| {
    let (t, i, x, y, w, h) = (a.x(0), a.w(1), a.x(2) as u16, a.x(3) as u16, a.x(4) as u16 as usize, a.x(5) as u16 as usize);
    let (yd, ys, ysz, uvd, uvs, uvsz) = (a.x(6), a.x(7), a.x(8), a.x(9), a.x(10), a.x(11));
    if x != 0 || y != 0 || ys != w as u64 || uvs != w as u64 || ysz < (w * h) as u64 || uvsz < (w * h / 2) as u64 {
        crate::log_once!("cv nv12 region", "[RED] nn::codec::ReadDecoderOutputImageNv12: region {x},{y} strides {ys} / {uvs}: only whole tight pictures are served");
        return 0;
    }
    let g = POOLS.lock().unwrap();
    let pic = pool_key(t).and_then(|k| g.as_ref()?.get(&k)?.img.get(i as usize)?.clone());
    drop(g);
    let mut tmp = vec![0u8; w * h * 3 / 2];
    match pic {
        Some(p) => p.nv12_into(&mut tmp, w, h),
        None => {
            tmp[..w * h].fill(16);
            tmp[w * h..].fill(128);
        }
    }
    crate::guest::write_bytes(yd, &tmp[..w * h]);
    crate::guest::write_bytes(uvd, &tmp[w * h..]);
    0
});

// the decoder families (Vp8 / Vp9 / H264 / Av1 DecoderType: one layout)
cfn!(work, |c, a| {
    crate::guest::out(a.x(0), 0x1000u64);
    crate::guest::out(a.x(1), 0x1000u64);
    0
});
fn kind_of(name: &str) -> Kind {
    if name.contains("Vp9") {
        Kind::Vp9
    } else if name.contains("Vp8") {
        Kind::Vp8
    } else if name.contains("H264") {
        Kind::H264
    } else {
        Kind::Av1
    }
}
unsafe fn kind_here(c: *mut Cpu) -> Kind {
    let n = nxrt::hostfn::name_of((*c).pc).unwrap_or_default();
    kind_of(&n)
}
/// Initialize…Decoder(t, work, wsize, bs, w, h, refs, in, out[, mode][, csd, csd_n]): the work-memory forms.
unsafe fn init_work(c: *mut Cpu) {
    let a = Args(&mut *c);
    let k = kind_here(c);
    let n = nxrt::hostfn::name_of((*c).pc).unwrap_or_default();
    let with_mode = n.contains("OutputMode");
    let (csd, csd_n) = if k == Kind::H264 { if with_mode { (a.x(10), a.x(11)) } else { (a.x(9), a.x(10)) } } else { (0, 0) };
    let (t, w, h, inf, outf) = (a.x(0), a.x(4) as u16 as u32, a.x(5) as u16 as u32, a.w(7), a.w(8));
    (*c).x[0] = dec_init(c, t, k, w, h, inf, outf, csd, csd_n);
}
/// Initialize…Decoder(t, bs, w, h, refs, in, out[, mode][, csd, csd_n]): the remote-memory forms.
unsafe fn init_remote(c: *mut Cpu) {
    let a = Args(&mut *c);
    let k = kind_here(c);
    let n = nxrt::hostfn::name_of((*c).pc).unwrap_or_default();
    let with_mode = n.contains("OutputMode");
    let (csd, csd_n) = if k == Kind::H264 { if with_mode { (a.x(8), a.x(9)) } else { (a.x(7), a.x(8)) } } else { (0, 0) };
    let (t, w, h, inf, outf) = (a.x(0), a.x(2) as u16 as u32, a.x(3) as u16 as u32, a.w(5), a.w(6));
    (*c).x[0] = dec_init(c, t, k, w, h, inf, outf, csd, csd_n);
}
/// SDK 19's remote-memory form: (t, bs, mem, w, h, refs, in, out[, csd, csd_n]).
unsafe fn init_remote19(c: *mut Cpu) {
    let a = Args(&mut *c);
    let k = kind_here(c);
    let (csd, csd_n) = if k == Kind::H264 { (a.x(8), a.x(9)) } else { (0, 0) };
    let (t, w, h, inf, outf) = (a.x(0), a.x(3) as u16 as u32, a.x(4) as u16 as u32, a.w(6), a.w(7));
    (*c).x[0] = dec_init(c, t, k, w, h, inf, outf, csd, csd_n);
}
unsafe fn fin(c: *mut Cpu) {
    let t = (*c).x[0];
    (*c).x[0] = dec_fin(c, t);
}
cfn!(reg, |c, a| { dec_reg(a.x(0), a.w(1), a.x(2), a.w(3)) });
cfn!(unreg, |c, a| { dec_unreg(a.x(0), a.w(1)) });
cfn!(start, |c, a| { dec_start(a.x(0)) });
cfn!(stop, |c, a| { dec_reset(a.x(0)) });
cfn!(event_out, |c, a| {
    let Some(d) = dec_of(a.x(1)) else { return BAD };
    w64(a.x(0), d.event);
    0
});
cfn!(event22, |c, a| { dec_of(a.x(0)).map_or(0, |d| d.event) });
// Submit…DecoderInputBuffer(u64 *out, t, data, size, TimeSpan ts, bool eos)
cfn!(submit, |c, a| { dec_submit(a.x(0), a.x(1), a.x(2), a.x(3), a.x(4) as i64, a.b(5)) });
cfn!(encrypted, |c, a| {
    crate::log_once!("cv enc", "[RED] nn::codec: an encrypted input buffer: protected streams are not served");
    BAD
});
cfn!(acquire, |c, a| { dec_acq(a.x(0), a.x(1), a.x(2)) });
unsafe fn release(c: *mut Cpu) {
    let a = Args(&mut *c);
    dec_rel(a.x(0), a.w(1));
}
// Get…DecoderOutputBufferProperties(w, h, cw, ch, TimeSpan *ts, bool *eos, t, i)
cfn!(props, |c, a| { dec_props(a.x(0), a.x(1), a.x(2), a.x(3), a.x(4), a.x(5), a.x(6), a.w(7)) });
// SDK 19 / the colour-space form: (w, h, cw, ch, ColorSpace *cs, TimeSpan *ts, bool *eos, t, i)
cfn!(props19, |c, a| {
    crate::guest::out(a.x(4), 0u32);
    dec_props(a.x(0), a.x(1), a.x(2), a.x(3), a.x(5), a.x(6), a.x(7), a.x(8) as u32)
});

cfn!(trywait_native, |c, a| { sync_reached(a.x(0)) as u64 });
unsafe fn spt_create(c: *mut Cpu) {
    let e = (*c).x[0];
    crate::guest::zero(e, 0x58);
    crate::services::os::bind_system_event(c, e, false);
    w32(e + 0x40, SYNC_MAGIC);
    (*c).x[0] = 0;
}
unsafe fn spt_holder(c: *mut Cpu) {
    let (holder, e) = ((*c).x[0], (*c).x[1]);
    if let Some(f) = crate::hle::sym("_ZN2nn2os25InitializeMultiWaitHolderEPNS0_19MultiWaitHolderTypeEPNS0_15SystemEventTypeE") {
        let _ = crate::guest::call(c, f, &[holder, e]);
    }
}
cfn!(spt_request, |c, a| {
    let (e, s) = (a.x(0), a.x(1));
    spt_unarm(e);
    crate::services::os::clear_system_event(e);
    for k in 0..16 {
        w8(e + 0x48 + k, crate::guest::r8(s + k));
    }
    w32(e + 0x44, 1);
    let mut g = SPT.lock().unwrap();
    if g.len() >= 64 {
        drop(g);
        crate::log_once!("spt full", "[RED] nn::syncpt: more than 64 armed events: signalled at once");
        crate::services::os::signal_bound(e);
        return 0;
    }
    g.push(e);
    drop(g);
    if sync_reached(s) {
        spt_unarm(e);
        crate::services::os::signal_bound(e);
    }
    0
});
cfn!(spt_cancel, |c, a| {
    spt_unarm(a.x(0));
    crate::services::os::clear_system_event(a.x(0));
    0
});
unsafe fn spt_trywait(c: *mut Cpu) {
    let e = (*c).x[0];
    let r = crate::hle::sym("_ZN2nn2os18TryWaitSystemEventEPNS0_15SystemEventTypeE").and_then(|f| crate::guest::call(c, f, &[e])).unwrap_or(0);
    (*c).x[0] = (r & 0xff != 0) as u64;
}
unsafe fn spt_destroy(c: *mut Cpu) {
    let e = (*c).x[0];
    spt_unarm(e);
    if let Some(f) = crate::hle::sym("_ZN2nn2os18DestroySystemEventEPNS0_15SystemEventTypeE") {
        let _ = crate::guest::call(c, f, &[e]);
    }
    w32(e + 0x40, 0);
}

/// The video decoders' lines for the log.
pub fn report() -> Vec<String> {
    DECS.lock()
        .unwrap()
        .iter()
        .flatten()
        .map(|d| {
            let st = d.st.lock().unwrap();
            format!("[codec] {:?} decoder {}: {} packets submitted, {} decoded, {} pictures ({} not black)", d.kind, d.id, st.submits, st.dec_n, st.pictures, st.lit)
        })
        .collect()
}

pub fn register(r: &mut Registry) {
    const W: &str = "nn::codec video decoders: decoded on the host (VP9 libvpx, H.264 Media Foundation, AV1 dav1d)";
    let mut f = |n: &str, h: unsafe fn(*mut Cpu)| r.force(n, W, h);
    f("_ZN2nn5codec5video10InitializeEv", ok);
    f("_ZN2nn5codec5video8FinalizeEv", ok);
    f("_ZN2nn5codec25MakeDecodeImagePreferenceEttNS0_24VideoDecoderOutputFormatENS0_9CodecModeE", pref);
    f("_ZN2nn5codec25MakeDecodeImagePreferenceEttNS0_24VideoDecoderOutputFormatENS0_15ImageTilingModeE", pref);
    f("_ZN2nn5codec3sdk34MakeDecodeImagePreferenceForCompatEttNS0_24VideoDecoderOutputFormatENS0_9CodecModeE", pref);
    f("_ZN2nn5codec31CalculateDecoderImagePoolBufferEPmS1_RKNS0_27VideoDecoderImagePreferenceEj", pool_size);
    f("_ZN2nn5codec26InitializeDecoderImagePoolEPNS0_25VideoDecoderImagePoolTypeEPvmRKNS0_27VideoDecoderImagePreferenceEjNS0_9CodecModeE", pool_init);
    f("_ZN2nn5codec26InitializeDecoderImagePoolEPNS0_25VideoDecoderImagePoolTypeEPvmRKNS0_27VideoDecoderImagePreferenceEj", pool_init);
    f("_ZN2nn5codec24FinalizeDecoderImagePoolEPNS0_25VideoDecoderImagePoolTypeE", pool_fin);
    f("_ZN2nn5codec36GetDecoderImagePoolNativeImageHandleEPKNS0_25VideoDecoderImagePoolTypeEj", pool_native);
    f("_ZN2nn5codec29ReadDecoderOutputImageRgbaHdrEPKNS0_25VideoDecoderImagePoolTypeEjttttPvmm", read_hdr);
    f("_ZN2nn5codec26ReadDecoderOutputImageRgbaEPKNS0_25VideoDecoderImagePoolTypeEjttttPvmm", read_rgba);
    f("_ZN2nn5codec26ReadDecoderOutputImageNv12EPKNS0_25VideoDecoderImagePoolTypeEjttttPvmmS4_mm", read_nv12);
    for (cls, ty) in [("Vp9", "14Vp9DecoderType"), ("Vp8", "14Vp8DecoderType"), ("H264", "15H264DecoderType"), ("Av1", "14Av1DecoderType")] {
        let l = cls.len();
        let csd = if cls == "H264" { "PKvm" } else { "" };
        f(&format!("_ZN2nn5codec{}Calculate{cls}DecoderWorkBufferEPmS1_mtt", 26 + l), work);
        f(&format!("_ZN2nn5codec{}Initialize{cls}DecoderEPNS0_{ty}EPvmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatE{csd}", 17 + l), init_work);
        f(&format!("_ZN2nn5codec{}Initialize{cls}DecoderEPNS0_{ty}EmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatE{csd}", 17 + l), init_remote);
        f(&format!("_ZN2nn5codec{}Initialize{cls}DecoderEPNS0_{ty}EmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatE{csd}", 17 + l), init_remote19);
        f(&format!("_ZN2nn5codec{}Initialize{cls}DecoderByTransferMemoryEPNS0_{ty}EPvmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatENS0_22VideoDecoderOutputModeE{csd}", 33 + l), init_work);
        f(&format!("_ZN2nn5codec{}Initialize{cls}DecoderByRemoteMemoryEPNS0_{ty}EmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatENS0_22VideoDecoderOutputModeE{csd}", 31 + l), init_remote);
        f(&format!("_ZN2nn5codec{}Finalize{cls}DecoderEPNS0_{ty}E", 15 + l), fin);
        f(&format!("_ZN2nn5codec{}Register{cls}DecoderBufferEPNS0_{ty}EjPNS0_25VideoDecoderImagePoolTypeEj", 21 + l), reg);
        f(&format!("_ZN2nn5codec{}Unregister{cls}DecoderBufferEPNS0_{ty}Ej", 23 + l), unreg);
        f(&format!("_ZN2nn5codec{}Start{cls}DecoderEPNS0_{ty}E", 12 + l), start);
        f(&format!("_ZN2nn5codec{}Stop{cls}DecoderEPNS0_{ty}E", 11 + l), stop);
        f(&format!("_ZN2nn5codec{}Flush{cls}DecoderEPNS0_{ty}E", 12 + l), stop);
        f(&format!("_ZN2nn5codec{}Get{cls}DecoderSubmissionReadyEventEPPNS_2os15SystemEventTypeEPNS0_{ty}E", 30 + l), event_out);
        f(&format!("_ZN2nn5codec{}Get{cls}DecoderSubmissionReadyEventEPNS0_{ty}E", 30 + l), event22);
        f(&format!("_ZN2nn5codec{}Submit{cls}DecoderInputBufferEPmPNS0_{ty}EPKvmNS_8TimeSpanEb", 24 + l), submit);
        f(&format!("_ZN2nn5codec{}SubmitEncrypted{cls}DecoderInputBufferEPmPNS0_{ty}EPKNS0_18EncryptionPropertyEPKvmNS_8TimeSpanEb", 33 + l), encrypted);
        f(&format!("_ZN2nn5codec{}Acquire{cls}DecoderOutputBufferEPjPNS_2gs16NativeSyncHandleEPNS0_{ty}E", 26 + l), acquire);
        f(&format!("_ZN2nn5codec{}Release{cls}DecoderOutputBufferEPNS0_{ty}EjNS_2gs16NativeSyncHandleE", 26 + l), release);
        f(&format!("_ZN2nn5codec{}Get{cls}DecoderOutputBufferPropertiesEPtS1_S1_S1_PNS_8TimeSpanEPbPNS0_{ty}Ej", 32 + l), props);
        f(&format!("_ZN2nn5codec{}Get{cls}DecoderOutputBufferPropertiesEPtS1_S1_S1_PNS_5movie10ColorSpaceEPNS_8TimeSpanEPbPNS0_{ty}Ej", 32 + l), props19);
    }
    f("_ZN2nn5codec21InitializeH264DecoderEPNS0_15H264DecoderTypeEPvmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatENS0_22VideoDecoderOutputModeEPKvm", init_work);
    f("_ZN2nn2gs17TryWaitNativeSyncERKNS0_16NativeSyncHandleE", trywait_native);
    f("_ZN2nn6syncpt3sdk20CreateSyncPointEventEPNS1_18SyncPointEventTypeE", spt_create);
    f("_ZN2nn6syncpt3sdk25InitializeMultiWaitHolderEPNS_2os19MultiWaitHolderTypeEPNS1_18SyncPointEventTypeE", spt_holder);
    f("_ZN2nn6syncpt3sdk21RequestSyncPointEventEPNS1_18SyncPointEventTypeERKNS1_15SyncPointHandleE", spt_request);
    f("_ZN2nn6syncpt3sdk20CancelSyncPointEventEPNS1_18SyncPointEventTypeE", spt_cancel);
    f("_ZN2nn6syncpt3sdk21TryWaitSyncPointEventEPKNS1_18SyncPointEventTypeE", spt_trywait);
    f("_ZN2nn6syncpt3sdk21DestroySyncPointEventEPNS1_18SyncPointEventTypeE", spt_destroy);
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_match_the_sdk() {
        let mut r = crate::hle::Registry::default();
        super::register(&mut r);
        let names: Vec<&str> = r.names().collect();
        for want in [
            "_ZN2nn5codec29CalculateVp9DecoderWorkBufferEPmS1_mtt",
            "_ZN2nn5codec20InitializeAv1DecoderEPNS0_14Av1DecoderTypeEPvmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatE",
            "_ZN2nn5codec36InitializeAv1DecoderByTransferMemoryEPNS0_14Av1DecoderTypeEPvmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatENS0_22VideoDecoderOutputModeE",
            "_ZN2nn5codec35InitializeH264DecoderByRemoteMemoryEPNS0_15H264DecoderTypeEmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatENS0_22VideoDecoderOutputModeEPKvm",
            "_ZN2nn5codec27SubmitVp9DecoderInputBufferEPmPNS0_14Vp9DecoderTypeEPKvmNS_8TimeSpanEb",
            "_ZN2nn5codec36SubmitEncryptedVp9DecoderInputBufferEPmPNS0_14Vp9DecoderTypeEPKNS0_18EncryptionPropertyEPKvmNS_8TimeSpanEb",
            "_ZN2nn5codec30AcquireH264DecoderOutputBufferEPjPNS_2gs16NativeSyncHandleEPNS0_15H264DecoderTypeE",
            "_ZN2nn5codec35GetAv1DecoderOutputBufferPropertiesEPtS1_S1_S1_PNS_8TimeSpanEPbPNS0_14Av1DecoderTypeEj",
            "_ZN2nn5codec33GetVp9DecoderSubmissionReadyEventEPPNS_2os15SystemEventTypeEPNS0_14Vp9DecoderTypeE",
            "_ZN2nn5codec34GetH264DecoderSubmissionReadyEventEPNS0_15H264DecoderTypeE",
            "_ZN2nn5codec36GetH264DecoderOutputBufferPropertiesEPtS1_S1_S1_PNS_5movie10ColorSpaceEPNS_8TimeSpanEPbPNS0_15H264DecoderTypeEj",
            "_ZN2nn5codec20InitializeVp9DecoderEPNS0_14Vp9DecoderTypeEmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatE",
            "_ZN2nn5codec27UnregisterH264DecoderBufferEPNS0_15H264DecoderTypeEj",
            "_ZN2nn5codec37InitializeH264DecoderByTransferMemoryEPNS0_15H264DecoderTypeEPvmmttjNS0_23VideoDecoderInputFormatENS0_24VideoDecoderOutputFormatENS0_22VideoDecoderOutputModeEPKvm",
        ] {
            assert!(names.contains(&want), "{want}");
        }
    }
}
