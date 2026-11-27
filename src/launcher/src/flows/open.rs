//! Opening a game's folders and files with the system (`openFolder`, `openGameFile`, the console's Open
//! log).

use super::App;
use crate::constants::{words_contexts as c, words_play as w};
use crate::model::path_text::join;
use crate::services::{files, shell_open};

impl App {
    pub(super) fn open_folder(&mut self, saves: bool) {
        let Some(g) = self.selected_game() else {
            return;
        };
        let path = if saves {
            join(&g.data_dir(), "save")
        } else {
            g.build_dir.clone()
        };
        if files::is_dir(&path) {
            shell_open::open(&path);
        } else {
            let (context, text) = if saves {
                (c::SAVE_FOLDER, w::NO_SAVES)
            } else {
                (c::BUILD_FOLDER, w::NO_BUILD)
            };
            self.message(context, w::NO_FOLDER_TITLE, text, false);
        }
    }

    pub(super) fn open_game_log(&mut self) {
        let Some(g) = self.selected_game() else {
            return;
        };
        let path = join(&g.build_dir, &format!("{}.log", g.output_name));
        if files::exists(&path) {
            shell_open::open(&path);
        } else {
            self.message(c::GAME_LOG, w::NO_FILE_TITLE, w::NO_FILE, false);
        }
    }

    /// The building game's log, else the selected game's.
    pub(super) fn open_build_log(&mut self) {
        let id = self
            .build
            .as_ref()
            .map(|b| b.game.id.clone())
            .unwrap_or_else(|| self.selected.clone());
        if let Some(g) = self.lib.find(&id) {
            shell_open::open(&g.build_log());
        }
    }
}
