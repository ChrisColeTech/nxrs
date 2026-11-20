//! An application package read in place with the user's keys: the game's Program NCA, the update of the same
//! package (a combined 1G+1U(+1D) NSP) or of a package of its own (NSP, XCI, or a bare NCA with its ticket beside
//! it), the ExeFS (the update's replaces the game's), the RomFS (the update's BKTR patch composed over the game's
//! section per read), version, control data, and the `romfs.nsp` marker the runtime reads.
use crate::package::bktr::Patched;
use crate::package::cnmt::{self, is_addon_of};
use crate::package::compressed::{self, Compressed};
use crate::package::error::{fmt, PackageError, Result};
use crate::package::io::{abspath, u32le, u64le, Blob};
use crate::package::keys::Keys;
use crate::package::nca::{open_ncas, Nca, Section, SectionStorage};
use crate::package::romfs::{image_tree, NodeKind, RomFs};
use crate::package::storage::{Slice, Storage};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Which update to apply to a game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UpdateChoice {
    /// The update its own package holds, if any (Turbo's default).
    #[default]
    Embedded,
    /// `--no-update`: the game as it was released, without the update its package holds.
    None,
    /// `--update <package>`: an update in a package of its own; one the game's package holds is left out.
    Package(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ExeFsNode {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

/// The ROMFS storage: a plain section or a BKTR-patched one, either possibly under a compression layer.
pub enum RomStorage {
    Plain(SectionStorage),
    Patched(Patched),
    Compressed(Compressed),
}

impl Storage for RomStorage {
    fn size(&self) -> u64 {
        match self {
            RomStorage::Plain(s) => s.size(),
            RomStorage::Patched(p) => p.size(),
            RomStorage::Compressed(c) => c.size(),
        }
    }
    fn read(&self, offset: u64, size: u64) -> Result<Vec<u8>> {
        match self {
            RomStorage::Plain(s) => s.read(offset, size),
            RomStorage::Patched(p) => p.read(offset, size),
            RomStorage::Compressed(c) => c.read(offset, size),
        }
    }
}

pub struct Package {
    pub path: PathBuf,
    pub keys_path: PathBuf,
    #[allow(dead_code)]
    pub ncas: Vec<Arc<Nca>>,
    pub title_id: u64,
    pub base: Option<Arc<Nca>>,
    pub embedded_update: Option<Arc<Nca>>,
    /// The update applied (the embedded one, a separate package's, or none).
    pub update: Option<Arc<Nca>>,
    /// The separate update package, when one is applied.
    pub update_path: Option<PathBuf>,
    #[allow(dead_code)]
    pub update_ncas: Vec<Arc<Nca>>,
    pub program: Arc<Nca>,
    pub encrypted: bool,
    /// The ExeFS section (decrypted, decompressed).
    pub exefs_section: Arc<dyn Storage>,
    pub exefs_nodes: Vec<ExeFsNode>,
    pub storage: Arc<RomStorage>,
    /// Compression layers (data level offset, table offset, table size): of the game's RomFS section under a patch,
    /// and of the section read last (the patched one, or the game's own when no update applies). The marker's
    /// `compressed` rows.
    pub base_compressed: Option<(u64, u64, u64)>,
    pub top_compressed: Option<(u64, u64, u64)>,
    pub romfs_at: u64,
    pub romfs_size: u64,
    pub version: u32,
    pub addons: Vec<u64>,
    pub display_version: Option<String>,
    pub control: BTreeMap<String, Vec<u8>>,
}

fn is_patch(n: &Nca) -> bool {
    n.is_patch()
}

pub(crate) fn pfs_files(nca: &Nca, s: &Section) -> BTreeMap<String, Vec<u8>> {
    // nca.py Package._files: {name: bytes} of a small PartitionFS section (content metadata)
    let (at, size) = (u64le(&s.fs, 0x40), u64le(&s.fs, 0x48));
    let mut files = BTreeMap::new();
    if size > 16 << 20 || at > s.size || size > s.size - at {
        return files;
    }
    let Ok(image) = nca.read(s, at, size, None) else { return files };
    if image.len() < 0x10 {
        return files;
    }
    let (count, strings_size) = (u32le(&image, 4) as usize, u32le(&image, 8) as usize);
    let data_at = 0x10 + count * 0x18 + strings_size;
    if &image[..4] != b"PFS0" || count > 1000 || data_at as u64 > size {
        return files;
    }
    let strings = &image[data_at - strings_size..data_at];
    for i in 0..count {
        let e = 0x10 + i * 0x18;
        let (rel, len, name_at) = (u64le(&image, e) as usize, u64le(&image, e + 8) as usize, u32le(&image, e + 16) as usize);
        let end = strings.get(name_at..).and_then(|x| x.iter().position(|&b| b == 0)).map(|p| name_at + p);
        let Some(end) = end else { return BTreeMap::new() };
        if (data_at + rel + len) as u64 > size {
            return BTreeMap::new();
        }
        files.insert(String::from_utf8_lossy(&strings[name_at..end]).into_owned(), image[data_at + rel..data_at + rel + len].to_vec());
    }
    files
}

/// (title version, add-on content title ids, name of the Control NCA to use) from the content metadata of `ncas`.
fn metadata(ncas: &[Arc<Nca>], title_id: u64, updated: bool) -> (u32, Vec<u64>, Option<String>) {
    let mut version = 0;
    let mut addons = vec![];
    let mut control: BTreeMap<u8, String> = BTreeMap::new();
    let want = if updated { cnmt::PATCH } else { cnmt::APPLICATION };
    for nca in ncas {
        if nca.content_type != 1 || nca.sections.is_empty() {
            continue;
        }
        let files = pfs_files(nca, &nca.sections[0]);
        for (name, data) in &files {
            if !name.to_ascii_lowercase().ends_with(".cnmt") {
                continue;
            }
            let Ok(c) = cnmt::parse(data) else { break };
            if c.kind == cnmt::ADD_ON && is_addon_of(c.title_id, title_id) {
                addons.push(c.title_id);
            }
            if c.kind != want || cnmt::base_of(c.title_id) != cnmt::base_of(title_id) {
                continue;
            }
            version = version.max(c.version);
            for r in &c.records {
                if r.kind == 3 {
                    control.insert(c.kind, format!("{}.nca", r.nca_id));
                }
            }
        }
    }
    addons.sort();
    (version, addons, control.get(&want).cloned())
}

/// The game's offline HTML documents (its Manual NCA's `html-document/` tree, which nn::web's offline pages open),
/// keyed `html-document/<path>`; at most 64 MB. Empty when the package has none.
pub(crate) fn manual_files(ncas: &[Arc<Nca>], title_id: u64) -> BTreeMap<String, Vec<u8>> {
    fn walk(n: &crate::package::romfs::Node, dir: &str, image: &[u8], out: &mut BTreeMap<String, Vec<u8>>) {
        for k in n.children() {
            let path = if dir.is_empty() { k.name.clone() } else { format!("{dir}/{}", k.name) };
            match &k.kind {
                NodeKind::Dir(_) => walk(k, &path, image, out),
                NodeKind::File { size, loc: crate::package::romfs::FileLoc::Image(o) } => {
                    if let Some(b) = image.get(*o as usize..(*o + *size) as usize) {
                        out.insert(path, b.to_vec());
                    }
                }
                _ => {}
            }
        }
    }
    for nca in ncas {
        if nca.content_type != 3 || cnmt::base_of(nca.title_id) != cnmt::base_of(title_id) {
            continue;
        }
        let Some(si) = nca.section(3, 0) else { continue };
        let s = &nca.sections[si];
        if s.patch || s.sparse {
            continue;
        }
        let files = (|| -> Result<BTreeMap<String, Vec<u8>>> {
            let view = section_data(nca, si, &nca.blob.path.display().to_string())?;
            let (at, mut size) = nca.ivfc_data(s)?;
            if s.compressed {
                size = view.size().saturating_sub(at);
            }
            if size > 64 << 20 || at > view.size() || size > view.size() - at {
                return fmt("skip");
            }
            let image = view.read(at, size)?;
            let read = |o: u64, n: u64| image.get(o as usize..(o + n) as usize).map(|x| x.to_vec()).ok_or_else(|| PackageError::Format("cut".into()));
            let root = image_tree(&nca.blob.path.display().to_string(), size, &read)?;
            let mut out = BTreeMap::new();
            walk(&root, "", &image, &mut out);
            Ok(out)
        })();
        let Ok(files) = files else { continue };
        let html: BTreeMap<String, Vec<u8>> = files.into_iter().filter(|(k, _)| k.starts_with("html-document/")).collect();
        if !html.is_empty() {
            return html;
        }
    }
    BTreeMap::new()
}

/// The Control NCA's files (control.nacp, icons): the one `wanted` names, else the first of the title.
fn control_files(ncas: &[Arc<Nca>], title_id: u64, wanted: Option<&str>) -> (BTreeMap<String, Vec<u8>>, Option<String>) {
    let mut found: Option<BTreeMap<String, Vec<u8>>> = None;
    for nca in ncas {
        if nca.content_type != 2 || cnmt::base_of(nca.title_id) != cnmt::base_of(title_id) {
            continue;
        }
        let Some(si) = nca.section(3, 0) else { continue };
        let s = &nca.sections[si];
        if s.patch || s.sparse {
            continue;
        }
        let files = (|| -> Result<BTreeMap<String, Vec<u8>>> {
            let view = section_data(nca, si, &nca.blob.path.display().to_string())?;
            let (at, mut size) = nca.ivfc_data(s)?;
            if s.compressed {
                size = view.size().saturating_sub(at);
            }
            if size > 64 << 20 || at > view.size() || size > view.size() - at {
                return fmt("skip");
            }
            let image = view.read(at, size)?;
            let read = |o: u64, n: u64| {
                image.get(o as usize..(o + n) as usize).map(|x| x.to_vec()).ok_or_else(|| PackageError::Format("cut".into()))
            };
            let root = image_tree(&nca.blob.path.display().to_string(), size, &read)?;
            let mut files = BTreeMap::new();
            for k in root.children() {
                if let NodeKind::File { size, loc: crate::package::romfs::FileLoc::Image(o) } = &k.kind {
                    files.insert(k.name.clone(), image[*o as usize..(*o + *size) as usize].to_vec());
                }
            }
            Ok(files)
        })();
        let Ok(files) = files else { continue };
        if files.get("control.nacp").map_or(0, |v| v.len()) < 0x3070 {
            continue;
        }
        if found.is_none() || nca.name.to_ascii_lowercase() == wanted.unwrap_or("").to_ascii_lowercase() {
            found = Some(files);
        }
    }
    let Some(found) = found else { return (BTreeMap::new(), None) };
    let nacp = &found["control.nacp"][0x3060..0x3070];
    let end = nacp.iter().position(|&b| b == 0).unwrap_or(nacp.len());
    let display = String::from_utf8_lossy(&nacp[..end]).into_owned();
    (found, Some(display))
}

impl Package {
    /// nca.py `Package(path, keys, update, embedded)`.
    pub fn open(path: &Path, keys: &Keys, choice: &UpdateChoice) -> Result<Package> {
        let path = abspath(path);
        let p = path.display().to_string();
        let blob = Arc::new(Blob::open(&path)?);
        let (ncas, _) = open_ncas(&blob, keys)?;
        let programs: Vec<Arc<Nca>> = ncas.iter().filter(|n| n.content_type == 0).cloned().collect();
        if programs.is_empty() {
            return fmt(format!("{p}: no decryptable Program NCA; check the installed keys"));
        }
        // a multi-program application (programs at title id + index, started by oe::ExecuteProgram): program 0
        // unless `NXRS_PROGRAM_INDEX` (the cli's `--program`) names another
        let index = std::env::var("NXRS_PROGRAM_INDEX").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
        let title_id = programs.iter().map(|n| n.title_id).min().unwrap() + index;
        if !programs.iter().any(|n| n.title_id == title_id) {
            return fmt(format!("{p}: no program {index} (title {title_id:016x}) in the package"));
        }
        let mut programs: Vec<Arc<Nca>> = programs.into_iter().filter(|n| n.title_id == title_id).collect();
        let patches: Vec<Arc<Nca>> = programs.iter().filter(|n| is_patch(n)).cloned().collect();
        let bases: Vec<Arc<Nca>> = programs.iter().filter(|n| !is_patch(n)).cloned().collect();
        if patches.len() > 1 || bases.len() > 1 {
            return fmt(format!("{p}: the package holds several versions of the game; keep one game and one update"));
        }
        let base = bases.first().cloned();
        let embedded_update = patches.first().cloned();
        let mut update_path = None;
        let mut update_ncas = ncas.clone();
        let alone = || format!("{p}: this package is an update on its own; the game's package is needed as the source");
        let update = match choice {
            UpdateChoice::Package(u) => {
                if base.is_none() {
                    return fmt(alone());
                }
                let up = abspath(u);
                let ublob = Arc::new(Blob::open(&up)?);
                let (uncas, _) = open_ncas(&ublob, keys)?;
                let theirs: Vec<Arc<Nca>> = uncas.iter().filter(|n| n.content_type == 0 && is_patch(n)).cloned().collect();
                let mine: Vec<Arc<Nca>> = theirs.iter().filter(|n| n.title_id == title_id).cloned().collect();
                if mine.is_empty() {
                    if let Some(t) = theirs.first() {
                        return fmt(format!(
                            "{}: this update is for {:016x}, not for this game ({title_id:016x})",
                            up.display(),
                            t.title_id
                        ));
                    }
                    return fmt(format!("{}: the package holds no game update", up.display()));
                }
                if mine.len() > 1 {
                    return fmt(format!("{}: the package holds several updates; keep one", up.display()));
                }
                update_path = Some(up);
                update_ncas = uncas;
                programs = vec![base.clone().unwrap(), mine[0].clone()];
                Some(mine[0].clone())
            }
            UpdateChoice::Embedded => embedded_update.clone(),
            UpdateChoice::None => {
                if base.is_none() {
                    return fmt(alone());
                }
                programs = vec![base.clone().unwrap()];
                None
            }
        };
        let program = update.clone().or_else(|| base.clone()).unwrap();
        let encrypted = programs.iter().any(|n| n.sections.iter().any(|s| s.crypto != 1));
        for n in &programs {
            for s in &n.sections {
                if s.sparse {
                    return fmt(format!("{p}: sparse storage in the Program NCA is not supported"));
                }
                if ![1, 3, 4].contains(&s.crypto) {
                    return fmt(format!(
                        "{p}: the Program NCA uses an encryption the reader does not support (type {})",
                        s.crypto
                    ));
                }
            }
        }
        let mut exefs_section = None;
        let mut exefs_nodes = vec![];
        for nca in [&base, &update].into_iter().flatten() {
            let Some(si) = nca.section(2, 1) else { return fmt(format!("{p}: the Program NCA has no ExeFS")) };
            let code = &nca.sections[si];
            let (at, size) = (u64le(&code.fs, 0x40), u64le(&code.fs, 0x48));
            if at > code.size || size > code.size - at {
                return fmt(format!("{p}: ExeFS range is out of bounds"));
            }
            let st = section_data(nca, si, &p)?;
            // a compressed data level is its decompressed size
            let size = if compressed::table_of(&code.fs).is_some() { st.size().saturating_sub(at) } else { size };
            if at > st.size() || size > st.size() - at {
                return fmt(format!("{p}: ExeFS range is out of bounds"));
            }
            exefs_nodes = Self::exefs_of(&p, nca, &*st, at, size)?;
            exefs_section = Some(st);
        }
        let Some(di) = program.section(3, 0) else { return fmt(format!("{p}: the Program NCA has no RomFS")) };
        let data = &program.sections[di];
        let base_section = base.as_ref().and_then(|b| b.section(3, 0).map(|i| (b, i)));
        // A patch with a compression table of its own relocates onto the base's stored (compressed) bytes and is
        // decompressed on top (Jamboree 1.1.0 over its card image: the IVFC hashes match only that way); a patch
        // without one reads the base decompressed.
        let patch_compressed = data.patch && compressed::table_of(&data.fs).is_some();
        let base_data: Option<Arc<dyn Storage>> = match base_section {
            Some((b, i)) if patch_compressed => Some(Arc::new(SectionStorage::new(b, i))),
            Some((b, i)) => Some(section_data(b, i, &p)?),
            None => None,
        };
        let base_compressed = match base_section {
            Some((b, i)) if data.patch && !patch_compressed => {
                compressed::over(Arc::new(SectionStorage::new(b, i)), &b.sections[i].fs, &p)?.map(|c| c.1)
            }
            _ => None,
        };
        let (storage, top_compressed) = if data.patch {
            let patched = Patched::new(SectionStorage::new(&program, di), base_data)?;
            if compressed::table_of(&data.fs).is_some() {
                let fs = data.fs.clone();
                let (c, t) = compressed::over(Arc::new(patched), &fs, &p)?.unwrap();
                (RomStorage::Compressed(c), Some(t))
            } else {
                (RomStorage::Patched(patched), None)
            }
        } else {
            match compressed::over(Arc::new(SectionStorage::new(&program, di)), &data.fs, &p)? {
                Some((c, t)) => (RomStorage::Compressed(c), Some(t)),
                None => (RomStorage::Plain(SectionStorage::new(&program, di)), None),
            }
        };
        let (romfs_at, mut romfs_size) = program.ivfc_data(data)?;
        if top_compressed.is_some() {
            romfs_size = storage.size().saturating_sub(romfs_at);
        }
        if romfs_at > storage.size() || romfs_size > storage.size() - romfs_at {
            return fmt(format!("{p}: RomFS range is out of bounds"));
        }
        let meta_ncas = if update.is_some() { &update_ncas } else { &ncas };
        let (version, _, wanted) = metadata(meta_ncas, title_id, update.is_some());
        let (_, addons, base_wanted) = metadata(&ncas, title_id, false);
        let (mut control, mut display_version) = control_files(meta_ncas, title_id, wanted.as_deref());
        if control.is_empty() && update_path.is_some() {
            (control, display_version) = control_files(&ncas, title_id, base_wanted.as_deref());
        }
        // the offline HTML documents ride with the control files (staged under html-document/)
        let mut manual = manual_files(meta_ncas, title_id);
        if manual.is_empty() {
            manual = manual_files(&ncas, title_id);
        }
        control.extend(manual);
        Ok(Package {
            keys_path: keys.path.clone(),
            path,
            ncas,
            title_id,
            base,
            embedded_update,
            update,
            update_path,
            update_ncas,
            program,
            encrypted,
            exefs_section: exefs_section.unwrap(),
            exefs_nodes,
            storage: Arc::new(storage),
            base_compressed,
            top_compressed,
            romfs_at,
            romfs_size,
            version,
            addons,
            display_version,
            control,
        })
    }

    fn exefs_of(p: &str, nca: &Nca, s: &dyn Storage, at: u64, size: u64) -> Result<Vec<ExeFsNode>> {
        let head = s.read(at, 0x10)?;
        let (count, strings_size) = (u32le(&head, 4) as u64, u32le(&head, 8) as u64);
        let table_size = 0x10 + count * 0x18 + strings_size;
        if &head[..4] != b"PFS0" || count > 1000 || table_size > size {
            let kek = nca.wrapped.as_deref().unwrap_or("").split(':').next().unwrap_or("").to_string();
            return fmt(format!(
                "{p}: the ExeFS did not decrypt; the installed keys probably lack this game's generation ({kek})"
            ));
        }
        let table = s.read(at, table_size)?;
        let strings = &table[(0x10 + count * 0x18) as usize..];
        let mut nodes = vec![];
        for i in 0..count as usize {
            let e = 0x10 + i * 0x18;
            let (rel, len, name_at) = (u64le(&table, e), u64le(&table, e + 8), u32le(&table, e + 16) as usize);
            let end = strings.get(name_at..).and_then(|x| x.iter().position(|&b| b == 0)).map(|q| name_at + q);
            let room = size - table_size;
            let Some(end) = end.filter(|_| rel <= room && len <= room - rel) else {
                return fmt(format!("{p}: ExeFS entry is out of range"));
            };
            nodes.push(ExeFsNode {
                name: String::from_utf8_lossy(&strings[name_at..end]).into_owned(),
                offset: at + table_size + rel,
                size: len,
            });
        }
        Ok(nodes)
    }

    /// The ExeFS files (name -> bytes): the update's when one is applied.
    pub fn exefs(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        self.exefs_nodes.iter().map(|n| Ok((n.name.clone(), self.exefs_section.read(n.offset, n.size)?))).collect()
    }

    /// The RomFS image (IVFC level 6 of the possibly patched section) as a storage.
    pub fn romfs_image(&self) -> Arc<dyn Storage> {
        Arc::new(Slice { inner: self.storage.clone(), offset: self.romfs_at, size: self.romfs_size })
    }

    pub fn romfs(&self) -> Result<RomFs> {
        RomFs::from_image(self.romfs_image(), self.path.display().to_string())
    }

    /// The runtime marker `romfs.nsp`. Version 2 when a section is encrypted or patched (keys named, content keys
    /// still wrapped as the package holds them); version 1 (path and byte range) for a plaintext package.
    pub fn marker(&self) -> String {
        let p = self.path.display();
        match &*self.storage {
            RomStorage::Plain(s) if !self.encrypted && self.top_compressed.is_none() => {
                format!("SWRNSP1\n{p}\n{:x}\n{:x}\n", s.file_offset + self.romfs_at, self.romfs_size)
            }
            storage => {
                let mut lines = vec![
                    "SWRNSP2".to_string(),
                    p.to_string(),
                    format!("keys {}", self.keys_path.display()),
                    format!("romfs {:x} {:x}", self.romfs_at, self.romfs_size),
                ];
                let patched = self.program.sections[self.program.section(3, 0).unwrap()].patch;
                match (storage, patched) {
                    (_, true) => {
                        if let Some(b) = &self.base {
                            if let Some(i) = b.section(3, 0) {
                                if self.base.is_some() {
                                    lines.push(format!("base {}", b.layer(&b.sections[i])));
                                    if let Some((h, a, n)) = self.base_compressed {
                                        lines.push(format!("compressed base {h:x} {a:x} {n:x}"));
                                    }
                                }
                            }
                        }
                        if let Some(u) = &self.update_path {
                            lines.push(format!("update {}", u.display()));
                        }
                        let fs = &self.program.sections[self.program.section(3, 0).unwrap()].fs;
                        let (a, b, c, d) = (u64le(fs, 0x100), u64le(fs, 0x108), u64le(fs, 0x120), u64le(fs, 0x128));
                        lines.push(format!("patch {} {a:x} {b:x} {c:x} {d:x}", self.program.layer(&self.program.sections[self.program.section(3, 0).unwrap()])));
                    }
                    (_, false) => {
                        lines.push(format!("base {}", self.program.layer(&self.program.sections[self.program.section(3, 0).unwrap()])))
                    }
                }
                if let Some((h, a, n)) = self.top_compressed {
                    lines.push(format!("compressed {h:x} {a:x} {n:x}"));
                }
                lines.join("\n") + "\n"
            }
        }
    }
}

/// What an update package is (nca.py `update_info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub path: PathBuf,
    pub title_id: u64,
    pub version: u32,
    pub display: String,
}

impl UpdateInfo {
    /// The launcher's line: `UPDATE<tab>path<tab>title id<tab>version<tab>display version`.
    pub fn line(&self) -> String {
        format!("UPDATE\t{}\t{:016x}\t{}\t{}", self.path.display(), self.title_id, self.version, self.display)
    }
}

/// An NSP (PFS0) or a card image (XCI: `HEAD` at 0x100, or at 0x1100 behind the key area).
fn is_nsp(p: &Path) -> bool {
    let Ok(b) = Blob::open(p) else { return false };
    b.read_at(0, 4).is_ok_and(|m| m == b"PFS0")
        || [0x100u64, 0x1100].iter().any(|&at| b.size >= at + 4 && b.read_at(at, 4).is_ok_and(|m| m == b"HEAD"))
}

/// An NCA section as a storage: decrypted, and decompressed when the FS header has a compression table.
fn section_data(nca: &Arc<Nca>, index: usize, what: &str) -> Result<Arc<dyn Storage>> {
    let plain = Arc::new(SectionStorage::new(nca, index));
    if nca.sections[index].patch {
        return Ok(plain);
    }
    Ok(match compressed::over(plain.clone(), &nca.sections[index].fs, what)? {
        Some((c, _)) => Arc::new(c),
        None => plain,
    })
}

/// nca.py `update_info`: what an update package is. With `game` (the game's own package) the update is opened over
/// it, which proves that it belongs to that game and that the keys read it.
pub fn update_info(path: &Path, keys: Option<&Path>, game: Option<&Path>) -> Result<UpdateInfo> {
    let path = abspath(path);
    if !path.is_file() {
        return fmt(format!("{}: the update package is missing", path.display()));
    }
    if let Some(game) = game {
        if !(game.is_file() && is_nsp(game)) {
            let what = if game.to_string_lossy().to_lowercase().ends_with(".nup") {
                "a NUP package, which holds its update already"
            } else {
                "an extracted folder or another kind of file"
            };
            return fmt(format!(
                "{}: a separate update needs the game as an NSP package; this game is built from {what}",
                path.display()
            ));
        }
        let keys = Keys::load(&crate::package::keys::keys_path(keys)?)?;
        let package = Package::open(game, &keys, &UpdateChoice::Package(path.clone()))?;
        package.romfs()?; // nca.py builds the RomFS tree in Package(): a broken patch is refused here too
        return Ok(UpdateInfo {
            path,
            title_id: package.title_id,
            version: package.version,
            display: package.display_version.unwrap_or_default(),
        });
    }
    let keys = Keys::load(&crate::package::keys::keys_path(keys)?)?;
    let blob = Arc::new(Blob::open(&path)?);
    let (ncas, _) = open_ncas(&blob, &keys)?;
    let Some(patch) = ncas.iter().find(|n| n.content_type == 0 && n.is_patch()) else {
        return fmt(format!("{}: the package holds no game update", path.display()));
    };
    let title_id = patch.title_id;
    let (version, _, wanted) = metadata(&ncas, title_id, true);
    let (_, display) = control_files(&ncas, title_id, wanted.as_deref());
    Ok(UpdateInfo { path, title_id, version, display: display.unwrap_or_default() })
}
