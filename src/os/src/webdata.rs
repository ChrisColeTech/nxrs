//! The game's data folder as the engine reads its game data (docs/35 W3): the ExeFS modules, the control data and the
//! RomFS. Natively the host file system; on the web the I/O worker (`web::io`), which streams the files by HTTP range
//! into an OPFS cache, for every file the build's data manifest (`data.json`) lists, and the in-memory file system for
//! the rest.
//!
//! The boot-set recorder: `NXRS_BOOTSET=<file>` records the byte ranges the engine reads from the data folder
//! (`exefs/<module>`, `control.nacp`, RomFS files as `romfs/<path>`), files in first-touch order, ranges merged
//! within 4 KiB, and writes them as `[[path, [[first, last], ...]], ...]` (inclusive bytes) `NXRS_BOOTSET_SECONDS`
//! (default 60) after the first read. The web packager (`nxrs-build --target web`) turns the recording in the data
//! folder (`bootset.rec.json`) into the build's `bootset.json`, which the page prefetches while the module compiles.
//!
//! Saves on the web: `pack_save` turns a committed save folder into one blob (`NXSV1`), which the I/O worker stores in
//! the browser (`web::saves`); the page unpacks it at the next start.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A whole file of the data folder; `rel` is `/`-separated, relative to it.
pub fn read(data: &Path, rel: &str) -> std::io::Result<Vec<u8>> {
    #[cfg(target_os = "emscripten")]
    if let Some((id, size)) = crate::web::io::file(data, rel) {
        let v = crate::web::io::read(id, 0, size)?;
        record(rel, 0, size);
        return Ok(v);
    }
    let v = std::fs::read(data.join(rel))?;
    record(rel, 0, v.len() as u64);
    Ok(v)
}

// ---------------------------------------------------------------- boot-set recorder

/// Ranges closer than this are one range.
const MERGE_GAP: u64 = 4096;

struct Rec {
    order: Vec<String>,
    ranges: HashMap<String, Vec<(u64, u64)>>,
    done: bool,
}

static REC: Mutex<Option<Rec>> = Mutex::new(None);

fn target() -> Option<&'static str> {
    static T: OnceLock<Option<String>> = OnceLock::new();
    T.get_or_init(|| {
        std::env::var("NXRS_BOOTSET")
            .ok()
            .filter(|v| !v.trim().is_empty())
    })
    .as_deref()
}

/// Notes a read of `len` bytes at `off` of the data folder's file `rel` (while `NXRS_BOOTSET` records).
pub fn record(rel: &str, off: u64, len: u64) {
    let Some(file) = target() else { return };
    if len == 0 {
        return;
    }
    let mut g = REC.lock().unwrap_or_else(|e| e.into_inner());
    let r = g.get_or_insert_with(|| {
        let secs = std::env::var("NXRS_BOOTSET_SECONDS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);
        let file = file.to_string();
        let _ = std::thread::Builder::new()
            .name("bootset recorder".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(secs));
                write_recording(&file, secs);
            });
        Rec {
            order: vec![],
            ranges: HashMap::new(),
            done: false,
        }
    });
    if r.done {
        return;
    }
    let v = r.ranges.entry(rel.to_string()).or_insert_with(Vec::new);
    if v.is_empty() {
        r.order.push(rel.to_string());
    }
    v.push((off, off + len - 1));
}

fn write_recording(file: &str, secs: u64) {
    let t = Instant::now();
    let (order, ranges) = {
        let mut g = REC.lock().unwrap_or_else(|e| e.into_inner());
        let Some(r) = g.as_mut() else { return };
        r.done = true;
        (std::mem::take(&mut r.order), std::mem::take(&mut r.ranges))
    };
    let mut files = vec![];
    let mut bytes = 0u64;
    let mut count = 0usize;
    for p in &order {
        let m = merge(ranges[p].clone());
        bytes += m.iter().map(|(a, b)| b - a + 1).sum::<u64>();
        count += m.len();
        files.push(serde_json::json!([
            p,
            m.iter().map(|(a, b)| [a, b]).collect::<Vec<_>>()
        ]));
    }
    let text = serde_json::Value::Array(files).to_string();
    match std::fs::write(file, text) {
        Ok(()) => crate::log!(
            "bootset: {} files, {count} ranges, {:.1} MiB read in the first {secs} s, written to {file} ({:.0} ms)",
            order.len(),
            bytes as f64 / 1048576.0,
            t.elapsed().as_secs_f64() * 1e3
        ),
        Err(e) => crate::log!("[RED] bootset: writing {file}: {e}"),
    }
}

/// Inclusive ranges sorted, overlapping ones and ones within [`MERGE_GAP`] joined.
pub fn merge(mut v: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    v.sort_unstable();
    let mut out: Vec<(u64, u64)> = vec![];
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1.saturating_add(MERGE_GAP) => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

// ---------------------------------------------------------------- saves

pub const SAVE_MAGIC: &[u8; 8] = b"NXSV1\0\0\0";

/// A save folder as one blob: `NXSV1\0\0\0`, then per entry (directories before what they hold, names sorted)
/// u32 kind (0 directory, 1 file), u32 path length, the `/`-separated UTF-8 path relative to the folder, u64 size, the
/// bytes (none for a directory). All little-endian.
pub fn pack_save(dir: &Path) -> std::io::Result<Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<u8>) -> std::io::Result<()> {
        let mut ents: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        ents.sort_by_key(|e| e.file_name());
        for e in ents {
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .map_err(|_| std::io::Error::other("outside the save"))?
                .to_string_lossy()
                .replace('\\', "/");
            let is_dir = e.file_type()?.is_dir();
            let bytes = if is_dir { vec![] } else { std::fs::read(&p)? };
            out.extend_from_slice(&u32::from(!is_dir).to_le_bytes());
            out.extend_from_slice(&(rel.len() as u32).to_le_bytes());
            out.extend_from_slice(rel.as_bytes());
            out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            out.extend_from_slice(&bytes);
            if is_dir {
                walk(root, &p, out)?;
            }
        }
        Ok(())
    }
    let mut out = SAVE_MAGIC.to_vec();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_within_a_block() {
        assert_eq!(
            merge(vec![
                (9200, 9999),
                (0, 99),
                (100, 199),
                (4295, 5000),
                (20000, 20000)
            ]),
            [(0, 5000), (9200, 9999), (20000, 20000)]
        );
        assert_eq!(
            merge(vec![(0, 199), (4296, 4300)]),
            [(0, 199), (4296, 4300)]
        );
        assert_eq!(merge(vec![(0, 10), (2, 5)]), [(0, 10)]);
    }

    /// The blob the page unpacks (src/web/saves.js): directories first, then their files, paths relative.
    #[test]
    fn save_blob_layout() {
        let d = std::env::temp_dir().join(format!("nxrs-packsave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.bin"), b"xyz").unwrap();
        std::fs::write(d.join("sub/b"), b"").unwrap();
        let b = pack_save(&d).unwrap();
        let mut want = SAVE_MAGIC.to_vec();
        for (kind, path, data) in [
            (1u32, "a.bin", &b"xyz"[..]),
            (0, "sub", b""),
            (1, "sub/b", b""),
        ] {
            want.extend_from_slice(&kind.to_le_bytes());
            want.extend_from_slice(&(path.len() as u32).to_le_bytes());
            want.extend_from_slice(path.as_bytes());
            want.extend_from_slice(&(data.len() as u64).to_le_bytes());
            want.extend_from_slice(data);
        }
        assert_eq!(b, want);
        let _ = std::fs::remove_dir_all(&d);
    }
}
