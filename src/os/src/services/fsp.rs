//! `fsp-srv` for SDK file system calls that are not taken over by name (`fs.rs` takes over the usual mounts):
//! - 1 SetCurrentProcess;
//! - 18 OpenSdCardFileSystem: a host folder, `<data dir>\sdmc`, as an IFileSystem (files, directories, sizes,
//!   time stamps and free space are the host's);
//! - 60 / 61 / 62 OpenSaveDataInfoReader (all, by space, cache storage only): the game's saves that exist in its save
//!   folder (account saves of each user, the device save, the cache storages), one SaveDataInfo each;
//! - 400 OpenDeviceOperator: an SD card is inserted (the folder above), no game card;
//! - 500 / 501 the SD card and game card detection notifiers: events that never fire (nothing is ever removed).
//!
//! The save info reader lists what exists.

use crate::ipc::{self, Ctx, Service};
use crate::kobj::Event;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// fs ResultPathNotFound (2-1) and ResultPathAlreadyExists (2-2); ResultInvalidPath (2-6000) for a path that leaves
/// its root.
pub const R_NOT_FOUND: u32 = 2 | 1 << 9;
pub const R_EXISTS: u32 = 2 | 2 << 9;
pub const R_INVALID_PATH: u32 = 2 | 6000 << 9;
const R_NOT_EMPTY: u32 = 2 | 8 << 9;

struct Fsp;

