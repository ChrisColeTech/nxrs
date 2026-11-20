//! The software keyboard (nn::swkbd: ShowKeyboard and the InlineKeyboard), answered on the host. A game asking for
//! text gets a text box over its window (Turbo's keyboard window: a dark panel, the Hind font, typed on the PC's
//! keyboard in any layout; Enter = OK, Esc = keep the default text). The default is the game's initial text, else
//! the `keyboard_text` setting ("Player"). With no window to show it over (`NXOS_SWKBD=auto`, a headless run) the
//! default is answered at once. nxnative `hle_swkbd.c`: the ShowKeyboardArg layout and the inline keyboard's
//! callbacks, from Calc.

use crate::guest::{r32, r64, r8, w32, w64, Args};
use crate::hle::Registry;
use crate::sched::without_core;
use nxrt::cpu::Cpu;
use std::sync::Mutex;

const ARG_HEADER: u64 = 0x24;
const ARG_MAXLEN: u64 = 0x3ac;
const ARG_UTF8: u64 = 0x3bd;
const ARG_INIT_OFF: u64 = 0x3c0;
const ARG_INIT_LEN: u64 = 0x3c4;
const ARG_WORKBUF: u64 = 0x4d0;
const CANCELED: u64 = 0x29f;

fn u16s(a: u64, max: usize) -> String {
    let mut v = vec![];
    for i in 0..max as u64 {
        let c = crate::guest::r16(a + 2 * i);
        if c == 0 {
            break;
        }
        v.push(c);
    }
    String::from_utf16_lossy(&v)
}

/// The text the player gives (or the default, with no window); None = cancelled.
pub fn ask(title: &str, default: &str, max: usize) -> Option<String> {
    let auto = std::env::var("NXOS_SWKBD").is_ok_and(|v| v == "auto") || std::env::var("NXGFX_HEADLESS").is_ok();
    crate::services::oe::applet_enter("swkbd");
    let r = if auto { Some(default.to_string()) } else { without_core(|| window::input(title, default, max)) };
    crate::services::oe::applet_leave();
    let r = r.map(|t| t.chars().take(if max > 0 { max } else { 500 }).collect::<String>());
    crate::log!("swkbd: \"{title}\" -> {}", match &r {
        Some(t) => format!("OK '{t}'"),
        None => "cancelled".into(),
    });
    r
}

fn default_text() -> String {
    let t = crate::options::get("keyboard_text");
    if t.is_empty() {
        "Player".into()
    } else {
        t
    }
}

/// ShowKeyboard(String *out, const ShowKeyboardArg &arg): out = {+0 buffer, +8 capacity in bytes}.
unsafe fn show(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (out, arg) = (a.x(0), a.x(1));
    let title = u16s(arg + ARG_HEADER, 65);
    let max = r32(arg + ARG_MAXLEN) as usize;
    let utf8 = r8(arg + ARG_UTF8) != 0;
    let work = r64(arg + ARG_WORKBUF);
    let (off, len) = (r32(arg + ARG_INIT_OFF) as u64, r32(arg + ARG_INIT_LEN) as usize);
    let init = if work != 0 && len > 0 { u16s(work + off, len.min(500)) } else { String::new() };
    let def = if init.is_empty() { default_text() } else { init };
    let Some(text) = ask(&title, &def, max) else {
        (*c).x[0] = CANCELED;
        return;
    };
    let (buf, cap) = (r64(out), r64(out + 8));
    if buf != 0 && cap > 0 {
        crate::guest::zero(buf, cap);
        if utf8 {
            crate::guest::put_cstr(buf, cap, &text);
        } else {
            let u: Vec<u16> = text.encode_utf16().take((cap as usize / 2).saturating_sub(1)).collect();
            for (i, ch) in u.iter().enumerate() {
                crate::guest::w16(buf + 2 * i as u64, *ch);
            }
        }
    }
    (*c).x[0] = 0;
}

// ---- the inline keyboard

#[derive(Default, Clone, Copy)]
struct Ikb {
    kb: u64,
    cancel: u64,
    fin_init: u64,
    released: u64,
    finished: u64,
    enter: u64,
    changed: u64,
    pending: bool,
    fin_init_due: bool,
    released_due: bool,
    finished_due: bool,
    max: usize,
    state: u64,
}

static IKB: Mutex<Vec<Ikb>> = Mutex::new(Vec::new());

