//! A card's two buttons: the primary (Play, Build, Rebuild, Stop, Cancel build, with its painted icon;
//! its tooltip is the status, or the build's progress) and Properties.

use crate::constants::theme as tokens;
use crate::flows::view_state::BuildProgress;
use crate::model::card_state::CardState;
use crate::ui::icons;
use crate::ui::theme::{Palette, semibold};
use egui::{CornerRadius, Id, Rect, Response, Sense, Ui, pos2, vec2};

pub fn primary(
    ui: &mut Ui,
    p: &Palette,
    (id, rect, title): (Id, Rect, &str),
    state: &CardState,
    progress: &BuildProgress,
) -> Response {
    let enabled = state.action_enabled;
    let resp = ui.interact(
        rect,
        id.with("primary"),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let label = format!("{} {title}", state.action);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label));
    let hot = enabled && (resp.hovered() || resp.has_focus());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(5),
        if hot { p.selected } else { p.raised },
    );
    let ink = if !enabled {
        p.disabled()
    } else if hot {
        p.accent
    } else {
        p.text
    };
    let galley =
        ui.painter()
            .layout_no_wrap(state.action.clone(), semibold(tokens::FONT_BODY), ink);
    let x0 = rect.center().x - (22.0 + galley.size().x) / 2.0;
    icons::card(
        ui.painter(),
        Rect::from_min_size(pos2(x0, rect.center().y - 8.0), vec2(16.0, 16.0)),
        &state.action,
        if enabled { p.text } else { ink },
        p,
    );
    ui.painter().galley(
        pos2(x0 + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    let tip = match (state.building, progress.total) {
        (true, 0) => progress.phase.clone(),
        (true, total) => format!("{}: {}/{total}", progress.phase, progress.current),
        _ => state.status.clone(),
    };
    resp.on_hover_text(tip)
}

pub fn properties(ui: &mut Ui, p: &Palette, (id, rect): (Id, Rect), title: &str) -> Response {
    let resp = ui.interact(rect, id.with("properties"), Sense::click());
    let name = format!("Properties for {title}");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    let hot = resp.hovered() || resp.has_focus();
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(5),
        if hot { p.selected } else { p.raised },
    );
    icons::card(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(18.0, 18.0)),
        "properties",
        p.text,
        p,
    );
    resp.on_hover_text(format!("Properties — {title}"))
}
