//! The product name at the centre of the title bar, in the family's wordmark type (giga-cut's brand
//! lockup: mono bold, letter-spaced, the text colour). Centred on the window's full width; never
//! shifted, clipped or drawn under a control: where it would come near the menu titles or the window
//! buttons it is not drawn. It stays part of the bar's drag area.

use crate::constants::layout as c;
use crate::ui::theme::{self, Palette};
use egui::text::{LayoutJob, TextFormat};
use egui::{Galley, Rect, Role, Sense, Ui, Vec2, WidgetInfo};

/// Where the name goes: centred on `row` (the bar's full width), or None when it would come within
/// the gap of `left` (the menu titles' right edge) or `right` (the window buttons' left edge).
pub fn place(row: Rect, size: Vec2, left: f32, right: f32) -> Option<Rect> {
    let at = Rect::from_center_size(row.center(), size);
    (at.left() >= left + c::WORDMARK_GAP && at.right() <= right - c::WORDMARK_GAP).then_some(at)
}

/// Paints the name in `row` between `left` and `right`; its rectangle when drawn.
pub fn show(ui: &mut Ui, p: &Palette, row: Rect, left: f32, right: f32) -> Option<Rect> {
    let mut job = LayoutJob::default();
    job.append(
        c::WORDMARK,
        0.0,
        TextFormat {
            font_id: theme::mono_bold(c::WORDMARK_SIZE),
            color: p.text,
            extra_letter_spacing: c::WORDMARK_SPACING,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    // centred by its ink (the capitals), not its line box: a mono face's descender room and the
    // trailing letter space would sit it high and left of the bar's centre
    let ink = ink(&galley);
    let at = place(row, ink.size(), left, right)?;
    ui.painter()
        .galley(at.min - ink.min.to_vec2(), galley, p.text);
    // named for the accessibility tree; hover only, so the bar under it still drags the window
    let resp = ui.interact(at, ui.scope_id().with("wordmark"), Sense::hover());
    resp.widget_info(|| WidgetInfo::labeled(Role::Label, true, c::WORDMARK));
    Some(at)
}

/// The painted glyphs' bounds within the galley (the whole galley when it has none).
fn ink(galley: &Galley) -> Rect {
    let mut r = Rect::NOTHING;
    for placed in &galley.rows {
        for g in &placed.row.glyphs {
            let min = placed.pos + g.pos.to_vec2() + g.uv_rect.offset;
            r = r.union(Rect::from_min_size(min, g.uv_rect.size));
        }
    }
    if r.is_positive() { r } else { galley.rect }
}
