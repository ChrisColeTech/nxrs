//! The grips of a window with no native frame: four edges and four corners; a press on one starts the
//! OS's own resize in that direction.

use crate::constants::child_window as c;
use egui::{
    CursorIcon, Id, Rect, Sense, Ui, ViewportCommand, pos2, vec2, viewport::ResizeDirection,
};

fn all(whole: Rect) -> [(Rect, ResizeDirection, CursorIcon); 8] {
    use ResizeDirection::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
    let g = c::GRIP;
    let (l, t, r, b) = (whole.left(), whole.top(), whole.right(), whole.bottom());
    let side = Rect::from_min_max;
    let corner = |at| Rect::from_min_size(at, vec2(g, g));
    [
        (
            side(pos2(l + g, t), pos2(r - g, t + g)),
            North,
            CursorIcon::ResizeVertical,
        ),
        (
            side(pos2(l + g, b - g), pos2(r - g, b)),
            South,
            CursorIcon::ResizeVertical,
        ),
        (
            side(pos2(l, t + g), pos2(l + g, b - g)),
            West,
            CursorIcon::ResizeHorizontal,
        ),
        (
            side(pos2(r - g, t + g), pos2(r, b - g)),
            East,
            CursorIcon::ResizeHorizontal,
        ),
        (corner(pos2(l, t)), NorthWest, CursorIcon::ResizeNwSe),
        (corner(pos2(r - g, t)), NorthEast, CursorIcon::ResizeNeSw),
        (corner(pos2(l, b - g)), SouthWest, CursorIcon::ResizeNeSw),
        (
            corner(pos2(r - g, b - g)),
            SouthEast,
            CursorIcon::ResizeNwSe,
        ),
    ]
}

/// Lays the grips over the window's edges, before the content (a control at an edge wins).
pub fn lay(ui: &Ui, key: &str, whole: Rect) {
    for (at, (rect, direction, icon)) in all(whole).into_iter().enumerate() {
        let grip = ui.interact(rect, Id::unique((c::GRIP_ID, key, at)), Sense::drag());
        let grip = grip.on_hover_cursor(icon);
        if grip.drag_started() {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}
