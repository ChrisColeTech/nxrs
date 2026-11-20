//! A gallery card (`GameCard`): the cover (artwork or the placeholder, the attention badge, the build's
//! progress strip), the title footer, the primary button and Properties.

use super::artwork_cache::{Art, ArtworkCache, crop_uv};
use crate::constants::{layout, theme as tokens, words_library as w};
use crate::flows::{Action, App};
use crate::model::game::Game;
use crate::model::path_text::native;
use crate::ui::icons;
use crate::ui::theme::{self, Palette, alpha, sans, semibold};
use egui::{Align2, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

fn cover(ui: &Ui, p: &Palette, art: Option<Art>, rect: Rect) -> String {
    let top = CornerRadius {
        nw: 5,
        ne: 5,
        sw: 0,
        se: 0,
    };
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
    icons::gamepad(ui.painter(), rect.center() - vec2(0.0, 7.0), p);
    let text = if error.is_empty() {
        w::NO_ARTWORK
    } else {
        w::BAD_ARTWORK
    };
    ui.painter().text(
        pos2(rect.center().x, rect.bottom() - 13.0),
        Align2::CENTER_BOTTOM,
        text,
        sans(tokens::FONT_SMALL),
        p.muted,
    );
    error
}

fn badge(ui: &Ui, p: &Palette, cover: Rect, status: &str) {
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

pub fn show(
    ui: &mut Ui,
    app: &App,
    art: &mut ArtworkCache,
    game: &Game,
    rect: Rect,
) -> Option<Action> {
    let p = theme::of(ui.ctx());
    let mut out = None;
    let state = app.card_of(game);
    let cover_rect = Rect::from_min_size(
        rect.min,
        vec2(rect.width(), (rect.width() * layout::COVER_SHARE).floor()),
    );
    let footer = Rect::from_min_size(
        cover_rect.left_bottom(),
        vec2(rect.width(), layout::CARD_FOOTER),
    );
    let row = Rect::from_min_size(
        footer.left_bottom() + vec2(0.0, 6.0),
        vec2(rect.width(), layout::CARD_BUTTON),
    );
    let primary = Rect::from_min_max(
        row.min,
        pos2(row.right() - layout::CARD_PROPERTIES - 5.0, row.bottom()),
    );
    let props = Rect::from_min_max(
        pos2(row.right() - layout::CARD_PROPERTIES, row.top()),
        row.max,
    );
    let id = Id::unique(("card", &game.id));
    let whole = ui.interact(Rect::from_min_max(rect.min, footer.max), id, Sense::click());
    let name = format!("{}, {}", game.title, state.status);
    whole.widget_info(|| egui::WidgetInfo::labeled(egui::Role::GenericContainer, true, &name));
    let error = cover(ui, &p, art.get(&game.cover), cover_rect);
    if state.needs_attention {
        badge(ui, &p, cover_rect, &state.status);
    }
    if state.building && app.progress.total > 0 {
        let strip = Rect::from_min_max(
            pos2(cover_rect.left(), cover_rect.bottom() - 3.0),
            cover_rect.max,
        );
        ui.painter()
            .rect_filled(strip, CornerRadius::ZERO, p.border);
        let share = (app.progress.current as f32 / app.progress.total as f32).clamp(0.0, 1.0);
        ui.painter().rect_filled(
            Rect::from_min_size(strip.min, vec2(strip.width() * share, 3.0)),
            CornerRadius::ZERO,
            p.accent,
        );
    }
    let selected = app.selected == game.id;
    ui.painter().rect_filled(
        footer,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: 5,
            se: 5,
        },
        if selected { p.selected } else { p.raised },
    );
    let galley = ui.painter().layout(
        game.title.clone(),
        semibold(tokens::FONT_BODY),
        p.text,
        footer.width() - 18.0,
    );
    let gh = galley.size().y.min(footer.height() - 4.0);
    ui.painter().with_clip_rect(footer).galley(
        pos2(footer.left() + 9.0, footer.center().y - gh / 2.0),
        galley,
        p.text,
    );
    let art_tip = if !error.is_empty() {
        format!("{}{error}", w::ARTWORK_ERROR)
    } else if game.cover.is_empty() {
        w::CHOOSE_ARTWORK.into()
    } else {
        native(&game.cover)
    };
    let tip = if state.needs_attention {
        format!("{}\n{art_tip}", state.status)
    } else {
        art_tip
    };
    let whole = whole.on_hover_text(tip);
    if whole.clicked() || whole.secondary_clicked() {
        out = Some(Action::Select(game.id.clone()));
    }
    if whole.double_clicked() {
        out = Some(Action::CardActivate(game.id.clone()));
    }
    super::list::context_menu(&whole, app, &mut out);
    let prim =
        super::card_buttons::primary(ui, &p, (id, primary, &game.title), &state, &app.progress);
    if prim.clicked() {
        out = Some(Action::CardPrimary(game.id.clone()));
    }
    if super::card_buttons::properties(ui, &p, (id, props), &game.title).clicked() {
        out = Some(Action::CardProperties(game.id.clone()));
    }
    out
}
