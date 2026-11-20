//! nn::codec's hardware Opus decoders (the hwopus service on a console), decoded on the host with libopus:
//! the `HardwareOpusDecoder` / `HardwareOpusMultiStreamDecoder` classes, their C interface, SDK 21's
//! asynchronous decoder manager, and the `hwopus` IPC service itself for SDK clients that talk to it directly
//! (Turbo c406ae6: before libopus was linked every packet decoded to silence).
//!
//! Packets carry the Switch's 8-byte header (size and final range, big-endian); the PCM comes out interleaved
//! 16-bit. A game's work buffer is not used: the host keeps each decoder's state by the guest object's address.

use crate::guest::{r64, w32, w64, Args};
use crate::hle::Registry;
use crate::ipc::{self, Ctx, Service};
use crate::media::opus::{self, Decoder};
use nxrt::cpu::Cpu;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

// nn::codec::OpusResult
const OR_OK: u64 = 0;
const OR_BAD_RATE: u64 = 2;
const OR_BAD_CH: u64 = 3;
const OR_SHORT_PCM: u64 = 6;
const OR_BROKEN: u64 = 7;
// nn::codec's Result for a request the manager cannot take (module 111).
const R_CODEC: u64 = 0x3a6f;

static DECS: Mutex<Option<HashMap<u64, Decoder>>> = Mutex::new(None);

fn with_dec<R>(obj: u64, f: impl FnOnce(&mut Decoder) -> R) -> Option<R> {
    DECS.lock().unwrap().as_mut()?.get_mut(&obj).map(f)
}

fn put_dec(obj: u64, d: Decoder) {
    let first = DECS.lock().unwrap().get_or_insert_with(HashMap::new).insert(obj, d).is_none();
    if first {
        crate::log_once!("opus up", "codec: Opus decoders are {} on the host (hwopus)", opus::version());
    }
}

fn valid_rate(r: i32) -> bool {
    matches!(r, 8000 | 12000 | 16000 | 24000 | 48000)
}

// One framed packet decoded into a guest buffer: (OpusResult, consumed, samples a channel).
fn decode_into(d: &mut Decoder, inp: u64, in_size: u64, out: u64, out_size: u64) -> (u64, u64, i32) {
    let data = crate::guest::bytes(inp, in_size);
    let Some((pkt, used)) = opus::frame(data) else { return (OR_BROKEN, 0, 0) };
    let ch = d.channels.max(1) as usize;
    let mut pcm = vec![0i16; 5760 * ch];
    match d.decode(pkt, &mut pcm) {
        Ok(n) => {
            let bytes = n * ch * 2;
            if bytes as u64 > out_size {
                return (OR_SHORT_PCM, 0, 0);
            }
            let b = crate::guest::bytes_mut(out, bytes as u64);
            for (i, s) in pcm[..n * ch].iter().enumerate() {
                b[2 * i..2 * i + 2].copy_from_slice(&s.to_le_bytes());
            }
            (OR_OK, used as u64, n as i32)
        }
        Err(e) => {
            crate::log_once!(format!("opus err {e}"), "[RED] codec: libopus refused a {}-byte packet ({e})", pkt.len());
            (OR_BROKEN, 0, 0)
        }
    }
}

