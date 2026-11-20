//! Add-on content on disk: the data folder's `dlc.manifest`, the packages' signatures.

use super::files::{exists, is_file, read_at, read_text, size};
use crate::constants::{engine as e, words_dlc as w};
use crate::model::dlc_title::{self, DlcTitle};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::path_text::{file_name, join, suffix};

/// The titles installed in a built game's data folder; none without a manifest.
pub fn installed(data_dir: &str) -> Vec<DlcTitle> {
    if data_dir.is_empty() {
        return Vec::new();
    }
    read_text(&join(data_dir, e::DLC_MANIFEST))
        .map(|t| dlc_title::parse_manifest(&t))
        .unwrap_or_default()
}

fn has(bytes: &[u8], i: usize, what: &[u8]) -> bool {
    bytes.get(i..i + what.len()) == Some(what)
}

/// One of the package types the engine installs, with the right signature.
pub fn valid_package(path: &str) -> Result<(), String> {
    let name = file_name(path);
    if !is_file(path) {
        return Err(format!("{}{path}", w::NO_FILE));
    }
    let kind = suffix(path);
    if !["nsp", "nsz", "xci", "xcz", "nca"].contains(&kind.as_str()) {
        return Err(fill(w::NOT_PACKAGE, &[&name]));
    }
    let h = read_at(path, 0, 0x1104)?;
    let card = has(&h, 0x100, b"HEAD") || has(&h, 0x1100, b"HEAD");
    match kind.as_str() {
        "nsp" | "nsz" if !h.starts_with(b"PFS0") => Err(fill(w::BAD_NSP, &[&name])),
        "xci" | "xcz" if !card => Err(fill(w::BAD_XCI, &[&name])),
        "nca" if size(path) < 0xC00 => Err(fill(w::SMALL_NCA, &[&name])),
        _ => Ok(()),
    }
}

pub fn up_to_date(game: &Game) -> bool {
    dlc_title::up_to_date(&game.dlc, &installed(&game.data_dir()))
}

/// Installed but unlisted packages join the list (installed from the command line, or by a launcher
/// whose library file was overwritten since). True when it changed.
pub fn adopt_installed(game: &mut Game) -> bool {
    let titles = installed(&game.data_dir());
    dlc_title::adopt(&mut game.dlc, &titles)
}

/// The list rows: (description, the package's native path).
pub fn rows(game: &Game, packages: &[String], built: bool) -> Vec<(String, String)> {
    let titles = installed(&game.data_dir());
    packages
        .iter()
        .map(|p| {
            (
                dlc_title::description(p, &titles, built, exists(p)),
                crate::model::path_text::native(p),
            )
        })
        .collect()
}
