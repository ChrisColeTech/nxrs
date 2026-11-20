//! Adding to the library: a new entry, an imported build, every build in a folder (`Library::add`,
//! `importBuild`, `discoverBuilds`).

use super::build_facts::has_build;
use super::files::{is_dir, list};
use super::sources::game_from_executable;
use crate::constants::engine as e;
use crate::model::game::Game;
use crate::model::library::Library;
use crate::model::path_text::{from_native, join, same_path};

/// Refuses a duplicate source or executable.
pub fn add(lib: &mut Library, game: Game) -> bool {
    let duplicate = lib.games.iter().any(|g| {
        (!game.source.is_empty() && same_path(&g.source, &game.source))
            || same_path(&g.executable(), &game.executable())
    });
    if !duplicate {
        lib.games.push(game);
    }
    !duplicate
}

/// The same executable again changes nothing; an unbuilt entry of the same source takes the build
/// (keeping its title, cover and options); otherwise the build is a new entry. Returns its id.
pub fn import_build(lib: &mut Library, game: &Game) -> String {
    if let Some(g) = lib
        .games
        .iter()
        .find(|g| same_path(&g.executable(), &game.executable()))
    {
        return g.id.clone();
    }
    let unbuilt = lib
        .games
        .iter_mut()
        .find(|g| !game.source.is_empty() && same_path(&g.source, &game.source) && !has_build(g));
    let Some(g) = unbuilt else {
        lib.games.push(game.clone());
        return game.id.clone();
    };
    g.build_dir = game.build_dir.clone();
    g.output_name = game.output_name.clone();
    g.built_source = game.built_source.clone();
    g.build_incomplete = false;
    g.renderer = game.renderer.clone();
    g.engine = game.engine.clone();
    g.built_update = game.built_update.clone();
    for u in &game.updates {
        if !g.updates.iter().any(|h| same_path(&h.path, &u.path)) {
            g.updates.push(u.clone());
        }
    }
    if g.cover.is_empty() {
        g.cover = game.cover.clone();
    }
    g.id.clone()
}

/// Imports every native build in a folder and its subfolders; returns how many.
pub fn discover_builds(lib: &mut Library, folder: &str) -> usize {
    if !is_dir(folder) {
        return 0;
    }
    let mut folders = vec![from_native(folder)];
    folders.extend(
        list(folder)
            .into_iter()
            .filter(|(_, dir)| *dir)
            .map(|(name, _)| join(folder, &name)),
    );
    let mut count = 0;
    for dir in folders {
        for (name, is_dir) in list(&dir) {
            let Some(stem) = name.strip_suffix(e::MODULE_SUFFIX).filter(|_| !is_dir) else {
                continue;
            };
            let Ok(game) =
                game_from_executable(&join(&dir, &format!("{stem}{}", e::EXECUTABLE_SUFFIX)))
            else {
                continue;
            };
            if !lib
                .games
                .iter()
                .any(|g| same_path(&g.executable(), &game.executable()))
            {
                import_build(lib, &game);
                count += 1;
            }
        }
    }
    count
}
