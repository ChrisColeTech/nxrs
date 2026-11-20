// The F7 writer on Linux (included by rec.rs): the FFmpeg command-line encoder, as Windows uses Media Foundation's
// sink writer. Video frames go to an `ffmpeg` process as raw BGRA on its stdin and come out as H.264 (libx264, else
// libopenh264, else NVENC / VA-API: the first this FFmpeg has) in `<file>.video.mp4`; the sound goes to `<file>.pcm`
// (48 kHz stereo s16); `finish` muxes both into `<file>` with the sound as AAC 192 kbit/s and removes the parts. The
// clock, frame numbering and interleaving rules are rec.rs's own (`write_audio` keeps the sound within 0.1 s of the
// video).

struct Writer {
    child: std::process::Child,
    video_pipe: Option<std::process::ChildStdin>,
    audio: Option<std::io::BufWriter<std::fs::File>>,
    path: std::path::PathBuf,
    video_path: std::path::PathBuf,
    audio_path: std::path::PathBuf,
    width: u32,
    height: u32,
    fps: u32,
    frame: u64,
    /// Stereo frames of audio written.
    samples: u64,
    row: Vec<u8>,
}

/// The H.264 encoders this machine's ffmpeg has, the best first (`ffmpeg -encoders`, asked once).
fn h264_encoder() -> Result<&'static str, String> {
    static E: std::sync::OnceLock<Result<&'static str, String>> = std::sync::OnceLock::new();
    E.get_or_init(|| {
        let out = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("ffmpeg is not installed or not on PATH ({e}): recordings need it on Linux"))?;
        let list = String::from_utf8_lossy(&out.stdout);
        ["libx264", "libopenh264", "h264_nvenc", "h264_vaapi"]
            .into_iter()
            .find(|e| list.lines().any(|l| l.split_whitespace().nth(1) == Some(e)))
            .ok_or_else(|| "this ffmpeg has no H.264 encoder (libx264, libopenh264, NVENC or VA-API)".to_string())
    })
    .clone()
}

impl Writer {
    unsafe fn new(path: &std::path::Path, width: u32, height: u32, fps: u32) -> Result<Writer, String> {
        let enc = h264_encoder()?;
        let video_path = path.with_extension("video.mp4");
        let audio_path = path.with_extension("pcm");
        // about 0.15 bits a pixel at 60 FPS: 1080p60 gets 19 Mbit/s (as the Media Foundation writer)
        let bitrate = ((width as u64 * height as u64 * fps as u64) / 6).clamp(4_000_000, 60_000_000);
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgra"])
            .args(["-s", &format!("{width}x{height}"), "-r", &fps.to_string(), "-i", "pipe:0"]);
        match enc {
            "h264_vaapi" => {
                cmd.args(["-vaapi_device", "/dev/dri/renderD128", "-vf", "format=nv12,hwupload", "-c:v", enc]);
            }
            "libx264" => {
                cmd.args(["-c:v", enc, "-preset", "veryfast", "-pix_fmt", "yuv420p"]);
            }
            _ => {
                cmd.args(["-c:v", enc, "-pix_fmt", "yuv420p"]);
            }
        }
        cmd.args(["-b:v", &bitrate.to_string()]).arg(&video_path);
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null());
        let mut child = cmd.spawn().map_err(|e| format!("ffmpeg: {e}"))?;
        let video_pipe = child.stdin.take();
        let audio = match std::fs::File::create(&audio_path) {
            Ok(f) => Some(std::io::BufWriter::new(f)),
            Err(e) => {
                crate::flog!("F7: no sound in this recording ({}: {e})", audio_path.display());
                None
            }
        };
        crate::flog!("F7: encoder ffmpeg {enc}");
        Ok(Writer { child, video_pipe, audio, path: path.to_path_buf(), video_path, audio_path, width, height, fps, frame: 0, samples: 0, row: Vec::new() })
    }

    /// One frame (BGRA top-down of any size: cropped or padded to the video's).
    unsafe fn write(&mut self, f: &crate::capture::Frame) -> Result<(), String> {
        use std::io::Write;
        let n = (self.width * self.height * 4) as usize;
        self.row.clear();
        self.row.resize(n, 0);
        let (cw, ch) = (self.width.min(f.w) as usize, self.height.min(f.h) as usize);
        for y in 0..ch {
            let s = &f.bgra[y * f.w as usize * 4..][..cw * 4];
            self.row[y * self.width as usize * 4..][..cw * 4].copy_from_slice(s);
        }
        let p = self.video_pipe.as_mut().ok_or("the encoder is closed")?;
        p.write_all(&self.row).map_err(|e| format!("the encoder stopped taking frames ({e})"))?;
        self.frame += 1;
        Ok(())
    }

    /// The queued sound, up to 0.1 s past the video (the same rule as the Media Foundation writer).
    unsafe fn write_audio(&mut self, draining: bool) -> Result<(), String> {
        use std::io::Write;
        let Some(out) = self.audio.as_mut() else {
            AUDIO.lock().unwrap().clear();
            return Ok(());
        };
        loop {
            let video_end = self.frame as f64 / self.fps as f64;
            let ahead = self.samples as f64 / AUDIO_RATE as f64 - video_end;
            if ahead > if draining { 0.0 } else { 0.1 } {
                return Ok(());
            }
            let next = AUDIO.lock().unwrap().pop_front();
            let (pcm, rate, ch, speed) = match next {
                Some(x) => x,
                None if (self.samples as f64 / AUDIO_RATE as f64) < video_end - 0.5 => (vec![0f32; 2 * 960], AUDIO_RATE, 2, 1.0),
                None => return Ok(()),
            };
            if rate != AUDIO_RATE || ch == 0 {
                continue;
            }
            let src = pcm.len() / ch as usize;
            if src == 0 {
                continue;
            }
            let frames = if speed < 0.999 { ((src as f64 * speed).round() as usize).max(1) } else { src };
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
            out.write_all(&b).map_err(|e| format!("{}: {e}", self.audio_path.display()))?;
            self.samples += frames as u64;
        }
    }

    unsafe fn finish(mut self) -> Result<(), String> {
        use std::io::Write;
        self.write_audio(true)?;
        drop(self.video_pipe.take());
        let st = self.child.wait().map_err(|e| e.to_string())?;
        if !st.success() {
            return Err(format!("ffmpeg ended with {st}"));
        }
        let have_audio = match self.audio.take() {
            Some(mut a) => a.flush().is_ok() && self.samples > 0,
            None => false,
        };
        let r = if have_audio {
            let st = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
                .arg(&self.video_path)
                .args(["-f", "s16le", "-ar", &AUDIO_RATE.to_string(), "-ac", "2", "-i"])
                .arg(&self.audio_path)
                .args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart"])
                .arg(&self.path)
                .stdin(std::process::Stdio::null())
                .status()
                .map_err(|e| e.to_string())?;
            if st.success() {
                Ok(())
            } else {
                Err(format!("muxing the sound in failed ({st})"))
            }
        } else {
            std::fs::rename(&self.video_path, &self.path).map_err(|e| e.to_string())
        };
        if r.is_ok() {
            let _ = std::fs::remove_file(&self.video_path);
        }
        let _ = std::fs::remove_file(&self.audio_path);
        r
    }
}
