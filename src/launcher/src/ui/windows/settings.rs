//! Settings: Engine, Build defaults, Controller, Appearance, Play down the rail (the family's work
//! window, as giga-cut's Settings); Cancel and Apply. The appearance
//! previews live; Cancel, Escape and the X restore it.

use super::{Reply, settings_tabs};
use crate::constants::words_settings as w;
use crate::flows::App;
use crate::flows::settings::SettingsDraft;
use crate::ui::popups::authored;
use crate::ui::theme;
use crate::ui::widgets::rail::{self, Icon};
use egui::{Rect, Sense, Ui, UiBuilder, pos2, vec2};

/// The sections down the rail, in the order of `words_settings::SETTINGS_TABS`.
const ICONS: [Icon; 5] = [
    Icon::Engine,
    Icon::Build,
    Icon::Controller,
    Icon::Appearance,
    Icon::Play,
];

pub fn show(ui: &mut Ui, app: &mut App, s: &mut SettingsDraft) -> Reply {
    let p = theme::of(ui.ctx());
    // the family's work window: the rail down the left, the section's form scrolling beside it
    let size = vec2(ui.available_width(), authored::body_height(ui));
    let (area, _) = ui.allocate_exact_size(size, Sense::hover());
    let rail_rect = Rect::from_min_size(area.min, vec2(rail::WIDTH, area.height()));
    let entries: Vec<(&str, Icon)> = w::SETTINGS_TABS.iter().copied().zip(ICONS).collect();
    if let Some(t) = rail::rail(ui, rail_rect, &entries, s.tab) {
        s.tab = t;
    }
    let body = Rect::from_min_max(pos2(rail_rect.right() + 16.0, area.top()), area.max);
    let mut pane = ui.new_child(UiBuilder::new().max_rect(body).id_salt("settings-body"));
    egui::ScrollArea::vertical()
        .id_salt(("settings-page", s.tab))
        .auto_shrink([false, false])
        .show(&mut pane, |ui| {
            ui.add_space(2.0);
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
