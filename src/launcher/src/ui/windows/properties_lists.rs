//! Properties' DLC and Updates pages (`DlcPanel`, `UpdatePanel`): the lists, removal on Apply.

use crate::constants::{words_dlc as d, words_updates as u};
use crate::flows::App;
use crate::flows::properties::PropertiesDraft;
use crate::model::path_text::parent;
use crate::ui::theme;
use crate::ui::widgets::{form, pick_list};
use egui::Ui;

/// Removes the chosen rows (Remove or Delete) from a draft list.
fn remove<T>(items: &mut Vec<T>, rows: &std::collections::BTreeSet<usize>) {
    let mut at = 0;
    items.retain(|_| {
        at += 1;
        !rows.contains(&(at - 1))
    });
}

pub fn dlc(ui: &mut Ui, app: &App, draft: &mut PropertiesDraft) {
    let p = theme::of(ui.ctx());
    let locked = draft.locked || app.dlc_busy();
    let rows = app.dlc_rows(&draft.game, &draft.dlc);
    let packages = draft.dlc.clone();
    let height = ui.available_height() - 52.0;
    let out = pick_list::show(
        ui,
        &p,
        d::LIST_NAME,
        &rows,
        &mut draft.dlc_picks,
        height,
        &|i, _| {
            vec![
                (d::REMOVE.into(), !locked),
                (
                    d::OPEN_FOLDER.into(),
                    packages.get(i).is_some_and(|p| app.exists(p)),
                ),
            ]
        },
    );
    match out.menu {
        Some((_, 0)) if !locked => remove(&mut draft.dlc, &draft.dlc_picks.rows),
        Some((i, _)) => app.open_path(&parent(packages.get(i).map_or("", |s| s))),
        None => {}
    }
    if out.delete && !locked {
        remove(&mut draft.dlc, &draft.dlc_picks.rows);
    }
    if out.delete || matches!(out.menu, Some((_, 0))) {
        draft.dlc_picks.rows.clear();
    }
    let note = if draft.dlc.is_empty() {
        d::PANEL_EMPTY
    } else if locked {
        d::HINT_LOCKED
    } else {
        d::PANEL_NOTE
    };
    form::note(ui, &p, note);
}

pub fn updates(ui: &mut Ui, app: &App, draft: &mut PropertiesDraft) {
    let p = theme::of(ui.ctx());
    let locked = draft.locked || app.probe_busy();
    let mut state = draft.game.clone();
    state.updates = draft.updates.clone();
    let rows = app.update_rows(&state);
    let paths: Vec<String> = draft.updates.iter().map(|u| u.path.clone()).collect();
    let height = ui.available_height() - 66.0;
    let out = pick_list::show(
        ui,
        &p,
        u::LIST_NAME,
        &rows,
        &mut draft.update_picks,
        height,
        &|i, _| {
            vec![
                (u::REMOVE.into(), !locked),
                (
                    d::OPEN_FOLDER.into(),
                    paths.get(i).is_some_and(|p| app.exists(p)),
                ),
            ]
        },
    );
    match out.menu {
        Some((_, 0)) if !locked => remove(&mut draft.updates, &draft.update_picks.rows),
        Some((i, _)) => app.open_path(&parent(paths.get(i).map_or("", |s| s))),
        None => {}
    }
    if out.delete && !locked {
        remove(&mut draft.updates, &draft.update_picks.rows);
    }
    if out.delete || matches!(out.menu, Some((_, 0))) {
        draft.update_picks.rows.clear();
    }
    state.updates = draft.updates.clone();
    let note = if state.updates.is_empty() {
        u::PANEL_EMPTY.to_string()
    } else if locked {
        u::HINT_LOCKED.to_string()
    } else {
        let pending = app.update_pending_text(&state);
        if pending.is_empty() {
            u::PANEL_NOTE.to_string()
        } else {
            format!("{}\n{pending}", u::PANEL_NOTE)
        }
    };
    form::note(ui, &p, &note);
}
