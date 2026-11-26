//! Where a game's build goes (owner, 2026-11-26): `<source folder>\nxrs\<name>`, or `<Build location>\nxrs\<name>`
//! when Settings > Build location is set; and which older folders move there (`<source folder>\SwitchRecomp
//! Builds\<name> [nxrs]`, the layout before).

use super::path_text::{file_name, from_native, join, parent};
use crate::constants::engine as e;

fn trimmed(path: &str) -> String {
    let mut p = from_native(path);
    while p.len() > 1 && p.ends_with('/') && !p.ends_with(":/") {
        p.pop();
    }
    p
}

/// The build folder of a new entry: `<base>/nxrs/<output name>`, base = the Build location, else the
/// source's folder.
pub fn build_dir_for(source: &str, output_root: &str, output_name: &str) -> String {
    let base = if output_root.trim().is_empty() {
        parent(&trimmed(source))
    } else {
        trimmed(output_root)
    };
    join(&join(&base, e::BUILDS_FOLDER), output_name)
}

/// A folder in the layout before: `<base>/SwitchRecomp Builds/<name> [nxrs]`. Its place now:
/// `<base>/nxrs/<name>`.
pub fn migrated_build_dir(build_dir: &str) -> Option<String> {
    let dir = trimmed(build_dir);
    let name = file_name(&dir).strip_suffix(e::LEGACY_SUFFIX)?.to_string();
    let holder = parent(&dir);
    if name.is_empty() || !file_name(&holder).eq_ignore_ascii_case(e::LEGACY_BUILDS) {
        return None;
    }
    Some(join(&join(&parent(&holder), e::BUILDS_FOLDER), &name))
}

/// An entry saved with another engine (or none) builds with nxrs into a folder of its own, never over the
/// other engine's build: `<base>/nxrs/<name>`, base = the folder that holds its build folder (above
/// `SwitchRecomp Builds`, the old default). A folder already there keeps its place.
pub fn nxrs_folder_for(build_dir: &str) -> String {
    if let Some(moved) = migrated_build_dir(build_dir) {
        return moved;
    }
    let dir = trimmed(build_dir);
    let holder = parent(&dir);
    if file_name(&holder) == e::BUILDS_FOLDER {
        return dir;
    }
    let base = if file_name(&holder).eq_ignore_ascii_case(e::LEGACY_BUILDS) {
        parent(&holder)
    } else {
        holder
    };
    let name = file_name(&dir);
    let name = name.strip_suffix(e::LEGACY_SUFFIX).unwrap_or(&name);
    join(&join(&base, e::BUILDS_FOLDER), name)
}
