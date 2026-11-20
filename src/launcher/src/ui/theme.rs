//! The palette built from the tokens, and egui's look built from the palette (Qt's 13 px text, raised
//! buttons with a one-pixel border and radius 5, no focus rectangles).

use crate::constants::theme as t;
use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub canvas: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub field: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub selected: Color32,
    pub border: Color32,
    pub hover: Color32,
    pub light: bool,
}

pub fn hex(s: &str) -> Color32 {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    let [_, r, g, b] = v.to_be_bytes();
    Color32::from_rgb(r, g, b)
}
pub fn blend(a: Color32, b: Color32, amount: f32) -> Color32 {
    let mix = |x: u8, y: u8| {
        (f32::from(x) * (1.0 - amount) + f32::from(y) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub fn palette(mode: &str, accent: &str) -> Palette {
    let light = mode == "light";
    let pick = |(l, d): (&str, &str)| hex(if light { l } else { d });
    let chosen = t::ACCENTS
        .iter()
        .find(|a| a.0 == accent)
        .unwrap_or(&t::ACCENTS[0]);
    let surface = pick(t::SURFACE);
    let accent = hex(if light { chosen.3 } else { chosen.2 });
    Palette {
        canvas: pick(t::CANVAS),
        surface,
        raised: pick(t::RAISED),
        field: pick(t::FIELD),
        text: pick(t::TEXT),
        muted: pick(t::MUTED),
        border: pick(t::BORDER),
        hover: pick(t::HOVER),
        accent,
        accent_text: pick(t::ACCENT_TEXT),
        selected: blend(
            surface,
            accent,
            if light {
                t::SELECTED_SHARE.0
            } else {
                t::SELECTED_SHARE.1
            },
        ),
        light,
    }
}
impl Palette {
    pub fn disabled(&self) -> Color32 {
        blend(self.muted, self.surface, t::DISABLED_SHARE)
    }
    pub fn accent_hover(&self) -> Color32 {
        blend(
            self.accent,
            if self.light {
                Color32::BLACK
            } else {
                Color32::WHITE
            },
            0.10,
        )
    }
}

/// The palette in use (kept in the context each frame by the shell).
pub fn of(ctx: &egui::Context) -> Palette {
    ctx.data(|d| d.get_temp::<Palette>(egui::Id::unique("nxrs-palette")))
        .unwrap_or_else(|| palette("dark", "lime"))
}

pub const SEMIBOLD: &str = "semibold";
pub const MONO: &str = "mono";
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}
pub fn sans(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Applies `p` to egui (once per change).
pub fn apply(ctx: &egui::Context, p: &Palette) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("nxrs-palette"), *p));
    let mut v = if p.light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    v.override_text_color = Some(p.text);
    (
        v.panel_fill,
        v.window_fill,
        v.extreme_bg_color,
        v.faint_bg_color,
    ) = (p.surface, p.surface, p.field, p.surface);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.selected;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.text_cursor.stroke = Stroke::new(1.5, p.text);
    let radius = CornerRadius::same(crate::constants::popups::RADIUS);
    let w = &mut v.widgets;
    (w.noninteractive.bg_fill, w.noninteractive.weak_bg_fill) = (p.surface, p.surface);
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    for (s, fill) in [
        (&mut w.inactive, p.raised),
        (&mut w.hovered, p.hover),
        (&mut w.active, p.selected),
        (&mut w.open, p.selected),
    ] {
        (s.bg_fill, s.weak_bg_fill, s.corner_radius, s.expansion) = (fill, fill, radius, 0.0);
        s.bg_stroke = Stroke::new(1.0, p.border);
        s.fg_stroke = Stroke::new(1.5, p.text);
    }
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.window_shadow = v.popup_shadow;
    let theme = if p.light {
        egui::Theme::Light
    } else {
        egui::Theme::Dark
    };
    ctx.set_visuals_of(theme, v);
    ctx.set_theme(theme);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, sans(t::FONT_SMALL)),
            (TextStyle::Body, sans(t::FONT_BODY)),
            (TextStyle::Button, sans(t::FONT_BODY)),
            (TextStyle::Heading, semibold(t::FONT_POPUP_TITLE)),
            (
                TextStyle::Monospace,
                FontId::new(t::FONT_MONO, FontFamily::Name(MONO.into())),
            ),
        ]
        .into();
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.spacing.interact_size.y = crate::constants::popups::FIELD;
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.interaction.tooltip_delay = 0.5;
    });
}
