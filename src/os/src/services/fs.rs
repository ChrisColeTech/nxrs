//! nn::fs: on the console it goes through fsp-srv; here it reaches the game's packages and host folders directly.
//!
//! | Mount | Backed by |
//! |---|---|
//! | `MountRom` | the game's RomFS, read in place from its package with the update applied (`translator::package`) |
//! | `MountAddOnContent(index)` | the add-on title's RomFS (`dlc::title_romfs`, installed into the data folder) |
//! | `MountSaveData(uid)`, `MountDeviceSaveData` | `<save dir>/user<n>`, `<save dir>/device`: transactional (below) |
//! | `MountTemporaryStorage`, `MountCacheStorage` | `<save dir>/temp`, `<save dir>/cache<n>`, written directly |
//! | `MountRom(name, program index)` | `<data dir>/romfs_programs/<index>.img` (another program's RomFS image), else the game's |
//! | `MountSystemData` | the console's own archive under `<save dir>/system/<id>`, else "target not found" |
//!
//! Save transactions: a mount works on `<save>.work`, a copy of the committed `<save>` made at the first mount;
//! `Commit` copies the work tree to `<save>.new`, moves the committed one to `<save>.old`, `<save>.new` into its
//! place and deletes the old one. A run killed at any point leaves the committed save whole (a `.new` left with no
//! committed save is taken as committed at the next mount).
//!
//! Cache storages: storage `<n>` is the folder `cache<n>`; CreateCacheStorage records its size beside it
//! (`cache<n>.size`, "<data> <journal>"), which GetCacheStorageSize reports (else the control data's default, else
//! 256 MB of each); the list is the cache folders that exist, in index order. The limits come from the control
//! data (`<data dir>/control.nacp`: CacheStorageDataAndJournalSizeMax at 0x3180, CacheStorageIndexMax at 0x3188).
//! A Commit of a mount list commits it only when every transactional mount in it is the same save (one host
//! transaction); a list that spans two saves is refused before either is published.
//!
//! Handles: a FileHandle the guest holds is the host's number shifted left by 4 (16-byte aligned, as the SDK's
//! FileAccessor pointers are: games mask low bits off). DirectoryEntry is {name[0x301], pad[3], i8 type, pad[3],
//! i64 size}.

use crate::guest::{cstr, w32, w64, w8, Args};
use crate::hle::{Registry, RES_FAIL, RES_OK};
use nxrt::cpu::Cpu;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use translator::package::RomFs;

/// Positioned reads and writes on Linux with the names Windows' `FileExt` gives them (pread / pwrite).
#[cfg(unix)]
pub(crate) mod posio {
    pub trait FileExt {
        fn seek_read(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize>;
        fn seek_write(&self, buf: &[u8], offset: u64) -> std::io::Result<usize>;
    }
    impl FileExt for std::fs::File {
        fn seek_read(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
            std::os::unix::fs::FileExt::read_at(self, buf, offset)
        }
        fn seek_write(&self, buf: &[u8], offset: u64) -> std::io::Result<usize> {
            std::os::unix::fs::FileExt::write_at(self, buf, offset)
        }
    }
}

pub const RES_PATH_NOT_FOUND: u64 = 0x202;
pub const RES_PATH_EXISTS: u64 = 0x402;
pub const RES_TARGET_NOT_FOUND: u64 = 0x7D402;
pub const RES_OUT_OF_RANGE: u64 = 0x2EE202; // fs 6001: InvalidOffset
#[cfg(test)]
const MODE_READ: u32 = 1;
const MODE_WRITE: u32 = 2;
const MODE_APPEND: u32 = 4;
const ENT_DIR: i8 = 0;
const ENT_FILE: i8 = 1;
const DIRMODE_DIR: u32 = 1;
const DIRMODE_FILE: u32 = 2;
const DIRENT_SIZE: u64 = 0x310;

#[derive(Clone)]
enum Backing {
    Rom(Arc<RomFs>),
    Dir { root: PathBuf, ro: bool, txn: Option<usize> },
}

struct Txn {
    committed: PathBuf,
    work: PathBuf,
}

enum Open {
    Rom { fs: Arc<RomFs>, path: String, size: u64 },
    Host { file: File, #[allow(dead_code)] txn: Option<usize> },
}

struct FileH {
    open: Open,
    mode: u32,
}

struct DirH {
    entries: Vec<(String, i8, u64)>,
    pos: usize,
    mode: u32,
}

#[derive(Default)]
struct State {
    mounts: HashMap<String, Backing>,
    txns: Vec<Txn>,
    files: HashMap<u64, FileH>,
    dirs: HashMap<u64, DirH>,
    next: u64,
}

static ST: Mutex<Option<State>> = Mutex::new(None);
static ROMFS: OnceLock<Option<Arc<RomFs>>> = OnceLock::new();

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = ST.lock().unwrap();
    f(g.get_or_insert_with(|| State { next: 1, ..Default::default() }))
}

pub fn data_dir() -> PathBuf {
    crate::boot::try_process().map(|p| p.data_dir.clone()).unwrap_or_else(|| PathBuf::from("."))
}

pub fn save_dir() -> PathBuf {
    crate::boot::try_process().map(|p| p.save_dir.clone()).unwrap_or_else(|| PathBuf::from("save"))
}

/// The game's RomFS, opened at the first mount.
pub fn romfs() -> Option<Arc<RomFs>> {
    ROMFS
        .get_or_init(|| {
            let t0 = std::time::Instant::now();
            match translator::package::open_data_dir(&data_dir(), None) {
                Ok(r) => {
                    crate::log!("fs: RomFS from {} ({} files, {:.2} s)", r.source, r.walk().len(), t0.elapsed().as_secs_f64());
                    Some(Arc::new(r))
                }
                Err(e) => {
                    crate::log!("[RED] fs: the RomFS cannot be opened: {e}");
                    None
                }
            }
        })
        .clone()
}

/// Whether this is the game's own RomFS (the one the player's mods layer over).
fn is_game(fs: &Arc<RomFs>) -> bool {
    ROMFS.get().and_then(|r| r.as_ref()).is_some_and(|g| Arc::ptr_eq(g, fs))
}

/// Splits `mount:/a/b` into the mount and the normalized path (`a/b`).
pub fn split(path: &str) -> Option<(String, String)> {
    let (m, rest) = path.split_once(':')?;
    let mut parts: Vec<&str> = vec![];
    for p in rest.split(['/', '\\']) {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    Some((m.to_string(), parts.join("/")))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &t)?;
        } else {
            std::fs::copy(e.path(), t)?;
        }
    }
    Ok(())
}

/// A transaction over the committed save folder `dir` (created when missing): its work copy.
fn txn_open(st: &mut State, dir: &Path) -> std::io::Result<usize> {
    if let Some(i) = st.txns.iter().position(|t| t.committed == dir) {
        return Ok(i);
    }
    let new = dir.with_extension("new");
    let old = dir.with_extension("old");
    let work = dir.with_extension("work");
    if !dir.is_dir() && new.is_dir() {
        std::fs::rename(&new, dir)?;
    }
    let _ = std::fs::remove_dir_all(&new);
    let _ = std::fs::remove_dir_all(&old);
    std::fs::create_dir_all(dir)?;
    let _ = std::fs::remove_dir_all(&work);
    copy_tree(dir, &work)?;
    st.txns.push(Txn { committed: dir.to_path_buf(), work });
    Ok(st.txns.len() - 1)
}

fn txn_commit(t: &Txn) -> std::io::Result<()> {
    let new = t.committed.with_extension("new");
    let old = t.committed.with_extension("old");
    let _ = std::fs::remove_dir_all(&new);
    copy_tree(&t.work, &new)?;
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&t.committed, &old)?;
    std::fs::rename(&new, &t.committed)?;
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

fn mount(name: &str, b: Backing) -> u64 {
    with(|st| {
        st.mounts.insert(name.to_string(), b);
    });
    RES_OK
}

