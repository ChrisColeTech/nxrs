//! Fault-triggered captures (`NXGFX_FAULT_SHOTS=<dir>`, off by default): a one-frame fault is captured every time it
//! happens, not found by luck in a sparse screenshot.
//!
//! Each present is copied to host memory (`super::copy_presented`, like the F7 frame sink). When the copy has landed
//! a sparse sample of it (about 480 columns) is compared with the previous present's: a trip is a large area that
//! turned fully white (every channel >= 250) or near black (luma < 16 from >= 48) in one frame. A trip, or an anomaly
//! the engine noted since the last present (`note`: every red line, a zero stand-in bound, a draw skipped for want of
//! a render target), writes that present at full resolution and the previous one at the sample's resolution to the
//! directory, with a log line `fault capture N: <why> at present P: <files>`; the present after a white/black trip
//! logs how much of the new area was gone again (a one-frame flash, or a lasting change such as a scene cut).
//!
//! - `NXGFX_FAULT_SHOTS=<dir>`: on (`1`: `captures/faults`);
//! - `NXGFX_FAULT_FLASH=<white>,<black>`: the fractions of the picture that trip (default `0.02,0.25`; 0 = off);
//! - `NXGFX_FAULT_MAX=<n>`: captures per run (default 32; at most `NXGFX_FAULT_PER_REASON`, default 4, per reason).
//!
//! Cost when on: one copy of the presented picture per frame on the GPU and a sparse sample of it on the completion
//! thread (about 0.2 ms at 1080p); a capture converts and writes its pictures on a thread of its own.

use super::super::Gpu;
use crate::nvn::obj::tex_t;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

struct Cfg {
    dir: PathBuf,
    white: f32,
    black: f32,
    max: u32,
}

/// Captures per reason (`NXGFX_FAULT_PER_REASON`, default 4): OoT's forest fog trips the white-flash check four times
/// before Link's house, where the one-frame white props come (OOT-FLASH).
fn per_reason() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("NXGFX_FAULT_PER_REASON")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4)
    })
}
/// sample columns across the picture
const SAMPLE_W: u32 = 480;

fn cfg() -> Option<&'static Cfg> {
    static C: OnceLock<Option<Cfg>> = OnceLock::new();
    C.get_or_init(|| {
        let v = std::env::var("NXGFX_FAULT_SHOTS").ok().filter(|v| !v.is_empty() && v != "0")?;
        let dir = if v == "1" { PathBuf::from("captures/faults") } else { PathBuf::from(v) };
        let _ = std::fs::create_dir_all(&dir);
        let mut f = std::env::var("NXGFX_FAULT_FLASH").unwrap_or_default().split(',').map(|s| s.trim().parse::<f32>().ok()).collect::<Vec<_>>();
        f.resize(2, None);
        let c = Cfg {
            dir,
            white: f[0].unwrap_or(0.02),
            black: f[1].unwrap_or(0.25),
            max: std::env::var("NXGFX_FAULT_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(32),
        };
        crate::vklog!(
            "fault captures to {}: anomalies and one-frame flashes (white {} / black {} of the picture), at most {} (NXGFX_FAULT_SHOTS)",
            c.dir.display(),
            c.white,
            c.black,
            c.max
        );
        Some(c)
    })
    .as_ref()
}

pub fn on() -> bool {
    cfg().is_some()
}

/// The previous present's sample and what a trip left to follow up.
struct Prev {
    frame: u64,
    w: u32,
    h: u32,
    /// RGBA8, `w` x `h`, top row first
    px: Vec<u8>,
}

#[derive(Default)]
struct State {
    /// (reason, the present that shows the work it was noted in, more notes since)
    pending: Option<(String, u64, u32)>,
    prev: Option<Prev>,
    taken: u32,
    per_reason: HashMap<String, u32>,
    /// a white/black trip's capture number and kind: the next present says how much of it is gone again
    follow: Option<(u32, bool)>,
}

static ST: Mutex<Option<State>> = Mutex::new(None);
static NOTES: AtomicU64 = AtomicU64::new(0);

/// An anomaly the engine noticed while translating or running the current frame (any thread). Never call it with
/// the fault state locked (it is not reentrant); it logs nothing itself.
pub fn note(kind: &str, detail: &str) {
    if !on() || crate::log::muted() {
        return;
    }
    NOTES.fetch_add(1, Ordering::Relaxed);
    let mut s = ST.lock().unwrap();
    let s = s.get_or_insert_with(State::default);
    match &mut s.pending {
        Some((_, _, more)) => *more += 1,
        None => {
            let mut d: String = detail.chars().take(200).collect();
            d.retain(|c| c != '\n');
            s.pending = Some((format!("{kind}: {d}"), super::super::present::frames() + 1, 0));
        }
    }
}

