//! The live translator: a shader the game sets that the build did not translate (its key is not in `<data>/shaders`)
//! is translated at run time, on a thread of its own, into `<data>/shaders/live/<key>/` (the shaders crate's
//! `translate_one`: the same translator, fixes and validation as the build).
//!
//! A draw whose shaders have not arrived waits for them, once per set of programs and at most `NXGFX_SHADER_WAIT_MS`
//! (3000; 0: skipped at once), instead of being skipped: a skipped draw is not always made up for by the next frame
//! (Mario Kart 8 Deluxe draws parts of a course's lighting once as the course loads; skipped, the race stayed black).
//! While a draw waits, missing entries are looked for at every poll, else twice a second.

use crate::nvn::prog::{key_hex, ShaderKey};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

struct St {
    queued: HashSet<ShaderKey>,
    q: VecDeque<(ShaderKey, Vec<u8>, Vec<u8>)>,
    missed_at: HashMap<ShaderKey, Instant>,
    failed: HashSet<ShaderKey>,
    started: bool,
}
static ST: Mutex<Option<St>> = Mutex::new(None);
static CV: Condvar = Condvar::new();
static DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

fn dir() -> Option<PathBuf> {
    DIR.get_or_init(|| Mutex::new(None)).lock().unwrap().clone()
}

/// Where live translations go (`<data>/shaders/live`).
pub fn set_dir(d: PathBuf) {
    *DIR.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(d);
}

pub fn wait_ms() -> u64 {
    static V: OnceLock<u64> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_SHADER_WAIT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000)
    })
}

/// A shader set by the game whose key the library lacks: queued for translation (once per key).
pub fn request(key: ShaderKey, ctl: Vec<u8>, code: Vec<u8>) {
    if std::env::var("NXGFX_LIVE").as_deref() == Ok("0") {
        return;
    }
    let mut g = ST.lock().unwrap();
    let st = g.get_or_insert_with(|| St {
        queued: HashSet::new(),
        q: VecDeque::new(),
        missed_at: HashMap::new(),
        failed: HashSet::new(),
        started: false,
    });
    if !st.queued.insert(key) {
        return;
    }
    st.missed_at.insert(key, Instant::now());
    st.q.push_back((key, ctl, code));
    if !st.started {
        st.started = true;
        // as many translators as the build uses (NXGFX_LIVE_JOBS, 4)
        let n = std::env::var("NXGFX_LIVE_JOBS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4usize)
            .max(1);
        for i in 0..n {
            std::thread::Builder::new()
                .name(format!("live shader translator {i}"))
                .spawn(worker)
                .ok();
        }
    }
    CV.notify_one();
}

/// A draw waits for `keys`: they go to the front of the queue.
pub fn prioritize(keys: &[ShaderKey]) {
    let mut g = ST.lock().unwrap();
    let Some(st) = g.as_mut() else { return };
    for k in keys {
        if let Some(i) = st.q.iter().position(|j| &j.0 == k) {
            let j = st.q.remove(i).unwrap();
            st.q.push_front(j);
        }
    }
}

fn worker() {
    loop {
        let (key, ctl, code) = {
            let mut g = ST.lock().unwrap();
            loop {
                if let Some(j) = g.as_mut().and_then(|s| s.q.pop_front()) {
                    break j;
                }
                g = CV.wait(g).unwrap();
            }
        };
        let Some(d) = dir() else {
            crate::log::red_once("live shader translator: no shader folder set");
            continue;
        };
        let t0 = Instant::now();
        let hex = key_hex(&key);
        match shaders::prebuild::translate_one(&ctl, &code, &d) {
            Ok(k) => {
                let since = ST
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|s| s.missed_at.get(&key).copied())
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                if k != hex {
                    crate::log::red(&format!("live shader translator: key {k} for the game's {hex} (the two key functions differ)"));
                }
                crate::vklog!(
                    "live shader {hex} translated in {:.2} s ({since:.1} s after it was missed)",
                    t0.elapsed().as_secs_f64()
                );
            }
            Err(e) => {
                if let Some(s) = ST.lock().unwrap().as_mut() {
                    s.failed.insert(key);
                }
                crate::log::red(&format!("live shader {hex}: {e}"));
            }
        }
    }
}

/// Whether the live translator gave up on `key` (a draw stops waiting for it).
pub fn failed(key: &ShaderKey) -> bool {
    ST.lock()
        .unwrap()
        .as_ref()
        .is_some_and(|s| s.failed.contains(key))
}

