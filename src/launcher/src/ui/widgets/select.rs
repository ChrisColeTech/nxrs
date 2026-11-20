//! A select: a field-shaped drop-down of labelled values (Qt's `QComboBox`).

use egui::{ComboBox, Ui};

/// Shows `choices` (label, value); true when the value changed.
pub fn select<T: Clone + PartialEq>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    choices: &[(String, T)],
) -> bool {
    let shown = choices
        .iter()
        .find(|(_, v)| v == value)
        .map(|(l, _)| l.clone())
        .unwrap_or_default();
    let before = value.clone();
    let width = ui.available_width();
    let response = ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(width)
        .show_ui(ui, |ui| {
            for (label, choice) in choices {
                ui.selectable_value(value, choice.clone(), label);
            }
        });
    response
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, id));
    *value != before
}

/// Static string choices.
pub fn words<T: Clone>(choices: &[(&str, T)]) -> Vec<(String, T)> {
    choices
        .iter()
        .map(|(l, v)| (l.to_string(), v.clone()))
        .collect()
}
