//! A build's end (`BuildManager` finished, `Window` finished): its outcome, the library entry, the DLC
//! sync of a fresh build; and cancelling.

use super::runs::BuildRun;
use super::{App, DlcReason, Outcome};
use crate::constants::{engine as e, timing, words_build as w, words_library as l};
use crate::model::dates;
use crate::model::fill::fill;
use crate::model::path_text::{file_name, native};
use crate::services::jobs::JobEvent;
use crate::services::{build_facts, ini_file, kept_saves, markers};

impl App {
    pub(super) fn drain_build(&mut self) {
        let Some(run) = self.build.as_mut() else {
            return;
        };
        let events = run.job.drain(timing::EVENTS_PER_FRAME);
        let mut exit = None;
        for event in events {
            match event {
                JobEvent::Line(line) => {
                    if let Some(shown) = self.build_line(line) {
                        self.console.push(shown);
                    }
                }
                JobEvent::Exited(code) => exit = Some(code),
            }
        }
        // the run is taken only once it has ended
        if let Some(run) = exit.and_then(|code| self.build.take().map(|run| (run, code))) {
            self.finish_build(run.0, run.1);
        }
    }

    /// The outcome's detail line, checking the files and writing the game's settings on success.
    fn outcome(run: &BuildRun, code: Option<i32>) -> (bool, String) {
        if run.job.cancelled() {
            return (false, w::CANCELLED.into());
        }
        if code != Some(0) {
            return (false, fill(w::FAILED, &[&code.unwrap_or(-1).to_string()]));
        }
        let mut game = run.game.clone();
        (game.built_source, game.built_update, game.build_incomplete) =
            (game.source.clone(), game.update_token(), false);
        if !build_facts::has_build(&game) {
            return (false, w::MISSING_FILES.into());
        }
        // the copy's layout was resolved when the build started
        match ini_file::write_game_settings(&game, &game.button_layout, &run.play) {
            Err(err) => (false, format!("{}{err}", w::NO_SETTINGS)),
            Ok(()) => (true, w::DONE.into()),
        }
    }

    fn finish_build(&mut self, run: BuildRun, code: Option<i32>) {
        if run.web {
            return self.finish_web_build(run, code);
        }
        let (ok, detail) = Self::outcome(&run, code);
        run.job.log_line(&detail);
        let id = run.game.id.clone();
        let mut note = None;
        if let Some(g) = self.lib.find_mut(&id)
            && ok
        {
            // a package with the game and its DLC: the first build from it lists it as DLC too
            let before = g.built_source.clone();
            if run.holds_dlc && markers::adopt_package_dlc(g, &before) {
                note = Some(fill(w::PACKAGE_DLC, &[&file_name(&g.source)]));
            }
            (g.built_source, g.built_update, g.build_incomplete) =
                (g.source.clone(), run.game.update_token(), false);
            g.last_built = Some(dates::now());
        }
        if let Some(n) = note {
            self.console.push(n);
        }
        // saves kept when the game was removed with its build go back into the new one
        let game = self.lib.find(&id).cloned().filter(|_| ok);
        match game.map(|g| kept_saves::restore(&g, &self.saves_root())) {
            Some(Ok(Some(kept))) => {
                let line = fill(l::RESTORED, &[&native(&kept)]);
                run.job.log_line(&line);
                self.console.push(line);
            }
            Some(Err(err)) => self.console.push(err),
            _ => {}
        }
        self.save();
        self.console.push(format!("\n{detail}"));
        self.console.title = if ok {
            w::OUTPUT_COMPLETE
        } else {
            w::OUTPUT_STOPPED
        }
        .into();
        self.bar = None;
        self.job_label = if ok { w::JOB_COMPLETE } else { w::JOB_STOPPED }.into();
        self.refresh_facts();
        if self.closing {
            self.outcomes.push(Outcome::Smoke(ok));
            self.close_now = true;
            return;
        }
        // a fresh build gets the game's DLC put (back) into its data folder; --build waits for that too
        let has_dlc = self.lib.find(&id).is_some_and(|g| !g.dlc.is_empty());
        if ok && has_dlc && self.start_dlc_sync(&id, DlcReason::Built) {
            self.build_dlc_pending = true;
            return;
        }
        self.outcomes.push(Outcome::Smoke(ok));
    }

    /// Cancel build: the whole process tree ends.
    pub(super) fn cancel_build(&mut self) {
        if let Some(b) = &self.build {
            b.job.cancel();
        }
        self.set_progress(e::STOPPING, 0, 0);
    }

    /// The window closing during a build: confirmed, the build cancelled, closed when it ends.
    pub(super) fn close_after_cancel(&mut self) {
        self.closing = true;
        self.cancel_build();
    }

    pub fn request_close(&mut self) {
        if self.build.is_none() {
            self.close_now = true;
        } else if !self.closing {
            self.ask_close();
        }
    }

    /// The question a close during a build asks (also `--show-window close-build`, a capture).
    pub fn ask_close(&mut self) {
        let ask = super::surface::Ask::new(w::CLOSE_TITLE, w::CLOSE_TEXT, "", w::CLOSE_VERB);
        self.question(ask, super::Action::ConfirmClose, None, false);
    }
}
