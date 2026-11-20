//! The sound card: every audio stream of the game (each audio renderer's output, each AudioOut) is a queue of
//! 48 kHz stereo blocks that one output device plays, mixed. The device is the system's default output through
//! WASAPI (cpal), in its own format and rate (resampled linearly from 48 kHz). A block goes back to its producer
//! (released, with its tag) once a 48 kHz wall clock has gone past its end (as Turbo's audout clock: the guest's
//! buffers come back in real time, smoothly every 2 ms, not when the device's callback or the slowed-down playback
//! reaches them; the samples were copied at append), and the producer's release hook runs on a host thread. A
//! producer paced by its releases (two 5 ms buffers in flight, Beach Bounce) so keeps real time, and its queue
//! fills the playback's reserve instead of draining it at every device period.
//!
//! Smooth slowdown (`audio_slowdown`, Turbo's option, on by default): when a stream runs short because the PC
//! cannot produce its sound in real time, it plays slower, eased toward the fill of its queue, instead of breaking
//! into gaps; every queued sample is kept. Off, a stream that runs dry is an underrun: its last sample fades out
//! and the gap is counted. A stream starts playing once it holds 50 ms or has waited 50 ms. The mix goes through a
//! linked peak limiter (instant attack, 100 ms release) before the device's format.
//!
//! No output device at all (none installed, or every open fails): the streams are consumed in real time by a
//! clock, so the game keeps its pace, and the log says so in red.
//!
//! Muting (`set_muted`, the player's `mute_on_focus_loss`): the device gets silence, faded over 10 ms; the streams
//! play on at their pace (the game is not slowed or told), and a recording (the tap) keeps the sound.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
const PRIME: u32 = 2400; // 50 ms
const FADE: f32 = 1.0 / 240.0;
const CHUNK: usize = 240;

type Hook = Box<dyn Fn() + Send + Sync>;

struct Block {
    pcm: Vec<f32>, // stereo, interleaved
    at: usize,     // frames already played
}

#[derive(Default)]
struct St {
    started: bool,
    closed: bool,
    blocks: VecDeque<Block>,
    queued: usize, // frames not yet played
    released: VecDeque<u64>,
    pending: VecDeque<(u64, u64)>, // blocks not released yet: (tag, end on the appended timeline)
    appended: u64,                 // frames appended so far
    paced: u64,                    // the release clock: frames due by now (never past `appended`)
    clock0: Option<(Instant, u64)>, // the release clock's start (time, `paced` then); None while it waits for frames
    high: f64,     // the queue's high-water mark, decaying (the slowdown's reserve)
    primed: bool,
    waiting: u32,
    phase: f64,
    rate: f64,
    prev: [f32; 2],
    cur: [f32; 2],
    env: f32,
    dry: bool,
    // statistics
    played: u64,
    underruns: u64,
    slowed: u64,
    nonsilent: u64,
}

pub struct Stream {
    pub name: String,
    st: Mutex<St>,
    cv: Condvar,
    hook: Mutex<Option<Hook>>,
}

