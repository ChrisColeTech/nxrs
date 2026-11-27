//! The sound card: every audio stream of the game (each audio renderer's output, each AudioOut) is a queue of
//! 48 kHz stereo blocks that one output device plays, mixed. The device is the system's default output through
//! WASAPI (cpal), in its own format and rate (resampled linearly from 48 kHz). A block goes back to its producer
//! (released, with its tag) once a 48 kHz wall clock has gone past its end (the audout clock: the guest's
//! buffers come back in real time, smoothly every 2 ms, not when the device's callback or the slowed-down playback
//! reaches them; the samples were copied at append), and the producer's release hook runs on a host thread. A
//! producer paced by its releases (two 5 ms buffers in flight, Beach Bounce) so keeps real time, and its queue
//! fills the playback's reserve instead of draining it at every device period.
//!
//! Smooth slowdown (`audio_slowdown`, on by default): when a stream runs short because the PC
//! cannot produce its sound in real time, it plays slower, eased toward the fill of its queue, instead of breaking
//! into gaps; every queued sample is kept. Off, a stream that runs dry is an underrun: its last sample fades out
//! and the gap is counted. A stream starts playing once it holds 50 ms or has waited 50 ms. The mix goes through a
//! linked peak limiter (instant attack, 100 ms release) before the device's format.
//!
//! No output device at all (none installed, or every open fails): the streams are consumed in real time by a
//! clock, so the game keeps its pace, and the log says so in red.
//!
//! Muting (`set_muted`, the player's `mute_on_focus_loss`; `NXOS_AUDIO_MUTE=1` for a whole run): the device gets
//! silence, faded over 10 ms; the streams play on at their pace (the game is not slowed or told), and a recording
//! (the tap) keeps the sound.
//!
//! Diagnostics (nxnative DM_AUDIO_*): every 5 s of output the log gets each channel's RMS and the
//! peak (silence once, then every minute; sound every 5 s for the first 5 minutes, then every minute), so "is there
//! sound" can be read from a log; `NXOS_AUDIO_DEVICE=<part of a name>` plays on that output device instead of the
//! default (a name no device has: the default, said in red); `NXOS_AUDIO_DUMP=<file.wav>` writes the mix as it goes
//! out (48 kHz stereo 16-bit WAV, its header kept valid as it grows).

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
    high: f64, // the queue's high-water mark, decaying (the slowdown's reserve)
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
        // F7's copy of the game's own sound, before the slowdown and the limiter
        if pcm.len() >= 2 {
            pre_tap(self as *const Stream as usize, &pcm);
        }
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
        sink()
            .streams
            .lock()
            .unwrap()
            .retain(|s| !Arc::ptr_eq(s, self));
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
        let s = [
            st.prev[0] + (next[0] - st.prev[0]) * p,
            st.prev[1] + (next[1] - st.prev[1]) * p,
        ];
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
    let Some(b) = st.blocks.front_mut() else {
        return false;
    };
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
        st: Mutex::new(St {
            rate: 1.0,
            env: 1.0,
            ..Default::default()
        }),
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
    let mut v =
        vec![format!(
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

/// The game's sound as it produced it, for F7: every stream's
/// appended blocks are mixed on the game clock's timeline, before the slowdown (which stretches what the sound card
/// plays) and the device's limiter. A block is placed so that it ends now; when that is within 10 ms of where the
/// stream's last block ended, it follows it (the event jitter snaps to the sample clock, real gaps stay). Mixed
/// frames older than `LAG` go to the tap, peak-limited for the recording on their own.
pub struct Timeline {
    /// The frame index of `buf[0]` on the game clock's 48 kHz timeline.
    base: u64,
    buf: VecDeque<[f32; 2]>,
    /// Each stream's end on the timeline.
    next: std::collections::HashMap<usize, u64>,
    gain: f32,
}

impl Timeline {
    pub const LAG: u64 = RATE as u64 / 5; // 200 ms for every stream's block of that time to arrive
    const JITTER: u64 = 480; // 10 ms
    const CAP: u64 = RATE as u64 * 4; // a block further ahead is cut

    pub fn new(now: u64) -> Timeline {
        Timeline {
            base: now,
            buf: VecDeque::new(),
            next: Default::default(),
            gain: 1.0,
        }
    }

    /// Where a block of `frames` that arrives at `now` starts, for a stream that ended at `next`.
    pub fn place(now: u64, frames: u64, next: Option<u64>) -> u64 {
        let start = now.saturating_sub(frames);
        match next {
            Some(n) if start <= n + Self::JITTER => n,
            _ => start,
        }
    }

    /// Stream `id`'s block (stereo, interleaved) arriving at `now`; returns the mixed frames now due (interleaved).
    pub fn add(&mut self, id: usize, pcm: &[f32], now: u64) -> Vec<f32> {
        let n = (pcm.len() / 2) as u64;
        let at = Self::place(now, n, self.next.get(&id).copied());
        self.next.insert(id, at + n);
        for f in 0..n {
            let i = at + f;
            if i < self.base || i >= self.base + Self::CAP {
                continue; // already handed over, or absurdly far ahead
            }
            let k = (i - self.base) as usize;
            if k >= self.buf.len() {
                self.buf.resize(k + 1, [0.0; 2]);
            }
            let s = &mut self.buf[k];
            for c in 0..2 {
                let x = pcm[2 * f as usize + c];
                s[c] += if x.is_finite() { x } else { 0.0 };
            }
        }
        self.due(now)
    }

    /// The frames before `now - LAG`, peak-limited (instant attack to 0.95, 100 ms release).
    pub fn due(&mut self, now: u64) -> Vec<f32> {
        let upto = now.saturating_sub(Self::LAG);
        if upto <= self.base {
            return vec![];
        }
        let k = ((upto - self.base) as usize).min(self.buf.len());
        let mut out: Vec<f32> = self.buf.drain(..k).flat_map(|s| s).collect();
        out.resize(2 * (upto - self.base) as usize, 0.0); // a stretch no stream wrote is silence
        self.base = upto;
        let peak = out.iter().fold(0f32, |m, &x| m.max(x.abs()));
        let ceiling = if peak > 0.95 { 0.95 / peak } else { 1.0 };
        let released = 1.0 - (1.0 - self.gain) * (-((out.len() / 2) as f32) / 4800.0).exp();
        self.gain = ceiling.min(released);
        for x in &mut out {
            *x *= self.gain;
        }
        out
    }
}

static PRE_TAP: Mutex<Option<(Tap, Timeline)>> = Mutex::new(None);

/// Sets (or clears) the tap of the game's sound before the slowdown and the limiter (`Timeline`): called with
/// stretches of the mix on the game clock as the streams append, (samples, rate, channels).
pub fn set_pre_tap(f: Option<Tap>) {
    *PRE_TAP.lock().unwrap() = f.map(|f| (f, Timeline::new(clock_frames())));
}

fn clock_frames() -> u64 {
    crate::clock::now_ns() / 1000 * RATE as u64 / 1_000_000
}

fn pre_tap(id: usize, pcm: &[f32]) {
    let mut g = PRE_TAP.lock().unwrap();
    if let Some((f, tl)) = g.as_mut() {
        let out = tl.add(id, pcm, clock_frames());
        if !out.is_empty() {
            f(&out, RATE, 2);
        }
    }
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
        crate::log!(
            "audio: {}",
            if on {
                "muted (the window lost the focus: mute_on_focus_loss)"
            } else {
                "unmuted"
            }
        );
    }
}

/// The graceful exit (nxnative hle_lifecycle.c `dm_audio_suspend`): the device silenced before graphics drains, so
/// the last blocks do not stutter or loop while the process ends.
pub fn pause_for_exit() {
    MUTED.store(true, Ordering::Relaxed);
    crate::log!("audio: paused for the exit");
}

pub fn muted() -> bool {
    MUTED.load(Ordering::Relaxed)
}

/// The mixed output measured: each channel's mean square and the peak over 5 s of sound.
#[derive(Default)]
struct Meter {
    sq: [f64; 2],
    peak: f32,
    n: u64,
    silent: u64,
    lines: u64,
}

impl Meter {
    /// Adds a block (stereo); every 5 s of it, the line to log (None: not due, or a repeat of silence / steady sound
    /// that is said less often).
    fn add(&mut self, out: &[f32]) -> Option<String> {
        for f in out.chunks_exact(2) {
            self.sq[0] += (f[0] as f64).powi(2);
            self.sq[1] += (f[1] as f64).powi(2);
            self.peak = self.peak.max(f[0].abs()).max(f[1].abs());
        }
        self.n += (out.len() / 2) as u64;
        if self.n < 5 * RATE as u64 {
            return None;
        }
        let (l, r) = (
            (self.sq[0] / self.n as f64).sqrt(),
            (self.sq[1] / self.n as f64).sqrt(),
        );
        let db = |x: f64| if x > 0.0 { 20.0 * x.log10() } else { -200.0 };
        let quiet = l < 1e-5 && r < 1e-5;
        let say = if quiet {
            self.silent += 1;
            self.silent % 12 == 1
        } else {
            self.silent = 0;
            self.lines < 60 || self.lines % 12 == 0
        };
        let line = say.then(|| {
            format!(
                "audio: output, last {:.0} s: RMS {l:.5} / {r:.5} ({:.1} / {:.1} dBFS), peak {:.4}",
                self.n as f64 / RATE as f64,
                db(l),
                db(r),
                self.peak
            )
        });
        self.lines += 1;
        (self.sq, self.peak, self.n) = ([0.0; 2], 0.0, 0);
        line
    }
}

/// A 48 kHz stereo 16-bit WAV file that grows: the header's sizes are rewritten after each block, so the file is
/// whole whenever the run ends.
pub struct Wav {
    f: std::fs::File,
    data: u64,
}

impl Wav {
    pub fn create(path: &std::path::Path) -> std::io::Result<Wav> {
        let mut w = Wav {
            f: std::fs::File::create(path)?,
            data: 0,
        };
        w.header()?;
        Ok(w)
    }
    fn header(&mut self) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let d = self.data.min(u32::MAX as u64 - 36) as u32;
        let mut h = Vec::with_capacity(44);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(36 + d).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes()); // PCM
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&RATE.to_le_bytes());
        h.extend_from_slice(&(RATE * 4).to_le_bytes());
        h.extend_from_slice(&4u16.to_le_bytes());
        h.extend_from_slice(&16u16.to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&d.to_le_bytes());
        self.f.seek(SeekFrom::Start(0))?;
        self.f.write_all(&h)?;
        self.f.seek(SeekFrom::End(0))?;
        Ok(())
    }
    /// Appends stereo samples (full scale = 1).
    pub fn write(&mut self, pcm: &[f32]) -> std::io::Result<()> {
        use std::io::Write;
        let b: Vec<u8> = pcm
            .iter()
            .flat_map(|&x| ((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())
            .collect();
        self.f.write_all(&b)?;
        self.data += b.len() as u64;
        self.header()
    }
}

/// `NXOS_AUDIO_DUMP`: the writer thread's queue (blocks of the mix), made at the first block.
fn dump_tx() -> Option<&'static Mutex<mpsc::Sender<Vec<f32>>>> {
    static TX: OnceLock<Option<Mutex<mpsc::Sender<Vec<f32>>>>> = OnceLock::new();
    TX.get_or_init(|| {
        let path = std::path::PathBuf::from(
            std::env::var_os("NXOS_AUDIO_DUMP").filter(|p| !p.is_empty())?,
        );
        let mut w = match Wav::create(&path) {
            Ok(w) => w,
            Err(e) => {
                crate::log!("[RED] audio: NXOS_AUDIO_DUMP={}: {e}", path.display());
                return None;
            }
        };
        crate::log!(
            "audio: the mix is written to {} as it goes out (NXOS_AUDIO_DUMP)",
            path.display()
        );
        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        std::thread::Builder::new()
            .name("nxos audio dump".into())
            .spawn(move || {
                while let Ok(b) = rx.recv() {
                    if let Err(e) = w.write(&b) {
                        crate::log!("[RED] audio: NXOS_AUDIO_DUMP stopped: {e}");
                        return;
                    }
                }
            })
            .ok()?;
        Some(Mutex::new(tx))
    })
    .as_ref()
}

