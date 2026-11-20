//! The menus' entries and their enabled states (`createMenusAndToolbar`, `refreshSelection`,
//! `showContextMenu`).

use crate::constants::menus as m;
use crate::flows::{Action, App};

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Separator,
    Command {
        label: String,
        shortcut: &'static str,
        enabled: bool,
        checked: Option<bool>,
        action: Action,
    },
}

fn cmd(label: impl Into<String>, shortcut: &'static str, enabled: bool, action: Action) -> Item {
    Item::Command {
        label: label.into(),
        shortcut,
        enabled,
        checked: None,
        action,
    }
}
fn check(label: &str, checked: bool, action: Action) -> Item {
    Item::Command {
        label: label.into(),
        shortcut: "",
        enabled: true,
        checked: Some(checked),
        action,
    }
}

/// What the selected game's commands can do now.
pub struct Enabled {
    pub play: bool,
    pub build: bool,
    pub stop: bool,
    pub stop_label: &'static str,
    pub exists: bool,
    pub remove: bool,
    pub per_game: bool,
    pub save_folder: bool,
    pub copy_saves: bool,
    pub log: bool,
}

pub fn enabled(app: &App) -> Enabled {
    let id = app.selected_id();
    let f = id.as_deref().map(|i| app.facts_of(i)).unwrap_or_default();
    let (playing, building) = id
        .as_deref()
        .map_or((false, false), |i| (app.is_running(i), app.is_building(i)));
    let exists = id.is_some();
    Enabled {
        play: exists && !app.demo && f.playable() && !playing && !building,
        build: exists
            && !app.demo
            && !app.builder_busy()
            && !playing
            && f.source_available
            && app.tools.can_build()
            && !f.playable(),
        stop: building || playing,
        stop_label: if building { m::CANCEL_BUILD } else { m::STOP },
        exists,
        remove: exists && !playing && !building,
        per_game: exists && !app.demo,
        save_folder: exists && f.has_build,
        copy_saves: app.can_copy_saves(),
        log: app.has_log(),
    }
}

pub fn filter_label(app: &App) -> String {
    let n = app.filters.query().active_count();
    if n > 0 {
        format!("{} ({n})", m::FILTER)
    } else {
        m::FILTER.into()
    }
}

pub fn menu(app: &App, name: &str) -> Vec<Item> {
    let e = enabled(app);
    match name {
        "File" => vec![
            cmd(m::ADD_GAME, m::KEY_ADD, true, Action::AddGame),
            cmd(m::ADD_FOLDER, "", true, Action::AddFolder),
            cmd(m::IMPORT_BUILD, "", true, Action::ImportBuildPick),
            Item::Separator,
            cmd(m::SETTINGS, "", true, Action::Settings(0)),
            Item::Separator,
            cmd(m::EXIT, m::KEY_EXIT, true, Action::Exit),
        ],
        "Game" => vec![
            cmd(m::PLAY, "", e.play, Action::Play),
            cmd(e.stop_label, "", e.stop, Action::Stop),
            cmd(app.build_label(), m::KEY_BUILD, e.build, Action::Build),
            Item::Separator,
            cmd(m::INSTALL_DLC, "", e.per_game, Action::InstallDlc),
            cmd(m::INSTALL_UPDATES, "", e.per_game, Action::InstallUpdates),
            cmd(m::COPY_SAVES, "", e.copy_saves, Action::CopySaves),
            Item::Separator,
            cmd(m::REMOVE, "", e.remove, Action::RemoveGame),
        ],
        "Keys" => vec![
            cmd(m::INSTALL_KEYS, "", true, Action::InstallKeys),
            cmd(m::KEYS_FOLDER, "", true, Action::OpenKeysFolder),
        ],
        "View" => vec![
            check(m::GALLERY, app.lib.view_mode == 0, Action::ViewMode(0)),
            check(m::LIST, app.lib.view_mode == 1, Action::ViewMode(1)),
            Item::Separator,
            cmd(
                filter_label(app),
                m::KEY_FILTER,
                true,
                Action::ToggleFilters,
            ),
            check(
                m::BACKGROUND,
                app.lib.show_background,
                Action::ToggleBackground,
            ),
            cmd(m::REFRESH, m::KEY_REFRESH, true, Action::Refresh),
            Item::Separator,
            cmd(m::CONSOLE, m::KEY_CONSOLE, true, Action::ViewConsole),
            check(
                m::OUTPUT_TOGGLE,
                app.lib.console.visible,
                Action::ToggleConsole,
            ),
        ],
        "Help" => vec![cmd(m::ABOUT, "", true, Action::About)],
        _ => Vec::new(),
    }
}

/// The game context menu of the gallery and the list.
pub fn context(app: &App) -> Vec<Item> {
    let e = enabled(app);
    vec![
        cmd(m::PLAY, "", e.play, Action::Play),
        cmd(app.build_label(), m::KEY_BUILD, e.build, Action::Build),
        cmd(
            m::PROPERTIES,
            m::KEY_PROPERTIES,
            e.exists,
            Action::Properties(0),
        ),
        Item::Separator,
        cmd(m::BUILD_FOLDER, "", e.exists, Action::OpenBuildFolder),
        cmd(m::SAVE_FOLDER, "", e.save_folder, Action::OpenSaveFolder),
        cmd(m::GAME_LOG, "", e.log, Action::ViewGameLog),
        Item::Separator,
        cmd(m::REMOVE, "", e.remove, Action::RemoveGame),
    ]
}

/// The Add game button's drop-down.
pub fn add_menu() -> Vec<Item> {
    vec![
        cmd(m::ADD_GAME, m::KEY_ADD, true, Action::AddGame),
        cmd(m::ADD_FOLDER, "", true, Action::AddFolder),
        cmd(m::IMPORT_BUILD, "", true, Action::ImportBuildPick),
    ]
}
