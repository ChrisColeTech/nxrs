//! Drawing a menu's entries (`QMenu`): 31-point rows, a check mark column, the shortcut at the right.

use super::menu_model::Item;
use crate::constants::theme as tokens;
use crate::flows::Action;
use crate::ui::theme::{Palette, sans};
use egui::{Align2, Color32, CornerRadius, Frame, Margin, Sense, Stroke, Ui, pos2, vec2};

pub fn frame(p: &Palette) -> Frame {
    let shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(Margin::same(5))
        .corner_radius(CornerRadius::same(4))
        .shadow(shadow)
}

/// The entries; the chosen action.
pub fn items(ui: &mut Ui, items: &[Item], p: &Palette) -> Option<Action> {
    let mut chosen = None;
    // as wide as the longest entry and its shortcut (QMenu sizes itself the same way)
    let measure = |t: &str, size: f32| {
        ui.painter()
            .layout_no_wrap(t.to_string(), sans(size), p.text)
            .size()
            .x
    };
    let width = items
        .iter()
        .filter_map(|i| {
            if let Item::Command {
                label, shortcut, ..
            } = i
            {
                Some(measure(label, tokens::FONT_BODY) + measure(shortcut, tokens::FONT_TOOL))
            } else {
                None
            }
        })
        .fold(170.0_f32, f32::max)
        + 76.0;
    ui.set_width(width);
    ui.spacing_mut().item_spacing.y = 0.0;
    for item in items {
        let Item::Command {
            label,
            shortcut,
            enabled,
            checked,
            action,
        } = item
        else {
            ui.add_space(5.0);
            let r = ui.available_rect_before_wrap();
            ui.painter().hline(
                r.left()..=r.left() + width,
                r.top(),
                Stroke::new(1.0, p.border),
            );
            ui.add_space(6.0);
            continue;
        };
        let sense = if *enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 31.0), sense);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, *enabled, label));
        let painter = ui.painter();
        if *enabled && resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(4), p.selected);
        }
        let ink = if *enabled { p.text } else { p.disabled() };
        if *checked == Some(true) {
            painter.text(
                pos2(rect.left() + 10.0, rect.center().y),
                Align2::LEFT_CENTER,
                "✓",
                sans(tokens::FONT_BODY),
                ink,
            );
        }
        painter.text(
            pos2(rect.left() + 28.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            sans(tokens::FONT_BODY),
            ink,
        );
        if !shortcut.is_empty() {
            let soft = if *enabled { p.muted } else { p.disabled() };
            painter.text(
                pos2(rect.right() - 12.0, rect.center().y),
                Align2::RIGHT_CENTER,
                *shortcut,
                sans(tokens::FONT_TOOL),
                soft,
            );
        }
        if resp.clicked() {
            chosen = Some(action.clone());
            ui.close();
        }
    }
    chosen
}
