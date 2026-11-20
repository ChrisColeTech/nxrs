//! The background canvas (`LibraryCanvas`): the selected game's background (else its cover) behind the
//! library, dimmed (.32 dark, .20 light) and fading toward the bottom.

use super::artwork_cache::{Art, ArtworkCache, crop_uv};
use crate::constants::theme as tokens;
use crate::flows::App;
use crate::ui::theme::{self, alpha};
use egui::{Color32, CornerRadius, Mesh, Rect, Shape, Ui};

pub fn show(ui: &Ui, app: &App, art: &mut ArtworkCache, rect: Rect) {
    let p = theme::of(ui.ctx());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.canvas);
    if !app.lib.show_background {
        return;
    }
    let Some(g) = app.selected_game() else { return };
    let path = if g.background.is_empty() {
        &g.cover
    } else {
        &g.background
    };
    let Some(Art::Ready(tex)) = art.get(path) else {
        return;
    };
    let opacity = if p.light {
        tokens::CANVAS_OPACITY.0
    } else {
        tokens::CANVAS_OPACITY.1
    };
    let tint = Color32::from_white_alpha((opacity * 255.0).round().clamp(0.0, 255.0) as u8);
    ui.painter()
        .image(tex.id(), rect, crop_uv(tex.size_vec2(), rect.size()), tint);
    let mut mesh = Mesh::default();
    let (top, bottom) = (
        alpha(p.canvas, tokens::CANVAS_FADE.0),
        alpha(p.canvas, tokens::CANVAS_FADE.1),
    );
    for (corner, color) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), bottom),
        (rect.left_bottom(), bottom),
    ] {
        mesh.colored_vertex(corner, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(Shape::mesh(mesh));
}
