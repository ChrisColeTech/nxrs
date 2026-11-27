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
    // where the replay's CPU time goes (nanoseconds, nested timers overlap)
    PrepNs,
    SetNs,
    ResolveNs,
    TexViewNs,
    PipeLookNs,
    PassNs,
    VtxNs,
    UploadsSame,
    ImagesReused,
    PoolSyncNs,
    HashNs,
    BankNs,
    SetKeyNs,
    SetWriteNs,
    // the game's threads blocked on the GPU side (nvnSyncWait, the window's acquire, nvnQueueFinish)
    WaitSyncNs,
    WaitAcquireNs,
    WaitFinishNs,
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
    SetsReused,
    VinSkipped,
    PoolSnapshots,
    PoolSnapshotReused,
    PoolSnapshotNs,
    PoolSnapshotBytes,
    PoolSnapshotDeviceReadBytes,
    PoolPrepareScans,
    PoolPrepareImages,
    /// stage sets made from the stage's last set with only uniform buffers / push constants written again
    SetsPatched,
    /// inline uniform buffer updates (UPDATE_UB), and those that ended an open render pass
    UbUpdates,
    UbPassEnds,
    /// written storage buffers bound with no size (a pushed plain address): noted as GPU-written over their whole
    /// binding (up to `NXGFX_SBUF0_MB`) at every submission
    SbufSize0Written,
    /// (SET-FAST) dynamic states and set offsets not set again: the command buffer holds those values already
    DynSkipped,
    /// (MKW-PIPE) the game's threads blocked before queueing an op (`aq_wait_room`): the replay 63 ops behind, the
    /// presents queued at their limit; nvnSyncWait waiting for a command-buffer fence to be published
    WaitRoomNs,
    WaitRoomPresentNs,
    WaitPublishNs,
    /// the replay thread's time in its ops (busy), and waiting for one (idle)
    AqBusyNs,
    AqIdleNs,
    /// parked streams run on at once without a pool sync of their own (`queue::chain_parked`)
    Chained,
    /// stage sets patched from a seed: a copy the segment's stream began with (`exec::set_seed_on`)
    SetsSeeded,
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
    "prep_ns",
    "set_ns",
    "resolve_ns",
    "tex_view_ns",
    "pipe_lookup_ns",
    "pass_ns",
    "vertex_ns",
    "uploads_same_bytes",
    "images_reused",
    "pool_sync_ns",
    "hash_ns",
    "bank_ns",
    "set_key_ns",
    "set_write_ns",
    "wait_sync_ns",
    "wait_acquire_ns",
    "wait_finish_ns",
    "tex_no_format",
    "tex_create_failed",
    "view_failed",
    "shader_unusable",
    "draw_prim_unsupported",
    "append_counter_reset",
    "sbuf_size0",
    "gmem_pages_learned",
    "tf_captures",
    "sets_reused",
    "vertex_input_skipped",
    "pool_snapshots",
    "pool_snapshot_reused",
    "pool_snapshot_ns",
    "pool_snapshot_bytes",
    "pool_snapshot_device_read_bytes",
    "pool_prepare_scans",
    "pool_prepare_images",
    "sets_patched",
    "uniform_updates",
    "uniform_update_pass_ends",
    "sbuf_size0_written",
    "dyn_skipped",
    "wait_room_ns",
    "wait_room_present_ns",
    "wait_publish_ns",
    "aq_busy_ns",
    "aq_idle_ns",
    "parked_chained",
    "sets_seeded",
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
    (0..C::N as usize)
        .map(|i| (NAMES[i], COUNT[i].load(Ordering::Relaxed)))
        .collect()
}

/// Graphics' columns of the per-frame metrics CSV (`frames-*.csv`, os `frame_row`): the header part.
pub const FRAME_COLUMNS: &str = "draws,draws_skipped,passes,dispatches,gpu_ms,replay_ms,acquire_wait_ms,tex_upload_ms,pipelines_made,prep_ms,pool_sync_ms,descriptor_ms,pipeline_ms";