fn with_ikb<R>(kb: u64, f: impl FnOnce(&mut Ikb) -> R) -> R {
    let mut g = IKB.lock().unwrap();
    let i = match g.iter().position(|k| k.kb == kb) {
        Some(i) => i,
        None => {
            g.push(Ikb { kb, ..Default::default() });
            g.len() - 1
        }
    };
    f(&mut g[i])
}

macro_rules! kfn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

kfn!(ikb_init, |a| {
    with_ikb(a.x(0), |k| k.fin_init_due = true);
    crate::log!("swkbd: inline keyboard {:#x} initialized (Appear asks for the text as ShowKeyboard does)", a.x(0));
    1
});
kfn!(ikb_fin, |a| {
    IKB.lock().unwrap().retain(|k| k.kb != a.x(0));
    0
});
kfn!(ikb_set_cancel, |a| {
    with_ikb(a.x(0), |k| k.cancel = a.x(1));
    0
});
kfn!(ikb_set_enter, |a| {
    with_ikb(a.x(0), |k| k.enter = a.x(1));
    0
});
kfn!(ikb_set_changed, |a| {
    with_ikb(a.x(0), |k| k.changed = a.x(1));
    0
});
kfn!(ikb_set_finished, |a| {
    with_ikb(a.x(0), |k| k.finished = a.x(1));
    0
});
kfn!(ikb_set_fin_init, |a| {
    with_ikb(a.x(0), |k| k.fin_init = a.x(1));
    0
});
kfn!(ikb_set_released, |a| {
    with_ikb(a.x(0), |k| k.released = a.x(1));
    0
});
kfn!(ikb_unset_words, |a| {
    with_ikb(a.x(0), |k| k.released_due = true);
    0
});
kfn!(ikb_appear, |a| {
    let max = if a.x(1) != 0 { r32(a.x(1) + 0x20) as i32 } else { 0 };
    with_ikb(a.x(0), |k| {
        k.pending = true;
        k.state = 3;
        k.max = if max > 0 && max <= 500 { max as usize } else { 0 };
    });
    0
});
kfn!(ikb_disappear, |a| {
    with_ikb(a.x(0), |k| {
        k.pending = false;
        k.finished_due = true;
        if k.state != 0 {
            k.state = 1;
        }
    });
    0
});
kfn!(ikb_ok, |a| { 0 });
kfn!(ikb_height, |a| { 400 });

/// Calc: the callbacks due, then a pending Appear's text through the enter (and changed-string) callbacks.
unsafe fn ikb_calc(c: *mut Cpu) {
    let kb = (*c).x[0];
    let k = with_ikb(kb, |k| {
        let snap = *k;
        if k.fin_init_due {
            if k.state == 0 {
                k.state = 1;
            }
            if k.fin_init != 0 {
                k.fin_init_due = false;
            }
        }
        if k.released_due && k.released != 0 {
            k.released_due = false;
        }
        k.finished_due = false;
        k.pending = false;
        snap
    });
    let call0 = |f: u64| {
        if f != 0 {
            let _ = crate::guest::call(c, f, &[]);
        }
    };
    if k.fin_init_due {
        call0(k.fin_init);
    }
    if k.released_due {
        call0(k.released);
    }
    if k.finished_due {
        call0(k.finished);
    }
    if !k.pending {
        (*c).x[0] = with_ikb(kb, |k| k.state);
        return;
    }
    let text = if k.enter != 0 { ask("", &default_text(), k.max) } else { None };
    with_ikb(kb, |k| k.state = 1);
    let Some(text) = text else {
        call0(k.cancel);
        (*c).x[0] = 1;
        return;
    };
    // ChangedStringArg / DecidedEnterArg: u16 text[502], then length, dictionary start / end, cursor (0x3fc bytes)
    static ARG: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let arg = *ARG.get_or_init(|| nxrt::kernel::get().space.kernel_alloc(0x400, 16));
    crate::guest::zero(arg, 0x400);
    let u: Vec<u16> = text.encode_utf16().take(if k.max > 0 { k.max } else { 500 }).collect();
    for (i, ch) in u.iter().enumerate() {
        crate::guest::w16(arg + 2 * i as u64, *ch);
    }
    w32(arg + 0x3ec, u.len() as u32);
    w32(arg + 0x3f8, u.len() as u32);
    if k.changed != 0 {
        let _ = crate::guest::call(c, k.changed, &[arg]);
    }
    let _ = crate::guest::call(c, k.enter, &[arg]);
    (*c).x[0] = 1;
}

/// The keyboard window (Win32): a dark panel over the game's window with the game's title, the text in the Hind
/// font (the default selected: the first key typed replaces it), Enter = OK, Esc = keep the default.
mod window {
    use std::sync::mpsc;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    static FONT: &[u8] = include_bytes!("../../third_party/fonts/Hind-Regular.ttf");