macro_rules! cfn {
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

// ---- the classes (this in x0)

unsafe fn ctor(c: *mut Cpu, orig: &str) {
    let this = (*c).x[0];
    match crate::hle::sym(orig) {
        Some(f) => {
            let a1 = (*c).x[1];
            let _ = crate::guest::call(c, f, &[this, a1]);
        }
        None => crate::guest::zero(this, 0x10),
    }
    DECS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&this);
}
unsafe fn ctor1(c: *mut Cpu) {
    ctor(c, "_ZN2nn5codec19HardwareOpusDecoderC1Ei")
}
unsafe fn ctor_ms(c: *mut Cpu) {
    ctor(c, "_ZN2nn5codec30HardwareOpusMultiStreamDecoderC1Ei")
}
cfn!(dtor, |a| {
    DECS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&a.x(0));
    0
});
cfn!(work_size, |a| { Decoder::size(a.i(2)).max(0x100) });
cfn!(work_size_ms, |a| { Decoder::multi_size(a.i(3), a.i(4)).max(0x100) });
cfn!(is_init, |a| { with_dec(a.x(0), |_| ()).is_some() as u64 });
cfn!(init, |a| {
    let (this, rate, ch) = (a.x(0), a.i(1), a.i(2));
    if !valid_rate(rate) {
        return OR_BAD_RATE;
    }
    if !(1..=2).contains(&ch) {
        return OR_BAD_CH;
    }
    match Decoder::new(rate, ch) {
        Ok(d) => {
            put_dec(this, d);
            OR_OK
        }
        Err(_) => OR_BAD_CH,
    }
});
cfn!(init_ms, |a| {
    let (this, rate, ch, streams, stereo, map) = (a.x(0), a.i(1), a.i(2), a.i(3), a.i(4), a.x(5));
    if !valid_rate(rate) {
        return OR_BAD_RATE;
    }
    let mapping = crate::guest::bytes(map, ch.clamp(0, 255) as u64).to_vec();
    match Decoder::multi(rate, ch, streams, stereo, &mapping) {
        Ok(d) => {
            put_dec(this, d);
            OR_OK
        }
        Err(_) => OR_BAD_CH,
    }
});
cfn!(fini, |a| {
    DECS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&a.x(0));
    0
});
cfn!(rate, |a| { with_dec(a.x(0), |d| d.rate as u64).unwrap_or(0) });
cfn!(channels, |a| { with_dec(a.x(0), |d| d.channels as u64).unwrap_or(0) });
cfn!(streams, |a| { with_dec(a.x(0), |d| d.streams as u64).unwrap_or(0) });
cfn!(stereo, |a| { with_dec(a.x(0), |d| d.stereo as u64).unwrap_or(0) });
cfn!(reset, |a| {
    with_dec(a.x(0), |d| d.reset());
    0
});
cfn!(set_context, |a| { OR_OK });
// DecodeInterleaved(this, size_t *consumed, int *samples, int16 *out, size_t out_size, const void *in, size_t in_size)
cfn!(decode_il, |a| {
    let (this, consumed, samples, out, out_size, inp, in_size) = (a.x(0), a.x(1), a.x(2), a.x(3), a.x(4), a.x(5), a.x(6));
    let r = with_dec(this, |d| decode_into(d, inp, in_size, out, out_size));
    let Some((res, used, n)) = r else { return OR_BROKEN };
    crate::guest::out(consumed, used);
    crate::guest::out(samples, n);
    res
});
// Decode(this, size_t *consumed, int *samples, int16 **outs, int count, size_t out_size, const void *in, size_t in_size):
// one buffer a channel
cfn!(decode_planar, |a| {
    let (this, consumed, samples, outs, count, out_size, inp, in_size) = (a.x(0), a.x(1), a.x(2), a.x(3), a.i(4), a.x(5), a.x(6), a.x(7));
    let r = with_dec(this, |d| {
        let data = crate::guest::bytes(inp, in_size);
        let Some((pkt, used)) = opus::frame(data) else { return (OR_BROKEN, 0, 0) };
        let ch = d.channels.max(1) as usize;
        let mut pcm = vec![0i16; 5760 * ch];
        match d.decode(pkt, &mut pcm) {
            Ok(n) => {
                if (n * 2) as u64 > out_size || (count as usize) < ch {
                    return (OR_SHORT_PCM, 0, 0);
                }
                for k in 0..ch {
                    let dst = r64(outs + 8 * k as u64);
                    let b = crate::guest::bytes_mut(dst, (n * 2) as u64);
                    for s in 0..n {
                        b[2 * s..2 * s + 2].copy_from_slice(&pcm[s * ch + k].to_le_bytes());
                    }
                }
                (OR_OK, used as u64, n as i32)
            }
            Err(_) => (OR_BROKEN, 0, 0),
        }
    });
    let Some((res, used, n)) = r else { return OR_BROKEN };
    crate::guest::out(consumed, used);
    crate::guest::out(samples, n);
    res
});

// ---- the C interface (HardwareOpusDecoderType*: the object's address is the key)

