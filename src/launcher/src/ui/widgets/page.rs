//! The family's page grammar (giga-cut `frames/form`): a Group is the heading (an accent-tinted icon
//! tile, a bold title, the info mark carrying its explanation) over its rows, and an accent-cored rule
//! closes it. A Row is a label in the fixed column, its info mark, and the control after it; when the
//! label does not fit beside the control it stacks above it. Explanations live in the info mark's
//! tooltip, never as hint text under a control.

use super::rail::{self, Icon};
use crate::ui::theme;
use egui::{Color32, CornerRadius, Mesh, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// The label column, the row height, the gap between rows.
pub const LABEL: f32 = 190.0;
const ROW: f32 = 28.0;
const ROW_GAP: f32 = 6.0;
const TILE: f32 = 32.0;

/// The info mark: a ringed "i" whose tooltip is the explanation.
pub fn info(ui: &mut Ui, tip: &str) {
    let (rect, response) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
    let p = theme::of(ui.ctx());
    let ink = if response.hovered() {
        p.accent
    } else {
        p.muted
    };
    let c = rect.center();
    ui.painter().circle_stroke(c, 6.5, Stroke::new(1.2, ink));
    ui.painter().circle_filled(c + vec2(0.0, -3.0), 0.9, ink);
    ui.painter().line_segment(
        [c + vec2(0.0, -0.8), c + vec2(0.0, 3.4)],
        Stroke::new(1.3, ink),
    );
    response.on_hover_text(tip);
}

/// A Group's heading; its rows follow.
pub fn group(ui: &mut Ui, icon: Icon, title: &str, tip: &str) {
    heading(ui, icon, title, tip, None);
}

/// The one body header of every dialog and Group: the icon tile, the bold title (wrapped when long),
/// the info mark when there is an explanation. `ink`: the tile's colour when it is not the accent
/// (an error's red).
pub fn heading(ui: &mut Ui, icon: Icon, title: &str, tip: &str, ink: Option<Color32>) {
    let p = theme::of(ui.ctx());
    // the tile's fill and edge follow its ink: an error's tile is red throughout, never red in green
    let (ink, fill, edge) = match ink {
        Some(c) => (c, c.gamma_multiply(0.16), c.gamma_multiply(0.45)),
        None => (p.accent, p.selected, p.border),
    };
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let (tile, _) = ui.allocate_exact_size(vec2(TILE, TILE), Sense::hover());
        let radius = CornerRadius::same(8);
        let edge = Stroke::new(1.0, edge);
        ui.painter()
            .rect(tile, radius, fill, edge, StrokeKind::Inside);
        rail::paint(ui, tile.center(), icon, ink);
        ui.add_space(4.0);
        let text = egui::RichText::new(title)
            .font(theme::semibold(16.0))
            .color(p.text);
        ui.add(egui::Label::new(text).wrap());
        if !tip.is_empty() {
            info(ui, tip);
        }
    });
    ui.add_space(10.0);
}

/// One labelled row; `add` draws the control in the rest of the width.
pub fn row(ui: &mut Ui, label: &str, tip: &str, add: impl FnOnce(&mut Ui)) {
    let p = theme::of(ui.ctx());
    let font = theme::sans(crate::constants::theme::FONT_BODY);
    let width = ui.fonts_mut(|f| {
        f.layout_no_wrap(label.into(), font.clone(), p.text)
            .size()
            .x
    });
    let stacked = width + 24.0 > LABEL;
    let label_part = |ui: &mut Ui| {
        ui.label(egui::RichText::new(label).font(font.clone()).color(p.text));
        if !tip.is_empty() {
            info(ui, tip);
        }
    };
    if stacked {
        ui.horizontal(|ui| label_part(ui));
        ui.scope(add);
    } else {
        ui.horizontal(|ui| {
            ui.set_min_height(ROW);
            let (cell, _) = ui.allocate_exact_size(vec2(LABEL, ROW), Sense::hover());
            let mut label_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(cell)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            label_part(&mut label_ui);
            add(ui);
        });
    }
    ui.add_space(ROW_GAP);
}

/// The rule between Groups: a hairline, accent-cored.
pub fn rule(ui: &mut Ui) {
    let p = theme::of(ui.ctx());
    ui.add_space(10.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    let stops = [
        (0.0, Color32::TRANSPARENT),
        (0.15, p.border),
        (0.5, p.accent.gamma_multiply(0.7)),
        (0.85, p.border),
        (1.0, Color32::TRANSPARENT),
    ];
    let mut mesh = Mesh::default();
    for (at, ink) in stops {
        let x = rect.left() + rect.width() * at;
        mesh.colored_vertex(pos2(x, rect.top()), ink);
        mesh.colored_vertex(pos2(x, rect.bottom()), ink);
    }
    for column in 0..stops.len() as u32 - 1 {
        let first = column * 2;
        mesh.add_triangle(first, first + 1, first + 2);
        mesh.add_triangle(first + 1, first + 2, first + 3);
    }
    ui.painter().add(mesh);
}

/// A read-only block in a row (the engine's health, its checks): selectable text on the field ground.
pub fn readout(ui: &mut Ui, text: &str) {
    let p = theme::of(ui.ctx());
    egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(egui::RichText::new(text).color(p.text))
                    .wrap()
                    .selectable(true),
            );
        });
}
