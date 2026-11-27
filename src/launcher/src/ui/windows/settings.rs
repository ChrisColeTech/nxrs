//! Settings: Engine, Build defaults, Controller, Appearance, Play down the rail (the family's work
//! window, as giga-cut's Settings); Apply. The appearance previews live; Escape and the X discard
//! the Draft and restore it.

use super::{Reply, settings_tabs};
use crate::constants::words_settings as w;
use crate::flows::App;
use crate::flows::settings::SettingsDraft;
use crate::ui::popups::authored;
use crate::ui::theme;
use crate::ui::widgets::rail::Icon;
use egui::Ui;

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
    let entries: Vec<(&str, Icon)> = w::SETTINGS_TABS.iter().copied().zip(ICONS).collect();
    let mut tab = s.tab;
    super::rail_window::show(ui, "settings", &entries, &mut tab, |ui, tab| match tab {
        0 => settings_tabs::engine(ui, app, s),
        1 => settings_tabs::builds(ui, app, s),
        2 => settings_tabs::controller(ui, s),
        3 => settings_tabs::appearance(ui, app, s),
        _ => settings_tabs::play(ui, s),
    });
    s.tab = tab;
    let mut reply = Reply::default();
    if authored::footer(ui, &p, w::APPLY, true) {
        reply.close = app.apply_settings(s);
    }
    reply
}
