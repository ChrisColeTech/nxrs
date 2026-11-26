//! (diagnosis, `NXGFX_UPLOAD_TRACE=1`) What the pool sync uploads: bytes a frame by pool, and how many of them were
//! the same as the page's last upload (CPU-written pages whose contents did not change). Logged with the pool summary.

use std::collections::HashMap;
use std::sync::Mutex;

pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_UPLOAD_TRACE").as_deref() == Ok("1"))
}

#[derive(Default)]
struct Trace {
    /// page -> hash of its bytes at its last upload
    last: HashMap<u64, u64>,
    /// pool base -> (pool size, bytes uploaded, of them unchanged, ranges, lowest and highest page written)
    pools: HashMap<u64, (u64, u64, u64, u64, u64, u64)>,
    /// GPU-copy changes that dropped `upskip` hashes: (address, bytes) -> times
    forgets: HashMap<(u64, u64), u64>,
}

/// `upskip::forget` of [a, a + n) (a GPU write, another upload path).
pub fn note_forget(a: u64, n: u64) {
    let mut t = T.lock().unwrap();
    *t.get_or_insert_with(Trace::default).forgets.entry((a, n)).or_default() += 1;
}

static T: Mutex<Option<Trace>> = Mutex::new(None);

const PAGE: u64 = 4096;

/// [a, a + n) of the pool at `base` (`size` bytes) uploaded now (identity-mapped guest memory).
pub fn note(base: u64, size: u64, a: u64, n: u64) {
    let mut t = T.lock().unwrap();
    let t = t.get_or_insert_with(Trace::default);
    let mut same = 0u64;
    let mut p = a;
    while p < a + n {
        let e = ((p / PAGE + 1) * PAGE).min(a + n);
        // SAFETY: the bytes the upload copies from (committed guest memory of a registered pool)
        let b = unsafe { core::slice::from_raw_parts(p as *const u8, (e - p) as usize) };
        let h = super::queue::memo_hash(b) as u64 ^ (e - p) << 48;
        if t.last.insert(p, h) == Some(h) {
            same += e - p;
        }
        p = e;
    }
    let s = t.pools.entry(base).or_insert((size, 0, 0, 0, u64::MAX, 0));
    s.1 += n;
    s.2 += same;
    s.3 += 1;
    s.4 = s.4.min(a);
    s.5 = s.5.max(a + n);
}

/// The pools that uploaded most since the last report, per frame.
pub fn report(frames: f64) -> String {
    let mut t = T.lock().unwrap();
    let Some(t) = t.as_mut() else {
        return String::new();
    };
    let mut v: Vec<_> = t.pools.drain().collect();
    v.sort_by_key(|(_, s)| std::cmp::Reverse(s.1));
    let total: u64 = v.iter().map(|(_, s)| s.1).sum();
    let same: u64 = v.iter().map(|(_, s)| s.2).sum();
    let mb = |b: u64| b as f64 / frames / 1048576.0;
    let mut out = format!(
        "; upload trace a frame: {:.2} MB, {:.2} MB of it unchanged since its last upload; by pool:",
        mb(total),
        mb(same)
    );
    for (base, s) in v.iter().take(8) {
        out += &format!(
            " [{base:#x}+{:#x}: {:.2} MB ({:.2} unchanged), {:.0} ranges, {:#x}..{:#x}]",
            s.0,
            mb(s.1),
            mb(s.2),
            s.3 as f64 / frames,
            s.4,
            s.5
        );
    }
    let mut f: Vec<_> = t.forgets.drain().collect();
    f.sort_by_key(|((_, n), c)| std::cmp::Reverse(n * c));
    out += "; hashes dropped most (address+bytes x a frame):";
    for ((a, n), c) in f.iter().take(8) {
        out += &format!(" {a:#x}+{n:#x} x{:.1}", *c as f64 / frames);
    }
    out
}
