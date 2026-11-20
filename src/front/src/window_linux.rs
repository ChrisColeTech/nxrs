//! The game window on Linux: a winit window (X11, or Wayland natively) on a thread of its own, handed to graphics as
//! the present surface (VK_KHR_xlib_surface / xcb / wayland). The same behaviour as the Win32 window (window.rs):
//! windowed or fullscreen (borderless on the window's monitor) from the ini's `fullscreen`, F11 and Alt+Enter switch
//! and keep the choice; the title shows the game, then the state (FPS limit, recording, frame generation).
//!
//! Keys: F1 help and controls, F3 screenshot (captures/), F4 FPS limit 60 / unlimited, F5 diagnostic dump, F7 record
//! video, F8 full-resolution capture, F11 fullscreen, Esc quit. A held key's auto-repeat never repeats these. Keys are
//! read by position (winit's physical key codes) and handed to the keyboard map as Windows virtual-key codes.

use crate::keys;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::platform::x11::EventLoopBuilderExtX11;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

static WINDOW: OnceLock<Arc<Window>> = OnceLock::new();
static FULLSCREEN: AtomicBool = AtomicBool::new(false);
static FOCUSED: AtomicBool = AtomicBool::new(false);
pub static HELP_OPEN: AtomicBool = AtomicBool::new(false);
static TITLE: Mutex<String> = Mutex::new(String::new());
static STATE: Mutex<[String; 3]> = Mutex::new([String::new(), String::new(), String::new()]);
/// The last cursor position in the client area (pixels) and the client size.
static CURSOR: Mutex<(f64, f64)> = Mutex::new((0.0, 0.0));
static LBUTTON: AtomicBool = AtomicBool::new(false);
static CLIENT: AtomicU32 = AtomicU32::new(0);

fn window() -> Option<&'static Arc<Window>> {
    WINDOW.get()
}

/// A state shown after the title: slot 0 the FPS limit, 1 recording, 2 frame generation ("" clears it).
pub fn set_state(slot: usize, text: &str) {
    STATE.lock().unwrap()[slot] = text.to_string();
    if let Some(w) = window() {
        w.set_title(&title_text());
    }
}

fn title_text() -> String {
    let mut t = TITLE.lock().unwrap().clone();
    for s in STATE.lock().unwrap().iter().filter(|s| !s.is_empty()) {
        t.push_str("  |  ");
        t.push_str(s);
    }
    t
}

pub fn set_fullscreen(on: bool) {
    let Some(w) = window() else { return };
    if FULLSCREEN.load(Ordering::Acquire) == on {
        return;
    }
    w.set_fullscreen(if on { Some(Fullscreen::Borderless(None)) } else { None });
    FULLSCREEN.store(on, Ordering::Release);
    crate::flog!("window: {}", if on { "fullscreen" } else { "windowed" });
    mouse_capture();
}

pub fn fullscreen() -> bool {
    FULLSCREEN.load(Ordering::Acquire)
}

/// F11 / Alt+Enter: the player's choice is kept in the ini, so the next start opens the same way.
fn toggle_fullscreen() {
    let on = !fullscreen();
    set_fullscreen(on);
    nxconfig::set("fullscreen", if on { "1" } else { "0" });
}

fn mouse_mode() -> bool {
    nxconfig::flag("mouse_right_stick")
}

fn minimized() -> bool {
    window().and_then(|w| w.is_minimized()).unwrap_or(false)
}

/// Grabs and hides the cursor while the mouse is the right stick and the game is in front; the cursor is hidden in
/// fullscreen too.
pub fn mouse_capture() {
    if let Some(w) = window() {
        let active = mouse_mode() && !HELP_OPEN.load(Ordering::Acquire) && FOCUSED.load(Ordering::Acquire) && !minimized();
        if active {
            if w.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                let _ = w.set_cursor_grab(CursorGrabMode::Confined);
            }
            w.set_cursor_visible(false);
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(!fullscreen());
        }
    }
    keys::MOUSE_DX.store(0, Ordering::Relaxed);
    keys::MOUSE_DY.store(0, Ordering::Relaxed);
}

