//! The RomFS markers a build writes into the game's data folder, and opening a RomFS from them (what the runtime
//! does at start). Formats (Turbo's, unchanged; docs/COMBINED_NSP.md §3, UPDATES.md §4, NUP_IN_PLACE.md):
//!
//! ```text
//! romfs.nsp  SWRNSP1                     a plaintext RomFS: the package and the image's byte range
//!            <package path>
//!            <offset hex>
//!            <size hex>
//!
//! romfs.nsp  SWRNSP2                     an encrypted and/or update-patched RomFS
//!            <the game's package>
//!            keys <prod.keys>
//!            romfs <offset of the RomFS image in the (virtual) section> <size>
//!            base <file offset> <size> <offset in NCA> <crypto 1|3> <counter> <key name>:<wrapped key>|-
//!            update <the update's package>                (only for an update in a package of its own)
//!            patch <file offset> <size> <offset in NCA> 4 <counter> <key name>:<wrapped key> <indirect offset>
//!                  <size> <AesCtrEx offset> <size>        (one line)
//!
//! romfs.nup  <package path>              a NUP's RomFS tree, read in place
//! romfs.dir  <folder path>               (this engine) a dumped folder's romfs/, read in place
//! ```
//! Content keys stay wrapped as the package holds them; they are unwrapped with prod.keys when the marker is opened
//! (the explicit keys, else SWR_PROD_KEYS, else the marker's `keys` path). No key is written by the build.
use crate::package::bktr::Patched;
use crate::package::crypto::ecb_decrypt;
use crate::package::error::{fmt, PackageError, Result};
use crate::package::io::Blob;
use crate::package::keys::Keys;
use crate::package::nca::SectionStorage;
use crate::package::romfs::RomFs;
use crate::package::storage::{FileRange, Slice, Storage};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const NSP: &str = "romfs.nsp";
pub const NUP: &str = "romfs.nup";
pub const DIR: &str = "romfs.dir";

fn hexnum(s: &str, what: &str) -> Result<u64> {
    u64::from_str_radix(s, 16).map_err(|_| PackageError::Format(format!("{what}: bad number {s:?} in the marker")))
}

fn unhex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let v: Option<Vec<u8>> = (0..16).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect();
    v.map(|v| v.try_into().unwrap())
}

/// A `base` / `patch` row: file offset, size, NCA offset, crypto, counter, key.
fn section_row(fields: &[&str], blob: &Arc<Blob>, keys: Option<&Keys>, what: &str) -> Result<SectionStorage> {
    if fields.len() < 6 {
        return fmt(format!("{what}: a section row of the marker is cut short"));
    }
    let crypto: u8 = fields[3].parse().map_err(|_| PackageError::Format(format!("{what}: bad crypto in the marker")))?;
    let counter = u64::from_str_radix(fields[4], 16).map_err(|_| PackageError::Format(format!("{what}: bad counter")))?;
    let key = if fields[5] == "-" {
        None
    } else {
        let (name, wrapped) = fields[5]
            .split_once(':')
            .ok_or_else(|| PackageError::Format(format!("{what}: bad key in the marker")))?;
        let wrapped = unhex16(wrapped).ok_or_else(|| PackageError::Format(format!("{what}: bad key in the marker")))?;
        let keys = keys.ok_or_else(|| PackageError::Format(format!("{what}: no keys")))?;
        Some(ecb_decrypt(&keys.get16(name)?, &wrapped)[..].try_into().unwrap())
    };
    Ok(SectionStorage {
        blob: blob.clone(),
        file_offset: hexnum(fields[0], what)?,
        size: hexnum(fields[1], what)?,
        nca_offset: hexnum(fields[2], what)?,
        crypto,
        upper: counter.to_be_bytes(),
        key,
        fs: vec![],
    })
}

