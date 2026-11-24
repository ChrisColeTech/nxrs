//! Properties' Apply: what changed copied into the entry, a built game's screen mode and layout written
//! into its INI now, removed DLC synced (`showProperties`).

use super::properties::PropertiesDraft;
use super::surface::Surface;
use super::{App, DlcReason};
use crate::model::game::Game;
use crate::model::path_text::{absolute, same_path};
use crate::model::play::{effective_button_layout, effective_docked};
use crate::services::{build_facts, dlc_files, ini_file, sources};

impl App {
    /// `--source`: the Properties source edit as a headless, composable product operation.
    pub fn set_source(&mut self, source: &str) -> Result<String, String> {
        sources::valid_source(source)?;
        let id = self
            .selected_id()
            .ok_or_else(|| "No game is selected".to_string())?;
        if self.busy(&id) {
            return Err("The selected game is running or building".into());
        }
        let path = absolute(source);
        let game = self
            .lib
            .find_mut(&id)
            .ok_or_else(|| "No game is selected".to_string())?;
        game.source = path.clone();
        self.save();
        self.refresh_facts();
        Ok(path)
    }

    pub fn show_properties(&mut self, tab: usize) {
        let Some(id) = self.selected_id() else { return };
        if self
            .lib
            .find_mut(&id)
            .is_some_and(dlc_files::adopt_installed)
        {
            self.save();
        }
        let Some(game) = self.lib.find(&id).cloned() else {
            return;
        };
        let draft = PropertiesDraft::new(self, game, tab, false);
        self.surfaces.push(Surface::Properties(Box::new(draft)));
    }

    /// Apply (or Add game): true when the window closes.
    pub fn apply_properties(&mut self, draft: &PropertiesDraft) -> bool {
        let edited = match draft.accept() {
            Ok(g) => g,
            Err((title, text)) => {
                self.message(&title, &text, true);
                return false;
            }
        };
        if draft.adding {
            self.add_game(edited, true);
            return true;
        }
        let id = draft.game.id.clone();
        let errors = self.copy_edits(&id, &edited);
        errors
            .iter()
            .for_each(|e| self.message(crate::constants::words_play::NO_SETTINGS, e, true));
        self.artwork_epoch += 1;
        self.save();
        self.refresh_facts();
        let dlc_changed = draft.game.dlc != edited.dlc;
        if dlc_changed && self.lib.find(&id).is_some_and(build_facts::has_build) {
            self.start_dlc_sync(&id, DlcReason::Changed);
        }
        true
    }

    fn copy_edits(&mut self, id: &str, edited: &Game) -> Vec<String> {
        let (docked, layout) = (self.lib.docked, self.lib.button_layout.clone());
        let Some(c) = self.lib.find_mut(id) else {
            return Vec::new();
        };
        (c.title, c.source, c.build_dir, c.renderer) = (
            edited.title.clone(),
            edited.source.clone(),
            edited.build_dir.clone(),
            edited.renderer.clone(),
        );
        (c.workers, c.cover, c.background, c.dlc) = (
            edited.workers,
            edited.cover.clone(),
            edited.background.clone(),
            edited.dlc.clone(),
        );
        // an update removed: a choice of it goes back to the newest
        if c.updates.len() != edited.updates.len() {
            c.updates = edited.updates.clone();
            let kept = c.update_choice.is_empty()
                || c.update_choice == "none"
                || c.updates
                    .iter()
                    .any(|u| same_path(&u.path, &c.update_choice));
            if !kept {
                c.update_choice.clear();
            }
        }
        let mut errors = Vec::new();
        let built = build_facts::has_build(c);
        if effective_docked(c, docked) != effective_docked(edited, docked) && built {
            errors.extend(
                ini_file::write_docked(&c.ini_path(), effective_docked(edited, docked)).err(),
            );
        }
        c.screen_mode = edited.screen_mode.clone();
        // already-built games get the new layout in their INI now, not only at the next Play
        let layout_changed = c.button_layout != edited.button_layout;
        c.button_layout = edited.button_layout.clone();
        if layout_changed && built {
            errors.extend(
                ini_file::write_button_layout(&c.ini_path(), &effective_button_layout(c, &layout))
                    .err(),
            );
        }
        errors
    }
}
