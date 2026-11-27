//! Properties and Add game (`editGame`, `showProperties`): a Draft of the game's fields; Apply checks the
//! title, the build folder and the artwork, then copies what changed into the entry, writes a built
//! game's screen mode and layout into its INI now and syncs removed DLC.

use super::App;
use super::surface::Picks;
use crate::constants::words_settings as w;
use crate::model::game::Game;
use crate::model::game_update::GameUpdate;
use crate::model::names::output_name_for;
use crate::model::path_text::{absolute, display, from_native, native};
use crate::services::artwork_decode;
#[derive(Clone, Debug, PartialEq)]
pub struct PropertiesDraft {
    pub game: Game,
    pub adding: bool,
    /// running or building: Build and Runtime are read-only
    pub locked: bool,
    pub tab: usize,
    pub title: String,
    pub source: String,
    pub build_dir: String,
    pub output_name: String,
    pub workers: i64,
    pub button_layout: String,
    pub screen_mode: String,
    pub cover: String,
    pub background: String,
    pub dlc: Vec<String>,
    pub updates: Vec<GameUpdate>,
    pub dlc_picks: Picks,
    pub update_picks: Picks,
    pub status: String,
}

impl PropertiesDraft {
    pub fn new(app: &App, game: Game, tab: usize, adding: bool) -> PropertiesDraft {
        let tabs = if adding { 4 } else { 6 };
        PropertiesDraft {
            adding,
            locked: app.busy(&game.id),
            tab: tab.min(tabs - 1),
            title: game.title.clone(),
            source: if game.source.is_empty() {
                String::new()
            } else {
                native(&game.source)
            },
            build_dir: display(&game.build_dir),
            output_name: game.output_name.clone(),
            workers: game.workers,
            button_layout: game.button_layout.clone(),
            screen_mode: game.screen_mode.clone(),
            cover: game.cover.clone(),
            background: game.background.clone(),
            dlc: game.dlc.clone(),
            updates: game.updates.clone(),
            dlc_picks: Picks::default(),
            update_picks: Picks::default(),
            status: if adding {
                w::NOT_BUILT.into()
            } else {
                app.status_of(&game)
            },
            game,
        }
    }

    /// The game as edited, or (title, text) of the refusal.
    pub fn accept(&self) -> Result<Game, (String, String)> {
        // an imported web build alone has no build folder (nothing is built for it)
        let needs_folder = self.game.web_build.is_empty() || !self.source.trim().is_empty();
        if self.title.trim().is_empty() || (needs_folder && self.build_dir.trim().is_empty()) {
            return Err((w::MISSING_TITLE.into(), w::MISSING.into()));
        }
        for (new, old) in [
            (self.cover.trim(), &self.game.cover),
            (self.background.trim(), &self.game.background),
        ] {
            if !new.is_empty() && new != old {
                artwork_decode::decode(new)
                    .map_err(|e| (w::ARTWORK_TITLE.to_string(), format!("{e}\n{new}")))?;
            }
        }
        let mut g = self.game.clone();
        g.title = self.title.trim().into();
        g.source = if self.source.trim().is_empty() {
            String::new()
        } else {
            absolute(self.source.trim())
        };
        g.build_dir = if self.build_dir.trim().is_empty() {
            String::new()
        } else {
            absolute(self.build_dir.trim())
        };
        if self.adding {
            g.output_name = output_name_for(&self.output_name);
        }
        g.workers = self.workers.clamp(0, 64);
        if g.nxrs() {
            g.renderer = "vulkan".into();
        }
        (g.button_layout, g.screen_mode) = (self.button_layout.clone(), self.screen_mode.clone());
        (g.cover, g.background) = (
            from_native(self.cover.trim()),
            from_native(self.background.trim()),
        );
        if !self.adding {
            (g.dlc, g.updates) = (self.dlc.clone(), self.updates.clone());
        }
        Ok(g)
    }
}
