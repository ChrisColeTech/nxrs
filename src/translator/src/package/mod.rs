//! Package input, read in place and read-only: the game's code (ExeFS) for translation, and its RomFS, updates and
//! add-on content for the runtime's file system, with nothing extracted.
//!
//! # API
//!
//! | Call | What |
//! |---|---|
//! | [`open_game`] | a dumped folder, an `.nsp` (plain, encrypted, combined 1G+1U(+1D)), an `.xci` or a `.nup`, with an [`UpdateChoice`]: ExeFS, RomFS, version, control data, add-on hint |
//! | [`Game::write_data`] | stage a build's data folder: `exefs/`, the RomFS marker (`romfs.nsp` / `romfs.nup` / `romfs.dir`), `control.nacp` and icon |
//! | [`open_data_dir`] | what the runtime calls at start: the game's [`RomFs`] from its data folder's marker |
//! | [`RomFs`] | `list(dir)`, `walk()`, `find(path)`, `read(path, offset, len)`, `read_file(path)`, `image()`; `Send + Sync` |
//! | [`update_info`] | what an update package is (and, with the game, that it belongs to it): Turbo's `UPDATE\t...` line |
//! | [`dlc`] | `install`, `sync`, `remove`, `describe`, `read_manifest`, `title_romfs` (a title's RomFS, in place) |
//! | [`open_exefs`], [`list`] | the ExeFS alone (the translator's input), a debugging listing |

mod app;
mod bktr;
mod cnmt;
mod compressed;
mod container;
mod crypto;
pub mod dlc;
mod error;
mod folder;
mod io;
mod keys;
pub mod marker;
mod nca;
pub mod nro;
mod nsp;
mod nup;
mod pfs0;
pub mod romfs;
pub mod storage;
#[cfg(test)]
mod synthetic;

pub use app::{update_info, UpdateChoice, UpdateInfo};
pub use error::PackageError;
pub use keys::default_keys_path;
pub use marker::open_data_dir;
pub use romfs::{DirEntry, RomFs};

use error::{fmt, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Folder,
    Nsp,
    Nup,
    /// A homebrew NRO file.
    Nro,
}

/// A game's ExeFS: what the translator reads.
#[derive(Debug, Clone)]
pub struct ExeFs {
    pub kind: SourceKind,
    /// Program id (title id for a NUP), if known.
    pub title_id: Option<u64>,
    /// Title version if known (the update's when one is applied).
    pub version: Option<u32>,
    /// File name -> bytes.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl ExeFs {
    /// The NSO build id of `main` (bytes 0x40..0x60), trailing zero bytes trimmed.
    pub fn main_build_id(&self) -> Option<Vec<u8>> {
        nso_build_id(self.files.get("main")?)
    }
}

/// The build id of an NSO (bytes 0x40..0x60), trailing zero bytes trimmed.
pub fn nso_build_id(nso: &[u8]) -> Option<Vec<u8>> {
    if nso.len() < 0x60 || &nso[..4] != b"NSO0" {
        return None;
    }
    let id = &nso[0x40..0x60];
    let end = id.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
    Some(id[..end].to_vec())
}

/// Which update a build applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The package has no update (or the source cannot hold one).
    None,
    /// The update the game's own package holds (a combined NSP).
    Embedded,
    /// An update from a package of its own.
    Separate(PathBuf),
    /// `--no-update`: the update the package holds was left out.
    LeftOut,
}

/// Options for [`open_game`].
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// prod.keys; None => [`default_keys_path`] (only NSPs need keys).
    pub keys: Option<PathBuf>,
    pub update: UpdateChoice,
}

