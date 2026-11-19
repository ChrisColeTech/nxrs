//! A dumped game folder: either it holds an `exefs/` directory (with `romfs/` beside it), or the folder itself is an
//! ExeFS; one level deeper is searched too. A directory counts as an ExeFS when it has a `main` file (`main.npdm`
//! is optional, as in Turbo's loader). The `exefs/` child is preferred over the folder itself.
// TODO(romfs/bktr): the `romfs/` directory beside the ExeFS is not read.
use crate::package::error::{fmt, PackageError, Result};
use crate::package::{ExeFs, SourceKind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const EXEFS_NAMES: [&str; 15] = [
    "main", "main.npdm", "rtld", "sdk", "subsdk0", "subsdk1", "subsdk2", "subsdk3", "subsdk4", "subsdk5", "subsdk6",
    "subsdk7", "subsdk8", "subsdk9", "subsdk10",
];

fn is_exefs(dir: &Path) -> bool {
    dir.join("main").is_file()
}

fn exefs_child(dir: &Path) -> Option<PathBuf> {
    ["exefs", "ExeFS", "EXEFS"].iter().map(|n| dir.join(n)).find(|p| is_exefs(p))
}

/// The ExeFS directory for a dump folder, if any.
pub fn find_exefs(dir: &Path) -> Option<PathBuf> {
    if let Some(p) = exefs_child(dir) {
        return Some(p);
    }
    if is_exefs(dir) {
        return Some(dir.to_path_buf());
    }
    let mut subdirs: Vec<PathBuf> =
        std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subdirs.sort();
    subdirs.iter().find_map(|d| exefs_child(d).or_else(|| is_exefs(d).then(|| d.clone())))
}

pub fn exefs(dir: &Path) -> Result<ExeFs> {
    let Some(exefs) = find_exefs(dir) else {
        return fmt(format!("{}: no ExeFS (a `main` file) in the folder's exefs/, the folder, its exefs/ or one level deeper", dir.display()));
    };
    let mut files = BTreeMap::new();
    for name in EXEFS_NAMES {
        let p = exefs.join(name);
        if p.is_file() {
            let bytes = std::fs::read(&p).map_err(|source| PackageError::Io { path: p.clone(), source })?;
            files.insert(name.to_string(), bytes);
        }
    }
    Ok(ExeFs { kind: SourceKind::Folder, title_id: None, version: None, files })
}

pub fn list(dir: &Path) -> Result<Vec<String>> {
    let Some(exefs) = find_exefs(dir) else {
        return fmt(format!("{}: no ExeFS found", dir.display()));
    };
    let mut out = vec![format!("Folder {} (exefs: {})", dir.display(), exefs.display())];
    let mut entries: Vec<_> = std::fs::read_dir(&exefs)
        .map_err(|source| PackageError::Io { path: exefs.clone(), source })?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let len = e.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(format!("  exefs/{:<16} {:>12}", e.file_name().to_string_lossy(), len));
    }
    if let Some(parent) = exefs.parent() {
        if parent.join("romfs").is_dir() {
            out.push("  romfs/ present (not read)".into());
        }
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
        assert_eq!(e.files.keys().cloned().collect::<Vec<_>>(), vec!["main", "main.npdm"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
