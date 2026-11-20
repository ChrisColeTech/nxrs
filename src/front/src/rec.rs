//! F7: video recording. The game's own presented picture (graphics' frame sink, `nxgfx::vk::shot::set_frame_sink`:
//! read back at the present, so overlays, other windows and DWM never get in; Turbo's F7) at `recording_fps` (15..60)
//! as H.264 and the mixed
//! game sound (the os layer's output tap, 48 kHz stereo) as AAC, in an MP4 written by Media Foundation's sink writer
//! (QuickSync / NVENC / VCE when the system has them): `recordings\recording_<date>_<time>.mp4` beside the game. The
//! title bar says RECORDING while it runs.
//!
//! Video frames are numbered at the constant rate on the game's clock (`nxos::clock`): frame n shows the game at
//! n / fps seconds of game time. With `smooth_recording=1` (Turbo's virtual time) the game's clock is slowed while the
//! PC cannot keep up (the game presents, or the grab captures, fewer than `recording_fps` pictures a second of real
//! time), so every video frame is a new picture and the video plays smooth at normal speed; the clock goes back to
//! real time when the recording stops. With `smooth_recording=0` the clock is real time and a frame the grab could not
//! deliver repeats the last one. Audio and video are interleaved within a second of each other (Turbo's rec_flow.h).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
#[cfg(windows)]
use windows::core::{HSTRING, PCWSTR};
#[cfg(windows)]
use windows::Win32::Media::MediaFoundation::*;
#[cfg(windows)]
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

static ON: AtomicBool = AtomicBool::new(false);
static TOGGLE: AtomicBool = AtomicBool::new(false);
/// The recorder thread runs (until its file is finished).
static BUSY: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
/// Mixed audio from the tap, waiting to be written: (samples, rate, channels).
static AUDIO: Mutex<VecDeque<(Vec<f32>, u32, u32, f64)>> = Mutex::new(VecDeque::new());
/// The newest game picture graphics handed over, and how many came so far.
static LATEST: Mutex<(Option<crate::capture::Frame>, u64)> = Mutex::new((None, 0));

/// Graphics' frame sink (its completion thread): the presented picture, BGRA top-down.
fn game_frame(bgra: &[u8], w: u32, h: u32) {
    let mut l = LATEST.lock().unwrap();
    l.0 = Some(crate::capture::Frame { w, h, bgra: bgra.to_vec() });
    l.1 += 1;
}

/// The newest game picture after the `seen`-th (None while no newer one came).
fn newer_frame(seen: u64) -> Option<(crate::capture::Frame, u64)> {
    let mut l = LATEST.lock().unwrap();
    if l.1 > seen {
        let n = l.1;
        return l.0.take().map(|f| (f, n));
    }
    None
}

pub fn recording() -> bool {
    ON.load(Ordering::Acquire)
}

/// F7 (any thread): starts or stops at the next front-end tick.
pub fn toggle() {
    TOGGLE.store(true, Ordering::Release);
}

/// When the recorder last finished a write (ms since `epoch()`); the stall watchdog reads it.
static PROGRESS_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static STARTED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    static E: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    E.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// The encoder made no progress for 5 s since the later of the recording's start and its last finished write
/// (Turbo's recording stall watchdog, reimplemented).
pub fn stalled(now: u64, entered: u64, progress: u64) -> bool {
    now.saturating_sub(progress.max(entered)) >= 5000
}

/// From the front-end loop.
pub fn tick() {
    // the stall watchdog: a writer stuck in the encoder (Media Foundation) ends the recording, the game's clock back
    // to real time, instead of the game staying slowed down behind it
    if recording() && stalled(now_ms(), STARTED_MS.load(Ordering::Acquire), PROGRESS_MS.load(Ordering::Acquire)) {
        crate::flog!("[RED] F7: the encoder made no progress for 5 seconds; the recording stops (its file may be incomplete)");
        ON.store(false, Ordering::Release);
        nxos::media::sink::set_tap(None);
        nxgfx::vk::shot::set_frame_sink(0, None);
        nxos::clock::set_rate(1.0);
        crate::window::set_state(1, "Recording stopped: the encoder stalled");
    }
    if !TOGGLE.swap(false, Ordering::AcqRel) {
        return;
    }
    if recording() {
        // the file is finished off the front-end loop (input keeps flowing meanwhile)
        std::thread::spawn(stop);
    } else {
        start();
    }
}

