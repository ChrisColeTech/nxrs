//! The authored scaffold's parts: the title header with its close button (`Close <title>`), the
//! connected primary tabs, the footer with its one primary action.

use crate::constants::popups as c;
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::button;
use egui::{Align, Align2, Layout, Rect, Role, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};

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
    let glyph = (crate::ui::widgets::icon_button::Glyph::Close, name.as_str());
    let pressed = crate::ui::widgets::icon_button::show(ui, close, close_id, glyph, true);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, p.border),
    );
    pressed
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
    footer_bar(ui, p, |_| {}, cancel, Some((primary, enabled, "")))
}

/// Every child window's footer: a hairline; the surface's own verbs (`lead`, ghost buttons) at the
/// leading side; at the trailing side the ghost dismissal (Cancel, No, Close; empty: none) and the
/// one filled primary, last: (label, enabled, tooltip); None: a surface with nothing to commit.
/// (dismissed, primary pressed).
pub fn footer_bar(
    ui: &mut Ui,
    p: &Palette,
    lead: impl FnOnce(&mut Ui),
    ghost: &str,
    primary: Option<(&str, bool, &str)>,
) -> (bool, bool) {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.border));
    ui.add_space(c::GAP + 2.0);
    let mut out = (false, false);
    ui.horizontal(|ui| {
        lead(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some((label, enabled, tip)) = primary {
                out.1 = button::tip(button::primary(ui, p, label, enabled), tip).clicked();
            }
            if !ghost.is_empty() {
                out.0 = button::ghost(ui, ghost, true).clicked();
            }
        });
    });
    out
}

/// The footer at the bottom of the space left (a message's, a list window's).
pub fn footer_at_bottom(
    ui: &mut Ui,
    p: &Palette,
    lead: impl FnOnce(&mut Ui),
    ghost: &str,
    primary: Option<(&str, bool, &str)>,
) -> (bool, bool) {
    ui.add_space((ui.available_height() - c::FOOTER).max(0.0));
    footer_bar(ui, p, lead, ghost, primary)
}

/// An in-window popover's frame (Search & Filter, Build tools): raised fill, border, the shared
/// shadow and the scaffold's inset; its first row is `header`.
pub fn popover_frame(p: &Palette) -> egui::Frame {
    let shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(190),
    };
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(egui::CornerRadius::same(c::POPOVER_RADIUS))
        .inner_margin(egui::Margin::same(c::PAD))
        .shadow(shadow)
}

/// The room left for a body above a footer.
pub fn body_height(ui: &Ui) -> f32 {
    (ui.available_height() - c::FOOTER).max(80.0)
}
