//! What the selected game's commands can do now (`refreshSelection`): the menus, the context menu
//! and the toolbar read these.

use crate::constants::menus as m;
use crate::flows::App;

pub struct Enabled {
    /// Play (it asks Native or Web): either can run now
    pub play: bool,
    /// Build (it asks Native or Web): either can build now
    pub build: bool,
    pub stop: bool,
    pub stop_label: &'static str,
    pub exists: bool,
    pub remove: bool,
    pub per_game: bool,
    pub save_folder: bool,
    pub log: bool,
}

pub fn enabled(app: &App) -> Enabled {
    let id = app.selected_id();
    let f = id.as_deref().map(|i| app.facts_of(i)).unwrap_or_default();
    let (playing, building) = id
        .as_deref()
        .map_or((false, false), |i| (app.is_running(i), app.is_building(i)));
    let exists = id.is_some();
    let can = |build| id.as_deref().is_some_and(|i| app.can_target(i, build));
    Enabled {
        play: can(false),
        build: can(true),
        stop: building || playing,
        stop_label: if building { m::CANCEL_BUILD } else { m::STOP },
        exists,
        remove: exists && !playing && !building,
        per_game: exists && !app.demo,
        save_folder: exists && f.has_build,
        log: app.has_log(),
    }
}
