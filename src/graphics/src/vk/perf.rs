//! Replay counters, GPU time, frame telemetry and the per-frame log lines.
//!
//! - Counters of the replay's work (draws and the reasons a draw is skipped, passes, dispatches, clears, copies,
//!   descriptor sets, write-backs, pipelines and the time making them) and of work dropped on an error path, always
//!   counted (one relaxed add each).
//! - GPU time: a timestamp at the start and the end of every submission, read at its completion.
//! - Every 60 frames: `[vk] GPU time up to frame N: …` and `[vk] replay up to frame N, per frame: …`; every 600
//!   frames the sizes the game renders at (`[vk] render sizes …`); a frame longer than `NXGFX_SLOW_MS` (50) gets a
//!   `[vk] slow frame` line saying where its time went.
//! - `NXGFX_FRAME_CSV=<path>`: a row per frame (present interval, replay CPU time, GPU time, pipeline and upload
//!   time, write-back bytes, dropped work), written by a background thread from a bounded ring; a slow disk loses
//!   rows (counted in the log), never frames.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum C {
    Draws,
    DrawsSkipped,
    SkipNoShader,
    SkipNoPipeline,
    SkipNoTarget,
    Passes,
    Dispatches,
    Clears,
    TexCopies,
    BufCopies,
    DescSets,
    Writebacks,
    PipesGfx,
    PipesCompute,
    PipeNs,
    PipeMaxNs,
    GpuNs,
    GpuSubs,
    ReplayNs,
    TexUploadNs,
    // dropped or degraded work
    TexNoFormat,
    TexCreateFailed,
    ViewFailed,
    ShaderUnusable,
    DrawPrim,
    AppendReset,
    SbufSize0,
    GmemLearned,
    TfCaptures,
    N,
}

const NAMES: [&str; C::N as usize] = [
    "draws",
    "draws_skipped",
    "skip_no_shader",
    "skip_no_pipeline",
    "skip_no_target",
    "passes",
    "dispatches",
    "clears",
    "tex_copies",
    "buf_copies",
    "desc_sets",
    "writebacks",
    "pipelines_gfx",
    "pipelines_compute",
    "pipeline_ns",
    "pipeline_max_ns",
    "gpu_ns",
    "gpu_subs",
    "replay_ns",
    "tex_upload_ns",
    "tex_no_format",
    "tex_create_failed",
    "view_failed",
    "shader_unusable",
    "draw_prim_unsupported",
    "append_counter_reset",
    "sbuf_size0",
    "gmem_pages_learned",
    "tf_captures",
];

static COUNT: [AtomicU64; C::N as usize] = [const { AtomicU64::new(0) }; C::N as usize];

#[inline]
pub fn add(c: C, n: u64) {
    COUNT[c as usize].fetch_add(n, Ordering::Relaxed);
}
#[inline]
pub fn inc(c: C) {
    add(c, 1);
}
pub fn get(c: C) -> u64 {
    COUNT[c as usize].load(Ordering::Relaxed)
}
pub fn max(c: C, v: u64) {
    COUNT[c as usize].fetch_max(v, Ordering::Relaxed);
}
/// Every counter by name (the metrics and the harness).
pub fn snapshot() -> Vec<(&'static str, u64)> {
    (0..C::N as usize).map(|i| (NAMES[i], COUNT[i].load(Ordering::Relaxed))).collect()
}

/// A submission's GPU time (completion thread).
pub fn gpu_time(ns: u64) {
    add(C::GpuNs, ns);
    inc(C::GpuSubs);
}

/// Times `f` into counter `c`.
pub fn timed<R>(c: C, f: impl FnOnce() -> R) -> R {
    let t = std::time::Instant::now();
    let r = f();
    add(c, t.elapsed().as_nanos() as u64);
    r
}

// ---------- render sizes ----------
static RSZ: Mutex<Vec<(u32, u32, u64, u64)>> = Mutex::new(Vec::new());

/// A pass of `w` x `h` (with a depth target or not).
pub fn render_size(w: u32, h: u32, depth: bool) {
    let mut v = RSZ.lock().unwrap();
    if let Some(e) = v.iter_mut().find(|e| e.0 == w && e.1 == h) {
        e.2 += 1;
        e.3 += depth as u64;
    } else if v.len() < 24 {
        v.push((w, h, 1, depth as u64));
    }
}

