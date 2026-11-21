//! Search & Filter (`FilterPopup`): search, status, source, sort, the library view, the card size, how
//! many games are shown, Clear filters and Done. Anchored under its button, kept on the window; Escape
//! and Enter close it.

use crate::constants::{layout, theme as tokens, words_library as w};
use crate::flows::{Action, App};
use crate::model::fill::fill;
use crate::ui::theme::{self, sans, semibold};
use crate::ui::widgets::{button, select};
use egui::{Align, Color32, CornerRadius, Id, Layout, Rect, RichText, Stroke, Ui, vec2};

const STATES: [&str; 4] = ["All states", "Ready to play", "Needs build", "Running"];
const SOURCES: [&str; 5] = [
    "All sources",
    "NUP / NSP packages",
    "NRO homebrew",
    "Extracted folders",
    "Native builds",
];

fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(tokens::FONT_TOOL)));
}

pub fn show(ctx: &egui::Context, app: &mut App, anchor: Rect) -> Vec<Action> {
    let p = theme::of(ctx);
    let mut actions = Vec::new();
    let width = layout::FILTER_WIDTH;
    let screen = ctx.content_rect();
    let x = (anchor.right() - width + layout::POPOVER_OVERHANG)
        .clamp(screen.left() + 8.0, screen.right() - width - 8.0);
    let shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(190),
    };
    let frame = egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(16))
        .shadow(shadow);
    let area = egui::Area::new(Id::unique("filter-popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(x, anchor.bottom() - 4.0));
    let shown = area.show(ctx, |ui| {
        frame.show(ui, |ui| {
            ui.set_width(width - 32.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(crate::constants::menus::FILTER)
                        .font(semibold(tokens::FONT_POPUP_TITLE)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    use crate::ui::widgets::icon_button::{Glyph, add};
                    if add(ui, Glyph::Close, "Close (Esc)", true) {
                        app.filters.open = false;
                    }
                });
            });
            label(ui, "Search");
            let search = ui.add(
                egui::TextEdit::singleline(&mut app.filters.search)
                    .hint_text("Type a game title…")
                    .desired_width(f32::INFINITY)
                    .margin(vec2(9.0, 5.0)),
            );
            search.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search game titles")
            });
            if std::mem::take(&mut app.filters.focus_search) {
                search.request_focus();
            }
            ui.separator();
            let states: Vec<(String, usize)> = STATES
                .iter()
                .enumerate()
                .map(|(i, s)| (s.to_string(), i))
                .collect();
            let sources: Vec<(String, usize)> = SOURCES
                .iter()
                .enumerate()
                .map(|(i, s)| (s.to_string(), i))
                .collect();
            label(ui, "Status");
            select::select(ui, "Filter by build state", &mut app.filters.state, &states);
            label(ui, "Source");
            select::select(
                ui,
                "Filter by source format",
                &mut app.filters.source,
                &sources,
            );
            label(ui, "Sort by");
            let sorts = select::words(&[
                ("Name (A–Z)", Some(0)),
                ("Last played (newest first)", Some(1)),
                ("Build status", Some(2)),
                ("Custom table order", None),
            ]);
            if select::select(ui, "Sort games", &mut app.filters.sort, &sorts) {
                app.sort_preset_changed();
            }
            label(ui, "Library view");
            ui.columns(2, |cols| {
                for (i, (name, mode)) in [
                    (crate::constants::menus::GALLERY, 0),
                    (crate::constants::menus::LIST, 1),
                ]
                .iter()
                .enumerate()
                {
                    let on = app.lib.view_mode == *mode;
                    let ink = if on { p.accent } else { p.text };
                    let b = egui::Button::new(RichText::new(*name).color(ink))
                        .fill(if on { p.selected } else { p.field })
                        .min_size(vec2(cols[i].available_width(), 30.0));
                    if cols[i].add(b).clicked() {
                        actions.push(Action::ViewMode(*mode));
                    }
                }
            });
            label(ui, "Card size");
            let mut size = app.lib.cover_width;
            // the rail in the border ink (the resting fill is the popover's own), the chosen part lit
            let enabled = app.lib.view_mode == 0;
            let slider = ui
                .scope(|ui| {
                    ui.spacing_mut().slider_width = ui.available_width();
                    let rail = theme::of(ui.ctx()).border;
                    ui.visuals_mut().widgets.inactive.bg_fill = rail;
                    ui.visuals_mut().widgets.hovered.bg_fill = rail;
                    let range = layout::COVER_WIDTH.0..=layout::COVER_WIDTH.2;
                    let s = egui::Slider::new(&mut size, range)
                        .show_value(false)
                        .trailing_fill(true);
                    ui.add_enabled(enabled, s)
                })
                .inner;
            app.lib.cover_width = size;
            slider.widget_info(|| {
                egui::WidgetInfo::slider(app.lib.view_mode == 0, size as f64, "Gallery card size")
            });
            if slider.drag_stopped() {
                actions.push(Action::ViewMode(app.lib.view_mode));
            }
            let summary = fill(
                w::RESULT,
                &[
                    &app.visible_games().len().to_string(),
                    &app.lib.games.len().to_string(),
                ],
            );
            ui.label(
                RichText::new(summary)
                    .color(p.muted)
                    .font(sans(tokens::FONT_TOOL)),
            );
            ui.horizontal(|ui| {
                if button::ghost(ui, "Clear filters", true).clicked() {
                    app.filters.clear();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button::primary(ui, &p, "Done", true).clicked() {
                        app.filters.open = false;
                    }
                });
            });
        });
    });
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|at| !shown.response.rect.contains(at) && !anchor.contains(at))
    });
    if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter))
    {
        app.filters.open = false;
    }
    actions
}
