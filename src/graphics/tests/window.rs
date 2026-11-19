//! A window of its own, presented into through the NVN API as a game does (acquire, render, present at interval 1),
//! for a fixed number of frames; the pacing lines it logs are checked against the launcher's parser format. It
//! closes itself. `cargo test -p graphics --features gpu-tests --test window`.

mod common;
use common::*;
use nxgfx::nvn::cmd::*;
use nxgfx::nvn::queue::*;
use nxgfx::nvn::types::*;
use nxgfx::nvn::window::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::{Window, WindowId};

#[derive(Default)]
struct App {
    window: Option<Window>,
    closed: bool,
}
impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title("nxgfx present test")
                .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0))
                .with_active(false);
            self.window = Some(el.create_window(attrs).expect("window"));
        }
    }
    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        if let WindowEvent::CloseRequested = ev {
            self.closed = true;
        }
    }
}

const FRAMES: u32 = 300;

fn main() {
    let _g = setup();
    std::env::set_var("NXGFX_PACE_N", "60");
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let l = lines.clone();
        nxgfx::log::set_sink(move |s| {
            eprintln!("{s}");
            l.lock().unwrap().push(s.to_string());
        });
    }
    let mut el = EventLoop::new().expect("event loop");
    let mut app = App::default();
    let t0 = Instant::now();
    while app.window.is_none() && t0.elapsed() < Duration::from_secs(5) {
        el.pump_app_events(Some(Duration::from_millis(10)), &mut app);
    }
    let window = app.window.as_ref().expect("the window opened");
    let (hwnd, hinst) = match window.window_handle().unwrap().as_raw() {
        RawWindowHandle::Win32(h) => (h.hwnd.get(), h.hinstance.map(|x| x.get()).unwrap_or(0)),
        _ => panic!("not a Win32 window"),
    };

    let mut n = Nvn::new();
    let e0 = validation_errors();
    nxgfx::set_window(hwnd, hinst);
    // the game's swap chain: two 1280x720 RGBA8 textures
    let mut texs: Vec<Box<NVNtexture>> = (0..2).map(|_| n.texture(1280, 720, 37, tex_flags::DISPLAY, false)).collect();
    let tp: Vec<*mut NVNtexture> = texs.iter_mut().map(|t| &mut **t as *mut NVNtexture).collect();
    let mut wb = NVNwindowBuilder::boxed();
    let mut win = NVNwindow::boxed();
    unsafe {
        nvnWindowBuilderSetDevice(&mut *wb, n.d());
        nvnWindowBuilderSetDefaults(&mut *wb);
        nvnWindowBuilderSetTextures(&mut *wb, 2, tp.as_ptr());
        nvnWindowBuilderSetPresentInterval(&mut *wb, 1);
        assert_eq!(nvnWindowInitialize(&mut *win, &*wb), 1);
    }
    let vs = spv("tri_vert");
    let fs = spv("tri_frag");
    let prog = n.program(&[(&vs, &json("vs", "")), (&fs, &json("ps", ""))]);
    let mut acq = NVNsync::boxed();
    unsafe { nxgfx::nvn::sync::nvnSyncInitialize(&mut *acq, n.d()) };
    let start = Instant::now();
    for f in 0..FRAMES {
        el.pump_app_events(Some(Duration::ZERO), &mut app);
        if app.closed {
            break;
        }
        let mut idx = 0i32;
        unsafe { nvnWindowAcquireTexture(&mut *win, &mut *acq, &mut idx) };
        let t = &*texs[idx as usize];
        let a = f as f32 * 0.05;
        n.begin();
        n.set_target(t, None, 1280, 720);
        let bg = [0.1, 0.1 + 0.4 * (f as f32 / FRAMES as f32), 0.3, 1.0];
        unsafe { nvnCommandBufferClearColor(n.c(), 0, bg.as_ptr(), 0xf) };
        unsafe { nvnCommandBufferBindProgram(n.c(), &*prog, 0x1f) };
        let v = |k: f32| [0.7 * (a + k).cos(), 0.7 * (a + k).sin(), 0.0];
        let mut data = Vec::new();
        for (p, c) in [(v(0.0), [1.0f32, 0.2, 0.2, 1.0]), (v(2.094), [0.2, 1.0, 0.2, 1.0]), (v(4.189), [0.2, 0.2, 1.0, 1.0])] {
            data.extend_from_slice(&p);
            data.extend_from_slice(&c);
        }
        let va = n.data.put(&data, 256);
        n.vertex_layout(&[(34, 0), (46, 12)], 28);
        unsafe {
            nvnCommandBufferBindVertexBuffer(n.c(), 0, va, data.len() * 4);
            nvnCommandBufferDrawArrays(n.c(), prim::TRIANGLES, 0, 3);
            let h = nvnCommandBufferEndRecording(n.c());
            nvnQueueSubmitCommands(&mut *n.queue, 1, &h);
            nvnQueuePresentTexture(&mut *n.queue, &mut *win, idx);
            nvnCommandBufferAddCommandMemory(n.c(), n.cmd_pool.p(), 0, 1 << 20);
        }
        if n.data.next > (15 << 20) {
            n.finish();
            n.data.next = 0;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    n.finish();
    let presented = nxgfx::vk::present::presented();
    unsafe { nvnWindowFinalize(&mut *win) };
    nxgfx::close_window();
    drop(app.window.take());
    el.pump_app_events(Some(Duration::from_millis(10)), &mut app);

    // the pacing lines, as the launcher's parser reads them
    let mut paced = 0;
    for l in lines.lock().unwrap().iter() {
        if let Some(i) = l.find("] [vk] pacing: ") {
            let rest = &l[i + 15..];
            let fps: f64 = rest.split(' ').next().unwrap().parse().expect("FPS");
            assert!(rest.contains(" FPS | p95 ") && rest.contains(" p99 ") && rest.contains(" max ") && rest.contains(" ms | >16.67 "), "{l}");
            assert!(rest.contains(" >33/50/100 ") && rest.trim_end().ends_with("intervals)"), "{l}");
            assert!(l.starts_with('[') && l[1..i].trim().parse::<f64>().is_ok(), "{l}");
            assert!(fps > 30.0 && fps < 70.0, "paced near 60: {l}");
            paced += 1;
        }
    }
    let errs = validation_errors() - e0;
    eprintln!("window: {FRAMES} frames in {secs:.2} s ({:.1} per s), {presented} presented, {paced} pacing lines, {errs} validation errors", FRAMES as f64 / secs);
    assert!(presented >= FRAMES as u64 - 1, "presents reached the swap chain: {presented}");
    assert!(paced >= 3, "pacing lines: {paced}");
    assert_eq!(errs, 0, "validation errors");
    eprintln!("window test: ok");
}
