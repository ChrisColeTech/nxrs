//! `--screenshot <png>`: after its delay the window's frame is saved (at most 1600 px wide) and the
//! launcher exits; an open dialog is drawn inside the main window for it (as the Qt launcher composited
//! it). Also the window's placement, kept on close.

use crate::constants::layout;
use crate::model::library::{Library, WindowPlacement};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

#[derive(Default)]
pub struct Shot {
    /// (where, the time to take it)
    pub take: Option<(String, f64)>,
    pub asked: bool,
    /// the exit code once written: 0 saved, 1 failed; -1 still waiting
    pub done: Option<Arc<AtomicI32>>,
}

pub fn step(ctx: &egui::Context, shot: &mut Shot) {
    let Some((path, at)) = shot.take.clone() else {
        return;
    };
    if let Some(done) = &shot.done {
        if done.load(Ordering::SeqCst) >= 0 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
        return;
    }
    if ctx.input(|i| i.time) < at {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        return;
    }
    let code = Arc::new(AtomicI32::new(-1));
    let written = code.clone();
    let notify = ctx.clone();
    ctx.request_screenshot(move |image| {
        let [w, h] = image.size;
        let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
        let ok = crate::flows::write_png(&path, (w as u32, h as u32), rgba, layout::SCREENSHOT_MAX)
            .is_ok();
        written.store(if ok { 0 } else { 1 }, Ordering::SeqCst);
        notify.request_repaint();
    });
    shot.done = Some(code);
    shot.asked = true;
}

/// The exit code `--screenshot` ended with (1 when it never ran).
pub fn exit_code(shot: &Shot) -> i32 {
    shot.done
        .as_ref()
        .map(|d| d.load(Ordering::SeqCst))
        .filter(|c| *c >= 0)
        .unwrap_or(1)
}

/// Where the window is, for the next start.
pub fn keep_placement(ctx: &egui::Context, lib: &mut Library) {
    let (outer, inner, max) = ctx.input(|i| {
        (
            i.viewport().outer_rect,
            i.viewport().inner_rect,
            i.viewport().maximized.unwrap_or(false),
        )
    });
    let maximized = max || crate::ui::chrome::caption::maximized(ctx);
    if let (Some(o), Some(i)) = (outer, inner) {
        let keep = lib.window.clone().filter(|_| maximized);
        lib.window = Some(keep.map_or(
            WindowPlacement {
                x: o.min.x,
                y: o.min.y,
                width: i.width(),
                height: i.height(),
                maximized,
            },
            |k| WindowPlacement { maximized, ..k },
        ));
    }
}
