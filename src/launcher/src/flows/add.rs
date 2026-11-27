//! Adding games: a package or folder (through the Add game window), an imported build, dropped files
//! (`importSource`, `importBuild`, `dropEvent`).

use super::App;
use super::properties::PropertiesDraft;
use super::surface::Surface;
use crate::constants::{words_contexts as c, words_library as w};
use crate::model::game::Game;
use crate::model::path_text::{absolute, same_path};
use crate::services::pickers::{self, Pick};
use crate::services::{library_ops, sources};

impl App {
    pub(super) fn pick_games(&mut self) {
        for f in pickers::files(&Pick {
            title: w::PICK_GAME,
            filter: Some(w::GAME_FILTER),
            start: "",
        }) {
            self.import_source(&f, true, "");
        }
    }
    pub(super) fn pick_folder(&mut self) {
        if let Some(d) = pickers::folder(&Pick {
            title: w::PICK_FOLDER,
            filter: None,
            start: "",
        }) {
            self.import_source(&d, true, "");
        }
    }
    pub(super) fn pick_build(&mut self) {
        let native: (&str, &[&str]) = (
            "Native games",
            if cfg!(windows) { &["exe"] } else { &["*"] },
        );
        if let Some(f) = pickers::file(&Pick {
            title: w::PICK_BUILD,
            filter: Some(native),
            start: "",
        }) {
            self.import_build(&f);
        }
    }

    /// A source: checked, then (interactive) the Add game window, else added as it is. `output` is
    /// `--build-output`. A source already in the library is selected.
    pub fn import_source(&mut self, source: &str, interactive: bool, output: &str) -> bool {
        if let Err(e) = sources::valid_source(source) {
            self.message(c::ADD, w::CANNOT_ADD, &e, true);
            return false;
        }
        let mut game = sources::game_from_source(source, &self.lib.output_root);
        if !output.is_empty() {
            game.build_dir = absolute(output);
        }
        if let Some(g) = self
            .lib
            .games
            .iter()
            .find(|g| same_path(&g.source, &game.source))
        {
            self.selected = g.id.clone();
            return true;
        }
        if interactive {
            let draft = PropertiesDraft::new(self, game, 0, true);
            self.surfaces.push(Surface::Properties(Box::new(draft)));
            return true;
        }
        self.add_game(game, false)
    }

    /// The Add game window's Add: refused for a duplicate source or executable.
    pub fn add_game(&mut self, game: Game, interactive: bool) -> bool {
        let id = game.id.clone();
        if !library_ops::add(&mut self.lib, game) {
            if interactive {
                self.message(c::ADD, w::ALREADY_TITLE, w::ALREADY, false);
            }
            return false;
        }
        self.selected = id;
        self.save();
        self.refresh_facts();
        true
    }

    pub fn import_build(&mut self, executable: &str) -> bool {
        let game = match sources::game_from_executable(executable) {
            Ok(g) => g,
            Err(e) => {
                self.message(c::IMPORT, w::CANNOT_IMPORT, &e, true);
                return false;
            }
        };
        if self
            .lib
            .games
            .iter()
            .any(|g| same_path(&g.source, &game.source) && self.busy(&g.id))
        {
            self.message(c::IMPORT, w::BUSY_TITLE, w::BUSY_IMPORT, false);
            return false;
        }
        self.selected = library_ops::import_build(&mut self.lib, &game);
        self.save();
        self.refresh_facts();
        true
    }

    /// Files dropped on the window: native builds are imported, anything else is added.
    pub fn drop_files(&mut self, files: &[String]) {
        for path in files {
            if sources::game_from_executable(path).is_ok() {
                self.import_build(path);
            } else {
                self.import_source(path, true, "");
            }
        }
    }
}