impl Service for Fsp {
    fn name(&self) -> String {
        "fsp-srv".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            1 => 0,
            18 => {
                let root = crate::services::fs::data_dir().join("sdmc");
                if let Err(e) = fs::create_dir_all(&root) {
                    crate::log!("[RED] fsp-srv: the SD card folder {}: {e}", root.display());
                    return R_NOT_FOUND;
                }
                crate::log_once!("fsp sd", "fsp-srv: the SD card is {}", root.display());
                ctx.object(HostFs { root });
                0
            }
            60..=62 => {
                let space = ctx.u8(0);
                let all = saves(
                    &crate::services::fs::save_dir(),
                    crate::profile::get().program_id,
                );
                let v = match ctx.cmd() {
                    60 => all,
                    // by space: the game's saves are all in the user space (1)
                    61 => all.into_iter().filter(|_| space == 1).collect(),
                    _ => all.into_iter().filter(|s| s[9] == SAVE_CACHE).collect(),
                };
                ctx.object(SaveInfoReader { v, next: 0 });
                0
            }
            400 => {
                ctx.object(DeviceOperator);
                0
            }
            500 | 501 => {
                ctx.object(Notifier(None));
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

// ---- the game's saves as SaveDataInfo

const SAVE_ACCOUNT: u8 = 1;
const SAVE_DEVICE: u8 = 3;
const SAVE_CACHE: u8 = 5;

fn tree_size(p: &Path) -> u64 {
    fs::read_dir(p).map_or(0, |d| {
        d.flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => tree_size(&e.path()),
                _ => e.metadata().map_or(0, |m| m.len()),
            })
            .sum()
    })
}

/// SaveDataInfo (0x60): id, space 1 (user), type, uid, system save id, program id, size, index, rank.
fn save_info(id: u64, ty: u8, uid: (u64, u64), program: u64, size: u64, index: u16) -> [u8; 0x60] {
    let mut b = [0u8; 0x60];
    b[0..8].copy_from_slice(&id.to_le_bytes());
    b[8] = 1;
    b[9] = ty;
    b[0x10..0x18].copy_from_slice(&uid.0.to_le_bytes());
    b[0x18..0x20].copy_from_slice(&uid.1.to_le_bytes());
    b[0x28..0x30].copy_from_slice(&program.to_le_bytes());
    b[0x30..0x38].copy_from_slice(&size.to_le_bytes());
    b[0x38..0x3a].copy_from_slice(&index.to_le_bytes());
    b
}

/// The saves in `dir` (the layout `fs.rs` keeps: `user<n>`, `device`, `cache<n>`), committed ones only.
fn saves(dir: &Path, program: u64) -> Vec<[u8; 0x60]> {
    use crate::services::account::{UID0, UID1, USERS};
    let mut v = vec![];
    for n in 1..=USERS {
        let p = dir.join(format!("user{n}"));
        if p.is_dir() {
            v.push(save_info(
                0x10 + n,
                SAVE_ACCOUNT,
                (UID0 + n - 1, UID1),
                program,
                tree_size(&p),
                0,
            ));
        }
    }
    let d = dir.join("device");
    if d.is_dir() {
        v.push(save_info(
            0x20,
            SAVE_DEVICE,
            (0, 0),
            program,
            tree_size(&d),
            0,
        ));
    }
    for i in 0..256u16 {
        let p = dir.join(format!("cache{i}"));
        if p.is_dir() {
            v.push(save_info(
                0x100 + i as u64,
                SAVE_CACHE,
                (0, 0),
                program,
                tree_size(&p),
                i,
            ));
        }
    }
    v
}

struct SaveInfoReader {
    v: Vec<[u8; 0x60]>,
    next: usize,
}

impl Service for SaveInfoReader {
    fn name(&self) -> String {
        "ISaveDataInfoReader".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // ReadSaveDataInfo -> as many as the buffer holds, from where the last read stopped
            0 => {
                let room = (ctx.buf_out(0).size / 0x60) as usize;
                let n = room.min(self.v.len() - self.next);
                let b: Vec<u8> = self.v[self.next..self.next + n].concat();
                ctx.write_out(0, &b);
                self.next += n;
                ctx.push_u64(n as u64);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

struct DeviceOperator;

impl Service for DeviceOperator {
    fn name(&self) -> String {
        "IDeviceOperator".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // IsSdCardInserted: the SD card folder is always there
            0 => ctx.push_u8(1),
            // IsGameCardInserted: the game runs from the PC, no card
            200 => ctx.push_u8(0),
            _ => return ctx.unimplemented(),
        }
        0
    }
}

struct Notifier(Option<Event>);

impl Service for Notifier {
    fn name(&self) -> String {
        "IEventNotifier".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        if ctx.cmd() == 0 {
            let e = *self.0.get_or_insert_with(Event::new);
            ctx.copy_handle(e.readable);
        }
        0
    }
}

// ---- a host folder as an IFileSystem

pub struct HostFs {
    pub root: PathBuf,
}

/// The client's path in X buffer `i` (NUL-terminated, at most 0x301 bytes), under `root`; None when it leaves it.
fn path_in(ctx: &Ctx, root: &Path, i: usize) -> Option<PathBuf> {
    let raw = ctx.read_in(i);
    let s = String::from_utf8_lossy(
        &raw[..raw
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(raw.len())
            .min(0x301)],
    )
    .to_string();
    let mut p = root.to_path_buf();
    for part in s.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".") {
        if part == ".." || part.contains(':') {
            return None;
        }
        p.push(part);
    }
    Some(p)
}

fn free_space(p: &Path) -> (u64, u64) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
        let (mut free, mut total) = (0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) }
            != 0
        {
            return (free, total);
        }
        (0, 0)
    }
    // the web: the browser's storage quota comes from the page (navigator.storage.estimate, docs/35 W3); until then
    // the unknown answer the other hosts give on failure
    #[cfg(target_os = "emscripten")]
    {
        let _ = p;
        (0, 0)
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap_or_default();
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0 {
            return (
                s.f_bavail as u64 * s.f_frsize as u64,
                s.f_blocks as u64 * s.f_frsize as u64,
            );
        }
        (0, 0)
    }
}

fn secs(t: std::io::Result<std::time::SystemTime>) -> u64 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

