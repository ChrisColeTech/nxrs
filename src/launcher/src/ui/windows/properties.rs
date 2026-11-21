//! Properties / Add game: the rail and its section (the family's work window), the footer (Cancel, Apply or Add game). A Draft: nothing
//! changes until Apply; the X, Escape and Cancel discard it.

use super::{Reply, properties_lists, properties_tabs};
use crate::constants::words_settings as w;
use crate::flows::App;
use crate::flows::properties::PropertiesDraft;
use crate::ui::popups::authored;
use crate::ui::theme;
use crate::ui::widgets::rail::Icon;
use egui::Ui;

/// The sections down the rail, in the order of `words_settings::PROPERTIES_TABS`.
const ICONS: [Icon; 6] = [
    Icon::General,
    Icon::Build,
    Icon::Runtime,
    Icon::Artwork,
    Icon::Dlc,
    Icon::Updates,
];

pub fn show(ui: &mut Ui, app: &mut App, d: &mut PropertiesDraft) -> Reply {
    let p = theme::of(ui.ctx());
    // Add game shows the four sections a game has before it is built
    let count = if d.adding { 4 } else { 6 };
    let entries: Vec<(&str, Icon)> = w::PROPERTIES_TABS
        .iter()
        .copied()
        .zip(ICONS)
        .take(count)
        .collect();
    let mut tab = d.tab.min(count - 1);
    super::rail_window::show(ui, "properties", &entries, &mut tab, |ui, tab| match tab {
        0 => properties_tabs::general(ui, app, d),
        1 => {
            ui.add_enabled_ui(!d.locked, |ui| properties_tabs::build(ui, d));
        }
        2 => {
            ui.add_enabled_ui(!d.locked, |ui| properties_tabs::runtime(ui, app, d));
        }
        3 => properties_tabs::artwork(ui, d),
        4 => properties_lists::dlc(ui, app, d),
        _ => properties_lists::updates(ui, app, d),
    });
    d.tab = tab;
    let primary = if d.adding { w::ADD_TITLE } else { w::APPLY };
    let (cancel, apply) = authored::footer(ui, &p, primary, true, w::CANCEL);
    let mut reply = Reply {
        close: cancel,
        ..Reply::default()
    };
    if apply {
        reply.close = app.apply_properties(d);
    }
    reply
}
