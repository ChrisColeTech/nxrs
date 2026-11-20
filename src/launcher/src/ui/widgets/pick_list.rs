//! A list of rows with click, Ctrl and Shift selection, a context menu per row and Delete (the DLC and
//! update lists).

use crate::flows::surface::Picks;
use crate::ui::theme::Palette;
use egui::{CornerRadius, Sense, Stroke, Ui, vec2};

pub struct ListOut {
    /// (row, menu entry) chosen from a row's context menu
    pub menu: Option<(usize, usize)>,
    /// Delete pressed while the list had the keyboard
    pub delete: bool,
}

fn click(picks: &mut Picks, i: usize, m: egui::Modifiers) {
    match (m.shift, picks.anchor, m.command || m.ctrl) {
        (true, Some(a), _) => picks.rows = (a.min(i)..=a.max(i)).collect(),
        (_, _, true) => {
            if !picks.rows.remove(&i) {
                picks.rows.insert(i);
            }
            picks.anchor = Some(i);
        }
        _ => {
            picks.rows = [i].into();
            picks.anchor = Some(i);
        }
    }
}

/// `rows`: (text, tooltip). `menu(row, selected count)`: the entries (label, enabled) of its menu.
pub fn show(
    ui: &mut Ui,
    p: &Palette,
    name: &str,
    rows: &[(String, String)],
    picks: &mut Picks,
    height: f32,
    menu: &dyn Fn(usize, usize) -> Vec<(String, bool)>,
) -> ListOut {
    let mut out = ListOut {
        menu: None,
        delete: false,
    };
    let list_id = ui.scope_id().with(name);
    picks.rows.retain(|i| *i < rows.len());
    let frame = egui::Frame::new()
        .fill(p.field)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::same(2));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_height(height.max(40.0));
        egui::ScrollArea::vertical()
            .id_salt(name)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, (text, tip)) in rows.iter().enumerate() {
                    let galley = ui.painter().layout(
                        text.clone(),
                        egui::FontId::proportional(13.0),
                        p.text,
                        ui.available_width() - 12.0,
                    );
                    let (r, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), galley.size().y + 12.0),
                        Sense::click(),
                    );
                    let chosen = picks.rows.contains(&i);
                    let fill = if chosen {
                        Some(p.selected)
                    } else if resp.hovered() {
                        Some(p.hover)
                    } else {
                        None
                    };
                    if let Some(f) = fill {
                        ui.painter().rect_filled(r, CornerRadius::same(3), f);
                    }
                    ui.painter().galley(r.min + vec2(6.0, 6.0), galley, p.text);
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::ListItem, true, chosen, text)
                    });
                    let resp = resp.on_hover_text(tip);
                    if resp.clicked() {
                        click(picks, i, ui.input(|inp| inp.modifiers));
                        ui.memory_mut(|m| m.request_focus(list_id));
                    }
                    if resp.secondary_clicked() && !chosen {
                        (picks.rows, picks.anchor) = ([i].into(), Some(i));
                    }
                    let entries = menu(i, picks.rows.len().max(1));
                    resp.context_menu(|ui| {
                        for (k, (label, enabled)) in entries.iter().enumerate() {
                            if ui
                                .add_enabled(
                                    *enabled,
                                    egui::Button::new(label.as_str()).frame(false),
                                )
                                .clicked()
                            {
                                out.menu = Some((i, k));
                                ui.close();
                            }
                        }
                    });
                }
            });
    });
    let has_keys = ui.memory(|m| m.has_focus(list_id) || m.focused().is_none());
    out.delete = has_keys && ui.input(|i| i.key_pressed(egui::Key::Delete));
    out
}
