//! What a build folder says about itself: the package its data comes from (`romfs.nup` / `romfs.nsp`),
//! the update applied (`SWRNSP2` `update` row), its cover.

use super::files::{is_dir, is_file, read_text};
use crate::constants::engine as e;
use crate::model::game::Game;
use crate::model::game_update::GameUpdate;
use crate::model::path_text::{from_native, join, same_path};

/// The package a build reads its data from (empty: none, or a folder build).
pub fn source_of_build(data_dir: &str) -> String {
    if let Some(text) = read_text(&join(data_dir, e::ROMFS_NUP)) {
        let line = text.split('\n').next().unwrap_or("").trim().to_string();
        if !line.is_empty() {
            return line;
        }
    }
    let Some(text) = read_text(&join(data_dir, e::ROMFS_NSP)) else {
        return String::new();
    };
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or("").trim();
    // SWRNSP2: an encrypted package, or one with the game and its update; line 2 is the package in both
    if first == "SWRNSP1" || first == "SWRNSP2" {
        lines.next().unwrap_or("").trim().to_string()
    } else {
        String::new()
    }
}

/// The build was made with an update of its own package (its romfs.nsp names it) but the entry does
/// not say so: the update joins the list and counts as built. True when the game changed.
pub fn adopt_built_update(game: &mut Game) -> bool {
    if !game.built_update.is_empty() || game.build_dir.is_empty() || game.output_name.is_empty() {
        return false;
    }
    let Some(text) = read_text(&join(&game.data_dir(), e::ROMFS_NSP)) else {
        return false;
    };
    let mut lines = text.split('\n');
    if lines.next().unwrap_or("").trim() != "SWRNSP2" {
        return false;
    }
    let Some(rest) = lines.find_map(|row| row.trim().strip_prefix("update ").map(str::to_string))
    else {
        return false;
    };
    let path = from_native(rest.trim());
    if !game.updates.iter().any(|u| same_path(&u.path, &path)) {
        let display = super::build_facts::version(game);
        game.updates.push(GameUpdate {
            path: path.clone(),
            display,
            ..GameUpdate::default()
        });
    }
    game.built_update = path;
    true
}

/// A cover in a build's data folder (`icon.jpg`, `icon.png`, `icon_AmericanEnglish.dat`).
pub fn build_cover(data_dir: &str) -> String {
    e::BUILD_ICONS
        .iter()
        .map(|n| join(data_dir, n))
        .find(|p| is_file(p))
        .unwrap_or_default()
}

/// A package that holds the game with its DLC: the first build from it puts it on the DLC list (a
/// rebuild does not put back a package the user removed).
pub fn adopt_package_dlc(game: &mut Game, built_before: &str) -> bool {
    if game.source.is_empty()
        || same_path(built_before, &game.source)
        || game.dlc.iter().any(|p| same_path(p, &game.source))
    {
        return false;
    }
    game.dlc
        .push(crate::model::path_text::absolute(&game.source));
    true
}

/// The data folder is there (DLC goes into it).
pub fn has_data(game: &Game) -> bool {
    is_dir(&game.data_dir())
}
