//! What the selected game's commands can do now (`refreshSelection`): the menus, the context menu
//! and the toolbar read these.

use crate::constants::menus as m;
use crate::flows::App;

pub struct Enabled {
    pub play: bool,
    pub build: bool,
    pub stop: bool,
    pub stop_label: &'static str,
    pub exists: bool,
    pub remove: bool,
    pub per_game: bool,
    pub save_folder: bool,
    pub log: bool,
    /// Build for web: as Build, but an existing native or web build does not block it
    pub build_web: bool,
    /// Play in browser is shown at all (a web build and a Chromium browser exist)
    pub play_web: bool,
}

pub fn enabled(app: &App) -> Enabled {
    let id = app.selected_id();
    let f = id.as_deref().map(|i| app.facts_of(i)).unwrap_or_default();
    let (playing, building) = id
        .as_deref()
        .map_or((false, false), |i| (app.is_running(i), app.is_building(i)));
    let exists = id.is_some();
    let can_build =
        exists && !app.demo && !app.builder_busy() && f.source_available && app.tools.can_build();
    Enabled {
        play: exists && !app.demo && f.playable() && !playing && !building,
        build: can_build && !playing && !f.playable(),
        stop: building || playing,
        stop_label: if building { m::CANCEL_BUILD } else { m::STOP },
        exists,
        remove: exists && !playing && !building,
        per_game: exists && !app.demo,
        save_folder: exists && f.has_build,
        log: app.has_log(),
        build_web: can_build,
        play_web: id.as_deref().is_some_and(|i| app.can_play_web(i)),
    }
}
