//! A card's painting: the artwork over the whole tile (or the placeholder), the attention badge, and
//! the fade the title sits on.

use super::artwork_cache::{Art, crop_uv};
use crate::constants::{theme as tokens, words_library as w};
use crate::ui::icons;
use crate::ui::theme::{self, Palette, alpha, sans};
use egui::{Align2, CornerRadius, Rect, Stroke, Ui, pos2, vec2};

/// The artwork over the whole tile (`rect`, the title fades over its foot); the placeholder's
/// gamepad and words keep to the cover part (`cover`) above the title.
pub(super) fn cover(ui: &Ui, p: &Palette, art: Option<Art>, (rect, cover): (Rect, Rect)) -> String {
    let top = CornerRadius::same(5);
    if let Some(Art::Ready(tex)) = &art {
        let shape = egui::epaint::RectShape::filled(rect, top, egui::Color32::WHITE)
            .with_texture(tex.id(), crop_uv(tex.size_vec2(), rect.size()));
        ui.painter().add(shape);
        return String::new();
    }
    let error = if let Some(Art::Failed(e)) = &art {
        e.clone()
    } else {
        String::new()
    };
    ui.painter()
        .rect_filled(rect, top, theme::blend(p.raised, p.surface, 0.5));
    icons::gamepad(ui.painter(), cover.center() - vec2(0.0, 7.0), p);
    let text = if error.is_empty() {
        w::NO_ARTWORK
    } else {
        w::BAD_ARTWORK
    };
    ui.painter().text(
        pos2(cover.center().x, cover.bottom() - 13.0),
        Align2::CENTER_BOTTOM,
        text,
        sans(tokens::FONT_SMALL),
        p.muted,
    );
    error
}

pub(super) fn badge(ui: &Ui, p: &Palette, cover: Rect, status: &str) {
    let center = pos2(cover.right() - 31.0 + 11.5, cover.top() + 8.0 + 11.5);
    ui.painter()
        .circle_filled(center, 11.5, alpha(p.surface, 235));
    let s = Stroke::new(1.8, p.accent);
    if status.starts_with("Needs") {
        ui.painter()
            .line_segment([center + vec2(-5.0, 5.0), center + vec2(5.0, -5.0)], s);
        ui.painter()
            .line_segment([center + vec2(0.0, -6.0), center + vec2(6.0, 0.0)], s);
    } else {
        ui.painter()
            .line_segment([center + vec2(0.0, -5.0), center + vec2(0.0, 1.0)], s);
        ui.painter()
            .circle_filled(center + vec2(0.0, 5.0), 1.0, p.accent);
    }
}

/// A dark fade from clear at the top of `rect` to near black at its foot (the corners stay round).
pub(super) fn fade(ui: &Ui, rect: Rect) {
    let clear = egui::Color32::from_black_alpha(0);
    let dark = egui::Color32::from_black_alpha(215);
    let mut mesh = egui::Mesh::default();
    for (y, ink) in [
        (rect.top(), clear),
        (rect.center().y, egui::Color32::from_black_alpha(120)),
        (rect.bottom() - 5.0, dark),
    ] {
        mesh.colored_vertex(pos2(rect.left(), y), ink);
        mesh.colored_vertex(pos2(rect.right(), y), ink);
    }
    for row in 0..2u32 {
        let first = row * 2;
        mesh.add_triangle(first, first + 1, first + 2);
        mesh.add_triangle(first + 1, first + 2, first + 3);
    }
    ui.painter().add(mesh);
    let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 5.0), rect.max);
    let round = CornerRadius {
        nw: 0,
        ne: 0,
        sw: 5,
        se: 5,
    };
    ui.painter().rect_filled(foot, round, dark);
}
