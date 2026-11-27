//! One Play and one Build (owner, 2026-11-27). `why_not` is the one rule for whether a version,
//! Native or Web, can play or build now (the menus, the card, Enter and the Choices box all read
//! it) and, when it cannot, why (its disabled button's tooltip).

use super::surface::{Ask, Choice, Surface};
use super::{Action, App};
use crate::constants::{menus as m, words_contexts as c, words_target as w};
use crate::model::fill::fill;

impl App {
    /// Why Play (`build` false) or Build of `id` cannot go to Native or Web (`web`) now; None: it can.
    pub fn why_not(&self, id: &str, build: bool, web: bool) -> Option<String> {
        if self.lib.find(id).is_none() || self.demo {
            return Some(w::PREVIEW.into());
        }
        let f = self.facts_of(id);
        let (running, building) = (self.is_running(id), self.is_building(id));
        let reason = match (build, web) {
            (false, false) if building => w::BUILDING,
            (false, false) if running => w::RUNNING,
            (false, false) if !f.playable() => w::NO_NATIVE_BUILD,
            (false, true) if !f.web_build => w::NO_WEB_BUILD,
            (false, true) if self.tools.browser.is_none() => w::NO_BROWSER,
            (false, _) => return None,
            (true, _) if self.builder_busy() => w::OTHER_BUILD,
            (true, _) if !f.source_available => w::NO_SOURCE,
            (true, _) if !self.tools.can_build() => return Some(self.tools.problem()),
            (true, false) if running => w::RUNNING,
            (true, false) if f.playable() => w::UP_TO_DATE,
            (true, _) => return None,
        };
        Some(reason.into())
    }

    /// Play or Build of `id` can go somewhere now: the command is enabled.
    pub fn can_target(&self, id: &str, build: bool) -> bool {
        [false, true]
            .into_iter()
            .any(|web| self.why_not(id, build, web).is_none())
    }

    /// Play or Build of the selected game. Play runs the one version that can play, and asks Native
    /// or Web only when both can (owner, 2026-11-27); Build asks when either can build, a version
    /// that cannot disabled with its reason.
    pub fn ask_target(&mut self, build: bool) {
        let Some(id) = self.selected_id() else { return };
        if !self.tools_checked {
            // which versions can run depends on the browser and nxrs-build found: pressed before
            // the check ended, it is finished now (else Play would run native without asking)
            self.refresh_tools_now();
        }
        if !self.can_target(&id, build) || self.asking_target() {
            return;
        }
        let web = self.why_not(&id, build, true).is_none();
        let native = self.why_not(&id, build, false).is_none();
        match (build, native, web) {
            (false, true, false) => self.play(),
            (false, false, true) => self.play_in_browser(),
            _ => {
                let surface = self.target_choices(&id, build);
                self.surfaces.push(surface);
            }
        }
    }

    /// The Choices box: the game in its heading, Native then Web as equal buttons, one that
    /// cannot run disabled with its reason.
    fn target_choices(&self, id: &str, build: bool) -> Surface {
        let game = self.lib.find(id).map_or("", |g| g.title.as_str());
        let browser = self.tools.browser.as_ref().map_or("", |b| b.name.as_str());
        let (title, heading, text) = if build {
            (c::BUILD, w::BUILD_HEADING, w::BUILD_TEXT.to_string())
        } else {
            (c::PLAY, w::PLAY_HEADING, fill(w::PLAY_TEXT, &[browser]))
        };
        let option = |web: bool, label: &str| Choice {
            label: label.into(),
            action: answer(build, web),
            why_not: self.why_not(id, build, web).unwrap_or_default(),
        };
        Surface::Choices {
            ask: Ask::new(title, &fill(heading, &[game]), &text, ""),
            options: vec![option(false, w::NATIVE), option(true, w::WEB)],
        }
    }

    /// Play's or Build's choice is open (a second press does not stack another).
    fn asking_target(&self) -> bool {
        self.surfaces
            .iter()
            .any(|s| matches!(s, Surface::Choices { .. }))
    }

    /// The card's primary for a game with no native build to play but a web build that can: Play.
    pub(super) fn card_plays_web(&self, id: &str, action: &str) -> bool {
        (action == m::BUILD || action == m::REBUILD) && self.why_not(id, false, true).is_none()
    }
}

/// What Play or Build answered Native or Web runs.
fn answer(build: bool, web: bool) -> Action {
    match (build, web) {
        (false, false) => Action::PlayNative,
        (false, true) => Action::PlayWeb,
        (true, false) => Action::BuildNative,
        (true, true) => Action::BuildWeb,
    }
}
