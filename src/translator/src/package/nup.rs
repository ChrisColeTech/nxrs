//! NUP: a Switch 2 package format. One indexed file holding ExeFS and RomFS trees; raw entries are read
//! from their package ranges, codec-1 entries are one whole-file zstd frame.
//!
//! Layout: "NUP\x01", u32, u32 toc_at. At toc_at: "TOC\x01", u32 titles, u64 dirs, u64 files, u64 pool_at,
//! u64 pool_size (pool_at relative to toc_at); then title rows (0x40) from +0x30, dir rows (0x18), file rows (0x50),
//! u32 child indices (>= dirs means file index + dirs), then the UTF-8 name pool.
//! The RomFS is served in place as a file tree ([`NupReader`]): raw entries are ranged reads of the package,
//! codec-1 entries are decoded once and kept in a 256 MiB least-recently-used cache (as runtime/nup.c).
use crate::package::error::{fmt, Result};
use crate::package::io::{u32le, u64le, Blob};
use crate::package::{ExeFs, SourceKind};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use crate::package::romfs::{FileLoc, Node as RNode, NodeKind, RomFs};

pub const MAGIC: &[u8; 4] = b"NUP\x01";
const TOC_MAGIC: &[u8; 4] = b"TOC\x01";
const TOC_MAX: u64 = 256 << 20;
const TITLE: usize = 0x40;
const DIR: usize = 0x18;
const FILE: usize = 0x50;

#[derive(Debug, Clone)]
pub struct Title {
    pub title_id: u64,
    /// Second field of the title row; nup.py does not name it. Treated as the title version (0 for a v0 dump).
    pub version: u32,
    pub exefs_dir: u32,
    pub romfs_dir: u32,
}

#[derive(Debug, Clone)]
pub enum Node {
    Dir {
        name: String,
        #[allow(dead_code)]
        index: u32,
    },
    File(FileNode),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNode {
    /// Index of the file row (the cache key).
    pub index: u64,
    pub name: String,
    pub size: u64,
    pub offset: u64,
    pub stored: u64,
    pub codec: u8,
}

pub struct Toc {
    table: Vec<u8>,
    nd: u64,
    nf: u64,
    dirs_at: usize,
    files_at: usize,
    children_at: usize,
    pool_at: usize,
    package_size: u64,
    pub titles: Vec<Title>,
    /// The title nup.py selects: the last one that has both an ExeFS and a RomFS directory.
    pub title: usize,
}

impl Toc {
    pub fn open(blob: &Blob) -> Result<Toc> {
        let what = blob.path.display().to_string();
        let head = blob.read_at(0, 12)?;
        if &head[..4] != MAGIC {
            return fmt(format!("{what}: no NUP magic"));
        }
        let toc_at = u32le(&head, 8) as u64;
        if toc_at + 0x28 > blob.size {
            return fmt(format!("{what}: no NUP table of contents"));
        }
        let toc = blob.read_at(toc_at, 0x28)?;
        if &toc[..4] != TOC_MAGIC {
            return fmt(format!("{what}: no NUP table of contents"));
        }
        let (pool_at, pool_size) = (u64le(&toc, 0x18), u64le(&toc, 0x20));
        let size = pool_at.saturating_add(pool_size);
        if pool_at > TOC_MAX || pool_size > TOC_MAX || size > TOC_MAX || size > blob.size - toc_at {
            return fmt(format!("{what}: table of contents is out of range"));
        }
        Toc::parse(blob.read_at(toc_at, size)?, blob.size, &what)
    }

    /// Parse a whole table of contents (from "TOC\x01" to the end of the name pool).
    pub fn parse(table: Vec<u8>, package_size: u64, what: &str) -> Result<Toc> {
        if table.len() < 0x30 || &table[..4] != TOC_MAGIC {
            return fmt(format!("{what}: no NUP table of contents"));
        }
        let nt = u32le(&table, 4) as u64;
        let (nd, nf) = (u64le(&table, 8), u64le(&table, 0x10));
        let (pool_at, pool_size) = (u64le(&table, 0x18), u64le(&table, 0x20));
        if nt == 0 || nt > 4096 || nd == 0 || nd > 0xffff_ffff || nf > 0xffff_ffff
            || pool_at.saturating_add(pool_size) != table.len() as u64
        {
            return fmt(format!("{what}: table of contents is out of range"));
        }
        let dirs_at = 0x30 + nt as usize * TITLE;
        let files_at = dirs_at + nd as usize * DIR;
        let children_at = files_at + nf as usize * FILE;
        if children_at as u64 > pool_at {
            return fmt(format!("{what}: overlapping NUP tables"));
        }
        let titles: Vec<Title> = (0..nt as usize)
            .map(|i| {
                let at = 0x30 + i * TITLE;
                Title {
                    title_id: u64le(&table, at),
                    version: u32le(&table, at + 8),
                    exefs_dir: u32le(&table, at + 12),
                    romfs_dir: u32le(&table, at + 16),
                }
            })
            .collect();
        let title = match titles.iter().rposition(|t| (t.exefs_dir as u64) < nd && (t.romfs_dir as u64) < nd) {
            Some(i) => i,
            None => return fmt(format!("{what}: no title has both ExeFS and RomFS")),
        };
        Ok(Toc { table, nd, nf, dirs_at, files_at, children_at, pool_at: pool_at as usize, package_size, titles, title })
    }

