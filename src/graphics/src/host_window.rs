//! The game's host window until the front end (lane I) owns it: one window, opened on a thread of its own with a
//! winit event loop, handed to the present (`set_window`). Closing it ends the process (the run's log is flushed by
//! the exit path). The front end replaces this with its own window and calls `set_window` itself; there is never more
//! than one window (docs/INTERFACES.md, graphics).

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
#[cfg(windows)]
use winit::platform::windows::EventLoopBuilderExtWindows;
#[cfg(target_os = "linux")]
use winit::platform::x11::EventLoopBuilderExtX11;
use winit::window::{Window, WindowId};

static OPEN: AtomicBool = AtomicBool::new(false);

/// Keys go to the input layer through this (Windows virtual-key code, pressed): set by the binary that owns input.
pub type KeyHandler = fn(vk: u32, down: bool);
static KEYS: std::sync::OnceLock<KeyHandler> = std::sync::OnceLock::new();
pub fn set_key_handler(f: KeyHandler) {
    let _ = KEYS.set(f);
}

/// The Windows virtual-key code of a winit key (letters and digits as their ASCII capitals, as Win32 does).
fn vk_of(k: winit::keyboard::KeyCode) -> u32 {
    use winit::keyboard::KeyCode as K;
    let letters = [
        K::KeyA,
        K::KeyB,
        K::KeyC,
        K::KeyD,
        K::KeyE,
        K::KeyF,
        K::KeyG,
        K::KeyH,
        K::KeyI,
        K::KeyJ,
        K::KeyK,
        K::KeyL,
        K::KeyM,
        K::KeyN,
        K::KeyO,
        K::KeyP,
        K::KeyQ,
        K::KeyR,
        K::KeyS,
        K::KeyT,
        K::KeyU,
        K::KeyV,
        K::KeyW,
        K::KeyX,
        K::KeyY,
        K::KeyZ,
    ];
    if let Some(i) = letters.iter().position(|&l| l == k) {
        return b'A' as u32 + i as u32;
    }
    let digits = [
        K::Digit0,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
    ];
    if let Some(i) = digits.iter().position(|&d| d == k) {
        return b'0' as u32 + i as u32;
    }
    match k {
        K::Space => 0x20,
        K::Enter | K::NumpadEnter => 0x0D,
        K::Backspace => 0x08,
        K::Escape => 0x1B,
        K::ArrowLeft => 0x25,
        K::ArrowUp => 0x26,
        K::ArrowRight => 0x27,
        K::ArrowDown => 0x28,
        K::F1 => 0x70,
        K::F3 => 0x72,
        K::F4 => 0x73,
        K::F11 => 0x7A,
        _ => 0,
    }
}

struct App {
    title: String,
    size: (u32, u32),
    window: Option<Window>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(winit::dpi::PhysicalSize::new(self.size.0, self.size.1));
        let w = match el.create_window(attrs) {
            Ok(w) => w,
            Err(e) => {
                crate::log::red(&format!("host window: {e}"));
                el.exit();
                return;
            }
        };
        if let (Ok(d), Ok(h)) = (w.display_handle(), w.window_handle()) {
            match crate::vk::present::set_window_raw(d.as_raw(), h.as_raw()) {
                Ok(()) => crate::vklog!("host window {}x{} opened", self.size.0, self.size.1),
                Err(e) => crate::log::red(&format!("host window: {e}")),
            }
        }
        self.window = Some(w);
    }
    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => {
                crate::vklog!("host window closed: ending the run");
                crate::save_caches();
                crate::vk::present::close_window();
                std::process::exit(0);
            }
            WindowEvent::Resized(_) => crate::vk::present::window_resized(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let winit::keyboard::PhysicalKey::Code(k) = event.physical_key {
                    let vk = vk_of(k);
                    let down = event.state == winit::event::ElementState::Pressed;
                    if vk == 0x72 && down && !event.repeat {
                        crate::vk::shot::request("");
                    }
                    if let (Some(f), true) = (KEYS.get(), vk != 0) {
                        f(vk, down);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Opens the host window (once) on its own thread: `title`, client size `w` x `h`. Returns at once.
pub fn open(title: &str, w: u32, h: u32) {
    if crate::vk::present::has_window() || OPEN.swap(true, Ordering::SeqCst) {
        return;
    }
    let title = title.to_string();
    std::thread::Builder::new()
        .name("host window".into())
        .spawn(move || {
            let el = match EventLoop::builder().with_any_thread(true).build() {
                Ok(el) => el,
                Err(e) => {
                    crate::log::red(&format!("host window event loop: {e}"));
                    return;
                }
            };
            let mut app = App {
                title,
                size: (w, h),
                window: None,
            };
            if let Err(e) = el.run_app(&mut app) {
                crate::log::red(&format!("host window: {e}"));
            }
        })
        .expect("host window thread");
}
