//! fs: FSUSER over the build's staged `romfs/` (read), a `<data>\save` folder (read/write) and a
//! synthetic empty `sdmc/` root. Command ids are the 3dbrew `Filesystem services` headers' high 16
//! bits (the full header's low bits are normal/translate counts). The all-zero 0xC-byte binary
//! file-lowpath is the RomFS IVFC image (3dbrew `RomFS`), served read-only from the dump range.

use crate::hle::{
    FileSource, Host, Reply, Request, R_END_OF_FILE, R_INVALID_ARCHIVE, R_INVALID_CMD, R_OK,
    R_PATH_NOT_FOUND,
};
use runtime3ds::Process3ds;
use std::path::{Component, Path, PathBuf};

/// 3dbrew `FS` result: a file or directory already exists at the path
pub const R_ALREADY_EXISTS: u32 = 0x0820_44BE;
/// 3dbrew `FS` result: the archive or file is not writable
pub const R_WRITE_DENIED: u32 = 0xE0E0_46BE;

pub const INITIALIZE: u32 = 0x0801;
pub const OPEN_FILE: u32 = 0x0802;
pub const OPEN_FILE_DIRECTLY: u32 = 0x0803;
pub const DELETE_FILE: u32 = 0x0804;
pub const RENAME_FILE: u32 = 0x0805;
pub const DELETE_DIRECTORY: u32 = 0x0806;
pub const CREATE_FILE: u32 = 0x0808;
pub const CREATE_DIRECTORY: u32 = 0x0809;
pub const RENAME_DIRECTORY: u32 = 0x080A;
pub const OPEN_DIRECTORY: u32 = 0x080B;
pub const OPEN_ARCHIVE: u32 = 0x080C;
pub const CLOSE_ARCHIVE: u32 = 0x080E;
pub const GET_FREE_BYTES: u32 = 0x0812;
pub const GET_SDMC_ARCHIVE_RESOURCE: u32 = 0x0814;
pub const IS_SDMC_DETECTED: u32 = 0x0817;
pub const IS_SDMC_WRITABLE: u32 = 0x0818;

// file-session commands
pub const FILE_READ: u32 = 0x0802;
pub const FILE_WRITE: u32 = 0x0803;
pub const FILE_GET_SIZE: u32 = 0x0804;
pub const FILE_SET_SIZE: u32 = 0x0805;
pub const FILE_GET_ATTRIBUTES: u32 = 0x0806;
pub const FILE_CLOSE: u32 = 0x0808;
pub const FILE_FLUSH: u32 = 0x0809;

// directory-session commands
pub const DIR_READ: u32 = 0x0801;
pub const DIR_CLOSE: u32 = 0x0802;

// archive ids (3dbrew `Filesystem services` §Archives)
pub const ARCHIVE_SELF_NCCH: u32 = 0x0000_0003;
pub const ARCHIVE_SAVEDATA: u32 = 0x0000_0004;
pub const ARCHIVE_EXTDATA: u32 = 0x0000_0006;
pub const ARCHIVE_SHARED_EXTDATA: u32 = 0x0000_0007;
pub const ARCHIVE_SYSTEM_SAVEDATA: u32 = 0x0000_0008;
pub const ARCHIVE_SDMC: u32 = 0x0000_0009;

/// archives that keep their bytes in the W3 save root (writable)
fn is_writable(kind: u32) -> bool {
    matches!(
        kind,
        ARCHIVE_SAVEDATA | ARCHIVE_EXTDATA | ARCHIVE_SHARED_EXTDATA | ARCHIVE_SYSTEM_SAVEDATA | ARCHIVE_SDMC
    )
}

/// a path string from the request (3dbrew PathType: 1 empty, 2 binary, 3 ASCII, 4 UTF-16LE)
fn read_path(process: &Process3ds, path_type: u32, size: u32, address: u32) -> String {
    if path_type == 1 {
        return String::new();
    }
    let size = size.min(0x400) as usize;
    let mut bytes = Vec::with_capacity(size);
    for i in 0..size {
        bytes.push(process.memory.read8(address.wrapping_add(i as u32)));
    }
    if path_type == 2 {
        // a binary lowpath is per-archive; a hex name keeps each one distinct host-side
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    if path_type == 4 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|&unit| unit != 0)
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }
}

