//! The build output panel (`createBuildDock`): its title following the build, Open log and Clear view,
//! the lines (at most 12000, only the visible ones drawn, following the end unless scrolled up),
//! docked at the bottom or the right (`dock` owns its geometry), popped out, closable.

use crate::constants::{menus as m, words_build as w};
use crate::flows::{Action, App};
use crate::ui::theme::{self, semibold};
use egui::{CornerRadius, Stroke, Ui};

pub fn show(ui: &mut Ui, app: &mut App) -> Option<Action> {
    let p = theme::of(ui.ctx());
    let mut action = None;
    let full = ui.max_rect();
    ui.painter().rect_filled(full, CornerRadius::ZERO, p.field);
    // the header band, its rule and the lines abut
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::Frame::new()
        .fill(p.raised)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&app.console.title).font(semibold(13.0)));
                ui.add_space(12.0);
                if ui
                    .add(egui::Button::new(m::OPEN_LOG).frame(false))
                    .clicked()
                {
                    action = Some(Action::OpenBuildLog);
                }
                if ui
                    .add(egui::Button::new(m::CLEAR_VIEW).frame(false))
                    .clicked()
                {
                    action = Some(Action::ClearConsole);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let c = &mut app.lib.console;
                    if !c.floating
                        && ui
                            .add(egui::Button::new("×").frame(false))
                            .on_hover_text("Close")
                            .clicked()
                    {
                        action = Some(Action::ToggleConsole);
                    }
                    let float = if c.floating { "Dock" } else { "Pop out" };
                    if ui.add(egui::Button::new(float).frame(false)).clicked() {
                        c.floating = !c.floating;
                    }
                    if !c.floating {
                        let side = if c.right {
                            "Dock at bottom"
                        } else {
                            "Dock at right"
                        };
                        if ui.add(egui::Button::new(side).frame(false)).clicked() {
                            super::dock::switch_side(c);
                        }
                    }
                });
            });
        });
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.border));
    if app.console.lines.is_empty() {
        egui::Frame::new()
            .inner_margin(egui::Margin::same(7))
            .show(ui, |ui| {
                let text = egui::RichText::new(w::PLACEHOLDER)
                    .color(p.muted)
                    .monospace();
                ui.add(egui::Label::new(text).wrap());
            });
        return action;
    }
    let row = ui.text_style_height(&egui::TextStyle::Monospace);
    let lines = &app.console.lines;
    egui::ScrollArea::both()
        .id_salt("console")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show_rows(ui, row, lines.len(), |ui, range| {
            for line in lines.range(range) {
                ui.add(
                    egui::Label::new(egui::RichText::new(line).monospace().color(p.text)).extend(),
                );
            }
        });
    action
}
