//! What the library shows and in which order, the selection, and the status bar's lines
//! (`refreshLibrary`, `selectGame`, `refreshSelection`).

use super::App;
use crate::constants::words_library as w;
use crate::model::card_state::{CardState, card_state, status_for};
use crate::model::fill::{fill, plural};
use crate::model::game::Game;
use crate::services::build_facts;
use std::cmp::Ordering;

impl App {
    pub fn status_of(&self, g: &Game) -> String {
        status_for(
            &self.facts_of(&g.id),
            self.demo,
            self.is_building(&g.id),
            self.is_running(&g.id),
        )
        .to_string()
    }
    pub fn card_of(&self, g: &Game) -> CardState {
        let mut s = card_state(
            &self.facts_of(&g.id),
            self.is_running(&g.id),
            self.is_building(&g.id),
            self.builder_busy(),
            self.tools.can_build(),
        );
        s.action_enabled &= !self.demo;
        if self.card_plays_web(&g.id, &s.action) {
            s.action = crate::constants::menus::PLAY.into();
            s.action_enabled = true;
        }
        s
    }

    fn compare(&self, a: &Game, b: &Game, by: usize) -> Ordering {
        match by {
            1 => self.status_of(a).cmp(&self.status_of(b)),
            2 => a.format().cmp(b.format()),
            3 => a.renderer.cmp(&b.renderer),
            4 => a.last_played.cmp(&b.last_played),
            5 => a.build_dir.to_lowercase().cmp(&b.build_dir.to_lowercase()),
            _ => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        }
    }

    /// The games the filters let through, in the chosen order.
    pub fn visible_games(&self) -> Vec<Game> {
        let q = self.filters.query();
        let mut games: Vec<Game> = self
            .lib
            .games
            .iter()
            .filter(|g| {
                q.matches(
                    g,
                    &self.facts_of(&g.id),
                    self.is_running(&g.id),
                    self.is_building(&g.id),
                )
            })
            .cloned()
            .collect();
        let title = |a: &Game, b: &Game| a.title.to_lowercase().cmp(&b.title.to_lowercase());
        match self.filters.sort {
            Some(1) => {
                games.sort_by(|a, b| b.last_played.cmp(&a.last_played).then_with(|| title(a, b)))
            }
            Some(2) => games.sort_by(|a, b| self.compare(a, b, 1).then_with(|| title(a, b))),
            Some(_) => games.sort_by(title),
            None => {
                let (col, asc) = self.table_sort;
                games.sort_by(|a, b| {
                    if asc {
                        self.compare(a, b, col)
                    } else {
                        self.compare(b, a, col)
                    }
                });
            }
        }
        games
    }

    /// A column header clicked: that column, the other way round when it is already the sort. The
    /// popover's Sort follows when it is one of its presets ("Custom table order" otherwise).
    pub fn sort_by_column(&mut self, col: usize) {
        let asc = if self.table_sort.0 == col {
            !self.table_sort.1
        } else {
            col != 4
        };
        self.table_sort = (col, asc);
        self.filters.sort = match (col, asc) {
            (0, true) => Some(0),
            (4, false) => Some(1),
            (1, true) => Some(2),
            _ => None,
        };
    }
    /// The popover's Sort chosen: the list's column follows.
    pub fn sort_preset_changed(&mut self) {
        self.table_sort = match self.filters.sort {
            Some(1) => (4, false),
            Some(2) => (1, true),
            _ => (0, true),
        };
    }

    /// The selection is kept; when it is gone (filtered out, removed) the first game shown is chosen.
    pub fn fix_selection(&mut self) {
        let visible = self.visible_games();
        if !visible.iter().any(|g| g.id == self.selected) {
            self.selected = visible.first().map(|g| g.id.clone()).unwrap_or_default();
        }
    }
    pub fn select(&mut self, id: &str) {
        self.selected = id.to_string();
    }
    /// `--select-title`: by display title, case-insensitive.
    pub fn select_title(&mut self, title: &str) {
        if let Some(g) = self
            .lib
            .games
            .iter()
            .find(|g| g.title.to_lowercase() == title.to_lowercase())
        {
            self.selected = g.id.clone();
        }
    }

    /// "n games", or "x / y games" while filters are on.
    pub fn count_text(&self) -> String {
        let total = self.lib.games.len();
        if self.filters.query().active_count() > 0 {
            fill(
                w::SHOWN_OF,
                &[&self.visible_games().len().to_string(), &total.to_string()],
            )
        } else {
            fill(w::GAMES, &[&total.to_string(), plural(total)])
        }
    }
    /// The selected game for the status bar: "title · status · format · vX · renderer".
    pub fn selection_text(&self) -> String {
        let Some(g) = self.selected_game() else {
            return w::NO_SELECTION.into();
        };
        let version = if self.demo {
            String::new()
        } else {
            build_facts::version(g)
        };
        let v = if version.is_empty() {
            String::new()
        } else {
            format!(" · v{version}")
        };
        format!(
            "{} · {} · {}{v} · {}",
            g.title,
            self.status_of(g),
            g.format(),
            g.renderer
        )
    }
}