fn mount_dir(name: &str, dir: PathBuf, ro: bool, transactional: bool) -> u64 {
    let r = with(|st| -> std::io::Result<Backing> {
        if transactional {
            let i = txn_open(st, &dir)?;
            Ok(Backing::Dir { root: st.txns[i].work.clone(), ro, txn: Some(i) })
        } else {
            std::fs::create_dir_all(&dir)?;
            Ok(Backing::Dir { root: dir.clone(), ro, txn: None })
        }
    });
    match r {
        Ok(b) => {
            crate::log!("fs: {name}: -> {}{}", dir.display(), if transactional { " (transactional save)" } else { "" });
            mount(name, b)
        }
        Err(e) => {
            crate::log!("[RED] fs: mount {name} at {}: {e}", dir.display());
            RES_FAIL
        }
    }
}

/// What a guest path resolves to.
enum Target {
    Rom(Arc<RomFs>, String),
    Host(PathBuf, bool, Option<usize>),
}

fn resolve(path: &str) -> Result<Target, u64> {
    let Some((m, rest)) = split(path) else { return Err(RES_PATH_NOT_FOUND) };
    let b = with(|st| st.mounts.get(&m).cloned());
    match b {
        Some(Backing::Rom(fs)) => Ok(Target::Rom(fs, rest)),
        Some(Backing::Dir { root, ro, txn }) => Ok(Target::Host(if rest.is_empty() { root } else { root.join(&rest) }, ro, txn)),
        None => {
            crate::log_once!(format!("fs nomount {m}"), "[RED] fs: {path}: no mount {m:?}");
            Err(RES_PATH_NOT_FOUND)
        }
    }
}

fn io_res(e: &std::io::Error) -> u64 {
    match e.kind() {
        std::io::ErrorKind::NotFound => RES_PATH_NOT_FOUND,
        std::io::ErrorKind::AlreadyExists => RES_PATH_EXISTS,
        _ => RES_FAIL,
    }
}

fn new_handle(st: &mut State) -> u64 {
    let h = st.next;
    st.next += 1;
    h << 4
}

// ---- the operations (unit-testable without the guest)

/// `NXOS_FS_TRACE=1` (debugging): every file open with its result, `=miss` only the failed ones.
fn fs_trace() -> u8 {
    static T: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *T.get_or_init(|| match std::env::var("NXOS_FS_TRACE").as_deref() {
        Ok("1") => 2,
        Ok("miss") => 1,
        _ => 0,
    })
}

pub fn open_file(path: &str, mode: u32) -> Result<u64, u64> {
    let r = open_file_at(path, mode);
    match (fs_trace(), &r) {
        (2, Ok(_)) => crate::log!("fs trace: open {path}"),
        (1..=2, Err(e)) => crate::log!("fs trace: open {path} -> {e:#x}"),
        _ => {}
    }
    r
}

fn open_file_at(path: &str, mode: u32) -> Result<u64, u64> {
    match resolve(path)? {
        Target::Rom(fs, p) => {
            if mode & (MODE_WRITE | MODE_APPEND) != 0 {
                crate::log!("[RED] fs: write open of {path} on a read-only mount");
                return Err(RES_FAIL);
            }
            if let Some(hp) = is_game(&fs).then(|| crate::mods::romfs_file(&p)).flatten() {
                let f = std::fs::File::open(&hp).map_err(|e| io_res(&e))?;
                crate::log_once!(format!("mod file {p}"), "fs: {path} from a mod ({})", hp.display());
                return Ok(with(|st| {
                    let h = new_handle(st);
                    st.files.insert(h, FileH { open: Open::Host { file: f, txn: None }, mode });
                    h
                }));
            }
            match fs.find(&p) {
                Some(n) if n.is_file() => {
                    let size = n.size();
                    Ok(with(|st| {
                        let h = new_handle(st);
                        st.files.insert(h, FileH { open: Open::Rom { fs: fs.clone(), path: p, size }, mode });
                        h
                    }))
                }
                _ => Err(RES_PATH_NOT_FOUND),
            }
        }
        Target::Host(hp, ro, txn) => {
            let write = mode & (MODE_WRITE | MODE_APPEND) != 0;
            if write && ro {
                return Err(RES_FAIL);
            }
            if hp.is_dir() {
                return Err(RES_PATH_NOT_FOUND);
            }
            let f = std::fs::OpenOptions::new().read(true).write(write).open(&hp).map_err(|e| io_res(&e))?;
            Ok(with(|st| {
                let h = new_handle(st);
                st.files.insert(h, FileH { open: Open::Host { file: f, txn }, mode });
                h
            }))
        }
    }
}

pub fn read_file(h: u64, off: u64, len: u64) -> Result<Vec<u8>, u64> {
    let (fs, p, file) = with(|st| match st.files.get(&h) {
        Some(FileH { open: Open::Rom { fs, path, .. }, .. }) => Ok((Some(fs.clone()), path.clone(), None)),
        Some(FileH { open: Open::Host { file, .. }, .. }) => Ok((None, String::new(), file.try_clone().ok())),
        None => Err(RES_FAIL),
    })?;
    if let Some(fs) = fs {
        return fs.read(&p, off, len).map_err(|e| {
            crate::log!("[RED] fs: read {p} at {off:#x}+{len:#x}: {e}");
            RES_FAIL
        });
    }
    let f = file.ok_or(RES_FAIL)?;
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;
    #[cfg(unix)]
    use crate::services::fs::posio::FileExt;
    let mut buf = vec![0u8; len as usize];
    let mut got = 0usize;
    while got < buf.len() {
        match f.seek_read(&mut buf[got..], off + got as u64) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(io_res(&e)),
        }
    }
    buf.truncate(got);
    Ok(buf)
}

pub fn write_file(h: u64, off: u64, data: &[u8]) -> u64 {
    let f = with(|st| match st.files.get(&h) {
        Some(FileH { open: Open::Host { file, .. }, mode }) if mode & (MODE_WRITE | MODE_APPEND) != 0 => file.try_clone().ok(),
        _ => None,
    });
    let Some(f) = f else { return RES_FAIL };
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;
    #[cfg(unix)]
    use crate::services::fs::posio::FileExt;
    let mut put = 0usize;
    while put < data.len() {
        match f.seek_write(&data[put..], off + put as u64) {
            Ok(0) => return RES_FAIL,
            Ok(n) => put += n,
            Err(e) => return io_res(&e),
        }
    }
    RES_OK
}

pub fn file_size(h: u64) -> Result<u64, u64> {
    with(|st| match st.files.get(&h) {
        Some(FileH { open: Open::Rom { size, .. }, .. }) => Ok(*size),
        Some(FileH { open: Open::Host { file, .. }, .. }) => file.metadata().map(|m| m.len()).map_err(|e| io_res(&e)),
        None => Err(RES_FAIL),
    })
}

pub fn set_file_size(h: u64, n: u64) -> u64 {
    with(|st| match st.files.get(&h) {
        Some(FileH { open: Open::Host { file, .. }, .. }) => file.set_len(n).map_or_else(|e| io_res(&e), |_| RES_OK),
        _ => RES_FAIL,
    })
}

pub fn close_file(h: u64) {
    with(|st| st.files.remove(&h));
}

pub fn create_file(path: &str, size: u64) -> u64 {
    match resolve(path) {
        Err(r) => r,
        Ok(Target::Rom(fs, p)) => {
            if fs.find(&p).is_some() {
                RES_PATH_EXISTS
            } else {
                RES_FAIL
            }
        }
        Ok(Target::Host(hp, ro, _)) => {
            if hp.exists() {
                return RES_PATH_EXISTS;
            }
            if ro {
                return RES_FAIL;
            }
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&hp) {
                Ok(f) => f.set_len(size).map_or_else(|e| io_res(&e), |_| RES_OK),
                Err(e) => io_res(&e),
            }
        }
    }
}

fn host_op(path: &str, op: impl FnOnce(&Path) -> std::io::Result<()>) -> u64 {
    match resolve(path) {
        Err(r) => r,
        Ok(Target::Rom(..)) => RES_FAIL,
        Ok(Target::Host(_, true, _)) => RES_FAIL,
        Ok(Target::Host(hp, false, _)) => op(&hp).map_or_else(|e| io_res(&e), |_| RES_OK),
    }
}

