//! One Play and one Build (owner, 2026-11-27): each asks Native or Web. `why_not` is the one rule for
//! whether a target can run now (the menus, the card, Enter and the chooser all read it) and, when it
//! cannot, the line the chooser shows in its place.

use super::App;
use super::surface::Surface;
use crate::constants::{menus as m, words_target as w};

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
    /// or Web only when both can (owner, 2026-11-27); Build opens the chooser when either can build.
    pub fn ask_target(&mut self, build: bool) {
        let Some(id) = self.selected_id() else { return };
        if !self.can_target(&id, build) || self.has_target_chooser() {
            return;
        }
        let web = self.why_not(&id, build, true).is_none();
        let native = self.why_not(&id, build, false).is_none();
        match (build, native, web) {
            (false, true, false) => self.play(),
            (false, false, true) => self.play_in_browser(),
            _ => self.surfaces.push(Surface::Target { game_id: id, build }),
        }
    }

    fn has_target_chooser(&self) -> bool {
        self.surfaces
            .iter()
            .any(|s| matches!(s, Surface::Target { .. }))
    }

    /// The card's primary for a game with no native build to play but a web build that can: Play.
    pub(super) fn card_plays_web(&self, id: &str, action: &str) -> bool {
        (action == m::BUILD || action == m::REBUILD) && self.why_not(id, false, true).is_none()
    }
}
