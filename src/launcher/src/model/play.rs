//! Settings > Play and the controller layout: global values, with a game's own layout and screen mode
//! over them.

use super::game::Game;

/// The player's settings, written into every game's INI (the screen mode can be a game's own).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaySettings {
    pub pause: bool,
    pub mute: bool,
    pub docked: bool,
    pub ask_user: bool,
}
impl Default for PlaySettings {
    fn default() -> Self {
        PlaySettings {
            pause: false,
            mute: false,
            docked: true,
            ask_user: false,
        }
    }
}
impl PlaySettings {
    pub fn new(pause: bool, mute: bool, docked: bool, ask_user: bool) -> Self {
        PlaySettings {
            pause,
            mute,
            docked,
            ask_user,
        }
    }
}

pub fn valid_button_layout(layout: &str) -> bool {
    layout.is_empty() || layout == "switch" || layout == "xbox"
}
/// "switch"/"xbox" for those values and their engine aliases; empty otherwise.
pub fn normalize_button_layout(layout: &str) -> String {
    match layout {
        "switch" | "nintendo" | "position" | "positions" => "switch".into(),
        "xbox" | "label" | "labels" => "xbox".into(),
        _ => String::new(),
    }
}
pub fn effective_button_layout(game: &Game, launcher_default: &str) -> String {
    let own = normalize_button_layout(&game.button_layout);
    if !own.is_empty() {
        return own;
    }
    if normalize_button_layout(launcher_default) == "xbox" {
        "xbox"
    } else {
        "switch"
    }
    .into()
}
/// "tv" / "handheld" for those values (and docked / portable); empty otherwise.
pub fn normalize_screen_mode(mode: &str) -> String {
    match mode.trim().to_lowercase().as_str() {
        "tv" | "docked" => "tv".into(),
        "handheld" | "portable" => "handheld".into(),
        _ => String::new(),
    }
}
pub fn effective_docked(game: &Game, launcher_docked: bool) -> bool {
    let own = normalize_screen_mode(&game.screen_mode);
    if own.is_empty() {
        launcher_docked
    } else {
        own == "tv"
    }
}
/// The settings a game is played with: the launcher's, with its own screen mode.
pub fn play_for(game: &Game, launcher: &PlaySettings) -> PlaySettings {
    PlaySettings {
        docked: effective_docked(game, launcher.docked),
        ..*launcher
    }
}