pub fn delete_file(path: &str) -> u64 {
    host_op(path, |p| std::fs::remove_file(p))
}
pub fn create_dir(path: &str) -> u64 {
    match resolve(path) {
        Ok(Target::Host(hp, _, _)) if hp.exists() => RES_PATH_EXISTS,
        _ => host_op(path, |p| std::fs::create_dir(p)),
    }
}
pub fn delete_dir(path: &str) -> u64 {
    host_op(path, |p| std::fs::remove_dir(p))
}
pub fn delete_dir_rec(path: &str) -> u64 {
    host_op(path, |p| std::fs::remove_dir_all(p))
}
pub fn clean_dir_rec(path: &str) -> u64 {
    host_op(path, |p| {
        for e in std::fs::read_dir(p)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                std::fs::remove_dir_all(e.path())?;
            } else {
                std::fs::remove_file(e.path())?;
            }
        }
        Ok(())
    })
}
pub fn rename(from: &str, to: &str) -> u64 {
    match (resolve(from), resolve(to)) {
        (Ok(Target::Host(a, false, _)), Ok(Target::Host(b, false, _))) => {
            if b.exists() {
                return RES_PATH_EXISTS;
            }
            std::fs::rename(a, b).map_or_else(|e| io_res(&e), |_| RES_OK)
        }
        (Err(r), _) | (_, Err(r)) => r,
        _ => RES_FAIL,
    }
}

/// GetEntryType: Some(true) for a directory.
pub fn entry_type(path: &str) -> Result<bool, u64> {
    let r = entry_type_at(path);
    if fs_trace() > 0 {
        if let Err(e) = &r {
            crate::log!("fs trace: entry type {path} -> {e:#x}");
        }
    }
    r
}

fn entry_type_at(path: &str) -> Result<bool, u64> {
    match resolve(path)? {
        Target::Rom(fs, p) => {
            if is_game(&fs) {
                if crate::mods::romfs_file(&p).is_some() {
                    return Ok(false);
                }
                if crate::mods::romfs_has_dir(&p) {
                    return Ok(true);
                }
            }
            fs.find(&p).map(|n| !n.is_file()).ok_or(RES_PATH_NOT_FOUND)
        }
        Target::Host(hp, _, _) => std::fs::metadata(&hp).map(|m| m.is_dir()).map_err(|e| io_res(&e)),
    }
}

pub fn open_dir(path: &str, mode: u32) -> Result<u64, u64> {
    let entries: Vec<(String, i8, u64)> = match resolve(path)? {
        Target::Rom(fs, p) => {
            let layered = if is_game(&fs) { crate::mods::romfs_dir(&p) } else { vec![] };
            let mut v: Vec<(String, i8, u64)> = match fs.list(&p) {
                Ok(l) => l.into_iter().map(|e| (e.name, if e.is_dir { ENT_DIR } else { ENT_FILE }, e.size)).collect(),
                Err(_) if !layered.is_empty() || (is_game(&fs) && crate::mods::romfs_has_dir(&p)) => vec![],
                Err(_) => return Err(RES_PATH_NOT_FOUND),
            };
            for (name, dir, size) in layered {
                let e = (name, if dir { ENT_DIR } else { ENT_FILE }, size);
                match v.iter_mut().find(|x| x.0 == e.0) {
                    Some(x) => *x = e,
                    None => v.push(e),
                }
            }
            v
        }
        Target::Host(hp, _, _) => {
            let mut v = vec![];
            for e in std::fs::read_dir(&hp).map_err(|e| io_res(&e))? {
                let Ok(e) = e else { continue };
                let Ok(md) = e.metadata() else { continue };
                v.push((e.file_name().to_string_lossy().into_owned(), if md.is_dir() { ENT_DIR } else { ENT_FILE }, if md.is_dir() { 0 } else { md.len() }));
            }
            v.sort();
            v
        }
    };
    let entries = entries.into_iter().filter(|e| mode & if e.1 == ENT_DIR { DIRMODE_DIR } else { DIRMODE_FILE } != 0).collect();
    Ok(with(|st| {
        let h = new_handle(st);
        st.dirs.insert(h, DirH { entries, pos: 0, mode });
        h
    }))
}

pub fn read_dir(h: u64, max: usize) -> Result<Vec<(String, i8, u64)>, u64> {
    with(|st| {
        let d = st.dirs.get_mut(&h).ok_or(RES_FAIL)?;
        let _ = d.mode;
        let end = (d.pos + max).min(d.entries.len());
        let v = d.entries[d.pos..end].to_vec();
        d.pos = end;
        Ok(v)
    })
}

pub fn dir_count(h: u64) -> Result<u64, u64> {
    with(|st| st.dirs.get(&h).map(|d| d.entries.len() as u64).ok_or(RES_FAIL))
}

pub fn close_dir(h: u64) {
    with(|st| st.dirs.remove(&h));
}

pub fn commit(name: &str) -> u64 {
    let t = with(|st| match st.mounts.get(name) {
        Some(Backing::Dir { txn: Some(i), .. }) => Some((st.txns[*i].committed.clone(), st.txns[*i].work.clone())),
        _ => None,
    });
    let Some((committed, work)) = t else { return RES_OK };
    match txn_commit(&Txn { committed: committed.clone(), work }) {
        Ok(()) => {
            crate::log_once!(format!("commit {}", committed.display()), "fs: commit {name}: {} (first commit of this save)", committed.display());
            RES_OK
        }
        Err(e) => {
            crate::log!("[RED] fs: commit {name}: {e} (the committed save is unchanged)");
            RES_FAIL
        }
    }
}

/// A Commit of several mounts: they must exist and be writable, and the transactional ones must be one save.
pub fn commit_list(names: &[String]) -> u64 {
    let mut target: Option<(usize, String)> = None;
    for n in names {
        let b = with(|st| st.mounts.get(n).cloned());
        match b {
            Some(Backing::Dir { ro: false, txn, .. }) => {
                if let Some(i) = txn {
                    match &target {
                        Some((t, _)) if *t != i => {
                            crate::log!("[RED] fs: a Commit mount list spans independent saves; refused without a partial commit");
                            return RES_FAIL;
                        }
                        Some(_) => {}
                        None => target = Some((i, n.clone())),
                    }
                }
            }
            _ => return RES_FAIL,
        }
    }
    match target {
        Some((_, n)) => commit(&n),
        None => RES_OK,
    }
}

// ---- cache storages

fn cache_dir(index: i32) -> PathBuf {
    save_dir().join(format!("cache{index}"))
}
fn cache_size_path(index: i32) -> PathBuf {
    save_dir().join(format!("cache{index}.size"))
}

pub fn create_cache(index: i32, size: i64, journal: i64) -> u64 {
    let d = cache_dir(index);
    crate::log!("fs: create cache storage {index} ({size} bytes, journal {journal}) -> {}", d.display());
    if let Err(e) = std::fs::create_dir_all(&d) {
        crate::log!("[RED] fs: create cache storage {index}: {e}");
        return RES_FAIL;
    }
    let rec = cache_size_path(index);
    if !rec.exists() {
        let tmp = rec.with_extension("size.tmp");
        if std::fs::write(&tmp, format!("{size} {journal}\n")).and_then(|_| std::fs::rename(&tmp, &rec)).is_err() {
            crate::log!("[RED] fs: cache storage {index}: cannot record its size in {}", rec.display());
            let _ = std::fs::remove_file(&tmp);
        }
    }
    RES_OK
}

pub fn delete_cache(index: i32) -> u64 {
    let _ = std::fs::remove_dir_all(cache_dir(index));
    let _ = std::fs::remove_file(cache_size_path(index));
    RES_OK
}

/// `len` bytes of the application's control data at `off`.
fn control(off: u64, len: usize) -> Option<Vec<u8>> {
    let b = std::fs::read(data_dir().join("control.nacp")).ok()?;
    b.get(off as usize..off as usize + len).map(|s| s.to_vec())
}

/// (data size, journal size) of cache storage `index`; None when it does not exist.
pub fn cache_size(index: i32) -> Option<(i64, i64)> {
    if !cache_dir(index).is_dir() {
        return None;
    }
    let rec = std::fs::read_to_string(cache_size_path(index)).ok().and_then(|t| {
        let mut w = t.split_whitespace().map(|x| x.parse::<i64>());
        Some((w.next()?.ok()?, w.next()?.ok()?))
    });
    Some(rec.unwrap_or_else(|| {
        let c = control(0x3170, 16).map(|b| (i64::from_le_bytes(b[0..8].try_into().unwrap()), i64::from_le_bytes(b[8..16].try_into().unwrap())));
        match c {
            Some((s, j)) if s != 0 || j != 0 => (s, j),
            _ => (256 << 20, 256 << 20),
        }
    }))
}

