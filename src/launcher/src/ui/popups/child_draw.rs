//! What a child window draws: its chrome and body, the size and place it remembers, and the stand-in
//! drawn inside the main window where no second window can be made.

use super::authored;
use super::child_bounds::{Bounds, Limits};
use super::child_grip;
use super::child_spec::{ChildSpec, Chrome, Escape};
use super::held::held;
use crate::constants::{child_window as c, popups as p};
use egui::{
    Context, Frame, Id, Key, Modifiers, Order, Role, Sense, Stroke, StrokeKind, Ui, Vec2,
    ViewportCommand, WidgetInfo, vec2,
};

/// A real window keeps its size and place for its next opening.
pub fn note(ui: &Ui, key: &str) {
    let (outer, inner) = ui.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
    ui.ctx().data_mut(|d| {
        if let Some(outer) = outer {
            d.insert_temp(Id::unique((c::PLACE, key)), outer.min);
        }
        if let Some(inner) = inner {
            d.insert_temp(Id::unique((c::SIZE, key)), inner.size());
        }
    });
}

/// The chrome and the body, laid out to the room the window has. True when it was dismissed.
pub fn draw(
    ui: &mut Ui,
    spec: &ChildSpec<'_>,
    (own, fresh): (bool, bool),
    add: impl FnOnce(&mut Ui),
) -> bool {
    let palette = crate::ui::theme::of(ui.ctx());
    let whole = ui.max_rect();
    ui.painter().rect_filled(whole, 0, palette.surface);
    ui.painter().rect_stroke(
        whole,
        0,
        Stroke::new(c::BORDER, palette.border),
        StrokeKind::Inside,
    );
    let node = format!("{} {}", spec.title, spec.node);
    ui.interact(whole, Id::unique((c::NODE, spec.key)), Sense::hover())
        .widget_info(|| WidgetInfo::labeled(Role::GenericContainer, true, &node));
    if own {
        child_grip::lay(ui, spec.key, whole);
    }
    let (typing, menu) = held(ui.ctx());
    let mut closed = false;
    match spec.chrome {
        Chrome::Titled => {
            Frame::NONE.inner_margin(p::PAD).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, p::GAP);
                let width = (whole.width() - p::CHROME).max(0.0);
                let head = egui::Rect::from_min_size(ui.cursor().min, vec2(width, p::TITLE_HEIGHT));
                let grab = ui.interact(head, Id::unique((c::DRAG_ID, spec.key)), Sense::drag());
                if own && grab.drag_started() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
                }
                closed = authored::header(
                    ui,
                    width,
                    spec.title,
                    &format!("{}-{}", c::CLOSE_ID, spec.key),
                );
                ui.set_width(width);
                add(ui);
            });
        }
        Chrome::Bare => add(ui),
    }
    let heard = match spec.escape {
        Escape::Never => false,
        Escape::Always => !menu,
        Escape::UnlessTyping => !menu && !typing,
    };
    closed || (heard && !fresh && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)))
}

/// The window drawn inside the main window: the same content and names, kept inside it.
pub fn embedded<F: FnOnce(&mut Ui)>(
    ctx: &Context,
    spec: &ChildSpec<'_>,
    (bounds, limits, size): (&Bounds, &Limits, Vec2),
    (fresh, add): (bool, F),
) -> bool {
    let (mut closed, mut add) = (false, Some(add));
    let shown = egui::Window::new("")
        .id(Id::unique((c::EMBEDDED, spec.key)))
        .order(Order::Foreground)
        .title_bar(false)
        .frame(Frame::NONE)
        .resizable(true)
        .collapsible(false)
        .movable(false)
        .default_size(size)
        .min_size(limits.min)
        .max_size(limits.max)
        .default_pos(bounds.place(spec, size))
        .constrain_to(bounds.room())
        .show(ctx, |ui| {
            if let Some(add) = add.take() {
                ui.set_min_size(ui.max_rect().size());
                closed = draw(ui, spec, (false, fresh), add);
            }
        });
    if let Some(shown) = shown {
        let rect = shown.response.rect;
        ctx.data_mut(|d| {
            d.insert_temp(Id::unique((c::SIZE, spec.key)), rect.size());
            d.insert_temp(Id::unique((c::PLACE, spec.key)), rect.min);
        });
    }
    closed
}
