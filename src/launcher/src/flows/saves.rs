//! Remove from library, Install keys (`removeGame`, `installKeys`).

use super::{Action, App};
use crate::constants::words_library as l;
use crate::model::fill::fill;
use crate::services::pickers::{self, Pick};
use crate::services::{data_dir, keys};

impl App {
    /// Asks first; the package, builds and saves are not deleted.
    pub fn remove_game(&mut self) {
        let Some(g) = self.selected_game() else {
            return;
        };
        if self.busy(&g.id) {
            return;
        }
        let (id, text) = (g.id.clone(), fill(l::REMOVE_TEXT, &[&g.title]));
        self.question(
            l::REMOVE_TITLE,
            &text,
            Action::ConfirmRemove(id),
            None,
            false,
        );
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