/// Graphics' columns of one row: what the counters moved since the previous row (one row a present).
pub fn frame_columns() -> String {
    const CS: [C; 13] = [
        C::Draws,
        C::DrawsSkipped,
        C::Passes,
        C::Dispatches,
        C::GpuNs,
        C::ReplayNs,
        C::WaitAcquireNs,
        C::TexUploadNs,
        C::PipesGfx,
        C::PrepNs,
        C::PoolSyncNs,
        C::SetNs,
        C::PipeNs,
    ];
    static LAST: std::sync::Mutex<[u64; 13]> = std::sync::Mutex::new([0; 13]);
    let mut l = LAST.lock().unwrap();
    let mut out = String::new();
    for (i, c) in CS.iter().enumerate() {
        let mut v = get(*c);
        if matches!(c, C::PipesGfx) {
            v += get(C::PipesCompute);
        }
        let d = v.saturating_sub(l[i]);
        l[i] = v;
        if i > 0 {
            out.push(',');
        }
        if matches!(
            c,
            C::GpuNs
                | C::ReplayNs
                | C::WaitAcquireNs
                | C::TexUploadNs
                | C::PrepNs
                | C::PoolSyncNs
                | C::SetNs
                | C::PipeNs
        ) {
            out += &format!("{:.3}", d as f64 / 1e6);
        } else {
            out += &d.to_string();
        }
    }
    out
}

/// A submission's GPU time (completion thread).
pub fn gpu_time(ns: u64) {
    add(C::GpuNs, ns);
    inc(C::GpuSubs);
    FRAME_GPU.fetch_add(ns, Ordering::Relaxed);
}

/// GPU time of the submissions finished since the last `frame_gpu_take` (the frame step's present tick: a frame's
/// GPU cost; nxnative takes the latest finished frame's GPU time).
static FRAME_GPU: AtomicU64 = AtomicU64::new(0);

/// The GPU ns finished since the last call (0: none).
pub fn frame_gpu_take() -> u64 {
    FRAME_GPU.swap(0, Ordering::Relaxed)
}

/// The fine-grained timers inside the descriptor work (memory lookups, views, banks, keys, writes): thousands a
/// frame, a clock read each side (~1-2 ms a frame in Pokemon Legends Z-A). `NXGFX_PROFILE=1` turns them on.
pub fn fine() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NXGFX_PROFILE").as_deref() == Ok("1"))
}

