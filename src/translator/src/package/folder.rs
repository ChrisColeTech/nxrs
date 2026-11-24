//! A dumped game folder: either it holds an `exefs/` directory (with `romfs/` beside it), or the folder itself is an
//! ExeFS; one level deeper is searched too. A directory counts as an ExeFS when it has a `main` file (`main.npdm`
//! is optional, as in Turbo's loader). The `exefs/` child is preferred over the folder itself.
//! The RomFS is `romfs/` (or `RomFS/`) beside an `exefs/` directory, or inside a flat folder (the ExeFS files and
//! `romfs/` side by side), and optional; `control.nacp` and the icon come from the folder or its `control/`
//! (Turbo switchrecomp.py / nxloader.py).
use crate::package::error::{fmt, PackageError, Result};
use crate::package::{ExeFs, SourceKind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const EXEFS_NAMES: [&str; 15] = [
    "main",
    "main.npdm",
    "rtld",
    "sdk",
    "subsdk0",
    "subsdk1",
    "subsdk2",
    "subsdk3",
    "subsdk4",
    "subsdk5",
    "subsdk6",
    "subsdk7",
    "subsdk8",
    "subsdk9",
    "subsdk10",
];

fn is_exefs(dir: &Path) -> bool {
    dir.join("main").is_file()
}

fn exefs_child(dir: &Path) -> Option<PathBuf> {
    ["exefs", "ExeFS", "EXEFS"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| is_exefs(p))
}

/// The ExeFS directory for a dump folder, if any.
pub fn find_exefs(dir: &Path) -> Option<PathBuf> {
    if let Some(p) = exefs_child(dir) {
        return Some(p);
    }
    if is_exefs(dir) {
        return Some(dir.to_path_buf());
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    subdirs
        .iter()
        .find_map(|d| exefs_child(d).or_else(|| is_exefs(d).then(|| d.clone())))
}

/// The game's root for a dump: the parent of an `exefs/` directory, else (a flat folder) the ExeFS folder itself.
fn game_root(dir: &Path) -> Option<PathBuf> {
    let ex = find_exefs(dir)?;
    let named = ex
        .file_name()
        .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("exefs"));
    Some(if named {
        ex.parent()?.to_path_buf()
    } else {
        ex
    })
}

/// The dump's RomFS folder, if it has one.
pub fn find_romfs(dir: &Path) -> Option<PathBuf> {
    let root = game_root(dir)?;
    ["romfs", "RomFS", "ROMFS"]
        .iter()
        .map(|n| root.join(n))
        .find(|p| p.is_dir())
}

/// `control.nacp` and the icon, from the game's root or its `control/` folder (the names a build's data folder uses).
pub fn control(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let Some(root) = game_root(dir) else {
        return out;
    };
    for d in [root.clone(), root.join("control"), root.join("Control")] {
        for name in ["control.nacp", "icon_AmericanEnglish.dat"] {
            if !out.contains_key(name) {
                if let Ok(b) = std::fs::read(d.join(name)) {
                    out.insert(name.to_string(), b);
                }
            }
        }
    }
    out
}

pub fn exefs(dir: &Path) -> Result<ExeFs> {
    let Some(exefs) = find_exefs(dir) else {
        return fmt(format!("{}: no ExeFS (a `main` file) in the folder's exefs/, the folder, its exefs/ or one level deeper", dir.display()));
    };
    let mut files = BTreeMap::new();
    for name in EXEFS_NAMES {
        let p = exefs.join(name);
        if p.is_file() {
            let bytes = std::fs::read(&p).map_err(|source| PackageError::Io {
                path: p.clone(),
                source,
            })?;
            files.insert(name.to_string(), bytes);
        }
    }
    Ok(ExeFs {
        kind: SourceKind::Folder,
        title_id: None,
        version: None,
        files,
    })
}

pub fn list(dir: &Path) -> Result<Vec<String>> {
    let Some(exefs) = find_exefs(dir) else {
        return fmt(format!("{}: no ExeFS found", dir.display()));
    };
    let mut out = vec![format!(
        "Folder {} (exefs: {})",
        dir.display(),
        exefs.display()
    )];
    let mut entries: Vec<_> = std::fs::read_dir(&exefs)
        .map_err(|source| PackageError::Io {
            path: exefs.clone(),
            source,
        })?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let len = e.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(format!(
            "  exefs/{:<16} {:>12}",
            e.file_name().to_string_lossy(),
            len
        ));
    }
    match find_romfs(dir) {
        Some(r) => out.push(format!("  RomFS: {}", r.display())),
        None => out.push("  no RomFS (built with an empty one)".into()),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_exefs_layouts() {
        let root = std::env::temp_dir().join(format!("nx-package-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("game");
        let ex = game.join("exefs");
        std::fs::create_dir_all(&ex).unwrap();
        std::fs::create_dir_all(game.join("romfs")).unwrap();
        std::fs::write(ex.join("main"), b"NSO0").unwrap();
        std::fs::write(ex.join("main.npdm"), b"META").unwrap();
        std::fs::write(ex.join("junk.txt"), b"x").unwrap();
        assert_eq!(find_exefs(&ex).unwrap(), ex); // folder is the ExeFS
        assert_eq!(find_exefs(&game).unwrap(), ex); // exefs/ child
        assert_eq!(find_exefs(&root).unwrap(), ex); // one level deeper
        let e = exefs(&root).unwrap();
        assert_eq!(
            e.files.keys().cloned().collect::<Vec<_>>(),
            vec!["main", "main.npdm"]
        );
        assert_eq!(find_romfs(&root).unwrap(), game.join("romfs"));
        // a flat folder (Turbo): the ExeFS files, romfs/ and control/ side by side; no RomFS is allowed too
        let flat = root.join("flat");
        std::fs::create_dir_all(flat.join("control")).unwrap();
        std::fs::write(flat.join("main"), b"NSO0").unwrap();
        std::fs::write(flat.join("control/control.nacp"), b"NACP").unwrap();
        assert_eq!(find_exefs(&flat).unwrap(), flat);
        assert_eq!(find_romfs(&flat), None, "no RomFS: optional");
        assert_eq!(
            control(&flat).get("control.nacp").map(|v| v.as_slice()),
            Some(&b"NACP"[..])
        );
        std::fs::create_dir_all(flat.join("RomFS")).unwrap();
        assert!(find_romfs(&flat)
            .unwrap()
            .to_string_lossy()
            .eq_ignore_ascii_case(&flat.join("RomFS").to_string_lossy()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