fn render_sizes_line(frame: u64, frames: u64) -> Option<String> {
    let mut v = std::mem::take(&mut *RSZ.lock().unwrap());
    if v.is_empty() {
        return None;
    }
    let (mw, mh) = v.iter().filter(|e| e.3 > 0).map(|e| (e.0, e.1)).max_by_key(|e| e.0 as u64 * e.1 as u64).unwrap_or((0, 0));
    v.sort_by(|a, b| b.2.cmp(&a.2));
    let f = frames.max(1) as f64;
    let parts: Vec<String> = v.iter().take(8).map(|e| format!("{}x{} x{:.1} ({:.1} with depth)", e.0, e.1, e.2 as f64 / f, e.3 as f64 / f)).collect();
    Some(format!("render sizes up to frame {frame}, passes per frame: {}; largest with depth {mw}x{mh}", parts.join(", ")))
}

// ---------- frame telemetry ----------
struct Row {
    t_ns: u64,
    frame: u64,
    interval: u64,
    replay: u64,
    gpu: u64,
    pipeline: u64,
    texture: u64,
    wb_bytes: u64,
    dropped: u64,
}
struct Ring {
    rows: std::collections::VecDeque<Row>,
    lost: u64,
}
static RING: Mutex<Option<Ring>> = Mutex::new(None);

fn csv_start() {
    let Ok(p) = std::env::var("NXGFX_FRAME_CSV") else { return };
    let f = match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
        Ok(f) => f,
        Err(e) => {
            crate::log::red(&format!("cannot create frame telemetry {p}: {e}"));
            return;
        }
    };
    *RING.lock().unwrap() = Some(Ring { rows: Default::default(), lost: 0 });
    crate::vklog!("frame telemetry: {p}; bounded ring, background writer");
    std::thread::Builder::new()
        .name("vk frame telemetry".into())
        .spawn(move || {
            use std::io::Write;
            let mut w = std::io::BufWriter::new(f);
            let _ = writeln!(w, "frame,unix_ns,interval_ns,replay_ns,gpu_ns,pipeline_ns,texture_ns,writeback_bytes,dropped");
            let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
            let t0 = crate::nvn::device::now_ns();
            let mut lost = 0;
            loop {
                let (rows, l) = {
                    let mut g = RING.lock().unwrap();
                    let r = g.as_mut().unwrap();
                    (std::mem::take(&mut r.rows), r.lost)
                };
                for r in rows {
                    let _ = writeln!(w, "{},{},{},{},{},{},{},{},{}", r.frame, epoch + r.t_ns - t0.min(r.t_ns), r.interval, r.replay, r.gpu, r.pipeline, r.texture, r.wb_bytes, r.dropped);
                }
                if l != lost {
                    crate::vklog!("frame telemetry: {l} rows lost (ring full)");
                    lost = l;
                }
                if w.flush().is_err() {
                    crate::log::red("frame telemetry write failed");
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        })
        .ok();
}

fn dropped_total() -> u64 {
    (C::TexNoFormat as usize..C::N as usize).map(|i| COUNT[i].load(Ordering::Relaxed)).sum()
}

struct FrameState {
    started: bool,
    t_last: u64,
    prev: [u64; C::N as usize],
    prev_frame: u64,
    slow: [u64; C::N as usize],
    rsz_frame: u64,
    slow_lines: u64,
}
static FRAME: Mutex<FrameState> = Mutex::new(FrameState { started: false, t_last: 0, prev: [0; C::N as usize], prev_frame: 0, slow: [0; C::N as usize], rsz_frame: 0, slow_lines: 0 });

fn slow_ms() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NXGFX_SLOW_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(50))
}

