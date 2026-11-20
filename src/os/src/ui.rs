//! Host prompts over the game (nxnative's controller-support prompt `ctrl_prompt.h` and Home-quit `home_quit.h`,
//! reimplemented): the pure state machines, tested without a window, and one small topmost window that shows a title
//! and a line of text and reports Return / Escape (the keyboard's answer; pads answer through the state machines).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

// ---- the controller-support prompt

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpPhase {
    /// Nothing shown: the game has its input.
    Idle,
    /// The prompt is open: the game's thread waits, the game reads no pads.
    Open,
    /// Answered and gone: the game gets pads again once A and B are up (the press that closed it never reaches it).
    Settle,
}

/// A game wants `min` (up to `max`) players and sees fewer: on a Switch the player screen stays open until enough
/// controllers joined or the player closes it. B or Escape cancels; with the minimum in, A or Return finishes it, as
/// does the maximum joining or no connected controller being left to join. No time limit.
#[derive(Clone, Copy, Debug)]
pub struct CtrlPrompt {
    pub phase: CpPhase,
    pub min: i32,
    pub max: i32,
    pub joined: i32,
    a_up: bool,
    b_up: bool,
    /// The answer once settled: true done, false cancelled.
    pub ok: bool,
}

impl CtrlPrompt {
    pub fn open(min: i32, max: i32, joined: i32, a: bool, b: bool) -> CtrlPrompt {
        CtrlPrompt { phase: CpPhase::Open, min, max, joined, a_up: !a, b_up: !b, ok: false }
    }
    /// One step: `joined` players the game sees, `waiting` connected controllers that are no player yet, A / B held on
    /// any controller, Return / Escape pressed since the last step.
    pub fn step(&mut self, joined: i32, waiting: i32, a: bool, b: bool, confirm: bool, cancel: bool) -> CpPhase {
        match self.phase {
            CpPhase::Idle => {}
            CpPhase::Open => {
                if joined != self.joined {
                    // a press that made a controller join does not count as an answer
                    self.joined = joined;
                    self.a_up = !a;
                    self.b_up = !b;
                }
                if !a {
                    self.a_up = true;
                }
                if !b {
                    self.b_up = true;
                }
                if (b && self.b_up) || cancel {
                    self.phase = CpPhase::Settle;
                    self.ok = false;
                } else if joined >= self.min && ((a && self.a_up) || confirm || (self.max > 0 && joined >= self.max) || waiting == 0) {
                    self.phase = CpPhase::Settle;
                    self.ok = true;
                }
            }
            CpPhase::Settle => {
                if !a && !b {
                    self.phase = CpPhase::Idle;
                }
            }
        }
        self.phase
    }
    pub fn text(&self) -> (String, String) {
        (format!("{} players wanted", self.min), format!("Press a button on each controller to join.   Players: {} of {}   (A / Return: done, B / Esc: cancel)", self.joined, self.min))
    }
}

// ---- Home held: quit?

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HqPhase {
    Idle,
    /// Home is held, not long enough yet.
    Holding,
    /// "Quit?" is shown: the game reads no pads.
    Prompt,
    /// Closed: pads come back once Home, A and B are up.
    Settle,
    /// A was pressed: quit (final).
    Quit,
}

pub const HQ_HOLD_S: f64 = 2.0;

/// Holding Home (the logo button) for 2 s opens a "Quit?" prompt: A (or Return) quits, B, Escape or Home again
/// closes it; letting go of Home earlier does nothing.
#[derive(Clone, Copy, Debug)]
pub struct HomeQuit {
    pub phase: HqPhase,
    since: f64,
    home: bool,
    a_up: bool,
    b_up: bool,
}

impl Default for HomeQuit {
    fn default() -> Self {
        HomeQuit { phase: HqPhase::Idle, since: 0.0, home: false, a_up: true, b_up: true }
    }
}

impl HomeQuit {
    pub fn step(&mut self, now: f64, home: bool, a: bool, b: bool, confirm: bool, cancel: bool) -> HqPhase {
        let pressed = home && !self.home;
        self.home = home;
        match self.phase {
            HqPhase::Idle => {
                if pressed {
                    self.phase = HqPhase::Holding;
                    self.since = now;
                }
            }
            HqPhase::Holding => {
                if !home {
                    self.phase = HqPhase::Idle;
                } else if now - self.since >= HQ_HOLD_S {
                    self.phase = HqPhase::Prompt;
                    self.a_up = !a;
                    self.b_up = !b;
                }
            }
            HqPhase::Prompt => {
                if !a {
                    self.a_up = true;
                }
                if !b {
                    self.b_up = true;
                }
                if (a && self.a_up) || confirm {
                    self.phase = HqPhase::Quit;
                } else if (b && self.b_up) || cancel || pressed {
                    self.phase = HqPhase::Settle;
                }
            }
            HqPhase::Settle => {
                if !home && !a && !b {
                    self.phase = HqPhase::Idle;
                }
            }
            HqPhase::Quit => {}
        }
        self.phase
    }
}