fn audio_tap(s: &[f32], rate: u32, ch: u32) {
    let mut q = AUDIO.lock().unwrap();
    if q.len() < 4096 {
        // with the game's clock slowed (smooth_recording), a real second of sound holds `rate` seconds of game time
        q.push_back((s.to_vec(), rate, ch, nxos::clock::rate()));
    }
}

const AUDIO_RATE: u32 = 48000;

#[cfg(target_os = "linux")]
include!("rec_linux.rs");

#[cfg(windows)]
struct Writer {
    w: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
    width: u32,
    height: u32,
    fps: u32,
    frame: u64,
    /// Stereo frames of audio written.
    samples: u64,
}

#[cfg(windows)]
impl Writer {
    unsafe fn new(path: &std::path::Path, width: u32, height: u32, fps: u32) -> windows::core::Result<Writer> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2)?;
        let attrs = attrs.unwrap();
        attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        // interleaving is ours (write_audio): the writer must never block a stream waiting for the other
        attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
        let w = MFCreateSinkWriterFromURL(PCWSTR(HSTRING::from(path.as_os_str()).as_ptr()), None, &attrs)?;
        let size = (width as u64) << 32 | height as u64;
        let rate = (fps as u64) << 32 | 1;
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        // about 0.15 bits a pixel at 60 FPS: 1080p60 gets 19 Mbit/s
        out.SetUINT32(&MF_MT_AVG_BITRATE, ((width as u64 * height as u64 * fps as u64) / 6).clamp(4_000_000, 60_000_000) as u32)?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        out.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1 << 32 | 1)?;
        let video = w.AddStream(&out)?;
        let inp = MFCreateMediaType()?;
        inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        inp.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
        inp.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
        inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1 << 32 | 1)?;
        inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?; // top-down rows
        w.SetInputMediaType(video, &inp, None)?;
        // the sound: AAC 192 kbit/s from 48 kHz stereo floats; a system without the AAC encoder records video only
        let audio = (|| -> windows::core::Result<u32> {
            let ao = MFCreateMediaType()?;
            ao.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            ao.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            ao.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            ao.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
            ao.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            ao.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24000)?;
            let s = w.AddStream(&ao)?;
            let ai = MFCreateMediaType()?;
            ai.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            ai.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            ai.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            ai.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
            ai.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            ai.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            ai.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * 4)?;
            w.SetInputMediaType(s, &ai, None)?;
            Ok(s)
        })();
        if let Err(e) = &audio {
            crate::flog!("F7: no sound in this recording (the AAC encoder refused: {e})");
        }
        w.BeginWriting()?;
        Ok(Writer { w, video, audio: audio.ok(), width, height, fps, frame: 0, samples: 0 })
    }

    /// One frame (BGRA top-down of any size: cropped or padded to the video's).
    unsafe fn write(&mut self, f: &crate::capture::Frame) -> windows::core::Result<()> {
        let n = self.width * self.height * 4;
        let buf = MFCreateMemoryBuffer(n)?;
        let mut p: *mut u8 = std::ptr::null_mut();
        buf.Lock(&mut p, None, None)?;
        let dst = std::slice::from_raw_parts_mut(p, n as usize);
        dst.fill(0);
        let (cw, ch) = (self.width.min(f.w) as usize, self.height.min(f.h) as usize);
        for y in 0..ch {
            let s = &f.bgra[y * f.w as usize * 4..][..cw * 4];
            dst[y * self.width as usize * 4..][..cw * 4].copy_from_slice(s);
        }
        buf.Unlock()?;
        buf.SetCurrentLength(n)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(self.frame as i64 * 10_000_000 / self.fps as i64)?;
        s.SetSampleDuration(10_000_000i64 / self.fps as i64)?;
        self.w.WriteSample(self.video, &s)?;
        self.frame += 1;
        Ok(())
    }

    /// The queued sound, up to a second past the video (it waits otherwise: the writer buffers the stream ahead).
    unsafe fn write_audio(&mut self, draining: bool) -> windows::core::Result<()> {
        let Some(stream) = self.audio else {
            AUDIO.lock().unwrap().clear();
            return Ok(());
        };
        loop {
            let video_end = self.frame as f64 / self.fps as f64;
            // at most 0.1 s ahead while recording; at the end, the sound stops where the video does
            let ahead = self.samples as f64 / AUDIO_RATE as f64 - video_end;
            if ahead > if draining { 0.0 } else { 0.1 } {
                return Ok(());
            }
            let next = AUDIO.lock().unwrap().pop_front();
            let (pcm, rate, ch, speed) = match next {
                Some(x) => x,
                // the game is silent (or its sound late by half a second): silence keeps the streams together
                None if (self.samples as f64 / AUDIO_RATE as f64) < video_end - 0.5 => (vec![0f32; 2 * 960], AUDIO_RATE, 2, 1.0),
                None => return Ok(()),
            };
            if rate != AUDIO_RATE || ch == 0 {
                continue;
            }
            let src = pcm.len() / ch as usize;
            // game time: the block shortened by the clock's rate (linear resampling)
            let frames = if speed < 0.999 { ((src as f64 * speed).round() as usize).max(1) } else { src };
            if src == 0 {
                continue;
            }
            let mut b = Vec::with_capacity(frames * 4);
            for f in 0..frames {
                let at = if frames > 1 { f as f64 * (src - 1) as f64 / (frames - 1) as f64 } else { 0.0 };
                let (i, fr) = (at as usize, (at - at.floor()) as f32);
                let j = (i + 1).min(src - 1);
                for c in 0..2usize {
                    let k = c.min(ch as usize - 1);
                    let x = pcm[i * ch as usize + k] * (1.0 - fr) + pcm[j * ch as usize + k] * fr;
                    b.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                }
            }
            let buf = MFCreateMemoryBuffer(b.len() as u32)?;
            let mut p: *mut u8 = std::ptr::null_mut();
            buf.Lock(&mut p, None, None)?;
            std::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len());
            buf.Unlock()?;
            buf.SetCurrentLength(b.len() as u32)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            s.SetSampleTime((self.samples * 10_000_000 / AUDIO_RATE as u64) as i64)?;
            s.SetSampleDuration((frames as u64 * 10_000_000 / AUDIO_RATE as u64) as i64)?;
            self.w.WriteSample(stream, &s)?;
            self.samples += frames as u64;
        }
    }

    unsafe fn finish(mut self) -> windows::core::Result<()> {
        self.write_audio(true)?;
        self.w.Finalize()
    }
}

