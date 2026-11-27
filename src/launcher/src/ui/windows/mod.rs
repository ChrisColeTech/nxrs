//! The dialogs: each open surface is shown in its modal child window every frame; a window whose
//! surface closed is shut. Only the top surface takes input (a message over Properties, say).
mod dlc;
mod list_window;
mod message;
mod properties;
mod properties_lists;
mod properties_tabs;
mod rail_window;
mod settings;
mod settings_tabs;
mod target;
mod updates;

use crate::flows::surface::Surface;
use crate::flows::{Action, App};
use crate::ui::popups::child_window::{self, ChildSpec};
use egui::{Context, Id};

const OPEN_KEYS: &str = "nxrs-open-windows";

/// What a surface's body asks of the host.
#[derive(Default)]
pub struct Reply {
    /// the surface is done (accepted, cancelled, dismissed)
    pub close: bool,
    pub actions: Vec<Action>,
}

/// Files dropped on this window (the DLC and Updates windows take them as Add does).
pub fn dropped(ui: &egui::Ui) -> Vec<String> {
    ui.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| crate::model::path_text::from_native(&f.path().to_string_lossy()))
            .collect()
    })
}

/// The spec of a surface's window.
fn spec<'a>(surface: &Surface, key: &'a str, title: &'a str) -> ChildSpec<'a> {
    use crate::constants::child_window as c;
    match surface {
        Surface::Properties(p) if p.adding => ChildSpec::surface(key, title, c::ADD_GAME),
        Surface::Properties(_) => ChildSpec::surface(key, title, c::PROPERTIES),
        Surface::Settings(_) => ChildSpec::surface(key, title, c::SETTINGS),
        Surface::Dlc(_) => ChildSpec::surface(key, title, c::DLC),
        Surface::Updates(_) => ChildSpec::surface(key, title, c::UPDATES),
        Surface::Message { text, .. } => message::spec(key, title, text, false),
        Surface::Question { text, check, .. } => message::spec(key, title, text, check.is_some()),
        Surface::Target { .. } => target::spec(key, title),
        Surface::ToolPopup => ChildSpec::work(key, title, 0.0, 0.0),
    }
}

/// Shows every open dialog; returns the actions their answers ask for.
pub fn show(ctx: &Context, app: &mut App) -> Vec<Action> {
    let mut surfaces = std::mem::take(&mut app.surfaces);
    let count = surfaces.len();
    let mut kept = Vec::new();
    let mut actions = Vec::new();
    let mut open_keys: Vec<String> = Vec::new();
    for (at, mut surface) in surfaces.drain(..).enumerate() {
        if matches!(surface, Surface::ToolPopup) {
            kept.push(surface);
            continue;
        }
        let key = surface.key(at);
        let title = title_of(app, &surface);
        let top = at + 1 == count;
        let mut reply = Reply::default();
        let dismissed = child_window::show(ctx, &spec(&surface, &key, &title), |ui| {
            if !top {
                ui.disable();
            }
            reply = body(ui, app, &mut surface);
        });
        if dismissed {
            reply.close = true;
            reply.actions.extend(dismissal(app, &surface));
        }
        actions.append(&mut reply.actions);
        if reply.close {
            child_window::shut(ctx, &key);
        } else {
            open_keys.push(key);
            kept.push(surface);
        }
    }
    shut_gone(ctx, &open_keys);
    kept.append(&mut app.surfaces);
    app.surfaces = kept;
    actions
}

/// Windows open last frame but not now are shut (their next showing is a new opening).
fn shut_gone(ctx: &Context, open: &[String]) {
    let before: Vec<String> = ctx
        .data(|d| d.get_temp(Id::unique(OPEN_KEYS)))
        .unwrap_or_default();
    for key in before.iter().filter(|k| !open.contains(k)) {
        child_window::shut(ctx, key);
    }
    ctx.data_mut(|d| d.insert_temp(Id::unique(OPEN_KEYS), open.to_vec()));
}

fn title_of(app: &App, surface: &Surface) -> String {
    use crate::constants::{words_dlc as d, words_settings as s, words_updates as u};
    let game = |id: &str| {
        app.lib
            .find(id)
            .map(|g| g.title.clone())
            .unwrap_or_default()
    };
    match surface {
        Surface::Message { title, .. } | Surface::Question { title, .. } => {
            if title.is_empty() {
                "nxrs".into()
            } else {
                title.clone()
            }
        }
        Surface::Properties(p) if p.adding => s::ADD_TITLE.into(),
        Surface::Properties(p) => format!("{}{}", p.game.title, s::PROPERTIES_TITLE),
        Surface::Settings(_) => s::SETTINGS_TITLE.into(),
        Surface::Dlc(w) => format!("{}{}", d::WINDOW_TITLE, game(&w.game_id)),
        Surface::Updates(w) => format!("{}{}", u::WINDOW_TITLE, game(&w.game_id)),
        Surface::Target { build, .. } => target::title(*build),
        Surface::ToolPopup => String::new(),
    }
}

fn body(ui: &mut egui::Ui, app: &mut App, surface: &mut Surface) -> Reply {
    match surface {
        Surface::Message { text, error, .. } => message::message(ui, text, *error),
        Surface::Question {
            text,
            yes,
            no,
            default_no,
            check,
            ..
        } => message::question(ui, text, (yes, no.as_ref()), *default_no, check.as_mut()),
        Surface::Properties(p) => properties::show(ui, app, p),
        Surface::Settings(s) => settings::show(ui, app, s),
        Surface::Dlc(w) => dlc::show(ui, app, w),
        Surface::Updates(w) => updates::show(ui, app, w),
        Surface::Target { game_id, build } => target::show(ui, app, game_id, *build),
        Surface::ToolPopup => Reply::default(),
    }
}

/// The X, Escape or the OS closing a window: a Draft is discarded; a question is answered No.
fn dismissal(app: &mut App, surface: &Surface) -> Vec<Action> {
    match surface {
        Surface::Settings(_) => {
            app.cancel_settings();
            Vec::new()
        }
        Surface::Question { no: Some(no), .. } => vec![no.clone()],
        _ => Vec::new(),
    }
}