impl Stream {
    /// Queues a block of stereo 16-bit samples; `tag` comes back when it has played.
    pub fn append_i16(&self, tag: u64, pcm: &[i16]) {
        self.append(tag, pcm.iter().map(|&s| s as f32 / 32768.0).collect());
    }
    /// Queues a block of stereo float samples.
    pub fn append(&self, tag: u64, pcm: Vec<f32>) {
        let mut st = self.st.lock().unwrap();
        let n = pcm.len() / 2;
        if n == 0 {
            st.released.push_back(tag);
            drop(st);
            self.cv.notify_all();
            return;
        }
        st.queued += n;
        st.appended += n as u64;
        let end = st.appended;
        st.pending.push_back((tag, end));
        let q = st.queued as f64;
        if q > st.high {
            st.high = q;
        }
        st.blocks.push_back(Block { pcm, at: 0 });
    }
    pub fn start(&self) {
        let mut st = self.st.lock().unwrap();
        if !st.started {
            st.started = true;
            st.primed = false;
            st.waiting = 0;
        }
    }
    /// Stops playing; every block still queued goes back (the SDK returns them on Stop).
    pub fn stop(&self) {
        let mut st = self.st.lock().unwrap();
        st.started = false;
        st.blocks.clear();
        while let Some((tag, _)) = st.pending.pop_front() {
            st.released.push_back(tag);
        }
        st.paced = st.appended;
        st.clock0 = None;
        st.queued = 0;
        drop(st);
        self.cv.notify_all();
        self.fire();
    }
    pub fn started(&self) -> bool {
        self.st.lock().unwrap().started
    }
    /// The next block that has played, if any.
    pub fn released(&self) -> Option<u64> {
        self.st.lock().unwrap().released.pop_front()
    }
    /// Frames queued and not played yet.
    pub fn queued(&self) -> usize {
        self.st.lock().unwrap().queued
    }
    /// Blocks queued and not released yet.
    pub fn blocks(&self) -> usize {
        self.st.lock().unwrap().pending.len()
    }
    pub fn contains(&self, tag: u64) -> bool {
        let st = self.st.lock().unwrap();
        st.pending.iter().any(|b| b.0 == tag) || st.released.contains(&tag)
    }
    /// Waits up to `ms` for a release (or the stream's close).
    pub fn wait(&self, ms: u64) {
        let st = self.st.lock().unwrap();
        if st.released.is_empty() && !st.closed {
            let _ = self.cv.wait_timeout(st, Duration::from_millis(ms));
        }
    }
    /// The hook that runs (on a host thread) after blocks were released.
    pub fn on_release(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }
    fn fire(&self) {
        if let Some(h) = self.hook.lock().unwrap().as_ref() {
            h();
        }
    }
    pub fn close(self: &Arc<Self>) {
        {
            let mut st = self.st.lock().unwrap();
            st.closed = true;
            st.started = false;
        }
        self.cv.notify_all();
        sink().streams.lock().unwrap().retain(|s| !Arc::ptr_eq(s, self));
        let st = self.st.lock().unwrap();
        if st.played > 0 {
            crate::log!("{}", stats_line(&self.name, &st));
        }
    }

    /// One 48 kHz frame of this stream into `out` (added). Returns true if a block finished.
    fn pull(&self, st: &mut St, out: &mut [f32; 2], smooth: bool) -> bool {
        if !st.started {
            return false;
        }
        let mut finished = false;
        if !st.primed {
            st.waiting += 1;
            if st.queued > 0 && (st.queued as u32 >= PRIME || st.waiting >= PRIME) {
                st.primed = true;
                st.phase = 0.0;
                st.rate = 1.0;
                st.env = 1.0;
                finished |= take(st);
                st.prev = st.cur;
            } else {
                return finished;
            }
        }
        if st.queued == 0 && st.blocks.is_empty() {
            if !st.dry {
                st.dry = true;
                st.underruns += 1;
                crate::metric!("audio_underruns", 1);
            }
            out[0] += st.prev[0] * st.env;
            out[1] += st.prev[1] * st.env;
            st.env = (st.env - FADE).max(0.0);
            return finished;
        }
        st.dry = false;
        if st.env < 1.0 {
            st.env = (st.env + FADE).min(1.0);
            out[0] += st.prev[0] * st.env;
            out[1] += st.prev[1] * st.env;
            return finished;
        }
        let next = peek(st);
        let p = st.phase as f32;
        let s = [st.prev[0] + (next[0] - st.prev[0]) * p, st.prev[1] + (next[1] - st.prev[1]) * p];
        out[0] += s[0];
        out[1] += s[1];
        if s[0] != 0.0 || s[1] != 0.0 {
            st.nonsilent += 1;
        }
        st.played += 1;
        if smooth {
            // the reserve: a quarter of the queue's usual fill (at most 100 ms): a producer paced by its releases
            // keeps its queue near full, so only one that falls behind plays slower
            st.high = (st.high * 0.99999).max(st.queued as f64);
            let reserve = (st.high * 0.25).clamp(240.0, 4800.0);
            let target = (st.queued as f64 / reserve).clamp(0.02, 1.0);
            st.rate += (target - st.rate) * 0.0005;
            if st.rate < 0.999 {
                st.slowed += 1;
                crate::metric!("audio_slowed_frames", 1);
            }
        } else {
            st.rate = 1.0;
        }
        st.phase += st.rate;
        while st.phase >= 1.0 && st.queued > 0 {
            st.phase -= 1.0;
            st.prev = peek(st);
            finished |= take(st);
        }
        finished
    }
}

