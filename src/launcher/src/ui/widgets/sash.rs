//! A split's handle (the family's `widgets/sash`): a 5px strip with a 1px hairline, invisible at rest,
//! accent on hover, 2px accent while dragging. The drag is 1:1 and only reports a delta; the dock owns
//! every size. A double click asks for the first size back.

use crate::ui::theme;
use egui::{CursorIcon, Id, Rect, Sense, Ui, WidgetInfo, vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// The handle is a vertical bar; dragging moves it left/right.
    Column,
    /// The handle is a horizontal bar; dragging moves it up/down.
    Row,
}

#[derive(Default)]
pub struct SashMove {
    /// Pointer travel along the axis this frame, in points.
    pub delta: f32,
    pub reset: bool,
}

/// `pane` is the handle's accessible name and the saved size of the pane it resizes, which the handle
/// reports as its value.
pub fn sash(ui: &Ui, rect: Rect, id: &str, axis: Axis, pane: (&str, f32)) -> SashMove {
    let mut motion = SashMove::default();
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return motion;
    }
    let response = ui.interact(rect, Id::unique(id), Sense::click_and_drag());
    response.widget_info(|| {
        let mut info = WidgetInfo::labeled(egui::Role::Splitter, true, pane.0);
        info.value = Some(f64::from(pane.1));
        info
    });
    let dragging = response.dragged();
    if response.hovered() || dragging {
        ui.ctx().set_cursor_icon(match axis {
            Axis::Column => CursorIcon::ResizeHorizontal,
            Axis::Row => CursorIcon::ResizeVertical,
        });
        let thick = if dragging { 2.0 } else { 1.0 };
        let line = match axis {
            Axis::Column => Rect::from_center_size(rect.center(), vec2(thick, rect.height())),
            Axis::Row => Rect::from_center_size(rect.center(), vec2(rect.width(), thick)),
        };
        ui.painter()
            .rect_filled(line, 0, theme::of(ui.ctx()).accent);
    }
    if dragging {
        let delta = ui.input(|input| input.pointer.delta());
        motion.delta = match axis {
            Axis::Column => delta.x,
            Axis::Row => delta.y,
        };
    }
    motion.reset = response.double_clicked();
    motion
}
