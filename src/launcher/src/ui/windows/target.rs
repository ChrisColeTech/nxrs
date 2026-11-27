//! The Native / Web chooser Play and Build open (owner, 2026-11-27: one Play, one Build). The shared
//! header ("Play" / "Build"; its X and Escape dismiss, there is no Cancel), a Group heading naming the
//! game, a Row per target saying what it does or why it cannot run now, and the footer's two equal
//! buttons, Native and Web: a choice dialog's choices are its buttons, and each acts at once (the
//! style guide's exception to one filled primary).

use super::Reply;
use crate::constants::{child_window as c, words_target as w};
use crate::flows::{Action, App};
use crate::model::fill::fill;
use crate::ui::popups::{authored, child_window::ChildSpec};
use crate::ui::theme;
use crate::ui::widgets::{page, rail::Icon};
use egui::{RichText, Ui};

pub fn spec<'a>(key: &'a str, title: &'a str) -> ChildSpec<'a> {
    ChildSpec::work(key, title, c::TARGET_WIDTH, c::TARGET_BODY).popup()
}

/// The chooser's title: the verb.
pub fn title(build: bool) -> String {
    if build { w::BUILD } else { w::PLAY }.into()
}

/// What a target does, in its Row.
fn what(app: &App, build: bool, web: bool) -> String {
    match (build, web) {
        (false, false) => w::PLAY_NATIVE_WHAT.into(),
        (false, true) => {
            let browser = app.tools.browser.as_ref().map_or("", |b| b.name.as_str());
            fill(w::PLAY_WEB_WHAT, &[browser])
        }
        (true, false) => w::BUILD_NATIVE_WHAT.into(),
        (true, true) => w::BUILD_WEB_WHAT.into(),
    }
}

fn answer(build: bool, web: bool) -> Action {
    match (build, web) {
        (false, false) => Action::PlayNative,
        (false, true) => Action::PlayWeb,
        (true, false) => Action::BuildNative,
        (true, true) => Action::BuildWeb,
    }
}

const TARGETS: [(bool, &str); 2] = [(false, w::NATIVE), (true, w::WEB)];

pub fn show(ui: &mut Ui, app: &App, game_id: &str, build: bool) -> Reply {
    let p = theme::of(ui.ctx());
    let game = app.lib.find(game_id).map_or("", |g| g.title.as_str());
    let (icon, tip) = if build {
        (Icon::Build, w::BUILD_TIP)
    } else {
        (Icon::Play, w::PLAY_TIP)
    };
    page::group(ui, icon, game, tip);
    let why = TARGETS.map(|(web, _)| app.why_not(game_id, build, web));
    for ((web, name), why) in TARGETS.iter().zip(&why) {
        let (line, ink) = match why {
            Some(reason) => (reason.clone(), p.disabled()),
            None => (what(app, build, *web), p.text),
        };
        page::row(ui, name, "", |ui| {
            ui.add(egui::Label::new(RichText::new(line).color(ink)).wrap());
        });
    }
    // the footer: Native, then Web; no dismissal (the header X)
    let choices = [0, 1].map(|at| {
        let reason = why[at].as_deref().unwrap_or("");
        (TARGETS[at].1, why[at].is_none(), reason)
    });
    let chosen = authored::footer_choices(ui, &p, &choices);
    let mut reply = Reply::default();
    if let Some(web) = chosen.map(|at| TARGETS[at].0) {
        // the answers act on the selected game: this one
        reply.actions.push(Action::Select(game_id.into()));
        reply.actions.push(answer(build, web));
        reply.close = true;
    }
    reply
}