/// Times `f` into counter `c` when the fine timers are on (`fine`).
#[inline]
pub fn timed_fine<R>(c: C, f: impl FnOnce() -> R) -> R {
    if fine() {
        timed(c, f)
    } else {
        f()
    }
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
    let (mw, mh) = v
        .iter()
        .filter(|e| e.3 > 0)
        .map(|e| (e.0, e.1))
        .max_by_key(|e| e.0 as u64 * e.1 as u64)
        .unwrap_or((0, 0));
    v.sort_by(|a, b| b.2.cmp(&a.2));
    let f = frames.max(1) as f64;
    let parts: Vec<String> = v
        .iter()
        .take(8)
        .map(|e| {
            format!(
                "{}x{} x{:.1} ({:.1} with depth)",
                e.0,
                e.1,
                e.2 as f64 / f,
                e.3 as f64 / f
            )
        })
        .collect();
    Some(format!(
        "render sizes up to frame {frame}, passes per frame: {}; largest with depth {mw}x{mh}",
        parts.join(", ")
    ))
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
    let Ok(p) = std::env::var("NXGFX_FRAME_CSV") else {
        return;
    };
    let f = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
    {
        Ok(f) => f,
        Err(e) => {
            crate::log::red(&format!("cannot create frame telemetry {p}: {e}"));
            return;
        }
    };
    *RING.lock().unwrap() = Some(Ring {
        rows: Default::default(),
        lost: 0,
    });
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

/// Where the replay's time went over `frames` frames, ms per frame: submission set-up (dirty pools and textures),
/// descriptor sets (with the memory lookups and texture views inside them), pipeline lookups, passes, vertex input,
/// texture uploads.
fn breakdown(cur: &[u64; C::N as usize], base: &[u64; C::N as usize], frames: f64) -> String {
    let d = |c: C| (cur[c as usize] - base[c as usize]) as f64 / 1e6 / frames;
    format!(
        "prep {:.2} (pools {:.2}, texture hashes {:.2}), sets {:.2} (memory {:.2}, views {:.2}, banks {:.2}, keys {:.2}, writes {:.2}), pipeline lookups {:.2}, passes {:.2}, vertex input {:.2}, uploads {:.2} ms, {:.1} uploads skipped (same bytes), {:.1} images reused, pool snapshots {:.2} ms ({:.1} made, {:.1} reused), pool preparation {:.1} passes / {:.1} images",
        d(C::PrepNs),
        d(C::PoolSyncNs),
        d(C::HashNs),
        d(C::SetNs),
        d(C::ResolveNs),
        d(C::TexViewNs),
        d(C::BankNs),
        d(C::SetKeyNs),
        d(C::SetWriteNs),
        d(C::PipeLookNs),
        d(C::PassNs),
        d(C::VtxNs),
        d(C::TexUploadNs),
        (cur[C::UploadsSame as usize] - base[C::UploadsSame as usize]) as f64 / frames,
        (cur[C::ImagesReused as usize] - base[C::ImagesReused as usize]) as f64 / frames,
        d(C::PoolSnapshotNs),
        (cur[C::PoolSnapshots as usize] - base[C::PoolSnapshots as usize]) as f64 / frames,
        (cur[C::PoolSnapshotReused as usize] - base[C::PoolSnapshotReused as usize]) as f64 / frames,
        (cur[C::PoolPrepareScans as usize] - base[C::PoolPrepareScans as usize]) as f64 / frames,
        (cur[C::PoolPrepareImages as usize] - base[C::PoolPrepareImages as usize]) as f64 / frames
    )
}

fn dropped_total() -> u64 {
    // the counters of work not done (texture formats .. unsupported primitives), not the plain counts after them
    (C::TexNoFormat as usize..=C::DrawPrim as usize)
        .map(|i| COUNT[i].load(Ordering::Relaxed))
        .sum()
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
static FRAME: Mutex<FrameState> = Mutex::new(FrameState {
    started: false,
    t_last: 0,
    prev: [0; C::N as usize],
    prev_frame: 0,
    slow: [0; C::N as usize],
    rsz_frame: 0,
    slow_lines: 0,
});

fn slow_ms() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_SLOW_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50)
    })
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
            "slow frame {frame}: {:.0} ms: replay {:.1} ms ({}), GPU {:.1} ms ({} submissions), pipelines {:.1} ms ({} made), texture uploads {:.1} ms, {} draws",
            interval as f64 / 1e6,
            d(C::ReplayNs, &st.slow) as f64 / 1e6,
            breakdown(&cur, &st.slow, 1.0),
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
            "GPU time up to frame {frame}: {:.2} ms per frame ({:.2} ms per submission, {:.1} submissions per frame); replay on the CPU {:.2} ms per frame ({}); the game waited {:.2} ms per frame (sync {:.2}, acquire {:.2}, finish {:.2})",
            d(C::GpuNs, &p) as f64 / 1e6 / df,
            d(C::GpuNs, &p) as f64 / 1e6 / subs as f64,
            subs as f64 / df,
            d(C::ReplayNs, &p) as f64 / 1e6 / df,
            breakdown(&cur, &p, df),
            (d(C::WaitSyncNs, &p) + d(C::WaitAcquireNs, &p) + d(C::WaitFinishNs, &p)) as f64 / 1e6 / df,
            d(C::WaitSyncNs, &p) as f64 / 1e6 / df,
            d(C::WaitAcquireNs, &p) as f64 / 1e6 / df,
            d(C::WaitFinishNs, &p) as f64 / 1e6 / df
        );
        crate::vklog!(
            "pipelining up to frame {frame}, per frame: the game's threads waited for room in the replay queue {:.2} ms (ops {:.2}, presents {:.2}) and for fence publication {:.2} ms; the replay thread busy {:.2} ms, idle {:.2} ms; {:.1} parked streams run on at once without a pool sync of their own (NXGFX_PARK_CHAIN)",
            (d(C::WaitRoomNs, &p) + d(C::WaitRoomPresentNs, &p)) as f64 / 1e6 / df,
            d(C::WaitRoomNs, &p) as f64 / 1e6 / df,
            d(C::WaitRoomPresentNs, &p) as f64 / 1e6 / df,
            d(C::WaitPublishNs, &p) as f64 / 1e6 / df,
            d(C::AqBusyNs, &p) as f64 / 1e6 / df,
            d(C::AqIdleNs, &p) as f64 / 1e6 / df,
            d(C::Chained, &p) as f64 / df
        );
    }
    crate::vklog!(
        "replay up to frame {frame}, per frame: {:.1} draws ({:.1} skipped: no shader {}, no pipeline {}, no target {} in all), {:.1} passes, {:.1} dispatches, {:.1} clears, {:.1} texture copies, {:.1} buffer copies, {:.1} descriptor sets ({:.1} reused, {:.1} patched, {:.1} of them from a seed), {:.1} vertex inputs unchanged, {:.1} dynamic states / set offsets unchanged, {:.1} write-backs, {:.1} uniform updates ({:.1} ending a pass), {:.1} written storage buffers with no size; {} graphics + {} compute pipelines ({:.1} ms creating, worst {:.1} ms; {} linked from parts, {} of them optimized since, {} shader parts made ahead); dropped work {}",
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
        d(C::SetsReused, &p) as f64 / df,
        d(C::SetsPatched, &p) as f64 / df,
        d(C::SetsSeeded, &p) as f64 / df,
        d(C::VinSkipped, &p) as f64 / df,
        d(C::DynSkipped, &p) as f64 / df,
        d(C::Writebacks, &p) as f64 / df,
        d(C::UbUpdates, &p) as f64 / df,
        d(C::UbPassEnds, &p) as f64 / df,
        d(C::SbufSize0Written, &p) as f64 / df,
        cur[C::PipesGfx as usize],
        cur[C::PipesCompute as usize],
        cur[C::PipeNs as usize] as f64 / 1e6,
        cur[C::PipeMaxNs as usize] as f64 / 1e6,
        super::Gpu::try_get().map(|g| g.pipes.linked.load(Ordering::Relaxed)).unwrap_or(0),
        super::Gpu::try_get().map(|g| g.pipes.optimized.load(Ordering::Relaxed)).unwrap_or(0),
        super::Gpu::try_get().map(|g| g.pipes.prebuilt.load(Ordering::Relaxed)).unwrap_or(0),
        dropped_total()
    );
    st.prev = cur;
    st.prev_frame = frame;
    if frame - st.rsz_frame >= 600 {
        if let Some(l) = render_sizes_line(frame, frame - st.rsz_frame) {
            crate::vklog!("{l}");
        }
        if let Some(l) = super::image::upload_stats::line(frame - st.rsz_frame) {
            crate::vklog!("{l}");
        }
        crate::vklog!("{}", memory_line());
        st.rsz_frame = frame;
    }
}

