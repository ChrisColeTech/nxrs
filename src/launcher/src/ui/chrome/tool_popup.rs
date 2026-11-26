//! The engine badge's popup (`showToolPopup`): ready, or the problem; nxrs-build's health; Open
//! Settings. Above the badge, kept on the window; the shared header's X, Escape or a press outside
//! closes it.

use crate::constants::{layout, words_build as w};
use crate::flows::{Action, App};
use crate::ui::popups::authored;
use crate::ui::theme;
use crate::ui::widgets::{button, form};
use egui::{Align, CornerRadius, Id, Layout, Rect, Sense, vec2};

/// The popup; whether it stays open, and the action it asks.
pub fn show(ctx: &egui::Context, app: &App, anchor: Rect) -> (bool, Option<Action>) {
    let p = theme::of(ctx);
    let problem = app.tool_problem();
    let ready = problem.is_empty();
    let (mut keep, mut action) = (true, None);
    let frame = authored::popover_frame(&p);
    let area = egui::Area::new(Id::unique("tool-popup"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(anchor.right_top() - vec2(0.0, 6.0));
    let shown = area.show(ctx, |ui| {
        frame.show(ui, |ui| {
            ui.set_width(layout::TOOL_POPUP_WIDTH);
            let title = if ready {
                w::POPUP_READY
            } else {
                w::POPUP_PROBLEM
            };
            if authored::header(ui, layout::TOOL_POPUP_WIDTH, title, "tool-popup-close") {
                keep = false;
            }
            ui.add_space(crate::constants::popups::GAP);
            if !ready {
                ui.add(egui::Label::new(problem.as_str()).wrap().selectable(true));
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::ZERO, p.border);
            }
            form::selectable(ui, &p, &app.health_text());
            // a Live popover commits nothing: its verb is a ghost button
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button::ghost(ui, w::OPEN_SETTINGS, true).clicked() {
                    action = Some(Action::Settings(0));
                    keep = false;
                }
            });
        });
    });
    let outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|at| !shown.response.rect.contains(at) && !anchor.contains(at))
    });
    if outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        keep = false;
    }
    (keep, action)
}