cfn!(c_work_size, |a| { Decoder::size(a.i(1)).max(0x100) });
cfn!(c_work_size_ms, |a| { Decoder::multi_size(a.i(2), a.i(3)).max(0x100) });
// InitializeHardwareOpusDecoder(type, rate, ch, buf, size, option)
cfn!(c_init, |a| {
    let (t, rate, ch) = (a.x(0), a.i(1), a.i(2));
    if !valid_rate(rate) {
        return OR_BAD_RATE;
    }
    match Decoder::new(rate, ch) {
        Ok(d) => {
            put_dec(t, d);
            OR_OK
        }
        Err(_) => OR_BAD_CH,
    }
});
// InitializeHardwareOpusMultiStreamDecoder(type, rate, ch, streams, stereo, mapping, buf, size, option)
cfn!(c_init_ms, |a| {
    let (t, rate, ch, streams, stereo, map) = (a.x(0), a.i(1), a.i(2), a.i(3), a.i(4), a.x(5));
    if !valid_rate(rate) {
        return OR_BAD_RATE;
    }
    let mapping = crate::guest::bytes(map, ch.clamp(0, 255) as u64).to_vec();
    match Decoder::multi(rate, ch, streams, stereo, &mapping) {
        Ok(d) => {
            put_dec(t, d);
            OR_OK
        }
        Err(_) => OR_BAD_CH,
    }
});
unsafe fn c_fini(c: *mut Cpu) {
    let t = (*c).x[0];
    if mgr_dec_fini(t) {
        return;
    }
    DECS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&t);
}
unsafe fn c_reset(c: *mut Cpu) {
    let t = (*c).x[0];
    if mgr_dec_reset(t) {
        return;
    }
    with_dec(t, |d| d.reset());
}
cfn!(c_rate, |a| { with_dec(a.x(0), |d| d.rate as u64).or_else(|| mgr_dec(a.x(0), |d| d.dec.rate as u64)).unwrap_or(0) });
cfn!(c_channels, |a| { with_dec(a.x(0), |d| d.channels as u64).or_else(|| mgr_dec(a.x(0), |d| d.dec.channels as u64)).unwrap_or(0) });
// DecodeOpusInterleavedWithHardware(type, size_t *consumed, int *samples, int16 *out, size_t out_size, in, in_size)
cfn!(c_decode, |a| {
    let (t, consumed, samples, out, out_size, inp, in_size) = (a.x(0), a.x(1), a.x(2), a.x(3), a.x(4), a.x(5), a.x(6));
    let Some((res, used, n)) = with_dec(t, |d| decode_into(d, inp, in_size, out, out_size)) else { return OR_BROKEN };
    crate::guest::out(consumed, used);
    crate::guest::out(samples, n);
    res
});
cfn!(c_time, |a| { 0 });

// ---- SDK 21's decoder manager: packets queued per decoder, decoded in Execute, dequeued as PCM

struct MgrDec {
    mgr: u64,
    dec: Decoder,
    wait: VecDeque<Vec<u8>>,
    done: VecDeque<(Vec<u8>, i32)>,
}
struct Mgr {
    event: u64,
    io_total: i32,
    io_used: i32,
}
static MGRS: Mutex<Option<HashMap<u64, Mgr>>> = Mutex::new(None);
static MDECS: Mutex<Option<HashMap<u64, MgrDec>>> = Mutex::new(None);

