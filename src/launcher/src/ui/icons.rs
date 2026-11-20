//! The painted icons as vector shapes (crisp at any scale): the command icons (`commandIcon`), the card
//! buttons' (`localIcon`), the app glyph (`accentGlyph`), the cover placeholder, a chevron.

use super::theme::Palette;
use egui::epaint::{PathShape, PathStroke};
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};

/// A `units`-wide design grid laid into a rectangle.
struct Grid {
    origin: Pos2,
    scale: f32,
}
impl Grid {
    fn new(rect: Rect, units: f32) -> Grid {
        let side = rect.width().min(rect.height());
        Grid {
            origin: rect.center() - vec2(side, side) / 2.0,
            scale: side / units,
        }
    }
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x, y) * self.scale
    }
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.p(x, y), vec2(w, h) * self.scale)
    }
    fn s(&self, width: f32, color: Color32) -> Stroke {
        Stroke::new(width * self.scale, color)
    }
    fn round(&self, units: f32) -> CornerRadius {
        CornerRadius::same((units * self.scale).round().clamp(0.0, 255.0) as u8)
    }
    fn line(&self, p: &Painter, a: (f32, f32), b: (f32, f32), s: Stroke) {
        p.line_segment([self.p(a.0, a.1), self.p(b.0, b.1)], s);
    }
}

fn folder(p: &Painter, g: &Grid, c: &Palette) {
    let pts = [
        (3.0, 10.0),
        (3.0, 7.0),
        (5.0, 5.0),
        (12.0, 5.0),
        (15.0, 8.0),
        (26.0, 8.0),
        (28.0, 10.0),
        (28.0, 25.0),
        (3.0, 25.0),
    ];
    let points = pts.iter().map(|(x, y)| g.p(*x, *y)).collect();
    p.add(Shape::Path(PathShape {
        points,
        closed: true,
        fill: c.raised,
        stroke: PathStroke::new(1.4 * g.scale, c.text),
    }));
}

/// The Commands strip's icons on a 32-unit grid: add, folder, import, filter, settings.
pub fn command(p: &Painter, rect: Rect, kind: &str, c: &Palette) {
    let g = Grid::new(rect, 32.0);
    let (text, accent) = (g.s(1.65, c.text), g.s(1.8, c.accent));
    match kind {
        "add" | "folder" => {
            folder(p, &g, c);
            g.line(p, (11.0, 17.0), (21.0, 17.0), accent);
            if kind == "add" {
                g.line(p, (16.0, 12.0), (16.0, 22.0), accent);
            } else {
                g.line(p, (18.0, 14.0), (21.0, 17.0), accent);
                g.line(p, (18.0, 20.0), (21.0, 17.0), accent);
            }
        }
        "import" => {
            p.rect_stroke(
                g.r(4.0, 5.0, 24.0, 22.0),
                g.round(3.0),
                text,
                StrokeKind::Middle,
            );
            g.line(p, (4.0, 11.0), (28.0, 11.0), text);
            for (a, b) in [
                ((16.0, 14.0), (16.0, 24.0)),
                ((12.0, 20.0), (16.0, 24.0)),
                ((20.0, 20.0), (16.0, 24.0)),
            ] {
                g.line(p, a, b, accent);
            }
        }
        "filter" => {
            p.circle_stroke(g.p(12.0, 11.0), 8.0 * g.scale, text);
            g.line(p, (18.0, 17.0), (27.0, 26.0), text);
            g.line(p, (3.0, 24.0), (13.0, 24.0), g.s(1.5, c.accent));
            g.line(p, (6.0, 28.0), (13.0, 28.0), g.s(1.5, c.accent));
        }
        "settings" => {
            let points = (0..32)
                .map(|i| {
                    let angle =
                        i as f32 * std::f32::consts::TAU / 32.0 - std::f32::consts::FRAC_PI_2;
                    let radius = if i % 4 == 1 || i % 4 == 2 { 13.0 } else { 10.0 };
                    g.p(16.0 + angle.cos() * radius, 16.0 + angle.sin() * radius)
                })
                .collect();
            p.add(Shape::Path(PathShape {
                points,
                closed: true,
                fill: c.raised,
                stroke: PathStroke::new(1.5 * g.scale, c.text),
            }));
            p.circle_stroke(g.p(16.0, 16.0), 5.0 * g.scale, g.s(1.5, c.text));
        }
        _ => {}
    }
}

