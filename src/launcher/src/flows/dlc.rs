//! Game > Install DLC…: the window, adding and removing packages (`installDlc`, `installDlcPackages`,
//! `removeDlc`, `removeDlcPackages`, `dlcReport`).

use super::surface::{ListWindow, Surface};
use super::{App, DlcReason, Outcome};
use crate::constants::words_dlc as w;
use crate::model::fill::{fill, plural};
use crate::model::path_text::{absolute, same_path};
use crate::services::{build_facts, dlc_files};

impl App {
    pub(super) fn dlc_window(&mut self) -> Option<&mut ListWindow> {
        self.surfaces.iter_mut().rev().find_map(|s| {
            if let Surface::Dlc(w) = s {
                Some(w)
            } else {
                None
            }
        })
    }

    /// Where an outcome goes: the DLC window when it is open, the command line's caller when nobody can
    /// click a message box, a message box otherwise.
    pub(super) fn dlc_report(&mut self, title: &str, text: &str, error: bool) {
        if let Some(win) = self.dlc_window() {
            win.status = Some((text.into(), error));
        } else if self.dlc_quiet {
            self.dlc_quiet = false;
            self.outcomes.push(Outcome::Dlc(!error, text.into()));
        } else if !text.is_empty() {
            self.message(title, text, error);
        }
    }

    fn adopt_dlc(&mut self, id: &str) {
        if self
            .lib
            .find_mut(id)
            .is_some_and(dlc_files::adopt_installed)
        {
            self.save();
        }
    }

    pub fn show_dlc(&mut self) {
        let Some(id) = self.selected_id().filter(|_| !self.demo) else {
            return;
        };
        self.adopt_dlc(&id);
        let status = self
            .dlc_busy()
            .then(|| (w::BUSY_RUNNING.to_string(), false));
        self.surfaces.push(Surface::Dlc(ListWindow {
            game_id: id.clone(),
            locked: self.busy(&id),
            status,
            ..ListWindow::default()
        }));
    }

    /// Add DLC… / a drop / `--install-dlc`: checked, added to the list; a built game gets them
    /// installed now, an unbuilt one when it is built. From the command line: all or nothing.
    pub fn install_dlc_packages(&mut self, files: &[String], interactive: bool) -> bool {
        self.dlc_quiet = !interactive && self.dlc_window().is_none();
        let Some(id) = self.selected_id().filter(|_| !self.demo) else {
            self.dlc_report(w::CANNOT_INSTALL, w::SELECT_FIRST, true);
            return false;
        };
        if self.busy(&id) || self.dlc_busy() {
            self.dlc_report(w::BUSY_TITLE, w::BUSY, true);
            return false;
        }
        self.adopt_dlc(&id);
        let have = self
            .lib
            .find(&id)
            .map(|g| g.dlc.clone())
            .unwrap_or_default();
        let (mut added, mut problems): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        for path in files.iter().map(|f| absolute(f)) {
            if have.iter().chain(added.iter()).any(|h| same_path(h, &path)) {
                continue;
            }
            match dlc_files::valid_package(&path) {
                Ok(()) => added.push(path),
                Err(e) => problems.push(e),
            }
        }
        if added.is_empty() {
            match problems.is_empty() {
                true => self.dlc_report(w::ALREADY_TITLE, w::ALREADY, false),
                false => self.dlc_report(w::CANNOT_INSTALL, &problems.join("\n"), true),
            }
            return problems.is_empty();
        }
        if !problems.is_empty() && self.dlc_quiet {
            self.dlc_report(w::CANNOT_INSTALL, &problems.join("\n"), true);
            return false;
        }
        if let Some(g) = self.lib.find_mut(&id) {
            g.dlc.extend(added.iter().cloned());
        }
        self.save();
        let Some(game) = self.lib.find(&id).cloned() else {
            return false;
        };
        if !build_facts::has_build(&game) {
            let lead = if problems.is_empty() {
                String::new()
            } else {
                format!("{}\n\n", problems.join("\n"))
            };
            let text = fill(
                w::ADDED,
                &[&added.len().to_string(), plural(added.len()), &game.title],
            );
            self.dlc_report(
                w::ADDED_TITLE,
                &format!("{lead}{text}"),
                !problems.is_empty(),
            );
            return true;
        }
        if self.start_dlc_sync(&id, DlcReason::Installed) {
            // what was no package at all is said now; the engine's verdict on the others follows
            if !problems.is_empty() {
                match self.dlc_window() {
                    Some(win) => win.status = Some((problems.join("\n"), true)),
                    None => self.message(w::CANNOT_INSTALL, &problems.join("\n"), true),
                }
            }
            return true;
        }
        if let Some(g) = self.lib.find_mut(&id) {
            g.dlc.retain(|p| !added.contains(p));
        }
        self.save();
        false
    }

    /// `--remove-dlc`: as the window's Remove, the outcome to the caller.
    pub fn remove_dlc(&mut self, packages: &[String]) {
        self.dlc_quiet = self.dlc_window().is_none();
        let Some(id) = self.selected_id().filter(|_| !self.demo) else {
            return self.dlc_report(w::CANNOT_REMOVE, w::SELECT_FIRST, true);
        };
        self.adopt_dlc(&id);
        self.remove_dlc_packages(&id, packages);
    }

    /// The packages leave the list and, for a built game, its data folder.
    pub fn remove_dlc_packages(&mut self, id: &str, packages: &[String]) {
        if self.busy(id) || self.dlc_busy() {
            return self.dlc_report(w::BUSY_TITLE, w::BUSY, true);
        }
        let Some(g) = self.lib.find_mut(id) else {
            return;
        };
        let before = g.dlc.len();
        g.dlc.retain(|p| !packages.iter().any(|r| same_path(p, r)));
        let removed = before - g.dlc.len();
        if removed == 0 {
            return self.dlc_report(w::NOT_LISTED_TITLE, w::NOT_LISTED, true);
        }
        self.save();
        if self.lib.find(id).is_some_and(build_facts::has_build) {
            if !self.start_dlc_sync(id, DlcReason::Changed) && self.dlc_window().is_some() {
                self.adopt_dlc(id);
            }
        } else {
            self.dlc_report(
                w::REMOVED_TITLE,
                &fill(w::REMOVED, &[&removed.to_string(), plural(removed)]),
                false,
            );
        }
    }
}
