//! The status bar (`createStatusBar`): the game count, the build's bar, the selected game (with its
//! status), the job line, the engine badge (a click opens its popup), the Settings button, a size grip.

use crate::constants::{layout, menus as m, theme as tokens};
use crate::flows::{Action, App};
use crate::ui::icons;
use crate::ui::theme::{self, Palette, sans, semibold};
use egui::{Align2, CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

fn pill(
    ui: &mut Ui,
    p: &Palette,
    x_right: f32,
    row: Rect,
    (label, ink): (&str, egui::Color32),
    id: &str,
    icon: bool,
) -> (egui::Response, f32) {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), semibold(tokens::FONT_SMALL), ink);
    let width = galley.size().x + 20.0 + if icon { 17.0 } else { 0.0 };
    let r = Rect::from_center_size(
        pos2(x_right - width / 2.0, row.center().y),
        vec2(width, 19.0),
    );
    let resp = ui.interact(r, Id::unique(id), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if !icon || resp.hovered() {
        ui.painter()
            .rect_filled(r, CornerRadius::same(9), p.selected);
    }
    let text_x = if icon {
        icons::command(
            ui.painter(),
            Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(13.0, 13.0)),
            "settings",
            p,
        );
        r.left() + 27.0
    } else {
        r.left() + 10.0
    };
    let ink = if icon && resp.hovered() { p.text } else { ink };
    ui.painter().galley(
        pos2(text_x, r.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    (
        resp.on_hover_cursor(egui::CursorIcon::PointingHand),
        r.left() - 6.0,
    )
}

/// The bar; the action chosen and the badge's rectangle (the popup's anchor).
pub fn show(ui: &mut Ui, app: &App) -> (Option<Action>, Rect) {
    let p = theme::of(ui.ctx());
    let row = ui.max_rect();
    ui.painter().rect_filled(row, CornerRadius::ZERO, p.surface);
    ui.painter()
        .hline(row.x_range(), row.top() + 0.5, Stroke::new(1.0, p.border));
    let mut action = None;
    // from the right: the grip, Settings, the badge, the job line
    let grip = row.right() - 4.0;
    for k in 0..3 {
        let d = 4.0 * k as f32;
        ui.painter().line_segment(
            [
                pos2(grip - 2.0 - d, row.bottom() - 3.0),
                pos2(grip - 2.0, row.bottom() - 3.0 - d),
            ],
            Stroke::new(1.0, p.muted),
        );
    }
    let (settings, x) = pill(
        ui,
        &p,
        row.right() - 18.0,
        row,
        (m::SETTINGS, p.muted),
        "status-settings",
        true,
    );
    if settings.on_hover_text(m::TIP_SETTINGS).clicked() {
        action = Some(Action::Settings(0));
    }
    let (label, ok) = app.badge();
    let ink = if ok {
        p.accent
    } else {
        theme::hex(tokens::BADGE_ERROR)
    };
    let (badge, x) = pill(ui, &p, x, row, (label, ink), "engine-badge", false);
    let problem = app.tool_problem();
    let tip = if problem.is_empty() {
        app.native(&app.tools.nxrs_build)
    } else {
        problem
    };
    let badge_rect = badge.rect;
    if badge
        .on_hover_text(format!("{tip}\nClick for details"))
        .clicked()
    {
        action = Some(Action::ToolPopup);
    }
    let job = ui
        .painter()
        .layout_no_wrap(app.job_label.clone(), sans(tokens::FONT_BODY), p.muted);
    let job_width = job.size().x.min(layout::JOB_MAX);
    let job_rect = Rect::from_min_max(pos2(x - job_width, row.top()), pos2(x, row.bottom()));
    ui.painter().with_clip_rect(job_rect).galley(
        pos2(job_rect.left(), row.center().y - job.size().y / 2.0),
        job,
        p.muted,
    );
    // from the left: the count, the bar, the selection
    let mut left = row.left() + 8.0;
    let count = app.count_text();
    let cr = ui.painter().text(
        pos2(left, row.center().y),
        Align2::LEFT_CENTER,
        &count,
        sans(tokens::FONT_BODY),
        p.muted,
    );
    left += cr.width().max(65.0) + 10.0;
    if let Some((current, total)) = app.bar {
        let bar = Rect::from_min_size(
            pos2(left, row.center().y - 3.5),
            vec2(layout::STATUS_PROGRESS[0], layout::STATUS_PROGRESS[1]),
        );
        progress(ui, &p, bar, current, total);
        left = bar.right() + 10.0;
    }
    let room = (job_rect.left() - 10.0 - left).clamp(0.0, layout::SELECTION_MAX);
    let text = app.selection_text();
    let shown = Rect::from_min_size(pos2(left, row.top()), vec2(room, row.height()));
    let galley = ui
        .painter()
        .layout_no_wrap(text.clone(), sans(tokens::FONT_BODY), p.text);
    ui.painter().with_clip_rect(shown).galley(
        pos2(left, row.center().y - galley.size().y / 2.0),
        galley,
        p.text,
    );
    let source = app
        .selected_game()
        .map(|g| crate::model::path_text::display(&g.source))
        .unwrap_or_default();
    ui.interact(shown, Id::unique("selection-line"), Sense::hover())
        .on_hover_text(format!("{text}\n{source}"));
    (action, badge_rect)
}

/// The build's bar: determinate, or a busy sweep when the total is unknown.
fn progress(ui: &Ui, p: &Palette, bar: Rect, current: i64, total: i64) {
    ui.painter()
        .rect_filled(bar, CornerRadius::same(3), p.border);
    let (from, share) = if total > 0 {
        (0.0, (current as f32 / total as f32).clamp(0.0, 1.0))
    } else {
        ui.ctx().request_repaint();
        (((ui.input(|i| i.time) as f32) * 0.6).fract() * 0.75, 0.25)
    };
    let seg = Rect::from_min_size(
        pos2(bar.left() + bar.width() * from, bar.top()),
        vec2(bar.width() * share, bar.height()),
    );
    ui.painter()
        .rect_filled(seg, CornerRadius::same(3), p.accent);
}