/// Open the RomFS a `romfs.nsp` marker (version 1 or 2) describes.
pub fn open_nsp_marker(marker: &Path, keys: Option<&Path>) -> Result<RomFs> {
    let text = std::fs::read_to_string(marker).map_err(|source| PackageError::Io { path: marker.to_path_buf(), source })?;
    let lines: Vec<&str> = text.lines().collect();
    let what = marker.display().to_string();
    let package = PathBuf::from(*lines.get(1).ok_or_else(|| PackageError::Format(format!("{what}: empty marker")))?);
    let open = |p: &Path| -> Result<Arc<Blob>> {
        Blob::open(p).map(Arc::new).map_err(|_| {
            PackageError::Format(format!("{}: the package is missing (moved or its drive is offline)", p.display()))
        })
    };
    match lines[0] {
        "SWRNSP1" => {
            if lines.len() < 4 {
                return fmt(format!("{what}: marker is cut short"));
            }
            let blob = open(&package)?;
            let image = FileRange { blob, offset: hexnum(lines[2], &what)?, size: hexnum(lines[3], &what)? };
            RomFs::from_image(Arc::new(image), format!("NSP {}", package.display()))
        }
        "SWRNSP2" => {
            let mut keys_file: Option<PathBuf> = None;
            let mut romfs = None;
            let mut base_row = None;
            let mut patch_row = None;
            let mut update = None;
            for l in &lines[2..] {
                if let Some(r) = l.strip_prefix("keys ") {
                    keys_file = Some(PathBuf::from(r));
                } else if let Some(r) = l.strip_prefix("update ") {
                    update = Some(PathBuf::from(r));
                } else if let Some(r) = l.strip_prefix("romfs ") {
                    let f: Vec<&str> = r.split_whitespace().collect();
                    if f.len() != 2 {
                        return fmt(format!("{what}: bad romfs row"));
                    }
                    romfs = Some((hexnum(f[0], &what)?, hexnum(f[1], &what)?));
                } else if let Some(r) = l.strip_prefix("base ") {
                    base_row = Some(r.split_whitespace().collect::<Vec<_>>());
                } else if let Some(r) = l.strip_prefix("patch ") {
                    patch_row = Some(r.split_whitespace().collect::<Vec<_>>());
                }
            }
            let Some((at, size)) = romfs else { return fmt(format!("{what}: the marker has no romfs row")) };
            let keys_loader = || -> Result<Keys> {
                let p = keys
                    .map(Path::to_path_buf)
                    .or_else(|| std::env::var_os("SWR_PROD_KEYS").filter(|v| !v.is_empty()).map(PathBuf::from))
                    .or(keys_file.clone());
                match p {
                    Some(p) if p.is_file() => Keys::load(&p),
                    _ => fmt("Nintendo keys are not installed. In the launcher use Keys > Install keys, or set SWR_PROD_KEYS."),
                }
            };
            let needs_key = [&base_row, &patch_row].iter().any(|r| r.as_ref().is_some_and(|r| r.get(5).is_some_and(|k| *k != "-")));
            let loaded = if needs_key { Some(keys_loader()?) } else { None };
            let keys_once = loaded.as_ref();
            let game = open(&package)?;
            let base = match &base_row {
                Some(r) => Some(Arc::new(section_row(r, &game, keys_once, &what)?) as Arc<dyn Storage>),
                None => None,
            };
            let storage: Arc<dyn Storage> = match &patch_row {
                Some(r) => {
                    if r.len() < 10 {
                        return fmt(format!("{what}: the patch row is cut short"));
                    }
                    let pblob = match &update {
                        Some(u) => open(u).map_err(|_| {
                            PackageError::Format(format!(
                                "the update package {} is missing (moved or its drive is offline): remove the update \
                                 in the launcher (Game > Install Updates) and build the game again",
                                u.display()
                            ))
                        })?,
                        None => game.clone(),
                    };
                    let patch = section_row(r, &pblob, keys_once, &what)?;
                    let t = (hexnum(r[6], &what)?, hexnum(r[7], &what)?, hexnum(r[8], &what)?, hexnum(r[9], &what)?);
                    Arc::new(Patched::from_tables(patch, base, t)?)
                }
                None => base.ok_or_else(|| PackageError::Format(format!("{what}: the marker has no base row")))?,
            };
            if at > storage.size() || size > storage.size() - at {
                return fmt(format!("{what}: RomFS range is out of bounds"));
            }
            let source = match &update {
                Some(u) => format!("NSP {} with the update {}", package.display(), u.display()),
                None => format!("NSP {}", package.display()),
            };
            RomFs::from_image(Arc::new(Slice { inner: storage, offset: at, size }), source)
        }
        other => fmt(format!("{what}: unknown marker {other:?}")),
    }
}

/// Open the game's RomFS from its data folder, in the order the runtime checks (docs/NUP_IN_PLACE.md):
/// `romfs.nup`, `romfs.nsp`, `romfs.dir`, `romfs/`.
pub fn open_data_dir(datadir: &Path, keys: Option<&Path>) -> Result<RomFs> {
    let nup = datadir.join(NUP);
    if let Ok(t) = std::fs::read_to_string(&nup) {
        let p = PathBuf::from(t.lines().next().unwrap_or("").trim());
        return crate::package::nup::romfs(Blob::open(&p)?);
    }
    let nsp = datadir.join(NSP);
    if nsp.is_file() {
        return open_nsp_marker(&nsp, keys);
    }
    if let Ok(t) = std::fs::read_to_string(datadir.join(DIR)) {
        return RomFs::from_folder(Path::new(t.lines().next().unwrap_or("").trim()));
    }
    RomFs::from_folder(&datadir.join("romfs"))
}
