//! A game's (or an add-on title's) RomFS, read in place: a RomFS image inside a package (an NSP section, plain,
//! AES-CTR or BKTR-patched), the file tree of a NUP, or a dumped folder's `romfs/`. Nothing is extracted.
use crate::package::error::{fmt, PackageError, Result};
use crate::package::io::{u32le, u64le};
use crate::package::nup::{FileNode as NupFile, NupReader};
use crate::package::storage::Storage;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_TABLE: u64 = 256 << 20;
const EMPTY: u32 = 0xffff_ffff;

/// Where a file's bytes are.
#[derive(Debug, Clone)]
pub enum FileLoc {
    /// Offset in the RomFS image.
    Image(u64),
    Nup(NupFile),
    Disk(PathBuf),
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Dir(Vec<Node>),
    File { size: u64, loc: FileLoc },
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub kind: NodeKind,
}

impl Node {
    pub fn is_file(&self) -> bool {
        matches!(self.kind, NodeKind::File { .. })
    }
    pub fn size(&self) -> u64 {
        match &self.kind {
            NodeKind::File { size, .. } => *size,
            NodeKind::Dir(_) => 0,
        }
    }
    pub fn children(&self) -> &[Node] {
        match &self.kind {
            NodeKind::Dir(k) => k,
            NodeKind::File { .. } => &[],
        }
    }
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

pub(crate) enum Backend {
    Image(Arc<dyn Storage>),
    Nup(Arc<NupReader>),
    Folder,
}

/// A RomFS file tree with in-place reads. `Send + Sync`: share it (`Arc<RomFs>`) between reader threads.
pub struct RomFs {
    root: Node,
    backend: Backend,
    /// What it is read from, for logs: `NSP <path>`, `NUP <path>`, `folder <path>`.
    pub source: String,
}

impl std::fmt::Debug for RomFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RomFs({})", self.source)
    }
}

fn sort_kids(kids: &mut [Node]) {
    kids.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
}

/// nsp.py `romfs_tree`: the directory tree of a RomFS image of `romfs_size` bytes that `read` returns.
pub fn image_tree(path: &str, romfs_size: u64, read: &dyn Fn(u64, u64) -> Result<Vec<u8>>) -> Result<Node> {
    let head = read(0, 0x50)?;
    let v: Vec<u64> = (0..10).map(|i| u64le(&head, i * 8)).collect();
    if v[0] != 0x50 {
        return fmt(format!("{path}: invalid RomFS header"));
    }
    let (dirs_at, dirs_size, files_at, files_size, data_at) = (v[3], v[4], v[7], v[8], v[9]);
    for (at, size) in [(dirs_at, dirs_size), (files_at, files_size)] {
        if size > MAX_TABLE || at > romfs_size || size > romfs_size - at {
            return fmt(format!("{path}: RomFS metadata is out of range"));
        }
    }
    if data_at > romfs_size {
        return fmt(format!("{path}: RomFS data offset is out of range"));
    }
    let dirs = read(dirs_at, dirs_size)?;
    let files = read(files_at, files_size)?;
    let mut seen_dirs = HashSet::new();
    let mut seen_files = HashSet::new();

    fn name(path: &str, table: &[u8], at: usize, fixed: usize) -> Result<String> {
        if at + fixed > table.len() {
            return fmt(format!("{path}: RomFS entry is out of range"));
        }
        let length = u32le(table, at + fixed - 4) as usize;
        if length > 0x1000 || length > table.len() - at - fixed {
            return fmt(format!("{path}: RomFS name is out of range"));
        }
        String::from_utf8(table[at + fixed..at + fixed + length].to_vec())
            .map_err(|_| PackageError::Format(format!("{path}: non-UTF-8 RomFS name")))
    }

    #[allow(clippy::too_many_arguments)]
    fn directory(
        path: &str,
        dirs: &[u8],
        files: &[u8],
        romfs_size: u64,
        data_at: u64,
        at: u32,
        depth: u32,
        seen_dirs: &mut HashSet<u32>,
        seen_files: &mut HashSet<u32>,
    ) -> Result<Node> {
        if !seen_dirs.insert(at) || depth > 128 {
            return fmt(format!("{path}: cyclic RomFS directory tree"));
        }
        let a = at as usize;
        if a + 0x18 > dirs.len() {
            return fmt(format!("{path}: RomFS directory is out of range"));
        }
        let (child, first_file) = (u32le(dirs, a + 8), u32le(dirs, a + 12));
        let dname = name(path, dirs, a, 0x18)?;
        let mut kids = vec![];
        let mut current = child;
        while current != EMPTY {
            kids.push(directory(path, dirs, files, romfs_size, data_at, current, depth + 1, seen_dirs, seen_files)?);
            let c = current as usize;
            if c + 8 > dirs.len() {
                return fmt(format!("{path}: RomFS directory is out of range"));
            }
            current = u32le(dirs, c + 4);
        }
        let mut current = first_file;
        while current != EMPTY {
            let c = current as usize;
            if !seen_files.insert(current) || c + 0x20 > files.len() {
                return fmt(format!("{path}: cyclic or invalid RomFS file list"));
            }
            let (sibling, rel, size) = (u32le(files, c + 4), u64le(files, c + 8), u64le(files, c + 16));
            if rel > romfs_size - data_at || size > romfs_size - data_at - rel {
                return fmt(format!("{path}: RomFS file is out of range"));
            }
            kids.push(Node {
                name: name(path, files, c, 0x20)?,
                kind: NodeKind::File { size, loc: FileLoc::Image(data_at + rel) },
            });
            current = sibling;
        }
        sort_kids(&mut kids);
        if kids.windows(2).any(|w| w[0].name == w[1].name) {
            return fmt(format!("{path}: duplicate RomFS name"));
        }
        Ok(Node { name: dname, kind: NodeKind::Dir(kids) })
    }
    directory(path, &dirs, &files, romfs_size, data_at, 0, 0, &mut seen_dirs, &mut seen_files)
}