/// The cache storages that exist (folders `cache<n>`, canonical decimal, 0..65535), in index order.
pub fn cache_indices() -> Vec<u16> {
    let mut v: Vec<u16> = std::fs::read_dir(save_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    let d = n.strip_prefix("cache")?;
                    let ok = !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) && (d == "0" || !d.starts_with('0'));
                    if ok {
                        d.parse::<u16>().ok()
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable();
    v
}

/// Open cache storage lists: (handle, indices, next).
static CACHE_LISTS: Mutex<Vec<(u64, Vec<u16>, usize)>> = Mutex::new(Vec::new());
static CACHE_LIST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A RomFS image file, read in place.
struct ImageFile {
    file: File,
    size: u64,
}

impl translator::package::storage::Storage for ImageFile {
    fn size(&self) -> u64 {
        self.size
    }
    fn read(&self, offset: u64, size: u64) -> std::result::Result<Vec<u8>, translator::package::PackageError> {
        #[cfg(windows)]
        use std::os::windows::fs::FileExt;
        #[cfg(unix)]
        use crate::services::fs::posio::FileExt;
        if offset.checked_add(size).is_none_or(|e| e > self.size) {
            return Err(translator::package::PackageError::Format("read past the end of the RomFS image".into()));
        }
        let mut buf = vec![0u8; size as usize];
        let mut done = 0;
        while done < buf.len() {
            let n = self.file.seek_read(&mut buf[done..], offset + done as u64).map_err(|e| translator::package::PackageError::Format(e.to_string()))?;
            if n == 0 {
                return Err(translator::package::PackageError::Format("the RomFS image is cut short".into()));
            }
            done += n;
        }
        Ok(buf)
    }
}

/// The data folder of program `index` of the same application, beside this program's: `<name>_data` is program 0,
/// `<name>_p<N>_data` program N (what `nxrs-build` writes for every program of a package).
pub fn program_data_dir(data: &Path, index: u32) -> Option<PathBuf> {
    let file = data.file_name()?.to_str()?;
    let stem = file.strip_suffix("_data")?;
    let base = match stem.rsplit_once("_p") {
        Some((b, n)) if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) => b,
        _ => stem,
    };
    let name = if index == 0 { format!("{base}_data") } else { format!("{base}_p{index}_data") };
    Some(data.with_file_name(name))
}

/// Another program's RomFS: `romfs_programs/<index>.img` beside the game's data, else that program's own build
/// beside this one (`program_data_dir`, its RomFS marker read as the game's is).
fn program_romfs(index: i32) -> Option<Arc<RomFs>> {
    let img = data_dir().join("romfs_programs").join(format!("{index}.img"));
    if !img.is_file() {
        let dir = program_data_dir(&data_dir(), u32::try_from(index).ok()?).filter(|d| d.is_dir() && *d != data_dir())?;
        return match translator::package::open_data_dir(&dir, None) {
            Ok(r) => {
                crate::log!("fs: program {index}'s RomFS from its build ({})", r.source);
                Some(Arc::new(r))
            }
            Err(e) => {
                crate::log!("[RED] fs: program {index}'s RomFS ({}): {e}", dir.display());
                None
            }
        };
    }
    let file = File::open(&img).ok()?;
    let size = file.metadata().ok()?.len();
    let st = Arc::new(ImageFile { file, size });
    match RomFs::from_image(st, img.display().to_string()) {
        Ok(r) => Some(Arc::new(r)),
        Err(e) => {
            crate::log!("[RED] fs: {}: {e}", img.display());
            None
        }
    }
}

// ---- the RomFS working-memory size (Query...CacheSize)

fn romfs_buckets(n: usize) -> usize {
    if n < 3 {
        return 3;
    }
    if n < 19 {
        return n | 1;
    }
    let mut n = n;
    while !(!n.is_multiple_of(2) && !n.is_multiple_of(3) && !n.is_multiple_of(5) && !n.is_multiple_of(7) && !n.is_multiple_of(11) && !n.is_multiple_of(13) && !n.is_multiple_of(17)) {
        n += 1;
    }
    n
}

/// RomFsFileSystem::GetRequiredWorkingMemorySize of a tree: both hash tables (4 bytes a bucket) and every entry
/// (0x18 / 0x20 bytes plus the name, padded to 4).
pub fn romfs_work_size(fs: &RomFs) -> u64 {
    let (mut ndir, mut dbytes, mut nfile, mut fbytes) = (1usize, 0x18usize, 0usize, 0usize);
    fn walk(n: &translator::package::romfs::Node, nd: &mut usize, db: &mut usize, nf: &mut usize, fb: &mut usize) {
        for k in n.children() {
            let name = (k.name.len() + 3) & !3;
            if k.is_file() {
                *nf += 1;
                *fb += 0x20 + name;
            } else {
                *nd += 1;
                *db += 0x18 + name;
                walk(k, nd, db, nf, fb);
            }
        }
    }
    walk(fs.root(), &mut ndir, &mut dbytes, &mut nfile, &mut fbytes);
    (4 * romfs_buckets(ndir) + dbytes + 4 * romfs_buckets(nfile) + fbytes) as u64
}

// ---- add-on content

fn aoc_romfs(index: u32) -> Option<Arc<RomFs>> {
    static CACHE: Mutex<Vec<(u32, Arc<RomFs>)>> = Mutex::new(Vec::new());
    if let Some((_, r)) = CACHE.lock().unwrap().iter().find(|(i, _)| *i == index) {
        return Some(r.clone());
    }
    let base = crate::profile::get().program_id + 0x1000;
    let id = base + index as u64;
    match translator::package::dlc::title_romfs(&data_dir(), id, None) {
        Ok(r) => {
            let r = Arc::new(r);
            crate::log!("fs: add-on content {index} ({id:016X}) from {}", r.source);
            CACHE.lock().unwrap().push((index, r.clone()));
            Some(r)
        }
        Err(e) => {
            crate::log_once!(format!("aoc {index}"), "fs: add-on content {index} ({id:016X}) is not installed: {e}");
            None
        }
    }
}

/// The add-on content indices installed in the data folder whose data the host can read (Turbo 2cdeabd): a title in
/// the manifest whose package was moved or whose image was deleted is left out with a warning (a game told about DLC
/// it then cannot read can hang: Mario Kart 8 Deluxe on its loading screen); a `stub` line is listed regardless (an
/// empty RomFS, for diagnostics). Decided once, at the first question.
pub fn aoc_indices() -> Vec<u32> {
    static V: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let base = crate::profile::get().program_id + 0x1000;
        aoc_offered(base, &translator::package::dlc::read_manifest(&data_dir()), |i| aoc_romfs(i).is_some())
    })
    .clone()
}

/// The indices of the manifest's titles owned by the game (`base` = its program id + 0x1000) to offer: stubs, and
/// titles `readable` says have data.
fn aoc_offered(base: u64, manifest: &[translator::package::dlc::Entry], readable: impl Fn(u32) -> bool) -> Vec<u32> {
    let mut v: Vec<u32> = vec![];
    for e in manifest {
        let id = e.0;
        if id <= base || id >= base + 0x1000 {
            continue;
        }
        let index = (id - base) as u32;
        if v.contains(&index) {
            continue;
        }
        if e.2 == "stub" || readable(index) {
            v.push(index);
        } else {
            crate::log!("[RED] dlc: {id:016X} is installed but its data is missing ({}): not offered to the game. Install it again", e.3);
        }
    }
    v.sort();
    v
}

// ---- the takeovers

unsafe fn ret(c: *mut Cpu, v: u64) {
    (*c).x[0] = v;
}

macro_rules! fsfn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            ret(c, r);
        }
    };
}

