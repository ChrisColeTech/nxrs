//! THE AUTHORED CHILD WINDOW (the family's `28`): a dialog that is a real top-level window with no
//! native frame. It is an egui immediate viewport (its body borrows the app's state, and one frame loop
//! serves both windows); it opens inside the main window at the size its content wants (at most a share
//! of the main window), remembers its size and place for the run, takes the keyboard when it opens and
//! hands it back when it is shut, has no taskbar button. Closing is dismissal (its X, Escape in it, the
//! OS asking), never a commit. Where no second window can be made (tests, `--screenshot`) the same
//! content is drawn inside the main window (`child_draw::embedded`).

use super::child_bounds::{Bounds, Limits};
pub use super::child_spec::{ChildSpec, Chrome};
use super::{child_draw, child_scrim};
use crate::constants::{child_window as c, layout};
use egui::viewport::WindowLevel;
use egui::{Context, Id, Ui, Vec2, ViewportBuilder, ViewportClass, ViewportCommand, ViewportId};

/// The main window's title bar height (the scrim starts under it, except the caption cells).
pub fn note_bar(ctx: &Context, bar: f32) {
    ctx.data_mut(|d| d.insert_temp(Id::unique(c::CAPTION_STRIP), bar));
}

pub(crate) fn viewport_of(key: &str) -> ViewportId {
    ViewportId::from_hash_of((c::VIEWPORT, key))
}

/// Gives the window `key` the keyboard and brings it to the front.
pub fn focus(ctx: &Context, key: &str) {
    ctx.send_viewport_cmd_to(viewport_of(key), ViewportCommand::Focus);
}

fn opening(ctx: &Context, key: &str) -> bool {
    let at = Id::unique((c::SHOWN, key));
    let shown = ctx.data_mut(|d| d.get_temp::<bool>(at));
    ctx.data_mut(|d| d.insert_temp(at, true));
    shown != Some(true)
}

/// The caller says the window is not open this frame: its next showing is a new opening.
pub fn shut(ctx: &Context, key: &str) {
    let was = ctx.data_mut(|d| d.remove_temp::<bool>(Id::unique((c::SHOWN, key))));
    if was == Some(true) {
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
    }
}

fn opened_size(ctx: &Context, key: &str, fresh: bool, limits: &Limits) -> Vec2 {
    let at = Id::unique((c::OPENED_AT, key));
    if !fresh && let Some(kept) = ctx.data(|d| d.get_temp::<Vec2>(at)) {
        return kept;
    }
    let last = ctx.data(|d| d.get_temp::<Vec2>(Id::unique((c::SIZE, key))));
    let size = last.unwrap_or(limits.open).max(limits.min).min(limits.max);
    ctx.data_mut(|d| d.insert_temp(at, size));
    size
}

/// Shows the window this frame; `add` draws its body. True when the user dismissed it.
pub fn show(ctx: &Context, spec: &ChildSpec<'_>, add: impl FnOnce(&mut Ui)) -> bool {
    let id = viewport_of(spec.key);
    let bounds = Bounds::of(ctx);
    let limits = bounds.limits(spec);
    let fresh = opening(ctx, spec.key);
    let size = opened_size(ctx, spec.key, fresh, &limits);
    if spec.modal {
        let bar = ctx
            .data(|d| d.get_temp::<f32>(Id::unique(c::CAPTION_STRIP)))
            .unwrap_or(layout::BAR_HEIGHT);
        child_scrim::draw(ctx, spec.key, bar, layout::CAPTION_BUTTON * 3.0);
    }
    if ctx.embed_viewports() {
        return child_draw::embedded(ctx, spec, (&bounds, &limits, size), (fresh, add));
    }
    let mut builder = ViewportBuilder::default()
        .with_title(spec.title)
        .with_decorations(false)
        .with_taskbar(false)
        .with_resizable(true)
        .with_window_level(WindowLevel::Normal)
        .with_inner_size(size)
        .with_min_inner_size(limits.min)
        .with_max_inner_size(limits.max);
    if fresh {
        let kept = ctx.data(|d| d.get_temp::<egui::Pos2>(Id::unique((c::PLACE, spec.key))));
        let at = kept.map_or_else(
            || bounds.place(spec, size),
            |kept| bounds.inside(kept, size),
        );
        builder = builder.with_position(at);
    }
    let (mut add, mut closed) = (Some(add), false);
    ctx.show_viewport_immediate(id, builder, |ui, class| {
        let own = class == ViewportClass::Immediate;
        let asked = ui.input(|i| i.viewport().close_requested());
        if own && asked {
            ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
            closed = spec.chrome == Chrome::Titled;
        }
        if own {
            child_draw::note(ui, spec.key);
            if fresh {
                ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
            }
        }
        let Some(add) = add.take() else { return };
        closed |= child_draw::draw(ui, spec, (own, fresh), add);
    });
    closed
}
