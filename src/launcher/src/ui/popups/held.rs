//! What the last frame ended with: a field being typed in, a menu open. A key of this frame was typed
//! into that, so a field or a menu has Escape before the window does.

use egui::{Context, Id};

const KEY: &str = "nxrs-held";

/// At the end of each frame.
pub fn note(ctx: &Context) {
    let typing = ctx.egui_wants_keyboard_input();
    let menu = ctx.any_popup_open();
    ctx.data_mut(|d| d.insert_temp(Id::unique(KEY), (typing, menu)));
}

/// (typing, a menu was open) as the last frame ended.
pub fn held(ctx: &Context) -> (bool, bool) {
    ctx.data(|d| d.get_temp::<(bool, bool)>(Id::unique(KEY)))
        .unwrap_or_default()
}
