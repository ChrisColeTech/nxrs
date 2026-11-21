//! A gallery card (`GameCard`): the cover (artwork or the placeholder, the attention badge, the build's
//! progress strip) over the whole tile, the title on a fade at its foot, the primary button and
//! Properties.

use super::artwork_cache::ArtworkCache;
use crate::constants::{layout, theme as tokens, words_library as w};
use crate::flows::{Action, App};
use crate::model::game::Game;
use crate::model::path_text::native;
use crate::ui::theme::{self, semibold};
use egui::{CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

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
    let tile = Rect::from_min_max(rect.min, footer.max);
    let error = super::card_art::cover(ui, &p, art.get(&game.cover), (tile, cover_rect));
    if state.needs_attention {
        super::card_art::badge(ui, &p, cover_rect, &state.status);
    }
    if state.building && app.progress.total > 0 {
        let strip = Rect::from_min_max(
            pos2(tile.left() + 5.0, tile.bottom() - 3.0),
            tile.max - vec2(5.0, 0.0),
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
    // the title over a fade, not a bar: the artwork runs to the tile's foot
    super::card_art::fade(
        ui,
        Rect::from_min_max(pos2(tile.left(), footer.top() - 28.0), tile.max),
    );
    if selected {
        let edge = Stroke::new(2.0, p.accent);
        ui.painter()
            .rect_stroke(tile, CornerRadius::same(5), edge, egui::StrokeKind::Inside);
    }
    let ink = egui::Color32::from_gray(245);
    let galley = ui.painter().layout(
        game.title.clone(),
        semibold(tokens::FONT_BODY),
        ink,
        footer.width() - 18.0,
    );
    let gh = galley.size().y.min(footer.height() - 4.0);
    ui.painter().with_clip_rect(footer).galley(
        pos2(footer.left() + 9.0, footer.center().y - gh / 2.0),
        galley,
        ink,
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