pub fn focused() -> bool {
    FOCUSED.load(Ordering::Acquire) && !HELP_OPEN.load(Ordering::Acquire)
}

/// From the front-end loop: player 1's keyboard and mouse, and the focus.
pub fn poll_input() {
    static MOUSE: Mutex<keys::Mouse> = Mutex::new(keys::Mouse::new());
    let f = focused();
    nxos::input::set_focus(f);
    // the game's focus state (oe message 15): in front 1, behind another window 2, minimised 3
    let st = if minimized() { 3 } else if f { 1 } else { 2 };
    nxos::services::oe::set_focus(st);
    // nothing is presented while minimised (graphics asks Windows itself)
    nxgfx::vk::present::set_minimised(st == 3);
    // NXRS_CONTROLLER_BACKGROUND=1: the keyboard counts out of focus too (keys posted by a test script)
    static BG: OnceLock<bool> = OnceLock::new();
    let f = f || *BG.get_or_init(|| nxos::input::env_flag("NXRS_CONTROLLER_BACKGROUND"));
    nxos::input::set_suspended(HELP_OPEN.load(Ordering::Acquire));
    let held = if f { keys::HELD.load(Ordering::Acquire) } else { 0 };
    let m = if mouse_mode() { Some(MOUSE.lock().unwrap().stick(f)) } else { None };
    let (b, a) = keys::state(held, m);
    nxos::input::set_keyboard(b, a);
    home_quit();
}

/// Home held 2 s on a controller: "Quit the game?" over it (nxnative home_quit.h); A / Return quits, B / Escape /
/// Home closes it. The ini's `home_quit=0` (or `NXOS_HOME_QUIT=0`) turns it off.
fn home_quit() {
    use nxos::ui::{HomeQuit, HqPhase};
    static Q: Mutex<Option<(HomeQuit, std::time::Instant)>> = Mutex::new(None);
    if !nxconfig::flag("home_quit") || std::env::var("NXOS_HOME_QUIT").as_deref() == Ok("0") {
        return;
    }
    let mut g = Q.lock().unwrap();
    let (q, t0) = g.get_or_insert_with(|| (HomeQuit::default(), std::time::Instant::now()));
    let before = q.phase;
    let (pa, pb) = nxos::ui::pads_ab();
    let (confirm, cancel) = if before == HqPhase::Prompt { nxos::ui::keys() } else { (false, false) };
    let now = q.step(t0.elapsed().as_secs_f64(), nxos::input::home_held(), pa, pb, confirm, cancel);
    if now == before {
        return;
    }
    match now {
        HqPhase::Prompt => {
            crate::flog!("Home held: the quit prompt");
            nxos::input::set_prompt(true);
            let (title, hint) = nxos::ui_text::texts(nxos::ui_text::Prompt::HomeQuit, 0, 0);
            nxos::ui::show(&title, &hint);
        }
        HqPhase::Quit => {
            nxos::ui::hide();
            crate::flog!("Home: quit from the prompt");
            std::thread::spawn(|| crate::quit("Home"));
        }
        HqPhase::Settle => nxos::ui::hide(),
        HqPhase::Idle if before == HqPhase::Settle => nxos::input::set_prompt(false),
        _ => {}
    }
}

fn fps_limit_toggle() {
    let now = nxconfig::fps_limit_hz();
    let v = if now == 0 { "1" } else { "0" };
    nxconfig::set("fps_limit", v);
    let text = if v == "0" { "FPS limit: unlimited".to_string() } else { format!("FPS limit: {}", nxconfig::fps_limit_hz()) };
    crate::flog!("F4: {text}");
    set_state(0, &text);
}

