//! The authored scaffold's parts: the title header with its close button (`Close <title>`), the
//! connected primary tabs, the footer with its one primary action.

use crate::constants::{popups as c, theme as tokens};
use crate::ui::icons;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::button;
use egui::{
    Align, Align2, CornerRadius, Id, Layout, Rect, Role, Sense, Stroke, StrokeKind, Ui, WidgetInfo,
    pos2, vec2,
};

/// The title and a trailing close button named `Close <title>`; true when it was pressed.
pub fn header(ui: &mut Ui, width: f32, title: &str, close_id: &str) -> bool {
    let p = theme::of(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(width, c::TITLE_HEIGHT), Sense::hover());
    let label = Rect::from_min_max(
        rect.min,
        pos2(rect.right() - c::CLOSE_SIDE - c::GAP, rect.bottom()),
    );
    ui.painter().with_clip_rect(label).text(
        label.left_center(),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(c::TITLE_FONT),
        p.text,
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, title));
    let close = Rect::from_center_size(
        pos2(rect.right() - c::CLOSE_SIDE / 2.0, rect.center().y),
        vec2(c::CLOSE_SIDE, c::CLOSE_SIDE),
    );
    let name = format!("{} {title}", c::CLOSE_NAME);
    let hit = ui.interact(close, Id::unique(close_id), Sense::click());
    hit.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
    let fill = if hit.is_pointer_button_down_on() {
        p.selected
    } else if hit.hovered() {
        p.hover
    } else {
        p.surface
    };
    ui.painter().rect(
        close,
        CornerRadius::same(4),
        fill,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    icons::close_x(ui.painter(), close, p.text);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, p.border),
    );
    hit.on_hover_text(name).clicked()
}

/// Connected tabs (one segmented control, the selected one tinted); the chosen index when clicked.
pub fn tabs(ui: &mut Ui, p: &Palette, id: &str, labels: &[&str], selected: usize) -> Option<usize> {
    let row = ui.available_rect_before_wrap();
    let width = (c::TAB_SEGMENT * labels.len() as f32)
        .min(row.width() - c::TAB_SIDE * 2.0)
        .max(0.0);
    let (rect, _) = ui.allocate_exact_size(vec2(row.width(), c::TAB_HEIGHT), Sense::hover());
    let bar = Rect::from_center_size(rect.center(), vec2(width, c::TAB_HEIGHT));
    let radius = CornerRadius::same(c::RADIUS);
    ui.painter().rect(
        bar,
        radius,
        p.field,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    let segment = bar.width() / labels.len().max(1) as f32;
    let mut chosen = None;
    for (at, label) in labels.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(bar.left() + segment * at as f32, bar.top()),
            vec2(segment, bar.height()),
        );
        let hit = ui.interact(cell.shrink(1.0), Id::unique((id, at)), Sense::click());
        let on = selected == at;
        hit.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, on, *label));
        if on || hit.hovered() {
            ui.painter().rect_filled(
                cell.shrink(1.0),
                CornerRadius::same(c::RADIUS - 1),
                if on { p.selected } else { p.hover },
            );
        }
        let ink = if on { p.accent } else { p.text };
        ui.painter().text(
            cell.center(),
            Align2::CENTER_CENTER,
            *label,
            theme::sans(tokens::FONT_BODY),
            ink,
        );
        if at > 0 {
            ui.painter().vline(
                cell.left(),
                bar.top() + 3.0..=bar.bottom() - 3.0,
                Stroke::new(1.0, p.border),
            );
        }
        if hit.clicked() {
            chosen = Some(at);
        }
    }
    chosen
}

/// The footer: a hairline, then Cancel and the primary action at the right (primary last).
/// (cancelled, primary pressed).
pub fn footer(
    ui: &mut Ui,
    p: &Palette,
    primary: &str,
    enabled: bool,
    cancel: &str,
) -> (bool, bool) {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.border));
    ui.add_space(c::GAP + 2.0);
    let mut out = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        out.1 = button::primary(ui, p, primary, enabled).clicked();
        if !cancel.is_empty() {
            out.0 = button::ghost(ui, cancel, true).clicked();
        }
    });
    out
}

/// The room left for a body above a footer.
pub fn body_height(ui: &Ui) -> f32 {
    (ui.available_height() - c::FOOTER).max(80.0)
}
