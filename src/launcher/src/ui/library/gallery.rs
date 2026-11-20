//! The gallery: a wrapping grid of cards (width from the slider, height `w * 0.64 + 78`), only the
//! visible rows built; the arrow keys move the selection.

use super::artwork_cache::ArtworkCache;
use super::card;
use crate::constants::layout;
use crate::flows::{Action, App};
use crate::model::game::Game;
use egui::{Rect, Ui, vec2};

pub fn card_height(width: f32) -> f32 {
    (width * layout::COVER_SHARE).floor() + layout::CARD_EXTRA
}

/// The selection moved by an arrow key: `step` within `games`.
pub fn arrow(ui: &Ui, app: &App, games: &[Game], columns: usize) -> Option<Action> {
    if ui.memory(|m| m.focused().is_some()) || !app.surfaces.is_empty() || app.filters.open {
        return None;
    }
    let step = ui.input(|i| {
        if i.key_pressed(egui::Key::ArrowRight) {
            1
        } else if i.key_pressed(egui::Key::ArrowLeft) {
            -1
        } else if i.key_pressed(egui::Key::ArrowDown) {
            columns as i64
        } else if i.key_pressed(egui::Key::ArrowUp) {
            -(columns as i64)
        } else {
            0
        }
    });
    let at = games.iter().position(|g| g.id == app.selected)?;
    let next = (at as i64 + step).clamp(0, games.len() as i64 - 1) as usize;
    (step != 0).then(|| Action::Select(games.get(next).map(|g| g.id.clone()).unwrap_or_default()))
}

pub fn show(ui: &mut Ui, app: &mut App, art: &mut ArtworkCache, games: &[Game]) -> Option<Action> {
    let w = app.lib.cover_width as f32;
    let h = card_height(w);
    let (cell_w, cell_h) = (
        w + layout::CARD_GAP + layout::GALLERY_SPACING,
        h + layout::CARD_GAP + layout::GALLERY_SPACING,
    );
    let columns = (((ui.available_width() - 16.0) / cell_w).floor() as usize).max(1);
    let rows = games.len().div_ceil(columns);
    let mut out = arrow(ui, app, games, columns);
    if out.is_some() {
        app.scroll_to_selected = true;
    }
    let mut area = egui::ScrollArea::vertical()
        .id_salt("gallery")
        .auto_shrink([false, false]);
    if std::mem::take(&mut app.scroll_to_selected)
        && let Some(row) = games
            .iter()
            .position(|g| g.id == app.selected)
            .map(|i| i / columns)
    {
        let offset = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(egui::Id::unique("gallery-offset")))
            .unwrap_or(0.0);
        let (top, view) = (row as f32 * cell_h, ui.available_height());
        if top < offset || top + cell_h > offset + view {
            area = area.vertical_scroll_offset((top - 8.0).max(0.0));
        }
    }
    let shown = area.show_viewport(ui, |ui, viewport| {
        ui.set_height(rows as f32 * cell_h + 16.0);
        let origin = ui.min_rect().min + vec2(8.0, 8.0);
        let first = ((viewport.top() - 8.0) / cell_h).floor().max(0.0) as usize;
        let last = ((viewport.bottom() / cell_h).ceil() as usize + 1).min(rows);
        for row in first..last {
            for col in 0..columns {
                let Some(game) = games.get(row * columns + col) else {
                    break;
                };
                let at = origin + vec2(col as f32 * cell_w + 6.0, row as f32 * cell_h + 6.0);
                if let Some(a) = card::show(ui, app, art, game, Rect::from_min_size(at, vec2(w, h)))
                {
                    out = Some(a);
                }
            }
        }
    });
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::unique("gallery-offset"),
            shown.state.clamped_offset().y,
        )
    });
    out
}