/// A draw skipped because none of its render targets could be bound: a context line (present, submission, pixel
/// shader, each target's texture) for the first 16 and then every power of two, and a fault note.
pub fn no_target<'a>(
    what: &str,
    colour: impl Iterator<Item = Option<&'a tex_t>>,
    depth: Option<&tex_t>,
    pixel: Option<crate::nvn::prog::ShaderKey>,
    submission: u64,
) {
    if crate::log::muted() {
        return;
    }
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    let desc = |t: &tex_t| {
        format!(
            "ser{} {}x{}x{} fmt {:#x} target {}",
            t.serial, t.d.width, t.d.height, t.d.depth, t.d.format, t.d.target
        )
    };
    let detail = || {
        let c: Vec<String> = colour
            .enumerate()
            .map(|(i, t)| format!("colour {i} {}", t.map(desc).unwrap_or_else(|| "none".into())))
            .collect();
        format!(
            "after present {}, submission {submission}, pixel shader {}: {}; depth {}",
            super::super::present::frames(),
            pixel.map(|k| crate::nvn::prog::key_hex(&k)).unwrap_or_else(|| "none".into()),
            if c.is_empty() { "no colour slots".into() } else { c.join(", ") },
            depth.map(desc).unwrap_or_else(|| "none".into())
        )
    };
    if n <= 16 || n.is_power_of_two() {
        let d = detail();
        crate::vklog!("no render target: draw skipped #{n} ({what}): {d}");
        note("no-target", &d);
    } else if on() {
        note("no-target", what);
    }
}

/// In a present's submission (`super::record_sink`): the presented picture copied for the watcher when it is on.
pub(crate) fn record(g: &Gpu, sub: &mut super::super::queue::Sub, t: &tex_t, crop: Option<[i32; 4]>, flip: bool) {
    let Some(c) = cfg() else {
        return;
    };
    let frame = super::super::present::frames();
    let Some(rb) = super::copy_presented(g, sub, t, crop) else {
        return;
    };
    sub.post
        .push(super::super::queue::Post::Callback(Box::new(move |_g: &Gpu| {
            // SAFETY: the read-back buffer, freed after this callback (`Sub::transient`)
            let src = unsafe { core::slice::from_raw_parts(rb.ptr as *const u8, rb.n) };
            landed(c, &rb, src, flip, frame);
        })));
}

/// One texel of `src` (row `y` of the picture as shown, column `x`) as RGBA8.
fn texel(rb: &super::Readback, src: &[u8], flip: bool, x: u32, y: u32) -> Option<[u8; 4]> {
    let sy = if flip { rb.h - 1 - y } else { y };
    let o = (sy as usize * rb.w as usize + x as usize) * rb.bpb;
    let mut d = [0u8; 4];
    super::to_rgba8(rb.fmt, src.get(o..o + rb.bpb)?, 1, &mut d).then_some(d)
}

fn sample(rb: &super::Readback, src: &[u8], flip: bool) -> Option<(u32, u32, Vec<u8>)> {
    let step = (rb.w / SAMPLE_W).max(1);
    let (w, h) = (rb.w / step, rb.h / step);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&texel(rb, src, flip, x * step, y * step)?);
        }
    }
    Some((w, h, px))
}

fn white(p: &[u8]) -> bool {
    p[0] >= 250 && p[1] >= 250 && p[2] >= 250
}

