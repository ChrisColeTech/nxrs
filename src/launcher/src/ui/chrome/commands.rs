//! The Commands strip (`createMenusAndToolbar`): Add game with its drop-down (Add game, Add extracted
//! folder…, Import build…) at the left, Search & Filter (with its active count) at the right.

use super::{menu_draw, menu_model};
use crate::constants::{layout, menus as m, theme as tokens, words_library as w};
use crate::flows::{Action, App};
use crate::model::fill::fill;
use crate::ui::icons;
use crate::ui::theme::{self, Palette, sans};
use egui::{Align2, CornerRadius, Id, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

fn tool(ui: &mut Ui, p: &Palette, r: Rect, (icon, label): (&str, &str), on: bool) -> Response {
    let resp = ui.interact(
        r,
        Id::unique(("command", label.len(), icon)),
        Sense::click(),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let fill = if on {
        Some(p.selected)
    } else if resp.hovered() || resp.has_focus() {
        Some(p.hover)
    } else {
        None
    };
    if let Some(f) = fill {
        ui.painter().rect_filled(r, CornerRadius::same(5), f);
    }
    let icon_rect = Rect::from_center_size(
        pos2(r.center().x, r.top() + 6.0 + layout::TOOL_ICON / 2.0),
        vec2(layout::TOOL_ICON, layout::TOOL_ICON),
    );
    icons::command(ui.painter(), icon_rect, icon, p);
    ui.painter().text(
        pos2(r.center().x, r.bottom() - 9.0),
        Align2::CENTER_CENTER,
        label,
        sans(tokens::FONT_TOOL),
        p.text,
    );
    resp
}

/// The strip; the action chosen and the Search & Filter button's rectangle (the popover's anchor).
pub fn show(ui: &mut Ui, app: &App, add_open: &mut bool) -> (Option<Action>, Rect) {
    let p = theme::of(ui.ctx());
    let rect = Rect::from_min_size(
        ui.max_rect().min,
        vec2(ui.max_rect().width(), layout::COMMANDS_HEIGHT),
    );
    ui.allocate_rect(rect, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, p.surface);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, p.border),
    );
    let mut action = None;
    let add = Rect::from_min_size(
        pos2(rect.left() + 8.0, rect.top() + 4.0),
        vec2(layout::TOOL_BUTTON[0], layout::TOOL_BUTTON[1]),
    );
    if tool(ui, &p, add, ("add", m::ADD_GAME), false)
        .on_hover_text(m::TIP_ADD)
        .clicked()
    {
        action = Some(Action::AddGame);
    }
    let arrow = Rect::from_min_size(add.right_top(), vec2(layout::ADD_ARROW, add.height()));
    let more = ui.interact(arrow, Id::unique("command-add-menu"), Sense::click());
    more.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, m::ADD_OPTIONS));
    if more.hovered() || *add_open {
        ui.painter()
            .rect_filled(arrow, CornerRadius::same(5), p.hover);
    }
    icons::chevron(ui.painter(), arrow.center(), p.text);
    if more.clicked() {
        *add_open = !*add_open;
    }
    if *add_open {
        let area = egui::Area::new(Id::unique("add-game-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(add.left_bottom() + vec2(0.0, 2.0));
        let shown = area.show(ui.ctx(), |ui| {
            menu_draw::frame(&p)
                .show(ui, |ui| menu_draw::items(ui, &menu_model::add_menu(), &p))
                .inner
        });
        let outside = ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|at| !shown.response.rect.contains(at) && !arrow.contains(at))
        });
        if let Some(a) = shown.inner {
            action = Some(a);
        }
        if action.is_some() || outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *add_open = false;
        }
    }
    let label = menu_model::filter_label(app);
    let width = (ui
        .painter()
        .layout_no_wrap(label.clone(), sans(tokens::FONT_TOOL), p.text)
        .size()
        .x
        + 22.0)
        .max(77.0);
    let filter = Rect::from_min_size(
        pos2(rect.right() - 8.0 - width, rect.top() + 4.0),
        vec2(width, layout::TOOL_BUTTON[1]),
    );
    let n = app.filters.query().active_count();
    let tip = if n > 0 {
        fill(w::FILTERS_ACTIVE, &[&n.to_string()])
    } else {
        m::TIP_FILTER.to_string()
    };
    if tool(ui, &p, filter, ("filter", &label), app.filters.open)
        .on_hover_text(tip)
        .clicked()
    {
        action = Some(Action::ToggleFilters);
    }
    (action, filter)
}
