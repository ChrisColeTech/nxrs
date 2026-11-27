//! Requests to the page's I/O worker (`src/front/web/page/io-worker.js`): game-data reads and save commits, through a table of
//! slots in linear memory.
//!
//! Table (32-bit words): header 16 words — `[0]` doorbell (bumped by every request; the worker waits on it), `[1]`
//! slot count, `[2]` protocol version, `[3]` slots freed (engine threads waiting for a free slot wait on it). Then
//! [`SLOTS`] slots of 16 words: `[0]` state (0 idle, 4 claimed by an engine thread, 1 requested, 3 taken by the
//! worker, 2 done), `[1]` op (1 read, 2 save commit), `[2]` file id (read) or name length (save), `[3..5)` offset
//! (read) or name address (save), `[5..7)` destination (read) or data address (save), `[7]` length, `[8]` result
//! (bytes read, 0 for a stored save; negative: failed).
//!
//! An engine thread claims an idle slot, fills it, marks it requested, rings the doorbell and blocks on the slot's
//! state word until the worker marks it done; then it frees the slot. File ids are indexes into the build's data
//! manifest (`data.json`: `{"version", "title", "files": [[path, size], ...]}`), which the page puts beside the host
//! files and both sides read.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering::*};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use translator::package::romfs::StoredFiles;
use translator::package::RomFs;

const HEADER: usize = 16;
const SLOT: usize = 16;
pub const SLOTS: usize = 64;
const VERSION: u32 = 1;
pub const OP_READ: u32 = 1;
pub const OP_SAVE: u32 = 2;
const IDLE: u32 = 0;
const REQUESTED: u32 = 1;
const DONE: u32 = 2;
const CLAIMED: u32 = 4;
/// The largest single request (the slot's length is a 32-bit word).
const CHUNK: u64 = 64 << 20;

fn table() -> &'static [AtomicU32] {
    static T: OnceLock<&'static [AtomicU32]> = OnceLock::new();
    T.get_or_init(|| {
        let t = super::block(HEADER + SLOT * SLOTS);
        t[1].store(SLOTS as u32, Relaxed);
        t[2].store(VERSION, Release);
        // SAFETY: the table lives for the process; the page posts its address to the I/O worker
        unsafe { super::nxweb_io_start(t.as_ptr() as *const u32) };
        crate::log!(
            "web: I/O worker table at {:#x} ({SLOTS} slots)",
            t.as_ptr() as usize
        );
        t
    })
}

/// One request; the worker's result, or None when `limit_ms` passed first (the slot is then left to the worker and
/// never reused: what it points at must stay alive).
pub fn request(
    op: u32,
    a: u32,
    off: u64,
    addr: u64,
    len: u32,
    limit_ms: Option<f64>,
) -> Option<i32> {
    let t = table();
    let i = loop {
        if let Some(i) = (0..SLOTS).find(|&i| {
            t[HEADER + i * SLOT]
                .compare_exchange(IDLE, CLAIMED, AcqRel, Relaxed)
                .is_ok()
        }) {
            break i;
        }
        let freed = t[3].load(Acquire);
        super::wait(&t[3], freed, 5.0);
    };
    let s = &t[HEADER + i * SLOT..HEADER + (i + 1) * SLOT];
    for (w, v) in [
        (1, op),
        (2, a),
        (3, off as u32),
        (4, (off >> 32) as u32),
        (5, addr as u32),
        (6, (addr >> 32) as u32),
        (7, len),
        (8, 0),
    ] {
        s[w].store(v, Relaxed);
    }
    s[0].store(REQUESTED, Release);
    t[0].fetch_add(1, AcqRel);
    super::wake(&t[0]);
    let t0 = Instant::now();
    let mut said = false;
    loop {
        let st = s[0].load(Acquire);
        if st == DONE {
            break;
        }
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        if limit_ms.is_some_and(|l| ms >= l) {
            return None;
        }
        if !said && ms >= 10_000.0 {
            said = true;
            crate::log!("[RED] web: an I/O request (op {op}, file/name {a}, {len} bytes) has waited 10 s for the I/O worker");
        }
        super::wait(&s[0], st, 1000.0);
    }
    let r = s[8].load(Acquire) as i32;
    s[0].store(IDLE, Release);
    t[3].fetch_add(1, AcqRel);
    super::wake(&t[3]);
    Some(r)
}

