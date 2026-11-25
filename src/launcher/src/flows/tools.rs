//! The engine: finding nxrs-build off the UI thread, its doctor, the engine badge, `--engine` and
//! `--nxrs-build` (`refreshTools`, `toolProblem`, `setEngine`, `setNxrsBuild`).

use super::App;
use crate::constants::{engine as e, words_build as w};
use crate::model::path_text::absolute;
use crate::services::run;
use crate::services::toolchain::Toolchain;
use std::sync::mpsc::channel;

impl App {
    /// Finds nxrs-build and reads its contract on a worker thread (it may start nxrs-build).
    pub fn refresh_tools(&mut self) {
        let (tx, rx) = channel();
        let (chosen, keys, notify) = (
            self.lib.nxrs_build.clone(),
            self.lib.keys_path.clone(),
            self.notify.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(Toolchain::find(&chosen, &keys));
            notify();
        });
        self.tools_rx = Some(rx);
    }
    /// The same, waited for (command-line verbs).
    pub fn refresh_tools_now(&mut self) {
        (self.tools, self.tools_checked, self.tools_rx) = (
            Toolchain::find(&self.lib.nxrs_build, &self.lib.keys_path),
            true,
            None,
        );
        self.refresh_facts();
    }

    /// What keeps nxrs from building; empty when nothing does (or while it is being checked).
    pub fn tool_problem(&self) -> String {
        let p = if self.tools_checked {
            self.tools.problem()
        } else {
            String::new()
        };
        if p.is_empty() {
            p
        } else {
            format!("{}{p}", w::PROBLEM_PREFIX)
        }
    }
    /// The engine badge: "Checking…", "Engine ready" or "Setup needed".
    pub fn badge(&self) -> (&'static str, bool) {
        match (self.tools_checked, self.tool_problem().is_empty()) {
            (false, _) => (w::BADGE_CHECKING, true),
            (true, true) => (w::BADGE_READY, true),
            (true, false) => (w::BADGE_PROBLEM, false),
        }
    }
    pub fn health_text(&self) -> String {
        if self.tools_checked {
            self.tools.health_text()
        } else {
            w::BADGE_CHECKING.into()
        }
    }

    /// Settings > Engine's checks, run on a worker thread.
    pub fn run_doctor(&mut self) {
        if self.tools.nxrs_build.is_empty() {
            self.doctor_text = w::DOCTOR_NO_ENGINE.into();
            return;
        }
        (self.doctor_text, self.doctor_running) = (w::DOCTOR_CHECKING.into(), true);
        let (tx, rx) = channel();
        let (tools, notify) = (self.tools.clone(), self.notify.clone());
        std::thread::spawn(move || {
            let _ = tx.send(run::doctor(&tools));
            notify();
        });
        self.doctor_rx = Some(rx);
    }

    /// `--engine`: kept for older scripts; nxrs is the only engine, refused for a busy game.
    pub fn set_engine(&mut self, engine: &str) -> bool {
        let Some(id) = self.selected_id() else {
            return false;
        };
        engine == e::ENGINE_NXRS && !self.demo && !self.busy(&id)
    }
    /// `--nxrs-build`: saved; empty = the bundled one.
    pub fn set_nxrs_build(&mut self, path: &str) {
        self.lib.nxrs_build = if path.is_empty() {
            String::new()
        } else {
            absolute(path)
        };
        self.save();
    }
}
