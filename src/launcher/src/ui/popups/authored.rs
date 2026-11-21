//! The authored scaffold's parts: the title header with its close button (`Close <title>`), the
//! connected primary tabs, the footer with its one primary action.

use crate::constants::popups as c;
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