fn luma(p: &[u8]) -> u32 {
    (p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10
}

/// (fraction newly white, fraction newly near black, fraction of `prev`'s white that is not white now).
fn compare(prev: &[u8], cur: &[u8]) -> (f32, f32, f32) {
    let (mut nw, mut nb, mut pw, mut gone) = (0u32, 0u32, 0u32, 0u32);
    for (a, b) in prev.chunks_exact(4).zip(cur.chunks_exact(4)) {
        let (wa, wb) = (white(a), white(b));
        nw += (wb && !wa) as u32;
        nb += (luma(b) < 16 && luma(a) >= 48) as u32;
        pw += wa as u32;
        gone += (wa && !wb) as u32;
    }
    let n = (cur.len() / 4).max(1) as f32;
    (nw as f32 / n, nb as f32 / n, if pw > 0 { gone as f32 / pw as f32 } else { 0.0 })
}

/// Of `trip`'s near-black samples, the fraction that is light again (luma >= 48) in `next`.
fn dark_gone(trip: &[u8], next: &[u8]) -> f32 {
    let (mut dark, mut light) = (0u32, 0u32);
    for (a, b) in trip.chunks_exact(4).zip(next.chunks_exact(4)) {
        if luma(a) < 16 {
            dark += 1;
            light += (luma(b) >= 48) as u32;
        }
    }
    if dark > 0 {
        light as f32 / dark as f32
    } else {
        0.0
    }
}

fn landed(c: &'static Cfg, rb: &super::Readback, src: &[u8], flip: bool, frame: u64) {
    let Some((w, h, px)) = sample(rb, src, flip) else {
        return;
    };
    let mut guard = ST.lock().unwrap();
    let st = guard.get_or_insert_with(State::default);
    let mut reason = None;
    if let Some(prev) = st.prev.as_ref().filter(|p| p.w == w && p.h == h) {
        let (nw, nb, gone) = compare(&prev.px, &px);
        if let Some((n, was_white)) = st.follow.take() {
            let g = if was_white { gone } else { dark_gone(&prev.px, &px) };
            crate::vklog!(
                "fault capture {n}: at the next present {frame}, {:.0}% of the {} area is gone again ({})",
                g * 100.0,
                if was_white { "white" } else { "dark" },
                if g > 0.5 { "a one-frame flash" } else { "it stays" }
            );
        }
        if c.white > 0.0 && nw >= c.white {
            reason = Some((format!("flash-white: {:.1}% of the picture turned fully white", nw * 100.0), Some(true)));
        } else if c.black > 0.0 && nb >= c.black {
            reason = Some((format!("flash-black: {:.1}% of the picture turned near black", nb * 100.0), Some(false)));
        }
    }
    if reason.is_none() {
        if let Some((why, due, more)) = st.pending.take() {
            if frame >= due {
                let why = if more > 0 { format!("{why} (+{more} more notes)") } else { why };
                reason = Some((why, None));
            } else {
                st.pending = Some((why, due, more));
            }
        }
    }
    let prev = st.prev.replace(Prev { frame, w, h, px });
    let Some((why, flash)) = reason else {
        return;
    };
    // per reason: its kind and the start of its text, numbers left out (each red line kind gets its own captures)
    let key: String = why.chars().filter(|c| !c.is_ascii_digit()).take(48).collect();
    let key = if key.starts_with("flash-") { key[..11].to_string() } else { key };
    let per = st.per_reason.entry(key).or_insert(0);
    if st.taken >= c.max || *per >= per_reason() {
        return;
    }
    *per += 1;
    st.taken += 1;
    let n = st.taken;
    if let Some(f) = flash {
        st.follow = Some((n, f));
    }
    drop(guard);
    // the present itself at full resolution (converted here: its buffer goes after this callback)
    let (fw, fh) = (rb.w, rb.h);
    let mut full = vec![0u8; (fw * fh * 4) as usize];
    let ok = super::to_rgba8(rb.fmt, src, (fw * fh) as usize, &mut full);
    let cur_path = c.dir.join(format!("fault-{n:02}-p{frame}.png"));
    let prev_path = prev.as_ref().map(|p| c.dir.join(format!("fault-{n:02}-p{}-before.png", p.frame)));
    crate::vklog!(
        "fault capture {n}: {why} at present {frame}: {} ({fw}x{fh}){}",
        cur_path.display(),
        prev_path
            .as_ref()
            .map(|p| format!(", the present before: {} ({w}x{h})", p.display()))
            .unwrap_or_default()
    );
    std::thread::spawn(move || {
        if ok {
            if flip {
                let row = fw as usize * 4;
                for y in 0..fh as usize / 2 {
                    let (a, b) = full.split_at_mut((fh as usize - 1 - y) * row);
                    a[y * row..(y + 1) * row].swap_with_slice(&mut b[..row]);
                }
            }
            for p in full.chunks_exact_mut(4) {
                p[3] = 255;
            }
            let _ = super::png_write(&cur_path, &full, fw, fh);
        }
        if let (Some(p), Some(path)) = (prev, prev_path) {
            let mut px = p.px;
            for q in px.chunks_exact_mut(4) {
                q[3] = 255;
            }
            let _ = super::png_write(&path, &px, p.w, p.h);
        }
    });
}

/// Notes taken so far (tests and the control pipe's counters).
pub fn notes() -> u64 {
    NOTES.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_new_white_area_and_its_disappearance_are_measured() {
        let grey = [100u8, 100, 100, 255];
        let wht = [255u8, 255, 255, 255];
        let prev: Vec<u8> = [grey; 10].concat();
        let mut cur = prev.clone();
        cur[..8].copy_from_slice(&[wht, wht].concat());
        let (nw, nb, gone) = super::compare(&prev, &cur);
        assert_eq!((nw, nb, gone), (0.2, 0.0, 0.0));
        // the frame after: the white is gone again
        let (_, _, gone) = super::compare(&cur, &prev);
        assert_eq!(gone, 1.0);
        let black: Vec<u8> = [[0u8, 0, 0, 255]; 10].concat();
        assert_eq!(super::compare(&prev, &black).1, 1.0);
        assert_eq!(super::dark_gone(&black, &prev), 1.0);
        assert_eq!(super::dark_gone(&black, &black), 0.0);
    }
}