pub fn diag_dump() {
    crate::flog!("F5: diagnostic dump (presents {}, the os gaps so far below)", nxos::nvn::presents());
    nxos::log::report_gaps("F5");
    let dir = crate::game().exe_dir.join("captures");
    let _ = std::fs::create_dir_all(&dir);
    let mut s = format!("presents {}\n", nxos::nvn::presents());
    for (k, w, n) in nxos::log::gaps() {
        s.push_str(&format!("{k:?}\t{w}\t{n}\n"));
    }
    s.push_str(&nxconfig::ctl::run("pads"));
    // every guest thread: what it waits for, the host function it is in, its guest frames (Turbo's F5)
    let threads = nxrt::kernel::try_get().map(|_| nxrt::kernel::dump_threads()).unwrap_or_default();
    crate::flog!("F5: guest threads\n{threads}");
    s.push_str("\n# guest threads\n");
    s.push_str(&threads);
    let _ = std::fs::write(dir.join("threads.txt"), s);
}

/// The Windows virtual-key code of a key by position (letters and digits as their ASCII capitals, as Win32 does):
/// what `keys::keymap` and the hotkeys read.
pub fn vk_of(k: KeyCode) -> u32 {
    use KeyCode as K;
    const LETTERS: [KeyCode; 26] = [
        K::KeyA, K::KeyB, K::KeyC, K::KeyD, K::KeyE, K::KeyF, K::KeyG, K::KeyH, K::KeyI, K::KeyJ, K::KeyK, K::KeyL, K::KeyM, K::KeyN, K::KeyO, K::KeyP, K::KeyQ, K::KeyR,
        K::KeyS, K::KeyT, K::KeyU, K::KeyV, K::KeyW, K::KeyX, K::KeyY, K::KeyZ,
    ];
    if let Some(i) = LETTERS.iter().position(|&l| l == k) {
        return b'A' as u32 + i as u32;
    }
    const DIGITS: [KeyCode; 10] = [K::Digit0, K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8, K::Digit9];
    if let Some(i) = DIGITS.iter().position(|&d| d == k) {
        return b'0' as u32 + i as u32;
    }
    const FKEYS: [KeyCode; 12] = [K::F1, K::F2, K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12];
    if let Some(i) = FKEYS.iter().position(|&f| f == k) {
        return 0x70 + i as u32;
    }
    match k {
        K::Space => 0x20,
        K::Enter | K::NumpadEnter => 0x0D,
        K::Backspace => 0x08,
        K::Tab => 0x09,
        K::Escape => 0x1B,
        K::ArrowLeft => 0x25,
        K::ArrowUp => 0x26,
        K::ArrowRight => 0x27,
        K::ArrowDown => 0x28,
        K::ShiftLeft | K::ShiftRight => 0x10,
        K::ControlLeft | K::ControlRight => 0x11,
        K::AltLeft | K::AltRight => 0x12,
        _ => 0,
    }
}

const VK_ESCAPE: u32 = 0x1B;
const VK_RETURN: u32 = 0x0D;
const VK_F1: u32 = 0x70;
const VK_F3: u32 = 0x72;
const VK_F4: u32 = 0x73;
const VK_F5: u32 = 0x74;
const VK_F7: u32 = 0x76;
const VK_F8: u32 = 0x77;
const VK_F11: u32 = 0x7A;

