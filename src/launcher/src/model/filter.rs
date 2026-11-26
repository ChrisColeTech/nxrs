//! The library's search and filters (`LibraryFilter`).

use super::facts::Facts;
use super::game::{Game, in_place_package};
use super::path_text::ends_with_ci;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryFilter {
    pub text: String,
    /// All, Ready, Needs build, Running
    pub state: usize,
    /// All, NUP/NSP packages, NRO, extracted, native builds
    pub source: usize,
}

impl LibraryFilter {
    pub fn matches(&self, game: &Game, facts: &Facts, running: bool, building: bool) -> bool {
        if !game
            .title
            .to_lowercase()
            .contains(&self.text.trim().to_lowercase())
        {
            return false;
        }
        let package = in_place_package(&game.source);
        let nro = ends_with_ci(&game.source, ".nro");
        let source_ok = match self.source {
            1 => package,
            2 => nro,
            3 => !(package || nro || game.source.is_empty()),
            4 => game.source.is_empty(),
            _ => true,
        };
        if !source_ok {
            return false;
        }
        let ready = facts.playable();
        match self.state {
            1 => ready && !running && !building && !facts.source_offline(),
            2 => !(ready || building || running),
            3 => running,
            _ => true,
        }
    }
    pub fn active_count(&self) -> usize {
        usize::from(!self.text.trim().is_empty())
            + usize::from(self.state != 0)
            + usize::from(self.source != 0)
    }
}
