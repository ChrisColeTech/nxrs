//! The main window's sizes, in points (the Qt launcher's logical pixels).

pub const WINDOW_SIZE: [f32; 2] = [1280.0, 800.0];
pub const WINDOW_MIN: [f32; 2] = [920.0, 600.0];
pub const WINDOW_TITLE: &str = "nxrs";

/// The title bar row: its height, the window buttons' width, the app glyph.
pub const BAR_HEIGHT: f32 = 36.0;
pub const CAPTION_BUTTON: f32 = 46.0;
pub const GLYPH: f32 = 18.0;
pub const GLYPH_LEFT: f32 = 13.0;
/// The product name centred on the bar, in the family's wordmark type (giga-cut `brand.rs`: mono
/// bold 12, 1.2 letter spacing); not drawn when it would come within `WORDMARK_GAP` of a control.
pub const WORDMARK: &str = "NXRS";
pub const WORDMARK_SIZE: f32 = 12.0;
pub const WORDMARK_SPACING: f32 = 1.2;
pub const WORDMARK_GAP: f32 = 12.0;
/// The Commands strip under it (Add game, Search & Filter).
pub const COMMANDS_HEIGHT: f32 = 67.0;
pub const TOOL_BUTTON: [f32; 2] = [77.0, 59.0];
pub const TOOL_ICON: f32 = 32.0;
pub const ADD_ARROW: f32 = 15.0;
/// The status bar.
pub const STATUS_HEIGHT: f32 = 28.0;
pub const STATUS_PROGRESS: [f32; 2] = [150.0, 7.0];
pub const SELECTION_MAX: f32 = 410.0;
pub const JOB_MAX: f32 = 320.0;
/// The Linux frameless window's resize edge.
pub const EDGE: f32 = 6.0;

/// Gallery cards: width from the slider, height `width * 0.64 + 78`.
pub const COVER_SHARE: f32 = 0.64;
pub const CARD_EXTRA: f32 = 78.0;
pub const CARD_FOOTER: f32 = 42.0;
pub const CARD_BUTTON: f32 = 30.0;
pub const CARD_PROPERTIES: f32 = 32.0;
pub const CARD_GAP: f32 = 12.0;
pub const GALLERY_SPACING: f32 = 8.0;
pub const COVER_WIDTH: (i64, i64, i64) = (130, 180, 240);

/// The details list.
pub const ROW_HEIGHT: f32 = 32.0;
pub const HEADER_HEIGHT: f32 = 34.0;
pub const COLUMN_WIDTHS: [f32; 5] = [260.0, 130.0, 110.0, 80.0, 150.0];
pub const COLUMNS: [&str; 6] = [
    "Name",
    "Status",
    "Format",
    "Renderer",
    "Last played",
    "Build folder",
];

/// The build output panel.
pub const CONSOLE_DEFAULT: f32 = 190.0;
pub const CONSOLE_LINES: usize = 12000;

/// The Search & Filter popover and the engine badge's popup.
pub const FILTER_WIDTH: f32 = 362.0;
pub const TOOL_POPUP_WIDTH: f32 = 352.0;
pub const POPOVER_OVERHANG: f32 = 14.0;

/// Artwork: decoded no larger than this; the texture cache's budget.
pub const ARTWORK_MAX: u32 = 2048;
pub const ARTWORK_BUDGET: usize = 768 * 1024 * 1024;
pub const ARTWORK_UPLOADS: usize = 4;
pub const ARTWORK_WORKERS: usize = 2;
/// `--screenshot` pictures are at most this wide.
pub const SCREENSHOT_MAX: u32 = 1600;
