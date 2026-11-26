//! A RomFS written out into the build: the game's data folder gets its own copy (decrypted, the update's patch and a
//! NUP's tree already resolved), so a built game runs without its source package.
//!
//! [`tree`] writes the files into a folder (`romfs/`, what the runtime serves file by file); [`image`] writes the
//! RomFS image as one file (`romfs.bin`), for a tree a Windows folder cannot hold faithfully ([`unsafe_name`]: a
//! reserved or ill-formed name, or two names that differ only in case). Both write beside the target and rename it
//! into place once complete, so an interrupted build never leaves a half copy the runtime would serve.
use crate::package::error::{fmt, PackageError, Result};
use crate::package::romfs::{Node, NodeKind, RomFs};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Bytes read from the RomFS per call while copying a file.
const CHUNK: u64 = 16 << 20;
/// How often the progress callback is called while files are written.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

fn io_err(p: &Path) -> impl Fn(std::io::Error) -> PackageError + '_ {
    move |source| PackageError::Io {
        path: p.to_path_buf(),
        source,
    }
}

/// A name a Windows folder holds as itself: what an ExeFS name must be ([`super::safe_exefs_name`]) and none of
/// `<>"|?*` or a control character.
fn name_ok(name: &str) -> bool {
    super::safe_exefs_name(name)
        && !name
            .chars()
            .any(|c| (c as u32) < 0x20 || "<>\"|?*".contains(c))
}

/// The first path of the tree a Windows folder cannot hold as it is (None: the tree can be written as files).
pub fn unsafe_name(romfs: &RomFs) -> Option<String> {
    fn go(node: &Node, prefix: &str) -> Option<String> {
        let mut seen = HashSet::new();
        for k in node.children() {
            let p = format!("{prefix}/{}", k.name);
            if !name_ok(&k.name) || !seen.insert(k.name.to_uppercase()) {
                return Some(p);
            }
            if let Some(bad) = go(k, &p) {
                return Some(bad);
            }
        }
        None
    }
    go(romfs.root(), "")
}

/// (folders, files with their sizes) under `node`, as `/`-separated paths.
fn collect(node: &Node, prefix: &str, dirs: &mut Vec<String>, files: &mut Vec<(String, u64)>) {
    for k in node.children() {
        let p = if prefix.is_empty() {
            k.name.clone()
        } else {
            format!("{prefix}/{}", k.name)
        };
        match &k.kind {
            NodeKind::Dir(_) => {
                dirs.push(p.clone());
                collect(k, &p, dirs, files);
            }
            NodeKind::File { size, .. } => files.push((p, *size)),
        }
    }
}

/// `dest` with `.partial` after its name: where a copy is written before it is renamed into place.
fn partial_of(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    dest.with_file_name(name)
}

fn remove_any(p: &Path) -> Result<()> {
    let r = if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else if p.exists() {
        std::fs::remove_file(p)
    } else {
        return Ok(());
    };
    r.map_err(io_err(p))
}

/// Replace `dest` with the finished `partial`.
fn put_in_place(partial: &Path, dest: &Path) -> Result<()> {
    remove_any(dest)?;
    std::fs::rename(partial, dest).map_err(io_err(dest))
}

fn copy_file(romfs: &RomFs, path: &str, size: u64, to: &Path, done: &AtomicU64) -> Result<()> {
    let mut f = std::fs::File::create(to).map_err(io_err(to))?;
    let mut at = 0;
    while at < size {
        let data = romfs.read(path, at, CHUNK.min(size - at))?;
        if data.is_empty() {
            return fmt(format!("{path}: the RomFS file ends before its size"));
        }
        f.write_all(&data).map_err(io_err(to))?;
        at += data.len() as u64;
        done.fetch_add(data.len() as u64, Ordering::Relaxed);
    }
    Ok(())
}

/// Write every folder and file of `romfs` into the folder `dest` (replaced when it exists), on `threads` threads.
/// `progress(bytes written, total bytes)` is called from this thread while it works. Returns (files, bytes).
pub fn tree(
    romfs: &RomFs,
    dest: &Path,
    threads: usize,
    progress: &dyn Fn(u64, u64),
) -> Result<(usize, u64)> {
    if let Some(bad) = unsafe_name(romfs) {
        return fmt(format!(
            "{bad}: this RomFS name cannot be a file in a Windows folder"
        ));
    }
    let (mut dirs, mut files) = (vec![], vec![]);
    collect(romfs.root(), "", &mut dirs, &mut files);
    let total: u64 = files.iter().map(|f| f.1).sum();
    let partial = partial_of(dest);
    remove_any(&partial)?;
    std::fs::create_dir_all(&partial).map_err(io_err(&partial))?;
    for d in &dirs {
        let p = partial.join(d);
        std::fs::create_dir_all(&p).map_err(io_err(&p))?;
    }
    // the largest first: the threads finish together
    files.sort_by(|a, b| b.1.cmp(&a.1));
    let (next, done, stop) = (
        AtomicUsize::new(0),
        AtomicU64::new(0),
        AtomicBool::new(false),
    );
    let failed: Mutex<Option<PackageError>> = Mutex::new(None);
    let working = AtomicUsize::new(threads.max(1));
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((path, size)) = files.get(i) else {
                        break;
                    };
                    if let Err(e) = copy_file(romfs, path, *size, &partial.join(path), &done) {
                        stop.store(true, Ordering::Relaxed);
                        failed
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .get_or_insert(e);
                    }
                }
                working.fetch_sub(1, Ordering::Release);
            });
        }
        let mut last = Instant::now();
        while working.load(Ordering::Acquire) > 0 {
            std::thread::sleep(Duration::from_millis(20));
            if last.elapsed() >= PROGRESS_EVERY {
                progress(done.load(Ordering::Relaxed), total);
                last = Instant::now();
            }
        }
    });
    if let Some(e) = failed.into_inner().unwrap_or_else(|p| p.into_inner()) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    progress(total, total);
    put_in_place(&partial, dest)?;
    Ok((files.len(), total))
}

