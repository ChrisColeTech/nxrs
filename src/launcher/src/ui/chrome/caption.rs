//! The window buttons (`CaptionButton`) and the window's state: Windows answers the caption through its
//! own hit test (`services::platform::windows_caption`); elsewhere egui moves and resizes the window.

use crate::constants::{layout, theme as tokens};
use crate::ui::theme::{self, Palette};
use egui::{
    Color32, Context, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, ViewportCommand, pos2,
    vec2,
};

#[cfg(windows)]
use crate::services::platform::windows_caption as native;

pub fn native_caption() -> bool {
    #[cfg(windows)]
    return native::installed();
    #[cfg(not(windows))]
    false
}
pub fn maximized(ctx: &Context) -> bool {
    #[cfg(windows)]
    if native::installed() {
        return native::is_maximized();
    }
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}
pub fn toggle_maximized(ctx: &Context) {
    #[cfg(windows)]
    if native::installed() {
        return native::toggle_maximized();
    }
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
}
fn minimize(ctx: &Context) {
    #[cfg(windows)]
    if native::installed() {
        return native::minimize();
    }
    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
}
/// Points of the bar hidden off-screen while maximized (Windows).
pub fn top_inset(ppp: f32) -> f32 {
    #[cfg(windows)]
    return (native::top_inset_px() as f32 / ppp).ceil();
    #[cfg(not(windows))]
    {
        let _ = ppp;
        0.0
    }
}

/// The three buttons at the right of `row`; their rectangles. True in `.1` when Close was pressed.
pub fn buttons(ui: &mut Ui, p: &Palette, row: Rect) -> ([Rect; 3], bool) {
    let max = maximized(ui.ctx());
    let names = [
        "Minimize",
        if max { "Restore" } else { "Maximize" },
        "Close",
    ];
    let mut rects = [Rect::NOTHING; 3];
    let mut close = false;
    for (i, name) in names.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(
                row.right() - layout::CAPTION_BUTTON * (3 - i) as f32,
                row.top(),
            ),
            vec2(layout::CAPTION_BUTTON, layout::BAR_HEIGHT),
        );
        rects[i] = r;
        let resp = ui.interact(r, ui.scope_id().with(("caption", i)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, *name));
        let (hot, down) = (resp.hovered(), resp.is_pointer_button_down_on());
        #[cfg(windows)]
        let (hot, down) = if i == 1 && native::installed() {
            native::max_state()
        } else {
            (hot, down)
        };
        paint(ui, p, r, i, (hot, down), max);
        if resp.clicked() {
            match i {
                0 => minimize(ui.ctx()),
                1 => toggle_maximized(ui.ctx()),
                _ => close = true,
            }
        }
    }
    (rects, close)
}

fn paint(ui: &Ui, p: &Palette, r: Rect, i: usize, (hot, down): (bool, bool), max: bool) {
    let painter = ui.painter();
    let active = ui.input(|inp| inp.viewport().focused.unwrap_or(true));
    let mut ink = if active { p.text } else { p.muted };
    if i == 2 && (hot || down) {
        painter.rect_filled(
            r,
            CornerRadius::ZERO,
            theme::hex(if down {
                tokens::CLOSE_RED_DOWN
            } else {
                tokens::CLOSE_RED
            }),
        );
        ink = Color32::WHITE;
    } else if down {
        painter.rect_filled(r, CornerRadius::ZERO, p.selected);
    } else if hot {
        painter.rect_filled(r, CornerRadius::ZERO, p.hover);
    }
    // 10x10 glyphs, one logical pixel wide, like the system's
    let s = Stroke::new(1.0, ink);
    let b = Rect::from_center_size(r.center(), vec2(10.0, 10.0));
    match (i, max) {
        (0, _) => {
            painter.hline(b.x_range(), r.center().y, s);
        }
        (2, _) => {
            painter.line_segment([b.left_top(), b.right_bottom()], s);
            painter.line_segment([b.right_top(), b.left_bottom()], s);
        }
        (_, true) => {
            let front = Rect::from_min_max(b.min + vec2(0.0, 2.0), b.max - vec2(2.0, 0.0));
            painter.rect_stroke(front, CornerRadius::ZERO, s, StrokeKind::Middle);
            painter.line_segment([pos2(front.left() + 2.0, b.top()), b.right_top()], s);
            painter.line_segment([b.right_top(), pos2(b.right(), front.bottom() - 2.0)], s);
        }
        _ => {
            painter.rect_stroke(b, CornerRadius::ZERO, s, StrokeKind::Middle);
        }
    }
}

/// Linux (frameless): the outer 6 points resize the window through the window manager.
pub fn edge_resize(ctx: &Context) {
    use egui::viewport::ResizeDirection as D;
    if native_caption() || maximized(ctx) {
        return;
    }
    let Some(at) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let (s, b) = (ctx.content_rect(), layout::EDGE);
    let (l, r, t, bo) = (
        at.x < b,
        at.x >= s.right() - b,
        at.y < b,
        at.y >= s.bottom() - b,
    );
    let dir = match (l, r, t, bo) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (_, _, _, true) => D::South,
        _ => return,
    };
    let icon = match dir {
        D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
        D::West | D::East => egui::CursorIcon::ResizeHorizontal,
        _ => egui::CursorIcon::ResizeVertical,
    };
    ctx.set_cursor_icon(icon);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
    }
}
