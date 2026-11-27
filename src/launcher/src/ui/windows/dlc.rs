//! Game > Install DLC… (`DlcDialog`): the game's add-on content, changed at once: Add DLC… (or files
//! dropped on it) installs, Remove uninstalls the selected ones.

use super::Reply;
use super::list_window::{self, Buttons};
use crate::constants::words_dlc as w;
use crate::flows::App;
use crate::flows::surface::ListWindow;
use crate::model::path_text::parent;
use crate::ui::theme;
use egui::Ui;

pub fn hint(dlc_empty: bool, built: bool, locked: bool) -> &'static str {
    match (locked, dlc_empty, built) {
        (true, ..) => w::HINT_LOCKED,
        (_, true, _) => w::HINT_EMPTY,
        (_, _, true) => w::HINT_BUILT,
        _ => w::HINT_UNBUILT,
    }
}

pub fn show(ui: &mut Ui, app: &mut App, win: &mut ListWindow) -> Reply {
    let p = theme::of(ui.ctx());
    let Some(game) = app.lib.find(&win.game_id).cloned() else {
        return Reply {
            close: true,
            ..Reply::default()
        };
    };
    let (busy, built) = (app.dlc_busy(), app.facts_of(&game.id).has_build);
    let editable = !win.locked && !busy;
    let rows = app.dlc_rows(&game, &game.dlc);
    let packages = game.dlc.clone();
    let menu = |i: usize, n: usize| {
        let remove = if n > 1 {
            crate::model::fill::fill(w::REMOVE_MANY, &[&n.to_string()])
        } else {
            w::REMOVE.into()
        };
        vec![
            (remove, editable),
            (
                w::OPEN_FOLDER.into(),
                packages.get(i).is_some_and(|p| app.exists(p)),
            ),
        ]
    };
    // the state stays in view; how the list works is in the heading's info mark
    let tip = hint(game.dlc.is_empty(), built, false);
    crate::ui::widgets::page::group(ui, crate::ui::widgets::rail::Icon::Dlc, "DLC", tip);
    let empty = if game.dlc.is_empty() {
        w::LIST_EMPTY
    } else {
        ""
    };
    let lines = [if win.locked { w::HINT_LOCKED } else { "" }, empty];
    let buttons = Buttons {
        add: w::ADD,
        add_tip: w::ADD_TIP,
        remove_tip: w::REMOVE_TIP,
        build: None,
        lines: &lines,
    };
    let pressed = list_window::show(
        ui,
        &p,
        win,
        (w::LIST_NAME, &rows),
        (busy, editable),
        &buttons,
        &menu,
    );
    let remove = pressed.remove || matches!(pressed.menu, Some((_, 0)));
    if let Some((i, 1)) = pressed.menu {
        app.open_path(&parent(packages.get(i).map_or("", |s| s)));
    }
    if remove && editable {
        let chosen: Vec<String> = win
            .picks
            .rows
            .iter()
            .filter_map(|i| packages.get(*i).cloned())
            .collect();
        win.picks.rows.clear();
        app.remove_dlc_packages(&win.game_id, &chosen);
    }
    let dropped = super::dropped(ui);
    if !dropped.is_empty() {
        add(app, win, &dropped);
    }
    if pressed.add && editable {
        let start = packages
            .last()
            .map_or_else(|| parent(&game.source), |p| parent(p));
        let files = crate::flows::pick_files(w::PICKER, &start, w::FILTER);
        add(app, win, &files);
    }
    Reply::default()
}

/// Add DLC… or a drop on the window: the packages go to the main window, which installs them.
pub fn add(app: &mut App, win: &ListWindow, files: &[String]) {
    if files.is_empty() || win.locked || app.dlc_busy() {
        return;
    }
    app.select(&win.game_id);
    app.install_dlc_packages(files, true);
}
