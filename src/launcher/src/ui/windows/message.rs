//! Message and question boxes (`QMessageBox`): an icon, the text, Close or Yes / No.

use super::Reply;
use crate::constants::{child_window as c, popups as p, theme as tokens, words_settings as w};
use crate::flows::Action;
use crate::ui::popups::child_window::ChildSpec;
use crate::ui::theme;
use crate::ui::widgets::button;
use egui::{Align, Layout, Sense, Stroke, Ui, vec2};

/// Sized to its text: about 62 characters a line at its width.
pub fn spec<'a>(key: &'a str, title: &'a str, text: &str) -> ChildSpec<'a> {
    let lines: f32 = text
        .lines()
        .map(|l| (l.chars().count() as f32 / 62.0).ceil().max(1.0))
        .sum();
    let body = (lines * 19.0 + 70.0).clamp(110.0, 460.0);
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

fn text_area(ui: &mut Ui, glyph: &str, text: &str, error: bool) {
    let height = (ui.available_height() - p::FOOTER).max(40.0);
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
    text_area(ui, if error { "!" } else { "i" }, text, error);
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
) -> Reply {
    text_area(ui, "?", text, false);
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
                reply.actions.push(yes.clone());
            } else if said_no {
                reply.actions.extend(no.cloned());
            }
            reply.close = said_yes || said_no;
        });
    });
    reply
}
