//! The scrim over the main window while a modal child window is open: it dims the main window, swallows
//! every press on it, and a press brings the open window to the front with the keyboard. The caption
//! cells at the right of the title bar are left uncovered, so the app can still be closed or moved.

use super::child_window;
use crate::constants::child_window as c;
use egui::{Area, Color32, Context, Id, Order, Rect, Sense, pos2};

pub fn wash(light: bool) -> Color32 {
    let [r, g, b, a] = if light { c::SCRIM_LIGHT } else { c::SCRIM_DARK };
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Covers the main window for the modal window `key`; `bar` is the title bar's height and `strip`
/// the width of its caption cells.
pub fn draw(ctx: &Context, key: &str, bar: f32, strip: f32) {
    let screen = ctx.content_rect();
    let under = screen.top() + bar;
    let covers = [
        Rect::from_min_max(pos2(screen.left(), under), screen.max),
        Rect::from_min_max(screen.min, pos2(screen.right() - strip, under)),
    ];
    let light = crate::ui::theme::of(ctx).light;
    for (at, rect) in covers.into_iter().enumerate() {
        Area::new(Id::unique((c::SCRIM_ID, key, at)))
            .order(Order::Middle)
            .fixed_pos(rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, press) = ui.allocate_exact_size(rect.size(), Sense::click_and_drag());
                ui.painter().rect_filled(rect, 0, wash(light));
                if press.is_pointer_button_down_on() {
                    child_window::focus(ctx, key);
                }
            });
    }
}
