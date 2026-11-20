//! The details list: Name, Status, Format, Renderer, Last played, Build folder; sortable by a header
//! click (the popover's Sort follows), resizable columns, 32-point rows, alternating colours.

use crate::constants::{layout, theme as tokens};
use crate::flows::{Action, App};
use crate::model::dates;
use crate::model::game::Game;
use crate::model::path_text::display;
use crate::ui::chrome::{menu_draw, menu_model};
use crate::ui::theme::{self, semibold};
use egui::{Align2, CornerRadius, Sense, Shape, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};

/// The game context menu on a card or a row.
pub fn context_menu(resp: &egui::Response, app: &App, out: &mut Option<Action>) {
    let p = theme::of(&resp.ctx);
    let items = menu_model::context(app);
    resp.context_menu(|ui| {
        if let Some(a) = menu_draw::items(ui, &items, &p) {
            *out = Some(a);
        }
    });
}

fn header(ui: &mut Ui, app: &App, i: usize, name: &str) -> bool {
    let p = theme::of(ui.ctx());
    let r = ui.max_rect().expand2(vec2(4.0, 0.0));
    let resp = ui.interact(r, ui.scope_id().with(("header", i)), Sense::click());
    ui.painter().rect_filled(
        r,
        CornerRadius::ZERO,
        if resp.hovered() { p.hover } else { p.raised },
    );
    ui.painter()
        .vline(r.right() - 0.5, r.y_range(), Stroke::new(1.0, p.border));
    ui.painter()
        .hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.border));
    ui.painter().text(
        pos2(r.left() + 8.0, r.center().y),
        Align2::LEFT_CENTER,
        name,
        semibold(tokens::FONT_BODY),
        p.text,
    );
    if app.table_sort.0 == i {
        let (x, y) = (r.right() - 14.0, r.center().y);
        let d = if app.table_sort.1 { -1.0 } else { 1.0 };
        let tri = vec![
            pos2(x - 4.0, y - 2.0 * d),
            pos2(x + 4.0, y - 2.0 * d),
            pos2(x, y + 3.0 * d),
        ];
        ui.painter()
            .add(Shape::convex_polygon(tri, p.muted, Stroke::NONE));
    }
    resp.clicked()
}

pub fn show(ui: &mut Ui, app: &mut App, games: &[Game]) -> Option<Action> {
    let p = theme::of(ui.ctx());
    let mut out = super::gallery::arrow(ui, app, games, 1);
    ui.painter()
        .rect_filled(ui.max_rect(), CornerRadius::ZERO, p.field);
    // rows run edge to edge; the resize lines show only on a header's edge, as in Qt's list
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::NONE;
    let mut table = TableBuilder::new(ui)
        .striped(false)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
    for w in layout::COLUMN_WIDTHS {
        table = table.column(Column::initial(w).at_least(40.0).clip(true));
    }
    table = table
        .column(Column::remainder().at_least(80.0).clip(true))
        .min_scrolled_height(0.0);
    if (std::mem::take(&mut app.scroll_to_selected) || out.is_some())
        && let Some(row) = games.iter().position(|g| g.id == app.selected)
    {
        table = table.scroll_to_row(row, None);
    }
    let mut clicked = None;
    let app_ref: &App = app;
    table
        .header(layout::HEADER_HEIGHT, |mut h| {
            for (i, name) in layout::COLUMNS.iter().enumerate() {
                h.col(|ui| {
                    if header(ui, app_ref, i, name) {
                        clicked = Some(i);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(layout::ROW_HEIGHT, games.len(), |mut row| {
                let Some(g) = games.get(row.index()) else {
                    return;
                };
                let (selected, status) = (g.id == app_ref.selected, app_ref.status_of(g));
                let cells = [
                    g.title.clone(),
                    status.clone(),
                    g.format().into(),
                    g.renderer.clone(),
                    dates::list(&g.last_played),
                    display(&g.build_dir),
                ];
                let stripe = if row.index() % 2 == 1 {
                    p.surface
                } else {
                    p.field
                };
                for text in &cells {
                    row.col(|ui| {
                        let r = ui.max_rect().expand2(vec2(4.0, 0.0));
                        let fill = if selected {
                            p.selected
                        } else if ui.rect_contains_pointer(r) {
                            p.hover
                        } else {
                            stripe
                        };
                        ui.painter().rect_filled(r, CornerRadius::ZERO, fill);
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).color(p.text))
                                .truncate()
                                .selectable(false),
                        )
                        .on_hover_text(text);
                    });
                }
                let resp = row.response();
                let name = format!("{}, {status}", g.title);
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::GenericContainer, true, &name)
                });
                if resp.clicked() || resp.secondary_clicked() {
                    out = Some(Action::Select(g.id.clone()));
                }
                if resp.double_clicked() {
                    out = Some(Action::Activate);
                }
                context_menu(&resp, app_ref, &mut out);
            });
        });
    if let Some(col) = clicked {
        app.sort_by_column(col);
    }
    out
}