/// `NXOS_AUDIO_MUTE=1`: the whole run silent on the device (the meter, a dump and a recording keep the sound).
fn mute_run() -> bool {
    static M: OnceLock<bool> = OnceLock::new();
    *M.get_or_init(|| std::env::var("NXOS_AUDIO_MUTE").is_ok_and(|v| v.trim() == "1"))
}

/// Fills `out` (stereo, 48 kHz) from every stream; blocks that finished are announced to their producers.
struct Mixer {
    gain: f32,
    /// The mute fade: 1 sound, 0 silence.
    level: f32,
    meter: Meter,
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
        let peak = out
            .iter()
            .fold(0f32, |m, &x| if x.is_finite() { m.max(x.abs()) } else { m });
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
        if let Some(l) = self.meter.add(out) {
            crate::log!("{l}");
        }
        if let Some(tx) = dump_tx() {
            let _ = tx.lock().unwrap().send(out.to_vec());
        }
        mute_fade(
            &mut self.level,
            MUTED.load(Ordering::Relaxed) || mute_run(),
            out,
        );
    }
}

/// The mute fade on stereo frames: `level` moves toward 0 (muted) or 1 by 1/480 a frame (10 ms).
fn mute_fade(level: &mut f32, muted: bool, out: &mut [f32]) {
    let want = if muted { 0.0 } else { 1.0 };
    if *level == want && !muted {
        return;
    }
    for f in out.chunks_exact_mut(2) {
        *level = if want > *level {
            (*level + 1.0 / 480.0).min(1.0)
        } else {
            (*level - 1.0 / 480.0).max(0.0)
        };
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
        Resampler {
            mixer: Mixer {
                gain: 1.0,
                level: 1.0,
                meter: Meter::default(),
            },
            buf: vec![0.0; CHUNK * 2],
            pos: CHUNK,
            a: [0.0; 2],
            b: [0.0; 2],
            phase: 1.0,
            step: RATE as f64 / dev_rate as f64,
        }
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
        [
            self.a[0] + (self.b[0] - self.a[0]) * p,
            self.a[1] + (self.b[1] - self.a[1]) * p,
        ]
    }
}

