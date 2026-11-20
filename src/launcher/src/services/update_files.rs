//! Update packages on disk: their signatures and the list's rows.

use super::files::{exists, is_file, read_at, size};
use crate::constants::{words_dlc as d, words_updates as w};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::game_update::GameUpdate;
use crate::model::path_text::{file_name, native, suffix};
use crate::model::update_text;

/// A type an update can be read from in place, with the right signature.
pub fn valid_package(path: &str) -> Result<(), String> {
    let name = file_name(path);
    if !is_file(path) {
        return Err(format!("{}{path}", d::NO_FILE));
    }
    let kind = suffix(path);
    if kind == "nsz" || kind == "xcz" {
        return Err(fill(w::COMPRESSED, &[&name]));
    }
    if !["nsp", "xci", "nca"].contains(&kind.as_str()) {
        return Err(fill(w::NOT_UPDATE, &[&name]));
    }
    let h = read_at(path, 0, 0x1104)?;
    let card = h.get(0x100..0x104) == Some(b"HEAD") || h.get(0x1100..0x1104) == Some(b"HEAD");
    match kind.as_str() {
        "nsp" if !h.starts_with(b"PFS0") => Err(fill(w::BAD_NSP, &[&name])),
        "xci" if !card => Err(fill(w::BAD_XCI, &[&name])),
        "nca" if size(path) < 0xC00 => Err(fill(d::SMALL_NCA, &[&name])),
        _ => Ok(()),
    }
}

pub fn description(game: &Game, update: &GameUpdate) -> String {
    update_text::description(
        game,
        update,
        exists(&update.path),
        is_file(&game.executable()),
    )
}

/// The list rows: (description, the package's native path).
pub fn rows(game: &Game) -> Vec<(String, String)> {
    game.updates
        .iter()
        .map(|u| (description(game, u), native(&u.path)))
        .collect()
}