    struct Kb {
        title: Vec<u16>,
        text: Vec<u16>,
        all: bool,
        max: usize,
        done: Option<bool>,
    }
    static mut KB: Option<Kb> = None;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn proc_(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        #[allow(static_mut_refs)]
        let Some(kb) = KB.as_mut() else { return DefWindowProcW(h, m, wp, lp) };
        match m {
            WM_CHAR => {
                let ch = wp as u16;
                match ch {
                    0x0d => kb.done = Some(true),
                    0x1b => kb.done = Some(false),
                    0x08 => {
                        if kb.all {
                            kb.text.clear();
                        } else {
                            kb.text.pop();
                        }
                        kb.all = false;
                    }
                    c if c >= 0x20 => {
                        if kb.all {
                            kb.text.clear();
                            kb.all = false;
                        }
                        if kb.max == 0 || kb.text.len() < kb.max {
                            kb.text.push(c);
                        }
                    }
                    _ => {}
                }
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
                let face = w("Hind");
                let big = CreateFontW(34, 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, 5, 0, face.as_ptr());
                let small = CreateFontW(22, 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, 5, 0, face.as_ptr());
                let old = SelectObject(dc, small as _);
                SetTextColor(dc, 0x00e1_c800);
                let mut r1 = RECT { left: 28, top: 18, right: rc.right - 28, bottom: 60 };
                let t = if kb.title.len() > 1 { kb.title.clone() } else { w("Enter text") };
                DrawTextW(dc, t.as_ptr(), -1, &mut r1, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
                let field = RECT { left: 24, top: 70, right: rc.right - 24, bottom: 140 };
                let fb = CreateSolidBrush(if kb.all { 0x0050_3c14 } else { 0x0034_302c });
                FillRect(dc, &field, fb);
                DeleteObject(fb as _);
                SelectObject(dc, big as _);
                SetTextColor(dc, 0x00ff_ffff);
                let mut r2 = RECT { left: 40, top: 70, right: rc.right - 40, bottom: 140 };
                let mut shown = kb.text.clone();
                if !kb.all {
                    shown.push('|' as u16);
                }
                shown.push(0);
                DrawTextW(dc, shown.as_ptr(), -1, &mut r2, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
                SelectObject(dc, small as _);
                SetTextColor(dc, 0x00a0_a0a0);
                let mut r3 = RECT { left: 28, top: 160, right: rc.right - 28, bottom: 200 };
                let hint = w("Type with your keyboard.   Enter = OK   Esc = keep the text shown");
                DrawTextW(dc, hint.as_ptr(), -1, &mut r3, DT_LEFT | DT_SINGLELINE);
                SelectObject(dc, old);
                DeleteObject(big as _);
                DeleteObject(small as _);
                EndPaint(h, &ps);
                0
            }
            WM_CLOSE => {
                kb.done = Some(false);
                0
            }
            _ => DefWindowProcW(h, m, wp, lp),
        }
    }

    pub fn input(title: &str, default: &str, max: usize) -> Option<String> {
        let (tx, rx) = mpsc::channel();
        let (title, default) = (title.to_string(), default.to_string());
        std::thread::spawn(move || unsafe {
            let mut n = 0u32;
            AddFontMemResourceEx(FONT.as_ptr() as _, FONT.len() as u32, std::ptr::null(), &mut n);
            KB = Some(Kb { title: w(&title), text: default.encode_utf16().collect(), all: true, max, done: None });
            let cls = w("nxos keyboard");
            let hi = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: hi, lpszClassName: cls.as_ptr(), hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW), ..std::mem::zeroed() };
            RegisterClassW(&wc);
            let owner = GetForegroundWindow();
            let mut orc: RECT = std::mem::zeroed();
            if owner.is_null() || GetWindowRect(owner, &mut orc) == 0 {
                orc = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) };
            }
            let (ww, wh) = (680, 220);
            let (x, y) = ((orc.left + orc.right - ww) / 2, (orc.top + orc.bottom - wh) / 2);
            let hw = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, cls.as_ptr(), cls.as_ptr(), WS_POPUP | WS_VISIBLE | WS_BORDER, x, y, ww, wh, owner, std::ptr::null_mut(), hi, std::ptr::null());
            if hw.is_null() {
                let _ = tx.send(Some(default.clone()));
                return;
            }
            SetForegroundWindow(hw);
            SetFocus(hw);
            let mut msg: MSG = std::mem::zeroed();
            #[allow(static_mut_refs)]
            while KB.as_ref().is_some_and(|k| k.done.is_none()) && GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            #[allow(static_mut_refs)]
            let k = KB.take().unwrap();
            DestroyWindow(hw);
            if !owner.is_null() {
                SetForegroundWindow(owner);
            }
            let r = if k.done == Some(true) { String::from_utf16_lossy(&k.text) } else { default.clone() };
            let _ = tx.send(Some(r));
        });
        rx.recv().ok().flatten()
    }
}