fn folder_tree(dir: &Path, depth: u32) -> Result<Vec<Node>> {
    if depth > 128 {
        return fmt(format!("{}: the folder is nested too deeply", dir.display()));
    }
    let io = |source| PackageError::Io { path: dir.to_path_buf(), source };
    let mut kids = vec![];
    for e in std::fs::read_dir(dir).map_err(io)? {
        let e = e.map_err(io)?;
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let meta = std::fs::metadata(&p).map_err(|source| PackageError::Io { path: p.clone(), source })?;
        if meta.is_dir() {
            kids.push(Node { name, kind: NodeKind::Dir(folder_tree(&p, depth + 1)?) });
        } else {
            kids.push(Node { name, kind: NodeKind::File { size: meta.len(), loc: FileLoc::Disk(p) } });
        }
    }
    sort_kids(&mut kids);
    Ok(kids)
}

impl RomFs {
    /// A RomFS image (the IVFC level holding the RomFS) of a storage.
    pub fn from_image(image: Arc<dyn Storage>, source: String) -> Result<RomFs> {
        let size = image.size();
        let read = |o: u64, n: u64| image.read(o, n);
        let root = image_tree(&source, size, &read)?;
        Ok(RomFs { root, backend: Backend::Image(image), source })
    }

    pub(crate) fn from_nup(reader: Arc<NupReader>, root: Node, source: String) -> RomFs {
        RomFs { root, backend: Backend::Nup(reader), source }
    }

