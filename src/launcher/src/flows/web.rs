//! Build for web and Play in browser (docs/35 W5): the build runs like any other through contract v2
//! (`--target web`, into the game's web folder); Play serves that folder from the launcher's own web
//! server and opens it in a Chromium browser.

use super::runs::BuildRun;
use super::{App, Outcome};
use crate::constants::{web as c, words_build as w, words_web as ww};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::path_text::join;
use crate::model::web_build::web_dir;
use crate::services::browser;
use crate::services::web_serve::{self, WebServer};

impl App {
    pub fn start_web_build(&mut self) {
        self.start_build_kind(true);
    }

    /// The engine's log of a web build, in its web folder.
    pub(super) fn web_build_log(g: &Game) -> String {
        join(
            &web_dir(g),
            &format!("{}{}", g.output_name, c::BUILD_LOG_SUFFIX),
        )
    }

    /// A web build's end: its folder checked; the native build and the library entry stay as they were.
    pub(super) fn finish_web_build(&mut self, run: BuildRun, code: Option<i32>) {
        let done = code == Some(0) && web_serve::has_web_build(&web_dir(&run.game));
        let detail = if run.job.cancelled() {
            w::CANCELLED.to_string()
        } else if code != Some(0) {
            fill(w::FAILED, &[&code.unwrap_or(-1).to_string()])
        } else if done {
            ww::DONE.to_string()
        } else {
            ww::MISSING_FILES.to_string()
        };
        let ok = done && !run.job.cancelled();
        run.job.log_line(&detail);
        self.console.push(format!("\n{detail}"));
        self.console.title = if ok {
            w::OUTPUT_COMPLETE
        } else {
            w::OUTPUT_STOPPED
        }
        .into();
        self.bar = None;
        self.job_label = if ok { ww::JOB_DONE } else { w::JOB_STOPPED }.into();
        self.refresh_facts();
        if self.closing {
            self.close_now = true;
        }
        self.outcomes.push(Outcome::Smoke(ok));
    }

    /// Play in browser is offered: a finished web build and a browser that can play it.
    pub fn can_play_web(&self, id: &str) -> bool {
        !self.demo && self.tools.browser.is_some() && self.facts_of(id).web_build
    }

    /// Serves the selected game's web build (the server starts at the first Play) and opens it.
    pub fn play_in_browser(&mut self) {
        let Some(id) = self.selected_id() else { return };
        let (Some(game), Some(b)) = (self.lib.find(&id).cloned(), self.tools.browser.clone())
        else {
            return;
        };
        if !self.can_play_web(&id) {
            return;
        }
        match self
            .serve(&game)
            .and_then(|url| browser::open(&b, &url).map(|_| url))
        {
            Ok(url) => self.job_label = fill(ww::OPENED, &[&b.name, &url]),
            Err(err) => self.message(ww::NOT_SERVED_TITLE, &err, true),
        }
    }

    /// Mounts the game's web folder on the server (started now if it is not running); its address.
    fn serve(&mut self, game: &Game) -> Result<String, String> {
        let (_, mut mount) =
            web_serve::open_build(&web_dir(game)).map_err(|e| format!("{}{e}", ww::BAD_BUILD))?;
        // the page's run-time shaders are translated by the engine the launcher builds with
        mount.translator = self.tools.nxrs_build.clone();
        if self.web_server.is_none() {
            let server = WebServer::start(web_serve::wanted_port())?;
            if let Some(note) = server.moved_note() {
                self.console.push(note);
            }
            self.web_server = Some(server);
        }
        match &self.web_server {
            Some(s) => s.mount(&game.output_name, mount),
            None => Err(ww::NO_SERVER.into()),
        }
    }
}