fsfn!(query_rom_cache, |a| {
    crate::guest::out(a.x(0), 0x1000u64);
    RES_OK
});
fsfn!(mount_rom, |a| {
    let name = cstr(a.x(0), 32);
    match romfs() {
        Some(r) => {
            crate::log!("fs: {name}: -> the game's RomFS");
            mount(&name, Backing::Rom(r))
        }
        None => RES_FAIL,
    }
});
// MountRom(name, program index, cache, size): another program of the application by its index (its RomFS image),
// else this program's own.
fsfn!(mount_rom_index, |a| {
    let (name, idx) = (cstr(a.x(0), 32), a.i(1));
    if let Some(r) = program_romfs(idx) {
        crate::log!("fs: {name}: -> the RomFS of program {idx}");
        return mount(&name, Backing::Rom(r));
    }
    if idx != 0 {
        crate::log!("[RED] fs: MountRom({name}, program index {idx}): no romfs_programs/{idx}.img beside the game's data: this program's own");
    }
    match romfs() {
        Some(r) => {
            crate::log!("fs: {name}: -> the game's RomFS");
            mount(&name, Backing::Rom(r))
        }
        None => RES_FAIL,
    }
});
// IsPathMounted(path): whether the mount before ':' is one of the host's.
fsfn!(is_path_mounted, |a| {
    let p = cstr(a.x(0), 0x301);
    match p.split_once(':') {
        Some((m, _)) => with(|st| st.mounts.contains_key(m)) as u64,
        None => 0,
    }
});
// Commit(const char *const *names, int count)
fsfn!(commit_list_fn, |a| {
    let (names, n) = (a.x(0), a.i(1));
    if !(0..=64).contains(&n) || (n > 0 && names == 0) {
        return RES_FAIL;
    }
    let v: Vec<String> = (0..n as u64).map(|i| cstr(crate::guest::r64(names + 8 * i), 32)).collect();
    commit_list(&v)
});
fsfn!(create_cache_fn, |a| { create_cache(a.i(0), a.x(1) as i64, a.x(2) as i64) });
fsfn!(delete_cache_fn, |a| { delete_cache(a.i(0)) });
// GetCacheStorageMax(int *index, i64 *size): the control data's limits.
fsfn!(cache_max, |a| {
    let Some(b) = control(0x3180, 10) else {
        crate::log!("[RED] fs: GetCacheStorageMax: no control data ({})", data_dir().join("control.nacp").display());
        return RES_TARGET_NOT_FOUND;
    };
    let size = i64::from_le_bytes(b[0..8].try_into().unwrap());
    let index = u16::from_le_bytes([b[8], b[9]]) as u32;
    crate::guest::out(a.x(0), index);
    crate::guest::out(a.x(1), size);
    crate::log!("fs: cache storage max: index {index}, {size} bytes of data and journal");
    RES_OK
});
// GetCacheStorageSize(i64 *size, i64 *journal, int index)
fsfn!(cache_size_fn, |a| {
    match cache_size(a.i(2)) {
        Some((s, j)) => {
            crate::guest::out(a.x(0), s);
            crate::guest::out(a.x(1), j);
            RES_OK
        }
        None => RES_TARGET_NOT_FOUND,
    }
});
// OpenCacheStorageList(CacheStorageListHandle *): a snapshot of the cache storages that exist.
fsfn!(cache_list_open, |a| {
    let v = cache_indices();
    let id = CACHE_LIST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    crate::log!("fs: cache storage list: {}", v.len());
    CACHE_LISTS.lock().unwrap().push((id, v, 0));
    w64(a.x(0), id);
    RES_OK
});
// ReadCacheStorageList(int *n, CacheStorageInfo *info, handle, int max): 0x20-byte entries, the index first.
fsfn!(cache_list_read, |a| {
    let (n, info, h, max) = (a.x(0), a.x(1), a.x(2), a.i(3));
    let mut g = CACHE_LISTS.lock().unwrap();
    w32(n, 0);
    let Some(l) = g.iter_mut().find(|l| l.0 == h) else {
        crate::log!("[RED] fs: ReadCacheStorageList: handle {h:#x} is no open list");
        return RES_FAIL;
    };
    let mut k = 0u64;
    while l.2 < l.1.len() && (k as i32) < max {
        let e = info + 0x20 * k;
        crate::guest::zero(e, 0x20);
        w32(e, l.1[l.2] as u32);
        l.2 += 1;
        k += 1;
    }
    w32(n, k as u32);
    RES_OK
});
fsfn!(cache_list_close, |a| {
    CACHE_LISTS.lock().unwrap().retain(|l| l.0 != a.x(0));
    0
});
fsfn!(query_aoc_cache, |a| {
    let n = aoc_romfs(a.w(1)).map_or(0x1000, |r| romfs_work_size(&r));
    crate::guest::out(a.x(0), n);
    RES_OK
});
fsfn!(mount_aoc, |a| {
    let (name, idx) = (cstr(a.x(0), 32), a.w(1));
    match aoc_romfs(idx) {
        Some(r) => mount(&name, Backing::Rom(r)),
        None => RES_TARGET_NOT_FOUND,
    }
});
fsfn!(query_sysdata_cache, |a| {
    crate::guest::out(a.x(0), 0x1000u64);
    RES_OK
});
fsfn!(mount_sysdata, |a| {
    let (name, id) = (cstr(a.x(0), 32), a.x(1));
    let own = save_dir().join("system").join(format!("{id:016x}"));
    if own.is_dir() {
        return_mount(&name, own)
    } else if let Some(dir) = crate::services::mii::system_data(id) {
        return_mount(&name, dir)
    } else {
        crate::log::gap(crate::log::Gap::Other, &format!("system data {id:016x}"), &format!("{name}: the host has none (a console's own goes in {})", own.display()));
        RES_TARGET_NOT_FOUND
    }
});
fn return_mount(name: &str, dir: PathBuf) -> u64 {
    mount_dir(name, dir, true, false)
}
fsfn!(mount_temp, |a| {
    let name = cstr(a.x(0), 32);
    mount_dir(&name, save_dir().join("temp"), false, false)
});
fsfn!(mount_cache0, |a| {
    let name = cstr(a.x(0), 32);
    mount_dir(&name, save_dir().join("cache0"), false, false)
});
fsfn!(mount_cache, |a| {
    let name = cstr(a.x(0), 32);
    mount_dir(&name, save_dir().join(format!("cache{}", a.w(1))), false, false)
});
fsfn!(mount_save, |a| {
    let name = cstr(a.x(0), 32);
    let Some(user) = crate::services::account::user_of(a.x(1)) else { return RES_TARGET_NOT_FOUND };
    let dir = save_dir().join(format!("user{}", user + 1));
    if user == 0 {
        adopt_turbo_save(&save_dir(), &dir);
    }
    mount_dir(&name, dir, false, true)
});

