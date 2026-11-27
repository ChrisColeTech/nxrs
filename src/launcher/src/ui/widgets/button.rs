//! Buttons (the family's style guide, "Buttons and surfaces"): the ghost default and the one filled
//! accent action of a surface. Icon-only buttons are `icon_button`.
//! A disabled button says why in its tooltip when there is a reason.

use crate::constants::popups as c;
use crate::ui::theme::{self, Palette};
use egui::{Button, Response, RichText, Stroke, Ui, vec2};

/// The ghost button, the family's default (a dialog's own verbs, row and panel verbs): no frame at rest,
/// the hover fill under the pointer. Outlines belong to the title bar alone.
pub fn ghost(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let button = Button::new(label).frame_when_inactive(false);
    ui.add_enabled(enabled, button.min_size(vec2(c::BUTTON_MIN, c::BUTTON)))
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

/// An option of a Choices box (Play's Native or Web): equal to its siblings, none filled; the
/// raised fill and border so the options read as buttons side by side, the hover fill under the
/// pointer.
pub fn choice(ui: &mut Ui, p: &Palette, label: &str, enabled: bool) -> Response {
    let ink = if enabled { p.text } else { p.disabled() };
    let text = RichText::new(label)
        .color(ink)
        .font(theme::semibold(crate::constants::theme::FONT_BODY));
    ui.add_enabled(
        enabled,
        Button::new(text)
            .fill(p.raised)
            .stroke(Stroke::new(1.0, p.border))
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
