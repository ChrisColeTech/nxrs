//! The web build's shader packs (docs/35 §4.4, WEB-W6b; format: `shaders::pack`), written beside `game.js`:
//! `shaders/pack-<n>.bin` (+ its gzip copy, sent with `Content-Encoding`) and `shaders/packs.txt`.
//!
//! The members are the modules a recorded run loaded (`<name>_data/shaders.rec.txt`, `NXGFX_SHADER_REC=<file>` on a
//! native run), in first-use order, read from the data folder's shader library (`shaders/spv/`, where W7's
//! `--shaders-from` imports land too), cut into packs of about [`PACK_BYTES`]. The engine fetches a pack whole on the
//! first need of any member; a module outside the recording is still fetched as its own two files. No recording: no
//! packs.

use anyhow::{Context, Result};
use shaders::pack;
use std::path::Path;

/// Uncompressed bytes a pack is cut at: the boot's first draws need only the first pack.
pub const PACK_BYTES: usize = 1 << 20;

/// A recorded module from the library: its files in `<data>/shaders/spv` (or `<data>/shaders`).
fn read_module(data: &Path, key: &[u8; 8]) -> Option<pack::Entry> {
    let hex = pack::key_hex(key);
    let lib = data.join("shaders");
    [lib.join("spv"), lib].iter().find_map(|d| {
        let spv = std::fs::read(d.join(format!("{hex}.spv"))).ok()?;
        let json = std::fs::read(d.join(format!("{hex}.json"))).ok();
        Some(pack::Entry { key: *key, spv, json })
    })
}

/// Writes the packs of the data folder's recording into `<outdir>/shaders` (an earlier build's packs removed first);
/// a line for the build log.
pub fn write(outdir: &Path, data: &Path) -> Result<String> {
    let dir = outdir.join(pack::DIR);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("pack-") || n.starts_with(pack::INDEX) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let Ok(rec) = std::fs::read_to_string(data.join(pack::RECORDING)) else {
        return Ok(format!(
            "Shader packs: none (no {} in the data folder: record one with NXGFX_SHADER_REC); every module is fetched as its own files",
            pack::RECORDING
        ));
    };
    let keys = pack::recording(&rec);
    let mods: Vec<pack::Entry> = keys.iter().filter_map(|k| read_module(data, k)).collect();
    let missing = keys.len() - mods.len();
    let sizes: Vec<usize> = mods.iter().map(|e| e.spv.len() + e.json.as_ref().map_or(0, Vec::len)).collect();
    std::fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    let mut members = vec![];
    let (mut raw, mut zipped) = (0u64, 0u64);
    for (n, g) in pack::groups(&sizes, PACK_BYTES).into_iter().enumerate() {
        let entries = &mods[g];
        let p = dir.join(pack::pack_name(n));
        std::fs::write(&p, pack::encode(entries)).with_context(|| p.display().to_string())?;
        if let Some((r, z)) = crate::webzip::gzip_file(&p)? {
            raw += r;
            zipped += z;
        }
        members.push(entries.iter().map(|e| e.key).collect::<Vec<_>>());
    }
    std::fs::write(dir.join(pack::INDEX), pack::index_text(&members))?;
    Ok(format!(
        "Shader packs: {} modules of the recording in {} packs, {:.1} MiB ({:.1} MiB gzip){}",
        mods.len(),
        members.len(),
        raw as f64 / 1048576.0,
        zipped as f64 / 1048576.0,
        if missing > 0 { format!("; {missing} recorded modules are not in the library") } else { String::new() }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_hold_the_recorded_modules_in_order_and_an_index_names_them() {
        let root = std::env::temp_dir().join(format!("nxrs-webshaders-{}", std::process::id()));
        let (data, out) = (root.join("data"), root.join("out"));
        let spv = data.join("shaders").join("spv");
        std::fs::create_dir_all(&spv).unwrap();
        std::fs::create_dir_all(out.join("shaders")).unwrap();
        std::fs::write(out.join("shaders").join("pack-007.bin"), b"stale").unwrap();
        let big = vec![7u8; PACK_BYTES];
        std::fs::write(spv.join("00000000000000aa.spv"), &big).unwrap();
        std::fs::write(spv.join("00000000000000aa.json"), b"{}").unwrap();
        std::fs::write(spv.join("00000000000000bb.spv"), [1, 2, 3, 4]).unwrap();
        std::fs::write(data.join(pack::RECORDING), "00000000000000aa\n00000000000000cc\n00000000000000bb\n").unwrap();
        let line = write(&out, &data).unwrap();
        assert!(line.contains("2 modules of the recording in 2 packs") && line.contains("1 recorded modules are not"), "{line}");
        let p0 = pack::decode(&std::fs::read(out.join("shaders").join("pack-000.bin")).unwrap()).unwrap();
        let p1 = pack::decode(&std::fs::read(out.join("shaders").join("pack-001.bin")).unwrap()).unwrap();
        assert_eq!((p0.len(), p0[0].json.as_deref()), (1, Some(&b"{}"[..])));
        assert_eq!((p1[0].spv.as_slice(), p1[0].json.as_ref()), (&[1u8, 2, 3, 4][..], None));
        let idx = pack::parse_index(&std::fs::read_to_string(out.join("shaders").join(pack::INDEX)).unwrap()).unwrap();
        assert_eq!(idx.len(), 2);
        assert!(out.join("shaders").join("pack-000.bin.gz").is_file());
        assert!(!out.join("shaders").join("pack-007.bin").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
