//! The web build's data description (docs/35 W3), written beside `game.js` by `--target web`:
//!
//! - `data.json`: `{"version", "title", "files": [[path, size], ...]}`, every file of `<name>_data` (paths relative
//!   to it, `/`-separated, sorted) except `save/` and the recording below. A file's index is its number in the
//!   engine's I/O requests; the version (a hash of the paths, sizes and modification times) keys the page's OPFS
//!   cache, so a changed data folder starts a fresh cache.
//! - `bootset.json`: `[[path, [[first, last], ...]], ...]` (inclusive bytes), the ranges a recorded boot read, files
//!   in first-touch order, which the page prefetches while the module compiles. Made from the recording in the data
//!   folder (`bootset.rec.json`, written by a run with `NXRS_BOOTSET`, os/src/webdata.rs), checked against the
//!   manifest: files it no longer has and bytes past a file's end are dropped, ranges sorted and merged within 4 KiB.
//!   No recording: an empty boot set, and the page fetches everything on demand.

use anyhow::{Context, Result};
use std::path::Path;

pub const MANIFEST: &str = "data.json";
pub const BOOTSET: &str = "bootset.json";
/// The recording in the data folder that `bootset.json` is made from.
pub const RECORDING: &str = "bootset.rec.json";

/// (path, size, modification time in seconds) of every file the manifest lists.
fn files(data: &Path) -> Result<Vec<(String, u64, u64)>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64, u64)>) -> Result<()> {
        for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let e = e?;
            let p = e.path();
            let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
            if e.file_type()?.is_dir() {
                if rel != "save" {
                    walk(root, &p, out)?;
                }
            } else if rel != RECORDING && rel != MANIFEST {
                let m = e.metadata()?;
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                out.push((rel, m.len(), t));
            }
        }
        Ok(())
    }
    let mut v = vec![];
    walk(data, data, &mut v)?;
    v.sort();
    Ok(v)
}

fn version(files: &[(String, u64, u64)]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (p, n, t) in files {
        for b in p.bytes().chain(n.to_le_bytes()).chain(t.to_le_bytes()) {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// The boot set from a recording, checked against the manifest's files (path to size).
pub fn bootset(
    rec: &serde_json::Value,
    sizes: &std::collections::HashMap<&str, u64>,
) -> serde_json::Value {
    let mut out = vec![];
    for f in rec.as_array().into_iter().flatten() {
        let Some(path) = f[0].as_str() else { continue };
        let Some(&size) = sizes.get(path) else {
            continue;
        };
        let mut r: Vec<(u64, u64)> = f[1]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| Some((x[0].as_u64()?, x[1].as_u64()?)))
            .filter(|&(a, b)| a <= b && a < size)
            .map(|(a, b)| (a, b.min(size - 1)))
            .collect();
        r.sort_unstable();
        let mut m: Vec<(u64, u64)> = vec![];
        for (a, b) in r {
            match m.last_mut() {
                Some(l) if a <= l.1.saturating_add(4096) => l.1 = l.1.max(b),
                _ => m.push((a, b)),
            }
        }
        if !m.is_empty() {
            out.push(serde_json::json!([
                path,
                m.iter().map(|(a, b)| [a, b]).collect::<Vec<_>>()
            ]));
        }
    }
    serde_json::Value::Array(out)
}

/// Writes `data.json` and `bootset.json` into `outdir` for the data folder `data`; a line for the build log.
pub fn write(outdir: &Path, data: &Path, title: &str) -> Result<String> {
    let files = files(data)?;
    let ver = version(&files);
    let manifest = serde_json::json!({
        "version": ver,
        "title": title,
        "files": files.iter().map(|(p, n, _)| serde_json::json!([p, n])).collect::<Vec<_>>(),
    });
    let text = manifest.to_string();
    std::fs::write(outdir.join(MANIFEST), &text)?;
    // the engine reads the manifest from its data folder (the page puts it there with the host files)
    std::fs::write(data.join(MANIFEST), &text)?;
    let sizes: std::collections::HashMap<&str, u64> =
        files.iter().map(|(p, n, _)| (p.as_str(), *n)).collect();
    let (boot, said) = match std::fs::read_to_string(data.join(RECORDING)) {
        Ok(t) => {
            let rec: serde_json::Value = serde_json::from_str(&t)
                .with_context(|| format!("{}", data.join(RECORDING).display()))?;
            let b = bootset(&rec, &sizes);
            let bytes: u64 = b
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|f| f[1].as_array().cloned().unwrap_or_default())
                .map(|r| r[1].as_u64().unwrap_or(0) - r[0].as_u64().unwrap_or(0) + 1)
                .sum();
            let n = b.as_array().map_or(0, |a| a.len());
            (
                b,
                format!(
                    "boot set {n} files, {:.1} MiB (from {RECORDING})",
                    bytes as f64 / 1048576.0
                ),
            )
        }
        Err(_) => (
            serde_json::Value::Array(vec![]),
            format!(
                "no boot set (no {RECORDING} in the data folder: record one with NXRS_BOOTSET)"
            ),
        ),
    };
    std::fs::write(outdir.join(BOOTSET), boot.to_string())?;
    Ok(format!(
        "Web data: {} files, {:.1} MiB in {MANIFEST} (version {ver}); {said}",
        files.len(),
        files.iter().map(|f| f.1).sum::<u64>() as f64 / 1048576.0
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootset_is_checked_against_the_manifest() {
        let rec = serde_json::json!([
            [
                "romfs/b",
                [[5000, 5100], [0, 10], [12, 20], [90, 1u64 << 40]]
            ],
            ["romfs/gone", [[0, 1]]],
            ["exefs/main", [[200, 100], [0, 99]]]
        ]);
        let sizes = [("romfs/b", 6000u64), ("exefs/main", 100)]
            .into_iter()
            .collect();
        assert_eq!(
            bootset(&rec, &sizes),
            serde_json::json!([["romfs/b", [[0, 5999]]], ["exefs/main", [[0, 99]]]])
        );
    }

    #[test]
    fn manifest_lists_the_data_folder_but_saves() {
        let d = std::env::temp_dir().join(format!("nxrs-webpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let data = d.join("x_data");
        std::fs::create_dir_all(data.join("romfs/sub")).unwrap();
        std::fs::create_dir_all(data.join("save/user1")).unwrap();
        std::fs::write(data.join("romfs/sub/a"), b"abc").unwrap();
        std::fs::write(data.join("control.nacp"), b"n").unwrap();
        std::fs::write(data.join("save/user1/s"), b"s").unwrap();
        std::fs::write(data.join(RECORDING), r#"[["romfs/sub/a",[[1,1]]]]"#).unwrap();
        let line = write(&d, &data, "X").unwrap();
        let m: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(
            m["files"],
            serde_json::json!([["control.nacp", 1], ["romfs/sub/a", 3]])
        );
        assert_eq!(m["title"], "X");
        let b = std::fs::read_to_string(d.join(BOOTSET)).unwrap();
        assert_eq!(b, r#"[["romfs/sub/a",[[1,1]]]]"#);
        assert!(line.contains("2 files"), "{line}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