    fn name(&self, offset: u32, length: u32) -> Result<String> {
        let pool = &self.table[self.pool_at..];
        let (o, l) = (offset as usize, length as usize);
        if l == 0 || o > pool.len() || l > pool.len() - o {
            return fmt("NUP: bad name in the table of contents");
        }
        let raw = &pool[o..o + l];
        if raw.contains(&b'/') || raw.contains(&0) || raw == b"." || raw == b".." {
            return fmt("NUP: bad name in the table of contents");
        }
        String::from_utf8(raw.to_vec()).map_err(|_| crate::package::error::PackageError::Format("NUP: non-UTF-8 name".into()))
    }

    /// The children of directory `dir`, sorted by name.
    pub fn children(&self, dir: u32) -> Result<Vec<Node>> {
        if dir as u64 >= self.nd {
            return fmt("NUP: bad directory index");
        }
        let at = self.dirs_at + dir as usize * DIR;
        let (first, count) = (u32le(&self.table, at + 8) as usize, u32le(&self.table, at + 12) as usize);
        let child_count = (self.pool_at - self.children_at) / 4;
        if first > child_count || count > child_count - first {
            return fmt("NUP: child list is out of range");
        }
        let mut out = Vec::with_capacity(count);
        for i in first..first + count {
            let source = u32le(&self.table, self.children_at + i * 4) as u64;
            if source >= self.nd {
                let item = source - self.nd;
                if item >= self.nf {
                    return fmt("NUP: child index is out of range");
                }
                let row = self.files_at + item as usize * FILE;
                let t = &self.table;
                let name = self.name(u32le(t, row), u32le(t, row + 4))?;
                let (size, offset, stored, codec) = (u64le(t, row + 8), u64le(t, row + 16), u64le(t, row + 24), t[row + 64]);
                if codec > 1 || (codec == 0 && stored != size) {
                    return fmt(format!("NUP: invalid codec or stored size for {name}"));
                }
                if offset > self.package_size || stored > self.package_size - offset {
                    return fmt(format!("NUP: {name} extends past the package"));
                }
                out.push(Node::File(FileNode { index: item, name, size, offset, stored, codec }));
            } else {
                let row = self.dirs_at + source as usize * DIR;
                let name = self.name(u32le(&self.table, row), u32le(&self.table, row + 4))?;
                out.push(Node::Dir { name, index: source as u32 });
            }
        }
        out.sort_by(|a, b| node_name(a).as_bytes().cmp(node_name(b).as_bytes()));
        if out.windows(2).any(|w| node_name(&w[0]) == node_name(&w[1])) {
            return fmt("NUP: duplicate directory entry");
        }
        Ok(out)
    }
}

fn node_name(n: &Node) -> &str {
    match n {
        Node::Dir { name, .. } => name,
        Node::File(f) => &f.name,
    }
}

/// Read a whole file entry: raw, or one zstd frame (window up to 2^31) that must expand to exactly `size` bytes.
pub fn read_file(blob: &Blob, f: &FileNode) -> Result<Vec<u8>> {
    if f.codec == 0 {
        return blob.read_at(f.offset, f.size);
    }
    let src = blob.reader(f.offset, f.stored)?;
    let err = |e: std::io::Error| crate::package::error::PackageError::Format(format!("NUP: {}: zstd: {e}", f.name));
    let mut dec = zstd::stream::read::Decoder::new(src).map_err(err)?;
    dec.window_log_max(31).map_err(err)?;
    let mut out = Vec::with_capacity(f.size as usize);
    dec.take(f.size + 1).read_to_end(&mut out).map_err(err)?;
    if out.len() as u64 != f.size {
        return fmt(format!("NUP: {} decompresses to {} of {} bytes", f.name, out.len(), f.size));
    }
    Ok(out)
}

impl Toc {
    /// nup.py `_tree`: the whole tree under `dir` (each file at most once, no cycles, depth <= 64).
    pub fn tree(&self, dir: u32, seen_dirs: &mut std::collections::HashSet<u32>, seen_files: &mut std::collections::HashSet<u64>, depth: u32) -> Result<Vec<RNode>> {
        if dir as u64 >= self.nd || !seen_dirs.insert(dir) || depth > 64 {
            return fmt("NUP: bad directory tree");
        }
        let mut kids = vec![];
        for node in self.children(dir)? {
            match node {
                Node::File(f) => {
                    if !seen_files.insert(f.index) {
                        return fmt("NUP: file appears more than once in a tree");
                    }
                    kids.push(RNode { name: f.name.clone(), kind: NodeKind::File { size: f.size, loc: FileLoc::Nup(f) } });
                }
                Node::Dir { name, index } => {
                    kids.push(RNode { name, kind: NodeKind::Dir(self.tree(index, seen_dirs, seen_files, depth + 1)?) })
                }
            }
        }
        Ok(kids)
    }
}

/// Cache budget for decoded zstd entries (runtime/nup.c keeps 256 MiB).
pub const CACHE_BYTES: u64 = 256 << 20;

/// Reads NUP file entries in place; decoded zstd entries are cached (LRU, [`CACHE_BYTES`]).
pub struct NupReader {
    pub blob: Blob,
    cache: Mutex<Vec<(u64, Arc<Vec<u8>>)>>,
}

impl NupReader {
    pub fn new(blob: Blob) -> NupReader {
        NupReader { blob, cache: Mutex::new(vec![]) }
    }

