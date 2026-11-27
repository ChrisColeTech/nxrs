//! What the disk says about a game, gathered once (`services::build_facts`): the statuses, cards, menus
//! and filters decide from these.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    /// its build is there and is the one the entry asks for (`Game::hasBuild`)
    pub has_build: bool,
    /// built, but with another update than the one chosen now
    pub update_pending: bool,
    /// its source exists
    pub source_available: bool,
    /// its build reads its source package while it plays (an in-place marker, from before builds carried
    /// their data); a build with its own copy of the data never needs the source
    pub in_place: bool,
    pub build_incomplete: bool,
    /// its web folder holds a finished web build (`build.json` and the module): Play in browser
    pub web_build: bool,
}

impl Facts {
    /// Play can start it: its build is there.
    pub fn playable(&self) -> bool {
        self.has_build
    }
    /// A built game whose in-place package cannot be reached: one rule for the cards and the list.
    pub fn source_offline(&self) -> bool {
        self.in_place && !self.source_available
    }
}
