//! What a build folder says about itself: the package its data was made from (`romfs.source`, or the
//! in-place marker `romfs.nup` / `romfs.nsp` of a build made before the build carried its data), the
//! update applied (`SWRNSP2` `update` row), its cover.

use super::files::{is_dir, is_file, read_text};
use crate::constants::engine as e;
use crate::model::game::Game;
use crate::model::game_update::GameUpdate;
use crate::model::path_text::{from_native, join, same_path};

/// The marker text the build left: (the marker's file name, its text). `romfs.source` names the marker
/// it replaced in its `marker` row.
fn record(data_dir: &str) -> Option<(String, String)> {
    if let Some(text) = read_text(&join(data_dir, e::ROMFS_SOURCE)) {
        let kind = text
            .split('\n')
            .find_map(|row| row.trim().strip_prefix("marker ").map(str::to_string))
            .unwrap_or_default();
        return Some((kind, text));
    }
    [e::ROMFS_NUP, e::ROMFS_NSP]
        .iter()
        .find_map(|m| read_text(&join(data_dir, m)).map(|t| (m.to_string(), t)))
}

/// The package a build reads its data from (empty: none, or a folder build).
pub fn source_of_build(data_dir: &str) -> String {
    let Some((kind, text)) = record(data_dir) else {
        return String::new();
    };
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or("").trim();
    // SWRNSP2: an encrypted package, or one with the game and its update; line 2 is the package in both
    if first == "SWRNSP1" || first == "SWRNSP2" {
        lines.next().unwrap_or("").trim().to_string()
    } else if kind == e::ROMFS_NUP {
        first.to_string()
    } else {
        String::new()
    }
}

/// The build reads its data from its source while it plays (an in-place marker: a build made before
/// the build carried its data). A build with its own copy plays with the source gone.
pub fn reads_in_place(data_dir: &str) -> bool {
    [e::ROMFS_NUP, e::ROMFS_NSP, e::ROMFS_DIR]
        .iter()
        .any(|m| is_file(&join(data_dir, m)))
}

/// The build was made with an update of its own package (its record names it) but the entry does
/// not say so: the update joins the list and counts as built. True when the game changed.
pub fn adopt_built_update(game: &mut Game) -> bool {
    if !game.built_update.is_empty() || game.build_dir.is_empty() || game.output_name.is_empty() {
        return false;
    }
    let Some((_, text)) = record(&game.data_dir()) else {
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
