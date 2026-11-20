//! The pane table (the family's `constants/panes`): the build output's least, greatest and first size
//! on each side it docks at, the library's floor against it, and the sash. `ui::chrome::dock` is the
//! only consumer.

#[derive(Clone, Copy)]
pub struct Limits {
    pub min: f32,
    pub max: f32,
    pub preferred: f32,
}

const fn limits(min: f32, max: f32, preferred: f32) -> Limits {
    Limits {
        min,
        max,
        preferred,
    }
}

/// Docked at the bottom: a height. 190 is the Qt dock's (`showBuildOutput`).
pub const CONSOLE_BOTTOM: Limits = limits(90.0, 560.0, 190.0);
/// Docked at the right: a width.
pub const CONSOLE_RIGHT: Limits = limits(280.0, 760.0, 440.0);

/// The library keeps at least this much against the build output; a small window refits the painted
/// size and never the saved one.
pub const LIBRARY_MIN_HEIGHT: f32 = 160.0;
pub const LIBRARY_MIN_WIDTH: f32 = 320.0;

/// A sash's hit strip, centred on the seam; the hairline in it is 1px (2px while dragging).
pub const SASH: f32 = 5.0;
/// The sash's accessible name; its value is the build output's saved size, in points.
pub const SASH_CONSOLE: &str = "Resize build output";

/// How long the build output takes to fold away or back.
pub const COLLAPSE_SECONDS: f32 = 0.10;