/// Write the RomFS image of `romfs` (an image-backed one: a package's) into the file `dest`. Returns its size.
pub fn image(romfs: &RomFs, dest: &Path, progress: &dyn Fn(u64, u64)) -> Result<u64> {
    let Some(img) = romfs.image() else {
        return fmt(format!(
            "{}: this RomFS is not an image and its names cannot be files in a Windows folder",
            romfs.source
        ));
    };
    let partial = partial_of(dest);
    remove_any(&partial)?;
    let total = img.size();
    let mut f = std::fs::File::create(&partial).map_err(io_err(&partial))?;
    let (mut at, mut last) = (0, Instant::now());
    let r = (|| -> Result<()> {
        while at < total {
            let n = CHUNK.min(total - at);
            f.write_all(&img.read(at, n)?).map_err(io_err(&partial))?;
            at += n;
            if last.elapsed() >= PROGRESS_EVERY {
                progress(at, total);
                last = Instant::now();
            }
        }
        f.flush().map_err(io_err(&partial))
    })();
    drop(f);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    progress(total, total);
    put_in_place(&partial, dest)?;
    Ok(total)
}

/// Threads for [`tree`]: the machine's, at most 16 (the disk is the limit past that).
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::romfs::build_image;
    use crate::package::storage::Storage;
    use std::sync::Arc;

    struct Mem(Vec<u8>);
    impl Storage for Mem {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
            Ok(self.0[offset as usize..(offset + size) as usize].to_vec())
        }
    }

    fn rom(files: &[(&str, &[u8])]) -> RomFs {
        RomFs::from_image(Arc::new(Mem(build_image(files))), "test".into()).unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nxrs-extract-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Every file comes out with its bytes; the copy replaces an older tree; the folder reads back as the same
    /// RomFS through the runtime's folder backend.
    #[test]
    fn tree_writes_every_file_and_replaces_the_old_copy() {
        let big: Vec<u8> = (0..(CHUNK as usize + 777)).map(|i| i as u8).collect();
        let r = rom(&[("a.txt", b"alpha"), ("d/b.bin", &big), ("d/empty", b"")]);
        let d = tmp("tree");
        let dest = d.join("romfs");
        std::fs::create_dir_all(dest.join("stale")).unwrap();
        let (n, bytes) = tree(&r, &dest, 3, &|_, _| {}).unwrap();
        assert_eq!((n, bytes), (3, 5 + big.len() as u64));
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(dest.join("d/b.bin")).unwrap(), big);
        assert!(!dest.join("stale").exists() && !d.join("romfs.partial").exists());
        let back = RomFs::from_folder(&dest).unwrap();
        assert_eq!(back.walk(), r.walk());
        assert_eq!(back.read("d/b.bin", 5, 3).unwrap(), &big[5..8]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Names Windows folders cannot hold are found (case twins included); such a tree is refused as files and
    /// written as the image, which reads back the same.
    #[test]
    fn unsafe_names_go_to_the_image() {
        assert!(unsafe_name(&rom(&[("ok/a.txt", b"1")])).is_none());
        for bad in ["d/CON", "d/a?b", "d/x. "] {
            assert!(unsafe_name(&rom(&[(bad, b"1")])).is_some(), "{bad}");
        }
        let twins = rom(&[("d/Map.bin", b"1"), ("d/map.bin", b"2")]);
        assert_eq!(unsafe_name(&twins).as_deref(), Some("/d/map.bin"));
        let d = tmp("image");
        assert!(tree(&twins, &d.join("romfs"), 2, &|_, _| {}).is_err());
        let file = d.join("romfs.bin");
        image(&twins, &file, &|_, _| {}).unwrap();
        let blob = Arc::new(crate::package::io::Blob::open(&file).unwrap());
        let size = blob.size;
        let st = crate::package::storage::FileRange {
            blob,
            offset: 0,
            size,
        };
        let back = RomFs::from_image(Arc::new(st), "file".into()).unwrap();
        assert_eq!(back.read_file("d/map.bin").unwrap(), b"2");
        let _ = std::fs::remove_dir_all(&d);
    }
}
