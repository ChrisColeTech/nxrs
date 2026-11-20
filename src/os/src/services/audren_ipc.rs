//! The audio renderer service `audren:u` served by the host, for SDKs that ship no renderer of their own (SDK 1.x:
//! their nn::audio is only the service's client; later SDKs run their own renderer in-process, `audren.rs`).
//! Turbo's design (runtime/audio.c, main.c audren_update), reimplemented: every RequestUpdate parses the game's
//! update (voices, their channel resources, effects, splitters, mixes, sinks), mixes 5 ms blocks of 48 kHz stereo
//! for the sound card and answers each voice's played samples and consumed wave buffers.
//!
//! * Voices: PCM 8 / 16 / 24 / 32 / float and DSP ADPCM, linear resampling by rate x pitch, two biquads, up to
//!   four queued wave buffers (looping, stop and pause), REV15's 0x188-byte voices read as the 0x170 layout.
//! * Routing: a voice goes to its mix through its channel resources' volumes, or through its splitter to several
//!   mixes; mixes go to their destination mix by their matrix, or through a splitter; the final mix (id 0) through
//!   the device sink's inputs (5.1 folded to stereo with the sink's downmix).
//! * Turbo a22b102: since REV7 an update carries only the mixes that changed (behind a 0x20-byte header) and only
//!   the splitters that changed, so mixes and splitters are kept by id between updates.
//! * Turbo 8a265ab: a voice that names no mix and whose splitter reaches no mix in use stays audible in the final
//!   mix's front pair (Ocarina of Time's opening plays on such voices).
//! * Effects: buffer mixer, biquad filter and auxiliary sends (the game's DSP rings in its memory).
//! * Pacing: blocks go to the sound card (`media::sink`) ahead by at most 8 (40 ms); each block the card plays back
//!   signals the renderer's system event, which the client waits on before its next update.

use crate::ipc::{Ctx, Service};
use crate::media::sink::{self, Stream};
use std::sync::{Arc, Mutex};

const NV: usize = 4096;
const NM: usize = 64;
const NC: usize = 24;
/// Frames of one block (5 ms at 48 kHz).
pub const NF: usize = 240;
const NSPL: usize = 512;
const NSD: usize = 4096;
const SPLD: usize = 64;
const MIX: usize = 0x930;
const VOICE: usize = 0x170;
const VOICE2: usize = 0x188;
const RES: usize = 0x70;
const SINK: usize = 0x140;
const EFFECT: usize = 0xc0;
/// Blocks in flight to the sound card.
const AHEAD: usize = 8;
const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn rd32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn rd16(b: &[u8], o: usize) -> i16 {
    b.get(o..o + 2).map_or(0, |s| i16::from_le_bytes(s.try_into().unwrap()))
}
fn rd64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap()))
}
fn rdf(b: &[u8], o: usize) -> f32 {
    let v = f32::from_bits(rd32(b, o));
    if v.is_finite() {
        v
    } else {
        0.0
    }
}
fn gain(b: &[u8], o: usize) -> f32 {
    rdf(b, o).clamp(0.0, 16.0)
}
fn bytes_of(format: u8) -> u64 {
    match format {
        1 => 1,
        2 => 2,
        3 => 3,
        _ => 4,
    }
}

#[derive(Clone, Copy, Default)]
struct Wave {
    addr: u64,
    size: u64,
    start: i32,
    end: i32,
    looped: bool,
    slot: usize,
    ctx: u64,
    ctx_size: u64,
}