fn peek(st: &St) -> [f32; 2] {
    match st.blocks.front() {
        Some(b) => [b.pcm[b.at * 2], b.pcm[b.at * 2 + 1]],
        None => st.prev,
    }
}

/// Advances one frame; true when a block was finished (and released).
fn take(st: &mut St) -> bool {
    let Some(b) = st.blocks.front_mut() else { return false };
    st.cur = [b.pcm[b.at * 2], b.pcm[b.at * 2 + 1]];
    b.at += 1;
    st.queued -= 1;
    if b.at * 2 >= b.pcm.len() {
            st.blocks.pop_front();
    }
    false
}

/// The release clock at `now`: 48 kHz of wall time since it started (it starts with the first frame appended and
/// stops, waiting, when it catches up with the appended frames); blocks whose end it reached go back to the
/// producer. True if one did.
fn pace(st: &mut St, now: Instant) -> bool {
    if !st.started || st.paced >= st.appended {
        st.clock0 = None;
    } else {
        let (t0, base) = *st.clock0.get_or_insert((now, st.paced));
        let due = base + (now.duration_since(t0).as_secs_f64() * RATE as f64) as u64;
        if due >= st.appended {
            st.paced = st.appended;
            st.clock0 = None;
        } else {
            st.paced = due;
        }
    }
    let mut any = false;
    while st.pending.front().is_some_and(|&(_, end)| end <= st.paced) {
        let (tag, _) = st.pending.pop_front().unwrap();
        st.released.push_back(tag);
        any = true;
    }
    any
}

fn stats_line(name: &str, st: &St) -> String {
    format!(
        "audio: {name}: {:.1} s played ({:.1} s not silent), {} underruns, slowed down {:.1} s",
        st.played as f64 / RATE as f64,
        st.nonsilent as f64 / RATE as f64,
        st.underruns,
        st.slowed as f64 / RATE as f64
    )
}

struct Sink {
    streams: Mutex<Vec<Arc<Stream>>>,
    notify: Mutex<Option<mpsc::Sender<Arc<Stream>>>>,
    device: Mutex<String>,
    callbacks: AtomicU64,
    frames: AtomicU64,
    peak_limited: AtomicU64,
    started: AtomicBool,
    smooth: AtomicBool,
}

fn sink() -> &'static Sink {
    static S: OnceLock<Sink> = OnceLock::new();
    S.get_or_init(|| Sink {
        streams: Mutex::new(Vec::new()),
        notify: Mutex::new(None),
        device: Mutex::new(String::new()),
        callbacks: AtomicU64::new(0),
        frames: AtomicU64::new(0),
        peak_limited: AtomicU64::new(0),
        started: AtomicBool::new(false),
        smooth: AtomicBool::new(true),
    })
}

/// A new stream on the output (the device is opened on the first).
pub fn open(name: &str) -> Arc<Stream> {
    start();
    let s = Arc::new(Stream {
        name: name.to_string(),
        st: Mutex::new(St { rate: 1.0, env: 1.0, ..Default::default() }),
        cv: Condvar::new(),
        hook: Mutex::new(None),
    });
    sink().streams.lock().unwrap().push(s.clone());
    s
}

/// The device's name ("" until it is open).
pub fn device() -> String {
    sink().device.lock().unwrap().clone()
}

/// Smooth slowdown on or off (the `audio_slowdown` setting, read at start).
pub fn set_smooth(on: bool) {
    sink().smooth.store(on, Ordering::Relaxed);
}

/// The output's line for the log: the device and each open stream.
pub fn report() -> Vec<String> {
    let s = sink();
    let mut v = vec![format!(
        "audio: output {} ({} callbacks, {:.1} s of output, limiter engaged {} times, slowdown {})",
        s.device.lock().unwrap(),
        s.callbacks.load(Ordering::Relaxed),
        s.frames.load(Ordering::Relaxed) as f64 / RATE as f64,
        s.peak_limited.load(Ordering::Relaxed),
        if s.smooth.load(Ordering::Relaxed) { "on" } else { "off" }
    )];
    for st in s.streams.lock().unwrap().iter() {
        v.push(stats_line(&st.name, &st.st.lock().unwrap()));
    }
    v
}

