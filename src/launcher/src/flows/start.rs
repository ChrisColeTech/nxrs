//! Opening the library (`Window::Window`), and what every command shares: the selected game, whether a
//! game is busy, what the disk says, saving, message and question boxes.

use super::surface::{Ask, Check, Surface};
use super::{Action, App};
use crate::constants::{engine as e, words_contexts as c, words_library as w};
use crate::model::facts::Facts;
use crate::model::game::Game;
use crate::services::library_file::{load, save};
use crate::services::{build_facts, data_dir, files, platform};

impl App {
    /// Loads the library; finds the installed keys; the demo library instead with `--demo`.
    pub fn open(&mut self) {
        let error = load(&mut self.lib, &mut self.store).err();
        let installed = data_dir::installed_keys();
        if self.lib.keys_path.is_empty() && files::is_file(&installed) {
            self.lib.keys_path = installed;
        }
        self.appearance = (self.lib.color_mode.clone(), self.lib.accent_color.clone());
        if self.demo {
            self.seed_demo();
        } else {
            self.running = platform::find_running_games(&self.lib.games);
        }
        if let Some(e) = error {
            self.message(c::LIBRARY, w::LOAD_FAILED, &e, true);
        }
        self.refresh_facts();
        self.fix_selection();
    }

    fn seed_demo(&mut self) {
        self.store.writable = false;
        (self.lib.view_mode, self.lib.cover_width) = (0, 175);
        let names = [
            "Donkey Kong Bananza",
            "Mario Kart World",
            "Homebrew collection",
            "Another game",
            "Test application",
            "Local collection",
        ];
        self.lib.games = names
            .iter()
            .enumerate()
            .map(|(i, name)| Game {
                id: i.to_string(),
                title: name.to_string(),
                output_name: name.to_string(),
                source: format!("Games/{name}{}", if i == 2 { ".nro" } else { ".nup" }),
                build_dir: format!("Games/Native builds/{name}"),
                engine: e::ENGINE_NXRS.into(),
                ..Game::default()
            })
            .collect();
        self.selected = "0".into();
    }

    pub fn selected_game(&self) -> Option<&Game> {
        self.lib.find(&self.selected)
    }
    pub fn selected_id(&self) -> Option<String> {
        self.selected_game().map(|g| g.id.clone())
    }
    pub fn is_building(&self, id: &str) -> bool {
        self.build.as_ref().is_some_and(|b| b.game.id == id)
    }
    pub fn builder_busy(&self) -> bool {
        self.build.is_some()
    }
    pub fn is_running(&self, id: &str) -> bool {
        self.running.contains_key(id)
    }
    /// Running or building: its build, DLC and updates cannot change.
    pub fn busy(&self, id: &str) -> bool {
        self.is_running(id) || self.is_building(id)
    }
    pub fn probe_busy(&self) -> bool {
        self.probe.is_some()
    }
    pub fn dlc_busy(&self) -> bool {
        self.dlc_run.is_some()
    }

    /// What the disk says about a game (cached; `refresh_facts` reads it again).
    pub fn facts_of(&self, id: &str) -> Facts {
        self.facts.get(id).copied().unwrap_or_else(|| {
            self.lib
                .find(id)
                .map(build_facts::facts)
                .unwrap_or_default()
        })
    }
    pub fn refresh_facts(&mut self) {
        self.facts = self
            .lib
            .games
            .iter()
            .map(|g| (g.id.clone(), build_facts::facts(g)))
            .collect();
    }

    pub fn save(&mut self) {
        if self.demo {
            return;
        }
        if let Err(e) = save(&self.lib, &mut self.store) {
            self.message(c::LIBRARY, w::SAVE_FAILED, &e, true);
        }
    }

    /// A message box titled by its `context` (the task it belongs to, `words_contexts`), `title` its
    /// heading; a command-line verb prints it instead.
    pub fn message(&mut self, context: &str, title: &str, text: &str, error: bool) {
        if !self.interactive {
            eprintln!(
                "{}{text}",
                if title.is_empty() {
                    String::new()
                } else {
                    format!("{title}: ")
                }
            );
            return;
        }
        self.surfaces.push(Surface::Message {
            context: context.into(),
            title: title.into(),
            text: text.into(),
            error,
            links: Vec::new(),
        });
    }
    /// A message with web pages to open (About): each a ghost button at its footer's leading side.
    pub fn message_with_links(
        &mut self,
        (context, title): (&str, &str),
        text: &str,
        links: &[(&str, &str)],
    ) {
        self.message(context, title, text, false);
        if !self.interactive {
            return; // printed; no box was opened
        }
        if let Some(Surface::Message { links: kept, .. }) = self.surfaces.last_mut() {
            *kept = links
                .iter()
                .map(|(l, u)| (l.to_string(), u.to_string()))
                .collect();
        }
    }
    /// A question: its one button (the verb) asks for `yes`; the X and Escape for `no`.
    pub fn question(&mut self, ask: Ask, yes: Action, no: Option<Action>, default_no: bool) {
        self.surfaces.push(Surface::Question {
            ask,
            yes,
            no,
            default_no,
            check: None,
        });
    }
    /// A question with a tick box: the verb asks for `check.yes_on` while it is ticked.
    pub fn question_checked(&mut self, ask: Ask, yes: Action, check: Check) {
        self.surfaces.push(Surface::Question {
            ask,
            yes,
            no: None,
            default_no: false,
            check: Some(check),
        });
    }
    pub fn surface_open(&self, test: impl Fn(&Surface) -> bool) -> bool {
        self.surfaces.iter().any(test)
    }
}
