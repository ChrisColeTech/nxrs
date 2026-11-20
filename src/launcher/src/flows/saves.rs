//! Copy saves from the Turbo build, Remove from library, Install keys (`copySaves`, `removeGame`,
//! `installKeys`).

use super::{Action, App, Outcome};
use crate::constants::{engine as e, words_library as l, words_play as w};
use crate::model::fill::fill;
use crate::model::path_text::native;
use crate::services::pickers::{self, Pick};
use crate::services::{data_dir, keys, saves};

impl App {
    fn copy_done(&mut self, interactive: bool, ok: bool, title: &str, detail: &str) {
        if interactive {
            self.message(title, detail, !ok);
        } else {
            self.console.push(format!("\n{title}: {detail}"));
        }
        self.outcomes
            .push(Outcome::SaveCopy(ok, detail.to_string()));
    }

    /// The Turbo build's saves into this nxrs build, after a backup; `--copy-saves` without dialogs.
    pub fn copy_saves(&mut self, interactive: bool, confirm: bool) {
        let Some(game) = self.selected_game().cloned().filter(|_| !self.demo) else {
            return self.copy_done(interactive, false, w::COPY_FAILED, w::COPY_SELECT);
        };
        if self.busy(&game.id) {
            return self.copy_done(interactive, false, w::COPY_FAILED, w::COPY_BUSY);
        }
        if interactive && confirm {
            let text = fill(w::COPY_ASK, &[&game.title]);
            return self.question(w::COPY_TITLE, &text, Action::ConfirmCopySaves, None, false);
        }
        match saves::copy_from_turbo(&game) {
            Err(err) => self.copy_done(interactive, false, w::COPY_FAILED, &err),
            Ok(copy) => {
                if let Some(g) = self.lib.find_mut(&game.id) {
                    g.saves_offered = e::ENGINE_TURBO.into();
                }
                self.save();
                let tail = if copy.backup.is_empty() {
                    w::NO_BACKUP.to_string()
                } else {
                    format!("{}{}", w::BACKUP, native(&copy.backup))
                };
                self.copy_done(
                    interactive,
                    true,
                    w::COPIED,
                    &format!("{}{tail}", copy.report),
                );
            }
        }
    }

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
