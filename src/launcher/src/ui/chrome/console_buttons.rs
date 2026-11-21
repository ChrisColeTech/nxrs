//! The build output's header buttons: icon buttons (the family's panel glyphs on a 24-unit grid,
//! 2-unit strokes, rounded ends) with their names as tooltips and accessible names.

use crate::ui::theme;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo, vec2};

const SIZE: f32 = 26.0;
const GLYPH: f32 = 15.0;

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

/// One icon button; true when pressed.
pub fn icon(ui: &mut Ui, glyph: Glyph, name: &str) -> bool {
    let p = theme::of(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, name));
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.hover);
    }
    let ink = if response.hovered() { p.text } else { p.muted };
    draw(ui, rect, glyph, ink);
    response.on_hover_text(name).clicked()
}