struct Manifest {
    by_path: HashMap<String, (u32, u64)>,
    list: Vec<(String, u64)>,
}

/// The data manifest (`<data>/data.json`), read once from the first data folder asked about.
fn manifest(data: &Path) -> Option<&'static Manifest> {
    static M: OnceLock<Option<Manifest>> = OnceLock::new();
    M.get_or_init(|| {
        let p = data.join("data.json");
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) => {
                crate::log!("[RED] web: no data manifest at {} ({e}): game data comes from the in-memory file system only", p.display());
                return None;
            }
        };
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                crate::log!("[RED] web: {}: {e}", p.display());
                return None;
            }
        };
        let list: Vec<(String, u64)> = v["files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| Some((f[0].as_str()?.to_string(), f[1].as_u64()?)))
                    .collect()
            })
            .unwrap_or_default();
        let by_path = list
            .iter()
            .enumerate()
            .map(|(i, (p, n))| (p.clone(), (i as u32, *n)))
            .collect();
        crate::log!(
            "web: data manifest {}: {} files, {:.1} MiB, streamed by the I/O worker",
            v["version"].as_str().unwrap_or("?"),
            list.len(),
            list.iter().map(|f| f.1).sum::<u64>() as f64 / 1048576.0
        );
        Some(Manifest { by_path, list })
    })
    .as_ref()
}

/// The manifest's number and size of the data folder's file `rel`.
pub fn file(data: &Path, rel: &str) -> Option<(u32, u64)> {
    manifest(data)?.by_path.get(rel).copied()
}

/// Up to `len` bytes of file `id` at `off` (fewer at its end), read by the I/O worker straight into the result.
pub fn read(id: u32, off: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let mut v = vec![0u8; len as usize];
    let mut got = 0u64;
    while got < len {
        let n = (len - got).min(CHUNK);
        let dst = v.as_mut_ptr() as u64 + got;
        match request(OP_READ, id, off + got, dst, n as u32, None) {
            Some(r) if r >= 0 => {
                got += r as u64;
                if (r as u64) < n {
                    break;
                }
            }
            r => {
                return Err(std::io::Error::other(format!(
                    "the I/O worker could not read file {id} at {:#x} (+{n:#x}): {r:?}",
                    off + got
                )))
            }
        }
    }
    v.truncate(got as usize);
    Ok(v)
}

struct Store;

impl StoredFiles for Store {
    fn read(
        &self,
        id: u32,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, translator::package::PackageError> {
        let v = read(id, offset, len)
            .map_err(|e| translator::package::PackageError::Format(e.to_string()))?;
        if let Some(m) = MANIFEST_PATHS.get() {
            if let Some(p) = m.get(id as usize) {
                crate::webdata::record(p, offset, len);
            }
        }
        Ok(v)
    }
}

static MANIFEST_PATHS: OnceLock<Vec<String>> = OnceLock::new();

/// The game's RomFS: the manifest's `romfs/` files, read through the I/O worker.
pub fn romfs(data: &Path) -> Option<RomFs> {
    let m = manifest(data)?;
    let _ = MANIFEST_PATHS.set(m.list.iter().map(|f| f.0.clone()).collect());
    let files: Vec<(String, u64, u32)> = m
        .list
        .iter()
        .enumerate()
        .filter_map(|(i, (p, n))| Some((p.strip_prefix("romfs/")?.to_string(), *n, i as u32)))
        .collect();
    if files.is_empty() {
        return None;
    }
    match RomFs::from_stored(&files, Arc::new(Store), "the I/O worker (data.json)".into()) {
        Ok(r) => Some(r),
        Err(e) => {
            crate::log!("[RED] web: the RomFS from the data manifest: {e}");
            None
        }
    }
}
