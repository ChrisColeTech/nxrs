//! The main window's shortcuts: Ctrl+O, Ctrl+B, Ctrl+F, Ctrl+L, Alt+Enter, Enter, F5 (Escape and
//! Delete belong to the popover, the windows and the lists).

use crate::flows::{Action, App};
use egui::{Context, Key, Modifiers};

pub fn shortcuts(ctx: &Context, app: &App) -> Vec<Action> {
    let mut out = Vec::new();
    let typing = ctx.egui_wants_keyboard_input();
    ctx.input_mut(|i| {
        let ctrl = |i: &mut egui::InputState, k: Key| i.consume_key(Modifiers::COMMAND, k);
        if ctrl(i, Key::O) {
            out.push(Action::AddGame);
        }
        if ctrl(i, Key::B) && crate::ui::chrome::menu_model::enabled(app).build {
            out.push(Action::Build);
        }
        if ctrl(i, Key::F) {
            out.push(Action::ToggleFilters);
        }
        if ctrl(i, Key::L) {
            out.push(Action::ViewConsole);
        }
        if i.consume_key(Modifiers::ALT, Key::Enter) && app.selected_game().is_some() {
            out.push(Action::Properties(0));
        }
        if i.consume_key(Modifiers::NONE, Key::F5) {
            out.push(Action::Refresh);
        }
        if !typing && !app.filters.open && i.consume_key(Modifiers::NONE, Key::Enter) {
            out.push(Action::Activate);
        }
    });
    out
}