    /// `len` bytes of `f` from `offset` (the caller clamps to the file size).
    pub fn read(&self, f: &FileNode, offset: u64, len: u64) -> Result<Vec<u8>> {
        if f.codec == 0 {
            return self.blob.read_at(f.offset + offset, len);
        }
        let whole = {
            let mut c = self.cache.lock().unwrap();
            c.iter().position(|e| e.0 == f.index).map(|i| {
                let e = c.remove(i);
                c.push(e.clone());
                e.1
            })
        };
        let whole = match whole {
            Some(w) => w,
            None => {
                let w = Arc::new(read_file(&self.blob, f)?);
                if w.len() as u64 <= CACHE_BYTES {
                    let mut c = self.cache.lock().unwrap();
                    c.push((f.index, w.clone()));
                    while c.iter().map(|e| e.1.len() as u64).sum::<u64>() > CACHE_BYTES {
                        c.remove(0);
                    }
                }
                w
            }
        };
        Ok(whole[offset as usize..(offset + len) as usize].to_vec())
    }
}

/// The RomFS of the package's selected title, served in place.
pub fn romfs(blob: Blob) -> Result<RomFs> {
    let toc = Toc::open(&blob)?;
    let t = &toc.titles[toc.title];
    let kids = toc.tree(t.romfs_dir, &mut Default::default(), &mut Default::default(), 0)?;
    let source = format!("NUP {}", blob.path.display());
    Ok(RomFs::from_nup(Arc::new(NupReader::new(blob)), RNode { name: String::new(), kind: NodeKind::Dir(kids) }, source))
}

pub fn exefs(blob: &Blob) -> Result<ExeFs> {
    let toc = Toc::open(blob)?;
    let title = &toc.titles[toc.title];
    let mut files = BTreeMap::new();
    for node in toc.children(title.exefs_dir)? {
        if let Node::File(f) = node {
            files.insert(f.name.clone(), read_file(blob, &f)?);
        }
    }
    Ok(ExeFs { kind: SourceKind::Nup, title_id: Some(title.title_id), version: Some(title.version), files })
}

pub fn list(blob: &Blob) -> Result<Vec<String>> {
    let toc = Toc::open(blob)?;
    let mut out = vec![format!("NUP {} ({} bytes)", blob.path.display(), blob.size)];
    for (i, t) in toc.titles.iter().enumerate() {
        out.push(format!(
            "  title {:016x} v{} exefs dir {} romfs dir {}{}",
            t.title_id,
            t.version,
            t.exefs_dir,
            t.romfs_dir,
            if i == toc.title { " (selected)" } else { "" }
        ));
    }
    let t = &toc.titles[toc.title];
    for node in toc.children(t.exefs_dir)? {
        if let Node::File(f) = node {
            out.push(format!("  exefs/{:<16} {:>12} stored {:>12} codec {}", f.name, f.size, f.stored, f.codec));
        }
    }
    out.push(format!("  romfs/ {} top-level entries", toc.children(t.romfs_dir)?.len()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic NUP: title 0x0100..., dir 0 = exefs {main (raw), sdk (zstd)}, dir 1 = romfs {}.
    fn build() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let main = b"NSO0 raw main bytes".to_vec();
        let sdk: Vec<u8> = (0..5000u32).map(|i| (i % 13) as u8).collect();
        let sdk_z = zstd::encode_all(&sdk[..], 3).unwrap();
        let mut pkg = MAGIC.to_vec();
        pkg.extend_from_slice(&[0u8; 8]);
        let main_at = pkg.len() as u64;
        pkg.extend_from_slice(&main);
        let sdk_at = pkg.len() as u64;
        pkg.extend_from_slice(&sdk_z);
        let toc_at = pkg.len() as u32;
        pkg[8..12].copy_from_slice(&toc_at.to_le_bytes());

        let pool = b"exefsromfsmainsdk".to_vec();
        let (nt, nd, nf) = (1usize, 2usize, 2usize);
        let children = [2u32, 3u32]; // files 0,1 (index + nd)
        let pool_at = 0x30 + nt * TITLE + nd * DIR + nf * FILE + children.len() * 4;
        let mut toc = TOC_MAGIC.to_vec();
        toc.extend_from_slice(&(nt as u32).to_le_bytes());
        for v in [nd as u64, nf as u64, pool_at as u64, pool.len() as u64] {
            toc.extend_from_slice(&v.to_le_bytes());
        }
        toc.resize(0x30, 0);
        let mut title = vec![0u8; TITLE];
        title[..8].copy_from_slice(&0x0100_0000_0000_1000u64.to_le_bytes());
        title[8..12].copy_from_slice(&65536u32.to_le_bytes());
        title[12..16].copy_from_slice(&0u32.to_le_bytes());
        title[16..20].copy_from_slice(&1u32.to_le_bytes());
        toc.extend(title);
        for (name_off, name_len, first, count) in [(0u32, 5u32, 0u32, 2u32), (5, 5, 0, 2)] {
            for v in [name_off, name_len, first, count] {
                toc.extend_from_slice(&v.to_le_bytes());
            }
            toc.extend_from_slice(&[0u8; 8]);
        }
        for (name_off, name_len, size, offset, stored, codec) in [
            (10u32, 4u32, main.len() as u64, main_at, main.len() as u64, 0u8),
            (14, 3, sdk.len() as u64, sdk_at, sdk_z.len() as u64, 1),
        ] {
            let mut row = Vec::new();
            row.extend_from_slice(&name_off.to_le_bytes());
            row.extend_from_slice(&name_len.to_le_bytes());
            for v in [size, offset, stored] {
                row.extend_from_slice(&v.to_le_bytes());
            }
            row.extend_from_slice(&[0u8; 32]);
            row.push(codec);
            row.resize(FILE, 0);
            toc.extend(row);
        }
        for c in children {
            toc.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(toc.len(), pool_at);
        toc.extend_from_slice(&pool);
        pkg.extend(toc);
        (pkg, main, sdk)
    }

    #[test]
    fn parses_synthetic_nup() {
        let (pkg, main, sdk) = build();
        let dir = std::env::temp_dir().join(format!("nx-package-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.nup");
        std::fs::write(&path, &pkg).unwrap();
        let blob = Blob::open(&path).unwrap();
        let e = exefs(&blob).unwrap();
        assert_eq!(e.title_id, Some(0x0100_0000_0000_1000));
        assert_eq!(e.version, Some(65536));
        assert_eq!(e.files.keys().cloned().collect::<Vec<_>>(), vec!["main", "sdk"]);
        assert_eq!(e.files["main"], main);
        assert_eq!(e.files["sdk"], sdk);
        drop(blob);
        let r = romfs(Blob::open(&path).unwrap()).unwrap();
        assert_eq!(r.walk(), [("main".to_string(), main.len() as u64), ("sdk".into(), sdk.len() as u64)]);
        assert_eq!(r.read("sdk", 100, 50).unwrap(), sdk[100..150]);
        assert_eq!(r.read("sdk", 4990, 50).unwrap(), sdk[4990..]); // cached, clamped
        assert_eq!(r.read("main", 4, 3).unwrap(), main[4..7]);
        drop(r);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_bad_toc() {
        let (pkg, ..) = build();
        let toc_at = u32le(&pkg, 8) as usize;
        let mut table = pkg[toc_at..].to_vec();
        assert!(Toc::parse(table.clone(), pkg.len() as u64, "t").is_ok());
        table[0] = b'X';
        assert!(Toc::parse(table, pkg.len() as u64, "t").is_err());
    }
}