/// A tap of the mixed output (48 kHz stereo floats, after the limiter): the front end's recordings.
pub type Tap = fn(&[f32], u32, u32);
static TAP: Mutex<Option<Tap>> = Mutex::new(None);

/// Sets (or clears) the tap: called with each block of the mix as the sound card takes it, (samples, rate, channels).
pub fn set_tap(f: Option<Tap>) {
    *TAP.lock().unwrap() = f;
}

static MUTED: AtomicBool = AtomicBool::new(false);

/// Silences the device (true) or lets the sound through again; the streams play on either way.
pub fn set_muted(on: bool) {
    if MUTED.swap(on, Ordering::Relaxed) != on {
        crate::log!("audio: {}", if on { "muted (the window lost the focus: mute_on_focus_loss)" } else { "unmuted" });
    }
}

pub fn muted() -> bool {
    MUTED.load(Ordering::Relaxed)
}

/// Fills `out` (stereo, 48 kHz) from every stream; blocks that finished are announced to their producers.
struct Mixer {
    gain: f32,
    /// The mute fade: 1 sound, 0 silence.
    level: f32,
}

impl Mixer {
    fn fill(&mut self, out: &mut [f32]) {
        let s = sink();
        let smooth = s.smooth.load(Ordering::Relaxed);
        out.fill(0.0);
        let streams: Vec<Arc<Stream>> = s.streams.lock().unwrap().clone();
        for strm in &streams {
            let mut fin = false;
            {
                let mut st = strm.st.lock().unwrap();
                for f in out.chunks_exact_mut(2) {
                    let mut x = [0f32; 2];
                    fin |= strm.pull(&mut st, &mut x, smooth);
                    f[0] += x[0];
                    f[1] += x[1];
                }
            }
            if fin {
                strm.cv.notify_all();
                if let Some(tx) = s.notify.lock().unwrap().as_ref() {
                    let _ = tx.send(strm.clone());
                }
            }
        }
        // linked peak limiter: instant attack to 0.95 of full scale, 100 ms release
        let frames = out.len() / 2;
        let peak = out.iter().fold(0f32, |m, &x| if x.is_finite() { m.max(x.abs()) } else { m });
        let ceiling = if peak > 0.95 { 0.95 / peak } else { 1.0 };
        let released = 1.0 - (1.0 - self.gain) * (-(frames as f32) / 4800.0).exp();
        self.gain = ceiling.min(released);
        if self.gain < 0.999 {
            s.peak_limited.fetch_add(1, Ordering::Relaxed);
        }
        for x in out.iter_mut() {
            *x = if x.is_finite() { *x * self.gain } else { 0.0 };
        }
        s.frames.fetch_add(frames as u64, Ordering::Relaxed);
        if let Some(t) = *TAP.lock().unwrap() {
            t(out, RATE, 2);
        }
        mute_fade(&mut self.level, MUTED.load(Ordering::Relaxed), out);
    }
}

/// The mute fade on stereo frames: `level` moves toward 0 (muted) or 1 by 1/480 a frame (10 ms).
fn mute_fade(level: &mut f32, muted: bool, out: &mut [f32]) {
    let want = if muted { 0.0 } else { 1.0 };
    if *level == want && !muted {
        return;
    }
    for f in out.chunks_exact_mut(2) {
        *level = if want > *level { (*level + 1.0 / 480.0).min(1.0) } else { (*level - 1.0 / 480.0).max(0.0) };
        f[0] *= *level;
        f[1] *= *level;
    }
}

/// 48 kHz stereo to the device's rate and channels.
struct Resampler {
    mixer: Mixer,
    buf: Vec<f32>,
    pos: usize,
    a: [f32; 2],
    b: [f32; 2],
    phase: f64,
    step: f64,
}

