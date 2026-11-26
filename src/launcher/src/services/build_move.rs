//! The build-folder migration (owner, 2026-11-26): entries in the layout before
//! (`<source folder>\SwitchRecomp Builds\<name> [nxrs]`) move to `<source folder>\nxrs\<name>`, a rename
//! on the same drive, and their `buildDir` follows. A running game's folder, a folder in use, and one
//! whose new place is taken are left and reported. Other engines' folders are never touched; the old
//! `SwitchRecomp Builds` folder goes only when nothing is left in it.

use super::files::exists;
use crate::constants::words_library as w;
use crate::model::build_layout::migrated_build_dir;
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::library::Library;
use crate::model::path_text::{native, parent};
use std::collections::{BTreeMap, BTreeSet};

/// What the migration did, one line an entry (or old folder), and whether the library changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Migration {
    pub lines: Vec<String>,
    pub changed: bool,
    /// entries left in the old layout (running, in use, the new place taken, an error)
    pub skipped: usize,
}

/// A running game holds its executable and module open: they cannot be opened for writing.
fn in_use(game: &Game) -> bool {
    [game.executable(), game.code_module()]
        .iter()
        .any(|p| exists(p) && std::fs::OpenOptions::new().append(true).open(p).is_err())
}

/// Why this entry's folder cannot move now; None: it can.
fn blocked(game: &Game, to: &str, running: &BTreeSet<String>) -> Option<String> {
    if running.contains(&game.id) {
        return Some(w::MOVE_RUNNING.into());
    }
    if exists(to) {
        return Some(fill(w::MOVE_TAKEN, &[&native(to)]));
    }
    in_use(game).then(|| w::MOVE_IN_USE.into())
}

/// Moves every entry in the layout before (`only`: the entry with this title alone); `running`: the
/// ids of games running or building now.
pub fn migrate(lib: &mut Library, running: &BTreeSet<String>, only: Option<&str>) -> Migration {
    let mut m = Migration::default();
    let mut holders = BTreeMap::new();
    for game in &mut lib.games {
        let Some(to) = migrated_build_dir(&game.build_dir) else {
            continue;
        };
        if only.is_some_and(|t| t != game.title) {
            continue;
        }
        let from = game.build_dir.clone();
        let say = |what: &str| fill(what, &[&game.title, &native(&from), &native(&to)]);
        if !exists(&from) {
            // nothing built there: the entry only points at the new place
            m.lines.push(say(w::MOVE_NOTHING));
            game.build_dir = to;
            m.changed = true;
            continue;
        }
        let outcome = match blocked(game, &to, running) {
            Some(why) => Err(why),
            None => std::fs::create_dir_all(parent(&to))
                .and_then(|()| std::fs::rename(&from, &to))
                .map_err(|e| e.to_string()),
        };
        match outcome {
            Ok(()) => {
                m.lines.push(say(w::MOVED));
                holders.insert(parent(&from), ());
                game.build_dir = to;
                m.changed = true;
            }
            Err(why) => {
                m.lines.push(format!("{}{why}", say(w::MOVE_SKIPPED)));
                m.skipped += 1;
            }
        }
    }
    for holder in holders.keys() {
        // only an empty folder can be removed: anything else in it (another engine's build) keeps it
        let line = if std::fs::remove_dir(holder).is_ok() {
            w::HOLDER_REMOVED
        } else {
            w::HOLDER_LEFT
        };
        m.lines.push(fill(line, &[&native(holder)]));
    }
    m
}
