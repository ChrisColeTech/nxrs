//! Import build given a web build (owner, 2026-11-27): the one Import build action, `--import-build`
//! and drag and drop take an nxrs web build or another web game as well as a native build. Play then
//! serves it from the launcher's web server and opens it in the browser (`flows::web`).

use super::App;
use crate::model::path_text::{parent, same_path};
use crate::model::web_build::{imported_page, page_parts};
use crate::services::kept_saves;
use crate::services::web_import::{self, Found};

impl App {
    /// Adds the web build found at `picked`: the entry that has it already is selected; an nxrs web
    /// build joins the entry of the game it was built from (that entry then plays Native or Web);
    /// anything else becomes a new entry. The id of the entry.
    pub(super) fn import_web(&mut self, found: &Found, picked: &str) -> String {
        let place = match found {
            Found::Nxrs(dir) | Found::Page(dir) => dir.clone(),
        };
        // the entry that has this build already (a web game: the same site folder) is selected; a
        // game's start page found anew replaces the one it had
        let site = |p: &str| parent(page_parts(p).0);
        if let Some(g) = self.lib.games.iter_mut().find(|g| {
            same_path(&g.web_build, &place)
                || (matches!(found, Found::Page(_))
                    && imported_page(g).is_some()
                    && same_path(&site(&g.web_build), &site(&place)))
        }) {
            let changed = g.web_build != place;
            g.web_build = place;
            self.selected = g.id.clone();
            if changed {
                self.save();
                self.refresh_facts();
            }
            return self.selected.clone();
        }
        let tid = web_import::title_id(found);
        let same_game = self.lib.games.iter_mut().find(|g| {
            g.web_build.is_empty()
                && tid.is_some()
                && kept_saves::title_id(&g.data_dir()).as_deref() == tid.as_deref()
        });
        let id = match same_game {
            Some(g) => {
                g.web_build = place;
                g.id.clone()
            }
            None => {
                let game = web_import::game_for(found, picked);
                let id = game.id.clone();
                self.lib.games.push(game);
                id
            }
        };
        self.selected = id.clone();
        self.save();
        self.refresh_facts();
        id
    }
}
