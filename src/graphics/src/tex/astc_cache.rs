//! The ASTC -> BC7 disk cache (Turbo `vk_astc.cpp`'s pack): every BC7 level the GPU encoded goes into one append-only
//! pack file, so a texture is encoded once, ever, and later runs upload the BC7 blocks straight from it.
//!
//! Keyed by the ASTC bytes, the shape, block size, sRGB and the encoder's version (`ENCODER`): nothing here knows the
//! game. Two files in the game's cache folder (`set_dir`, the game's `_data`): `astc_bc7.pack` holds the blocks and
//! `astc_bc7.idx` 32 bytes per entry, written after its blocks, so a killed game loses at most its last entry. A fully
//! opaque level is not compacted (BC7 has no separate alpha half to drop, unlike Turbo's BC3 pack).
//!
//! Writes go to a thread of their own; reads happen where the upload is recorded (the replay thread), only for levels
//! big enough that a read beats an encode (`MIN_BYTES`). `NXGFX_ASTC_CACHE=0` off, `NXGFX_ASTC_CACHE_MB` the most the
//! pack holds (default 8192).

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

/// Changes whenever the BC7 encoder's output changes: every older entry then misses.
pub const ENCODER: u64 = 0x4e58_4243_3700_0002; // "NXBC7", revision 2 (modes 1, 3, 6, 7)
/// Levels smaller than this are encoded every time (a 256x256 level: 64 KiB of BC7).
pub const MIN_BYTES: usize = 64 << 10;
const PACK: &str = "astc_bc7.pack";
const INDEX: &str = "astc_bc7.idx";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Key(pub u64, pub u64);

#[derive(Clone, Copy)]
struct Ent {
    hi: u64,
    off: u64,
    size: u32,
}

struct Pack {
    dir: PathBuf,
    data: File,
    map: HashMap<u64, Ent>,
    end: u64,
    max: u64,
}

static PACK_: Mutex<Option<Pack>> = Mutex::new(None);
static WRITER: OnceLock<Mutex<Sender<(Key, Vec<u8>)>>> = OnceLock::new();
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);
static PUTS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn mix(a: u64, b: u64) -> u64 {
    let m = a as u128 * b as u128;
    (m as u64) ^ ((m >> 64) as u64)
}
fn hash64(p: &[u8], seed: u64) -> u64 {
    let mut h = seed ^ (p.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut ch = p.chunks_exact(16);
    for c in &mut ch {
        let a = u64::from_le_bytes(c[..8].try_into().unwrap());
        let b = u64::from_le_bytes(c[8..].try_into().unwrap());
        h = mix(a ^ 0xa076_1d64_78bd_642f ^ h, b ^ 0xe703_7ed1_a0b4_28db);
    }
    let mut t = [0u8; 16];
    t[..ch.remainder().len()].copy_from_slice(ch.remainder());
    let (a, b) = (
        u64::from_le_bytes(t[..8].try_into().unwrap()),
        u64::from_le_bytes(t[8..].try_into().unwrap()),
    );
    mix(
        h ^ a ^ 0x8ebc_6af0_9c88_c6e3,
        b ^ 0x5899_65cc_7537_4cc3 ^ p.len() as u64,
    )
}

/// The key of one level's ASTC blocks.
pub fn key(blocks: &[u8], w: u32, h: u32, slices: u32, bw: u32, bh: u32, srgb: bool) -> Key {
    let shape = w as u64
        | (h as u64) << 20
        | (slices as u64) << 40
        | (bw as u64) << 52
        | (bh as u64) << 56
        | (srgb as u64) << 60;
    Key(
        hash64(blocks, ENCODER ^ shape),
        hash64(blocks, !ENCODER ^ shape.wrapping_mul(0x2545_F491_4F6C_DD1D)),
    )
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXGFX_ASTC_CACHE").as_deref() != Ok("0"))
}

fn open(dir: &Path) -> std::io::Result<Pack> {
    std::fs::create_dir_all(dir)?;
    let data = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(PACK))?;
    let mut idx = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(INDEX))?;
    let dn = data.metadata()?.len();
    let mut raw = Vec::new();
    idx.read_to_end(&mut raw)?;
    let n = raw.len() / 32;
    let mut map = HashMap::with_capacity(n * 2 + 64);
    let mut end = 0u64;
    for r in raw.chunks_exact(32) {
        let w = |i: usize| u64::from_le_bytes(r[i * 8..i * 8 + 8].try_into().unwrap());
        let (lo, hi, off, size) = (w(0), w(1), w(2), w(3) as u32);
        if off + size as u64 > dn || (w(3) >> 32) != ENCODER & 0xffff_ffff {
            continue;
        }
        map.insert(lo, Ent { hi, off, size });
        end = end.max(off + size as u64);
    }
    // a torn last record is cut off
    idx.set_len((n * 32) as u64)?;
    let max = std::env::var("NXGFX_ASTC_CACHE_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(8192)
        << 20;
    crate::vklog!(
        "ASTC -> BC7 cache {}: {} levels, {} MB",
        dir.display(),
        map.len(),
        end >> 20
    );
    Ok(Pack {
        dir: dir.to_path_buf(),
        data,
        map,
        end,
        max,
    })
}

