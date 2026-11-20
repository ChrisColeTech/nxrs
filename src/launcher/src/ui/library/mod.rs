//! The library area: the background canvas, the gallery of cards, the details list, the empty text.
pub mod artwork_cache;
pub mod canvas;
pub mod card;
pub mod card_buttons;
pub mod gallery;
pub mod list;

use crate::constants::{theme as tokens, words_library as w};
use crate::flows::App;
use crate::ui::theme::{self, sans};
use egui::{Rect, Ui, pos2};

/// "Your library is empty …" / "No games match these filters …", centred.
pub fn empty(ui: &Ui, app: &App, rect: Rect) {
    let p = theme::of(ui.ctx());
    let text = if app.lib.games.is_empty() {
        w::EMPTY
    } else {
        w::NO_MATCH
    };
    let mut job =
        egui::text::LayoutJob::simple(text.into(), sans(tokens::FONT_EMPTY), p.muted, rect.width());
    job.halign = egui::Align::Center;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(rect.center().x, rect.center().y - galley.size().y / 2.0),
        galley,
        p.muted,
    );
}