/// A key from the window: the hotkeys on their press (never on auto-repeat), the rest into player 1's keyboard.
fn key(vk: u32, down: bool, repeat: bool, alt: bool) {
    if vk == 0 {
        return;
    }
    if down && alt && vk == VK_RETURN {
        if !repeat {
            toggle_fullscreen();
        }
        return;
    }
    let hot = [VK_ESCAPE, VK_F1, VK_F3, VK_F4, VK_F5, VK_F7, VK_F8, VK_F11].contains(&vk);
    if hot && down {
        if repeat {
            return;
        }
        match vk {
            VK_ESCAPE => {
                // NXRS_ESC_QUIT=0 (scripted runs): Esc is logged and ignored
                let fg = FOCUSED.load(Ordering::Acquire);
                if std::env::var("NXRS_ESC_QUIT").is_ok_and(|v| v.trim() == "0") {
                    crate::flog!("Esc ignored (NXRS_ESC_QUIT=0; window in the foreground: {fg})");
                } else {
                    crate::flog!("Esc: quit (window in the foreground: {fg})");
                    std::thread::spawn(|| crate::quit("Esc"));
                }
            }
            VK_F11 => toggle_fullscreen(),
            VK_F1 => crate::help::show(),
            VK_F3 => {
                std::thread::spawn(crate::capture::screenshot);
            }
            VK_F4 => fps_limit_toggle(),
            VK_F5 => diag_dump(),
            VK_F7 => crate::rec::toggle(),
            VK_F8 => crate::capture::full_res(),
            _ => {}
        }
        return;
    }
    if hot {
        return;
    }
    if down {
        keys::HELD.fetch_or(keys::keymap(vk), Ordering::AcqRel);
    } else {
        keys::HELD.fetch_and(!keys::keymap(vk), Ordering::AcqRel);
    }
}

/// The mouse's left button as the touch screen (not while the mouse is the right stick).
fn touch(down: bool) {
    let (x, y) = *CURSOR.lock().unwrap();
    let c = CLIENT.load(Ordering::Relaxed);
    let (cw, ch) = ((c >> 16).max(1) as f64, (c & 0xffff).max(1) as f64);
    let sx = (x.max(0.0) * 1280.0 / cw) as u32;
    let sy = (y.max(0.0) * 720.0 / ch) as u32;
    nxos::input::set_touch(down, sx.min(1279), sy.min(719));
}