/// Every present (frame number from 1).
pub fn frame(frame: u64) {
    let now = crate::nvn::device::now_ns();
    let cur: [u64; C::N as usize] = core::array::from_fn(|i| COUNT[i].load(Ordering::Relaxed));
    let mut st = FRAME.lock().unwrap();
    if !st.started {
        st.started = true;
        csv_start();
        st.prev = cur;
        st.slow = cur;
        st.prev_frame = frame;
        st.rsz_frame = frame;
        st.t_last = now;
        return;
    }
    let interval = now - st.t_last;
    st.t_last = now;
    let d = |c: C, base: &[u64; C::N as usize]| cur[c as usize] - base[c as usize];
    if let Some(r) = RING.lock().unwrap().as_mut() {
        if r.rows.len() < 4096 {
            r.rows.push_back(Row {
                t_ns: now,
                frame,
                interval,
                replay: d(C::ReplayNs, &st.slow),
                gpu: d(C::GpuNs, &st.slow),
                pipeline: d(C::PipeNs, &st.slow),
                texture: d(C::TexUploadNs, &st.slow),
                wb_bytes: super::mem::WRITEBACK_BYTES.load(Ordering::Relaxed),
                dropped: dropped_total(),
            });
        } else {
            r.lost += 1;
        }
    }
    if interval > slow_ms() * 1_000_000 && st.slow_lines < 20_000 {
        st.slow_lines += 1;
        crate::vklog!(
            "slow frame {frame}: {:.0} ms: replay {:.1} ms, GPU {:.1} ms ({} submissions), pipelines {:.1} ms ({} made), texture uploads {:.1} ms, {} draws",
            interval as f64 / 1e6,
            d(C::ReplayNs, &st.slow) as f64 / 1e6,
            d(C::GpuNs, &st.slow) as f64 / 1e6,
            d(C::GpuSubs, &st.slow),
            d(C::PipeNs, &st.slow) as f64 / 1e6,
            d(C::PipesGfx, &st.slow) + d(C::PipesCompute, &st.slow),
            d(C::TexUploadNs, &st.slow) as f64 / 1e6,
            d(C::Draws, &st.slow)
        );
    }
    st.slow = cur;
    if frame % 60 != 0 || frame == st.prev_frame {
        return;
    }
    let df = (frame - st.prev_frame).max(1) as f64;
    let p = st.prev;
    let subs = d(C::GpuSubs, &p);
    if subs > 0 {
        crate::vklog!(
            "GPU time up to frame {frame}: {:.2} ms per frame ({:.2} ms per submission, {:.1} submissions per frame); replay on the CPU {:.2} ms per frame",
            d(C::GpuNs, &p) as f64 / 1e6 / df,
            d(C::GpuNs, &p) as f64 / 1e6 / subs as f64,
            subs as f64 / df,
            d(C::ReplayNs, &p) as f64 / 1e6 / df
        );
    }
    crate::vklog!(
        "replay up to frame {frame}, per frame: {:.1} draws ({:.1} skipped: no shader {}, no pipeline {}, no target {} in all), {:.1} passes, {:.1} dispatches, {:.1} clears, {:.1} texture copies, {:.1} buffer copies, {:.1} descriptor sets, {:.1} write-backs; {} graphics + {} compute pipelines ({:.1} ms creating, worst {:.1} ms); dropped work {}",
        d(C::Draws, &p) as f64 / df,
        d(C::DrawsSkipped, &p) as f64 / df,
        cur[C::SkipNoShader as usize],
        cur[C::SkipNoPipeline as usize],
        cur[C::SkipNoTarget as usize],
        d(C::Passes, &p) as f64 / df,
        d(C::Dispatches, &p) as f64 / df,
        d(C::Clears, &p) as f64 / df,
        d(C::TexCopies, &p) as f64 / df,
        d(C::BufCopies, &p) as f64 / df,
        d(C::DescSets, &p) as f64 / df,
        d(C::Writebacks, &p) as f64 / df,
        cur[C::PipesGfx as usize],
        cur[C::PipesCompute as usize],
        cur[C::PipeNs as usize] as f64 / 1e6,
        cur[C::PipeMaxNs as usize] as f64 / 1e6,
        dropped_total()
    );
    st.prev = cur;
    st.prev_frame = frame;
    if frame - st.rsz_frame >= 600 {
        if let Some(l) = render_sizes_line(frame, frame - st.rsz_frame) {
            crate::vklog!("{l}");
        }
        st.rsz_frame = frame;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn render_sizes_are_listed_most_used_first() {
        super::render_size(1920, 1080, true);
        super::render_size(1920, 1080, true);
        super::render_size(64, 64, false);
        let l = super::render_sizes_line(600, 1).unwrap();
        assert!(l.starts_with("render sizes up to frame 600, passes per frame: 1920x1080 x2.0 (2.0 with depth), 64x64"), "{l}");
        assert!(l.ends_with("largest with depth 1920x1080"));
    }
}