/// A or B held on any controller (positional: right and bottom, before the button layout).
pub fn pads_ab() -> (bool, bool) {
    let d = crate::input::devices();
    (d.iter().any(|x| x.2 & 1 != 0), d.iter().any(|x| x.2 & 2 != 0))
}

/// Connected controllers that are no player yet.
pub fn pads_waiting() -> i32 {
    crate::input::devices().iter().filter(|x| x.1.is_none()).count() as i32
}

// ---- the window

struct Shown {
    title: Vec<u16>,
    text: Vec<u16>,
}
static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);
static CONFIRM: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static THREAD: AtomicBool = AtomicBool::new(false);

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Shows (or updates) the prompt window: a title and a line of text.
pub fn show(title: &str, text: &str) {
    *SHOWN.lock().unwrap() = Some(Shown { title: w(title), text: w(text) });
    if !THREAD.swap(true, Ordering::SeqCst) {
        CONFIRM.store(false, Ordering::SeqCst);
        CANCEL.store(false, Ordering::SeqCst);
        std::thread::Builder::new().name("nxos prompt".into()).spawn(window::run).ok();
    }
    window::repaint();
}

/// Closes the prompt window.
pub fn hide() {
    *SHOWN.lock().unwrap() = None;
    window::repaint();
}

/// Return / Escape pressed in the window since the last call.
pub fn keys() -> (bool, bool) {
    (CONFIRM.swap(false, Ordering::SeqCst), CANCEL.swap(false, Ordering::SeqCst))
}

/// The prompt on Linux: the desktop's question dialog (zenity / kdialog, `desk.rs`) with the title and the line;
/// OK = Return (confirm), Cancel / closing it = Escape (cancel). The pads answer through the state machines as on
/// Windows, and `hide` closes the dialog. With no display or no dialog tool nothing opens (the pads still answer).
#[cfg(target_os = "linux")]
mod window {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static GEN: AtomicU64 = AtomicU64::new(0);

    pub fn repaint() {
        GEN.fetch_add(1, Ordering::SeqCst);
    }

    fn text(v: &[u16]) -> String {
        String::from_utf16_lossy(&v[..v.len().saturating_sub(1)])
    }

    pub fn run() {
        let mut cur: Option<(Vec<u16>, Vec<u16>)> = None;
        let mut q: Option<crate::desk::Question> = None;
        let mut warned = false;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let want = SHOWN.lock().unwrap().as_ref().map(|s| (s.title.clone(), s.text.clone()));
            if want != cur {
                if let Some(old) = q.take() {
                    old.close();
                }
                if let Some((t, x)) = &want {
                    q = crate::desk::Question::open(&text(t), &text(x));
                    if q.is_none() && !warned {
                        warned = true;
                        crate::log!("prompt: no dialog on this desktop (no display, or neither zenity nor kdialog): the controllers answer it");
                    }
                }
                cur = want;
            }
            if let Some(a) = q.as_mut().and_then(|d| d.answer()) {
                if a {
                    CONFIRM.store(true, Ordering::SeqCst);
                } else {
                    CANCEL.store(true, Ordering::SeqCst);
                }
                q = None;
            }
        }
    }
}

#[cfg(windows)]
mod window {
    use super::*;
    use std::sync::atomic::AtomicIsize;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    static HWND_: AtomicIsize = AtomicIsize::new(0);

    pub fn repaint() {
        let h = HWND_.load(Ordering::SeqCst);
        if h != 0 {
            unsafe { PostMessageW(h as HWND, WM_APP, 0, 0) };
        }
    }

