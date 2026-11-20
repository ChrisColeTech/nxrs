//! The library: its games and the launcher's settings (`library.json`, schema 1).

use super::game::Game;
use super::play::PlaySettings;
use serde_json::{Map, Value};

/// Where this launcher's window was (a key only it writes).
#[derive(Clone, Debug, PartialEq)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

/// The build output panel: shown, docked at the bottom or the right, its size.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsolePanel {
    pub visible: bool,
    pub right: bool,
    /// popped out into a window of its own
    pub floating: bool,
    pub size: f32,
}
impl Default for ConsolePanel {
    fn default() -> Self {
        ConsolePanel {
            visible: false,
            right: false,
            floating: false,
            size: crate::constants::layout::CONSOLE_DEFAULT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    pub path: String,
    /// The Turbo engine folder the Qt launcher kept: read and written back, not used.
    pub tool_root: String,
    pub tool_root_chosen: bool,
    pub output_root: String,
    pub keys_path: String,
    pub nxrs_build: String,
    pub jobs: i64,
    pub view_mode: i64,
    pub cover_width: i64,
    pub show_background: bool,
    pub color_mode: String,
    pub accent_color: String,
    pub button_layout: String,
    pub pause_on_focus_loss: bool,
    pub mute_on_focus_loss: bool,
    pub docked: bool,
    pub ask_user: bool,
    /// the Qt launcher's window blobs, kept byte for byte
    pub geometry: Vec<u8>,
    pub dock_state: Vec<u8>,
    pub window: Option<WindowPlacement>,
    pub console: ConsolePanel,
    pub games: Vec<Game>,
    pub extra: Map<String, Value>,
}

impl Library {
    pub fn new(path: &str) -> Library {
        Library {
            path: path.to_string(),
            tool_root: String::new(),
            tool_root_chosen: false,
            output_root: String::new(),
            keys_path: String::new(),
            nxrs_build: String::new(),
            jobs: 8,
            view_mode: 0,
            cover_width: 180,
            show_background: true,
            color_mode: "dark".into(),
            accent_color: "lime".into(),
            button_layout: "switch".into(),
            pause_on_focus_loss: false,
            mute_on_focus_loss: false,
            docked: true,
            ask_user: false,
            geometry: Vec::new(),
            dock_state: Vec::new(),
            window: None,
            console: ConsolePanel::default(),
            games: Vec::new(),
            extra: Map::new(),
        }
    }
    pub fn find(&self, id: &str) -> Option<&Game> {
        self.games.iter().find(|g| g.id == id)
    }
    pub fn find_mut(&mut self, id: &str) -> Option<&mut Game> {
        self.games.iter_mut().find(|g| g.id == id)
    }
    pub fn play(&self) -> PlaySettings {
        PlaySettings::new(
            self.pause_on_focus_loss,
            self.mute_on_focus_loss,
            self.docked,
            self.ask_user,
        )
    }
    pub fn play_for(&self, game: &Game) -> PlaySettings {
        super::play::play_for(game, &self.play())
    }
}
