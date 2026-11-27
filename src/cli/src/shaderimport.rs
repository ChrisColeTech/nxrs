//! `--shaders-from <data folder>`: the translated shaders of another build of the same title, imported into this
//! build's library.
//!
//! A game that makes its shaders while it runs (Ren'Py, Unity variants) has none for the build's prebuild to find;
//! natively the live translator (`dmsh_xlate`, a native executable) translates them in the game's first runs, into
//! `<data>\shaders\live\<key>\` and, at the next build, the library. A web build cannot run that translator in the
//! browser, so it takes them from a native build that has run: every complete record (SPIR-V and reflection) the
//! other library has and this one lacks, from its `spv\` (with its manifest row) and its `live\<key>\`. Keys are
//! content hashes of the game's shader, so a record is valid for any build of the title. Pair rows are joined.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;

/// What an import did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// keys copied into this library
    pub shaders: usize,
    /// keys the other library has that this one already had
    pub present: usize,
    /// pair rows added
    pub pairs: usize,
}

fn is_key(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The manifest's record rows (`<key>\t...`), by key; comments and the column header are not records.
fn rows(manifest: &str) -> impl Iterator<Item = (&str, &str)> {
    manifest.lines().filter_map(|l| {
        let (k, _) = l.split_once('\t')?;
        is_key(k).then_some((k, l))
    })
}

/// Copies `<key>.spv` and `<key>.json` from `spv_dir` into `out_spv`; false when either is missing.
fn copy_record(spv_dir: &Path, key: &str, out_spv: &Path) -> Result<bool> {
    let spv = spv_dir.join(format!("{key}.spv"));
    let json = spv_dir.join(format!("{key}.json"));
    if !spv.is_file() || !json.is_file() {
        return Ok(false);
    }
    std::fs::copy(&spv, out_spv.join(format!("{key}.spv")))?;
    std::fs::copy(&json, out_spv.join(format!("{key}.json")))?;
    Ok(true)
}

/// Imports `<from>\shaders` into `<data>\shaders` (both data folders).
pub fn import(from: &Path, data: &Path) -> Result<Imported> {
    let src = from.join("shaders");
    if !src.is_dir() {
        anyhow::bail!("{} has no shader library", from.display());
    }
    let dst = data.join("shaders");
    let out_spv = dst.join("spv");
    std::fs::create_dir_all(&out_spv).with_context(|| out_spv.display().to_string())?;
    let mut manifest = std::fs::read_to_string(dst.join("manifest.tsv"))
        .unwrap_or_else(|_| "# nxshaders 1\nkey\tstage\n".into());
    let mut pairs = std::fs::read_to_string(dst.join("pairs.tsv"))
        .unwrap_or_else(|_| "# nxshaders pairs 1\n".into());
    let mut have: HashSet<String> = rows(&manifest).map(|(k, _)| k.to_string()).collect();
    let mut pair_rows: HashSet<String> = pairs.lines().map(str::to_string).collect();
    let mut r = Imported::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut take = |key: &str, row: &str, spv_dir: &Path, manifest: &mut String| -> Result<()> {
        if !seen.insert(key.to_string()) {
            return Ok(());
        }
        if have.contains(key) {
            r.present += 1;
        } else if copy_record(spv_dir, key, &out_spv)? {
            have.insert(key.to_string());
            manifest.push_str(row);
            manifest.push('\n');
            r.shaders += 1;
        }
        Ok(())
    };
    let mut pair_sources = vec![src.join("pairs.tsv")];
    // the library's own records
    let lib = std::fs::read_to_string(src.join("manifest.tsv")).unwrap_or_default();
    for (k, row) in rows(&lib) {
        take(k, row, &src.join("spv"), &mut manifest)?;
    }
    // live records a build has not promoted yet
    if let Ok(dir) = std::fs::read_dir(src.join("live")) {
        let mut dirs: Vec<_> = dir.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            let Some(key) = d.file_name().and_then(|n| n.to_str()).filter(|n| is_key(n)) else {
                continue;
            };
            let m = std::fs::read_to_string(d.join("manifest.tsv")).unwrap_or_default();
            let Some((_, row)) = rows(&m).find(|(k, _)| *k == key) else {
                continue;
            };
            let row = row.to_string();
            take(key, &row, &d.join("spv"), &mut manifest)?;
            pair_sources.push(d.join("pairs.tsv"));
        }
    }
    for p in pair_sources {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        for row in text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            if pair_rows.insert(row.to_string()) {
                pairs.push_str(row);
                pairs.push('\n');
                r.pairs += 1;
            }
        }
    }
    std::fs::write(dst.join("manifest.tsv"), manifest)?;
    std::fs::write(dst.join("pairs.tsv"), pairs)?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    const HEAD: &str =
        "# nxshaders 1\n# chain x\nkey\tstage\ttranslated_as\tarch\tstatus\tspv_bytes\tsource\n";

    /// The other library's records and its unpromoted live ones are copied with their rows; a key this library has is
    /// left as it is; an incomplete live record (no reflection) is not taken; pairs are joined once.
    #[test]
    fn imports_what_the_library_lacks() {
        let t = std::env::temp_dir().join(format!("nxrs-shaderimport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let (from, data) = (t.join("native_data"), t.join("web_data"));
        let a = "00000000000000aa";
        let b = "00000000000000bb";
        let c = "00000000000000cc";
        let d = "00000000000000dd";
        // the other library: a (promoted) and b (already in ours)
        write(
            &from.join("shaders/manifest.tsv"),
            &format!(
                "{HEAD}{a}\tps\tps\tmaxwell\tok\t4\tlive\n{b}\tvs\tvs\tmaxwell\tok\t4\tbuild\n"
            ),
        );
        write(&from.join(format!("shaders/spv/{a}.spv")), "spvA");
        write(&from.join(format!("shaders/spv/{a}.json")), "{}");
        write(&from.join(format!("shaders/spv/{b}.spv")), "spvB-native");
        write(&from.join(format!("shaders/spv/{b}.json")), "{}");
        write(&from.join("shaders/pairs.tsv"), "# pairs\n");
        // live: c complete, d without reflection, a again (once)
        for k in [a, c, d] {
            write(
                &from.join(format!("shaders/live/{k}/manifest.tsv")),
                &format!("{HEAD}{k}\tps\tps\tmaxwell\tok\t4\tlive (set by the game at run time)\n"),
            );
            write(&from.join(format!("shaders/live/{k}/spv/{k}.spv")), "spv");
        }
        write(&from.join(format!("shaders/live/{a}/spv/{a}.json")), "{}");
        write(&from.join(format!("shaders/live/{c}/spv/{c}.json")), "{}");
        write(
            &from.join(format!("shaders/live/{c}/pairs.tsv")),
            &format!("# p\n{b}\t{c}\tvar\n"),
        );
        write(
            &from.join(format!("shaders/live/{a}/pairs.tsv")),
            &format!("{b}\t{c}\tvar\n"),
        );
        // ours: b only
        write(
            &data.join("shaders/manifest.tsv"),
            &format!("{HEAD}{b}\tvs\tvs\tmaxwell\tok\t4\tbuild\n"),
        );
        write(&data.join(format!("shaders/spv/{b}.spv")), "spvB-ours");
        write(&data.join(format!("shaders/spv/{b}.json")), "{}");

        let r = import(&from, &data).unwrap();
        assert_eq!(
            r,
            Imported {
                shaders: 2,
                present: 1,
                pairs: 1
            }
        );
        let s = data.join("shaders");
        assert_eq!(
            std::fs::read_to_string(s.join(format!("spv/{a}.spv"))).unwrap(),
            "spvA"
        );
        assert!(s.join(format!("spv/{c}.json")).is_file());
        assert!(
            !s.join(format!("spv/{d}.spv")).exists(),
            "an incomplete record is not taken"
        );
        assert_eq!(
            std::fs::read_to_string(s.join(format!("spv/{b}.spv"))).unwrap(),
            "spvB-ours"
        );
        let m = std::fs::read_to_string(s.join("manifest.tsv")).unwrap();
        let keys: Vec<_> = rows(&m).map(|(k, _)| k.to_string()).collect();
        assert_eq!(keys, [b, a, c]);
        assert!(m.starts_with(HEAD), "the library's header stays");
        let p = std::fs::read_to_string(s.join("pairs.tsv")).unwrap();
        assert_eq!(p.matches(&format!("{b}\t{c}\tvar")).count(), 1);

        // again: nothing new
        let r = import(&from, &data).unwrap();
        assert_eq!(
            r,
            Imported {
                shaders: 0,
                present: 3,
                pairs: 0
            }
        );
        assert!(
            import(&t.join("none"), &data).is_err(),
            "a folder with no library is an error"
        );
        let _ = std::fs::remove_dir_all(&t);
    }
}