/// A game opened from its package: code, data, and what was applied.
pub struct Game {
    pub kind: SourceKind,
    /// The input, absolute.
    pub path: PathBuf,
    pub exefs: ExeFs,
    pub romfs: Arc<RomFs>,
    pub display_version: Option<String>,
    /// control.nacp and the icons (packages read with keys).
    pub control: BTreeMap<String, Vec<u8>>,
    /// Add-on content titles the game's package also holds (install them with [`dlc::install`]).
    pub addons: Vec<u64>,
    pub applied: Applied,
    /// The RomFS marker for the data folder: (file name, text).
    pub marker: (&'static str, String),
}

enum Detected {
    Folder,
    Nsp,
    Nup,
    Nro,
}

fn detect(input: &Path) -> Result<Detected> {
    if input.is_dir() {
        return Ok(Detected::Folder);
    }
    let blob = io::Blob::open(input)?;
    if blob.size < 4 {
        return Err(PackageError::Unrecognised(input.to_path_buf()));
    }
    match &blob.read_at(0, 4)?[..] {
        b"PFS0" => Ok(Detected::Nsp),
        m if m == nup::MAGIC => Ok(Detected::Nup),
        _ if blob.size >= 0x80 && blob.read_at(0x10, 4).is_ok_and(|m| m == b"NRO0") => Ok(Detected::Nro),
        // a card image (XCI), with or without the 0x1000-byte key area in front: its secure partition's NCAs are read
        // in place like an NSP's (container::members)
        _ if [0x100u64, 0x1100].iter().any(|&at| blob.size >= at + 4 && blob.read_at(at, 4).is_ok_and(|m| m == b"HEAD")) => Ok(Detected::Nsp),
        _ => Err(PackageError::Unrecognised(input.to_path_buf())),
    }
}

/// Open a game: a dumped folder, an NSP, an XCI (card image) or a NUP, applying the update `opts.update` chooses.
/// The program indices a package holds (a multi-program application started program by program through
/// oe::ExecuteProgram: Sonic X Shadow Generations, Super Mario 3D All-Stars); `[0]` for a folder, NRO or NUP.
pub fn program_indices(input: &Path, keys_file: Option<&Path>) -> Result<Vec<u64>> {
    let path = io::abspath(input);
    if !matches!(detect(&path)?, Detected::Nsp) {
        return Ok(vec![0]);
    }
    let keys = keys::Keys::load(&keys::keys_path(keys_file)?)?;
    app::program_indices(&path, &keys)
}

pub fn open_game(input: &Path, opts: &OpenOptions) -> Result<Game> {
    let path = io::abspath(input);
    let detected = detect(&path)?;
    if let UpdateChoice::Package(u) = &opts.update {
        if !matches!(detected, Detected::Nsp) {
            let what = if matches!(detected, Detected::Nup) {
                "a NUP package, which holds its update already"
            } else {
                "an extracted folder or another kind of file"
            };
            return fmt(format!(
                "{}: a separate update needs the game as an NSP package; this game is built from {what}",
                io::abspath(u).display()
            ));
        }
    }
    match detected {
        Detected::Folder => {
            let exefs = folder::exefs(&path)?;
            // the RomFS is optional (a game without data, homebrew): Turbo builds such a folder with an empty one
            let (romfs, marker) = match folder::find_romfs(&path) {
                Some(d) => (RomFs::from_folder(&d)?, (marker::DIR, format!("{}\n", d.display()))),
                None => (RomFs::empty(format!("{} (no romfs/)", path.display())), (marker::NONE, String::new())),
            };
            Ok(Game {
                kind: SourceKind::Folder,
                path: path.clone(),
                exefs,
                romfs: Arc::new(romfs),
                display_version: None,
                control: folder::control(&path),
                addons: vec![],
                applied: Applied::None,
                marker,
            })
        }
        Detected::Nro => {
            // homebrew: the NRO is `main`; its asset section (after the image) may hold an icon, a control.nacp and
            // a RomFS, which is read in place (a byte range of the file, as an NSP's plaintext RomFS)
            let d = std::fs::read(&path).map_err(|source| PackageError::Io { path: path.clone(), source })?;
            let size = u32::from_le_bytes(d[0x18..0x1c].try_into().unwrap()) as usize;
            if size < 0x80 || size > d.len() {
                return fmt(format!("{}: the NRO is cut short", path.display()));
            }
            let asset = |which: usize| -> Option<(u64, u64)> {
                let a = d.get(size..)?;
                if a.len() < 0x38 || &a[..4] != b"ASET" {
                    return None;
                }
                let at = 8 + which * 0x10;
                let (o, n) = (u64::from_le_bytes(a[at..at + 8].try_into().unwrap()), u64::from_le_bytes(a[at + 8..at + 16].try_into().unwrap()));
                (n > 0 && o + n <= a.len() as u64).then_some((size as u64 + o, n))
            };
            let mut control = BTreeMap::new();
            for (which, name) in [(0, "icon_AmericanEnglish.dat"), (1, "control.nacp")] {
                if let Some((o, n)) = asset(which) {
                    control.insert(name.to_string(), d[o as usize..(o + n) as usize].to_vec());
                }
            }
            let (romfs, marker) = match asset(2) {
                Some((o, n)) => {
                    let blob = Arc::new(io::Blob::open(&path)?);
                    let image = storage::FileRange { blob, offset: o, size: n };
                    (RomFs::from_image(Arc::new(image), format!("NRO {}", path.display()))?, (marker::NSP, format!("SWRNSP1\n{}\n{o:x}\n{n:x}\n", path.display())))
                }
                None => (RomFs::empty(format!("NRO {}", path.display())), (marker::NONE, String::new())),
            };
            let mut files = BTreeMap::new();
            files.insert("main".to_string(), d[..size].to_vec());
            Ok(Game {
                kind: SourceKind::Nro,
                path,
                exefs: ExeFs { kind: SourceKind::Nro, title_id: None, version: None, files },
                romfs: Arc::new(romfs),
                display_version: None,
                control,
                addons: vec![],
                applied: Applied::None,
                marker,
            })
        }
        Detected::Nup => {
            let blob = io::Blob::open(&path)?;
            let exefs = nup::exefs(&blob)?;
            let romfs = nup::romfs(blob)?;
            Ok(Game {
                kind: SourceKind::Nup,
                marker: (marker::NUP, format!("{}\n", path.display())),
                path,
                exefs,
                romfs: Arc::new(romfs),
                display_version: None,
                control: BTreeMap::new(),
                addons: vec![],
                applied: Applied::None,
            })
        }
        Detected::Nsp => {
            let keys = keys::Keys::load(&keys::keys_path(opts.keys.as_deref())?)?;
            let pkg = app::Package::open(&path, &keys, &opts.update)?;
            let romfs = pkg.romfs()?;
            let applied = match (&pkg.update, &pkg.update_path) {
                (Some(_), Some(p)) => Applied::Separate(p.clone()),
                (Some(_), None) => Applied::Embedded,
                (None, _) if pkg.embedded_update.is_some() => Applied::LeftOut,
                _ => Applied::None,
            };
            Ok(Game {
                kind: SourceKind::Nsp,
                exefs: ExeFs { kind: SourceKind::Nsp, title_id: Some(pkg.title_id), version: Some(pkg.version), files: pkg.exefs()? },
                romfs: Arc::new(romfs),
                display_version: pkg.display_version.clone(),
                control: pkg.control.clone(),
                addons: pkg.addons.clone(),
                applied,
                marker: (marker::NSP, pkg.marker()),
                path: pkg.path.clone(),
            })
        }
    }
}

/// An ExeFS name that is safe as a file in the data folder on Windows (switchrecomp.py:440): no separators or drive
/// colon, not `.` / `..`, no trailing dot or space (Windows drops them), and not a reserved device name (CON, PRN,
/// AUX, NUL, COM1-9, LPT1-9, with or without an extension).
pub fn safe_exefs_name(name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." || name != name.trim_end_matches([' ', '.']) {
        return false;
    }
    let base = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered = |p: &str| base.len() == 4 && base.starts_with(p) && matches!(base.as_bytes()[3], b'1'..=b'9');
    !(matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT"))
}

impl Game {
    /// Stage the data folder a build hands the runtime (switchrecomp.py `copy_data`): `exefs/` (modules only the
    /// other version had are removed), the RomFS marker (the other markers removed), `control.nacp` and the icon.
    /// Says what was applied, in Turbo's words.
    pub fn write_data(&self, datadir: &Path, say: &dyn Fn(&str)) -> Result<()> {
        let io_err = |p: &Path| {
            let p = p.to_path_buf();
            move |source| PackageError::Io { path: p.clone(), source }
        };
        let ex = datadir.join("exefs");
        std::fs::create_dir_all(&ex).map_err(io_err(&ex))?;
        for (name, data) in &self.exefs.files {
            if !safe_exefs_name(name) {
                return fmt(format!("unsafe ExeFS name in {}: {name:?}", self.path.display()));
            }
            let dst = ex.join(name);
            // ZBIC segments unpacked: the runtime's loader reads LZ4 only
            let data = crate::loader::nso::for_runtime(data).or_else(|e| fmt(format!("{}: exefs/{name}: {e}", self.path.display())))?;
            if std::fs::read(&dst).ok().as_deref() != Some(&data[..]) {
                std::fs::write(&dst, &data[..]).map_err(io_err(&dst))?;
            }
        }
        for e in std::fs::read_dir(&ex).map_err(io_err(&ex))?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !self.exefs.files.contains_key(&n) && e.path().is_file() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        for other in [marker::NSP, marker::NUP, marker::DIR, "romfs.bin"] {
            if other != self.marker.0 {
                let _ = std::fs::remove_file(datadir.join(other));
            }
        }
        if self.marker.0 == marker::NONE {
            // no RomFS: the runtime finds the empty romfs/ folder
            let r = datadir.join("romfs");
            std::fs::create_dir_all(&r).map_err(io_err(&r))?;
        } else {
            let m = datadir.join(self.marker.0);
            std::fs::write(&m, &self.marker.1).map_err(io_err(&m))?;
        }
        let icons: Vec<&String> = self.control.keys().filter(|n| n.starts_with("icon_")).collect();
        let icon = self.control.get("icon_AmericanEnglish.dat").or_else(|| icons.first().map(|n| &self.control[*n]));
        for (name, blob) in [("control.nacp", self.control.get("control.nacp")), ("icon_AmericanEnglish.dat", icon)] {
            if let Some(b) = blob {
                let p = datadir.join(name);
                std::fs::write(&p, b).map_err(io_err(&p))?;
            }
        }
        // the offline HTML documents (nn::web's offline pages open them in the PC's browser)
        let html = datadir.join("html-document");
        let _ = std::fs::remove_dir_all(&html);
        let mut docs = 0;
        for (name, b) in self.control.iter().filter(|(k, _)| k.starts_with("html-document/")) {
            if name.split('/').any(|c| c == ".." || c.is_empty()) || name.contains(':') || name.contains('\\') {
                continue;
            }
            let p = datadir.join(name);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(io_err(d))?;
            }
            std::fs::write(&p, b).map_err(io_err(&p))?;
            docs += 1;
        }
        if docs > 0 {
            say(&format!("Manual: {docs} offline HTML files staged in html-document/"));
        }
        let v = self.exefs.version.unwrap_or(0);
        let display = self.display_version.clone().unwrap_or_default();
        match &self.applied {
            Applied::Separate(u) => say(&format!("Package: game with its update {display} (v{v}), applied in place from {}", u.display())),
            Applied::Embedded => say(&format!("Package: game with its update {display} (v{v}), applied in place")),
            Applied::LeftOut => say(&format!("Package: the game without the update this package holds: {display} (v{v})")),
            Applied::None => {}
        }
        if self.marker.0 == marker::NONE {
            say(&format!("RomFS: {} has none", self.path.display()));
        } else {
            say(&format!("RomFS: using {} in place", self.path.display()));
        }
        if !self.addons.is_empty() {
            let ids: Vec<String> = self.addons.iter().map(|t| format!("{t:016x}")).collect();
            say(&format!(
                "DLC: this package also holds add-on content {}; install it with nxrs dlc install (the launcher does)",
                ids.join(" ")
            ));
        }
        Ok(())
    }
}

