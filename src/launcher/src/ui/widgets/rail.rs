//! A work window's rail (the family's `frames/rail`): its sections down the left edge, one width for
//! every rail. An entry is a leading icon and a label; the active one wears a 2px accent bar, accent
//! text and the selection tint. The host owns what a section shows; this owns only the rail.

use crate::ui::theme::{self, Palette};
use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, WidgetInfo};
use egui::{pos2, vec2};

/// The rail's width, an entry's height, the inset of its icon, the icon's size.
pub const WIDTH: f32 = 150.0;
pub const ENTRY: f32 = 32.0;
const PAD: f32 = 14.0;
const ICON: f32 = 15.0;
const GAP: f32 = 9.0;

/// What an entry's icon draws.
#[derive(Clone, Copy)]
pub enum Icon {
    Engine,
    Build,
    Controller,
    Appearance,
    Play,
}

/// A section's icon, centred on `center` (the rail's entries, the page's group tiles).
pub fn paint(ui: &Ui, center: Pos2, kind: Icon, ink: Color32) {
    let p = ui.painter();
    let h = ICON / 2.0;
    let s = Stroke::new(1.4, ink);
    let at = |x: f32, y: f32| center + vec2(x * h, y * h);
    match kind {
        Icon::Engine => {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                p.line_segment(
                    [at(a.cos() * 0.62, a.sin() * 0.62), at(a.cos(), a.sin())],
                    s,
                );
            }
            p.circle_stroke(center, h * 0.62, s);
            p.circle_stroke(center, h * 0.25, s);
        }
        Icon::Build => {
            p.line_segment([at(-0.85, 0.85), at(0.35, -0.35)], s);
            p.line_segment([at(-0.3, -0.95), at(0.95, 0.3)], s);
        }
        Icon::Controller => {
            let body = Rect::from_center_size(center, vec2(ICON, ICON * 0.64));
            p.rect_stroke(body, CornerRadius::same(4), s, StrokeKind::Middle);
            p.line_segment([at(-0.62, 0.0), at(-0.18, 0.0)], s);
            p.line_segment([at(-0.4, -0.22), at(-0.4, 0.22)], s);
            p.circle_filled(at(0.45, -0.1), 1.3, ink);
        }
        Icon::Appearance => {
            p.circle_stroke(center, h * 0.9, s);
            p.circle_filled(at(-0.3, -0.3), 1.5, ink);
            p.circle_filled(at(0.3, -0.35), 1.5, ink);
            p.circle_filled(at(0.4, 0.25), 1.5, ink);
        }
        Icon::Play => {
            let points = vec![at(-0.5, -0.8), at(0.8, 0.0), at(-0.5, 0.8)];
            p.add(egui::Shape::convex_polygon(points, Color32::TRANSPARENT, s));
        }
    }
}

fn entry(ui: &Ui, p: &Palette, rect: Rect, (label, kind): (&str, Icon), on: bool) -> bool {
    let response = ui.interact(rect, Id::unique(("rail", label)), Sense::click());
    response.widget_info(|| WidgetInfo::selected(egui::Role::Tab, true, on, label));
    let ink = if on {
        ui.painter().rect_filled(rect, 0, p.selected);
        let bar = Rect::from_min_size(rect.min, vec2(2.0, rect.height()));
        ui.painter().rect_filled(bar, 0, p.accent);
        p.accent
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0, p.hover);
        p.text
    } else {
        p.muted
    };
    let y = rect.center().y;
    let x = rect.left() + PAD;
    paint(ui, pos2(x + ICON / 2.0, y), kind, ink);
    let font = theme::sans(crate::constants::theme::FONT_BODY);
    let at = pos2(x + ICON + GAP, y);
    ui.painter()
        .text(at, egui::Align2::LEFT_CENTER, label, font, ink);
    response.clicked()
}

/// The rail down `rect`'s left edge, its hairline at the right. Returns the pressed entry.
pub fn rail(ui: &Ui, rect: Rect, entries: &[(&str, Icon)], selected: usize) -> Option<usize> {
    let p = theme::of(ui.ctx());
    ui.painter().rect_filled(rect, 0, p.raised);
    let right = rect.right() - 0.5;
    ui.painter()
        .vline(right, rect.y_range(), Stroke::new(1.0, p.border));
    let mut pressed = None;
    for (at, e) in entries.iter().enumerate() {
        let top = rect.top() + at as f32 * ENTRY;
        let row = Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width() - 1.0, ENTRY));
        if entry(ui, &p, row, *e, at == selected) {
            pressed = Some(at);
        }
    }
    pressed
}
