//! Files and folders: reading, atomic writing, copying trees.

use crate::constants::words_play as w;
use crate::model::path_text::{join, native};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub fn is_file(path: &str) -> bool {
    !path.is_empty() && Path::new(path).is_file()
}
pub fn is_dir(path: &str) -> bool {
    !path.is_empty() && Path::new(path).is_dir()
}
pub fn exists(path: &str) -> bool {
    !path.is_empty() && Path::new(path).exists()
}
pub fn size(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The file as text (UTF-8, lossy); None when it cannot be read.
pub fn read_text(path: &str) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Up to `len` bytes from `at`.
pub fn read_at(path: &str, at: u64, len: usize) -> Result<Vec<u8>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(len as u64)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Writes beside the file and renames over it: a reader never sees half a file.
pub fn write_atomic(path: &str, bytes: &[u8]) -> Result<(), String> {
    let target = Path::new(path);
    if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

pub fn make_dir(path: &str) -> bool {
    !path.is_empty() && std::fs::create_dir_all(path).is_ok()
}

/// The names in a folder: (name, is a folder).
pub fn list(dir: &str) -> Vec<(String, bool)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, bool)> = entries
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.file_type().is_ok_and(|t| t.is_dir()),
            )
        })
        .collect();
    out.sort();
    out
}

/// A folder with at least one file anywhere under it.
pub fn has_files(path: &str) -> bool {
    list(path)
        .iter()
        .any(|(name, dir)| !*dir || has_files(&join(path, name)))
}

/// Copies `from` into `to` (created), replacing files there; counts the files.
pub fn copy_tree(from: &str, to: &str, files: &mut usize) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|_| format!("{}{}", w::CANNOT_CREATE, native(to)))?;
    for (name, dir) in list(from) {
        let (source, target) = (join(from, &name), join(to, &name));
        if dir {
            copy_tree(&source, &target, files)?;
            continue;
        }
        if exists(&target) && std::fs::remove_file(&target).is_err() {
            return Err(format!("{}{}", w::CANNOT_REPLACE, native(&target)));
        }
        std::fs::copy(&source, &target)
            .map_err(|_| format!("{}{}", w::CANNOT_COPY, native(&source)))?;
        *files += 1;
    }
    Ok(())
}

pub fn remove_tree(path: &str) -> bool {
    std::fs::remove_dir_all(path).is_ok()
}

pub fn copy_file(from: &str, to: &str) -> Result<(), String> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A file's size and modification time (ms), what an image cache keys on; None for no file.
pub fn stamp(path: &str) -> Option<(u64, u128)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    meta.is_file().then_some((meta.len(), modified))
}

/// The bytes of a whole file.
pub fn read_bytes(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| e.to_string())
}