fn start() {
    let fps = nxconfig::int("recording_fps").clamp(15, 60) as u32;
    let smooth = nxconfig::flag("smooth_recording");
    let g = std::time::Instant::now();
    *LATEST.lock().unwrap() = (None, 0);
    nxgfx::vk::shot::set_frame_sink(fps, Some(game_frame));
    // the first picture: the next present's (at most 3 s: a game showing nothing records nothing)
    let mut first = None;
    while g.elapsed().as_secs_f64() < 3.0 {
        if let Some((f, _)) = newer_frame(0) {
            first = Some(f);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let first_s = g.elapsed().as_secs_f64();
    let Some(first) = first else {
        nxgfx::vk::shot::set_frame_sink(0, None);
        crate::flog!("[RED] F7: no game picture came in 3 s; no recording");
        return;
    };
    let dir = crate::game().exe_dir.join("recordings");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("recording_{}.mp4", crate::capture::stamp()));
    let (w, h) = (first.w & !1, first.h & !1);
    AUDIO.lock().unwrap().clear();
    STARTED_MS.store(now_ms(), Ordering::Release);
    PROGRESS_MS.store(0, Ordering::Release);
    ON.store(true, Ordering::Release);
    BUSY.store(true, Ordering::Release);
    let handle = std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || {
            let mut wr = match unsafe { Writer::new(&path, w, h, fps) } {
                Ok(wr) => wr,
                Err(e) => {
                    crate::flog!("[RED] F7: the H.264 writer could not start: {e}");
                    ON.store(false, Ordering::Release);
                    BUSY.store(false, Ordering::Release);
                    return;
                }
            };
            if wr.audio.is_some() {
                nxos::media::sink::set_tap(Some(audio_tap));
            }
            crate::flog!("F7: recording {w}x{h} at {fps} FPS{} to {} (smooth_recording={})", if wr.audio.is_some() { " with sound" } else { "" }, path.display(), smooth as u8);
            crate::window::set_state(1, "RECORDING (F7 to stop)");
            let g0 = nxos::clock::now_ns();
            let mut last = first;
            let (mut grabbed, mut grab_s, mut repeats, mut seen) = (0u64, 0f64, 0u64, 1u64);
            let (mut win_t, mut win_presents, mut win_grabs) = (std::time::Instant::now(), nxos::nvn::presents(), 0u64);
            // smooth: start at the rate the first grab allows, not at real time (the sound would run ahead)
            let mut rate = if smooth { (1.0 / first_s.max(1e-3) / fps as f64 * 0.95).clamp(0.05, 1.0) } else { 1.0 };
            if smooth {
                nxos::clock::set_rate(rate);
            }
            let mut min_rate = rate;
            while ON.load(Ordering::Acquire) {
                // the next frame's game time
                let due = g0 + wr.frame * 1_000_000_000 / fps as u64;
                while nxos::clock::now_ns() < due && ON.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                let t = std::time::Instant::now();
                let presents = nxos::nvn::presents();
                if let Some((f, n)) = newer_frame(seen) {
                    seen = n;
                    // a picture of another size (the game changed its resolution) is left out: the file keeps one
                    if (f.w & !1, f.h & !1) == (w, h) {
                        last = f;
                    }
                    grabbed += 1;
                    win_grabs += 1;
                    grab_s += t.elapsed().as_secs_f64();
                }
                let res = unsafe { wr.write(&last).and_then(|_| wr.write_audio(false)) };
                if let Err(e) = res {
                    crate::flog!("[RED] F7: writing failed: {e}");
                    ON.store(false, Ordering::Release);
                    break;
                }
                PROGRESS_MS.store(now_ms(), Ordering::Release);
                // behind real time without smooth recording: repeat the picture so the video keeps the game's speed
                if !smooth {
                    while (nxos::clock::now_ns() - g0) * fps as u64 / 1_000_000_000 > wr.frame {
                        if unsafe { wr.write(&last) }.is_err() {
                            break;
                        }
                        repeats += 1;
                    }
                }
                // smooth recording: twice a second, the clock's rate from what the PC delivers per real second
                let el = win_t.elapsed().as_secs_f64();
                if smooth && el >= 0.5 {
                    let real_presents = (presents - win_presents) as f64 / el / rate.max(0.05);
                    let real_grabs = win_grabs as f64 / el / rate.max(0.05);
                    let can = real_presents.min(real_grabs) / fps as f64;
                    let want = (can * 0.95).clamp(0.05, 1.0);
                    // attack fast, release slow
                    rate = if want < rate { want } else { rate * 0.8 + want * 0.2 };
                    nxos::clock::set_rate(rate);
                    min_rate = min_rate.min(rate);
                    win_t = std::time::Instant::now();
                    win_presents = presents;
                    win_grabs = 0;
                }
            }
            nxos::media::sink::set_tap(None);
            nxgfx::vk::shot::set_frame_sink(0, None);
            nxos::clock::set_rate(1.0);
            let (frames, samples) = (wr.frame, wr.samples);
            match unsafe { wr.finish() } {
                Ok(()) => crate::flog!(
                    "F7: saved {} ({frames} frames, {:.1} s video, {:.1} s sound, {grabbed} grabbed at {:.1} ms, {repeats} repeated, slowest clock rate {:.2})",
                    path.display(),
                    frames as f64 / fps as f64,
                    samples as f64 / AUDIO_RATE as f64,
                    if grabbed > 0 { grab_s * 1000.0 / grabbed as f64 } else { 0.0 },
                    min_rate
                ),
                Err(e) => crate::flog!("[RED] F7: finishing {} failed: {e}", path.display()),
            }
            crate::window::set_state(1, "Video saved in the recordings folder");
            BUSY.store(false, Ordering::Release);
        })
        .ok();
    *THREAD.lock().unwrap() = handle;
}

/// Stops and finalises the file (F7 again, or the exit).
pub fn stop() {
    ON.store(false, Ordering::Release);
    // another stop may be finishing the file already: wait for it (at most 20 s)
    for _ in 0..2000 {
        if !BUSY.load(Ordering::Acquire) || THREAD.lock().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let t = THREAD.lock().unwrap().take();
    if let Some(h) = t {
        let _ = h.join();
    }
}
#[cfg(test)]
mod tests {
    /// Five seconds without a finished write, counted from the later of the start and the last write.
    #[test]
    fn stall_after_five_seconds_without_progress() {
        assert!(!super::stalled(4_999, 0, 0));
        assert!(super::stalled(5_000, 0, 0));
        assert!(!super::stalled(12_000, 10_000, 0), "counted from the start");
        assert!(!super::stalled(12_000, 0, 8_000), "a write 4 s ago");
        assert!(super::stalled(13_000, 0, 8_000));
        assert!(!super::stalled(100, 5_000, 0), "a clock before the start is no stall");
    }
}