/// Read the ExeFS of a folder / NSP / NUP (the game with the update its own package holds).
pub fn open_exefs(input: &Path, keys: Option<&Path>) -> Result<ExeFs> {
    let opts = OpenOptions { keys: keys.map(Path::to_path_buf), update: UpdateChoice::Embedded };
    Ok(open_game(input, &opts)?.exefs)
}

/// Human-readable listing of the package entries (names, sizes, content types) for debugging.
pub fn list(input: &Path, keys: Option<&Path>) -> Result<Vec<String>> {
    let path = io::abspath(input);
    if path.is_dir() {
        return folder::list(&path);
    }
    let blob = io::Blob::open(&path)?;
    if blob.size >= 4 && &blob.read_at(0, 4)?[..] == nup::MAGIC {
        return nup::list(&blob);
    }
    let keys = keys::Keys::load(&keys::keys_path(keys)?)?;
    nsp::list(Arc::new(blob), &keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_trims_trailing_zeros() {
        let mut nso = vec![0u8; 0x100];
        nso[..4].copy_from_slice(b"NSO0");
        nso[0x40..0x48].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0, 1, 0, 0]);
        assert_eq!(nso_build_id(&nso).unwrap(), vec![0xde, 0xad, 0xbe, 0xef, 0, 1]);
        assert!(nso_build_id(b"NSO0").is_none());
    }

    /// switchrecomp.py:440: names Windows cannot hold as files are refused, ordinary module names pass.
    #[test]
    fn reserved_windows_names_refused() {
        for ok in ["main", "rtld", "sdk", "subsdk0", "main.npdm", "COM10", "console", "nul0", "LPT0"] {
            assert!(safe_exefs_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "a/b", "a\\b", "c:x", "CON", "con", "Nul.txt", "aux.npdm", "PRN", "COM1", "lpt9", "main.", "main "] {
            assert!(!safe_exefs_name(bad), "{bad:?}");
        }
    }
}
