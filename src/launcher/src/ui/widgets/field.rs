//! Text fields: a plain one and a path with its "…" browse button (`browseField`).

use crate::constants::popups as c;
use egui::{Response, TextEdit, Ui, vec2};

pub use crate::flows::Browse;

pub fn text(ui: &mut Ui, value: &mut String, hint: &str, name: &str) -> Response {
    let width = ui.available_width();
    let r = ui.add(
        TextEdit::singleline(value)
            .desired_width(width)
            .hint_text(hint)
            .margin(vec2(9.0, 5.0))
            .id_salt(name),
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name));
    r
}

/// A read-only line that looks like a field.
pub fn fixed(ui: &mut Ui, value: &str, name: &str) -> Response {
    let mut shown = value.to_string();
    let width = ui.available_width();
    ui.add(
        TextEdit::singleline(&mut shown)
            .desired_width(width)
            .interactive(false)
            .margin(vec2(9.0, 5.0))
            .id_salt(name),
    )
}

/// A path field and its browse button; the button opens the system's picker at the field's folder.
/// True when the button picked something.
pub fn path(
    ui: &mut Ui,
    value: &mut String,
    name: &str,
    title: &str,
    browse: Browse,
    hint: &str,
) -> bool {
    let mut picked = false;
    ui.horizontal(|ui| {
        // the field's own margins and the gap before the button are outside its desired width
        let gap = ui.spacing().item_spacing.x;
        let width = (ui.available_width() - (c::BUTTON + 2.0) - gap - 2.0 * 9.0).max(60.0);
        let edit = TextEdit::singleline(value)
            .desired_width(width)
            .hint_text(hint)
            .margin(vec2(9.0, 5.0))
            .id_salt(name);
        ui.add(edit)
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name));
        let button = ui
            .add_sized(vec2(c::BUTTON + 2.0, c::FIELD), egui::Button::new("…"))
            .on_hover_text(title);
        if button.clicked() {
            let result = crate::flows::pick(title, value, browse);
            if let Some(path) = result {
                *value = crate::model::path_text::native(&path);
                picked = true;
            }
        }
    });
    picked
}
