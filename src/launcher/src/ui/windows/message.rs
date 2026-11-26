//! Message and question boxes (`QMessageBox`): an icon, the text, Close or Yes / No.

use super::Reply;
use crate::constants::{child_window as c, popups as p, theme as tokens, words_settings as w};
use crate::flows::Action;
use crate::flows::surface::Check;
use crate::ui::popups::child_window::ChildSpec;
use crate::ui::theme;
use crate::ui::widgets::{button, page};
use egui::{Align, Layout, Sense, Stroke, Ui, vec2};

/// Sized to its text: about 62 characters a line at its width.
/// `checked`: a tick box sits over the buttons.
pub fn spec<'a>(key: &'a str, title: &'a str, text: &str, checked: bool) -> ChildSpec<'a> {
    let lines: f32 = text
        .lines()
        .map(|l| (l.chars().count() as f32 / 62.0).ceil().max(1.0))
        .sum();
    let body = (lines * 19.0 + 70.0 + footer(checked) - p::FOOTER).clamp(110.0, 500.0);
    ChildSpec::work(key, title, c::MESSAGE_WIDTH, body)
        .least(c::MESSAGE_LEAST, body.min(160.0))
        .popup()
}

fn mark(ui: &mut Ui, glyph: &str, error: bool) {
    let pal = theme::of(ui.ctx());
    let color = if error {
        theme::hex(tokens::BADGE_ERROR)
    } else {
        pal.accent
    };
    let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
    ui.painter()
        .circle_stroke(r.center(), 14.0, Stroke::new(2.0, color));
    ui.painter().text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::semibold(16.0),
        color,
    );
}

/// The buttons' strip, and the tick box's row over it.
fn footer(checked: bool) -> f32 {
    if checked { p::FOOTER + 34.0 } else { p::FOOTER }
}

fn text_area(ui: &mut Ui, glyph: &str, text: &str, error: bool, checked: bool) {
    let height = (ui.available_height() - footer(checked)).max(40.0);
    ui.horizontal_top(|ui| {
        mark(ui, glyph, error);
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(height)
            .show(ui, |ui| {
                ui.add(egui::Label::new(text).wrap().selectable(true));
            });
    });
}

pub fn message(ui: &mut Ui, text: &str, error: bool) -> Reply {
    text_area(ui, if error { "!" } else { "i" }, text, error, false);
    let pal = theme::of(ui.ctx());
    let mut reply = Reply::default();
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if button::primary(ui, &pal, w::CLOSE, true).clicked() || enter {
            reply.close = true;
        }
    });
    reply
}

pub fn question(
    ui: &mut Ui,
    text: &str,
    (yes, no): (&Action, Option<&Action>),
    default_no: bool,
    mut check: Option<&mut Check>,
) -> Reply {
    text_area(ui, "?", text, false, check.is_some());
    let pal = theme::of(ui.ctx());
    let mut reply = Reply::default();
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // always No | Yes, Yes filled and last; Enter answers the default (No where the Qt box did)
            let said_yes = button::primary(ui, &pal, w::YES, true).clicked();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let said_no = button::ghost(ui, w::NO, true).clicked() || (enter && default_no);
            let said_yes = said_yes || (enter && !default_no);
            if said_yes {
                let ticked = check.as_deref().filter(|c| c.on).map(|c| c.yes_on.clone());
                reply.actions.push(ticked.unwrap_or_else(|| yes.clone()));
            } else if said_no {
                reply.actions.extend(no.cloned());
            }
            reply.close = said_yes || said_no;
        });
        // the tick box sits over the buttons, its explanation in the info mark
        if let Some(c) = &mut check {
            ui.add_space(6.0);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add_space(40.0);
                ui.checkbox(&mut c.on, c.label.as_str());
                page::info(ui, &c.tip);
            });
        }
    });
    reply
}
