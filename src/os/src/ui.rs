//! Host prompts over the game (nxnative's controller-support prompt `ctrl_prompt.h`, Home-quit `home_quit.h` and
//! user selector `user_select_mac.h`, reimplemented): the pure state machines, tested without a window, and one small
//! topmost window that shows a title and a line of text and reports Return / Escape, Up / Down and digits (the
//! keyboard's answer; pads answer through the state machines). Texts in the game's language (`ui_text`).

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
        crate::ui_text::texts(crate::ui_text::Prompt::Controllers, self.min, self.joined)
    }
}

// ---- the profile picker (`ask_user`)

/// The game asks who is playing and more than one local profile exists: Up / Down (D-pad, arrow keys) move, a digit
/// picks that line, A or Return selects, B or Escape cancels. The press that opened or answered it is held back.
#[derive(Clone, Debug)]
pub struct UserPicker {
    pub phase: CpPhase,
    /// (profile slot, name)
    pub choices: Vec<(u64, String)>,
    pub cursor: usize,
    /// The answer once settled: the slot, or None (cancelled).
    pub chosen: Option<u64>,
    held: [bool; 4],
}

impl UserPicker {
    /// Opens on `current` (a slot of `choices`, else the first) with A, B, Up, Down as held now.
    pub fn open(choices: Vec<(u64, String)>, current: u64, held: [bool; 4]) -> UserPicker {
        let cursor = choices.iter().position(|c| c.0 == current).unwrap_or(0);
        UserPicker { phase: CpPhase::Open, choices, cursor, chosen: None, held }
    }

    /// One step: A, B, Up, Down held on any controller; Return / Escape, an arrow (-1 up, 1 down) and a digit
    /// (1-based line) from the keyboard since the last step.
    pub fn step(&mut self, pads: [bool; 4], confirm: bool, cancel: bool, arrow: i32, digit: Option<usize>) -> CpPhase {
        let edge: Vec<bool> = (0..4).map(|i| pads[i] && !self.held[i]).collect();
        self.held = pads;
        match self.phase {
            CpPhase::Idle => {}
            CpPhase::Open => {
                let n = self.choices.len().max(1);
                if edge[2] || arrow < 0 {
                    self.cursor = (self.cursor + n - 1) % n;
                }
                if edge[3] || arrow > 0 {
                    self.cursor = (self.cursor + 1) % n;
                }
                let pick = digit.filter(|&d| d >= 1 && d <= self.choices.len()).map(|d| d - 1);
                if let Some(k) = pick {
                    self.cursor = k;
                }
                if edge[1] || cancel {
                    self.phase = CpPhase::Settle;
                    self.chosen = None;
                } else if (edge[0] || confirm || pick.is_some()) && !self.choices.is_empty() {
                    self.phase = CpPhase::Settle;
                    self.chosen = Some(self.choices[self.cursor].0);
                }
            }
            CpPhase::Settle => {
                if !pads[0] && !pads[1] {
                    self.phase = CpPhase::Idle;
                }
            }
        }
        self.phase
    }

    /// The title, and the choices (the selected one in brackets) with the hint.
    pub fn text(&self) -> (String, String) {
        let (title, hint) = crate::ui_text::texts(crate::ui_text::Prompt::UserPicker, 0, 0);
        let line: Vec<String> = self
            .choices
            .iter()
            .enumerate()
            .map(|(i, (_, name))| if i == self.cursor { format!("[ {} {name} ]", i + 1) } else { format!("{} {name}", i + 1) })
            .collect();
        (title, format!("{}\n{hint}", line.join("     ")))
    }
}

