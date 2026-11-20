//! The colour tokens (`Theme::colorsFor`): dark and light, five accents. The chosen accent is the only
//! accent in use; the reds are semantic (close button, problem badge, error text).

/// (id, label, dark, light)
pub const ACCENTS: [(&str, &str, &str, &str); 5] = [
    ("lime", "Lime", "#b5f36b", "#38711b"),
    ("mint", "Mint", "#78e4ba", "#006b50"),
    ("violet", "Violet", "#c6adff", "#6741c0"),
    ("rose", "Rose", "#ff9cbd", "#ab2456"),
    ("cyan", "Cyan", "#77dbf2", "#006b88"),
];

/// (light, dark)
pub const CANVAS: (&str, &str) = ("#e8ebe9", "#111312");
pub const SURFACE: (&str, &str) = ("#f5f6f5", "#1c1f1d");
pub const RAISED: (&str, &str) = ("#ffffff", "#292e2b");
pub const FIELD: (&str, &str) = ("#ffffff", "#151916");
pub const TEXT: (&str, &str) = ("#202622", "#f1f5f2");
pub const MUTED: (&str, &str) = ("#505e54", "#b5c0b8");
pub const BORDER: (&str, &str) = ("#a2aea5", "#49594e");
pub const HOVER: (&str, &str) = ("#e0e7e2", "#354138");
pub const ACCENT_TEXT: (&str, &str) = ("#ffffff", "#10180b");
/// The selection tint: surface blended toward the accent by this much.
pub const SELECTED_SHARE: (f32, f32) = (0.10, 0.13);
pub const DISABLED_SHARE: f32 = 0.45;

pub const CLOSE_RED: &str = "#c42b1c";
pub const CLOSE_RED_DOWN: &str = "#b3261e";
pub const BADGE_ERROR: &str = "#e06c60";
pub const ERROR_TEXT: &str = "#ff7a7a";

/// The background canvas: the art's opacity (light, dark) and the fade's alphas (top, bottom).
pub const CANVAS_OPACITY: (f32, f32) = (0.20, 0.32);
pub const CANVAS_FADE: (u8, u8) = (15, 210);

pub const FONT_BODY: f32 = 13.0;
pub const FONT_SMALL: f32 = 11.0;
pub const FONT_TOOL: f32 = 12.0;
pub const FONT_EMPTY: f32 = 14.0;
pub const FONT_POPUP_TITLE: f32 = 17.0;
pub const FONT_MONO: f32 = 12.0;