    unsafe extern "system" fn proc_(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match m {
            WM_CHAR => {
                match wp as u16 {
                    0x0d => CONFIRM.store(true, Ordering::SeqCst),
                    0x1b => CANCEL.store(true, Ordering::SeqCst),
                    _ => {}
                }
                0
            }
            WM_CLOSE => {
                CANCEL.store(true, Ordering::SeqCst);
                0
            }
            WM_APP => {
                let shown = SHOWN.lock().unwrap().is_some();
                ShowWindow(h, if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
                InvalidateRect(h, std::ptr::null(), 0);
                0
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(h, &mut ps);
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(h, &mut rc);
                let bg = CreateSolidBrush(0x0022_1e1e);
                FillRect(dc, &rc, bg);
                DeleteObject(bg as _);
                SetBkMode(dc, TRANSPARENT as i32);
                if let Some(s) = SHOWN.lock().unwrap().as_ref() {
                    let face = w("Segoe UI");
                    let big = CreateFontW(30, 0, 0, 0, 600, 0, 0, 0, 0, 0, 0, 5, 0, face.as_ptr());
                    let small = CreateFontW(20, 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, 5, 0, face.as_ptr());
                    let old = SelectObject(dc, big as _);
                    SetTextColor(dc, 0x00ff_ffff);
                    let mut r1 = RECT { left: 28, top: 22, right: rc.right - 28, bottom: 70 };
                    DrawTextW(dc, s.title.as_ptr(), -1, &mut r1, DT_LEFT | DT_SINGLELINE);
                    SelectObject(dc, small as _);
                    SetTextColor(dc, 0x00c8_c8c8);
                    let mut r2 = RECT { left: 28, top: 80, right: rc.right - 28, bottom: rc.bottom - 16 };
                    DrawTextW(dc, s.text.as_ptr(), -1, &mut r2, DT_LEFT | DT_WORDBREAK);
                    SelectObject(dc, old);
                    DeleteObject(big as _);
                    DeleteObject(small as _);
                }
                EndPaint(h, &ps);
                0
            }
            _ => DefWindowProcW(h, m, wp, lp),
        }
    }

    pub fn run() {
        unsafe {
            let cls = w("nxos prompt");
            let hi = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: hi, lpszClassName: cls.as_ptr(), hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW), ..std::mem::zeroed() };
            RegisterClassW(&wc);
            // over the game's window (the front end's class), else centred on the screen
            let gcls = w("NxrsGameWindow");
            let owner = FindWindowW(gcls.as_ptr(), std::ptr::null());
            let mut orc: RECT = std::mem::zeroed();
            if owner.is_null() || GetWindowRect(owner, &mut orc) == 0 {
                orc = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) };
            }
            let (ww, wh) = (720, 170);
            let (x, y) = ((orc.left + orc.right - ww) / 2, (orc.top + orc.bottom - wh) / 2);
            let hw = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, cls.as_ptr(), cls.as_ptr(), WS_POPUP | WS_BORDER, x, y, ww, wh, owner, std::ptr::null_mut(), hi, std::ptr::null());
            if hw.is_null() {
                THREAD.store(false, Ordering::SeqCst);
                return;
            }
            HWND_.store(hw as isize, Ordering::SeqCst);
            PostMessageW(hw, WM_APP, 0, 0);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// nxnative t_ctrl_prompt's cases: a join is no answer, B cancels, A finishes with the minimum in, the maximum
    /// finishes by itself, and the closing press is held back until it is up.
    #[test]
    fn controller_prompt() {
        let mut q = CtrlPrompt::open(2, 4, 1, false, false);
        assert_eq!(q.step(1, 1, true, false, false, false), CpPhase::Open, "A with too few players does nothing");
        assert_eq!(q.step(2, 1, true, false, false, false), CpPhase::Open, "the A that joined a player is no answer");
        assert_eq!(q.step(2, 1, false, false, false, false), CpPhase::Open);
        assert_eq!(q.step(2, 1, true, false, false, false), CpPhase::Settle);
        assert!(q.ok);
        assert_eq!(q.step(2, 1, true, false, false, false), CpPhase::Settle, "held until A is up");
        assert_eq!(q.step(2, 1, false, false, false, false), CpPhase::Idle);
        let mut q = CtrlPrompt::open(2, 0, 0, false, true);
        assert_eq!(q.step(0, 1, false, true, false, false), CpPhase::Open, "B held when it opened does not cancel");
        q.step(0, 1, false, false, false, false);
        assert_eq!(q.step(0, 1, false, true, false, false), CpPhase::Settle);
        assert!(!q.ok);
        let mut q = CtrlPrompt::open(1, 2, 0, false, false);
        assert_eq!(q.step(2, 0, false, false, false, false), CpPhase::Settle, "the maximum joined");
        let mut q = CtrlPrompt::open(2, 0, 0, false, false);
        assert_eq!(q.step(0, 0, false, false, false, true), CpPhase::Settle, "Escape");
        assert!(!q.ok);
    }

    /// nxnative t_home_quit's cases: a short press does nothing, 2 s opens it, B closes, A quits.
    #[test]
    fn home_quit() {
        let mut q = HomeQuit::default();
        assert_eq!(q.step(0.0, true, false, false, false, false), HqPhase::Holding);
        assert_eq!(q.step(1.0, false, false, false, false, false), HqPhase::Idle, "let go early");
        q.step(10.0, true, false, false, false, false);
        assert_eq!(q.step(12.1, true, false, false, false, false), HqPhase::Prompt);
        assert_eq!(q.step(12.2, false, false, true, false, false), HqPhase::Settle, "B closes it");
        assert_eq!(q.step(12.3, false, false, false, false, false), HqPhase::Idle);
        q.step(20.0, true, false, false, false, false);
        q.step(22.0, true, true, false, false, false);
        assert_eq!(q.step(22.1, false, true, false, false, false), HqPhase::Prompt, "A held as it opened does not quit");
        q.step(22.2, false, false, false, false, false);
        assert_eq!(q.step(22.3, false, true, false, false, false), HqPhase::Quit);
    }
}
