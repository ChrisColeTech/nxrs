//! Buttons: the raised default and the one filled accent action of a surface (Qt's `AccentButton`).
//! A disabled button says why in its tooltip when there is a reason.

use crate::constants::popups as c;
use crate::ui::theme::{self, Palette};
use egui::{Button, Response, RichText, Stroke, Ui, vec2};

/// The raised button.
pub fn ghost(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    ui.add_enabled(
        enabled,
        Button::new(label).min_size(vec2(c::BUTTON_MIN, c::BUTTON)),
    )
}

/// The surface's one filled accent action.
pub fn primary(ui: &mut Ui, p: &Palette, label: &str, enabled: bool) -> Response {
    let ink = if enabled { p.accent_text } else { p.disabled() };
    let text = RichText::new(label)
        .color(ink)
        .font(theme::semibold(crate::constants::theme::FONT_BODY));
    let fill = if enabled { p.accent } else { p.raised };
    let edge = if enabled { p.accent } else { p.border };
    ui.add_enabled(
        enabled,
        Button::new(text)
            .fill(fill)
            .stroke(Stroke::new(1.0, edge))
            .min_size(vec2(c::BUTTON_MIN, c::BUTTON)),
    )
}

/// A tooltip, or the reason a disabled control is disabled.
pub fn tip(response: Response, text: &str) -> Response {
    if text.is_empty() {
        response
    } else if response.enabled() {
        response.on_hover_text(text)
    } else {
        response.on_disabled_hover_text(text)
    }
}
