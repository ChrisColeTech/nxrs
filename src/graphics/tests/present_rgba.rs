//! `vk::present::present_rgba` on the GPU: pictures made on the CPU (as vi's buffer queue hands them over) uploaded
//! and presented into a window of its own, at two sizes (the host image made again when the size changes), with the
//! validation layer on. `cargo test -p graphics --features gpu-tests --test present_rgba`.

mod common;
use common::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::{Window, WindowId};

#[derive(Default)]
struct App {
    window: Option<Window>,
}
impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title("nxgfx present_rgba test")
                .with_inner_size(winit::dpi::LogicalSize::new(640.0, 360.0))
                .with_active(false);
            self.window = Some(el.create_window(attrs).expect("window"));
        }
    }
    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, _ev: WindowEvent) {}
}

/// A w x h gradient that moves with the frame number.
fn picture(w: u32, h: u32, f: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[(x + f) as u8, (y * 2) as u8, (f * 3) as u8, 255]);
        }
    }
    v
}

fn main() {
    let _g = setup();
    let mut el = EventLoop::new().expect("event loop");
    let mut app = App::default();
    let t0 = Instant::now();
    while app.window.is_none() && t0.elapsed() < Duration::from_secs(5) {
        el.pump_app_events(Some(Duration::from_millis(10)), &mut app);
    }
    let window = app.window.as_ref().expect("the window opened");
    let raw = (window.display_handle().unwrap().as_raw(), window.window_handle().unwrap().as_raw());
    let mut n = Nvn::new(); // the device, as a game brings it up
    let e0 = validation_errors();
    nxgfx::set_window_raw(raw.0, raw.1).expect("a window Vulkan can present to");
    assert_eq!(nxgfx::vk::present::present_rgba(0, 0, &[]), 0, "an empty picture is refused");
    let p0 = nxgfx::vk::present::presented();
    let mut fences = 0;
    // 1280x720 (the docked layer of a raw vi client) then 320x240: the host image is made again
    for (w, h, frames) in [(1280u32, 720u32, 30u32), (320, 240, 30)] {
        for f in 0..frames {
            el.pump_app_events(Some(Duration::ZERO), &mut app);
            let pic = picture(w, h, f);
            if nxgfx::vk::present::present_rgba(w, h, &pic) != 0 {
                fences += 1;
            }
        }
    }
    n.finish();
    let presented = nxgfx::vk::present::presented() - p0;
    nxgfx::close_window();
    drop(app.window.take());
    el.pump_app_events(Some(Duration::from_millis(10)), &mut app);
    let errs = validation_errors() - e0;
    eprintln!("present_rgba: {fences} submitted, {presented} presented, {errs} validation errors");
    assert_eq!(fences, 60, "every picture submitted");
    assert!(presented >= 58, "the pictures reached the swap chain: {presented}");
    assert_eq!(errs, 0, "validation errors");
    eprintln!("present_rgba test: ok");
}
