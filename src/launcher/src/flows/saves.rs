//! Remove from library, Install keys (`removeGame`, `installKeys`).

use super::surface::Check;
use super::{Action, App};
use crate::constants::{engine as e, words_library as l};
use crate::model::fill::fill;
use crate::model::path_text::{join, native, parent};
use crate::services::pickers::{self, Pick};
use crate::services::{data_dir, files, kept_saves, keys, platform};

/// "1.4 GB", "312 MB".
fn size_text(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{} MB", bytes.div_ceil(1_048_576))
    }
}

impl App {
    /// Where a removed game's saves wait: beside the library file.
    pub fn saves_root(&self) -> String {
        join(&parent(&self.lib.path), e::SAVES_FOLDER)
    }

    /// Asks first; the package is never deleted. With a build on disk the question offers to delete it
    /// too (ticked): the player's files are kept first.
    pub fn remove_game(&mut self) {
        let Some(g) = self.selected_game() else {
            return;
        };
        if self.busy(&g.id) {
            return;
        }
        let (id, text) = (g.id.clone(), fill(l::REMOVE_TEXT, &[&g.title]));
        let yes = Action::ConfirmRemove(id.clone(), false);
        if !files::is_dir(&g.build_dir) {
            self.question(l::REMOVE_TITLE, &text, yes, None, false);
            return;
        }
        let size = size_text(kept_saves::folder_size(&g.build_dir));
        let tip = fill(
            l::DELETE_BUILD_TIP,
            &[&native(&g.build_dir), &native(&self.saves_root())],
        );
        let check = Check {
            label: fill(l::DELETE_BUILD, &[&size]),
            tip,
            on: true,
            yes_on: Action::ConfirmRemove(id, true),
        };
        self.question_checked(l::REMOVE_TITLE, &text, yes, check);
    }

    /// The answer: the entry goes; with `delete`, its saves are kept and its build folder deleted first
    /// (refused while it runs or builds, the entry then stays).
    pub fn confirm_remove(&mut self, id: &str, delete: bool) {
        let Some(game) = self.lib.find(id).cloned() else {
            return;
        };
        if delete {
            let running = platform::find_running_games(std::slice::from_ref(&game));
            if self.busy(id) || running.contains_key(id) {
                self.message(l::DELETE_TITLE, l::DELETE_BUSY, true);
                return;
            }
            let kept = match kept_saves::keep(&game, &self.saves_root()) {
                Ok(kept) => kept,
                Err(err) => return self.message(l::DELETE_TITLE, &err, true),
            };
            if let Err(err) = kept_saves::delete_build(&game) {
                return self.message(l::DELETE_TITLE, &err, true);
            }
            if let Some(kept) = kept {
                self.console
                    .push(fill(l::KEPT, &[&game.title, &native(&kept)]));
            }
        }
        self.lib.games.retain(|g| g.id != id);
        self.selected.clear();
        self.save();
    }

    pub fn install_keys(&mut self) {
        let Some(source) = pickers::file(&Pick {
            title: l::PICK_KEYS,
            filter: Some(l::KEYS_FILTER),
            start: "",
        }) else {
            return;
        };
        match keys::install(&source, &data_dir::data_dir()) {
            Err((title, text)) => self.message(&title, &text, true),
            Ok(dest) => {
                self.lib.keys_path = dest;
                self.save();
                self.refresh_tools();
                self.message(l::KEYS_DONE_TITLE, l::KEYS_DONE, false);
            }
        }
    }
}