/// Where the pack lives (the game's cache folder). Before this, nothing is cached.
pub fn set_dir(dir: &Path) {
    if !enabled() {
        return;
    }
    let mut p = PACK_.lock().unwrap();
    if p.as_ref().is_some_and(|p| p.dir == dir) {
        return;
    }
    match open(dir) {
        Ok(x) => *p = Some(x),
        Err(e) => crate::vklog!(
            "ASTC -> BC7 cache {}: cannot open ({e}), encoding as it comes",
            dir.display()
        ),
    }
}

/// Closes the pack (tests switch folders).
pub fn close() {
    flush();
    *PACK_.lock().unwrap() = None;
}

/// The cached BC7 blocks of `k` (`size` bytes), if the pack holds them.
pub fn get(k: Key, size: usize) -> Option<Vec<u8>> {
    let mut g = PACK_.lock().unwrap();
    let p = g.as_mut()?;
    let Some(e) = p
        .map
        .get(&k.0)
        .copied()
        .filter(|e| e.hi == k.1 && e.size as usize == size)
    else {
        MISSES.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let mut out = vec![0u8; size];
    let ok = p.data.seek(SeekFrom::Start(e.off)).is_ok() && p.data.read_exact(&mut out).is_ok();
    if !ok {
        MISSES.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    HITS.fetch_add(1, Ordering::Relaxed);
    Some(out)
}

/// True when the pack is open (a miss is then worth reading back and putting).
pub fn active() -> bool {
    PACK_.lock().unwrap().is_some()
}

fn write_one(k: Key, data: &[u8]) {
    let mut g = PACK_.lock().unwrap();
    let Some(p) = g.as_mut() else { return };
    if p.map.contains_key(&k.0) || p.end + data.len() as u64 > p.max {
        return;
    }
    let off = p.end;
    if p.data.seek(SeekFrom::Start(off)).is_err() || p.data.write_all(data).is_err() {
        return;
    }
    let mut rec = [0u8; 32];
    rec[..8].copy_from_slice(&k.0.to_le_bytes());
    rec[8..16].copy_from_slice(&k.1.to_le_bytes());
    rec[16..24].copy_from_slice(&off.to_le_bytes());
    rec[24..32]
        .copy_from_slice(&((data.len() as u64) | (ENCODER & 0xffff_ffff) << 32).to_le_bytes());
    let ok = OpenOptions::new()
        .append(true)
        .open(p.dir.join(INDEX))
        .and_then(|mut f| f.write_all(&rec))
        .is_ok();
    if ok {
        p.map.insert(
            k.0,
            Ent {
                hi: k.1,
                off,
                size: data.len() as u32,
            },
        );
        p.end = off + data.len() as u64;
        PUTS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Queues `data` (the BC7 blocks of `k`) for the pack, written on the cache's own thread.
pub fn put(k: Key, data: Vec<u8>) {
    let tx = WRITER.get_or_init(|| {
        let (tx, rx) = channel::<(Key, Vec<u8>)>();
        std::thread::Builder::new()
            .name("astc cache".into())
            .spawn(move || {
                for (k, d) in rx {
                    if d.is_empty() {
                        // a flush marker: the sender waits for the reply below
                        FLUSHED.fetch_add(1, Ordering::Release);
                        continue;
                    }
                    write_one(k, &d);
                }
            })
            .expect("astc cache thread");
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap().send((k, data));
}

static FLUSHED: AtomicU64 = AtomicU64::new(0);

/// Waits until every put so far is written.
pub fn flush() {
    if WRITER.get().is_none() {
        return;
    }
    let before = FLUSHED.load(Ordering::Acquire);
    put(Key(0, 0), Vec::new());
    let t0 = std::time::Instant::now();
    while FLUSHED.load(Ordering::Acquire) == before && t0.elapsed().as_secs() < 30 {
        std::thread::yield_now();
    }
}

/// (hits, misses, levels written)
pub fn stats() -> (u64, u64, u64) {
    (
        HITS.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed),
        PUTS.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trips_and_survives_a_reopen() {
        let dir = std::env::temp_dir().join(format!("nxgfx-astc-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        set_dir(&dir);
        let blocks: Vec<u8> = (0..4096u32).map(|i| (i * 7) as u8).collect();
        let k = key(&blocks, 256, 256, 1, 4, 4, false);
        assert_ne!(
            k,
            key(&blocks, 256, 256, 1, 4, 4, true),
            "sRGB is part of the key"
        );
        assert!(get(k, 65536).is_none());
        let bc7: Vec<u8> = (0..65536u32).map(|i| (i ^ (i >> 8)) as u8).collect();
        put(k, bc7.clone());
        flush();
        assert_eq!(get(k, 65536).as_deref(), Some(&bc7[..]));
        assert!(get(k, 1024).is_none(), "a different size misses");
        close();
        set_dir(&dir);
        assert_eq!(
            get(k, 65536).as_deref(),
            Some(&bc7[..]),
            "read back after a reopen"
        );
        close();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