pub fn register(r: &mut Registry) {
    const W: &str = "swkbd: a text box over the game window (keyboard_text by default)";
    let k = "_ZN2nn5swkbd14InlineKeyboard";
    for (n, f) in [
        ("_ZN2nn5swkbd12ShowKeyboardEPNS0_6StringERKNS0_15ShowKeyboardArgE".to_string(), show as unsafe fn(*mut Cpu)),
        (format!("{k}10InitializeEPv"), ikb_init),
        (format!("{k}8FinalizeEv"), ikb_fin),
        (format!("{k}D1Ev"), ikb_fin),
        (format!("{k}24SetDecidedCancelCallbackEPFvvE"), ikb_set_cancel),
        (format!("{k}23SetDecidedEnterCallbackEPFvPKNS0_15DecidedEnterArgEE"), ikb_set_enter),
        (format!("{k}24SetChangedStringCallbackEPFvPKNS0_16ChangedStringArgEE"), ikb_set_changed),
        (format!("{k}27SetFinishedKeyboardCallbackEPFvvE"), ikb_set_finished),
        (format!("{k}29SetFinishedInitializeCallbackEPFvvE"), ikb_set_fin_init),
        (format!("{k}31SetReleasedUserWordInfoCallbackEPFvvE"), ikb_set_released),
        (format!("{k}17UnsetUserWordInfoEv"), ikb_unset_words),
        (format!("{k}15SetUserWordInfoERKNS0_12UserWordInfoE"), ikb_ok),
        (format!("{k}25SetCustomizedDictionariesERKNS0_23CustomizedDictionarySetE"), ikb_ok),
        (format!("{k}27UnsetCustomizedDictionariesEv"), ikb_ok),
        (format!("{k}6AppearERKNS0_9AppearArgE"), ikb_appear),
        (format!("{k}9DisappearEv"), ikb_disappear),
        (format!("{k}4CalcEv"), ikb_calc),
        (format!("{k}26IsUsedTouchPointByKeyboardEii"), ikb_ok),
        (format!("{k}12GetMaxHeightEv"), ikb_height),
        (format!("{k}12SetInputTextERKNS0_9InputTextE"), ikb_ok),
        (format!("{k}12SetCursorPosEi"), ikb_ok),
    ] {
        r.force(&n, W, f);
    }
    let _ = w64;
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CHAR};

    /// Types into the real Win32 text box: `keys` posted as WM_CHAR to the window once it is up.
    fn type_into(keys: &'static [u16]) {
        std::thread::spawn(move || unsafe {
            let cls: Vec<u16> = "nxos keyboard\0".encode_utf16().collect();
            for _ in 0..500 {
                let h = FindWindowW(cls.as_ptr(), std::ptr::null());
                if !h.is_null() {
                    for &k in keys {
                        PostMessageW(h, WM_CHAR, k as usize, 0);
                    }
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
    }

    /// The text box path itself (a real window, its message loop and WM_CHAR handling): typed text replaces the
    /// selected default, Backspace edits, Enter answers it; Esc keeps the default. One test: one box at a time.
    /// Opt-in (`cargo test -p os -- --ignored win32_text_box`): it opens a real topmost window on the desktop, which
    /// a default test run must not do while someone uses the PC; with no interactive desktop the box cannot open and
    /// the default comes back.
    #[test]
    #[ignore]
    fn win32_text_box_answers_what_was_typed() {
        type_into(&[b'A' as u16, b'x' as u16, 0x08, b'b' as u16, 0x0d]);
        assert_eq!(super::window::input("Your name", "Player", 10).as_deref(), Some("Ab"));
        type_into(&[b'Z' as u16, 0x1b]);
        assert_eq!(super::window::input("Your name", "Player", 10).as_deref(), Some("Player"));
        // the length limit
        type_into(&[b'1' as u16, b'2' as u16, b'3' as u16, 0x0d]);
        assert_eq!(super::window::input("Your name", "", 2).as_deref(), Some("12"));
    }
}