struct App {
    tx: Option<std::sync::mpsc::Sender<()>>,
    mods: ModifiersState,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if window().is_some() {
            return;
        }
        // the client area: the largest 16:9 that fits three quarters of the primary monitor
        let mon = el.primary_monitor().or_else(|| el.available_monitors().next());
        let (mw, mh) = mon.as_ref().map(|m| (m.size().width as i32, m.size().height as i32)).unwrap_or((1920, 1080));
        nxos::services::vi::HOST_SCREEN.store((mw as u64) << 32 | mh as u32 as u64, Ordering::Release);
        let (aw, ah) = (mw * 3 / 4, mh * 3 / 4);
        let (cw, ch) = if aw * 9 > ah * 16 { (ah * 16 / 9, ah) } else { (aw, aw * 9 / 16) };
        // NXRS_NO_ACTIVATE=1 (scripted runs): the window shows without taking the focus from whoever works at the PC
        let no_activate = std::env::var("NXRS_NO_ACTIVATE").is_ok_and(|v| v.trim() == "1");
        let attrs = Window::default_attributes()
            .with_title(title_text())
            .with_inner_size(winit::dpi::PhysicalSize::new(cw.max(640) as u32, ch.max(360) as u32))
            .with_active(!no_activate);
        let w = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                crate::flog!("[RED] window: it could not be created ({e})");
                if let Some(tx) = self.tx.take() {
                    let _ = tx.send(());
                }
                el.exit();
                return;
            }
        };
        let _ = WINDOW.set(w.clone());
        if no_activate {
            crate::flog!("window: shown without taking the focus (NXRS_NO_ACTIVATE=1)");
        }
        if nxconfig::flag("fullscreen") {
            set_fullscreen(true);
        }
        let sz = w.inner_size();
        CLIENT.store(sz.width.min(0xffff) << 16 | sz.height.min(0xffff), Ordering::Relaxed);
        use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
        let ws = match (w.display_handle(), w.window_handle()) {
            (Ok(d), Ok(h)) => match nxgfx::set_window_raw(d.as_raw(), h.as_raw()) {
                Ok(()) => format!("{:?}", d.as_raw()).split('(').next().unwrap_or("").to_string(),
                Err(e) => {
                    crate::flog!("[RED] window: {e}");
                    String::new()
                }
            },
            _ => {
                crate::flog!("[RED] window: no native handles");
                String::new()
            }
        };
        crate::flog!("window: {}x{} client ({}, {ws})", sz.width, sz.height, if fullscreen() { "fullscreen" } else { "windowed" });
        crate::lsfg::launch_app();
        mouse_capture();
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        match ev {
            WindowEvent::CloseRequested => {
                // NXRS_CLOSE_QUIT=0 (scripted runs): a single close is logged and ignored; a second one within 10 s
                // closes the game
                if std::env::var("NXRS_CLOSE_QUIT").is_ok_and(|v| v.trim() == "0") {
                    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
                    let mut last = LAST.lock().unwrap();
                    if !last.is_some_and(|t| t.elapsed().as_secs_f64() < 10.0) {
                        *last = Some(std::time::Instant::now());
                        crate::flog!("window: close ignored (NXRS_CLOSE_QUIT=0; window in the foreground: {}); a second close within 10 s quits", FOCUSED.load(Ordering::Acquire));
                        return;
                    }
                }
                crate::flog!("window: closed by the user or the system");
                std::thread::spawn(|| crate::quit("the window was closed"));
            }
            WindowEvent::Focused(f) => {
                FOCUSED.store(f, Ordering::Release);
                if !f {
                    keys::HELD.store(0, Ordering::Release);
                }
                mouse_capture();
            }
            WindowEvent::Moved(_) | WindowEvent::Occluded(_) => mouse_capture(),
            WindowEvent::Resized(sz) => {
                CLIENT.store(sz.width.min(0xffff) << 16 | sz.height.min(0xffff), Ordering::Relaxed);
                mouse_capture();
                nxgfx::vk::present::window_resized();
            }
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(k) = event.physical_key {
                    key(vk_of(k), event.state == ElementState::Pressed, event.repeat, self.mods.alt_key());
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                *CURSOR.lock().unwrap() = (position.x, position.y);
                if LBUTTON.load(Ordering::Relaxed) && !mouse_mode() {
                    touch(true);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } if !mouse_mode() => {
                let down = state == ElementState::Pressed;
                LBUTTON.store(down, Ordering::Relaxed);
                touch(down);
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, ev: DeviceEvent) {
        // raw mouse motion: the right stick while the mouse is it
        if let DeviceEvent::MouseMotion { delta } = ev {
            if mouse_mode() && focused() {
                keys::MOUSE_DX.fetch_add(delta.0 as i32, Ordering::Relaxed);
                keys::MOUSE_DY.fetch_add(delta.1 as i32, Ordering::Relaxed);
            }
        }
    }
}

/// Opens the window on its own thread and gives it to graphics; returns once it exists.
pub fn open(title: &str) {
    *TITLE.lock().unwrap() = format!("{title} - nxrs  (F1 help)");
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("window".into())
        .spawn(move || {
            let el = match EventLoop::builder().with_any_thread(true).build() {
                Ok(el) => el,
                Err(e) => {
                    crate::flog!("[RED] window: no display connection ({e}): DISPLAY or WAYLAND_DISPLAY must name a running X11 / Wayland session");
                    let _ = tx.send(());
                    return;
                }
            };
            let mut app = App { tx: Some(tx), mods: ModifiersState::empty() };
            if let Err(e) = el.run_app(&mut app) {
                crate::flog!("[RED] window: the event loop failed ({e})");
            }
            crate::quit("the window's event loop ended");
        })
        .expect("window thread");
    let _ = rx.recv();
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys_by_position_are_virtual_key_codes() {
        use winit::keyboard::KeyCode as K;
        assert_eq!(super::vk_of(K::KeyX), 0x58);
        assert_eq!(super::vk_of(K::Digit3), 0x33);
        assert_eq!(super::vk_of(K::F11), 0x7A);
        assert_eq!(super::vk_of(K::ArrowDown), 0x28);
        assert_eq!(crate::keys::keymap(super::vk_of(K::KeyZ)), 1 << 1);
    }
}