fn mgr_dec<R>(gd: u64, f: impl FnOnce(&mut MgrDec) -> R) -> Option<R> {
    MDECS.lock().unwrap().as_mut()?.get_mut(&gd).map(f)
}
fn mgr_dec_fini(gd: u64) -> bool {
    let d = MDECS.lock().unwrap().as_mut().and_then(|m| m.remove(&gd));
    let Some(d) = d else { return false };
    if let Some(m) = MGRS.lock().unwrap().as_mut().and_then(|m| m.get_mut(&d.mgr)) {
        m.io_used -= (d.wait.len() + d.done.len()) as i32;
    }
    true
}
fn mgr_dec_reset(gd: u64) -> bool {
    let mut g = MDECS.lock().unwrap();
    let Some(d) = g.as_mut().and_then(|m| m.get_mut(&gd)) else { return false };
    let n = (d.wait.len() + d.done.len()) as i32;
    d.wait.clear();
    d.done.clear();
    d.dec.reset();
    let mgr = d.mgr;
    drop(g);
    if let Some(m) = MGRS.lock().unwrap().as_mut().and_then(|m| m.get_mut(&mgr)) {
        m.io_used -= n;
    }
    true
}
cfn!(mgr_work_size, |a| { 0x1000 });
// InitializeHardwareOpusDecoderManager(mgr, event, rate, ch, TimeSpan frame, decoders, io, buf, size)
unsafe fn mgr_init(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (gm, ev, io) = (a.x(0), a.x(1), a.i(6));
    crate::services::os::bind_system_event(c, ev, true);
    w64(gm, gm);
    MGRS.lock().unwrap().get_or_insert_with(HashMap::new).insert(gm, Mgr { event: ev, io_total: io, io_used: 0 });
    crate::log!("codec: Opus decoder manager {gm:#x}: {io} I/O buffers, decoded by libopus at each Execute");
    (*c).x[0] = 0;
}
cfn!(mgr_fini, |a| {
    let gm = a.x(0);
    MGRS.lock().unwrap().get_or_insert_with(HashMap::new).remove(&gm);
    MDECS.lock().unwrap().get_or_insert_with(HashMap::new).retain(|_, d| d.mgr != gm);
    w64(gm, 0);
    0
});
cfn!(mgr_free_io, |a| { MGRS.lock().unwrap().as_ref().and_then(|m| m.get(&a.x(0))).map_or(0, |m| (m.io_total - m.io_used).max(0) as u64) });
// InitializeHardwareOpusDecoder(type, rate, ch, TimeSpan frame, mgr)
cfn!(mgr_dec_init, |a| {
    let (gd, rate, ch, gm) = (a.x(0), a.i(1), a.i(2), a.x(4));
    match Decoder::new(rate, ch) {
        Ok(dec) => {
            MDECS.lock().unwrap().get_or_insert_with(HashMap::new).insert(gd, MgrDec { mgr: gm, dec, wait: VecDeque::new(), done: VecDeque::new() });
            0
        }
        Err(_) => R_CODEC,
    }
});
// EnqueueOpusDataToHardwareOpusDecoder(type, size_t *consumed, in, in_size)
cfn!(mgr_enqueue, |a| {
    let (gd, consumed, inp, n) = (a.x(0), a.x(1), a.x(2), a.x(3));
    crate::guest::out(consumed, 0u64);
    let Some(gm) = mgr_dec(gd, |d| d.mgr) else { return R_CODEC };
    {
        let mut g = MGRS.lock().unwrap();
        let Some(m) = g.as_mut().and_then(|m| m.get_mut(&gm)) else { return R_CODEC };
        if !(8..=0x1800).contains(&n) || m.io_used >= m.io_total {
            return R_CODEC;
        }
        m.io_used += 1;
    }
    let pkt = crate::guest::bytes(inp, n).to_vec();
    mgr_dec(gd, |d| d.wait.push_back(pkt));
    crate::guest::out(consumed, n);
    0
});
cfn!(mgr_execute, |a| {
    let gm = a.x(0);
    let mut any = false;
    if let Some(m) = MDECS.lock().unwrap().as_mut() {
        for d in m.values_mut().filter(|d| d.mgr == gm) {
            while let Some(p) = d.wait.pop_front() {
                let ch = d.dec.channels.max(1) as usize;
                let mut pcm = vec![0i16; 5760 * ch];
                let n = match opus::frame(&p).map(|(pkt, _)| d.dec.decode(pkt, &mut pcm)) {
                    Some(Ok(n)) if n > 0 => n,
                    _ => {
                        crate::log_once!("opus mgr bad", "[RED] codec: a {}-byte Opus packet did not decode: 20 ms of silence instead", p.len());
                        pcm.iter_mut().for_each(|s| *s = 0);
                        960
                    }
                };
                let bytes: Vec<u8> = pcm[..n * ch].iter().flat_map(|s| s.to_le_bytes()).collect();
                d.done.push_back((bytes, n as i32));
                any = true;
            }
        }
    }
    if any {
        if let Some(ev) = MGRS.lock().unwrap().as_ref().and_then(|m| m.get(&gm)).map(|m| m.event) {
            crate::services::os::signal_bound(ev);
        }
    }
    0
});
// DequeuePcmInterleavedDataFromHardwareOpusDecoder(type, int *samples, int16 *out, size_t out_size)
cfn!(mgr_dequeue, |a| {
    let (gd, samples, out, out_size) = (a.x(0), a.x(1), a.x(2), a.x(3));
    crate::guest::out(samples, 0i32);
    let r = mgr_dec(gd, |d| match d.done.front() {
        Some((b, _)) if b.len() as u64 > out_size => Err(()),
        Some(_) => Ok(Some((d.done.pop_front().unwrap(), d.mgr))),
        None => Ok(None),
    });
    match r {
        Some(Ok(Some(((b, n), gm)))) => {
            crate::guest::write_bytes(out, &b);
            crate::guest::out(samples, n);
            if let Some(m) = MGRS.lock().unwrap().as_mut().and_then(|m| m.get_mut(&gm)) {
                m.io_used -= 1;
            }
            0
        }
        _ => R_CODEC,
    }
});
cfn!(mgr_waiting, |a| { mgr_dec(a.x(0), |d| d.wait.len() as u64).unwrap_or(0) });
cfn!(mgr_executing, |a| { 0 });
cfn!(mgr_completed, |a| { mgr_dec(a.x(0), |d| d.done.len() as u64).unwrap_or(0) });

// ---- the hwopus IPC service (IHardwareOpusDecoderManager, IHardwareOpusDecoder)

struct HwOpus;
struct HwDecoder {
    dec: Decoder,
}

fn ms_params(b: &[u8]) -> (i32, i32, i32, i32, Vec<u8>) {
    let g = |o: usize| b.get(o..o + 4).map_or(0, |s| i32::from_le_bytes(s.try_into().unwrap()));
    let ch = g(4);
    let mapping = b.get(16..16 + ch.clamp(0, 255) as usize).map(|s| s.to_vec()).unwrap_or_default();
    (g(0), ch, g(8), g(12), mapping)
}