/// strips a `x:/` prefix and normalises separators, refusing `..`
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let mut relative = path.trim_start_matches('/').replace('\\', "/");
    if let Some((_prefix, rest)) = relative.split_once(':') {
        relative = rest.trim_start_matches('/').to_owned();
    }
    let mut out = root.to_path_buf();
    for component in Path::new(&relative).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// the names in a hosted directory, sorted
fn list_dir(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .map(|dir| dir.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn archive_root(host: &Host, kind: u32) -> Option<PathBuf> {
    Some(match kind {
        ARCHIVE_SELF_NCCH => host.romfs_dir.clone(),
        ARCHIVE_SAVEDATA => host.save_dir.clone(),
        ARCHIVE_EXTDATA | ARCHIVE_SHARED_EXTDATA => host.save_dir.join("extdata"),
        ARCHIVE_SYSTEM_SAVEDATA => host.save_dir.join("sysdata"),
        ARCHIVE_SDMC => host.sdmc_dir.clone(),
        _ => return None,
    })
}

fn archive_kind_of_path(path: &str) -> Option<u32> {
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("romfs") {
        Some(ARCHIVE_SELF_NCCH)
    } else if lower.starts_with("sdmc") {
        Some(ARCHIVE_SDMC)
    } else if lower.starts_with("save") {
        Some(ARCHIVE_SAVEDATA)
    } else {
        None
    }
}

/// FS open flags (3dbrew `FS:OpenFile`): bit 0 read, bit 1 write, bit 2 create
pub const OPEN_WRITE: u32 = 1 << 1;
pub const OPEN_CREATE: u32 = 1 << 2;

/// opens a file from an archive kind, streaming the bytes into the session; a missing file is
/// created only when the open flags ask for it on a writable archive
fn open_file(host: &Host, kind: u32, path: &str, flags: u32) -> Option<(u32, u32)> {
    let root = archive_root(host, kind)?;
    let full = resolve(&root, path)?;
    let display = full.display().to_string();
    let writable = is_writable(kind) && flags & OPEN_WRITE != 0;
    match std::fs::read(&full) {
        Ok(bytes) => Some(host.open_file_session(FileSource::Memory(bytes), display, writable)),
        Err(_) => {
            if writable && flags & OPEN_CREATE != 0 {
                if let Some(parent) = full.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                Some(host.open_file_session(FileSource::Memory(Vec::new()), display, true))
            } else {
                None
            }
        }
    }
}

/// SelfNCCH (archive 3) file lowpaths are binary, 0xC bytes (3dbrew `Filesystem services`
/// §SelfNCCH): word 0 = 0 names this title's RomFS, served as the dump's IVFC level-3 image
/// (the SDK parses the RomFS itself). Other kinds (ExeFS sections, update RomFS) are refused.
fn open_self_ncch(host: &Host, path_type: u32, lowpath: &[u8]) -> Reply {
    let romfs = path_type == 2 && lowpath.len() >= 4 && lowpath[..4] == [0, 0, 0, 0];
    if !romfs {
        host.gap(format!("fs: SelfNCCH lowpath {lowpath:02x?} (type {path_type}) is not served"));
        return Reply::err(R_PATH_NOT_FOUND);
    }
    let Some((image, base, size)) = host.romfs_image() else {
        host.gap("fs: SelfNCCH RomFS requested but the dump's RomFS image was not found");
        return Reply::err(R_PATH_NOT_FOUND);
    };
    let (handle, _id) = host.open_image_session(image, base, size);
    host.say(format!("fs: SelfNCCH RomFS -> image file {handle:#x} ({size:#x} bytes)"));
    Reply::new(vec![R_OK, 0x10, handle], 2)
}

/// the archive kind behind an archive handle, refused unless the archive is open and writable
fn writable_archive(host: &Host, archive: u32) -> Result<u32, Reply> {
    let kind = host.state.lock().unwrap().archives.get(&archive).map(|a| a.kind);
    match kind {
        None => Err(Reply::err(R_INVALID_ARCHIVE)),
        Some(kind) if !is_writable(kind) => {
            host.gap(format!("fs: a write to the read-only archive {kind:#x} refused"));
            Err(Reply::err(R_WRITE_DENIED))
        }
        Some(kind) => Ok(kind),
    }
}

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        INITIALIZE => Reply::ok(),
        OPEN_ARCHIVE => {
            let kind = request.word(0);
            if archive_root(host, kind).is_none() {
                host.gap(format!("fs: OpenArchive id {kind:#x} is not served"));
                return Reply::err(R_INVALID_ARCHIVE);
            }
            if let Some(root) = archive_root(host, kind) {
                let _ = std::fs::create_dir_all(&root);
            }
            let path_type = request.word(1);
            let path_size = request.word(2);
            let path_ptr = request.translated.get(1).copied().unwrap_or(0);
            let path = read_path(process, path_type, path_size, path_ptr);
            let mut state = host.state.lock().unwrap();
            let handle = state.fresh_handle();
            state.archives.insert(handle, crate::hle::Archive { kind });
            drop(state);
            host.say(format!("fs: OpenArchive id {kind:#x} {path:?} -> {handle:#x}"));
            Reply::new(vec![R_OK, handle, 0], 0)
        }
        CLOSE_ARCHIVE => {
            let handle = request.word(0);
            host.state.lock().unwrap().archives.remove(&handle);
            Reply::ok()
        }
        OPEN_FILE => {
            let archive = request.word(1);
            let path_type = request.word(3);
            let path_size = request.word(4);
            let path_ptr = request.translated.get(1).copied().unwrap_or(0);
            let path = read_path(process, path_type, path_size, path_ptr);
            let kind = host.state.lock().unwrap().archives.get(&archive).map(|a| a.kind);
            let Some(kind) = kind else {
                host.gap(format!("fs: OpenFile on an unknown archive {archive:#x}"));
                return Reply::err(R_INVALID_ARCHIVE);
            };
            if kind == ARCHIVE_SELF_NCCH {
                let mut lowpath = vec![0u8; path_size.min(32) as usize];
                process.memory.read_bytes(path_ptr, &mut lowpath);
                return open_self_ncch(host, path_type, &lowpath);
            }
            match open_file(host, kind, &path, request.word(5)) {
                Some((handle, _id)) => {
                    host.say(format!("fs: OpenFile {path:?} -> {handle:#x}"));
                    Reply::new(vec![R_OK, 0, handle], 2)
                }
                None => {
                    host.say(format!("fs: OpenFile {path:?}: not found"));
                    Reply::err(R_PATH_NOT_FOUND)
                }
            }
        }
        OPEN_FILE_DIRECTLY => {
            let archive_id = request.word(1);
            let archive_type = request.word(2);
            let archive_size = request.word(3);
            let file_type = request.word(4);
            let file_size = request.word(5);
            let archive_ptr = request.translated.get(1).copied().unwrap_or(0);
            let file_ptr = request.translated.get(3).copied().unwrap_or(0);
            let archive_path = read_path(process, archive_type, archive_size, archive_ptr);
            let file_path = read_path(process, file_type, file_size, file_ptr);
            let mut file_bytes = vec![0u8; file_size.min(32) as usize];
            process.memory.read_bytes(file_ptr, &mut file_bytes);
            host.say(format!(
                "fs: OpenFileDirectly id {archive_id:#x} archive {archive_path:?} file {file_path:?} ({file_size} bytes)"
            ));
            let kind = archive_kind_of_path(&archive_path)
                .or_else(|| archive_kind_of_path(&file_path))
                .unwrap_or(archive_id);
            if archive_root(host, kind).is_none() {
                host.gap(format!("fs: OpenFileDirectly archive id {kind:#x} is not served"));
                return Reply::err(R_INVALID_ARCHIVE);
            }
            if kind == ARCHIVE_SELF_NCCH {
                return open_self_ncch(host, file_type, &file_bytes);
            }
            match open_file(host, kind, &file_path, request.word(6)) {
                Some((handle, _id)) => {
                    host.say(format!("fs: OpenFileDirectly {file_path:?} -> {handle:#x}"));
                    Reply::new(vec![R_OK, 0, handle], 2)
                }
                None => {
                    host.say(format!("fs: OpenFileDirectly {file_path:?}: not found"));
                    Reply::err(R_PATH_NOT_FOUND)
                }
            }
        }
        OPEN_DIRECTORY => {
            let archive = request.word(1);
            let path_type = request.word(3);
            let path_size = request.word(4);
            let path_ptr = request.translated.get(1).copied().unwrap_or(0);
            let path = read_path(process, path_type, path_size, path_ptr);
            let kind = host.state.lock().unwrap().archives.get(&archive).map(|a| a.kind);
            let Some(kind) = kind else {
                return Reply::err(R_INVALID_ARCHIVE);
            };
            let root = archive_root(host, kind).unwrap_or_default();
            let full = resolve(&root, &path).unwrap_or_default();
            let names = list_dir(&full);
            let (handle, _id) = host.open_dir_session(names);
            host.say(format!("fs: OpenDirectory {path:?} -> {handle:#x}"));
            Reply::new(vec![R_OK, 0, handle], 2)
        }
        CREATE_DIRECTORY => {
            let archive = request.word(1);
            let path_type = request.word(3);
            let path_size = request.word(4);
            let path_ptr = request.translated.get(1).copied().unwrap_or(0);
            let path = read_path(process, path_type, path_size, path_ptr);
            let kind = match writable_archive(host, archive) {
                Ok(kind) => kind,
                Err(reply) => return reply,
            };
            let Some(full) = archive_root(host, kind).and_then(|root| resolve(&root, &path)) else {
                return Reply::err(R_PATH_NOT_FOUND);
            };
            if full.exists() {
                return Reply::err(R_ALREADY_EXISTS);
            }
            match std::fs::create_dir_all(&full) {
                Ok(()) => Reply::ok(),
                Err(_) => Reply::err(R_PATH_NOT_FOUND),
            }
        }
        CREATE_FILE => {
            // 3dbrew FS:CreateFile: word2-3 archive, word4 path type, word5 path size,
            // word6 attributes, word7-8 zero-fill size, translate = path
            let archive = request.word(1);
            let path_type = request.word(3);
            let path_size = request.word(4);
            let zero_fill = (request.word(6) as u64) | ((request.word(7) as u64) << 32);
            let path_ptr = request.translated.get(1).copied().unwrap_or(0);
            let path = read_path(process, path_type, path_size, path_ptr);
            host.say(format!(
                "fs: CreateFile archive {archive:#x} {path:?} ({path_size} bytes, type {path_type}, fill {zero_fill})"
            ));
            let kind = match writable_archive(host, archive) {
                Ok(kind) => kind,
                Err(reply) => return reply,
            };
            let Some(full) = archive_root(host, kind).and_then(|root| resolve(&root, &path)) else {
                return Reply::err(R_PATH_NOT_FOUND);
            };
            if full.exists() {
                return Reply::err(R_ALREADY_EXISTS);
            }
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&full, vec![0u8; zero_fill.min(0x1000_0000) as usize]) {
                Ok(()) => Reply::ok(),
                Err(_) => Reply::err(R_PATH_NOT_FOUND),
            }
        }
        DELETE_FILE | DELETE_DIRECTORY => {
            // 3dbrew FS:DeleteFile/DeleteDirectory: word 2-3 archive, word 4 path type, word 5
            // path size; translate = the path
            let archive = request.word(1);
            let path_ptr = request.translated.get(1).copied().unwrap_or(0);
            let path = read_path(process, request.word(3), request.word(4), path_ptr);
            let kind = match writable_archive(host, archive) {
                Ok(kind) => kind,
                Err(reply) => return reply,
            };
            let Some(full) = archive_root(host, kind).and_then(|root| resolve(&root, &path)) else {
                return Reply::err(R_PATH_NOT_FOUND);
            };
            let removed = if request.cmd == DELETE_FILE {
                std::fs::remove_file(&full)
            } else {
                std::fs::remove_dir(&full)
            };
            host.say(format!("fs: delete {path:?} -> {}", removed.is_ok()));
            match removed {
                Ok(()) => Reply::ok(),
                Err(_) => Reply::err(R_PATH_NOT_FOUND),
            }
        }
        RENAME_FILE | RENAME_DIRECTORY => {
            // 3dbrew FS:RenameFile: words 2-3 source archive, 4/5 source path type/size, 6-7
            // destination archive, 8/9 its path type/size; translate = both paths
            let (source_archive, target_archive) = (request.word(1), request.word(5));
            let source_ptr = request.translated.get(1).copied().unwrap_or(0);
            let target_ptr = request.translated.get(3).copied().unwrap_or(0);
            let source = read_path(process, request.word(3), request.word(4), source_ptr);
            let target = read_path(process, request.word(7), request.word(8), target_ptr);
            let kinds = (writable_archive(host, source_archive), writable_archive(host, target_archive));
            let (source_kind, target_kind) = match kinds {
                (Ok(a), Ok(b)) => (a, b),
                (Err(reply), _) | (_, Err(reply)) => return reply,
            };
            let from = archive_root(host, source_kind).and_then(|root| resolve(&root, &source));
            let to = archive_root(host, target_kind).and_then(|root| resolve(&root, &target));
            let (Some(from), Some(to)) = (from, to) else {
                return Reply::err(R_PATH_NOT_FOUND);
            };
            if to.exists() {
                return Reply::err(R_ALREADY_EXISTS);
            }
            match std::fs::rename(&from, &to) {
                Ok(()) => Reply::ok(),
                Err(_) => Reply::err(R_PATH_NOT_FOUND),
            }
        }
        GET_FREE_BYTES => Reply::new(vec![R_OK, 0, 0x4000_0000], 0),
        GET_SDMC_ARCHIVE_RESOURCE => Reply::new(
            vec![R_OK, 0, 0x4000_0000, 0x4000_0000, 0x4000_0000, 0x200],
            0,
        ),
        IS_SDMC_DETECTED | IS_SDMC_WRITABLE => Reply::new(vec![R_OK, 1], 0),
        other => {
            host.gap(format!("fs: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

pub fn file_command(
    host: &Host,
    process: &Process3ds,
    _tls: u32,
    id: u32,
    request: &Request,
) -> Reply {
    match request.cmd {
        FILE_READ => {
            // 3dbrew FSFile:Read: words 1-2 = offset, word 3 = size, translate = buffer
            let offset = (request.word(0) as u64) | ((request.word(1) as u64) << 32);
            let size = request.word(2) as usize;
            let buffer = request.translated.get(1).copied().unwrap_or(0);
            let state = host.state.lock().unwrap();
            let Some(file) = state.files.get(&id) else {
                return Reply::err(R_INVALID_ARCHIVE);
            };
            if offset > file.source.len() {
                return Reply::err(R_END_OF_FILE);
            }
            let chunk = file.source.read(offset, size);
            process.memory.write_bytes(buffer, &chunk);
            Reply::new(vec![R_OK, chunk.len() as u32], 0)
        }
        FILE_WRITE => {
            // 3dbrew FSFile:Write: words 1-2 = offset, word 3 = size, word 4 = option, translate
            let offset = (request.word(0) as u64) | ((request.word(1) as u64) << 32);
            let size = request.word(2) as usize;
            let buffer = request.translated.get(1).copied().unwrap_or(0);
            let mut state = host.state.lock().unwrap();
            let Some(file) = state.files.get_mut(&id) else {
                return Reply::err(R_INVALID_ARCHIVE);
            };
            if !file.writable {
                return Reply::err(R_WRITE_DENIED);
            }
            let mut chunk = vec![0u8; size];
            process.memory.read_bytes(buffer, &mut chunk);
            if file.write(offset as usize, &chunk) {
                file.pos = offset + size as u64;
                // response: result, bytes written
                Reply::new(vec![R_OK, size as u32], 0)
            } else {
                Reply::err(R_INVALID_ARCHIVE)
            }
        }
        FILE_GET_SIZE => {
            let state = host.state.lock().unwrap();
            let Some(file) = state.files.get(&id) else {
                return Reply::err(R_INVALID_ARCHIVE);
            };
            let size = file.source.len();
            Reply::new(vec![R_OK, size as u32, (size >> 32) as u32], 0)
        }
        FILE_SET_SIZE => {
            let size = ((request.word(0) as u64) | ((request.word(1) as u64) << 32)) as usize;
            let mut state = host.state.lock().unwrap();
            let Some(file) = state.files.get_mut(&id) else {
                return Reply::err(R_INVALID_ARCHIVE);
            };
            file.set_size(size);
            Reply::ok()
        }
        FILE_GET_ATTRIBUTES => Reply::new(vec![R_OK, 0], 0),
        FILE_FLUSH => {
            let mut state = host.state.lock().unwrap();
            if let Some(file) = state.files.get_mut(&id) {
                if file.dirty {
                    if let FileSource::Memory(bytes) = &file.source {
                        let _ = std::fs::write(&file.path, bytes);
                    }
                    file.dirty = false;
                }
            }
            Reply::ok()
        }
        FILE_CLOSE => {
            let mut state = host.state.lock().unwrap();
            if let Some(file) = state.files.remove(&id) {
                if file.dirty {
                    if let FileSource::Memory(bytes) = &file.source {
                        let _ = std::fs::write(&file.path, bytes);
                    }
                }
            }
            state.sessions.retain(|_, service| *service != crate::hle::Service::File(id));
            Reply::ok()
        }
        other => {
            host.gap(format!("fs: file command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

pub fn dir_command(
    host: &Host,
    process: &Process3ds,
    _tls: u32,
    id: u32,
    request: &Request,
) -> Reply {
    match request.cmd {
        DIR_READ => {
            // request: word1 = max entries; translate: static buffer descriptor + pointer
            let max = request.word(0) as usize;
            let descriptor = request.translated.first().copied().unwrap_or(0);
            let buffer = request.translated.get(1).copied().unwrap_or(0);
            let buffer_entries = ((descriptor >> 14) as usize) / 0x228;
            let take = if max == 0 { buffer_entries } else { max.min(buffer_entries) };
            let mut state = host.state.lock().unwrap();
            let Some(dir) = state.dirs.get_mut(&id) else {
                return Reply::err(R_INVALID_ARCHIVE);
            };
            let start = dir.index;
            let end = (start + take).min(dir.entries.len());
            let names: Vec<String> = dir.entries[start..end].to_vec();
            dir.index = end;
            let count = names.len();
            drop(state);
            for (slot, name) in names.iter().enumerate() {
                let at = buffer + (slot * 0x228) as u32;
                let mut entry = vec![0u8; 0x228];
                let units: Vec<u16> = name.encode_utf16().take(0x106 - 1).collect();
                for (i, unit) in units.iter().enumerate() {
                    entry[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
                }
                entry[0x21A] = 1;
                process.memory.write_bytes(at, &entry);
            }
            Reply::new(vec![R_OK, count as u32], 0)
        }
        DIR_CLOSE => {
            host.state.lock().unwrap().dirs.remove(&id);
            Reply::ok()
        }
        other => {
            host.gap(format!("fs: directory command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_with_parent_dirs_is_refused() {
        let root = Path::new("C:/data");
        assert!(resolve(root, "System/hello.bin").is_some());
        assert!(resolve(root, "../escape").is_none());
        assert!(resolve(root, "/System/x").is_some());
    }
}
