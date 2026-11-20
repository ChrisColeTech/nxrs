//! The title bar row (`TitleBar`): the app glyph in the accent, File / Game / Keys / View / Help in the
//! caption row (VS Code style), the window buttons; no title text. Windows hit-tests it natively (drag,
//! double-click, snap, Snap Layouts on the maximize button); elsewhere egui drags the window.

use super::{caption, menu_draw, menu_model};
use crate::constants::{layout, menus};
use crate::flows::{Action, App};
use crate::ui::icons;
use crate::ui::theme::{self, sans};
use egui::{CornerRadius, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

#[derive(Default)]
pub struct BarState {
    pub open: Option<usize>,
    pub titles: [Option<Rect>; 5],
}

/// Pops a menu open by its name (`--show-menu`).
pub fn open_menu(bar: &mut BarState, name: &str) {
    bar.open = menus::MENUS
        .iter()
        .position(|m| m.eq_ignore_ascii_case(name));
}

pub fn show(ui: &mut Ui, app: &App, bar: &mut BarState) -> Option<Action> {
    let p = theme::of(ui.ctx());
    let ppp = ui.ctx().pixels_per_point();
    let inset = caption::top_inset(ppp);
    let rect = Rect::from_min_size(
        ui.max_rect().min,
        vec2(ui.max_rect().width(), layout::BAR_HEIGHT + inset),
    );
    ui.allocate_rect(rect, Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, p.surface);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, p.border),
    );
    let top = rect.top() + inset;
    let glyph = Rect::from_min_size(
        pos2(
            rect.left() + layout::GLYPH_LEFT,
            top + (layout::BAR_HEIGHT - layout::GLYPH) / 2.0,
        ),
        vec2(layout::GLYPH, layout::GLYPH),
    );
    icons::accent_glyph(ui.painter(), glyph, p.accent, p.surface);
    let mut x = glyph.right() + 7.0;
    for (i, name) in menus::MENUS.iter().enumerate() {
        let galley = ui
            .painter()
            .layout_no_wrap(name.to_string(), sans(13.0), p.text);
        let r = Rect::from_min_size(
            pos2(x, top + 5.0),
            vec2(galley.size().x + 20.0, layout::BAR_HEIGHT - 10.0),
        );
        let resp = ui.interact(r, Id::unique(("menu-title", i)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, *name));
        let open = bar.open == Some(i);
        if open || resp.hovered() {
            ui.painter()
                .rect_filled(r, CornerRadius::same(5), p.selected);
        }
        ui.painter().galley(
            pos2(r.left() + 10.0, r.center().y - galley.size().y / 2.0),
            galley,
            p.text,
        );
        if resp.clicked() {
            bar.open = if open { None } else { Some(i) };
        } else if resp.hovered() && bar.open.is_some() && !open {
            bar.open = Some(i); // moving along the bar with a menu open switches menus
        }
        bar.titles[i] = Some(r);
        x = r.right();
    }
    let row = Rect::from_min_max(pos2(rect.left(), top), rect.max);
    let (buttons, close) = caption::buttons(ui, &p, row);
    let mut chosen = close.then_some(Action::Exit);
    // the empty row (and the glyph) drag the window; double-click maximizes (natively on Windows)
    for area in [
        Rect::from_min_max(pos2(x, rect.top()), pos2(buttons[0].left(), rect.bottom())),
        Rect::from_min_max(rect.min, pos2(glyph.right() + 7.0, rect.bottom())),
    ] {
        let resp = ui.interact(
            area,
            Id::unique(("caption-area", area.left() as i32)),
            Sense::click_and_drag(),
        );
        if !caption::native_caption() {
            if resp.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if resp.double_clicked() {
                caption::toggle_maximized(ui.ctx());
            }
        }
    }
    publish(ppp, rect, &buttons, bar);
    if let Some(a) = dropdown(ui, app, bar) {
        chosen = Some(a);
    }
    chosen
}

/// Windows: where the buttons and the menu titles are, for the native hit test.
fn publish(ppp: f32, bar_rect: Rect, buttons: &[Rect; 3], bar: &BarState) {
    #[cfg(windows)]
    {
        use crate::services::platform::windows_caption::{PxRect, Regions, publish};
        let px = |r: Rect| PxRect {
            x0: (r.left() * ppp) as i32,
            y0: (r.top() * ppp) as i32,
            x1: (r.right() * ppp).ceil() as i32,
            y1: (r.bottom() * ppp).ceil() as i32,
        };
        let mut client: Vec<PxRect> = bar.titles.iter().flatten().map(|r| px(*r)).collect();
        if bar.open.is_some() {
            client.push(px(bar_rect)); // an open menu: the whole bar takes clicks
        }
        publish(Regions {
            bar_bottom: (bar_rect.bottom() * ppp) as i32,
            minimize: px(buttons[0]),
            maximize: px(buttons[1]),
            close: px(buttons[2]),
            client,
        });
    }
    #[cfg(not(windows))]
    let _ = (ppp, bar_rect, buttons, bar);
}

fn dropdown(ui: &mut Ui, app: &App, bar: &mut BarState) -> Option<Action> {
    let i = bar.open?;
    let anchor = bar.titles.get(i).copied().flatten()?;
    let p = theme::of(ui.ctx());
    let items = menu_model::menu(app, menus::MENUS.get(i).copied().unwrap_or(""));
    let area = egui::Area::new(Id::unique(("title-menu", i)))
        .order(egui::Order::Foreground)
        .fixed_pos(anchor.left_bottom() + vec2(0.0, 2.0));
    let shown = area.show(ui.ctx(), |ui| {
        menu_draw::frame(&p)
            .show(ui, |ui| menu_draw::items(ui, &items, &p))
            .inner
    });
    let titles: Vec<Rect> = bar.titles.iter().flatten().copied().collect();
    let outside = ui.input(|inp| {
        inp.pointer.any_pressed()
            && inp.pointer.interact_pos().is_some_and(|at| {
                !shown.response.rect.contains(at) && !titles.iter().any(|r| r.contains(at))
            })
    });
    if shown.inner.is_some() || outside || ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
        bar.open = None;
    }
    shown.inner
}