#[cfg(test)]
mod tests {
    /// One row of graphics' frame columns: as many values as the header names, the counters' moves since the last row.
    #[test]
    fn frame_columns_are_deltas_matching_the_header() {
        let _ = super::frame_columns();
        super::add(super::C::Draws, 7);
        super::add(super::C::GpuNs, 2_500_000);
        super::add(super::C::PoolSyncNs, 3_500_000);
        let row = super::frame_columns();
        let f: Vec<&str> = row.split(',').collect();
        assert_eq!(f.len(), super::FRAME_COLUMNS.split(',').count());
        assert!(f[0].parse::<u64>().unwrap() >= 7, "draws {row}");
        assert!(f[4].parse::<f64>().unwrap() >= 2.5, "gpu_ms {row}");
        assert!(f[10].parse::<f64>().unwrap() >= 3.5, "pool_sync_ms {row}");
    }
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

/// Where memory is (every 600 frames): the process's private bytes, the allocator's blocks and allocations by name,
/// images, parked images, staging kept by recycled submissions, and imported / mirrored pools.
pub fn memory_line() -> String {
    let Some(g) = super::Gpu::try_get() else {
        return String::new();
    };
    let private = private_mb();
    let rep = g.alloc.lock().unwrap().generate_report();
    let mut by: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    for a in &rep.allocations {
        let e = by.entry(a.name.clone()).or_default();
        e.0 += 1;
        e.1 += a.size;
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    let top: Vec<String> = v
        .iter()
        .take(8)
        .map(|(n, (c, b))| format!("{n} {c} = {} MB", b >> 20))
        .collect();
    let (ni, parked) = g.image_counts();
    format!(
        "memory: private {private} MB; allocator {} MB in blocks, {} MB allocated ({}); this layer's system memory {} MB ({} refused over its cap); images {ni}, parked {parked}, {} registered without one yet; replay queue {} ops behind, {} submits replayed with the one before",
        rep.total_reserved_bytes >> 20,
        rep.total_allocated_bytes >> 20,
        top.join(", "),
        super::HOST_LIVE.load(std::sync::atomic::Ordering::Relaxed) >> 20,
        super::HOST_REFUSED.load(std::sync::atomic::Ordering::Relaxed),
        super::image::lazy_waiting(),
        super::queue::aq_backlog(),
        super::queue::AQ_MERGED.load(std::sync::atomic::Ordering::Relaxed)
    )
}

#[cfg(windows)]
fn private_mb() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { core::mem::zeroed() };
    c.cb = core::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb) };
    if ok != 0 {
        c.PrivateUsage as u64 >> 20
    } else {
        0
    }
}
#[cfg(target_os = "linux")]
fn private_mb() -> u64 {
    let (rss, swap) = nxrt::linux::process_memory();
    (rss + swap) >> 20
}
/// The web: the linear memory's current size.
#[cfg(target_os = "emscripten")]
fn private_mb() -> u64 {
    extern "C" {
        fn emscripten_get_heap_size() -> usize;
    }
    // SAFETY: a plain size read
    (unsafe { emscripten_get_heap_size() } as u64) >> 20
}
