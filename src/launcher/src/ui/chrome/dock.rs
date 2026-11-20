//! The splitter boundary (the family's `layout` + `workspace`): the sole owner of the library and the
//! docked build output's geometry. The saved size is a preference in points; the panes abut (a 1px
//! hairline is the build output's own border) and the sash is a 5px strip laid over the seam. A
//! toggle commits at once; only the paint travels.

use crate::constants::panes::{self as p, Limits};
use crate::model::library::ConsolePanel;
use crate::ui::theme;
use crate::ui::widgets::sash::{Axis, sash};
use egui::{Id, Rect, Stroke, Ui, UiBuilder, pos2};

pub struct Split {
    pub library: Rect,
    pub console: Rect,
    pub sash: Rect,
}

pub fn limits(right: bool) -> Limits {
    if right {
        p::CONSOLE_RIGHT
    } else {
        p::CONSOLE_BOTTOM
    }
}

/// What the build output paints at: its saved size clamped to the table, scaled by the fold tween
/// (`shown` runs 0..1).
pub fn painted(c: &ConsolePanel, shown: f32) -> f32 {
    let l = limits(c.right);
    c.size.clamp(l.min, l.max) * shown
}

/// [library | build output] across, or [library / build output] down. A small window gives the
/// library its floor first and refits the painted size, never the saved one.
pub fn split(area: Rect, size: f32, right: bool) -> Split {
    let floor = if right {
        p::LIBRARY_MIN_WIDTH
    } else {
        p::LIBRARY_MIN_HEIGHT
    };
    let room = if right { area.width() } else { area.height() };
    let size = size.min((room - floor).max(0.0)).max(0.0);
    let half = if size > 0.0 { p::SASH / 2.0 } else { 0.0 };
    if right {
        let x = area.right() - size;
        Split {
            library: Rect::from_min_max(area.min, pos2(x, area.bottom())),
            console: Rect::from_min_max(pos2(x, area.top()), area.max),
            sash: Rect::from_min_max(pos2(x - half, area.top()), pos2(x + half, area.bottom())),
        }
    } else {
        let y = area.bottom() - size;
        Split {
            library: Rect::from_min_max(area.min, pos2(area.right(), y)),
            console: Rect::from_min_max(pos2(area.left(), y), area.max),
            sash: Rect::from_min_max(pos2(area.left(), y - half), pos2(area.right(), y + half)),
        }
    }
}

/// The fold tween's position: 0 shut .. 1 open.
pub fn shown(ui: &Ui, c: &ConsolePanel) -> f32 {
    let open = c.visible && !c.floating;
    let id = Id::unique(("console-shown", c.right));
    ui.ctx()
        .animate_bool_with_time(id, open, p::COLLAPSE_SECONDS)
}

/// A child Ui clipped to `rect`.
pub fn child(ui: &mut Ui, rect: Rect, id: &str) -> Ui {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(id));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child
}

/// The build output's border (the open pane owns it) and the sash; a drag moves the saved size 1:1
/// within the table, a double click gives the first size back.
pub fn sash_and_rule(ui: &Ui, c: &mut ConsolePanel, s: &Split) {
    let p = theme::of(ui.ctx());
    let stroke = Stroke::new(1.0, p.border);
    if c.right {
        ui.painter()
            .vline(s.console.left(), s.console.y_range(), stroke);
    } else {
        ui.painter()
            .hline(s.console.x_range(), s.console.top(), stroke);
    }
    let l = limits(c.right);
    let axis = if c.right { Axis::Column } else { Axis::Row };
    let moved = sash(ui, s.sash, "sash-console", axis, (p::SASH_CONSOLE, c.size));
    // the seam moves with the pointer: toward the library grows the build output
    c.size = (c.size.clamp(l.min, l.max) - moved.delta).clamp(l.min, l.max);
    if moved.reset {
        c.size = l.preferred;
    }
}

/// Docking at the other side starts at that side's first size (one saved size, two axes).
pub fn switch_side(c: &mut ConsolePanel) {
    c.right = !c.right;
    c.size = limits(c.right).preferred;
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn the_library_keeps_its_floor_and_the_saved_size_survives() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let s = split(area, 190.0, false);
        assert_eq!((s.library.height(), s.console.height()), (410.0, 190.0));
        assert_eq!(s.sash.height(), p::SASH);
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 300.0));
        let s = split(small, 400.0, false);
        assert_eq!(s.library.height(), p::LIBRARY_MIN_HEIGHT);
        let s = split(area, 440.0, true);
        assert_eq!((s.library.width(), s.console.width()), (560.0, 440.0));
        let shut = split(area, 0.0, false);
        assert_eq!((shut.library, shut.sash.height()), (area, 0.0));
        let c = ConsolePanel {
            size: 5000.0,
            ..ConsolePanel::default()
        };
        assert_eq!(painted(&c, 1.0), p::CONSOLE_BOTTOM.max);
        assert_eq!(painted(&c, 0.5), p::CONSOLE_BOTTOM.max / 2.0);
        let mut c = ConsolePanel {
            size: 300.0,
            ..ConsolePanel::default()
        };
        switch_side(&mut c);
        assert!(c.right && c.size == p::CONSOLE_RIGHT.preferred);
    }
}
