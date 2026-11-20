//! Properties / Add game: the tabs, the page, the footer (Cancel, Apply or Add game). A Draft: nothing
//! changes until Apply; the X, Escape and Cancel discard it.

use super::{Reply, properties_lists, properties_tabs};
use crate::constants::words_settings as w;
use crate::flows::App;
use crate::flows::properties::PropertiesDraft;
use crate::ui::popups::authored;
use crate::ui::theme;
use egui::Ui;

pub fn show(ui: &mut Ui, app: &mut App, d: &mut PropertiesDraft) -> Reply {
    let p = theme::of(ui.ctx());
    let tabs: &[&str] = if d.adding {
        &w::PROPERTIES_TABS[..4]
    } else {
        &w::PROPERTIES_TABS
    };
    if let Some(t) = authored::tabs(ui, &p, "properties-tabs", tabs, d.tab) {
        d.tab = t;
    }
    ui.add_space(4.0);
    let height = authored::body_height(ui);
    egui::ScrollArea::vertical()
        .id_salt(("properties-page", d.tab))
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(8.0);
            match d.tab {
                0 => properties_tabs::general(ui, app, d),
                1 => {
                    ui.add_enabled_ui(!d.locked, |ui| properties_tabs::build(ui, d))
                        .inner
                }
                2 => {
                    ui.add_enabled_ui(!d.locked, |ui| properties_tabs::runtime(ui, app, d))
                        .inner
                }
                3 => properties_tabs::artwork(ui, d),
                4 => properties_lists::dlc(ui, app, d),
                _ => properties_lists::updates(ui, app, d),
            }
        });
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
