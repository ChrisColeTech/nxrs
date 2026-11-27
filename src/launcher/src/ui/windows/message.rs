//! Message and question boxes (`QMessageBox`) with the one body header every dialog has: the icon
//! tile (`i`, `!` in the error red, `?`) and the heading, then the text (a question's tick box as its
//! last paragraph). Every box ends with one filled primary and no dismissal button: a message's OK, a
//! question's verb ("Remove"); the X and Escape close a message and answer a question No.

use super::Reply;
use crate::constants::{child_window as c, popups as p, theme as tokens};
use crate::flows::Action;
use crate::flows::surface::{Ask, Check};
use crate::ui::popups::{authored, child_window::ChildSpec};
use crate::ui::theme;
use crate::ui::widgets::{page, rail::Icon};
use egui::Ui;

/// The heading row (tile, gaps) and a wrapped heading's extra line; text lines; the gap under it.
const HEADING: f32 = 54.0;
const HEADING_LINE: f32 = 22.0;
const HEADING_CHARS: f32 = 40.0;
const LINE: f32 = 19.0;
const LINE_CHARS: f32 = 62.0;
const BOTTOM: f32 = 14.0;

/// Sized to its heading, text and footer. `checked`: a tick box is the text's last paragraph.
pub fn spec<'a>(
    (key, title): (&'a str, &'a str),
    (heading, text): (&str, &str),
    checked: bool,
) -> ChildSpec<'a> {
    let wraps = |s: &str, per: f32| (s.chars().count() as f32 / per).ceil().max(1.0);
    let lines: f32 = text.lines().map(|l| wraps(l, LINE_CHARS)).sum();
    let extra = (wraps(heading, HEADING_CHARS) - 1.0) * HEADING_LINE;
    let check = if checked { c::MESSAGE_CHECK_ROW } else { 0.0 };
    let body = HEADING + extra + lines * LINE + BOTTOM + p::FOOTER + check;
    let body = body.clamp(80.0, 500.0);
    ChildSpec::work(key, title, c::MESSAGE_WIDTH, body)
        .least(c::MESSAGE_LEAST, body.min(160.0))
        .popup()
}

/// The header, and under it the text with the tick box as its last paragraph (its explanation in
/// the info mark after it), in the room left above `below`.
fn body(ui: &mut Ui, (icon, heading, error): (Icon, &str, bool), text: &str, below: f32) {
    let ink = error.then(|| theme::hex(tokens::BADGE_ERROR));
    page::heading(ui, icon, heading, "", ink);
    let height = (ui.available_height() - below).max(30.0);
    if !text.is_empty() {
        egui::ScrollArea::vertical()
            .max_height(height)
            .show(ui, |ui| {
                ui.add(egui::Label::new(text).wrap().selectable(true));
            });
    }
}

/// A message's footer is its one filled OK; OK, Enter, the X and Escape close it.
pub fn message(ui: &mut Ui, heading: &str, text: &str, error: bool) -> Reply {
    let icon = if error { Icon::Error } else { Icon::Info };
    body(ui, (icon, heading, error), text, p::FOOTER);
    let pal = theme::of(ui.ctx());
    let ok = authored::footer_at_bottom(ui, &pal, (crate::constants::words_settings::OK, true, ""));
    Reply {
        close: ok || ui.input(|i| i.key_pressed(egui::Key::Enter)),
        ..Reply::default()
    }
}

pub fn question(
    ui: &mut Ui,
    ask: &Ask,
    (yes, no): (&Action, Option<&Action>),
    default_no: bool,
    mut check: Option<&mut Check>,
) -> Reply {
    let below = p::FOOTER + check.as_ref().map_or(0.0, |_| c::MESSAGE_CHECK_ROW);
    body(ui, (Icon::Question, &ask.heading, false), &ask.text, below);
    if let Some(c) = check.as_deref_mut() {
        ui.add_space(c::MESSAGE_CHECK_GAP);
        ui.horizontal(|ui| {
            ui.checkbox(&mut c.on, c.label.as_str());
            page::info(ui, &c.tip);
        });
    }
    let pal = theme::of(ui.ctx());
    // the one verb, filled; Enter answers the default (No where the Qt box did, as the X does)
    let said_yes = authored::footer_at_bottom(ui, &pal, (&ask.verb, true, ""));
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let said_no = enter && default_no;
    let said_yes = said_yes || (enter && !default_no);
    let mut reply = Reply::default();
    if said_yes {
        let ticked = check.as_deref().filter(|c| c.on).map(|c| c.yes_on.clone());
        reply.actions.push(ticked.unwrap_or_else(|| yes.clone()));
    } else if said_no {
        reply.actions.extend(no.cloned());
    }
    reply.close = said_yes || said_no;
    reply
}
