//! The web build's compression (docs/35 W6), done once at build time:
//!
//! - **Code and descriptions** (`game.wasm`, `game.cold.wasm`, `game.js`, `data.json`): a gzip copy beside each
//!   (`<file>.gz`) that a server sends whole with `Content-Encoding: gzip` when the browser accepts it. Compressed in
//!   parallel pieces that end on a full flush and join into one deflate stream (pigz's layout), so a 400 MiB module
//!   takes seconds; one gzip member, any decoder reads it.
//! - **Game data** (RomFS, ExeFS): read by range, so a whole-file encoding would not do. Each file of 64 KiB or more is
//!   cut into 64 KiB extents, each raw-deflated on its own (an extent that does not shrink is kept as is), joined in
//!   `zdata/<path>` beside the page; the extents' ends go into `data.json` (`[path, size, [extent, end0, end1, ...]]`,
//!   ends cumulative in the compressed file, starting at 0). The I/O worker fetches whole extents by exact range from
//!   the compressed file and inflates them (`DecompressionStream('deflate-raw')`); its OPFS cache keeps the inflated
//!   blocks. A file that would not shrink to 90% stays identity (no third element).
//!
//! Measured on Anime Girls (WEB-W6): the module gzips to 24%, RomFS whole-file to 45%; the game reads 838 of 937 MiB
//! in its first minute, so data is where most of the first launch's bytes are.

use anyhow::{Context, Result};
use flate2::{Compress, Compression, FlushCompress};
use std::path::{Path, PathBuf};

/// Uncompressed bytes per data extent: 16 of the I/O worker's 4 KiB blocks.
pub const EXTENT: usize = 64 * 1024;
/// Files smaller than this stay identity.
pub const MIN_FILE: u64 = EXTENT as u64;
/// The folder beside the page that holds the compressed data files.
pub const ZDATA: &str = "zdata";
/// Bytes per independently compressed piece of a gzip copy.
const PIECE: usize = 4 << 20;

/// Raw deflate of `data` as one piece: ends on a full flush (byte-aligned, no history carried), or finished.
fn deflate_piece(data: &[u8], level: u32, last: bool) -> Vec<u8> {
    let mut c = Compress::new(Compression::new(level), false);
    let mut out = Vec::with_capacity(data.len() / 3 + 64);
    let flush = if last { FlushCompress::Finish } else { FlushCompress::Full };
    loop {
        let done = c.total_in() as usize;
        if out.capacity() - out.len() < 64 << 10 {
            out.reserve(out.capacity().max(64 << 10));
        }
        let st = c.compress_vec(&data[done..], &mut out, flush).expect("deflate");
        if c.total_in() as usize == data.len() && (st == flate2::Status::StreamEnd || (!last && out.capacity() > out.len())) {
            return out;
        }
    }
}

/// `f(i)` for `0..n` on every core, results in order.
fn parallel<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(8, |n| n.get()).min(n.max(1));
    let mut parts: Vec<Vec<(usize, T)>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = vec![];
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= n {
                            return mine;
                        }
                        mine.push((i, f(i)));
                    }
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("compression thread")).collect()
    });
    let mut all: Vec<(usize, T)> = parts.drain(..).flatten().collect();
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, t)| t).collect()
}

/// One gzip member of `data`, compressed in parallel pieces.
pub fn gzip(data: &[u8], level: u32) -> Vec<u8> {
    let n = data.len().div_ceil(PIECE).max(1);
    let pieces = parallel(n, |i| {
        let a = i * PIECE;
        let b = ((i + 1) * PIECE).min(data.len());
        deflate_piece(&data[a..b], level, i + 1 == n)
    });
    let mut crc = flate2::Crc::new();
    crc.update(data);
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    for p in pieces {
        out.extend_from_slice(&p);
    }
    out.extend_from_slice(&crc.sum().to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// Writes `<file>.gz` beside `file` (when the file exists); the sizes, for the build log.
pub fn gzip_file(file: &Path) -> Result<Option<(u64, u64)>> {
    let Ok(data) = std::fs::read(file) else {
        return Ok(None);
    };
    let z = gzip(&data, 6);
    let gz = gz_path(file);
    std::fs::write(&gz, &z).with_context(|| format!("writing {}", gz.display()))?;
    Ok(Some((data.len() as u64, z.len() as u64)))
}

/// `<file>.gz`.
pub fn gz_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".gz");
    PathBuf::from(s)
}

/// A data file cut into extents: the compressed file's bytes and the extents' cumulative ends (first 0).
pub fn extents(data: &[u8], level: u32) -> (Vec<u8>, Vec<u64>) {
    let n = data.len().div_ceil(EXTENT);
    let parts = parallel(n, |i| {
        let raw = &data[i * EXTENT..((i + 1) * EXTENT).min(data.len())];
        let z = deflate_piece(raw, level, true);
        // an extent that does not shrink is kept as is: the reader knows it by its length
        if z.len() >= raw.len() {
            raw.to_vec()
        } else {
            z
        }
    });
    let mut ends = vec![0u64];
    let mut out = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for p in parts {
        out.extend_from_slice(&p);
        ends.push(out.len() as u64);
    }
    (out, ends)
}

