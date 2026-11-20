//! A game's INI on disk: what the launcher owns in it, written atomically, everything else kept.

use super::files::{read_text, write_atomic};
use crate::model::game::Game;
use crate::model::ini_text::{
    self, ASK_USER, BUTTON_LAYOUT, DOCKED, MUTE, PAUSE, RENDERER, SETTINGS_VERSION,
};
use crate::model::play::{PlaySettings, effective_button_layout, play_for};

pub fn write_key(ini: &str, (section, key): (&str, &str), value: &str) -> Result<(), String> {
    let text = read_text(ini).unwrap_or_default();
    write_atomic(
        ini,
        ini_text::set_key(&text, section, key, value).as_bytes(),
    )
}

fn value(ini: &str, (_, key): (&str, &str)) -> Option<String> {
    read_text(ini).and_then(|t| ini_text::value(&t, key))
}
/// The engine's default when the INI has none: OpenGL.
pub fn read_renderer(ini: &str) -> String {
    value(ini, RENDERER).unwrap_or_else(|| "opengl".into())
}
/// TV mode when the INI has no value.
pub fn read_docked(ini: &str) -> bool {
    value(ini, DOCKED).is_none_or(|v| v != "0")
}
pub fn read_pause(ini: &str) -> bool {
    value(ini, PAUSE).is_some_and(|v| v == "1")
}
pub fn read_mute(ini: &str) -> bool {
    value(ini, MUTE).is_some_and(|v| v == "1")
}
pub fn read_ask_user(ini: &str) -> bool {
    value(ini, ASK_USER).is_some_and(|v| v == "1")
}

fn flag(on: bool) -> &'static str {
    if on { "1" } else { "0" }
}

pub fn write_renderer(ini: &str, renderer: &str) -> Result<(), String> {
    if renderer != "vulkan" && renderer != "opengl" {
        return Err("Unknown renderer.".into());
    }
    write_key(ini, RENDERER, renderer)
}
pub fn write_button_layout(ini: &str, layout: &str) -> Result<(), String> {
    if layout != "switch" && layout != "xbox" {
        return Err("Unknown controller layout.".into());
    }
    write_key(ini, BUTTON_LAYOUT, layout)
}
/// The engine moves an INI without settings_version (or below 2) to TV mode once; a choice made
/// here is not one to move, so such an INI gets version 2. A version the engine wrote stays.
pub fn write_docked(ini: &str, docked: bool) -> Result<(), String> {
    let versioned = read_text(ini).is_some_and(|t| t.contains("settings_version="));
    write_key(ini, DOCKED, flag(docked))?;
    if versioned {
        Ok(())
    } else {
        write_key(ini, SETTINGS_VERSION, "2")
    }
}
pub fn write_pause(ini: &str, pause: bool) -> Result<(), String> {
    write_key(ini, PAUSE, flag(pause))
}
pub fn write_mute(ini: &str, mute: bool) -> Result<(), String> {
    write_key(ini, MUTE, flag(mute))
}
pub fn write_play_settings(ini: &str, play: &PlaySettings) -> Result<(), String> {
    write_pause(ini, play.pause)?;
    write_mute(ini, play.mute)?;
    write_docked(ini, play.docked)?;
    write_key(ini, ASK_USER, flag(play.ask_user))
}
/// Everything the launcher owns in a game's INI: at build, at Play and when a setting changes.
pub fn write_game_settings(
    game: &Game,
    launcher_layout: &str,
    play: &PlaySettings,
) -> Result<(), String> {
    write_renderer(&game.ini_path(), &game.renderer)?;
    write_button_layout(
        &game.ini_path(),
        &effective_button_layout(game, launcher_layout),
    )?;
    write_play_settings(&game.ini_path(), &play_for(game, play))
}

/// Writes to every built game; (failures, the last error) for the message.
pub fn write_all(
    games: &[Game],
    mut each: impl FnMut(&Game) -> Option<Result<(), String>>,
) -> (usize, String) {
    let (mut failed, mut error) = (0, String::new());
    for g in games {
        if let Some(Err(e)) = each(g) {
            failed += 1;
            error = e;
        }
    }
    (failed, error)
}