    /// A folder of files served as a RomFS (a dump's `romfs/`, or a DLC title's hand-filled `romfs/`).
    pub fn from_folder(dir: &Path) -> Result<RomFs> {
        if !dir.is_dir() {
            return fmt(format!("{}: no romfs/ folder", dir.display()));
        }
        let kids = folder_tree(dir, 0)?;
        Ok(RomFs { root: Node { name: String::new(), kind: NodeKind::Dir(kids) }, backend: Backend::Folder, source: format!("folder {}", dir.display()) })
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The raw RomFS image when the RomFS is one (NSP sources): what a console's storage would return.
    pub fn image(&self) -> Option<Arc<dyn Storage>> {
        match &self.backend {
            Backend::Image(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// The node at a `/`-separated path (`""` or `/` is the root).
    pub fn find(&self, path: &str) -> Option<&Node> {
        let mut node = &self.root;
        for part in path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".") {
            let kids = match &node.kind {
                NodeKind::Dir(k) => k,
                NodeKind::File { .. } => return None,
            };
            let i = kids.binary_search_by(|k| k.name.as_bytes().cmp(part.as_bytes())).ok()?;
            node = &kids[i];
        }
        Some(node)
    }

    /// The entries of a directory, sorted by name.
    pub fn list(&self, path: &str) -> Result<Vec<DirEntry>> {
        match self.find(path) {
            Some(Node { kind: NodeKind::Dir(kids), .. }) => Ok(kids
                .iter()
                .map(|k| DirEntry { name: k.name.clone(), is_dir: !k.is_file(), size: k.size() })
                .collect()),
            Some(_) => fmt(format!("{path}: not a directory")),
            None => fmt(format!("{path}: no such file or directory in the RomFS")),
        }
    }

    /// Every file: (path, size), depth first, names sorted.
    pub fn walk(&self) -> Vec<(String, u64)> {
        fn go(node: &Node, prefix: &str, out: &mut Vec<(String, u64)>) {
            for k in node.children() {
                let p = if prefix.is_empty() { k.name.clone() } else { format!("{prefix}/{}", k.name) };
                match &k.kind {
                    NodeKind::File { size, .. } => out.push((p, *size)),
                    NodeKind::Dir(_) => go(k, &p, out),
                }
            }
        }
        let mut out = vec![];
        go(&self.root, "", &mut out);
        out
    }

    /// Up to `len` bytes of a file from `offset` (fewer at the end of the file; none at or past it).
    pub fn read(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let Some(node) = self.find(path) else { return fmt(format!("{path}: no such file in the RomFS")) };
        let NodeKind::File { size, loc } = &node.kind else { return fmt(format!("{path}: is a directory")) };
        if offset >= *size {
            return Ok(vec![]);
        }
        let len = len.min(size - offset);
        match (loc, &self.backend) {
            (FileLoc::Image(at), Backend::Image(img)) => img.read(at + offset, len),
            (FileLoc::Nup(f), Backend::Nup(r)) => r.read(f, offset, len),
            (FileLoc::Disk(p), _) => {
                use std::io::{Read, Seek, SeekFrom};
                let io = |source| PackageError::Io { path: p.clone(), source };
                let mut f = std::fs::File::open(p).map_err(io)?;
                f.seek(SeekFrom::Start(offset)).map_err(io)?;
                let mut out = Vec::with_capacity(len as usize);
                f.take(len).read_to_end(&mut out).map_err(io)?;
                Ok(out)
            }
            _ => fmt(format!("{path}: file location does not match its RomFS")),
        }
    }

    /// A whole file.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.read(path, 0, u64::MAX)
    }
}

/// A tiny RomFS image builder for tests: `files` are (path, bytes) with `/`-separated paths.
#[cfg(test)]
pub(crate) fn build_image(files: &[(&str, &[u8])]) -> Vec<u8> {
    // directories: collect, assign table offsets; one level of subdirectories is enough for the tests
    let mut dir_names: Vec<String> = vec![String::new()];
    for (p, _) in files {
        if let Some((d, _)) = p.rsplit_once('/') {
            if !dir_names.iter().any(|x| x == d) {
                dir_names.push(d.to_string());
            }
        }
    }
    let align4 = |n: usize| (n + 3) & !3;
    let mut dir_off = vec![];
    let mut at = 0;
    for d in &dir_names {
        dir_off.push(at);
        let leaf = d.rsplit('/').next().unwrap();
        at += 0x18 + align4(leaf.len());
    }
    let dirs_size = at;
    let mut file_off = vec![];
    let mut at = 0;
    for (p, _) in files {
        file_off.push(at);
        let leaf = p.rsplit('/').next().unwrap();
        at += 0x20 + align4(leaf.len());
    }
    let files_size = at;
    let parent_of = |p: &str| -> usize {
        let d = p.rsplit_once('/').map_or("", |x| x.0);
        dir_names.iter().position(|x| x == d).unwrap()
    };
    let mut dirs = vec![0u8; dirs_size];
    for (i, d) in dir_names.iter().enumerate() {
        let o = dir_off[i];
        let leaf = if i == 0 { "" } else { d.rsplit('/').next().unwrap() };
        let parent = if i == 0 { 0 } else { dir_off[parent_of(&format!("{d}/x"))] };
        let subs: Vec<usize> = (1..dir_names.len()).filter(|&j| parent_of(&format!("{}/x", dir_names[j])) == i && j != i && dir_names[j].rsplit_once('/').map_or("", |x| x.0) == *d).collect();
        let subs: Vec<usize> = if i == 0 { (1..dir_names.len()).filter(|&j| !dir_names[j].contains('/')).collect() } else { subs };
        let sibling = if i == 0 { EMPTY } else {
            let peers: Vec<usize> = (1..dir_names.len()).filter(|&j| dir_names[j].rsplit_once('/').map(|x| x.0) == d.rsplit_once('/').map(|x| x.0)).collect();
            let pos = peers.iter().position(|&j| j == i).unwrap();
            peers.get(pos + 1).map_or(EMPTY, |&j| dir_off[j] as u32)
        };
        let child = subs.first().map_or(EMPTY, |&j| dir_off[j] as u32);
        let mine: Vec<usize> = (0..files.len()).filter(|&f| parent_of(files[f].0) == i).collect();
        let first = mine.first().map_or(EMPTY, |&f| file_off[f] as u32);
        dirs[o..o + 4].copy_from_slice(&(parent as u32).to_le_bytes());
        dirs[o + 4..o + 8].copy_from_slice(&sibling.to_le_bytes());
        dirs[o + 8..o + 12].copy_from_slice(&child.to_le_bytes());
        dirs[o + 12..o + 16].copy_from_slice(&first.to_le_bytes());
        dirs[o + 16..o + 20].copy_from_slice(&EMPTY.to_le_bytes());
        dirs[o + 20..o + 24].copy_from_slice(&(leaf.len() as u32).to_le_bytes());
        dirs[o + 24..o + 24 + leaf.len()].copy_from_slice(leaf.as_bytes());
    }
    let mut ftab = vec![0u8; files_size];
    let mut data = vec![];
    for (i, (p, bytes)) in files.iter().enumerate() {
        let o = file_off[i];
        let leaf = p.rsplit('/').next().unwrap();
        let d = parent_of(p);
        let mine: Vec<usize> = (0..files.len()).filter(|&f| parent_of(files[f].0) == d).collect();
        let pos = mine.iter().position(|&f| f == i).unwrap();
        let sibling = mine.get(pos + 1).map_or(EMPTY, |&f| file_off[f] as u32);
        while data.len() % 16 != 0 {
            data.push(0);
        }
        ftab[o..o + 4].copy_from_slice(&(dir_off[d] as u32).to_le_bytes());
        ftab[o + 4..o + 8].copy_from_slice(&sibling.to_le_bytes());
        ftab[o + 8..o + 16].copy_from_slice(&(data.len() as u64).to_le_bytes());
        ftab[o + 16..o + 24].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
        ftab[o + 24..o + 28].copy_from_slice(&EMPTY.to_le_bytes());
        ftab[o + 28..o + 32].copy_from_slice(&(leaf.len() as u32).to_le_bytes());
        ftab[o + 32..o + 32 + leaf.len()].copy_from_slice(leaf.as_bytes());
        data.extend_from_slice(bytes);
    }
    let dirs_at = 0x50u64;
    let files_at = dirs_at + dirs.len() as u64;
    let data_at = (files_at + ftab.len() as u64 + 15) & !15;
    let mut img = vec![0u8; data_at as usize];
    let head = [0x50u64, 0, 0, dirs_at, dirs.len() as u64, 0, 0, files_at, ftab.len() as u64, data_at];
    for (i, v) in head.iter().enumerate() {
        img[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    img[dirs_at as usize..files_at as usize].copy_from_slice(&dirs);
    img[files_at as usize..files_at as usize + ftab.len()].copy_from_slice(&ftab);
    img.extend(data);
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tree_list_and_read() {
        let img = build_image(&[("a.txt", b"hello"), ("dir/b.bin", b"0123456789"), ("dir/c", b""), ("z", b"zz")]);
        let r = RomFs::from_image(Arc::new(img), "test".into()).unwrap();
        let names: Vec<_> = r.list("").unwrap().into_iter().map(|e| (e.name, e.is_dir)).collect();
        assert_eq!(names, [("a.txt".into(), false), ("dir".into(), true), ("z".into(), false)]);
        assert_eq!(r.walk(), [("a.txt".to_string(), 5), ("dir/b.bin".into(), 10), ("dir/c".into(), 0), ("z".into(), 2)]);
        assert_eq!(r.read("dir/b.bin", 3, 4).unwrap(), b"3456");
        assert_eq!(r.read("dir/b.bin", 8, 100).unwrap(), b"89");
        assert_eq!(r.read("/a.txt", 0, u64::MAX).unwrap(), b"hello");
        assert!(r.read("dir/b.bin", 10, 1).unwrap().is_empty());
        assert!(r.read("nope", 0, 1).is_err());
        assert!(r.list("a.txt").is_err());
    }

    #[test]
    fn folder_romfs() {
        let d = std::env::temp_dir().join(format!("nxrs-romfs-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/f.dat"), b"abcdef").unwrap();
        std::fs::write(d.join("top"), b"T").unwrap();
        let r = RomFs::from_folder(&d).unwrap();
        assert_eq!(r.walk(), [("sub/f.dat".to_string(), 6), ("top".into(), 1)]);
        assert_eq!(r.read("sub/f.dat", 2, 2).unwrap(), b"cd");
        let _ = std::fs::remove_dir_all(&d);
    }
}
