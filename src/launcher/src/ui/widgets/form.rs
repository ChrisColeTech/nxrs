//! A form's rows (Qt's `QFormLayout`): a label column and the control; notes in muted text.

use crate::constants::popups as c;
use crate::ui::theme::{self, Palette};
use egui::{Align2, RichText, Sense, Ui, vec2};

/// One labelled row; the control takes the rest of the width.
pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(c::LABEL_COLUMN, c::FIELD), Sense::hover());
        let ink = ui.visuals().text_color();
        ui.painter().text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            label,
            theme::sans(crate::constants::theme::FONT_BODY),
            ink,
        );
        add(ui);
    });
    ui.add_space(c::GAP);
}

/// A text value in a row (Format, Version, Status).
pub fn value(ui: &mut Ui, label: &str, text: &str) {
    row(ui, label, |ui| {
        ui.label(text);
    });
}

/// Muted, wrapped text under the rows.
pub fn note(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(p.muted)).wrap());
}

/// Selectable muted text (paths, health, the doctor's checks).
pub fn selectable(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add(
        egui::Label::new(RichText::new(text).color(p.muted))
            .wrap()
            .selectable(true),
    );
}

/// The status line of a window: an error in the semantic red.
pub fn status(ui: &mut Ui, p: &Palette, text: &str, error: bool) {
    let ink = if error {
        theme::hex(crate::constants::theme::ERROR_TEXT)
    } else {
        p.text
    };
    ui.add(
        egui::Label::new(RichText::new(text).color(ink))
            .wrap()
            .selectable(true),
    );
}

/// The indeterminate bar of a running engine call.
pub fn busy(ui: &mut Ui, p: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    ui.painter()
        .rect_filled(r, egui::CornerRadius::same(3), p.border);
    let t = ui.input(|i| i.time) as f32;
    let w = r.width() * 0.25;
    let x = r.left() + (t * 0.6).fract() * (r.width() + w) - w;
    let seg = egui::Rect::from_min_max(
        egui::pos2(x.max(r.left()), r.top()),
        egui::pos2((x + w).min(r.right()), r.bottom()),
    );
    ui.painter()
        .rect_filled(seg, egui::CornerRadius::same(3), p.accent);
    ui.ctx().request_repaint();
}
