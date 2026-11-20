//! `nxrs-build dlc sync`: the data folder made to match the game's DLC list (`startDlcSync`,
//! `DlcInstaller`, its finished handler).

use super::runs::DlcRun;
use super::{App, DlcReason, Outcome};
use crate::constants::{timing, words_dlc as w};
use crate::model::dlc_title::title_label;
use crate::model::fill::{fill, plural};
use crate::model::path_text::{parent, same_path};
use crate::model::{args, output_lines};
use crate::services::jobs::{Job, JobEvent, Spec};
use crate::services::{dlc_files, markers};

impl App {
    fn sync_check(&self, id: &str) -> Result<(), String> {
        let Some(game) = self.lib.find(id) else {
            return Err(w::SELECT_FIRST.into());
        };
        if self.dlc_run.is_some() {
            return Err(w::SYNC_BUSY.into());
        }
        if !self.tools.can_build() {
            return Err(self.tools.problem());
        }
        if !markers::has_data(game) {
            return Err(w::BUILD_FIRST.into());
        }
        if !game.dlc.is_empty() && !self.tools.keys_exist() {
            return Err(w::NEEDS_KEYS.into());
        }
        Ok(())
    }

    pub fn start_dlc_sync(&mut self, id: &str, reason: DlcReason) -> bool {
        if let Err(err) = self.sync_check(id) {
            self.dlc_report(w::CANNOT_INSTALL, &err, true);
            return false;
        }
        let Some(game) = self.lib.find(id).cloned() else {
            return false;
        };
        let a = args::dlc_sync(&game, &self.tools.keys, self.tools.keys_exist());
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
                self.dlc_report(w::CANNOT_INSTALL, &err, true);
                return false;
            }
            Ok(job) => {
                self.dlc_run = Some(DlcRun {
                    job,
                    game_id: id.into(),
                    errors: Vec::new(),
                })
            }
        }
        self.dlc_reason = reason;
        let changing = game.dlc.is_empty() || reason == DlcReason::Changed;
        self.job_label = if changing {
            w::JOB_UPDATING
        } else {
            w::JOB_INSTALLING
        }
        .into();
        self.bar = Some((0, 0));
        let busy = if reason == DlcReason::Changed {
            w::BUSY_REMOVING
        } else {
            w::BUSY_INSTALLING
        };
        if let Some(win) = self.dlc_window() {
            win.status = Some((busy.into(), false));
        }
        self.console.push(format!("\nDLC: {}", game.title));
        true
    }

    pub(super) fn drain_dlc(&mut self) {
        let Some(run) = self.dlc_run.as_mut() else {
            return;
        };
        let mut exit = None;
        let mut lines = Vec::new();
        for event in run.job.drain(timing::EVENTS_PER_FRAME) {
            match event {
                JobEvent::Line(l) if l.is_empty() => {}
                JobEvent::Line(l) => {
                    if let Some(reason) = output_lines::dlc_error(&l) {
                        run.errors.push(reason.to_string());
                    }
                    lines.push(l);
                }
                JobEvent::Exited(code) => exit = Some(code),
            }
        }
        lines.into_iter().for_each(|l| self.console.push(l));
        if let Some((run, code)) = exit.and_then(|code| self.dlc_run.take().map(|run| (run, code)))
        {
            self.finish_dlc(run, code);
        }
    }

    fn finish_dlc(&mut self, run: DlcRun, code: Option<i32>) {
        let mut errors = run.errors;
        if code != Some(0) && errors.is_empty() {
            errors.push(fill(w::STOPPED, &[&code.unwrap_or(-1).to_string()]));
        }
        self.bar = None;
        let id = run.game_id;
        let Some(data) = self.lib.find(&id).map(|g| g.data_dir()) else {
            return self.job_label.clear();
        };
        // what the data folder holds now is the truth: a package the engine refused leaves the list
        let installed = dlc_files::installed(&data);
        let titles: Vec<String> = installed.iter().map(title_label).collect();
        if let Some(g) = self.lib.find_mut(&id) {
            g.dlc
                .retain(|p| installed.iter().any(|t| same_path(&t.source, p)));
        }
        self.save();
        if self.dlc_reason == DlcReason::Built && self.build_dlc_pending {
            self.build_dlc_pending = false;
            self.outcomes.push(Outcome::Smoke(errors.is_empty()));
        }
        if !errors.is_empty() {
            self.job_label = w::JOB_FAILED.into();
            return self.dlc_report(w::FAILED_TITLE, &errors.join("\n\n"), true);
        }
        let n = installed.len();
        self.job_label = if n == 0 {
            w::JOB_REMOVED.into()
        } else {
            fill(w::JOB_INSTALLED, &[&n.to_string(), plural(n)])
        };
        self.report_sync(&id, &titles);
        self.dlc_quiet = false;
        if self.dlc_reason == DlcReason::Play
            && self.lib.find(&id).is_some_and(dlc_files::up_to_date)
        {
            self.select(&id);
            self.play();
        }
    }

    fn report_sync(&mut self, id: &str, titles: &[String]) {
        let window = self.dlc_window().is_some();
        let game = self
            .lib
            .find(id)
            .map(|g| g.title.clone())
            .unwrap_or_default();
        if self.dlc_reason == DlcReason::Installed && !titles.is_empty() {
            let text = if window || self.dlc_quiet {
                fill(w::INSTALLED_SHORT, &[&titles.join(", ")])
            } else {
                fill(w::INSTALLED_LONG, &[&game, &titles.join("\n")])
            };
            self.dlc_report(w::INSTALLED_TITLE, &text, false);
        } else if self.dlc_reason == DlcReason::Changed && window {
            let text = if titles.is_empty() {
                w::REMOVED_NONE.into()
            } else {
                fill(w::REMOVED_SOME, &[&titles.join(", ")])
            };
            self.dlc_report("", &text, false);
        } else if self.dlc_quiet {
            let text = if titles.is_empty() {
                w::QUIET_NONE.into()
            } else {
                fill(w::QUIET_SOME, &[&titles.join(", ")])
            };
            self.dlc_report("", &text, false);
        }
    }
}