/// The profile picker over the game until it is answered (blocking the calling guest thread, as the console's
/// selector applet does): the chosen slot, or None (cancelled). The game is out of focus meanwhile.
pub fn pick_user(choices: Vec<(u64, String)>, current: u64) -> Option<u64> {
    let held = pads_nav();
    let mut q = UserPicker::open(choices, current, held);
    crate::log!("account: profile picker open ({} profiles; ask_user)", q.choices.len());
    crate::services::oe::applet_enter("profile picker");
    crate::input::set_prompt(true);
    let mut shown = String::new();
    loop {
        let (confirm, cancel) = keys();
        let (arrow, digit) = nav_keys();
        let phase = q.step(pads_nav(), confirm, cancel, arrow, digit);
        if phase == CpPhase::Open {
            let (t, x) = q.text();
            if x != shown {
                show(&t, &x);
                shown = x;
            }
        } else {
            hide();
        }
        if phase == CpPhase::Idle {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    crate::input::set_prompt(false);
    crate::services::oe::applet_leave();
    crate::log!("account: profile picker: {}", q.chosen.map_or("cancelled".into(), |s| format!("profile {}", s + 1)));
    q.chosen
}

/// A, B, D-pad up, D-pad down held on any controller (positional, before the button layout).
pub fn pads_nav() -> [bool; 4] {
    let d = crate::input::devices();
    let any = |bit: u32| d.iter().any(|x| x.2 >> bit & 1 != 0);
    [any(0), any(1), any(13), any(15)]
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
/// Arrow keys since the last `nav_keys` (up -1, down +1) and the last digit typed (0 none).
static ARROW: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static DIGIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
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

/// Arrow keys (-1 up, +1 down, 0 none) and a digit (1-9) typed in the window since the last call.
pub fn nav_keys() -> (i32, Option<usize>) {
    let a = ARROW.swap(0, Ordering::SeqCst).signum();
    let d = DIGIT.swap(0, Ordering::SeqCst);
    (a, (d > 0).then_some(d))
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
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_UP};
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
                    c @ 0x31..=0x39 => DIGIT.store((c - 0x30) as usize, Ordering::SeqCst),
                    _ => {}
                }
                0
            }
            WM_KEYDOWN if wp == VK_UP as usize || wp == VK_DOWN as usize => {
                ARROW.fetch_add(if wp == VK_UP as usize { -1 } else { 1 }, Ordering::SeqCst);
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

    /// The profile picker: the press that opened it is no answer, D-pad / arrows move (wrapping), A selects, B
    /// cancels, a digit picks its line.
    #[test]
    fn user_picker() {
        let users = vec![(0, "Player".to_string()), (2, "Player 3".to_string()), (5, "Player 6".to_string())];
        let mut q = UserPicker::open(users.clone(), 2, [true, false, false, false]);
        assert_eq!(q.cursor, 1, "opens on the current profile");
        assert_eq!(q.step([true, false, false, false], false, false, 0, None), CpPhase::Open, "A held as it opened");
        q.step([false, false, false, true], false, false, 0, None);
        assert_eq!(q.cursor, 2);
        q.step([false, false, false, true], false, false, 0, None);
        assert_eq!(q.cursor, 2, "held: one step");
        q.step([false, false, false, false], false, false, 1, None);
        assert_eq!(q.cursor, 0, "wraps");
        assert!(q.text().1.contains("[ 1 Player ]"));
        assert_eq!(q.step([true, false, false, false], false, false, 0, None), CpPhase::Settle);
        assert_eq!(q.chosen, Some(0));
        assert_eq!(q.step([true, false, false, false], false, false, 0, None), CpPhase::Settle, "held until A is up");
        assert_eq!(q.step([false; 4], false, false, 0, None), CpPhase::Idle);
        let mut q = UserPicker::open(users.clone(), 0, [false; 4]);
        q.step([false; 4], false, true, 0, None);
        assert_eq!(q.chosen, None, "Escape cancels");
        let mut q = UserPicker::open(users, 0, [false; 4]);
        q.step([false; 4], false, false, 0, Some(3));
        assert_eq!(q.chosen, Some(5), "the digit 3 picks the third line");
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