/// Where a live translation of `key` is (`<live>/<key>/spv`), when it exists.
pub fn spv_dir(key: &ShaderKey) -> Option<PathBuf> {
    let d = dir()?.join(key_hex(key)).join("spv");
    d.is_dir().then_some(d)
}

static WAITED: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// A frame's shader waits end after this many ms (five new shader sets in one frame of a race were a
/// stop of 2.3 s): the rest of the frame's draws that lack shaders are skipped and looked at again in the next frame.
/// `NXGFX_SHADER_WAIT_FRAME_MS` (default 1500; 0: no cap).
pub fn frame_cap_ms() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NXGFX_SHADER_WAIT_FRAME_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500)
    })
}

/// (frame, ms waited in it so far)
static FRAME_WAIT: Mutex<(u64, u64)> = Mutex::new((0, 0));

/// How long a wait may take now in frame `frame`: the per-wait limit `ms`, within what is left of the frame's cap
/// (0: none left).
pub fn budget(frame: u64, ms: u64, cap: u64) -> u64 {
    let f = FRAME_WAIT.lock().unwrap();
    let used = if f.0 == frame { f.1 } else { 0 };
    if cap == 0 {
        ms
    } else {
        ms.min(cap.saturating_sub(used))
    }
}

/// `waited` ms were spent waiting in frame `frame`.
pub fn spent(frame: u64, waited: u64) {
    let mut f = FRAME_WAIT.lock().unwrap();
    if f.0 != frame {
        *f = (frame, 0);
    }
    f.1 += waited;
}

/// A draw that lacks shaders it needs: waits (once per set of keys) for the live translator, calling `look` every
/// 5 ms until it answers true or the wait runs out. Returns whether they came.
pub fn wait_for(keys: &[ShaderKey], mut look: impl FnMut() -> bool) -> bool {
    let ms = wait_ms();
    if ms == 0 || keys.is_empty() || keys.iter().all(failed) {
        return false;
    }
    let mut h = 1469598103934665603u64;
    for k in keys {
        for b in k {
            h = (h ^ *b as u64).wrapping_mul(1099511628211);
        }
    }
    if WAITED
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .contains(&h)
    {
        return false; // waited for once: a shader that never comes is skipped from then on
    }
    // the frame's waits so far: none left, the draw is skipped and the set waited for in a later frame
    let frame = super::present::frames();
    let ms = budget(frame, ms, frame_cap_ms());
    if ms == 0 {
        prioritize(keys);
        return false;
    }
    WAITED
        .lock()
        .unwrap()
        .get_or_insert_with(HashSet::new)
        .insert(h);
    prioritize(keys);
    let t0 = Instant::now();
    let mut came = false;
    while t0.elapsed() < Duration::from_millis(ms) {
        if look() {
            came = true;
            break;
        }
        if keys.iter().all(failed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    spent(frame, t0.elapsed().as_millis() as u64);
    static TOTAL: Mutex<(u64, f64, Option<Instant>)> = Mutex::new((0, 0.0, None));
    let mut t = TOTAL.lock().unwrap();
    t.0 += 1;
    t.1 += t0.elapsed().as_secs_f64();
    if t.2.is_none_or(|s| s.elapsed() > Duration::from_secs(2)) {
        t.2 = Some(Instant::now());
        crate::vklog!(
            "draws that waited for their shaders from the live translator so far: {}, {:.1} s in all (the last one {:.0} ms, {})",
            t.0,
            t.1,
            t0.elapsed().as_secs_f64() * 1e3,
            if came { "they came" } else { "not all came: skipped" }
        );
    }
    came
}

#[cfg(test)]
mod tests {
    /// A frame's waits end at its cap; the next frame has the whole cap again.
    #[test]
    fn a_frames_shader_waits_are_capped() {
        let f = 1_000_000_007;
        assert_eq!(
            super::budget(f, 3000, 1500),
            1500,
            "one wait at most the frame's cap"
        );
        super::spent(f, 1200);
        assert_eq!(super::budget(f, 3000, 1500), 300, "what is left of it");
        super::spent(f, 400);
        assert_eq!(
            super::budget(f, 3000, 1500),
            0,
            "none left: skipped this frame"
        );
        assert_eq!(super::budget(f + 1, 3000, 1500), 1500, "the next frame");
        assert_eq!(super::budget(f, 3000, 0), 3000, "no cap");
    }
}