fn start() {
    let s = sink();
    if s.started.swap(true, Ordering::SeqCst) {
        return;
    }
    s.smooth.store(
        crate::options::int("audio_slowdown") != 0,
        Ordering::Relaxed,
    );
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
    // the web: the page's AudioWorklet through a shared ring (docs/35 W3)
    #[cfg(target_os = "emscripten")]
    let device: fn() = web_device_thread;
    #[cfg(not(target_os = "emscripten"))]
    let device: fn() = device_thread;
    std::thread::Builder::new()
        .name("nxos audio device".into())
        .spawn(device)
        .expect("audio device thread");
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
                crate::log!(
                    "audio: output on {name}; smooth slowdown {}",
                    if sink().smooth.load(Ordering::Relaxed) {
                        "on"
                    } else {
                        "off"
                    }
                );
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

/// The web's output device: the mix written into the ring the page's AudioWorklet plays, kept at [`WEB_FILL`] frames
/// ahead. Until the page's AudioContext runs (the browser starts sound only after a click or a key) or while it does
/// not (its heartbeat stops), the mix is consumed in real time, as without a device, and the log says so.
#[cfg(target_os = "emscripten")]
const WEB_FILL: u32 = 3072;

#[cfg(target_os = "emscripten")]
fn web_device_thread() {
    use crate::web::audio::{Ring, CAPACITY};
    let ring = Ring::publish();
    let mut r = Resampler::new(RATE);
    let mut running = false;
    let mut beat = (ring.heartbeat(), Instant::now());
    let mut clock0 = (Instant::now(), 0u64);
    let mut report = (Instant::now(), (0u32, 0u32), false);
    let mut buf: Vec<[f32; 2]> = Vec::with_capacity(CAPACITY as usize);
    crate::log!("audio: the page's sound starts at the first click or key (the browser's autoplay rule); until then the game's sound is consumed in real time, not heard");
    // the longest pass of this loop since the last report (a stall that empties the ring shows here)
    let mut last = Instant::now();
    let mut longest = Duration::ZERO;
    loop {
        std::thread::sleep(Duration::from_millis(2));
        let now = Instant::now();
        // the page's menu pauses the game (WEB-W9): its AudioContext is suspended, the ring is left as it is and
        // nothing is consumed, so the sound goes on where it stopped
        if crate::clock::held() {
            beat = (ring.heartbeat(), now);
            clock0 = (now, 0);
            last = now;
            continue;
        }
        longest = longest.max(now.duration_since(last));
        last = now;
        let hb = ring.heartbeat();
        if hb != beat.0 {
            beat = (hb, now);
        }
        let (state, rate) = ring.context();
        let live = state == 1 && now.duration_since(beat.1) < Duration::from_millis(500);
        if live != running {
            running = live;
            if live {
                *sink().device.lock().unwrap() = format!("the page's AudioWorklet ({rate} Hz)");
                crate::log!("audio: output on the page's AudioWorklet ({rate} Hz context, 48 kHz stereo ring of {CAPACITY} frames kept {WEB_FILL} ahead)");
            } else {
                crate::log!("[RED] audio: the page's AudioWorklet stopped (context state {state}): the game's sound is consumed in real time, not heard");
                clock0 = (now, 0);
            }
        }
        if running {
            let fill = ring.fill();
            if fill < WEB_FILL {
                buf.clear();
                buf.extend((0..WEB_FILL - fill).map(|_| r.next48()));
                ring.push(&buf);
            }
        } else {
            let due = (now.duration_since(clock0.0).as_secs_f64() * RATE as f64) as u64;
            while clock0.1 < due {
                r.next48();
                clock0.1 += 1;
            }
        }
        if running && now.duration_since(report.0) >= Duration::from_secs(5) {
            let u = ring.underruns();
            // the first report once running, then whenever the underruns grew
            if u != report.1 || !report.2 {
                crate::log!(
                    "audio: web output: {} underruns ({} frames of silence) so far; ring fill {}/{CAPACITY}, worklet calls {hb}, longest refill gap {:.0} ms",
                    u.1,
                    u.0,
                    ring.fill(),
                    longest.as_secs_f64() * 1e3
                );
            }
            report = (now, u, true);
            longest = Duration::ZERO;
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

/// The output device `want` names: the first whose name contains it, case aside.
fn pick_device(names: &[String], want: &str) -> Option<usize> {
    let w = want.trim().to_lowercase();
    names.iter().position(|n| n.to_lowercase().contains(&w))
}

fn open_device(lost: Arc<AtomicBool>) -> Result<(cpal::Stream, String), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let mut picked = None;
    if let Some(want) = std::env::var("NXOS_AUDIO_DEVICE")
        .ok()
        .filter(|w| !w.trim().is_empty())
    {
        let devs: Vec<cpal::Device> = host
            .output_devices()
            .map(|d| d.collect())
            .unwrap_or_default();
        let names: Vec<String> = devs.iter().map(|d| d.to_string()).collect();
        match pick_device(&names, &want) {
            Some(i) => picked = Some(devs[i].clone()),
            None => crate::log_once!("audio device pick", "[RED] audio: NXOS_AUDIO_DEVICE='{want}' names no output device ({}): the default output plays", names.join(", ")),
        }
    }
    let dev = match picked {
        Some(d) => d,
        None => host
            .default_output_device()
            .ok_or("no default output device")?,
    };
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let name = format!(
        "{} ({:?}, {} Hz, {} channels, {:?})",
        dev,
        host.id(),
        cfg.sample_rate(),
        cfg.channels(),
        cfg.sample_format()
    );
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
                            *o = <$t as cpal::FromSample<f32>>::from_sample_(if ch == 1 {
                                (s[0] + s[1]) * 0.5
                            } else {
                                v
                            });
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

    /// F7's timeline: a block ends where it arrives, follows its stream's last one
    /// when that is within 10 ms, real gaps stay; two streams mix; frames come out LAG behind, gaps silent.
    #[test]
    fn timeline_mixes_streams_on_the_game_clock() {
        assert_eq!(Timeline::place(10_000, 960, None), 9_040);
        assert_eq!(
            Timeline::place(10_000, 960, Some(9_000)),
            9_000,
            "40 frames of jitter: contiguous"
        );
        assert_eq!(
            Timeline::place(20_000, 960, Some(9_000)),
            19_040,
            "a real gap stays"
        );
        let lag = Timeline::LAG;
        let mut tl = Timeline::new(0);
        let a = vec![0.25f32; 2 * 960];
        let b = vec![0.5f32; 2 * 480];
        assert!(tl.add(1, &a, 960).is_empty(), "nothing is due before LAG");
        assert!(tl.add(2, &b, 960).is_empty());
        let out = tl.due(960 + lag);
        assert_eq!(out.len(), 2 * 960);
        assert_eq!(out[0], 0.25, "stream 1 alone");
        assert_eq!(out[2 * 600], 0.75, "both streams over 480..960");
        // nothing more appended: the next stretch is silence
        let out = tl.due(2000 + lag);
        assert_eq!(out.len(), 2 * (2000 - 960));
        assert!(out.iter().all(|&x| x == 0.0));
        // a block that lands in frames already handed over is cut, the rest kept
        let out = tl.add(1, &a, 2500);
        assert!(out.is_empty());
        let out = tl.due(3000 + lag);
        assert_eq!(out.len(), 2 * 1000);
        // too loud: limited to 0.95
        let mut tl = Timeline::new(0);
        tl.add(1, &vec![0.8f32; 2 * 480], 480);
        tl.add(2, &vec![0.8f32; 2 * 480], 480);
        let out = tl.due(480 + lag);
        assert!(out.iter().all(|&x| x <= 0.95 + 1e-6) && out[0] > 0.9);
    }
    fn st() -> Stream {
        Stream {
            name: "t".into(),
            st: Mutex::new(St {
                rate: 1.0,
                env: 1.0,
                ..Default::default()
            }),
            cv: Condvar::new(),
            hook: Mutex::new(None),
        }
    }
    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_micros(n * 1000)
    }

    /// The output measured every 5 s: a 0.5 square wave's RMS and peak; silence said once,
    /// then every 12th time (a minute); sound every 5 s while the first 60 lines last.
    #[test]
    fn rms_and_peak_line_every_5_s() {
        let mut m = Meter::default();
        let tone: Vec<f32> = (0..2 * 480)
            .map(|i| if (i / 2) % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let lines: Vec<String> = (0..500).filter_map(|_| m.add(&tone)).collect();
        assert_eq!(
            lines,
            vec![
                "audio: output, last 5 s: RMS 0.50000 / 0.50000 (-6.0 / -6.0 dBFS), peak 0.5000"
                    .to_string()
            ]
        );
        let quiet = vec![0f32; 2 * 480];
        let said = (0..500 * 24).filter_map(|_| m.add(&quiet)).count();
        assert_eq!(said, 2, "24 silent periods: said at the first and the 13th");
        assert_eq!(
            (0..500).filter_map(|_| m.add(&tone)).count(),
            1,
            "sound again: said"
        );
    }

    /// NXOS_AUDIO_DUMP's file: a WAV whose header (RIFF / data sizes, 48 kHz stereo 16-bit) is right after each block,
    /// samples as written.
    #[test]
    fn wav_dump_stays_whole() {
        let p = std::env::temp_dir().join(format!("nxos-dump-{}.wav", std::process::id()));
        let mut w = Wav::create(&p).unwrap();
        w.write(&[0.5, -0.5, 1.0, -2.0]).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(
            (&b[..4], &b[8..16], &b[36..40]),
            (&b"RIFF"[..], &b"WAVEfmt "[..], &b"data"[..])
        );
        assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 48000);
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 8);
        let s: Vec<i16> = b[44..]
            .chunks(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(s, vec![16384, -16384, 32767, -32767]);
        w.write(&[0.0; 4]).unwrap();
        let b = std::fs::read(&p).unwrap();
        assert_eq!(
            (
                b.len(),
                u32::from_le_bytes(b[4..8].try_into().unwrap()),
                u32::from_le_bytes(b[40..44].try_into().unwrap())
            ),
            (60, 52, 16)
        );
        drop(w);
        let _ = std::fs::remove_file(&p);
    }

    /// NXOS_AUDIO_DEVICE: a part of a device's name, case aside; none: the default.
    #[test]
    fn output_device_by_name() {
        let names = vec![
            "Speakers (Realtek(R) Audio)".to_string(),
            "Headphones (DualSense Wireless Controller)".to_string(),
        ];
        assert_eq!(pick_device(&names, "dualsense"), Some(1));
        assert_eq!(pick_device(&names, " Realtek"), Some(0));
        assert_eq!(pick_device(&names, "HDMI"), None);
    }

    #[test]
    fn mute_fades_to_silence_and_back() {
        let mut level = 1.0;
        let mut out = vec![0.5f32; 2 * 960];
        mute_fade(&mut level, true, &mut out);
        assert!(out[0] > 0.49 && out[0] < 0.5, "fades, no click");
        assert!(
            out[2 * 480..].iter().all(|&x| x == 0.0),
            "silent after 10 ms"
        );
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
        assert!(
            got[3000..].windows(2).all(|w| w[1] <= w[0]),
            "then the last sample fades out"
        );
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
        assert_eq!(
            st.queued, 512,
            "nothing played yet: the samples stay queued for the device"
        );
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
        assert!(
            !pace(&mut st, ms(t0, 100)),
            "starts again from the new frames"
        );
        assert!(!pace(&mut st, ms(t0, 105)));
        assert!(pace(&mut st, ms(t0, 111)));
    }
}