impl Service for HostFs {
    fn name(&self) -> String {
        "IFileSystem(host)".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        let cmd = ctx.cmd();
        if matches!(cmd, 10..=12) {
            match cmd {
                10 => {}
                11 => ctx.push_u64(free_space(&self.root).0),
                _ => ctx.push_u64(free_space(&self.root).1),
            }
            return 0;
        }
        let Some(p) = path_in(ctx, &self.root, 0) else {
            return R_INVALID_PATH;
        };
        let io = |r: std::io::Result<()>| match r {
            Ok(()) => 0,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => R_EXISTS,
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => R_NOT_EMPTY,
            Err(_) => R_NOT_FOUND,
        };
        match cmd {
            // CreateFile(path; mode, size)
            0 => {
                if p.exists() {
                    return R_EXISTS;
                }
                let size = ctx.u64(8);
                io(File::create_new(&p).and_then(|f| f.set_len(size)))
            }
            1 if p.is_file() => io(fs::remove_file(&p)),
            2 if p.exists() => R_EXISTS,
            2 => io(fs::create_dir(&p)),
            3 if p.is_dir() && p != self.root => io(fs::remove_dir(&p)),
            4 if p.is_dir() && p != self.root => io(fs::remove_dir_all(&p)),
            // RenameFile / RenameDirectory(old, new)
            5 | 6 => {
                let Some(q) = path_in(ctx, &self.root, 1) else {
                    return R_INVALID_PATH;
                };
                if (cmd == 5 && !p.is_file()) || (cmd == 6 && !p.is_dir()) {
                    return R_NOT_FOUND;
                }
                if q.exists() {
                    return R_EXISTS;
                }
                io(fs::rename(&p, &q))
            }
            // GetEntryType: 0 directory, 1 file
            7 if p.is_dir() => {
                ctx.push_u32(0);
                0
            }
            7 if p.is_file() => {
                ctx.push_u32(1);
                0
            }
            // OpenFile(mode: 1 read, 2 write, 4 append)
            8 if p.is_file() => {
                let mode = ctx.u32(0);
                match fs::OpenOptions::new()
                    .read(true)
                    .write(mode & 6 != 0)
                    .open(&p)
                {
                    Ok(f) => {
                        ctx.object(HostFile {
                            f,
                            append: mode & 4 != 0,
                        });
                        0
                    }
                    Err(_) => R_NOT_FOUND,
                }
            }
            // OpenDirectory(filter: 1 directories, 2 files)
            9 if p.is_dir() => {
                let filter = match ctx.u32(0) & 3 {
                    0 => 3,
                    f => f,
                };
                let mut v: Vec<(String, bool, u64)> = fs::read_dir(&p)
                    .map(|d| {
                        d.flatten()
                            .filter_map(|e| {
                                let m = e.metadata().ok()?;
                                let want = if m.is_dir() {
                                    filter & 1 != 0
                                } else {
                                    filter & 2 != 0
                                };
                                want.then(|| {
                                    (
                                        e.file_name().to_string_lossy().into_owned(),
                                        m.is_dir(),
                                        if m.is_dir() { 0 } else { m.len() },
                                    )
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                v.sort();
                ctx.object(HostDir { v, next: 0 });
                0
            }
            // CleanDirectoryRecursively: empty it, keep it
            13 if p.is_dir() => {
                for e in fs::read_dir(&p).into_iter().flatten().flatten() {
                    let q = e.path();
                    let r = if q.is_dir() {
                        fs::remove_dir_all(&q)
                    } else {
                        fs::remove_file(&q)
                    };
                    if r.is_err() {
                        return R_NOT_FOUND;
                    }
                }
                0
            }
            // GetFileTimeStampRaw: created, modified, accessed (POSIX seconds), valid
            14 if p.exists() => {
                let Ok(m) = fs::metadata(&p) else {
                    return R_NOT_FOUND;
                };
                let mut b = [0u8; 0x20];
                b[0..8].copy_from_slice(&secs(m.created()).to_le_bytes());
                b[8..16].copy_from_slice(&secs(m.modified()).to_le_bytes());
                b[16..24].copy_from_slice(&secs(m.accessed()).to_le_bytes());
                b[24] = 1;
                ctx.push_bytes(&b);
                0
            }
            15 => 0,
            1 | 3 | 4 | 7 | 8 | 9 | 13 | 14 => R_NOT_FOUND,
            _ => ctx.unimplemented(),
        }
    }
}

struct HostFile {
    f: File,
    append: bool,
}

impl Service for HostFile {
    fn name(&self) -> String {
        "IFile(host)".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // Read(option; offset, size) -> bytes read
            0 => {
                let (off, size) = (ctx.u64(8), ctx.u64(16).min(ctx.buf_out(0).size));
                let mut b = vec![0u8; size.min(64 << 20) as usize];
                let n = match self
                    .f
                    .seek(SeekFrom::Start(off))
                    .and_then(|_| read_full(&mut self.f, &mut b))
                {
                    Ok(n) => n,
                    Err(_) => return R_NOT_FOUND,
                };
                ctx.write_out(0, &b[..n]);
                ctx.push_u64(n as u64);
                0
            }
            // Write(option; offset, size): option 1 flushes
            1 => {
                let (opt, off, size) = (ctx.u32(0), ctx.u64(8), ctx.u64(16));
                let data = ctx.read_in(0);
                let data = &data[..(size as usize).min(data.len())];
                let end = self.f.metadata().map_or(0, |m| m.len());
                if off + data.len() as u64 > end && !self.append {
                    // fs ResultFileExtensionWithoutOpenModeAllowAppend (2-6062)
                    return 2 | 6062 << 9;
                }
                let r = self
                    .f
                    .seek(SeekFrom::Start(off))
                    .and_then(|_| self.f.write_all(data))
                    .and_then(|_| if opt & 1 != 0 { self.f.flush() } else { Ok(()) });
                if r.is_err() {
                    return R_NOT_FOUND;
                }
                0
            }
            2 => {
                let _ = self.f.flush();
                0
            }
            // SetSize
            3 => {
                if self.f.set_len(ctx.u64(0)).is_err() {
                    return R_NOT_FOUND;
                }
                0
            }
            // GetSize
            4 => {
                ctx.push_u64(self.f.metadata().map_or(0, |m| m.len()));
                0
            }
            // OperateRange: no range information
            5 => {
                ctx.push_bytes(&[0; 0x40]);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

fn read_full(f: &mut File, b: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < b.len() {
        match f.read(&mut b[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

struct HostDir {
    v: Vec<(String, bool, u64)>,
    next: usize,
}

impl Service for HostDir {
    fn name(&self) -> String {
        "IDirectory(host)".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // Read -> DirectoryEntry[] {name[0x301], pad[3], type (0 directory, 1 file), pad[3], size}
            0 => {
                let room = (ctx.buf_out(0).size / 0x310) as usize;
                let n = room.min(self.v.len() - self.next);
                let mut b = vec![0u8; n * 0x310];
                for (k, (name, dir, size)) in self.v[self.next..self.next + n].iter().enumerate() {
                    let e = &mut b[k * 0x310..(k + 1) * 0x310];
                    let nb = name.as_bytes();
                    e[..nb.len().min(0x300)].copy_from_slice(&nb[..nb.len().min(0x300)]);
                    e[0x304] = !dir as u8;
                    e[0x308..0x310].copy_from_slice(&size.to_le_bytes());
                }
                ctx.write_out(0, &b);
                self.next += n;
                ctx.push_u64(n as u64);
                0
            }
            // GetEntryCount
            1 => {
                ctx.push_u64(self.v.len() as u64);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

pub fn register() {
    ipc::register("fsp-srv", || Box::new(Fsp));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::hipc::{self, Buf, Builder, T_REQUEST};

    fn send(
        h: u32,
        cmd: u32,
        data: Vec<u8>,
        x: Vec<Buf>,
        a: Vec<Buf>,
        b: Vec<Buf>,
    ) -> hipc::Response {
        let mut m = Builder {
            ty: T_REQUEST,
            cmd,
            data,
            x,
            a,
            b,
            ..Default::default()
        }
        .build();
        crate::ipc::process(h, &mut m);
        hipc::parse_response(&m, false).unwrap()
    }
    /// `n` bytes the test owns at an address a guest could have: an X (pointer) descriptor carries 42 address bits, so
    /// a Linux heap pointer (~2^47) would reach the service truncated. Windows heaps are already low.
    fn low_bytes(n: usize) -> &'static mut [u8] {
        #[cfg(target_os = "linux")]
        {
            use std::sync::atomic::{AtomicUsize, Ordering};
            const SIZE: usize = 1 << 20;
            static BASE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
            static USED: AtomicUsize = AtomicUsize::new(0);
            let base = *BASE.get_or_init(|| {
                // exactly there or not at all (a plain hint is ignored): the first free candidate below 2^42
                (1..64usize)
                    .find_map(|k| {
                        let want = k << 32;
                        let p = unsafe {
                            libc::mmap(want as *mut libc::c_void, SIZE, libc::PROT_READ | libc::PROT_WRITE,
                                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE, -1, 0)
                        };
                        (p != libc::MAP_FAILED).then_some(p as usize)
                    })
                    .expect("no free low range for the test's path buffers")
            });
            let off = USED.fetch_add(n.next_multiple_of(16), Ordering::Relaxed);
            assert!(off + n <= SIZE, "test scratch exhausted");
            let s = unsafe { std::slice::from_raw_parts_mut((base + off) as *mut u8, n) };
            assert!((s.as_ptr() as u64) < 1 << 42, "test buffer must fit an X descriptor's address");
            s
        }
        #[cfg(not(target_os = "linux"))]
        {
            Box::leak(vec![0u8; n].into_boxed_slice())
        }
    }
    fn path(s: &str) -> (&'static [u8], Vec<Buf>) {
        let v = low_bytes(0x301);
        v.fill(0);
        v[..s.len()].copy_from_slice(s.as_bytes());
        let b = vec![Buf {
            addr: v.as_ptr() as u64,
            size: 0x301,
            tag: 0,
        }];
        (v, b)
    }
    fn u64_at(r: &hipc::Response, o: usize) -> u64 {
        u64::from_le_bytes(r.data[o..o + 8].try_into().unwrap())
    }

    /// A host folder as the SD card's IFileSystem (fsp 18's object): create, write past the end only
    /// with append, read back, size, entry types, directory listing (DirectoryEntry layout), rename, time stamps,
    /// free space, the not-found / exists results and a path leaving the root refused.
    #[test]
    fn host_folder_as_a_file_system() {
        let root = std::env::temp_dir().join(format!("nxos-fsp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let h = crate::ipc::open(Box::new(HostFs { root: root.clone() }));
        let (_p, x) = path("/dir");
        assert_eq!(send(h, 2, vec![], x.clone(), vec![], vec![]).result, 0);
        assert_eq!(
            send(h, 2, vec![], x.clone(), vec![], vec![]).result,
            R_EXISTS
        );
        let (_q, xf) = path("/dir/a.bin");
        assert_eq!(
            send(
                h,
                0,
                [0u64.to_le_bytes(), 4u64.to_le_bytes()].concat(),
                xf.clone(),
                vec![],
                vec![]
            )
            .result,
            0
        );
        assert_eq!(fs::metadata(root.join("dir/a.bin")).unwrap().len(), 4);
        let r = send(
            h,
            8,
            6u32.to_le_bytes().to_vec(),
            xf.clone(),
            vec![],
            vec![],
        );
        assert_eq!((r.result, r.moved.len()), (0, 1));
        let f = r.moved[0];
        let data = *b"hello world";
        let wa = vec![Buf {
            addr: data.as_ptr() as u64,
            size: data.len() as u64,
            tag: 0,
        }];
        assert_eq!(
            send(
                f,
                1,
                [
                    1u64.to_le_bytes(),
                    2u64.to_le_bytes(),
                    (data.len() as u64).to_le_bytes()
                ]
                .concat(),
                vec![],
                wa,
                vec![]
            )
            .result,
            0
        );
        assert_eq!(u64_at(&send(f, 4, vec![], vec![], vec![], vec![]), 0), 13);
        let mut rb = [0u8; 32];
        let r = send(
            f,
            0,
            [0u64.to_le_bytes(), 0u64.to_le_bytes(), 32u64.to_le_bytes()].concat(),
            vec![],
            vec![],
            vec![Buf {
                addr: rb.as_mut_ptr() as u64,
                size: 32,
                tag: 0,
            }],
        );
        assert_eq!(u64_at(&r, 0), 13);
        assert_eq!(&rb[..13], b"\0\0hello world");
        crate::ipc::close(f);
        let (_r, xr) = path("/dir/a.bin");
        let r = send(h, 8, 2u32.to_le_bytes().to_vec(), xr, vec![], vec![]);
        let wb = *b"zz";
        let wa = vec![Buf {
            addr: wb.as_ptr() as u64,
            size: 2,
            tag: 0,
        }];
        assert_eq!(
            send(
                r.moved[0],
                1,
                [0u64.to_le_bytes(), 13u64.to_le_bytes(), 2u64.to_le_bytes()].concat(),
                vec![],
                wa,
                vec![]
            )
            .result,
            2 | 6062 << 9,
            "no growing without append"
        );
        crate::ipc::close(r.moved[0]);
        assert_eq!(
            &send(h, 7, vec![], xf.clone(), vec![], vec![]).data[..4],
            &1u32.to_le_bytes()
        );
        assert_eq!(
            &send(h, 7, vec![], x.clone(), vec![], vec![]).data[..4],
            &0u32.to_le_bytes()
        );
        let (_n, xn) = path("/nope");
        assert_eq!(send(h, 7, vec![], xn, vec![], vec![]).result, R_NOT_FOUND);
        let (_e, xe) = path("/../escape");
        assert_eq!(
            send(h, 7, vec![], xe, vec![], vec![]).result,
            R_INVALID_PATH
        );
        let r = send(h, 9, 3u32.to_le_bytes().to_vec(), x.clone(), vec![], vec![]);
        let d = r.moved[0];
        assert_eq!(u64_at(&send(d, 1, vec![], vec![], vec![], vec![]), 0), 1);
        let mut ents = vec![0u8; 0x310 * 2];
        let r = send(
            d,
            0,
            vec![],
            vec![],
            vec![],
            vec![Buf {
                addr: ents.as_mut_ptr() as u64,
                size: ents.len() as u64,
                tag: 0,
            }],
        );
        assert_eq!(u64_at(&r, 0), 1);
        assert_eq!(&ents[..6], b"a.bin\0");
        assert_eq!(
            (
                ents[0x304],
                u64::from_le_bytes(ents[0x308..0x310].try_into().unwrap())
            ),
            (1, 13)
        );
        let (_m, xm) = path("/dir/b.bin");
        assert_eq!(
            send(h, 5, vec![], [xf[0], xm[0]].to_vec(), vec![], vec![]).result,
            0
        );
        assert!(root.join("dir/b.bin").is_file());
        let r = send(h, 14, vec![], xm.clone(), vec![], vec![]);
        assert!(
            u64_at(&r, 8) > 1_600_000_000 && r.data[24] == 1,
            "modified time, valid"
        );
        assert!(
            u64_at(&send(h, 12, vec![], vec![], vec![], vec![]), 0) > 0,
            "the disk's size"
        );
        assert_eq!(
            send(h, 3, vec![], x.clone(), vec![], vec![]).result,
            R_NOT_EMPTY
        );
        assert_eq!(send(h, 4, vec![], x, vec![], vec![]).result, 0);
        assert!(!root.join("dir").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// The save info reader lists the saves that exist (user1 and cache2 here) as SaveDataInfo: type, user space,
    /// uid, program id, size, index; read in pieces; the cache-only reader (62) only the cache.
    #[test]
    fn save_info_lists_existing_saves() {
        let dir = std::env::temp_dir().join(format!("nxos-saves-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("user1")).unwrap();
        fs::write(dir.join("user1/save.dat"), [1u8; 100]).unwrap();
        fs::create_dir_all(dir.join("cache2")).unwrap();
        let v = saves(&dir, 0x0100_0000_0000_1000);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0][8], v[0][9]), (1, SAVE_ACCOUNT));
        assert_eq!(
            u64::from_le_bytes(v[0][0x10..0x18].try_into().unwrap()),
            crate::services::account::UID0
        );
        assert_eq!(
            u64::from_le_bytes(v[0][0x28..0x30].try_into().unwrap()),
            0x0100_0000_0000_1000
        );
        assert_eq!(
            u64::from_le_bytes(v[0][0x30..0x38].try_into().unwrap()),
            100
        );
        assert_eq!(
            (v[1][9], u16::from_le_bytes([v[1][0x38], v[1][0x39]])),
            (SAVE_CACHE, 2)
        );
        let h = crate::ipc::open(Box::new(SaveInfoReader { v, next: 0 }));
        let mut b = [0u8; 0x60];
        let ba = b.as_mut_ptr() as u64;
        let one = move || {
            vec![Buf {
                addr: ba,
                size: 0x60,
                tag: 0,
            }]
        };
        assert_eq!(u64_at(&send(h, 0, vec![], vec![], vec![], one()), 0), 1);
        assert_eq!(b[9], SAVE_ACCOUNT);
        assert_eq!(u64_at(&send(h, 0, vec![], vec![], vec![], one()), 0), 1);
        assert_eq!(b[9], SAVE_CACHE);
        assert_eq!(
            u64_at(&send(h, 0, vec![], vec![], vec![], one()), 0),
            0,
            "then none"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// fsp-srv's own commands: 1 succeeds, 400 a device operator with an SD card and no game card, 500 a notifier
    /// whose event never fires, 61 for another space lists nothing.
    #[test]
    fn fsp_srv_objects() {
        crate::mem::tests::test_kernel();
        register();
        let h = crate::ipc::connect("fsp-srv").unwrap();
        assert_eq!(send(h, 1, vec![0; 8], vec![], vec![], vec![]).result, 0);
        let op = send(h, 400, vec![], vec![], vec![], vec![]).moved[0];
        assert_eq!(send(op, 0, vec![], vec![], vec![], vec![]).data[0], 1);
        assert_eq!(send(op, 200, vec![], vec![], vec![], vec![]).data[0], 0);
        let n = send(h, 500, vec![], vec![], vec![], vec![]).moved[0];
        let r = send(n, 0, vec![], vec![], vec![], vec![]);
        assert_eq!(nxrt::kernel::get().event_signaled(r.copy[0]), Some(false));
        let rd = send(h, 61, vec![0], vec![], vec![], vec![]).moved[0];
        let mut b = [0u8; 0x60];
        assert_eq!(
            u64_at(
                &send(
                    rd,
                    0,
                    vec![],
                    vec![],
                    vec![],
                    vec![Buf {
                        addr: b.as_mut_ptr() as u64,
                        size: 0x60,
                        tag: 0
                    }]
                ),
                0
            ),
            0
        );
    }
}
