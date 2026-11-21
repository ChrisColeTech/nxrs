//! The one icon-only button (the family's `IconButton`): a square with a glyph on a 24-unit grid, its
//! name as tooltip and accessible name. Transparent at rest with a muted glyph, the hover fill and
//! the text ink under the pointer. Borderless, except a title header's close (`edge`), which the
//! family's style guide borders; every close in the app is this button with the same X.

use crate::constants::popups::CLOSE_SIDE as SIZE;
use crate::ui::theme;
use egui::{
    Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo, vec2,
};

const GLYPH: f32 = 14.0;

#[derive(Clone, Copy)]
pub enum Glyph {
    /// `x`
    Close,
    /// a window with an arrow out of its corner: into a window of its own
    PopOut,
    /// `panel-bottom`: docked along the bottom
    PanelBottom,
    /// `panel-right`: docked along the right
    PanelRight,
}

fn draw(ui: &Ui, rect: Rect, glyph: Glyph, ink: Color32) {
    let p = ui.painter();
    let u = GLYPH / 24.0;
    let s = Stroke::new(1.4, ink);
    let at = |x: f32, y: f32| rect.center() + vec2((x - 12.0) * u, (y - 12.0) * u);
    let frame = |p: &egui::Painter| {
        let r = Rect::from_min_max(at(3.0, 3.0), at(21.0, 21.0));
        p.rect_stroke(r, CornerRadius::same(2), s, StrokeKind::Middle);
    };
    let line = |a: Pos2, b: Pos2| p.line_segment([a, b], s);
    match glyph {
        Glyph::Close => {
            line(at(6.0, 6.0), at(18.0, 18.0));
            line(at(18.0, 6.0), at(6.0, 18.0));
        }
        Glyph::PopOut => {
            // an open box with an arrow leaving its top right corner
            line(at(11.0, 4.0), at(4.0, 4.0));
            line(at(4.0, 4.0), at(4.0, 20.0));
            line(at(4.0, 20.0), at(20.0, 20.0));
            line(at(20.0, 20.0), at(20.0, 13.0));
            line(at(11.0, 13.0), at(20.0, 4.0));
            line(at(14.0, 4.0), at(20.0, 4.0));
            line(at(20.0, 4.0), at(20.0, 10.0));
        }
        Glyph::PanelBottom => {
            frame(p);
            line(at(3.0, 15.0), at(21.0, 15.0));
        }
        Glyph::PanelRight => {
            frame(p);
            line(at(15.0, 3.0), at(15.0, 21.0));
        }
    }
}

/// The button in `rect`; true when pressed. `edge`: a title header's close.
pub fn show(ui: &Ui, rect: Rect, id: &str, (glyph, name): (Glyph, &str), edge: bool) -> bool {
    let p = theme::of(ui.ctx());
    let response = ui.interact(rect, Id::unique(("icon-button", id)), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, name));
    let hot = response.hovered();
    let fill = if response.is_pointer_button_down_on() {
        p.selected
    } else if hot {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    let border = if edge { p.border } else { Color32::TRANSPARENT };
    let radius = CornerRadius::same(4);
    ui.painter().rect(
        rect,
        radius,
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    draw(ui, rect, glyph, if hot { p.text } else { p.muted });
    response.on_hover_text(name).clicked()
}

/// The button laid out in the current row; true when pressed.
pub fn add(ui: &mut Ui, glyph: Glyph, name: &str, edge: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::hover());
    show(ui, rect, name, (glyph, name), edge)
}
