//! Starting a build (`Window::startBuild`, `BuildManager::start`): its checks in order, the log, the run.

use super::App;
use super::Outcome;
use super::runs::BuildRun;
use super::view_state::BuildProgress;
use crate::constants::{engine as e, menus, words_build as w};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::names::output_name_for;
use crate::model::path_text::{display, ends_with_ci, file_name, parent};
use crate::model::play::effective_button_layout;
use crate::model::{args, output_lines};
use crate::services::jobs::{Job, Spec, open_build_log};
use crate::services::{files, sources};

impl App {
    pub fn start_build(&mut self) {
        let Some(id) = self.selected_id() else { return };
        if self.demo || self.is_running(&id) {
            return;
        }
        if let Some(g) = self.lib.find_mut(&id) {
            // a name from an older library keeps step with what the engine writes
            g.output_name = output_name_for(&g.output_name);
        }
        let Some(game) = self.lib.find(&id).cloned() else {
            return;
        };
        let mut build = game.clone();
        build.button_layout = effective_button_layout(&game, &self.lib.button_layout);
        if let Err(err) = self.start_build_job(build) {
            self.outcomes.push(Outcome::Smoke(false));
            self.message(
                crate::constants::words_contexts::BUILD,
                w::CANNOT_START,
                &err,
                true,
            );
            self.refresh_facts();
            return;
        }
        if let Some(g) = self.lib.find_mut(&id) {
            g.build_incomplete = true;
        }
        self.save();
        self.console.clear();
        self.console.push(format!(
            "Source: {}\nOutput: {}\n",
            display(&game.source),
            display(&game.build_dir)
        ));
        self.console.title = format!("{} — {}", w::OUTPUT, game.title);
        self.lib.console.visible = true;
        self.refresh_facts();
    }

    fn check_build(&self, next: &Game) -> Result<(), String> {
        if self.build.is_some() {
            return Err(w::BUSY.into());
        }
        if !self.tools.can_build() {
            return Err(self.tools.problem());
        }
        let xci = ends_with_ci(&next.source, ".xci");
        if (xci || ends_with_ci(&next.source, ".nsp")) && !self.tools.keys_exist() {
            return Err(format!(
                "{}{}",
                if xci { "XCI" } else { "NSP" },
                w::NEEDS_KEYS
            ));
        }
        sources::valid_source(&next.source)?;
        if let Some(u) = next.active_update() {
            if !files::is_file(&u.path) {
                return Err(fill(w::UPDATE_MISSING, &[&file_name(&u.path)]));
            }
            if !self.tools.keys_exist() {
                return Err(w::UPDATE_KEYS.into());
            }
        }
        if !files::make_dir(&next.build_dir) {
            return Err(w::NO_FOLDER.into());
        }
        Ok(())
    }

    fn start_build_job(&mut self, next: Game) -> Result<(), String> {
        self.check_build(&next)?;
        let log = open_build_log(&next.build_log())?;
        let a = args::build(
            &next,
            &self.tools.keys,
            self.tools.keys_exist(),
            self.lib.jobs,
        );
        let (program, cwd, env) = (
            self.tools.nxrs_build.clone(),
            parent(&self.tools.nxrs_build),
            self.tools.environment(),
        );
        let spec = Spec {
            program: &program,
            args: &a,
            cwd: &cwd,
            env: &env,
            log: Some(log),
        };
        let job = Job::start(spec, self.notify.clone())
            .map_err(|err| format!("{}{err}", w::NOT_STARTED))?;
        self.build = Some(BuildRun {
            job,
            game: next,
            play: self.lib.play(),
            holds_dlc: false,
        });
        self.set_progress(e::STARTING, 0, 0);
        Ok(())
    }

    pub(super) fn set_progress(&mut self, phase: &str, current: i64, total: i64) {
        self.progress = BuildProgress {
            phase: phase.into(),
            current,
            total,
        };
        self.job_label = if total != 0 {
            format!("{phase}  {current}/{total}")
        } else {
            phase.into()
        };
        self.bar = Some((current, total));
    }

    /// The card's primary button: Stop / Cancel build, Play, or Build.
    pub fn card_action(&mut self, id: &str) {
        self.select(id);
        if self.demo || self.lib.find(id).is_none() {
            return;
        }
        if self.busy(id) {
            self.stop();
        } else if self.facts_of(id).playable() {
            self.play();
        } else {
            self.start_build();
        }
    }

    /// Enter / double-click: Play when it can, else Properties > Build.
    pub fn activate(&mut self) {
        let Some(id) = self.selected_id() else { return };
        if self.facts_of(&id).playable() {
            self.play()
        } else {
            self.show_properties(1)
        }
    }

    /// The build's label in menus: Rebuild when the update changed.
    pub fn build_label(&self) -> &'static str {
        let pending = self
            .selected_id()
            .is_some_and(|id| self.facts_of(&id).update_pending);
        if pending {
            menus::REBUILD
        } else {
            menus::BUILD
        }
    }

    /// Reads one output line of the build: progress, the package-holds-DLC line; what is shown.
    pub(super) fn build_line(&mut self, line: String) -> Option<String> {
        if let Some((phase, c, t)) = output_lines::progress(&line) {
            if self.build.as_ref().is_some_and(|b| !b.job.cancelled()) {
                self.set_progress(&phase, c, t);
            }
        } else if output_lines::reports_package_dlc(&line)
            && let Some(b) = self.build.as_mut()
        {
            b.holds_dlc = true;
        }
        if line.starts_with('@') {
            None
        } else {
            Some(line)
        }
    }
}
