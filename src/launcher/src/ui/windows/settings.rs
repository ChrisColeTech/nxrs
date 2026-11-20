//! Settings: Engine, Build defaults, Controller, Appearance, Play; Cancel and Apply. The appearance
//! previews live; Cancel, Escape and the X restore it.

use super::{Reply, settings_tabs};
use crate::constants::words_settings as w;
use crate::flows::App;
use crate::flows::settings::SettingsDraft;
use crate::ui::popups::authored;
use crate::ui::theme;
use egui::Ui;

pub fn show(ui: &mut Ui, app: &mut App, s: &mut SettingsDraft) -> Reply {
    let p = theme::of(ui.ctx());
    if let Some(t) = authored::tabs(ui, &p, "settings-tabs", &w::SETTINGS_TABS, s.tab) {
        s.tab = t;
    }
    ui.add_space(4.0);
    let height = authored::body_height(ui);
    egui::ScrollArea::vertical()
        .id_salt(("settings-page", s.tab))
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(8.0);
            match s.tab {
                0 => settings_tabs::engine(ui, app, s),
                1 => settings_tabs::builds(ui, s),
                2 => settings_tabs::controller(ui, s),
                3 => settings_tabs::appearance(ui, app, s),
                _ => settings_tabs::play(ui, s),
            }
        });
    let (cancel, apply) = authored::footer(ui, &p, w::APPLY, true, w::CANCEL);
    let mut reply = Reply::default();
    if cancel {
        app.cancel_settings();
        reply.close = true;
    }
    if apply {
        reply.close = app.apply_settings(s);
    }
    reply
}
