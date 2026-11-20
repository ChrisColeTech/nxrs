//! Game > Install Updates…: the window, adding (through the engine's check), removing and choosing the
//! version to build (`installUpdates`, `installUpdatePackages`, `removeUpdatePackages`,
//! `setUpdateChoice`, `updateReport`).

use super::surface::{ListWindow, Surface};
use super::{App, Outcome};
use crate::constants::words_updates as w;
use crate::model::fill::{fill, plural};
use crate::model::path_text::{absolute, same_path};
use crate::model::update_text;
use crate::services::update_files;

impl App {
    pub(super) fn updates_window(&mut self) -> Option<&mut ListWindow> {
        self.surfaces.iter_mut().rev().find_map(|s| {
            if let Surface::Updates(w) = s {
                Some(w)
            } else {
                None
            }
        })
    }

    pub(super) fn update_report(&mut self, title: &str, text: &str, error: bool) {
        if let Some(win) = self.updates_window() {
            win.status = Some((text.into(), error));
        } else if self.update_quiet {
            self.update_quiet = false;
            self.outcomes.push(Outcome::Update(!error, text.into()));
        } else {
            self.message(title, text, error);
        }
    }

    /// Whether the chosen version needs a build, for the reports.
    pub(super) fn update_outcome(&self, id: &str) -> String {
        let f = self.facts_of(id);
        self.lib
            .find(id)
            .map(|g| update_text::outcome(g, f.update_pending, f.has_build))
            .unwrap_or_default()
    }

    pub fn show_updates(&mut self) {
        let Some(id) = self.selected_id().filter(|_| !self.demo) else {
            return;
        };
        let status = self
            .probe_busy()
            .then(|| (w::BUSY_RUNNING.to_string(), false));
        self.surfaces.push(Surface::Updates(ListWindow {
            game_id: id.clone(),
            locked: self.busy(&id),
            status,
            ..ListWindow::default()
        }));
    }

    /// Add update… / a drop / `--install-update`: checked here, then by the engine against the game.
    pub fn install_update_packages(&mut self, files: &[String], interactive: bool) -> bool {
        self.update_quiet = !interactive && self.updates_window().is_none();
        let Some(id) = self.selected_id().filter(|_| !self.demo) else {
            self.update_report(
                w::CANNOT_INSTALL,
                crate::constants::words_dlc::SELECT_FIRST,
                true,
            );
            return false;
        };
        if self.busy(&id) || self.probe_busy() {
            self.update_report(crate::constants::words_dlc::BUSY_TITLE, w::BUSY, true);
            return false;
        }
        let have: Vec<String> = self
            .lib
            .find(&id)
            .map(|g| g.updates.iter().map(|u| u.path.clone()).collect())
            .unwrap_or_default();
        let (mut wanted, mut problems): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        for path in files.iter().map(|f| absolute(f)) {
            if have
                .iter()
                .chain(wanted.iter())
                .any(|h| same_path(h, &path))
            {
                continue;
            }
            match update_files::valid_package(&path) {
                Ok(()) => wanted.push(path),
                Err(e) => problems.push(e),
            }
        }
        if wanted.is_empty() {
            match problems.is_empty() {
                true => self.update_report(w::ALREADY_TITLE, w::ALREADY, false),
                false => self.update_report(w::CANNOT_INSTALL, &problems.join("\n"), true),
            }
            return problems.is_empty();
        }
        if !problems.is_empty() && self.update_quiet {
            self.update_report(w::CANNOT_INSTALL, &problems.join("\n"), true);
            return false;
        }
        self.start_probe(&id, &wanted, &problems)
    }

    /// `--remove-update`: as the window's Remove, the outcome to the caller.
    pub fn remove_updates(&mut self, packages: &[String]) {
        self.update_quiet = self.updates_window().is_none();
        let Some(id) = self.selected_id().filter(|_| !self.demo) else {
            return self.update_report(
                w::CANNOT_REMOVE,
                crate::constants::words_dlc::SELECT_FIRST,
                true,
            );
        };
        self.remove_update_packages(&id, packages);
    }

    pub fn remove_update_packages(&mut self, id: &str, packages: &[String]) {
        if self.busy(id) || self.probe_busy() {
            return self.update_report(crate::constants::words_dlc::BUSY_TITLE, w::BUSY, true);
        }
        let Some(g) = self.lib.find_mut(id) else {
            return;
        };
        let before = g.updates.len();
        if packages.iter().any(|p| same_path(&g.update_choice, p)) {
            g.update_choice.clear(); // its choice goes with it: back to the newest
        }
        g.updates
            .retain(|u| !packages.iter().any(|p| same_path(&u.path, p)));
        let removed = before - g.updates.len();
        if removed == 0 {
            return self.update_report(w::NOT_LISTED_TITLE, w::NOT_LISTED, true);
        }
        self.save();
        self.refresh_facts();
        let were = if removed == 1 { " was" } else { "s were" };
        let text = fill(
            w::REMOVED,
            &[
                &removed.to_string(),
                plural(removed),
                were,
                &self.update_outcome(id),
            ],
        );
        self.update_report(w::REMOVED_TITLE, &text, false);
    }

    /// `--update-choice newest|none|<pkg>`.
    pub fn choose_update(&mut self, choice: &str) {
        self.update_quiet = self.updates_window().is_none();
        let Some(id) = self.selected_id().filter(|_| !self.demo) else {
            return self.update_report(
                w::CANNOT_CHOOSE,
                crate::constants::words_dlc::SELECT_FIRST,
                true,
            );
        };
        self.set_update_choice(&id, if choice == "newest" { "" } else { choice });
    }

    pub fn set_update_choice(&mut self, id: &str, choice: &str) {
        if self.busy(id) {
            return self.update_report(crate::constants::words_dlc::BUSY_TITLE, w::BUSY_RUN, true);
        }
        let Some(g) = self.lib.find_mut(id) else {
            return;
        };
        let next = match choice {
            "" | "none" => choice.to_string(),
            path => match g.updates.iter().find(|u| same_path(&u.path, path)) {
                Some(u) => u.path.clone(),
                None => return self.update_report(w::NOT_LISTED_TITLE, w::NOT_LISTED, true),
            },
        };
        g.update_choice = next;
        self.save();
        self.refresh_facts();
        let text = self.update_outcome(id);
        self.update_report(w::CHOSEN_TITLE, &text, false);
    }
}