impl Resampler {
    fn new(dev_rate: u32) -> Resampler {
        Resampler { mixer: Mixer { gain: 1.0, level: 1.0 }, buf: vec![0.0; CHUNK * 2], pos: CHUNK, a: [0.0; 2], b: [0.0; 2], phase: 1.0, step: RATE as f64 / dev_rate as f64 }
    }
    fn next48(&mut self) -> [f32; 2] {
        if self.pos >= CHUNK {
            self.mixer.fill(&mut self.buf);
            self.pos = 0;
        }
        let f = [self.buf[self.pos * 2], self.buf[self.pos * 2 + 1]];
        self.pos += 1;
        f
    }
    fn frame(&mut self) -> [f32; 2] {
        if self.step == 1.0 {
            return self.next48();
        }
        while self.phase >= 1.0 {
            self.a = self.b;
            self.b = self.next48();
            self.phase -= 1.0;
        }
        let p = self.phase as f32;
        self.phase += self.step;
        [self.a[0] + (self.b[0] - self.a[0]) * p, self.a[1] + (self.b[1] - self.a[1]) * p]
    }
}

fn start() {
    let s = sink();
    if s.started.swap(true, Ordering::SeqCst) {
        return;
    }
    s.smooth.store(crate::options::int("audio_slowdown") != 0, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<Arc<Stream>>();
    *s.notify.lock().unwrap() = Some(tx);
    std::thread::Builder::new()
        .name("nxos audio releases".into())
        .spawn(move || {
            while let Ok(st) = rx.recv() {
                st.fire();
            }
        })
        .expect("audio release thread");
    std::thread::Builder::new().name("nxos audio device".into()).spawn(device_thread).expect("audio device thread");
    std::thread::Builder::new()
        .name("nxos audio clock".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_millis(2));
            tick(Instant::now());
        })
        .expect("audio clock thread");
}

/// The release clock of every stream (every 2 ms): blocks it is past go back to their producers.
fn tick(now: Instant) {
    let s = sink();
    let streams: Vec<Arc<Stream>> = s.streams.lock().unwrap().clone();
    for strm in &streams {
        if pace(&mut strm.st.lock().unwrap(), now) {
            strm.cv.notify_all();
            if let Some(tx) = s.notify.lock().unwrap().as_ref() {
                let _ = tx.send(strm.clone());
            }
        }
    }
}

fn device_thread() {
    let lost = Arc::new(AtomicBool::new(false));
    let mut failures = 0;
    loop {
        lost.store(false, Ordering::SeqCst);
        match open_device(lost.clone()) {
            Ok((stream, name)) => {
                failures = 0;
                *sink().device.lock().unwrap() = name.clone();
                crate::log!("audio: output on {name}; smooth slowdown {}", if sink().smooth.load(Ordering::Relaxed) { "on" } else { "off" });
                while !lost.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                drop(stream);
                crate::log!("audio: the output device {name} went away or changed: opening the default device again");
            }
            Err(e) => {
                failures += 1;
                if failures == 1 {
                    crate::log!("[RED] audio: no output device ({e}): the game's sound is consumed in real time by a clock, not heard");
                }
                clock(Duration::from_secs(2));
            }
        }
    }
}

/// Real-time consumption without a device (for `d`), so producers keep their pace.
fn clock(d: Duration) {
    let mut r = Resampler::new(RATE);
    let t0 = Instant::now();
    let mut done = 0u64;
    while t0.elapsed() < d {
        std::thread::sleep(Duration::from_millis(5));
        let due = (t0.elapsed().as_secs_f64() * RATE as f64) as u64;
        while done < due {
            r.next48();
            done += 1;
        }
    }
}