/// Compresses the manifest's data files into `<outdir>\zdata` (identity where it does not pay); returns each
/// compressed file's extent ends by its path, and (raw, compressed) totals for the log. Folders of files no longer
/// compressed are cleaned of them.
pub fn compress_data(
    outdir: &Path,
    data: &Path,
    files: &[(String, u64, u64)],
) -> Result<(std::collections::HashMap<String, Vec<u64>>, u64, u64)> {
    let zroot = outdir.join(ZDATA);
    let mut ends = std::collections::HashMap::new();
    let (mut raw, mut comp) = (0u64, 0u64);
    for (path, size, _) in files {
        let z = zroot.join(path);
        if *size < MIN_FILE || !(path.starts_with("romfs/") || path.starts_with("exefs/")) {
            let _ = std::fs::remove_file(&z);
            continue;
        }
        let bytes = std::fs::read(data.join(path)).with_context(|| format!("reading {path}"))?;
        let (zbytes, e) = extents(&bytes, 6);
        if zbytes.len() as u64 * 10 > bytes.len() as u64 * 9 {
            let _ = std::fs::remove_file(&z);
            continue;
        }
        if let Some(d) = z.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::write_if_changed(&z, &zbytes).with_context(|| format!("writing {}", z.display()))?;
        raw += bytes.len() as u64;
        comp += zbytes.len() as u64;
        ends.insert(path.clone(), e);
    }
    Ok((ends, raw, comp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn sample(n: usize) -> Vec<u8> {
        // compressible but not trivial: text-like runs with a counter
        let mut v = Vec::with_capacity(n);
        let mut x: u32 = 1;
        while v.len() < n {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let word = b"nxrs web extent ";
            v.extend_from_slice(&word[..(x >> 28) as usize % word.len() + 1]);
            v.push((x >> 16) as u8);
        }
        v.truncate(n);
        v
    }

    #[test]
    fn gzip_in_pieces_is_one_member_any_decoder_reads() {
        for n in [0, 1, PIECE - 1, PIECE, PIECE * 2 + 12345] {
            let d = sample(n);
            let z = gzip(&d, 6);
            let mut back = vec![];
            // a single-member decoder: the pieces must form one stream
            flate2::read::GzDecoder::new(&z[..]).read_to_end(&mut back).unwrap();
            assert_eq!(back, d, "{n} bytes");
            if n > 1000 {
                assert!(z.len() < n / 2, "{n}: {} compressed", z.len());
            }
        }
    }

    #[test]
    fn extents_inflate_one_by_one_and_incompressible_ones_stay_raw() {
        let mut d = sample(EXTENT * 3 + 777);
        // extent 1: random bytes (does not shrink)
        let mut x: u64 = 7;
        for b in &mut d[EXTENT..2 * EXTENT] {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        let (z, ends) = extents(&d, 6);
        assert_eq!(ends.len(), 5);
        assert_eq!(*ends.last().unwrap(), z.len() as u64);
        for i in 0..4 {
            let part = &z[ends[i] as usize..ends[i + 1] as usize];
            let raw = &d[i * EXTENT..((i + 1) * EXTENT).min(d.len())];
            let back = if part.len() == raw.len() {
                part.to_vec()
            } else {
                let mut v = vec![];
                flate2::read::DeflateDecoder::new(part).read_to_end(&mut v).unwrap();
                v
            };
            assert_eq!(back, raw, "extent {i}");
        }
        assert_eq!(ends[2] - ends[1], EXTENT as u64, "the random extent is stored raw");
    }

    #[test]
    fn data_files_compress_only_where_it_pays() {
        let d = std::env::temp_dir().join(format!("nxrs-webzip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let data = d.join("x_data");
        std::fs::create_dir_all(data.join("romfs")).unwrap();
        std::fs::write(data.join("romfs/big"), sample(EXTENT * 2)).unwrap();
        std::fs::write(data.join("romfs/small"), sample(100)).unwrap();
        std::fs::write(data.join("icon.dat"), sample(EXTENT * 2)).unwrap();
        let files = vec![
            ("icon.dat".to_string(), (EXTENT * 2) as u64, 0),
            ("romfs/big".to_string(), (EXTENT * 2) as u64, 0),
            ("romfs/small".to_string(), 100, 0),
        ];
        let (ends, raw, comp) = compress_data(&d, &data, &files).unwrap();
        assert_eq!(ends.keys().collect::<Vec<_>>(), ["romfs/big"]);
        assert_eq!(raw, (EXTENT * 2) as u64);
        assert!(comp < raw / 2);
        assert_eq!(
            std::fs::metadata(d.join("zdata/romfs/big")).unwrap().len(),
            *ends["romfs/big"].last().unwrap()
        );
        assert!(!d.join("zdata/romfs/small").exists());
        assert!(!d.join("zdata/icon.dat").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