/// The card buttons' icons on a 24-unit grid: Play, Build, Rebuild, Stop, Cancel build, properties.
pub fn card(p: &Painter, rect: Rect, action: &str, ink: Color32, c: &Palette) {
    let g = Grid::new(rect, 24.0);
    let s = g.s(1.6, ink);
    match action {
        "Play" => {
            p.add(Shape::convex_polygon(
                vec![g.p(8.0, 5.0), g.p(19.0, 12.0), g.p(8.0, 19.0)],
                if ink == c.text { c.accent } else { ink },
                Stroke::NONE,
            ));
        }
        "Build" | "Rebuild" => {
            g.line(p, (6.0, 19.0), (17.0, 7.0), s);
            g.line(p, (11.0, 4.0), (21.0, 13.0), s);
            g.line(p, (17.0, 3.0), (22.0, 8.0), s);
        }
        "Stop" => {
            p.rect_filled(g.r(6.0, 6.0, 12.0, 12.0), g.round(2.0), ink);
        }
        "Cancel build" => {
            g.line(p, (6.0, 6.0), (18.0, 18.0), s);
            g.line(p, (18.0, 6.0), (6.0, 18.0), s);
        }
        _ => {
            g.line(p, (4.0, 7.0), (20.0, 7.0), s);
            g.line(p, (4.0, 17.0), (20.0, 17.0), s);
            p.circle(g.p(9.0, 7.0), 3.0 * g.scale, c.raised, s);
            p.circle(g.p(15.0, 17.0), 3.0 * g.scale, c.raised, s);
        }
    }
}

/// The app glyph in the accent: a rounded square with the controller cut out to `behind`.
pub fn accent_glyph(p: &Painter, rect: Rect, accent: Color32, behind: Color32) {
    let s = rect.width();
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * s;
    let rr = |x1: f32, y1: f32, x2: f32, y2: f32, r: f32, color: Color32| {
        let radius = CornerRadius::same((r * s).round().clamp(0.0, 255.0) as u8);
        p.rect_filled(Rect::from_min_max(at(x1, y1), at(x2, y2)), radius, color);
    };
    rr(0.0, 0.0, 1.0, 1.0, 0.22, accent);
    rr(0.11, 0.30, 0.89, 0.64, 0.17, behind);
    for cx in [0.255, 0.745] {
        p.circle_filled(at(cx, 0.54), 0.16 * s, behind);
    }
    let (cx, cy, arm, w) = (0.295, 0.465, 0.105, 0.044);
    rr(cx - arm, cy - w, cx + arm, cy + w, w * 0.5, accent);
    rr(cx - w, cy - arm, cx + w, cy + arm, w * 0.5, accent);
    for (bx, by) in [(0.665, 0.51), (0.75, 0.425)] {
        p.circle_filled(at(bx, by), 0.052 * s, accent);
    }
}

/// The cover placeholder's gamepad outline.
pub fn gamepad(p: &Painter, center: Pos2, c: &Palette) {
    let s = Stroke::new(1.6, c.muted);
    p.rect_stroke(
        Rect::from_center_size(center, vec2(50.0, 32.0)),
        CornerRadius::same(10),
        s,
        StrokeKind::Middle,
    );
    p.line_segment([center + vec2(-17.0, 0.0), center + vec2(-5.0, 0.0)], s);
    p.line_segment([center + vec2(-11.0, -6.0), center + vec2(-11.0, 6.0)], s);
    p.circle_stroke(center + vec2(11.0, -4.0), 2.0, s);
    p.circle_stroke(center + vec2(17.0, 3.0), 2.0, s);
}

pub fn chevron(p: &Painter, center: Pos2, color: Color32) {
    let s = Stroke::new(1.4, color);
    p.line_segment([center + vec2(-4.0, -2.0), center + vec2(0.0, 2.0)], s);
    p.line_segment([center + vec2(0.0, 2.0), center + vec2(4.0, -2.0)], s);
}

/// The header close button's X.
pub fn close_x(p: &Painter, rect: Rect, ink: Color32) {
    let b = Rect::from_center_size(rect.center(), vec2(9.0, 9.0));
    let s = Stroke::new(1.3, ink);
    p.line_segment([b.left_top(), b.right_bottom()], s);
    p.line_segment([b.right_top(), b.left_bottom()], s);
}
