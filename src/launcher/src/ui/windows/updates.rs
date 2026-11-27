//! Game > Install Updates… (`UpdateDialog`): Version to build, the list (changed at once), what the next
//! build will be, Build now when a rebuild is pending.

use super::Reply;
use super::list_window::{self, Buttons};
use crate::constants::{words_dlc as d, words_updates as w};
use crate::flows::App;
use crate::flows::surface::ListWindow;
use crate::model::fill::fill;
use crate::model::path_text::parent;
use crate::model::update_text;
use crate::ui::theme;
use crate::ui::widgets::select;
use egui::Ui;

pub fn hint(empty: bool, locked: bool) -> &'static str {
    if locked {
        w::HINT_LOCKED
    } else if empty {
        w::HINT_EMPTY
    } else {
        w::HINT_SOME
    }
}

fn version_row(ui: &mut Ui, app: &mut App, win: &ListWindow, editable: bool) {
    let Some(game) = app.lib.find(&win.game_id).cloned() else {
        return;
    };
    let choices = update_text::choices(&game);
    let mut index = update_text::choice_index(&game);
    let indexed: Vec<(String, usize)> = choices
        .iter()
        .enumerate()
        .map(|(i, (l, _))| (l.clone(), i))
        .collect();
    // a Row like every labelled control: the label column, its explanation in the info mark
    crate::ui::widgets::page::row(ui, w::VERSION, w::VERSION_TIP, |ui| {
        let changed = ui
            .add_enabled_ui(editable, |ui| {
                select::select(ui, w::VERSION, &mut index, &indexed)
            })
            .inner;
        if changed {
            let value = choices.get(index).map(|c| c.1.clone()).unwrap_or_default();
            app.set_update_choice(&win.game_id, &value);
        }
    });
}

pub fn show(ui: &mut Ui, app: &mut App, win: &mut ListWindow) -> Reply {
    let p = theme::of(ui.ctx());
    let Some(game) = app.lib.find(&win.game_id).cloned() else {
        return Reply {
            close: true,
            ..Reply::default()
        };
    };
    let busy = app.probe_busy();
    let editable = !win.locked && !busy;
    let tip = hint(game.updates.is_empty(), false);
    crate::ui::widgets::page::group(ui, crate::ui::widgets::rail::Icon::Updates, "Updates", tip);
    version_row(ui, app, win, editable);
    ui.add_space(4.0);
    let rows = app.update_rows(&game);
    let paths: Vec<String> = game.updates.iter().map(|u| u.path.clone()).collect();
    let selected = win.picks.rows.len();
    let menu = |i: usize, n: usize| {
        let remove = if n > 1 {
            fill(w::REMOVE_MANY, &[&n.to_string()])
        } else {
            w::REMOVE.into()
        };
        vec![
            (w::BUILD_WITH.into(), editable && selected <= 1),
            (remove, editable),
            (
                d::OPEN_FOLDER.into(),
                paths.get(i).is_some_and(|p| app.exists(p)),
            ),
        ]
    };
    let pending = app.update_pending_text(&game);
    // the state stays in view; how the list works is in the heading's info mark
    let locked = if win.locked { w::HINT_LOCKED } else { "" };
    let empty = if game.updates.is_empty() {
        w::LIST_EMPTY
    } else {
        ""
    };
    let lines = [locked, empty, pending.as_str()];
    let buttons = Buttons {
        add: w::ADD,
        add_tip: w::ADD_TIP,
        remove_tip: w::REMOVE_TIP,
        build: Some((w::BUILD_NOW, w::BUILD_NOW_TIP, !pending.is_empty())),
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
    match pressed.menu {
        Some((i, 0)) if editable => {
            app.set_update_choice(&win.game_id, paths.get(i).map_or("", |s| s))
        }
        Some((i, 2)) => app.open_path(&parent(paths.get(i).map_or("", |s| s))),
        _ => {}
    }
    let remove = pressed.remove || matches!(pressed.menu, Some((_, 1)));
    if remove && editable {
        let chosen: Vec<String> = win
            .picks
            .rows
            .iter()
            .filter_map(|i| paths.get(*i).cloned())
            .collect();
        win.picks.rows.clear();
        app.remove_update_packages(&win.game_id, &chosen);
    }
    let dropped = super::dropped(ui);
    if !dropped.is_empty() {
        add(app, win, &dropped);
    }
    if pressed.add && editable {
        let start = paths
            .last()
            .map_or_else(|| parent(&game.source), |p| parent(p));
        let files = crate::flows::pick_files(w::PICKER, &start, w::FILTER);
        add(app, win, &files);
    }
    let mut reply = Reply::default();
    if pressed.build && editable {
        app.select(&win.game_id);
        reply.close = true;
        reply.actions.push(crate::flows::Action::Build);
    }
    reply
}

pub fn add(app: &mut App, win: &ListWindow, files: &[String]) {
    if files.is_empty() || win.locked || app.probe_busy() {
        return;
    }
    app.select(&win.game_id);
    app.install_update_packages(files, true);
}
