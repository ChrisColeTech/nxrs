//! Where a child window may be and how large: inside the main window and on the screen. It opens at
//! the size its content wants but no more than a share of the main window's, can be made as large as
//! the main window and no smaller than its least, and is put wholly inside the main window.

use super::child_spec::{ChildSpec, Place};
use crate::constants::child_window as c;
use egui::{Context, Pos2, Rect, Vec2, pos2, vec2};

pub struct Bounds {
    pub outer: Rect,
    pub inner: Vec2,
    pub monitor: Option<Vec2>,
}

pub struct Limits {
    pub open: Vec2,
    pub min: Vec2,
    pub max: Vec2,
}

impl Bounds {
    pub fn of(ctx: &Context) -> Self {
        let screen = ctx.content_rect();
        let (outer, inner, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect.map(|r| r.size()), v.monitor_size)
        });
        Self {
            outer: outer.unwrap_or(screen),
            inner: inner.unwrap_or_else(|| screen.size()),
            monitor,
        }
    }

    /// The main window's rectangle, cut to the monitor.
    pub fn room(&self) -> Rect {
        let screen = self
            .monitor
            .map(|size| Rect::from_min_size(Pos2::ZERO, size));
        let shown = screen.map(|s| self.outer.intersect(s));
        shown.filter(|r| r.is_positive()).unwrap_or(self.outer)
    }

    pub fn place(&self, spec: &ChildSpec<'_>, size: Vec2) -> Pos2 {
        let room = self.room();
        let at = match spec.place {
            Place::Corner => room.min + vec2(c::OFFSET.0, c::OFFSET.1),
            Place::Centre => room.center() - size / 2.0,
        };
        self.inside(at, size)
    }

    /// `at`, moved the least that puts a window of `size` wholly inside the room.
    pub fn inside(&self, at: Pos2, size: Vec2) -> Pos2 {
        let room = self.room();
        let low = room.min;
        let high = pos2(
            (room.max.x - size.x).max(low.x),
            (room.max.y - size.y).max(low.y),
        );
        pos2(at.x.clamp(low.x, high.x), at.y.clamp(low.y, high.y)).round()
    }

    pub fn limits(&self, spec: &ChildSpec<'_>) -> Limits {
        let max = self.inner;
        let min = spec.min_size().min(max);
        let monitor = self.monitor.map_or(max, |size| {
            size - vec2(c::SCREEN_MARGIN.0, c::SCREEN_MARGIN.1)
        });
        let open = spec
            .size()
            .min(self.inner * c::OPEN_SHARE)
            .min(monitor)
            .max(min)
            .min(max);
        Limits { open, min, max }
    }
}