fn open_device(lost: Arc<AtomicBool>) -> Result<(cpal::Stream, String), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or("no default output device")?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let name = format!("{} ({:?}, {} Hz, {} channels, {:?})", dev, host.id(), cfg.sample_rate(), cfg.channels(), cfg.sample_format());
    let fmt = cfg.sample_format();
    let config: cpal::StreamConfig = cfg.into();
    let ch = config.channels as usize;
    let mut rs = Resampler::new(config.sample_rate);
    let l2 = lost.clone();
    let err = move |e: cpal::Error| {
        crate::log!("audio: output stream error: {e}");
        l2.store(true, Ordering::SeqCst);
    };
    macro_rules! build {
        ($t:ty) => {
            dev.build_output_stream(
                config.clone(),
                move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    sink().callbacks.fetch_add(1, Ordering::Relaxed);
                    for f in data.chunks_mut(ch) {
                        let s = rs.frame();
                        for (i, o) in f.iter_mut().enumerate() {
                            let v = match i {
                                0 => s[0],
                                1 => s[1],
                                _ => 0.0,
                            };
                            *o = <$t as cpal::FromSample<f32>>::from_sample_(if ch == 1 { (s[0] + s[1]) * 0.5 } else { v });
                        }
                    }
                },
                err,
                None,
            )
        };
    }
    let stream = match fmt {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::F64 => build!(f64),
        f => return Err(format!("sample format {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> Stream {
        Stream { name: "t".into(), st: Mutex::new(St { rate: 1.0, env: 1.0, ..Default::default() }), cv: Condvar::new(), hook: Mutex::new(None) }
    }
    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_micros(n * 1000)
    }

    #[test]
    fn mute_fades_to_silence_and_back() {
        let mut level = 1.0;
        let mut out = vec![0.5f32; 2 * 960];
        mute_fade(&mut level, true, &mut out);
        assert!(out[0] > 0.49 && out[0] < 0.5, "fades, no click");
        assert!(out[2 * 480..].iter().all(|&x| x == 0.0), "silent after 10 ms");
        let mut out = vec![0.5f32; 2 * 960];
        mute_fade(&mut level, false, &mut out);
        assert_eq!(out[2 * 959], 0.5, "back to full");
        let mut out = vec![0.5f32; 4];
        mute_fade(&mut level, false, &mut out);
        assert_eq!(out, vec![0.5; 4], "untouched when not muted");
    }

    #[test]
    fn plays_every_sample() {
        let s = st();
        s.start();
        let ramp: Vec<f32> = (0..2 * 3000).map(|i| (i / 2) as f32 / 4000.0).collect();
        s.append(7, ramp);
        let mut got = vec![];
        let mut st = s.st.lock().unwrap();
        for _ in 0..3000 + PRIME as usize {
            let mut o = [0f32; 2];
            s.pull(&mut st, &mut o, false);
            got.push(o[0]);
        }
        assert_eq!(st.underruns, 1, "one gap after the block");
        // the samples come out in order, none skipped
        assert!(got[..3000].windows(2).all(|w| w[1] > w[0]));
        assert!(got[3000..].windows(2).all(|w| w[1] <= w[0]), "then the last sample fades out");
    }

    #[test]
    fn slowdown_keeps_samples() {
        let s = st();
        s.start();
        s.append(1, vec![0.5; 2 * 2400]);
        let mut st = s.st.lock().unwrap();
        let mut n = 0;
        while st.queued > 0 && n < 100_000 {
            let mut o = [0f32; 2];
            s.pull(&mut st, &mut o, true);
            n += 1;
        }
        assert!(n >= 2399, "every sample played ({n} pulls)");
    }

    #[test]
    fn releases_on_the_wall_clock() {
        // two 256-frame (5.33 ms) buffers: the first back at 5.33 ms, the second at 10.67 ms, whatever the playback
        let s = st();
        s.start();
        s.append(1, vec![0.1; 2 * 256]);
        s.append(2, vec![0.1; 2 * 256]);
        let mut st = s.st.lock().unwrap();
        let t0 = Instant::now();
        assert!(!pace(&mut st, t0));
        assert!(!pace(&mut st, ms(t0, 5)));
        assert!(pace(&mut st, ms(t0, 6)));
        assert_eq!(st.released.iter().copied().collect::<Vec<_>>(), vec![1]);
        assert!(pace(&mut st, ms(t0, 11)));
        assert_eq!(st.released.len(), 2);
        assert_eq!(st.queued, 512, "nothing played yet: the samples stay queued for the device");
    }

    #[test]
    fn the_clock_waits_for_a_late_producer() {
        // the producer appends late: the clock does not run ahead over the gap (no burst of releases after it)
        let s = st();
        s.start();
        s.append(1, vec![0.1; 2 * 480]);
        let mut st = s.st.lock().unwrap();
        let t0 = Instant::now();
        pace(&mut st, t0);
        assert!(pace(&mut st, ms(t0, 11)));
        assert!(st.clock0.is_none(), "caught up: the clock waits");
        drop(st);
        s.append(2, vec![0.1; 2 * 480]);
        let mut st = s.st.lock().unwrap();
        assert!(!pace(&mut st, ms(t0, 100)), "starts again from the new frames");
        assert!(!pace(&mut st, ms(t0, 105)));
        assert!(pace(&mut st, ms(t0, 111)));
    }
}