/// A Turbo build of the same game keeps the account save in `save\user` (older ones in `save\system`); an nxrs build
/// keeps one per user (`save\user<N>`). The first time user 1's save opens and there is none, Turbo's is copied in
/// (left where it was), so a player moving from a Turbo build keeps their progress (as Turbo svc_fs.c copies its own
/// older `save\system`).
fn adopt_turbo_save(root: &Path, dir: &Path) {
    if dir.exists() {
        return;
    }
    for old in ["user", "system"] {
        let from = root.join(old);
        if from.is_dir() && std::fs::read_dir(&from).is_ok_and(|mut d| d.next().is_some()) {
            match copy_tree(&from, dir) {
                Ok(()) => crate::log!("fs: {} is a Turbo build's save: copied to {} (the original is kept)", from.display(), dir.display()),
                Err(e) => crate::log!("[RED] fs: copying the Turbo save {} to {}: {e}", from.display(), dir.display()),
            }
            return;
        }
    }
}
fsfn!(mount_save_ro, |a| {
    let (name, app) = (cstr(a.x(0), 32), a.x(1));
    if app != crate::profile::get().program_id {
        crate::log!("[RED] fs: read-only mount {name} of another program's save data ({app:016x}): none");
        return RES_TARGET_NOT_FOUND
    }
    let Some(user) = crate::services::account::user_of(a.x(2)) else { return RES_TARGET_NOT_FOUND };
    mount_dir(&name, save_dir().join(format!("user{}", user + 1)), true, false)
});
fsfn!(mount_device_save, |a| {
    let name = cstr(a.x(0), 32);
    mount_dir(&name, save_dir().join("device"), false, true)
});
fsfn!(save_size, |a| {
    crate::guest::out(a.x(0), 256u64 << 20);
    crate::guest::out(a.x(1), 256u64 << 20);
    RES_OK
});
fsfn!(ok_fn, |a| { RES_OK });
fsfn!(save_existing_app, |a| { (a.x(0) == crate::profile::get().program_id && crate::services::account::user_of(a.x(1)).is_some()) as u64 });
fsfn!(save_existing, |a| { 1 });
fsfn!(free_space, |a| {
    crate::guest::out(a.x(0), 256u64 << 20);
    RES_OK
});
fsfn!(commit_fn, |a| { commit(&cstr(a.x(0), 32)) });
fsfn!(is_mounted, |a| {
    let n = cstr(a.x(0), 32);
    with(|st| st.mounts.contains_key(&n)) as u64
});
fsfn!(unmount, |a| {
    let n = cstr(a.x(0), 32);
    with(|st| {
        if matches!(st.mounts.get(&n), Some(Backing::Rom(_))) && n != "rom" {
            st.mounts.remove(&n);
        }
    });
    0
});
fsfn!(open_file_fn, |a| {
    let (out, path, mode) = (a.x(0), cstr(a.x(1), 0x301), a.w(2));
    match open_file(&path, mode) {
        Ok(h) => {
            if crate::trace::on("fs") {
                crate::log!("fs open {path} mode {mode} -> {h:#x}");
            }
            w64(out, h);
            RES_OK
        }
        Err(r) => {
            if crate::trace::on("fs") {
                crate::log!("fs open {path} mode {mode} -> {r:#x}");
            }
            r
        }
    }
});
unsafe fn read_into(h: u64, off: u64, buf: u64, n: u64) -> Result<u64, u64> {
    let t = std::time::Instant::now();
    let data = read_file(h, off, n)?;
    if crate::trace::on("fsread") {
        crate::log!("fs read {h:#x} at {off:#x} {n:#x} bytes: {:#x} in {:.2} ms", data.len(), t.elapsed().as_secs_f64() * 1e3);
    }
    crate::guest::write_bytes(buf, &data);
    Ok(data.len() as u64)
}
fsfn!(read_file_out, |a| {
    let (out, h, off, buf, n) = (a.x(0), a.x(1), a.x(2), a.x(3), a.x(4));
    match read_into(h, off, buf, n) {
        Ok(got) => {
            crate::guest::out(out, got);
            RES_OK
        }
        Err(r) => r,
    }
});
fsfn!(read_file_exact, |a| {
    let (h, off, buf, n) = (a.x(0), a.x(1), a.x(2), a.x(3));
    match read_into(h, off, buf, n) {
        Ok(got) if got == n => RES_OK,
        Ok(got) => {
            crate::log!("[RED] fs: short read of handle {h:#x} at {off:#x}: wanted {n:#x}, got {got:#x}");
            RES_OUT_OF_RANGE
        }
        Err(r) => r,
    }
});
fsfn!(write_file_fn, |a| {
    let (h, off, buf, n) = (a.x(0), a.x(1), a.x(2), a.x(3));
    write_file(h, off, crate::guest::bytes(buf, n))
});
fsfn!(get_file_size, |a| {
    match file_size(a.x(1)) {
        Ok(n) => {
            w64(a.x(0), n);
            RES_OK
        }
        Err(r) => r,
    }
});
fsfn!(set_file_size_fn, |a| { set_file_size(a.x(0), a.x(1)) });
fsfn!(flush_file, |a| { RES_OK });
fsfn!(close_file_fn, |a| {
    close_file(a.x(0));
    0
});
fsfn!(file_open_mode, |a| { with(|st| st.files.get(&a.x(0)).map_or(0, |f| f.mode as u64)) });
fsfn!(create_file_fn, |a| { create_file(&cstr(a.x(0), 0x301), a.x(1)) });
fsfn!(delete_file_fn, |a| { delete_file(&cstr(a.x(0), 0x301)) });
fsfn!(create_dir_fn, |a| { create_dir(&cstr(a.x(0), 0x301)) });
fsfn!(delete_dir_fn, |a| { delete_dir(&cstr(a.x(0), 0x301)) });
fsfn!(delete_dir_rec_fn, |a| { delete_dir_rec(&cstr(a.x(0), 0x301)) });
fsfn!(clean_dir_rec_fn, |a| { clean_dir_rec(&cstr(a.x(0), 0x301)) });
fsfn!(rename_fn, |a| { rename(&cstr(a.x(0), 0x301), &cstr(a.x(1), 0x301)) });
fsfn!(entry_type_fn, |a| {
    let (out, path) = (a.x(0), cstr(a.x(1), 0x301));
    match entry_type(&path) {
        Ok(d) => {
            w32(out, if d { ENT_DIR as u32 } else { ENT_FILE as u32 });
            RES_OK
        }
        Err(r) => r,
    }
});
fsfn!(open_dir_fn, |a| {
    let (out, path, mode) = (a.x(0), cstr(a.x(1), 0x301), a.w(2));
    match open_dir(&path, mode) {
        Ok(h) => {
            w64(out, h);
            RES_OK
        }
        Err(r) => r,
    }
});
fsfn!(read_dir_fn, |a| {
    let (count, ents, h, max) = (a.x(0), a.x(1), a.x(2), a.x(3) as i64);
    match read_dir(h, max.max(0) as usize) {
        Ok(v) => {
            for (i, (name, ty, size)) in v.iter().enumerate() {
                let e = ents + i as u64 * DIRENT_SIZE;
                crate::guest::zero(e, DIRENT_SIZE);
                crate::guest::put_cstr(e, 0x301, name);
                w8(e + 0x304, *ty as u8);
                w64(e + 0x308, *size);
            }
            w64(count, v.len() as u64);
            RES_OK
        }
        Err(r) => {
            w64(count, 0);
            r
        }
    }
});
fsfn!(dir_count_fn, |a| {
    match dir_count(a.x(1)) {
        Ok(n) => {
            w64(a.x(0), n);
            RES_OK
        }
        Err(r) => r,
    }
});
fsfn!(close_dir_fn, |a| {
    close_dir(a.x(0));
    0
});
fsfn!(zero_fn, |a| { 0 });
fsfn!(path_len_max, |a| {
    crate::guest::out(a.x(0), 0x300u64);
    RES_OK
});
fsfn!(utf16_len_max, |a| {
    crate::guest::out(a.x(0), 0u8);
    crate::guest::out(a.x(1), 0u64);
    RES_OK
});
fsfn!(restore_flag, |a| {
    crate::guest::out(a.x(0), 0u8);
    RES_OK
});