#[derive(Clone, Copy, Default)]
struct Adpcm {
    frame: i64,
    h0: i16,
    h1: i16,
    seed0: i16,
    pcm: [i16; 14],
    prev: [i16; 14],
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// One sample through b0 b1 b2 a1 a2 (the renderer's Q14 feedback carries its sign: the a terms are added).
    fn run(&mut self, c: &[f32; 5], x: f32) -> f32 {
        let mut y = c[0] * x + c[1] * self.x1 + c[2] * self.x2 + c[3] * self.y1 + c[4] * self.y2;
        if !y.is_finite() {
            *self = Biquad::default();
            y = 0.0;
        }
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

#[derive(Default)]
struct Voice {
    q: Vec<Wave>,
    consumed: u32,
    played: u64,
    pos: f64,
    fraction: f64,
    carry: Option<(i16, i16)>,
    defer: u32,
    busy: [bool; 4],
    seen: [bool; 4],
    sent: [bool; 4],
    sig: [u64; 4],
    accepted: [u64; 4],
    was_playing: bool,
    unrouted_logged: bool,
    ad: Adpcm,
    filter: [[Biquad; 6]; 2],
}

impl Voice {
    fn ad_reset(&mut self, restart: bool) {
        self.ad.frame = self.q.first().map_or(-1, |w| w.start as i64 / 14 - 1);
        let (h0, h1) = match self.carry {
            Some(c) if !restart => c,
            _ => (0, 0),
        };
        self.ad = Adpcm { frame: self.ad.frame, h0, h1, ..Default::default() };
        if let Some(w) = self.q.first() {
            if w.ctx_size >= 6 && crate::guest::readable(w.ctx, 6) {
                let p = crate::guest::bytes(w.ctx, 6);
                self.ad.h0 = rd16(p, 2);
                self.ad.h1 = rd16(p, 4);
            }
        }
        self.ad.seed0 = self.ad.h0;
    }
    fn pop_wave(&mut self) {
        if self.q.is_empty() {
            return;
        }
        let w = self.q.remove(0);
        self.busy[w.slot] = false;
        self.consumed += 1;
        self.ad_reset(false);
    }
    /// DSP ADPCM sample `n` of `w` (frames of 8 bytes: a predictor / scale byte and 14 nibbles).
    fn ad_sample(&mut self, w: &Wave, n: i64, co: &[i16; 16]) -> f32 {
        let target = n / 14;
        if target == self.ad.frame - 1 && self.ad.frame >= 1 {
            return self.ad.prev[(n % 14) as usize] as f32 / 32768.0;
        }
        if target < self.ad.frame {
            self.ad_reset(true);
        }
        while self.ad.frame < target {
            let f = self.ad.frame + 1;
            if f as u64 >= w.size / 8 {
                return 0.0;
            }
            let p = crate::guest::bytes(w.addr + f as u64 * 8, 8);
            let (pred, shift) = ((p[0] >> 4) as usize, p[0] & 15);
            if pred >= 8 {
                return 0.0;
            }
            let a = &mut self.ad;
            a.prev = a.pcm;
            let first = if w.start as i64 > f * 14 { (w.start as i64 - f * 14) as usize } else { 0 };
            for i in first..14 {
                let mut nib = ((p[1 + i / 2] >> if i % 2 == 1 { 0 } else { 4 }) & 15) as i64;
                if nib >= 8 {
                    nib -= 16;
                }
                let x = (nib * (1i64 << shift) * 2048 + co[pred * 2] as i64 * a.h0 as i64 + co[pred * 2 + 1] as i64 * a.h1 as i64 + 1024) >> 11;
                let x = x.clamp(-32768, 32767) as i16;
                a.h1 = a.h0;
                a.h0 = x;
                a.pcm[i] = x;
            }
            a.frame = f;
        }
        self.ad.pcm[(n % 14) as usize] as f32 / 32768.0
    }
    /// Past the end of the head wave: loop it, or pop it (keeping the ADPCM history for the next one).
    fn normalize(&mut self, format: u8, co: &[i16; 16]) {
        while let Some(&w) = self.q.first() {
            let len = (w.end - w.start) as f64;
            if self.pos + 1e-8 < len {
                return;
            }
            if self.pos < len {
                self.pos = len;
            }
            if w.looped {
                self.pos %= len;
                self.ad_reset(true);
                return;
            }
            if format == 6 {
                let last1 = if w.end - w.start > 1 { (self.ad_sample(&w, w.end as i64 - 2, co) * 32768.0).round() as i16 } else { self.ad.seed0 };
                let last0 = (self.ad_sample(&w, w.end as i64 - 1, co) * 32768.0).round() as i16;
                self.carry = Some((last0, last1));
            }
            self.pos -= len;
            self.pop_wave();
        }
    }
    fn sample(&mut self, w: &Wave, n: i64, ch: u32, channels: u32, format: u8, co: &[i16; 16]) -> f32 {
        let n = n.clamp(w.start as i64, w.end as i64 - 1);
        if format == 6 {
            return if channels == 1 { self.ad_sample(w, n, co) } else { 0.0 };
        }
        let b = bytes_of(format);
        let index = n as u64 * channels as u64 + ch as u64;
        if index >= w.size / b {
            return 0.0;
        }
        let p = crate::guest::bytes(w.addr + index * b, b);
        match format {
            1 => p[0] as i8 as f32 / 128.0,
            2 => rd16(p, 0) as f32 / 32768.0,
            3 => {
                let mut x = (p[0] as i32) | (p[1] as i32) << 8 | (p[2] as i32) << 16;
                if x & 0x80_0000 != 0 {
                    x -= 0x100_0000;
                }
                x as f32 / 8_388_608.0
            }
            4 => rd32(p, 0) as i32 as f32 / 2_147_483_648.0,
            5 => rdf(p, 0),
            _ => 0.0,
        }
    }
    /// The game's wave buffers (from the head index on) the voice has not taken yet.
    fn queue_waves(&mut self, vi: &[u8], channels: u32, format: u8, serial: u64) {
        let head = (rd16(vi, 0x40) as i32).clamp(0, 3) as usize;
        for j in 0..4 {
            let k = (head + j) % 4;
            let p = &vi[0x60 + k * 0x38..0x60 + (k + 1) * 0x38];
            let (addr, size, start, end, sent) = (rd64(p, 0), rd64(p, 8), rd32(p, 0x10) as i32, rd32(p, 0x14) as i32, p[0x1a] != 0);
            let sig = addr ^ size.wrapping_mul(31) ^ ((start as u32 as u64) << 32) ^ end as u32 as u64;
            let fresh = !self.seen[k] || self.accepted[k] != serial || self.sent[k] || self.sig[k] != sig;
            if addr != 0 && size != 0 && !sent && !self.busy[k] && fresh && self.q.len() < 4 {
                let capacity = if format == 6 { (size / 8) * 14 } else { size / (channels as u64 * bytes_of(format)) };
                if start >= 0 && end > start && end as u64 <= capacity && size <= 128 << 20 && crate::guest::readable(addr, size) {
                    self.q.push(Wave { addr, size, start, end, looped: p[0x18] != 0, slot: k, ctx: rd64(p, 0x20), ctx_size: rd64(p, 0x28) });
                    self.busy[k] = true;
                    self.accepted[k] = serial;
                    if self.q.len() == 1 {
                        self.ad_reset(false);
                    }
                }
            }
            self.seen[k] = true;
            self.sent[k] = sent;
            self.sig[k] = sig;
        }
    }
}

struct Effect {
    config: [u8; EFFECT],
    filter: [Biquad; 6],
    aux_logged: bool,
}

#[derive(Clone, Copy)]
struct SplitDest {
    in_use: bool,
    mix: i32,
    vol: [f32; NC],
}

/// One renderer's mixing state, kept between updates.
pub struct Mixer {
    voices: Vec<Option<Box<Voice>>>,
    mix: Vec<f32>,
    mixtab: Vec<Option<Box<[u8]>>>,
    nmix: usize,
    split: Vec<Vec<i32>>,
    sdst: Vec<SplitDest>,
    sdd_size: usize,
    effects: Vec<Effect>,
    order: Vec<usize>,
    update_blocks: u32,
    serial: u64,
    route_logged: bool,
    logged: bool,
    unrouted_said: u32,
}

impl Default for Mixer {
    fn default() -> Self {
        Mixer {
            voices: (0..NV).map(|_| None).collect(),
            mix: vec![],
            mixtab: (0..NM).map(|_| None).collect(),
            nmix: 0,
            split: vec![vec![]; NSPL],
            sdst: vec![SplitDest { in_use: false, mix: -1, vol: [0.0; NC] }; NSD],
            sdd_size: 0,
            effects: vec![],
            order: vec![],
            update_blocks: 0,
            serial: 0,
            route_logged: false,
            logged: false,
            unrouted_said: 0,
        }
    }
}

/// A REV15 (Switch 2) voice: float biquads and moved wave-buffer fields, read into the 0x170 layout.
fn voice_v2_to_legacy(s: &[u8]) -> [u8; VOICE] {
    let mut d = [0u8; VOICE];
    d[..0x24].copy_from_slice(&s[..0x24]);
    for f in 0..2 {
        let (sf, df) = (0x24 + f * 0x18, 0x24 + f * 0x0c);
        d[df] = s[sf];
        let q = |x: f32| (if x.is_finite() { x * 16384.0 } else { 0.0 }).clamp(-32768.0, 32767.0).round() as i16;
        for j in 0..3 {
            d[df + 2 + j * 2..df + 4 + j * 2].copy_from_slice(&q(rdf(s, sf + 4 + j * 4)).to_le_bytes());
        }
        for j in 0..2 {
            d[df + 8 + j * 2..df + 10 + j * 2].copy_from_slice(&q(rdf(s, sf + 0x10 + j * 4)).to_le_bytes());
        }
    }
    d[0x3c..0x40].copy_from_slice(&s[0x54..0x58]);
    d[0x40..0x42].copy_from_slice(&((rd32(s, 0x58) & 3) as u16).to_le_bytes());
    d[0x48..0x60].copy_from_slice(&s[0x60..0x78]);
    d[0x60..0x140].copy_from_slice(&s[0x78..0x158]);
    d[0x140..0x158].copy_from_slice(&s[0x158..0x170]);
    d[0x158] = s[0x170];
    d[0x159] = s[0x171];
    d[0x15c] = s[0x174];
    d[0x15e] = s[0x176];
    d
}

impl Mixer {
    pub fn begin_update(&mut self, blocks: u32) {
        self.update_blocks = blocks;
        self.serial += 1;
    }

    pub fn configure_effects(&mut self, data: &[u8], count: usize, stride: usize) {
        if count > 4096 || (count > 0 && stride < EFFECT) || data.len() < count * stride {
            return;
        }
        if self.effects.len() != count {
            self.effects = (0..count).map(|_| Effect { config: [0; EFFECT], filter: [Biquad::default(); 6], aux_logged: false }).collect();
        }
        for (i, e) in self.effects.iter_mut().enumerate() {
            let p = &data[i * stride..i * stride + EFFECT];
            if p[1] != 0 || p[0] != e.config[0] || p[2] != e.config[2] {
                e.filter = [Biquad::default(); 6];
            }
            e.config.copy_from_slice(p);
        }
        self.order = (0..count).collect();
        let effects = &self.effects;
        self.order.sort_by_key(|&i| (rd32(&effects[i].config, 0x18), i));
    }

    /// The update's splitter section and mix section into the kept tables (Turbo a22b102).
    pub fn update_routing(&mut self, spl: &[u8], mixes: &[u8]) {
        const SNDH: u32 = 0x4844_4E53;
        const SNDI: u32 = 0x4944_4E53;
        const SNDD: u32 = 0x4444_4E53;
        let (mut ninfo, mut ndest) = (0, 0);
        if spl.len() >= 0x20 && rd32(spl, 0) == SNDH {
            let mut off = 0x20;
            while off + 0x10 <= spl.len() {
                match rd32(spl, off) {
                    SNDI => {
                        let (id, n) = (rd32(spl, off + 4) as usize, rd32(spl, off + 12) as usize);
                        if n > 4096 || off + 0x10 + 4 * n > spl.len() {
                            break;
                        }
                        if id < NSPL {
                            self.split[id] = (0..n.min(SPLD)).map(|k| rd32(spl, off + 0x10 + 4 * k) as i32).collect();
                        }
                        ninfo += 1;
                        off += 0x10 + 4 * n;
                    }
                    SNDD => {
                        if off + 0x70 > spl.len() {
                            break;
                        }
                        let mut next = off + 0x70;
                        while next + 4 <= spl.len() && !matches!(rd32(spl, next), SNDI | SNDD) {
                            next += 4;
                        }
                        // 0x70 bytes with the in-use flag at 0x6c; newer records carry biquads before the flag and
                        // end 12 bytes after it: the size is learned from the first record with one behind it
                        if next + 4 <= spl.len() && self.sdd_size == 0 {
                            self.sdd_size = next - off;
                            crate::log!("audren:u: splitter destinations of {:#x} bytes", self.sdd_size);
                        }
                        let mut size = if self.sdd_size != 0 { self.sdd_size } else { 0x70 };
                        if off + size > spl.len() {
                            size = 0x70;
                        }
                        let id = rd32(spl, off + 4) as usize;
                        if id < NSD {
                            let d = &mut self.sdst[id];
                            d.mix = rd32(spl, off + 0x68) as i32;
                            d.in_use = spl[off + if size <= 0x70 { 0x6c } else { size - 12 }] != 0;
                            for k in 0..NC {
                                d.vol[k] = rdf(spl, off + 8 + k * 4);
                            }
                        }
                        ndest += 1;
                        off = next;
                    }
                    _ => off += 4,
                }
            }
        }
        // the whole list, or (0x20 bytes over a whole number of mixes) only the mixes that changed
        let dirty = mixes.len() >= 0x20 && mixes.len() % MIX == 0x20;
        let (list, n) = if dirty {
            (&mixes[0x20..], (rd32(mixes, 4) as usize).min((mixes.len() - 0x20) / MIX))
        } else if !mixes.is_empty() && mixes.len() % MIX == 0 {
            for m in self.mixtab.iter_mut() {
                *m = None;
            }
            (mixes, mixes.len() / MIX)
        } else {
            (mixes, 0)
        };
        for i in 0..n {
            let m = &list[i * MIX..(i + 1) * MIX];
            let id = rd32(m, 0x10) as usize;
            if id >= NM {
                continue;
            }
            if m[0xc] != 0 {
                self.mixtab[id] = Some(m.to_vec().into_boxed_slice());
            } else if dirty {
                self.mixtab[id] = None;
            }
        }
        self.nmix = self.mixtab.iter().rposition(|m| m.is_some()).map_or(0, |i| i + 1);
        if !self.route_logged && (n > 0 || ninfo > 0 || ndest > 0) {
            self.route_logged = true;
            crate::log!(
                "audren:u: mix section {} bytes ({}, {n} in this update), splitter section {} bytes ({ninfo} splitters, {ndest} destinations)",
                mixes.len(),
                if dirty { "changed mixes only" } else { "every mix" },
                spl.len()
            );
        }
    }

    /// The effects of mix `index` (id `id`) on its buffers, in the effects' order.
    fn mix_effects(&mut self, index: usize, id: u32, buffers: usize) {
        let base = index * NC * NF;
        for &n in &self.order {
            let e = &mut self.effects[n];
            if rd32(&e.config, 4) != id {
                continue;
            }
            let (ty, enabled) = (e.config[0], e.config[2] != 0);
            let v = &e.config[0x20..];
            let mix = &mut self.mix[base..base + NC * NF];
            match ty {
                // buffer mixer: input[24], output[24], volume[24], count
                1 if enabled => {
                    let count = rd32(v, 0x90) as usize;
                    if count > NC {
                        continue;
                    }
                    for k in 0..count {
                        let (src, dst, vol) = (v[k] as usize, v[24 + k] as usize, rdf(v, 48 + k * 4));
                        if src >= buffers || dst >= buffers {
                            continue;
                        }
                        for f in 0..NF {
                            mix[dst * NF + f] += mix[src * NF + f] * vol;
                        }
                    }
                }
                2 => aux(e, mix, buffers),
                // biquad filter: input[6], output[6], b0 b1 b2 a1 a2 (Q14), channel count
                6 => {
                    let count = v[0x16] as usize;
                    if count == 0 || count > 6 {
                        continue;
                    }
                    let mut c = [0f32; 5];
                    for (k, x) in c.iter_mut().enumerate() {
                        *x = rd16(v, 12 + k * 2) as f32 / 16384.0;
                    }
                    let mut input = vec![[0f32; NF]; count];
                    for (k, inp) in input.iter_mut().enumerate() {
                        if (v[k] as usize) < buffers {
                            inp.copy_from_slice(&mix[v[k] as usize * NF..v[k] as usize * NF + NF]);
                        }
                    }
                    let filters = &mut e.filter;
                    for (k, inp) in input.iter().enumerate() {
                        let o = v[6 + k] as usize;
                        if o < buffers {
                            for f in 0..NF {
                                mix[o * NF + f] = if enabled { filters[k].run(&c, inp[f]) } else { inp[f] };
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// One block: the voices into their mixes, the mixes down the graph, the final mix through the sink to
    /// `stereo` (NF interleaved frames). `status` gets each voice's played samples (u64) and consumed wave buffers
    /// (u32), 0x10 bytes a voice. `running`: the renderer was started (a stopped one keeps its voices where they
    /// are). Returns the voices that played.
    #[allow(clippy::too_many_arguments)]
    pub fn mix(&mut self, voices: &[u8], count: usize, stride: usize, resources: &[u8], sinks: &[u8], status: &mut [u8], stereo: &mut [f32], running: bool) -> usize {
        stereo[..NF * 2].fill(0.0);
        if count > NV || (stride < VOICE && stride != VOICE2) || voices.len() < count * stride {
            return 0;
        }
        let nm = self.nmix;
        let mut map = [-1i32; NM];
        let mut nbuf = [0usize; NM];
        for i in 0..nm {
            if let Some(m) = &self.mixtab[i] {
                let nb = rd32(m, 8) as usize;
                if nb > 0 && nb <= NC {
                    map[i] = i as i32;
                    nbuf[i] = nb;
                }
            }
        }
        if nm > 0 {
            self.mix.clear();
            self.mix.resize(nm * NC * NF, 0.0);
        }
        let mut active = 0;
        for i in 0..count {
            let legacy;
            let vi: &[u8] = if stride == VOICE2 {
                legacy = voice_v2_to_legacy(&voices[i * stride..(i + 1) * stride]);
                &legacy
            } else {
                &voices[i * stride..i * stride + VOICE]
            };
            let id = rd32(vi, 0) as usize;
            if id >= NV {
                continue;
            }
            if vi[8] != 0 || self.voices[id].is_none() {
                self.voices[id] = Some(Box::default());
            }
            let mut v = self.voices[id].take().unwrap();
            let channels = match rd32(vi, 0x18) {
                c @ 1..=6 => c,
                _ => 1,
            };
            let format = vi[0xb];
            let state = vi[0xa];
            if vi[9] != 0 {
                let (was_empty, was_stopped) = (v.q.is_empty(), !v.was_playing);
                v.queue_waves(vi, channels, format, self.serial);
                // catch-up blocks are time from before this update: a sound starting in it waits them out
                if !v.q.is_empty() && state == 0 && (vi[8] != 0 || was_empty || was_stopped) {
                    v.defer = self.update_blocks.saturating_sub(1);
                }
                if state == 1 && v.was_playing {
                    v.q.clear();
                    v.busy = [false; 4];
                    v.pos = 0.0;
                    v.played = 0;
                    v.fraction = 0.0;
                    v.carry = None;
                    v.ad_reset(true);
                    v.filter = Default::default();
                }
                v.was_playing = state != 1;
                let deferred = state == 0 && !v.q.is_empty() && running && v.defer > 0;
                if deferred {
                    v.defer -= 1;
                    active += 1;
                }
                if state == 0 && !v.q.is_empty() && running && !deferred {
                    active += 1;
                    self.play_voice(&mut v, vi, id, channels, format, &map, &nbuf, resources, stereo);
                }
            } else {
                v.q.clear();
                v.pos = 0.0;
                v.carry = None;
                v.filter = Default::default();
                v.busy = [false; 4];
            }
            if let Some(s) = status.get_mut(i * 16..i * 16 + 12) {
                s[..8].copy_from_slice(&v.played.to_le_bytes());
                s[8..12].copy_from_slice(&v.consumed.to_le_bytes());
            }
            self.voices[id] = Some(v);
        }
        self.route_mixes(&map, &nbuf, running);
        let fm = map[0];
        if fm >= 0 {
            let fm = fm as usize;
            let mut downmix = [1.0, SQRT_HALF, 0.5, SQRT_HALF];
            let mut inputs = [0usize, 1, 2, 3, 4, 5];
            let mut ninput = nbuf[fm].min(6);
            for k in 0..sinks.len() / SINK {
                let p = &sinks[k * SINK..(k + 1) * SINK];
                let n = rd32(p, 0x120) as usize;
                if p[0] == 1 && p[1] != 0 && (1..=6).contains(&n) {
                    ninput = n;
                    for j in 0..n {
                        inputs[j] = p[0x124 + j] as usize;
                    }
                    if p[0x12b] != 0 {
                        for (j, d) in downmix.iter_mut().enumerate() {
                            *d = rdf(p, 0x12c + j * 4);
                        }
                    }
                    break;
                }
            }
            for x in inputs.iter_mut().take(ninput) {
                if *x >= nbuf[fm] {
                    *x = 0;
                }
            }
            let volume = gain(self.mixtab[fm].as_deref().unwrap(), 0);
            for f in 0..NF {
                let mut ch = [0f32; 6];
                for j in 0..ninput {
                    ch[j] = self.mix[(fm * NC + inputs[j]) * NF + f];
                }
                let (mut l, mut r) = (ch[0], if ninput > 1 { ch[1] } else { ch[0] });
                // a 5.1 sink: FL FR FC LFE BL BR folded to the PC's stereo, centre and rears kept
                if ninput == 6 {
                    l = downmix[0] * ch[0] + downmix[1] * ch[2] + downmix[2] * ch[3] + downmix[3] * ch[4];
                    r = downmix[0] * ch[1] + downmix[1] * ch[2] + downmix[2] * ch[3] + downmix[3] * ch[5];
                }
                stereo[f * 2] += l * volume;
                stereo[f * 2 + 1] += r * volume;
            }
        }
        if active > 0 && !self.logged {
            self.logged = true;
            crate::log!("audren:u: host mixer active ({active} voices, {nm} mixes, {})", if nm > 0 { "mix routing" } else { "straight to stereo" });
        }
        self.update_blocks = self.update_blocks.saturating_sub(1);
        active
    }

    /// One playing voice's block into its mix, its splitter's mixes, the final mix (unrouted) or `stereo` (no mixes).
    #[allow(clippy::too_many_arguments)]
    fn play_voice(&mut self, v: &mut Voice, vi: &[u8], id: usize, channels: u32, format: u8, map: &[i32; NM], nbuf: &[usize; NM], resources: &[u8], stereo: &mut [f32]) {
        let nm = self.nmix;
        let rate = match rd32(vi, 0xc) {
            r @ 1..=192_000 => r,
            _ => 48000,
        };
        let pitch = match rdf(vi, 0x1c) {
            p if p > 0.0 && p <= 8.0 => p,
            _ => 1.0,
        };
        let step = rate as f64 * pitch as f64 / 48000.0;
        let mut vol = gain(vi, 0x20);
        let mut co = [0i16; 16];
        let (extra, esz) = (rd64(vi, 0x48), rd64(vi, 0x50));
        if format == 6 && esz >= 32 && crate::guest::readable(extra, 32) {
            let b = crate::guest::bytes(extra, 32);
            for (k, c) in co.iter_mut().enumerate() {
                *c = rd16(b, k * 2);
            }
        }
        if !(1..=6).contains(&format) || (format == 6 && channels != 1) {
            crate::log_once!(format!("audren:u fmt {format} {channels}"), "[RED] audren:u: voice format {format} with {channels} channels is not mixed (its time still passes)");
            vol = 0.0;
        }
        let mix_id = rd32(vi, 0x58) as usize;
        let dest = if mix_id < NM { map[mix_id] } else { -1 };
        let mut routing = [[0f32; NC]; 6];
        if dest >= 0 {
            let d = dest as usize;
            for (ch, r) in routing.iter_mut().enumerate().take(channels as usize) {
                let cid = rd32(vi, 0x140 + ch * 4) as usize;
                if cid < resources.len() / RES && resources[cid * RES + 0x64] != 0 {
                    for (k, g) in r.iter_mut().enumerate().take(nbuf[d]) {
                        *g = rdf(resources, cid * RES + 4 + k * 4);
                    }
                } else {
                    r[if ch < nbuf[d] { ch } else { 0 }] = if channels == 1 { SQRT_HALF } else { 1.0 };
                    if channels == 1 && nbuf[d] > 1 {
                        r[1] = SQRT_HALF;
                    }
                }
            }
        }
        let mut coeff = [[0f32; 5]; 2];
        let mut enabled = [false; 2];
        for stage in 0..2 {
            let o = 0x24 + stage * 12;
            enabled[stage] = vi[o] != 0;
            for j in 0..5 {
                coeff[stage][j] = rd16(vi, o + 2 + j * 2) as f32 / 16384.0;
            }
            if !enabled[stage] {
                v.filter[stage] = Default::default();
            }
        }
        // decode and filter once into planar channels, then route whole blocks
        let mut decoded = [[0f32; NF]; 6];
        let mut produced = 0;
        for f in 0..NF {
            v.normalize(format, &co);
            let Some(&w) = v.q.first() else { break };
            let n = w.start as i64 + v.pos as i64;
            let frac = (v.pos - v.pos.floor()) as f32;
            for ch in 0..channels {
                let mut x = v.sample(&w, n, ch, channels, format, &co);
                if frac > 0.0 {
                    x += (v.sample(&w, n + 1, ch, channels, format, &co) - x) * frac;
                }
                for stage in 0..2 {
                    if enabled[stage] {
                        x = v.filter[stage][ch as usize].run(&coeff[stage], x);
                    }
                }
                decoded[ch as usize][f] = x * vol;
            }
            produced += 1;
            v.pos += step;
            v.fraction += step;
            let advance = (v.fraction + 1e-9) as u64;
            v.played += advance;
            v.fraction -= advance as f64;
        }
        v.normalize(format, &co);
        let sid = rd32(vi, 0x5c) as usize;
        let split = if dest < 0 && nm > 0 && sid < NSPL && !self.split[sid].is_empty() { Some(self.split[sid].clone()) } else { None };
        for ch in 0..channels as usize {
            let mut routed = false;
            if dest >= 0 {
                let d = dest as usize;
                for k in 0..nbuf[d] {
                    let g = routing[ch][k];
                    if g != 0.0 {
                        let o = (d * NC + k) * NF;
                        for f in 0..produced {
                            self.mix[o + f] += decoded[ch][f] * g;
                        }
                    }
                }
            } else if let Some(sp) = &split {
                // channel ch of N goes to the splitter's destinations ch, ch + N, ch + 2N...
                for &did in sp.iter().skip(ch).step_by(channels as usize) {
                    let Some(sd) = usize::try_from(did).ok().and_then(|d| self.sdst.get(d)).copied() else { continue };
                    if !sd.in_use || sd.mix < 0 || sd.mix as usize >= NM || map[sd.mix as usize] < 0 {
                        continue;
                    }
                    let dm = map[sd.mix as usize] as usize;
                    routed = true;
                    for k in 0..nbuf[dm] {
                        let g = sd.vol[k];
                        if g != 0.0 {
                            let o = (dm * NC + k) * NF;
                            for f in 0..produced {
                                self.mix[o + f] += decoded[ch][f] * g;
                            }
                        }
                    }
                }
            }
            if dest < 0 && !routed && nm > 0 && map[0] >= 0 {
                // Turbo 8a265ab: the mixes are known and this voice reaches none (no mix named, its splitter has
                // no destination in use): kept audible in the final mix's front pair
                let fm = map[0] as usize;
                if !v.unrouted_logged {
                    v.unrouted_logged = true;
                    if self.unrouted_said < 8 {
                        self.unrouted_said += 1;
                        let listed = if sid < NSPL { self.split[sid].len() } else { 0 };
                        crate::log!("audren:u: voice {id} names no mix and its splitter {} has no destination in use ({listed} listed): played in the final mix", sid as i32);
                    }
                }
                if channels == 1 {
                    for k in 0..2.min(nbuf[fm]) {
                        let o = (fm * NC + k) * NF;
                        for f in 0..produced {
                            self.mix[o + f] += decoded[ch][f] * SQRT_HALF;
                        }
                    }
                } else {
                    let o = (fm * NC + if ch < nbuf[fm] { ch } else { 0 }) * NF;
                    for f in 0..produced {
                        self.mix[o + f] += decoded[ch][f];
                    }
                }
            }
            if dest < 0 && split.is_none() && nm == 0 {
                // no mixes at all: straight to stereo
                if channels == 1 {
                    for f in 0..produced {
                        let x = decoded[ch][f] * SQRT_HALF;
                        stereo[f * 2] += x;
                        stereo[f * 2 + 1] += x;
                    }
                } else {
                    let side = (ch == 1 || ch == 5) as usize;
                    for f in 0..produced {
                        stereo[f * 2 + side] += decoded[ch][f];
                    }
                }
            }
        }
    }

    /// Every mix into its destination (a mix, or a splitter's mixes), feeders first.
    fn route_mixes(&mut self, map: &[i32; NM], nbuf: &[usize; NM], running: bool) {
        let nm = self.nmix;
        let mut incoming = [0i32; NM];
        let mut done = [false; NM];
        let target = |m: &[u8]| (rd32(m, 0x924) as usize, rd32(m, 0x928) as usize);
        for i in 0..nm {
            let Some(m) = self.mixtab[i].as_deref().filter(|_| map[i] >= 0) else { continue };
            let (id, sid) = target(m);
            if id < NM && map[id] >= 0 {
                if map[id] != i as i32 {
                    incoming[map[id] as usize] += 1;
                }
            } else if sid < NSPL {
                for &did in &self.split[sid] {
                    let Some(d) = usize::try_from(did).ok().and_then(|d| self.sdst.get(d)) else { continue };
                    if d.in_use && d.mix >= 0 && (d.mix as usize) < NM && map[d.mix as usize] >= 0 && map[d.mix as usize] != i as i32 {
                        incoming[map[d.mix as usize] as usize] += 1;
                    }
                }
            }
        }
        for _ in 0..nm {
            for i in 0..nm {
                if map[i] < 0 || done[i] || incoming[i] > 0 {
                    continue;
                }
                done[i] = true;
                let m = self.mixtab[i].clone().unwrap();
                if running {
                    self.mix_effects(i, rd32(&m, 0x10), nbuf[i]);
                }
                let (id, sid) = target(&m);
                let dst = if id < NM { map[id] } else { -1 };
                let volume = gain(&m, 0);
                if dst >= 0 && dst != i as i32 {
                    let dst = dst as usize;
                    for sc in 0..nbuf[i] {
                        for dc in 0..nbuf[dst] {
                            let g = rdf(&m, 0x24 + (sc * NC + dc) * 4) * volume;
                            if g != 0.0 {
                                for f in 0..NF {
                                    self.mix[(dst * NC + dc) * NF + f] += self.mix[(i * NC + sc) * NF + f] * g;
                                }
                            }
                        }
                    }
                    incoming[dst] -= 1;
                } else if dst < 0 && sid < NSPL {
                    // destination j of the splitter takes the mix's buffer j % buffers with its own volumes
                    for (j, &did) in self.split[sid].clone().iter().enumerate() {
                        let Some(d) = usize::try_from(did).ok().and_then(|d| self.sdst.get(d)).copied() else { continue };
                        if !d.in_use || d.mix < 0 || d.mix as usize >= NM || map[d.mix as usize] < 0 || map[d.mix as usize] == i as i32 {
                            continue;
                        }
                        let dm = map[d.mix as usize] as usize;
                        let sc = j % nbuf[i];
                        for dc in 0..nbuf[dm] {
                            let g = d.vol[dc] * volume;
                            if g != 0.0 {
                                for f in 0..NF {
                                    self.mix[(dm * NC + dc) * NF + f] += self.mix[(i * NC + sc) * NF + f] * g;
                                }
                            }
                        }
                        incoming[dm] -= 1;
                    }
                }
            }
        }
    }
}

/// An auxiliary send: the mix's buffers out to the game's send ring and its return ring back in (two 0x40-byte
/// headers, then i32 PCM16-amplitude samples in planar blocks of NF a channel).
fn aux(e: &mut Effect, mix: &mut [f32], buffers: usize) {
    let v = &e.config[0x20..];
    let (channels, capacity) = (rd32(v, 0x30) as usize, rd32(v, 0x38) as usize);
    if channels == 0 || channels > NC {
        return;
    }
    let mut input = vec![[0f32; NF]; channels];
    for (c, inp) in input.iter_mut().enumerate() {
        if (v[c] as usize) < buffers {
            inp.copy_from_slice(&mix[v[c] as usize * NF..v[c] as usize * NF + NF]);
        }
    }
    let (send, ret) = (rd64(v, 0x40), rd64(v, 0x50));
    let ring = 0x80 + capacity as u64 * 4;
    let valid = capacity >= channels * NF && capacity <= 1 << 24 && crate::guest::readable(send, ring) && crate::guest::readable(ret, ring);
    if e.config[2] == 0 {
        if valid {
            crate::guest::zero(send + 0x40, 12);
            crate::guest::zero(ret + 0x40, 12);
        }
        for (c, inp) in input.iter().enumerate() {
            let o = v[24 + c] as usize;
            if o < buffers {
                mix[o * NF..o * NF + NF].copy_from_slice(inp);
            }
        }
        return;
    }
    if !valid {
        if !e.aux_logged {
            e.aux_logged = true;
            crate::log!("[RED] audren:u: an auxiliary send's rings are not in memory ({channels} channels, {capacity} samples): the dry signal kept");
        }
        return;
    }
    let (write, read) = (crate::guest::r32(send + 0x44) as usize, crate::guest::r32(ret + 0x40) as usize);
    if write >= capacity || read >= capacity {
        return;
    }
    for (c, inp) in input.iter().enumerate() {
        for (f, &x) in inp.iter().enumerate() {
            let off = c * NF + f;
            let (wi, ri) = ((write + off) % capacity, (read + off) % capacity);
            let s = (x as f64 * 32768.0).clamp(i32::MIN as f64, i32::MAX as f64) as i32;
            crate::guest::w32(send + 0x80 + wi as u64 * 4, s as u32);
            let o = v[24 + c] as usize;
            if o < buffers {
                mix[o * NF + f] = crate::guest::r32(ret + 0x80 + ri as u64 * 4) as i32 as f32 / 32768.0;
            }
        }
    }
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    crate::guest::w32(send + 0x44, ((write + channels * NF) % capacity) as u32);
    crate::guest::w32(ret + 0x40, ((read + channels * NF) % capacity) as u32);
    if !e.aux_logged {
        e.aux_logged = true;
        crate::log!("audren:u: auxiliary send active ({channels} channels, {capacity} samples)");
    }
}

// ---- the service

/// AudioRendererParameter as audren:u takes it: rate, samples, mix buffers, submixes, voices, sinks, effects,
/// performance frames, voice drop, splitters, splitter destinations, external context, revision (at 0x30).
#[derive(Clone, Copy, Default, Debug)]
pub struct Params {
    pub rate: u32,
    pub samples: u32,
    pub mix_buffers: u32,
    pub voices: u32,
    pub sinks: u32,
    pub effects: u32,
    pub revision: u32,
}

impl Params {
    pub fn read(b: &[u8]) -> Params {
        Params { rate: rd32(b, 0), samples: rd32(b, 4), mix_buffers: rd32(b, 8), voices: rd32(b, 0x10), sinks: rd32(b, 0x14), effects: rd32(b, 0x18), revision: rd32(b, 0x30) }
    }
}

pub struct Manager;

impl Service for Manager {
    fn name(&self) -> String {
        "audren:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // OpenAudioRenderer(parameter, work size, aruid; transfer memory, process)
            0 | 4 => {
                let raw = ctx.input().get(..0x34).map(|s| s.to_vec()).unwrap_or_default();
                let p = Params::read(&raw);
                crate::log!(
                    "audren:u: OpenAudioRenderer rate {} samples {} mix buffers {} voices {} sinks {} effects {} revision REV{} (the host mixes)",
                    p.rate,
                    p.samples,
                    p.mix_buffers,
                    p.voices,
                    p.sinks,
                    p.effects,
                    (p.revision >> 24).wrapping_sub(0x30)
                );
                ctx.object(Renderer::new(p));
                0
            }
            // GetWorkBufferSize(parameter)
            1 => {
                let raw = ctx.input().get(..0x34).map(|s| s.to_vec()).unwrap_or_default();
                let mut p = raw.clone();
                p.resize(0x40, 0);
                let n = crate::services::audren::service_work_size(&p, rd32(&raw, 0x30));
                ctx.push_u64(if n != 0 { n } else { 0x10_0000 });
                0
            }
            // GetAudioDeviceService(aruid) / WithRevisionInfo
            2 | 3 => {
                ctx.object(Device { event: None });
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

/// The renderer's state shared with the sound card's release hook.
struct Shared {
    event: Option<crate::kobj::Event>,
}

pub struct Renderer {
    p: Params,
    mixer: Mixer,
    started: bool,
    stream: Option<Arc<Stream>>,
    shared: Arc<Mutex<Shared>>,
    frames: u64,
    tag: u64,
    limit: u32,
}

impl Renderer {
    pub fn new(p: Params) -> Renderer {
        Renderer { p, mixer: Mixer::default(), started: false, stream: None, shared: Arc::new(Mutex::new(Shared { event: None })), frames: 0, tag: 0, limit: 100 }
    }

    fn stream(&mut self) -> &Arc<Stream> {
        if self.stream.is_none() {
            let s = sink::open("audren:u");
            let sh = self.shared.clone();
            s.on_release(move || {
                if let Some(e) = sh.lock().unwrap().event {
                    e.signal();
                }
            });
            self.stream = Some(s);
        }
        self.stream.as_ref().unwrap()
    }

    /// RequestUpdate: the game's update in `input`, the answer into `out` (its size); `blocks` 5 ms blocks mixed
    /// (`None`: as many as the sound card has room for). The mixed blocks are returned (interleaved stereo).
    pub fn update(&mut self, input: &[u8], out: &mut Vec<u8>, blocks: Option<usize>) -> Result<Vec<f32>, u32> {
        const R_BAD: u32 = 153 | 4 << 9; // nn::audio, 4
        if input.len() < 0x40 {
            return Err(R_BAD);
        }
        let ih: Vec<u32> = (0..16).map(|k| rd32(input, k * 4)).collect();
        let sections: u64 = ih[1..=10].iter().map(|&x| x as u64).sum();
        if sections > input.len() as u64 - 0x40 {
            return Err(R_BAD);
        }
        let rev = ih[0];
        let revn = ((rev >> 24) & 0xff).saturating_sub(b'0' as u32);
        let (behav, mp, vo, vr, ef) = (ih[1] as usize, ih[2] as usize, ih[3] as usize, ih[4] as usize, ih[5] as usize);
        let (mut nvoices, mut neff, mut nsinks) = (vo / VOICE, ef / EFFECT, ih[7] as usize / SINK);
        let npools = mp / 0x20;
        let (mut vstride, mut estride) = (VOICE, EFFECT);
        // the counts the renderer was opened with; the input strides from size / count (newer SDKs grew them)
        let (pv, ps, pe) = (self.p.voices as usize, self.p.sinks as usize, self.p.effects as usize);
        if pv > 0 && pv <= 4096 && vo > 0 && vo % pv == 0 && vo / pv >= VOICE {
            nvoices = pv;
            vstride = vo / pv;
        }
        if pe > 0 && pe <= 4096 && ef > 0 && ef % pe == 0 && ef / pe >= EFFECT {
            neff = pe;
            estride = ef / pe;
        }
        if ps <= 64 && ih[7] as usize % ps.max(1) == 0 {
            nsinks = ps;
        }
        if nvoices > 4096 || npools > 4096 || neff > 4096 || nsinks > 64 {
            return Err(R_BAD);
        }
        let mut at = 0x40 + behav;
        let mut take = |n: usize| {
            let s = &input[at.min(input.len())..(at + n).min(input.len())];
            at += n;
            s
        };
        let pools = take(mp);
        let resources = take(vr);
        let voices = take(vo);
        let effects = take(ef);
        let mut spl_size = ih[9] as usize;
        let after_effects = at;
        if spl_size >= 4 && rd32(input, after_effects) != 0x4844_4E53 {
            crate::log_once!("audren:u no splitter", "audren:u: header word 9 is {spl_size} bytes but no splitter header follows the effects: read as none");
            spl_size = 0;
        }
        let mut take = |n: usize| {
            let s = &input[at.min(input.len())..(at + n).min(input.len())];
            at += n;
            s
        };
        let splitters = take(spl_size);
        let mixes = take(ih[6] as usize);
        let sinks = take(ih[7] as usize);
        let eff_v2 = revn >= 9;
        let (o_mp, o_vo, o_ef, o_sk, o_pf, o_bh, o_ri) = (npools * 0x10, nvoices * 0x10, neff * if eff_v2 { 0x90 } else { 0x10 }, nsinks * 0x20, 0x10, 0xb0, if revn >= 5 { 0x10 } else { 0 });
        let total = 0x40 + o_mp + o_vo + o_ef + o_sk + o_pf + o_bh + o_ri;
        out.clear();
        out.resize(total, 0);
        let oh = [rev, o_bh as u32, o_mp as u32, o_vo as u32, 0, o_ef as u32, 0, o_sk as u32, o_pf as u32, 0, o_ri as u32, 0, 0, 0, 0, total as u32];
        for (k, w) in oh.iter().enumerate() {
            out[k * 4..k * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        // memory pools: RequestAttach (4) -> Attached (5), RequestDetach (2) -> Detached (3)
        for i in 0..npools {
            let s = rd32(pools, i * 0x20 + 0x10);
            let ns = match s {
                4 => 5,
                2 => 3,
                x => x,
            };
            out[0x40 + i * 0x10..0x44 + i * 0x10].copy_from_slice(&ns.to_le_bytes());
        }
        self.mixer.update_routing(splitters, mixes);
        self.mixer.configure_effects(effects, neff, estride);
        let n = match blocks {
            Some(n) => n,
            None if self.started => AHEAD.saturating_sub(self.stream().blocks()),
            None => 1,
        };
        self.mixer.begin_update(n.max(1) as u32);
        let mut pcm = vec![];
        let mut stereo = [0f32; NF * 2];
        let mut cont: Option<Vec<u8>> = None;
        let status_at = 0x40 + o_mp;
        for b in 0..n.max(1) {
            let vin: &[u8] = match (&cont, b) {
                (Some(c), _) => c,
                (None, 0) => voices,
                (None, _) => {
                    // a voice's setup and buffer drops happen once an update, not once a block
                    let mut c = voices.to_vec();
                    for i in 0..nvoices {
                        c[i * vstride + 8] = 0;
                    }
                    cont = Some(c);
                    cont.as_deref().unwrap()
                }
            };
            let running = self.started && n > 0;
            let status = &mut out[status_at..status_at + o_vo];
            self.mixer.mix(vin, nvoices, vstride, resources, sinks, status, &mut stereo, running);
            if running {
                pcm.extend_from_slice(&stereo);
            }
        }
        if !pcm.is_empty() && blocks.is_none() {
            for blk in pcm.chunks(NF * 2) {
                self.tag += 1;
                let tag = self.tag;
                self.stream().append(tag, blk.to_vec());
            }
        }
        // effects: enabled (3) / disabled (4)
        let mut q = status_at + o_vo;
        for i in 0..neff {
            out[q + i * if eff_v2 { 0x90 } else { 0x10 }] = if effects.get(i * estride + 2).copied().unwrap_or(0) != 0 { 3 } else { 4 };
        }
        q += o_ef + o_sk + o_pf + o_bh;
        if o_ri > 0 {
            out[q..q + 8].copy_from_slice(&self.frames.to_le_bytes());
            self.frames += 1;
        }
        Ok(pcm)
    }
}

impl Service for Renderer {
    fn name(&self) -> String {
        "audren:u renderer".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            0 => {
                ctx.push_u32(self.p.rate);
                0
            }
            1 => {
                ctx.push_u32(self.p.samples);
                0
            }
            2 => {
                ctx.push_u32(self.p.mix_buffers);
                0
            }
            // GetState: 0 started, 1 stopped
            3 => {
                ctx.push_u32(!self.started as u32);
                0
            }
            // RequestUpdate (A / B buffers) / RequestUpdateAuto (X / C)
            4 | 10 => {
                let input = ctx.read_in(0);
                let mut out = vec![];
                match self.update(&input, &mut out, None) {
                    Ok(_) => {
                        ctx.write_out(0, &out);
                        0
                    }
                    Err(r) => r,
                }
            }
            5 => {
                self.started = true;
                self.stream().start();
                if let Some(e) = self.shared.lock().unwrap().event {
                    e.signal();
                }
                0
            }
            6 => {
                self.started = false;
                if let Some(s) = &self.stream {
                    s.stop();
                }
                0
            }
            // QuerySystemEvent: signalled as blocks play
            7 => {
                let mut sh = self.shared.lock().unwrap();
                let e = *sh.event.get_or_insert_with(crate::kobj::Event::signalled);
                drop(sh);
                ctx.copy_handle(e.readable);
                0
            }
            8 => {
                self.limit = ctx.u32(0);
                0
            }
            9 => {
                ctx.push_u32(self.limit);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
    fn close(&mut self) {
        if let Some(s) = self.stream.take() {
            s.close();
        }
    }
}

/// IAudioDevice: the outputs a console lists, the active one, its volume and change event.
struct Device {
    event: Option<crate::kobj::Event>,
}

const DEVICES: [&str; 3] = ["AudioTvOutput", "AudioStereoJackOutput", "AudioBuiltInSpeakerOutput"];

impl Service for Device {
    fn name(&self) -> String {
        "audren:u device".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        let name = |s: &str| {
            let mut b = vec![0u8; 0x100];
            b[..s.len()].copy_from_slice(s.as_bytes());
            b
        };
        match ctx.cmd() {
            // ListAudioDeviceName(Auto)
            0 | 6 => {
                let room = ctx.buf_out(0).size as usize / 0x100;
                let n = DEVICES.len().min(room);
                let all: Vec<u8> = DEVICES[..n].iter().flat_map(|d| name(d)).collect();
                ctx.write_out(0, &all);
                ctx.push_u32(n as u32);
                0
            }
            // SetAudioDeviceOutputVolume(Auto)
            1 | 7 => 0,
            // GetAudioDeviceOutputVolume(Auto)
            2 | 8 => {
                ctx.push_u32(1.0f32.to_bits());
                0
            }
            // GetActiveAudioDeviceName(Auto)
            3 | 10 => {
                ctx.write_out(0, &name(DEVICES[0]));
                0
            }
            // QueryAudioDeviceSystemEvent / InputEvent / OutputEvent
            4 | 11 | 12 => {
                let e = *self.event.get_or_insert_with(crate::kobj::Event::new);
                ctx.copy_handle(e.readable);
                0
            }
            // GetActiveChannelCount
            5 => {
                ctx.push_u32(2);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

pub fn register() {
    crate::ipc::register("audren:u", || Box::new(Manager));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w32(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn wf(b: &mut [u8], o: usize, v: f32) {
        w32(b, o, v.to_bits());
    }
    #[track_caller]
    fn near(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "{a} != {b}");
    }

    /// A mono PCM16 voice of `n` samples at `pcm` (one wave buffer), playing, at full volume.
    fn voice(pcm: &[i16]) -> [u8; VOICE] {
        let mut v = [0u8; VOICE];
        v[9] = 1;
        v[0xb] = 2;
        w32(&mut v, 0xc, 48000);
        w32(&mut v, 0x18, 1);
        wf(&mut v, 0x1c, 1.0);
        wf(&mut v, 0x20, 1.0);
        v[0x60..0x68].copy_from_slice(&(pcm.as_ptr() as u64).to_le_bytes());
        v[0x68..0x70].copy_from_slice(&(pcm.len() as u64 * 2).to_le_bytes());
        w32(&mut v, 0x74, pcm.len() as u32);
        v
    }
    /// Mix `id` with 2 buffers, volume 1, going to mix `to` (or none) and splitter `sid` (or none); its matrix
    /// passes buffer k to buffer k.
    fn mix_entry(m: &mut [u8], id: u32, to: u32, sid: u32) {
        wf(m, 0, 1.0);
        w32(m, 8, 2);
        m[0xc] = 1;
        w32(m, 0x10, id);
        w32(m, 0x924, to);
        w32(m, 0x928, sid);
        wf(m, 0x24, 1.0);
        wf(m, 0x24 + (NC + 1) * 4, 1.0);
    }
    /// A splitter section: splitter 0 with destinations `dests` (id, mix, volume of buffer 0, volume of buffer 1).
    fn splitter(dests: &[(u32, u32, f32, f32)]) -> Vec<u8> {
        let mut s = vec![0u8; 0x20 + 0x10 + 4 * dests.len() + 0x70 * dests.len()];
        w32(&mut s, 0, 0x4844_4E53);
        w32(&mut s, 4, 1);
        w32(&mut s, 8, dests.len() as u32);
        let si = 0x20;
        w32(&mut s, si, 0x4944_4E53);
        w32(&mut s, si + 4, 0);
        w32(&mut s, si + 12, dests.len() as u32);
        for (k, d) in dests.iter().enumerate() {
            w32(&mut s, si + 0x10 + 4 * k, d.0);
        }
        let mut o = si + 0x10 + 4 * dests.len();
        for d in dests {
            w32(&mut s, o, 0x4444_4E53);
            w32(&mut s, o + 4, d.0);
            wf(&mut s, o + 8, d.2);
            wf(&mut s, o + 12, d.3);
            w32(&mut s, o + 0x68, d.1);
            s[o + 0x6c] = 1;
            o += 0x70;
        }
        s
    }
    fn run(m: &mut Mixer, v: &[u8]) -> ([f32; NF * 2], [u8; 16]) {
        let (mut out, mut st) = ([0f32; NF * 2], [0u8; 16]);
        m.begin_update(1);
        m.mix(v, 1, VOICE, &[], &[], &mut st, &mut out, true);
        (out, st)
    }

    #[test]
    fn pcm_voice_plays_and_reports_its_samples() {
        let pcm = vec![8192i16; 240];
        let mut m = Mixer::default();
        let (out, st) = run(&mut m, &voice(&pcm));
        near(out[0], 0.25 * SQRT_HALF);
        near(out[1], out[0]);
        assert_eq!(u64::from_le_bytes(st[..8].try_into().unwrap()), 240);
        assert_eq!(u32::from_le_bytes(st[8..12].try_into().unwrap()), 1, "the wave buffer is consumed");
        let mut v = voice(&pcm);
        v[0x60 + 0x1a] = 1; // the client marks the buffer sent
        let (out, st) = run(&mut m, &v);
        near(out[0], 0.0);
        assert_eq!(u32::from_le_bytes(st[8..12].try_into().unwrap()), 1, "the same buffer is not played twice");
    }

    /// Turbo a22b102: an update with only the changed mixes (behind a 0x20-byte header) keeps the others; the next
    /// update with no mix changed keeps them all.
    #[test]
    fn mixes_are_kept_between_updates() {
        let pcm = vec![16384i16; 480];
        let mut v = voice(&pcm);
        w32(&mut v, 0x58, 1); // to mix 1, which goes to the final mix 0
        w32(&mut v, 0x5c, u32::MAX);
        let mut whole = vec![0u8; 2 * MIX];
        mix_entry(&mut whole[..MIX], 0, u32::MAX, u32::MAX);
        mix_entry(&mut whole[MIX..], 1, 0, u32::MAX);
        let mut m = Mixer::default();
        m.update_routing(&[], &whole);
        let (out, _) = run(&mut m, &v);
        near(out[0], 0.5 * SQRT_HALF);
        // the next update: a changed-mixes section with mix 1 at half volume only
        let mut dirty = vec![0u8; 0x20 + MIX];
        w32(&mut dirty, 4, 1);
        mix_entry(&mut dirty[0x20..], 1, 0, u32::MAX);
        wf(&mut dirty, 0x20, 0.5);
        m.update_routing(&[], &dirty);
        v[8] = 0;
        let (out, _) = run(&mut m, &v);
        near(out[0], 0.25 * SQRT_HALF);
        // and one with no mix changed: both still there
        m.update_routing(&[], &[0u8; 0x20]);
        let mut v2 = voice(&pcm);
        w32(&mut v2, 0x58, 1);
        let (out, _) = run(&mut m, &v2);
        near(out[0], 0.25 * SQRT_HALF);
    }

    /// Turbo a22b102: a voice through a splitter reaches each destination's mix with that destination's volumes,
    /// and the splitter is kept when the next update sends none; a mix through a splitter likewise.
    #[test]
    fn splitters_route_voices_and_mixes() {
        let pcm = vec![16384i16; 480];
        let mut v = voice(&pcm);
        w32(&mut v, 0x18, 1);
        w32(&mut v, 0x58, u32::MAX); // no mix: splitter 0
        w32(&mut v, 0x5c, 0);
        let mut mixes = vec![0u8; 0x20 + 2 * MIX];
        w32(&mut mixes, 4, 2);
        mix_entry(&mut mixes[0x20..0x20 + MIX], 0, u32::MAX, u32::MAX);
        mix_entry(&mut mixes[0x20 + MIX..], 1, 0, u32::MAX);
        // destination 0: the final mix, buffer 0 at 0.5; destination 1: mix 1, buffer 1 at 0.25
        m_split(&mut v, &mixes, &splitter(&[(0, 0, 0.5, 0.0), (1, 1, 0.0, 0.25)]));

        fn m_split(v: &mut [u8; VOICE], mixes: &[u8], spl: &[u8]) {
            let mut m = Mixer::default();
            m.update_routing(spl, mixes);
            let (out, _) = run(&mut m, v);
            near(out[0], 0.25); // 0.5 x 0.5 into the final mix's buffer 0
            near(out[1], 0.125); // 0.5 x 0.25 into mix 1's buffer 1, then to the final mix
            m.update_routing(&[], &[0u8; 0x20]);
            v[8] = 0;
            let (out, _) = run(&mut m, v);
            near(out[0], 0.25);
            near(out[1], 0.125);
        }

        // a mix through a splitter: mix 1 names no destination mix, its splitter 0 sends its buffer 0 to the final
        // mix's buffer 0 at 0.5 and its buffer 1 to the final mix's buffer 1 at 0.25
        let mut v = voice(&pcm);
        w32(&mut v, 0x58, 1);
        let mut mixes = vec![0u8; 2 * MIX];
        mix_entry(&mut mixes[..MIX], 0, u32::MAX, u32::MAX);
        mix_entry(&mut mixes[MIX..], 1, u32::MAX, 0);
        let mut m = Mixer::default();
        m.update_routing(&splitter(&[(0, 0, 0.5, 0.0), (1, 0, 0.0, 0.25)]), &mixes);
        let (out, _) = run(&mut m, &v);
        near(out[0], 0.5 * SQRT_HALF * 0.5);
        near(out[1], 0.5 * SQRT_HALF * 0.25);
    }

    /// Turbo 8a265ab: with mixes known, a voice that names no mix and whose splitter has no destination in use is
    /// played in the final mix's front pair instead of going silent.
    #[test]
    fn a_voice_reaching_no_mix_stays_audible() {
        let pcm = vec![16384i16; 480];
        let mut mixes = vec![0u8; MIX];
        mix_entry(&mut mixes, 0, u32::MAX, u32::MAX);
        for spl in [splitter(&[]), {
            let mut s = splitter(&[(0, 0, 1.0, 1.0)]);
            let n = s.len();
            s[n - 0x70 + 0x6c] = 0; // its one destination not in use
            s
        }] {
            let mut v = voice(&pcm);
            w32(&mut v, 0x58, u32::MAX);
            w32(&mut v, 0x5c, 0);
            let mut m = Mixer::default();
            m.update_routing(&spl, &mixes);
            let (out, _) = run(&mut m, &v);
            near(out[0], 0.5 * SQRT_HALF);
            near(out[1], 0.5 * SQRT_HALF);
        }
    }

    /// audren:u over IPC (SDK 1.x's client): GetWorkBufferSize, OpenAudioRenderer, the getters, RequestUpdate with
    /// a playing voice: the answer's header, pool states and the voice's played samples.
    #[test]
    fn audren_u_serves_an_sdk1_client() {
        use crate::ipc::hipc::{self, Buf, Builder, T_REQUEST};
        register();
        let mut param = vec![0u8; 0x34];
        for (i, v) in [48000u32, 240, 24, 1, 4, 1, 0, 0].iter().enumerate() {
            w32(&mut param, i * 4, *v);
        }
        w32(&mut param, 0x30, 0x3156_4552); // REV1
        let h = crate::ipc::connect("audren:u").unwrap();
        let send = |h: u32, cmd: u32, data: Vec<u8>, a: Vec<Buf>, b: Vec<Buf>| {
            let mut m = Builder { ty: T_REQUEST, cmd, data, a, b, ..Default::default() }.build();
            crate::ipc::process(h, &mut m);
            hipc::parse_response(&m, false).unwrap()
        };
        let r = send(h, 1, param.clone(), vec![], vec![]);
        assert_eq!(r.result, 0);
        assert!(u64::from_le_bytes(r.data[..8].try_into().unwrap()) >= 0x1000);
        let r = send(h, 0, [param.clone(), vec![0; 12], 0x10000u64.to_le_bytes().to_vec(), 0u64.to_le_bytes().to_vec()].concat(), vec![], vec![]);
        assert_eq!((r.result, r.moved.len()), (0, 1));
        let ar = r.moved[0];
        for (cmd, want) in [(0, 48000u32), (1, 240), (2, 24), (3, 1)] {
            let r = send(ar, cmd, vec![], vec![], vec![]);
            assert_eq!(u32::from_le_bytes(r.data[..4].try_into().unwrap()), want, "command {cmd}");
        }
        // an update: one pool to attach, 4 voices (voice 0 playing 240 samples), no mixes, one sink
        let pcm = vec![8192i16; 240];
        let mut voices = vec![0u8; 4 * VOICE];
        voices[..VOICE].copy_from_slice(&voice(&pcm));
        for i in 1..4 {
            w32(&mut voices, i * VOICE, i as u32); // the unused voices have ids of their own
        }
        let mut input = vec![0u8; 0x40];
        let mut pool = vec![0u8; 0x20];
        w32(&mut pool, 0x10, 4);
        let sink = vec![0u8; SINK];
        for (k, n) in [(0usize, 0x3156_4552u32), (2, 0x20), (3, 4 * VOICE as u32), (7, SINK as u32)] {
            w32(&mut input, k * 4, n);
        }
        input.extend_from_slice(&pool);
        input.extend_from_slice(&voices);
        input.extend_from_slice(&sink);
        let mut out = vec![0u8; 0x1000];
        let r = send(ar, 4, vec![], vec![Buf { addr: input.as_ptr() as u64, size: input.len() as u64, tag: 0 }], vec![Buf { addr: out.as_mut_ptr() as u64, size: out.len() as u64, tag: 0 }]);
        assert_eq!(r.result, 0);
        assert_eq!(rd32(&out, 0), 0x3156_4552, "revision echoed");
        assert_eq!((rd32(&out, 8), rd32(&out, 12)), (0x10, 4 * 0x10), "pool and voice status sizes");
        assert_eq!(rd32(&out, 0x40), 5, "the pool is attached");
        // not started: the voice has not moved
        assert_eq!(rd64(&out, 0x50), 0);
        // the same update mixed as a started renderer does, two blocks: the voice's samples come out
        let mut ren = Renderer::new(Params::read(&param));
        ren.started = true;
        let mut o2 = vec![];
        let pcm_out = ren.update(&input, &mut o2, Some(2)).unwrap();
        assert_eq!(pcm_out.len(), 2 * NF * 2);
        // a sound new in this update waits out its catch-up block (time from before the update), then plays
        near(pcm_out[0], 0.0);
        near(pcm_out[NF * 2], 0.25 * SQRT_HALF);
        assert_eq!(rd64(&o2, 0x50), 240, "voice 0 played its 240 samples");
        assert_eq!(rd32(&o2, 0x58), 1, "and consumed its wave buffer");
    }
}
