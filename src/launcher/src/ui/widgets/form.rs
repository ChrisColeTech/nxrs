//! A window's state lines: selectable text, the status line (an error in red), the busy bar. Forms
//! are `page`'s Groups and Rows.

use crate::ui::theme::{self, Palette};
use egui::{RichText, Sense, Ui, vec2};

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
