//! The family's work window body (giga-cut `frames/window`): the rail down the left, the chosen
//! section's form scrolling beside it, above the window's footer. Settings and Properties use it.

use crate::ui::popups::authored;
use crate::ui::widgets::rail::{self, Icon};
use egui::{Rect, Sense, Ui, UiBuilder, pos2, vec2};

/// The rail of `entries` with `tab` chosen, and `body` drawn for the chosen one (it is given) beside it.
pub fn show(
    ui: &mut Ui,
    id: &str,
    entries: &[(&str, Icon)],
    tab: &mut usize,
    body: impl FnOnce(&mut Ui, usize),
) {
    let size = vec2(ui.available_width(), authored::body_height(ui));
    let (area, _) = ui.allocate_exact_size(size, Sense::hover());
    let rail_rect = Rect::from_min_size(area.min, vec2(rail::WIDTH, area.height()));
    if let Some(t) = rail::rail(ui, rail_rect, entries, *tab) {
        *tab = t;
    }
    let chosen = *tab;
    let page = Rect::from_min_max(pos2(rail_rect.right() + 16.0, area.top()), area.max);
    let mut pane = ui.new_child(UiBuilder::new().max_rect(page).id_salt((id, "body")));
    egui::ScrollArea::vertical()
        .id_salt((id, "page", *tab))
        .auto_shrink([false, false])
        .show(&mut pane, |ui| {
            ui.add_space(2.0);
            body(ui, chosen);
            ui.add_space(12.0);
        });
}
