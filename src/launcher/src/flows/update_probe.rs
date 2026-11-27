//! `nxrs-build update-info`: what the update packages are and whether they belong to the game
//! (`UpdateProbe` and its finished handler). Nothing is written anywhere by the engine.

use super::App;
use super::runs::ProbeRun;
use crate::constants::{timing, words_dlc as d, words_updates as w};
use crate::model::args;
use crate::model::fill::fill;
use crate::model::output_lines::{UpdateLine, update_line};
use crate::model::path_text::{parent, same_path};
use crate::services::build_facts;
use crate::services::jobs::{Job, JobEvent, Spec};

impl App {
    fn probe_check(&self, id: &str) -> Result<(), String> {
        if !self.tools.can_build() {
            return Err(self.tools.problem());
        }
        if !self.tools.keys_exist() {
            return Err(w::NEEDS_KEYS.into());
        }
        if !self.lib.find(id).is_some_and(build_facts::source_available) {
            return Err(w::NO_SOURCE.into());
        }
        Ok(())
    }

    pub(super) fn start_probe(&mut self, id: &str, wanted: &[String], problems: &[String]) -> bool {
        if let Err(err) = self.probe_check(id) {
            self.update_report(w::CANNOT_INSTALL, &err, true);
            return false;
        }
        let Some(game) = self.lib.find(id).cloned() else {
            return false;
        };
        let a = args::update_info(&game, &self.tools.keys, self.tools.keys_exist(), wanted);
        let (program, cwd, env) = (
            self.tools.nxrs_build.clone(),
            parent(&self.tools.nxrs_build),
            self.tools.environment(),
        );
        match Job::start(
            Spec {
                program: &program,
                args: &a,
                cwd: &cwd,
                env: &env,
                log: None,
            },
            self.notify.clone(),
        ) {
            Err(err) => {
                self.update_report(w::CANNOT_INSTALL, &err, true);
                return false;
            }
            Ok(job) => {
                self.probe = Some(ProbeRun {
                    job,
                    game_id: id.into(),
                    found: Vec::new(),
                    errors: Vec::new(),
                })
            }
        }
        self.job_label = w::JOB_CHECKING.into();
        self.bar = Some((0, 0));
        self.console.push(format!("\nUpdates: {}", game.title));
        let text = if problems.is_empty() {
            w::BUSY_CHECKING.to_string()
        } else {
            format!("{}{}", problems.join("\n"), w::BUSY_OTHERS)
        };
        if let Some(win) = self.updates_window() {
            win.status = Some((text, false));
        } else if !problems.is_empty() {
            let context = crate::constants::words_contexts::UPDATES;
            self.message(context, w::CANNOT_INSTALL, &problems.join("\n"), true);
        }
        true
    }

    pub(super) fn drain_probe(&mut self) {
        let Some(run) = self.probe.as_mut() else {
            return;
        };
        let mut exit = None;
        let mut lines = Vec::new();
        for event in run.job.drain(timing::EVENTS_PER_FRAME) {
            match event {
                JobEvent::Line(l) if l.trim().is_empty() => {}
                JobEvent::Line(l) => {
                    match update_line(&l) {
                        UpdateLine::Found(u) => run.found.push(u),
                        UpdateLine::Error(e) => run.errors.push(e),
                        UpdateLine::Other => {}
                    }
                    lines.push(l);
                }
                JobEvent::Exited(code) => exit = Some(code),
            }
        }
        lines.into_iter().for_each(|l| self.console.push(l));
        if let Some((run, code)) = exit.and_then(|code| self.probe.take().map(|run| (run, code))) {
            self.finish_probe(run, code);
        }
    }

    fn finish_probe(&mut self, run: ProbeRun, code: Option<i32>) {
        let mut errors = run.errors;
        if code != Some(0) && errors.is_empty() {
            errors.push(fill(d::STOPPED, &[&code.unwrap_or(-1).to_string()]));
        }
        (self.bar, self.job_label) = (None, String::new());
        let Some(g) = self.lib.find_mut(&run.game_id) else {
            return;
        };
        let mut added = Vec::new();
        for update in &run.found {
            match g
                .updates
                .iter_mut()
                .find(|h| same_path(&h.path, &update.path))
            {
                Some(have) => *have = update.clone(),
                None => g.updates.push(update.clone()),
            }
            added.push(update.label());
        }
        if !run.found.is_empty() {
            self.save();
            self.refresh_facts();
        }
        let mut text = errors.join("\n\n");
        if !added.is_empty() {
            let gap = if text.is_empty() { "" } else { "\n\n" };
            text += &format!(
                "{gap}{}",
                fill(
                    w::ADDED,
                    &[&added.join(", "), &self.update_outcome(&run.game_id)]
                )
            );
        }
        let error = !errors.is_empty();
        self.update_report(
            if error {
                w::FAILED_TITLE
            } else {
                w::ADDED_TITLE
            },
            &text,
            error,
        );
    }
}