pub fn register(r: &mut Registry) {
    const W: &str = "fsp-srv: the host's file system";
    for (n, f) in [
        ("_ZN2nn2fs22QueryMountRomCacheSizeEPm", query_rom_cache as unsafe fn(*mut Cpu)),
        ("_ZN2nn2fs8MountRomEPKcPvm", mount_rom),
        ("_ZN2nn2fs8MountRomEPKc", mount_rom),
        ("_ZN2nn2fs22QueryMountRomCacheSizeEPmi", query_rom_cache),
        ("_ZN2nn2fs8MountRomEPKciPvm", mount_rom_index),
        ("_ZN2nn2fs13IsPathMountedEPKc", is_path_mounted),
        ("_ZN2nn2fs6CommitEPKPKci", commit_list_fn),
        ("_ZN2nn2fs18CreateCacheStorageEill", create_cache_fn),
        ("_ZN2nn2fs18DeleteCacheStorageEi", delete_cache_fn),
        ("_ZN2nn2fs18GetCacheStorageMaxEPiPl", cache_max),
        ("_ZN2nn2fs19GetCacheStorageSizeEPlS1_i", cache_size_fn),
        ("_ZN2nn2fs20OpenCacheStorageListEPNS0_22CacheStorageListHandleE", cache_list_open),
        ("_ZN2nn2fs20ReadCacheStorageListEPiPNS0_16CacheStorageInfoENS0_22CacheStorageListHandleEi", cache_list_read),
        ("_ZN2nn2fs21CloseCacheStorageListENS0_22CacheStorageListHandleE", cache_list_close),
        ("_ZN2nn2fs31QueryMountAddOnContentCacheSizeEPmi", query_aoc_cache),
        ("_ZN2nn2fs17MountAddOnContentEPKciPvm", mount_aoc),
        ("_ZN2nn2fs29QueryMountSystemDataCacheSizeEPmNS_3ncm12SystemDataIdE", query_sysdata_cache),
        ("_ZN2nn2fs15MountSystemDataEPKcNS_3ncm12SystemDataIdEPvm", mount_sysdata),
        ("_ZN2nn2fs21MountTemporaryStorageEPKc", mount_temp),
        ("_ZN2nn2fs17MountCacheStorageEPKc", mount_cache0),
        ("_ZN2nn2fs17MountCacheStorageEPKci", mount_cache),
        ("_ZN2nn2fs13MountSaveDataEPKcRKNS_7account3UidE", mount_save),
        ("_ZN2nn2fs21MountSaveDataReadOnlyEPKcNS_13ApplicationIdERKNS_7account3UidE", mount_save_ro),
        ("_ZN2nn2fs19MountDeviceSaveDataEPKc", mount_device_save),
        ("_ZN2nn2fs15GetSaveDataSizeEPlS1_RKNS_7account3UidE", save_size),
        ("_ZN2nn2fs18GetSaveDataSizeMaxEPlS1_", save_size),
        ("_ZN2nn2fs21GetDeviceSaveDataSizeEPlS1_", save_size),
        ("_ZN2nn2fs14ExtendSaveDataERKNS_7account3UidEll", ok_fn),
        ("_ZN2nn2fs20ExtendDeviceSaveDataEll", ok_fn),
        ("_ZN2nn2fs14EnsureSaveDataERKNS_7account3UidE", ok_fn),
        ("_ZN2nn2fs18IsSaveDataExistingENS_13ApplicationIdERKNS_7account3UidE", save_existing_app),
        ("_ZN2nn2fs18IsSaveDataExistingERKNS_7account3UidE", save_existing),
        ("_ZN2nn2fs16GetFreeSpaceSizeEPlPKc", free_space),
        ("_ZN2nn2fs22GetSaveDataRestoreFlagEPbPKc", restore_flag),
        ("_ZN2nn2fs6CommitEPKc", commit_fn),
        ("_ZN2nn2fs14CommitSaveDataEPKc", commit_fn),
        ("_ZN2nn2fs6CommitEPKcRKNS0_12CommitOptionE", commit_fn),
        ("_ZN2nn2fs9IsMountedEPKc", is_mounted),
        ("_ZN2nn2fs7UnmountEPKc", unmount),
        ("_ZN2nn2fs8OpenFileEPNS0_10FileHandleEPKci", open_file_fn),
        ("_ZN2nn2fs8ReadFileEPmNS0_10FileHandleElPvm", read_file_out),
        ("_ZN2nn2fs8ReadFileEPmNS0_10FileHandleElPvmRKNS0_10ReadOptionE", read_file_out),
        ("_ZN2nn2fs8ReadFileENS0_10FileHandleElPvm", read_file_exact),
        ("_ZN2nn2fs8ReadFileENS0_10FileHandleElPvmRKNS0_10ReadOptionE", read_file_exact),
        ("_ZN2nn2fs9WriteFileENS0_10FileHandleElPKvmRKNS0_11WriteOptionE", write_file_fn),
        ("_ZN2nn2fs11GetFileSizeEPlNS0_10FileHandleE", get_file_size),
        ("_ZN2nn2fs11SetFileSizeENS0_10FileHandleEl", set_file_size_fn),
        ("_ZN2nn2fs9FlushFileENS0_10FileHandleE", flush_file),
        ("_ZN2nn2fs9CloseFileENS0_10FileHandleE", close_file_fn),
        ("_ZN2nn2fs15GetFileOpenModeENS0_10FileHandleE", file_open_mode),
        ("_ZN2nn2fs10CreateFileEPKcl", create_file_fn),
        ("_ZN2nn2fs10DeleteFileEPKc", delete_file_fn),
        ("_ZN2nn2fs15CreateDirectoryEPKc", create_dir_fn),
        ("_ZN2nn2fs15DeleteDirectoryEPKc", delete_dir_fn),
        ("_ZN2nn2fs26DeleteDirectoryRecursivelyEPKc", delete_dir_rec_fn),
        ("_ZN2nn2fs25CleanDirectoryRecursivelyEPKc", clean_dir_rec_fn),
        ("_ZN2nn2fs10RenameFileEPKcS2_", rename_fn),
        ("_ZN2nn2fs15RenameDirectoryEPKcS2_", rename_fn),
        ("_ZN2nn2fs12GetEntryTypeEPNS0_18DirectoryEntryTypeEPKc", entry_type_fn),
        ("_ZN2nn2fs13OpenDirectoryEPNS0_15DirectoryHandleEPKci", open_dir_fn),
        ("_ZN2nn2fs13ReadDirectoryEPlPNS0_14DirectoryEntryENS0_15DirectoryHandleEl", read_dir_fn),
        ("_ZN2nn2fs22GetDirectoryEntryCountEPlNS0_15DirectoryHandleE", dir_count_fn),
        ("_ZN2nn2fs14CloseDirectoryENS0_15DirectoryHandleE", close_dir_fn),
        ("_ZN2nn2fs25EnableGlobalFileDataCacheEPvm", zero_fn),
        ("_ZN2nn2fs26DisableGlobalFileDataCacheEv", zero_fn),
        ("_ZN2nn2fs29EnableIndividualFileDataCacheEPKcPvm", ok_fn),
        ("_ZN2nn2fs30DisableIndividualFileDataCacheEPKc", zero_fn),
        ("_ZN2nn2fs26SetPriorityOnCurrentThreadENS0_8PriorityE", zero_fn),
        ("_ZN2nn2fs6detail18IsEnabledAccessLogEj", zero_fn),
        ("_ZN2nn2fs6detail18IsEnabledAccessLogEv", zero_fn),
        ("_ZN2nn2fs16GetPathLengthMaxEPmPKc", path_len_max),
        ("_ZN2nn2fs21GetEntryNameLengthMaxEPmPKc", path_len_max),
        ("_ZN2nn2fs21GetUtf16PathLengthMaxEPbPmPKc", utf16_len_max),
        ("_ZN2nn2fs26GetUtf16EntryNameLengthMaxEPbPmPKc", utf16_len_max),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// MountRom(name, N) of a multi-program application finds program N's build beside this one.
    #[test]
    fn other_programs_data_folders() {
        let d = Path::new(r"D:\b\Sonic_data");
        assert_eq!(program_data_dir(d, 1).unwrap(), Path::new(r"D:\b\Sonic_p1_data"));
        assert_eq!(program_data_dir(d, 0).unwrap(), d);
        let p1 = Path::new(r"D:\b\Sonic_p1_data");
        assert_eq!(program_data_dir(p1, 0).unwrap(), d);
        assert_eq!(program_data_dir(p1, 2).unwrap(), Path::new(r"D:\b\Sonic_p2_data"));
        assert_eq!(program_data_dir(Path::new(r"D:\b\Top_pad_data"), 1).unwrap(), Path::new(r"D:\b\Top_pad_p1_data"));
        assert!(program_data_dir(Path::new(r"D:\b\other"), 1).is_none());
    }

    /// A Turbo build's `save\user` (or older `save\system`) becomes user 1's save the first time, once.
    #[test]
    fn turbo_save_is_adopted() {
        let root = std::env::temp_dir().join(format!("nxos-turbo-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("user/sub")).unwrap();
        std::fs::write(root.join("user/sub/main"), b"turbo").unwrap();
        let dir = root.join("user1");
        adopt_turbo_save(&root, &dir);
        assert_eq!(std::fs::read(dir.join("sub/main")).unwrap(), b"turbo");
        assert!(root.join("user/sub/main").exists(), "the original stays");
        std::fs::write(root.join("user/sub/main"), b"newer").unwrap();
        adopt_turbo_save(&root, &dir);
        assert_eq!(std::fs::read(dir.join("sub/main")).unwrap(), b"turbo", "only when user 1 has no save yet");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Turbo 2cdeabd: a manifest title whose data is gone is not offered; a stub line is; other titles' DLC and
    /// duplicates are left out.
    #[test]
    fn dlc_with_missing_data_is_not_offered() {
        let base = 0x0100_1520_0002_2000u64 + 0x1000;
        let m: Vec<translator::package::dlc::Entry> = vec![
            (base + 1, 1, "data".into(), "P:/gone.nsp".into()),
            (base + 2, 1, "data".into(), "P:/here.nsp".into()),
            (base + 3, 1, "stub".into(), String::new()),
            (base + 2, 1, "data".into(), "P:/here.nsp".into()),
            (0x0100_0000_0001_1001, 1, "data".into(), "P:/other.nsp".into()),
        ];
        assert_eq!(aoc_offered(base, &m, |i| i == 2), vec![2, 3]);
    }

    #[test]
    fn paths() {
        assert_eq!(split("rom:/a/./b/../c"), Some(("rom".into(), "a/c".into())));
        assert_eq!(split("save:\\x\\y"), Some(("save".into(), "x/y".into())));
        assert_eq!(split("nomount"), None);
        assert_eq!(romfs_buckets(2), 3);
        assert_eq!(romfs_buckets(4), 5);
        assert_eq!(romfs_buckets(19), 19);
        assert_eq!(romfs_buckets(20), 23);
    }

    /// A synthetic package's RomFS image (header, directory and file tables, data), read in place through the
    /// translator's image backend, as an NSP's RomFS is.
    struct Image(Vec<u8>);
    impl translator::package::storage::Storage for Image {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&self, offset: u64, size: u64) -> std::result::Result<Vec<u8>, translator::package::PackageError> {
            Ok(self.0[offset as usize..(offset + size) as usize].to_vec())
        }
    }

    /// root/{a.txt, d/{b.bin, c.txt}}
    fn image() -> Vec<u8> {
        const E: u32 = 0xFFFF_FFFF;
        let entry = |fixed: &[u32], name: &str| {
            let mut v: Vec<u8> = fixed.iter().flat_map(|w| w.to_le_bytes()).collect();
            v.extend((name.len() as u32).to_le_bytes());
            v.extend(name.as_bytes());
            while v.len() % 4 != 0 {
                v.push(0);
            }
            v
        };
        // directories: root at 0 (child d, first file a.txt), d at 0x18 (files b.bin, c.txt)
        let mut dirs = entry(&[0, E, 0x18, 0, E], "");
        dirs.extend(entry(&[0, E, E, 0x28, E], "d"));
        let data: Vec<u8> = b"hello in placeBBBBcc".to_vec();
        let file = |parent: u32, sib: u32, off: u64, size: u64, name: &str| {
            let mut v = vec![];
            v.extend(parent.to_le_bytes());
            v.extend(sib.to_le_bytes());
            v.extend(off.to_le_bytes());
            v.extend(size.to_le_bytes());
            v.extend(E.to_le_bytes());
            v.extend((name.len() as u32).to_le_bytes());
            v.extend(name.as_bytes());
            while v.len() % 4 != 0 {
                v.push(0);
            }
            v
        };
        let mut files = file(0, E, 0, 14, "a.txt");
        assert_eq!(files.len(), 0x28);
        files.extend(file(0x18, 0x50, 14, 4, "b.bin"));
        files.extend(file(0x18, E, 18, 2, "c.txt"));
        let dirs_at = 0x50u64;
        let files_at = dirs_at + dirs.len() as u64;
        let data_at = files_at + files.len() as u64;
        let head = [0x50u64, dirs_at, 0, dirs_at, dirs.len() as u64, files_at, 0, files_at, files.len() as u64, data_at];
        let mut img: Vec<u8> = head.iter().flat_map(|w| w.to_le_bytes()).collect();
        img.extend(dirs);
        img.extend(files);
        img.extend(data);
        img
    }

    #[test]
    fn package_romfs_in_place() {
        let rom = Arc::new(RomFs::from_image(Arc::new(Image(image())), "synthetic".into()).unwrap());
        mount("pkg", Backing::Rom(rom.clone()));
        let h = open_file("pkg:/a.txt", 1).unwrap();
        assert_eq!(read_file(h, 0, 64).unwrap(), b"hello in place");
        close_file(h);
        let h = open_file("pkg:/d/c.txt", 1).unwrap();
        assert_eq!((file_size(h), read_file(h, 1, 8).unwrap()), (Ok(2), b"c".to_vec()));
        close_file(h);
        let d = open_dir("pkg:/d", DIRMODE_FILE).unwrap();
        assert_eq!(read_dir(d, 8).unwrap(), vec![("b.bin".to_string(), ENT_FILE, 4), ("c.txt".to_string(), ENT_FILE, 2)]);
        close_dir(d);
        assert_eq!(entry_type("pkg:/d"), Ok(true));
        assert_eq!(create_file("pkg:/a.txt", 1), RES_PATH_EXISTS, "a read-only mount says it exists");
        // 2 directories (3 buckets), 3 files (3 buckets), 0x18 + 0x18 + 4 directory bytes, 3 x (0x20 + 8) file bytes
        assert_eq!(romfs_work_size(&rom), 4 * 3 + 0x18 + 0x18 + 4 + 4 * 3 + 3 * 0x28);
    }

    /// A folder RomFS (the translator's folder backend) and a transactional save, through the same calls the
    /// takeovers make.
    #[test]
    fn rom_and_save() {
        let tmp = std::env::temp_dir().join(format!("nxos-fs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("romfs/Data/Sub")).unwrap();
        std::fs::write(tmp.join("romfs/Data/a.bin"), b"hello romfs").unwrap();
        std::fs::write(tmp.join("romfs/Data/Sub/b.bin"), vec![7u8; 100]).unwrap();
        let rom = Arc::new(RomFs::from_folder(&tmp.join("romfs")).unwrap());
        mount("trom", Backing::Rom(rom.clone()));
        let h = open_file("trom:/Data/a.bin", MODE_READ).unwrap();
        assert_eq!(h % 16, 0, "handles are 16-byte aligned");
        assert_eq!(file_size(h), Ok(11));
        assert_eq!(read_file(h, 6, 100).unwrap(), b"romfs");
        assert_eq!(read_file(h, 20, 4).unwrap(), b"");
        close_file(h);
        assert_eq!(open_file("trom:/Data/nope", MODE_READ), Err(RES_PATH_NOT_FOUND));
        assert_eq!(open_file("trom:/Data/a.bin", MODE_WRITE), Err(RES_FAIL));
        assert_eq!(entry_type("trom:/Data/Sub"), Ok(true));
        let d = open_dir("trom:/Data", DIRMODE_DIR | DIRMODE_FILE).unwrap();
        assert_eq!(dir_count(d), Ok(2));
        let v = read_dir(d, 10).unwrap();
        assert_eq!(v, vec![("Sub".to_string(), ENT_DIR, 0), ("a.bin".to_string(), ENT_FILE, 11)]);
        assert!(read_dir(d, 10).unwrap().is_empty());
        close_dir(d);
        assert!(romfs_work_size(&rom) > 0x18);

        let save = tmp.join("save/user1");
        assert_eq!(mount_dir("tsave", save.clone(), false, true), RES_OK);
        assert_eq!(create_dir("tsave:/dir"), RES_OK);
        assert_eq!(create_dir("tsave:/dir"), RES_PATH_EXISTS);
        assert_eq!(create_file("tsave:/dir/f.dat", 4), RES_OK);
        let f = open_file("tsave:/dir/f.dat", MODE_READ | MODE_WRITE).unwrap();
        assert_eq!(write_file(f, 0, b"abcdef"), RES_OK);
        assert_eq!(read_file(f, 2, 10).unwrap(), b"cdef");
        close_file(f);
        assert!(!save.join("dir/f.dat").exists(), "nothing committed yet");
        assert_eq!(commit("tsave"), RES_OK);
        assert_eq!(std::fs::read(save.join("dir/f.dat")).unwrap(), b"abcdef");
        assert_eq!(rename("tsave:/dir/f.dat", "tsave:/dir/g.dat"), RES_OK);
        assert_eq!(delete_file("tsave:/dir/g.dat"), RES_OK);
        assert_eq!(delete_file("tsave:/dir/g.dat"), RES_PATH_NOT_FOUND);
        assert_eq!(std::fs::read(save.join("dir/f.dat")).unwrap(), b"abcdef", "uncommitted changes stay in the work copy");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
