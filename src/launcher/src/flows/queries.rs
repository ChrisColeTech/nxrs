//! What the windows read about a game from the disk: its lists' rows, its version, its files. (The UI
//! asks here; it never reads the disk itself.)

use super::App;
use crate::model::game::Game;
use crate::model::path_text::native;
use crate::model::update_text;
use crate::services::{build_facts, dlc_files, files, update_files};

impl App {
    /// The DLC list's rows for `packages` (a Properties draft's, or the game's).
    pub fn dlc_rows(&self, game: &Game, packages: &[String]) -> Vec<(String, String)> {
        dlc_files::rows(game, packages, build_facts::has_build(game))
    }
    /// The update list's rows for a game (or a draft of it).
    pub fn update_rows(&self, game: &Game) -> Vec<(String, String)> {
        update_files::rows(game)
    }
    /// "The current build is …" when a build with another update is there.
    pub fn update_pending_text(&self, game: &Game) -> String {
        update_text::pending(game, build_facts::update_pending(game))
    }
    pub fn version_of(&self, game: &Game) -> String {
        if self.demo {
            String::new()
        } else {
            build_facts::version(game)
        }
    }
    pub fn exists(&self, path: &str) -> bool {
        files::exists(path)
    }
    pub fn has_log(&self) -> bool {
        self.selected_game().is_some_and(build_facts::has_log)
    }
    pub fn open_path(&self, path: &str) {
        crate::services::shell_open::open(path);
    }
    pub fn native(&self, path: &str) -> String {
        native(path)
    }
}