impl Service for HwOpus {
    fn name(&self) -> String {
        "hwopus".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // OpenHardwareOpusDecoder(rate, ch, work size; work memory) / Ex (rate, ch, large frames)
            0 | 4 => {
                let (rate, ch) = (ctx.u32(0) as i32, ctx.u32(4) as i32);
                match Decoder::new(rate, ch) {
                    Ok(dec) => {
                        crate::log_once!("hwopus ipc", "hwopus: decoders opened through the service, decoded by {}", opus::version());
                        ctx.object(HwDecoder { dec });
                        0
                    }
                    Err(_) => R_CODEC as u32,
                }
            }
            // GetWorkBufferSize / Ex / ExEx
            1 | 5 | 8 => {
                ctx.push_u32(Decoder::size(ctx.u32(4) as i32).max(0x1000) as u32);
                0
            }
            // OpenHardwareOpusDecoderForMultiStream / Ex: the parameters in a buffer
            2 | 6 => {
                let (rate, ch, streams, stereo, mapping) = ms_params(&ctx.read_in(0));
                match Decoder::multi(rate, ch, streams, stereo, &mapping) {
                    Ok(dec) => {
                        ctx.object(HwDecoder { dec });
                        0
                    }
                    Err(_) => R_CODEC as u32,
                }
            }
            3 | 7 | 9 => {
                let (_, _, streams, stereo, _) = ms_params(&ctx.read_in(0));
                ctx.push_u32(Decoder::multi_size(streams, stereo).max(0x1000) as u32);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

impl Service for HwDecoder {
    fn name(&self) -> String {
        "hwopus decoder".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // DecodeInterleaved(Old / WithPerf / WithPerfAndReset, single and multi-stream): in A or X, out B or C
            0 | 2 | 4 | 5 | 6 | 7 | 8 | 9 => {
                let cmd = ctx.cmd();
                if matches!(cmd, 6..=9) && ctx.u8(0) & 1 != 0 {
                    self.dec.reset();
                }
                let input = ctx.read_in(0);
                let Some((pkt, used)) = opus::frame(&input) else { return R_CODEC as u32 };
                let ch = self.dec.channels.max(1) as usize;
                let mut pcm = vec![0i16; 5760 * ch];
                let n = match self.dec.decode(pkt, &mut pcm) {
                    Ok(n) => n,
                    Err(e) => {
                        crate::log_once!("hwopus ipc err", "[RED] hwopus: libopus refused a packet ({e})");
                        return R_CODEC as u32;
                    }
                };
                let bytes: Vec<u8> = pcm[..n * ch].iter().flat_map(|s| s.to_le_bytes()).collect();
                ctx.write_out(0, &bytes);
                ctx.push_u32(used as u32);
                ctx.push_u32(n as u32);
                if cmd >= 4 {
                    ctx.push_u64(0); // the decode time
                }
                0
            }
            // SetContext / SetContextForMultiStream
            1 | 3 => 0,
            _ => ctx.unimplemented(),
        }
    }
}

pub fn register(r: &mut Registry) {
    ipc::register("hwopus", || Box::new(HwOpus));
    const W: &str = "hwopus: decoded on the host by libopus";
    let class = "_ZN2nn5codec19HardwareOpusDecoder";
    let ms = "_ZN2nn5codec30HardwareOpusMultiStreamDecoder";
    for (n, f) in [
        (format!("{class}C1Ei"), ctor1 as unsafe fn(*mut Cpu)),
        (format!("{class}C2Ei"), ctor1),
        (format!("{class}D0Ev"), dtor),
        (format!("{class}D1Ev"), dtor),
        (format!("{class}D2Ev"), dtor),
        ("_ZNK2nn5codec19HardwareOpusDecoder17GetWorkBufferSizeEii".into(), work_size),
        ("_ZNK2nn5codec19HardwareOpusDecoder13IsInitializedEv".into(), is_init),
        (format!("{class}10InitializeEiiPvm"), init),
        (format!("{class}8FinalizeEv"), fini),
        ("_ZNK2nn5codec19HardwareOpusDecoder13GetSampleRateEv".into(), rate),
        ("_ZNK2nn5codec19HardwareOpusDecoder15GetChannelCountEv".into(), channels),
        (format!("{class}5ResetEv"), reset),
        (format!("{class}17DecodeInterleavedEPmPiPsmPKvm"), decode_il),
        (format!("{class}6DecodeEPmPiPPsimPKvm"), decode_planar),
        (format!("{class}10SetContextEPKvm"), set_context),
        (format!("{ms}C1Ei"), ctor_ms),
        (format!("{ms}C2Ei"), ctor_ms),
        (format!("{ms}D0Ev"), dtor),
        (format!("{ms}D1Ev"), dtor),
        (format!("{ms}D2Ev"), dtor),
        ("_ZNK2nn5codec30HardwareOpusMultiStreamDecoder17GetWorkBufferSizeEiiii".into(), work_size_ms),
        ("_ZNK2nn5codec30HardwareOpusMultiStreamDecoder13IsInitializedEv".into(), is_init),
        (format!("{ms}10InitializeEiiiiPKhPvm"), init_ms),
        (format!("{ms}8FinalizeEv"), fini),
        ("_ZNK2nn5codec30HardwareOpusMultiStreamDecoder13GetSampleRateEv".into(), rate),
        ("_ZNK2nn5codec30HardwareOpusMultiStreamDecoder15GetChannelCountEv".into(), channels),
        ("_ZNK2nn5codec30HardwareOpusMultiStreamDecoder19GetTotalStreamCountEv".into(), streams),
        ("_ZNK2nn5codec30HardwareOpusMultiStreamDecoder20GetStereoStreamCountEv".into(), stereo),
        (format!("{ms}5ResetEv"), reset),
        (format!("{ms}17DecodeInterleavedEPmPiPsmPKvm"), decode_il),
        (format!("{ms}6DecodeEPmPiPPsimPKvm"), decode_planar),
        (format!("{ms}10SetContextEPKvm"), set_context),
        ("_ZN2nn5codec36GetHardwareOpusDecoderWorkBufferSizeEiii".into(), c_work_size),
        ("_ZN2nn5codec36GetHardwareOpusDecoderWorkBufferSizeEii".into(), c_work_size),
        ("_ZN2nn5codec29InitializeHardwareOpusDecoderEPNS0_23HardwareOpusDecoderTypeEiiPvmi".into(), c_init),
        ("_ZN2nn5codec27FinalizeHardwareOpusDecoderEPNS0_23HardwareOpusDecoderTypeE".into(), c_fini),
        ("_ZN2nn5codec32GetHardwareOpusDecoderSampleRateEPKNS0_23HardwareOpusDecoderTypeE".into(), c_rate),
        ("_ZN2nn5codec34GetHardwareOpusDecoderChannelCountEPKNS0_23HardwareOpusDecoderTypeE".into(), c_channels),
        ("_ZN2nn5codec24ResetHardwareOpusDecoderEPNS0_23HardwareOpusDecoderTypeE".into(), c_reset),
        ("_ZN2nn5codec33DecodeOpusInterleavedWithHardwareEPNS0_23HardwareOpusDecoderTypeEPmPiPsmPKvm".into(), c_decode),
        ("_ZN2nn5codec47GetHardwareOpusMultiStreamDecoderWorkBufferSizeEiiii".into(), c_work_size_ms),
        ("_ZN2nn5codec47GetHardwareOpusMultiStreamDecoderWorkBufferSizeEiiiii".into(), c_work_size_ms),
        ("_ZN2nn5codec40InitializeHardwareOpusMultiStreamDecoderEPNS0_34HardwareOpusMultiStreamDecoderTypeEiiiiPKhPvmi".into(), c_init_ms),
        ("_ZN2nn5codec38FinalizeHardwareOpusMultiStreamDecoderEPNS0_34HardwareOpusMultiStreamDecoderTypeE".into(), c_fini),
        ("_ZN2nn5codec43GetHardwareOpusMultiStreamDecoderSampleRateEPKNS0_34HardwareOpusMultiStreamDecoderTypeE".into(), c_rate),
        ("_ZN2nn5codec45GetHardwareOpusMultiStreamDecoderChannelCountEPKNS0_34HardwareOpusMultiStreamDecoderTypeE".into(), c_channels),
        ("_ZN2nn5codec49GetHardwareOpusMultiStreamDecoderTotalStreamCountEPKNS0_34HardwareOpusMultiStreamDecoderTypeE".into(), streams),
        ("_ZN2nn5codec50GetHardwareOpusMultiStreamDecoderStereoStreamCountEPKNS0_34HardwareOpusMultiStreamDecoderTypeE".into(), stereo),
        ("_ZN2nn5codec35ResetHardwareOpusMultiStreamDecoderEPNS0_34HardwareOpusMultiStreamDecoderTypeE".into(), c_reset),
        ("_ZN2nn5codec44DecodeOpusMultiStreamInterleavedWithHardwareEPNS0_34HardwareOpusMultiStreamDecoderTypeEPmPiPsmPKvm".into(), c_decode),
        ("_ZN2nn5codec47GetHardwareOpusMultiStreamDecoderProcessingTimeEPKNS0_34HardwareOpusMultiStreamDecoderTypeE".into(), c_time),
        ("_ZN2nn5codec43GetHardwareOpusDecoderManagerWorkBufferSizeEiiNS_8TimeSpanEii".into(), mgr_work_size),
        ("_ZN2nn5codec36InitializeHardwareOpusDecoderManagerEPNS0_30HardwareOpusDecoderManagerTypeEPNS_2os15SystemEventTypeEiiNS_8TimeSpanEiiPvm".into(), mgr_init),
        ("_ZN2nn5codec34FinalizeHardwareOpusDecoderManagerEPNS0_30HardwareOpusDecoderManagerTypeE".into(), mgr_fini),
        ("_ZN2nn5codec46GetHardwareOpusDecoderManagerFreeIoBufferCountEPKNS0_30HardwareOpusDecoderManagerTypeE".into(), mgr_free_io),
        ("_ZN2nn5codec29InitializeHardwareOpusDecoderEPNS0_23HardwareOpusDecoderTypeEiiNS_8TimeSpanEPNS0_30HardwareOpusDecoderManagerTypeE".into(), mgr_dec_init),
        ("_ZN2nn5codec36EnqueueOpusDataToHardwareOpusDecoderEPNS0_23HardwareOpusDecoderTypeEPmPKvm".into(), mgr_enqueue),
        ("_ZN2nn5codec34ExecuteHardwareOpusDecoderDecodingEPNS0_30HardwareOpusDecoderManagerTypeE".into(), mgr_execute),
        ("_ZN2nn5codec48DequeuePcmInterleavedDataFromHardwareOpusDecoderEPNS0_23HardwareOpusDecoderTypeEPiPsm".into(), mgr_dequeue),
        ("_ZN2nn5codec47GetHardwareOpusDecoderWaitingDecodeRequestCountEPKNS0_23HardwareOpusDecoderTypeE".into(), mgr_waiting),
        ("_ZN2nn5codec49GetHardwareOpusDecoderExecutingDecodeRequestCountEPKNS0_23HardwareOpusDecoderTypeE".into(), mgr_executing),
        ("_ZN2nn5codec49GetHardwareOpusDecoderCompletedDecodeRequestCountEPKNS0_23HardwareOpusDecoderTypeE".into(), mgr_completed),
    ] {
        r.force(&n, W, f);
    }
    let _ = w32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::hipc::{self, Buf, Builder, T_REQUEST};

    /// The Opus packets of an Ogg Opus file (RFC 7845): (pre-skip, audio packets).
    fn ogg_opus_packets(b: &[u8]) -> (usize, Vec<Vec<u8>>) {
        let (mut packets, mut cur, mut o) = (vec![], vec![], 0);
        while o + 27 <= b.len() {
            assert_eq!(&b[o..o + 4], b"OggS", "page at {o}");
            let nseg = b[o + 26] as usize;
            let lace = &b[o + 27..o + 27 + nseg];
            let mut p = o + 27 + nseg;
            for &l in lace {
                cur.extend_from_slice(&b[p..p + l as usize]);
                p += l as usize;
                if l < 255 {
                    packets.push(std::mem::take(&mut cur));
                }
            }
            o = p;
        }
        assert_eq!(&packets[0][..8], b"OpusHead");
        assert_eq!(&packets[1][..8], b"OpusTags");
        let pre_skip = u16::from_le_bytes([packets[0][10], packets[0][11]]) as usize;
        (pre_skip, packets.split_off(2))
    }

    /// The Switch's framing of one packet: size and final range (big-endian), then the packet.
    fn framed(p: &[u8]) -> Vec<u8> {
        let mut v = (p.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(p);
        v
    }

    fn send(h: u32, cmd: u32, data: Vec<u8>, a: &[u8], b: &mut [u8]) -> hipc::Response {
        let bd = |addr: u64, size: usize| Buf { addr, size: size as u64, tag: 0 };
        let mut m = Builder {
            ty: T_REQUEST,
            cmd,
            data,
            a: if a.is_empty() { vec![] } else { vec![bd(a.as_ptr() as u64, a.len())] },
            b: if b.is_empty() { vec![] } else { vec![bd(b.as_mut_ptr() as u64, b.len())] },
            ..Default::default()
        }
        .build();
        crate::ipc::process(h, &mut m);
        hipc::parse_response(&m, false).unwrap()
    }

    /// Every packet of the file through a hwopus decoder session: the PCM it wrote, interleaved.
    fn decode_ipc(dec: u32, cmd: u32, packets: &[Vec<u8>]) -> Vec<i16> {
        let mut pcm = vec![];
        let mut out = vec![0u8; 5760 * 2 * 2];
        for (i, p) in packets.iter().enumerate() {
            let input = framed(p);
            let data = if cmd >= 6 { vec![(i == 0) as u8, 0, 0, 0] } else { vec![] };
            let r = send(dec, cmd, data, &input, &mut out);
            assert_eq!(r.result, 0, "packet {i}");
            let consumed = u32::from_le_bytes(r.data[0..4].try_into().unwrap()) as usize;
            let samples = u32::from_le_bytes(r.data[4..8].try_into().unwrap()) as usize;
            assert_eq!(consumed, input.len(), "packet {i}: the whole frame is consumed");
            assert_eq!(samples, 960, "packet {i}: 20 ms at 48 kHz");
            assert!(cmd < 4 || r.data.len() >= 16, "the WithPerf commands add the decode time");
            pcm.extend(out[..samples * 4].chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]])));
        }
        pcm
    }

    /// Real Opus (ffmpeg + libopus: a 440 Hz left / 660 Hz right tone, 2 s, 96 kb/s, `testdata/media/tone.opus`)
    /// through the hwopus IPC service: single- and multi-stream sessions, the PCM checked sample for sample against
    /// libopus decoding the same packets, and against the tone itself.
    #[test]
    fn hwopus_ipc_decodes_real_opus_as_libopus() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/media/tone.opus")).unwrap();
        let (pre_skip, packets) = ogg_opus_packets(&file);
        assert!(packets.len() >= 100, "{} packets", packets.len());

        // libopus on its own
        let mut lib = Decoder::new(48000, 2).unwrap();
        let mut want = vec![];
        for p in &packets {
            let mut pcm = vec![0i16; 5760 * 2];
            let n = lib.decode(p, &mut pcm).unwrap();
            want.extend_from_slice(&pcm[..n * 2]);
        }

        let mgr = crate::ipc::open(Box::new(HwOpus));
        let mut none = [0u8; 0];
        let rate_ch = [48000u32.to_le_bytes(), 2u32.to_le_bytes()].concat();
        let r = send(mgr, 1, rate_ch.clone(), &[], &mut none);
        assert_eq!(r.result, 0);
        assert!(u32::from_le_bytes(r.data[..4].try_into().unwrap()) >= Decoder::size(2) as u32);

        // OpenHardwareOpusDecoder(48000, 2), DecodeInterleavedOld
        let r = send(mgr, 0, [rate_ch.clone(), 0x1000u32.to_le_bytes().to_vec()].concat(), &[], &mut none);
        assert_eq!((r.result, r.moved.len()), (0, 1));
        let got = decode_ipc(r.moved[0], 0, &packets);
        assert_eq!(got.len(), want.len());
        assert!(got == want, "hwopus PCM differs from libopus at sample {:?}", got.iter().zip(&want).position(|(a, b)| a != b));

        // OpenHardwareOpusDecoderForMultiStream (one coupled stream, mapping 0 1), DecodeInterleaved with reset
        let mut params = [48000u32, 2, 1, 1].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        params.extend_from_slice(&[0, 1]);
        params.resize(0x110, 0);
        let mut m = Builder { ty: T_REQUEST, cmd: 2, data: 0x1000u32.to_le_bytes().to_vec(), x: vec![Buf { addr: params.as_ptr() as u64, size: params.len() as u64, tag: 0 }], ..Default::default() }.build();
        crate::ipc::process(mgr, &mut m);
        let r = hipc::parse_response(&m, false).unwrap();
        assert_eq!((r.result, r.moved.len()), (0, 1));
        let got_ms = decode_ipc(r.moved[0], 8, &packets);
        assert!(got_ms == want, "multi-stream PCM differs from libopus");

        // and it is the tone: after the encoder's delay, each channel against its sine (ffmpeg's amplitude 1/8)
        let (mut sig, mut err) = (0f64, 0f64);
        for (ch, f) in [(0usize, 440f64), (1, 660f64)] {
            for k in 4800..96000 - pre_skip {
                let s = (2.0 * std::f64::consts::PI * f * k as f64 / 48000.0).sin() * 32768.0 / 8.0;
                let d = got[(k + pre_skip) * 2 + ch] as f64;
                sig += s * s;
                err += (d - s) * (d - s);
            }
        }
        let snr = 10.0 * (sig / err).log10();
        assert!(snr > 20.0, "decoded tone SNR {snr:.1} dB");
    }
}
