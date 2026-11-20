//! The host's controllers: every physical pad found, given a player slot, read on an input thread of its own, and
//! published for the hid service (`services/hid.rs`), which shows them to the game as its npads.
//!
//! Sources, by kind: XInput users (Xbox pads), native HID (DualSense / DualSense Edge / DualShock 4 with lights,
//! rumble and motion; Switch Pro Controller, Joy-Con L / R and Nintendo's Online pads with HD rumble, player lights
//! and motion), WinMM joysticks as the fallback when no other pad is there; the keyboard and mouse through the front
//! end (`set_keyboard`); scripted presses (`NXRS_KEYS`, the control pipe's `press`); a replayed recording.
//!
//! Every pad reports its buttons by position (`layout.rs`), mapped once here through `button_layout` / `button_map`.
//! Slots follow nxnative's `pad_slots.h`: a pad takes the slot a pad of its name last held, else player 1 when free,
//! else the lowest slot never held, else the lowest free one. A left and a right Joy-Con make one pad (dual), as the
//! console pairs them. Hot-plug: XInput is read every tick, HID and WinMM are rescanned every 2 s (and when the front
//! end reports a device change).
//!
//! Switches: `NXRS_CONTROLLER_TRACE=1` (every change and output), `NXRS_CONTROLLER_SELFTEST=1` (lights, low motor
//! 0.5 s, high motor 0.5 s on each rumble pad once it is active), `NXRS_CONTROLLER_BACKGROUND=1` (input while the window
//! is not in front), `NXRS_NO_SONY_HID=1` / `NXRS_NO_NINTENDO_HID=1` (skip those native paths, e.g. under DS4Windows
//! without HidHide), `NXRS_INPUT_RECORD=<file>` / `NXRS_INPUT_REPLAY=<file>` (Turbo's record / replay format),
//! `NXRS_KEYS=<steps>` (scripted presses), `NXRS_MOTION_GAIN`. Turbo's `SWR_` names are read too.

pub mod hidio;
pub mod layout;
pub mod motion;
pub mod nintendo;
pub mod script;
pub mod sony;
pub mod switch2;
pub mod switch2_ble;
pub mod xinput;
pub mod winmm;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

pub const PLAYERS: usize = 8;
pub const STICK_MAX: i32 = 32767;

pub fn env_flag(name: &str) -> bool {
    let get = |n: &str| std::env::var(n).ok().is_some_and(|v| !v.is_empty() && v != "0");
    get(name) || name.strip_prefix("NXRS_").is_some_and(|rest| get(&format!("SWR_{rest}")))
}
pub fn env_str(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty()).or_else(|| name.strip_prefix("NXRS_").and_then(|r| std::env::var(format!("SWR_{r}")).ok()).filter(|v| !v.is_empty()))
}

macro_rules! ilog {
    ($($a:tt)*) => { crate::log::line(&format!("controllers: {}", format!($($a)*))) };
}
pub(crate) use ilog;

pub fn pack(a: [i32; 4]) -> u64 {
    a.iter().enumerate().fold(0u64, |v, (i, &x)| v | ((x.clamp(-32768, 32767) as i16 as u16 as u64) << (16 * i)))
}
pub fn unpack(v: u64) -> [i32; 4] {
    std::array::from_fn(|i| (v >> (16 * i)) as u16 as i16 as i32)
}
/// Whichever of two values is deflected more.
pub fn pick(a: i32, b: i32) -> i32 {
    if a.abs() >= b.abs() {
        a
    } else {
        b
    }
}

/// One input path: held buttons, a latch of presses since the game's last read (a press shorter than a frame still
/// reaches it), and the sticks.
pub struct Path {
    btn: AtomicU64,
    latch: AtomicU64,
    axes: AtomicU64,
}

impl Path {
    pub const fn new() -> Path {
        Path { btn: AtomicU64::new(0), latch: AtomicU64::new(0), axes: AtomicU64::new(0) }
    }
    pub fn set(&self, b: u64, a: [i32; 4]) {
        let old = self.btn.swap(b, Ordering::AcqRel);
        self.latch.fetch_or(b & !old, Ordering::AcqRel);
        self.axes.store(pack(a), Ordering::Release);
    }
    /// The held buttons and the presses latched since the last take, and the sticks.
    pub fn take(&self) -> (u64, [i32; 4]) {
        (self.btn.load(Ordering::Acquire) | self.latch.swap(0, Ordering::AcqRel), unpack(self.axes.load(Ordering::Acquire)))
    }
    pub fn peek(&self) -> (u64, [i32; 4]) {
        (self.btn.load(Ordering::Acquire), unpack(self.axes.load(Ordering::Acquire)))
    }
    pub fn clear(&self) {
        self.set(0, [0; 4]);
        self.latch.store(0, Ordering::Release);
    }
}

impl Default for Path {
    fn default() -> Self {
        Path::new()
    }
}

/// Player 1's keyboard and mouse (the front end).
pub static KEYS: Path = Path::new();
/// Scripted presses for player 1 (NXRS_KEYS, the control pipe, a replay).
pub static SCRIPT: Path = Path::new();
/// Scripted presses for the other players (the control pipe's `P<n>`).
pub static SCRIPT_P: [Path; PLAYERS] = [const { Path::new() }; PLAYERS];
/// Each player's physical controller, guest buttons (merged over its devices).
pub static PADS: [Path; PLAYERS] = [const { Path::new() }; PLAYERS];
/// A pad's Joy-Cons one by one: [player][0 left, 1 right], guest buttons with the rail buttons, each its own stick in
/// the first two axes.
pub static JOY: [[Path; 2]; PLAYERS] = [const { [const { Path::new() }, const { Path::new() }] }; PLAYERS];
/// Bit 0 left, bit 1 right: the Joy-Cons a player's pad holds (0: a Pro-like pad).
pub static JOY_SIDES: [AtomicU32; PLAYERS] = [const { AtomicU32::new(0) }; PLAYERS];
/// Bit p: player p has a controller (player 1 always counts: the keyboard).
pub static PLAYERS_MASK: AtomicU32 = AtomicU32::new(1);
/// Scripted players connected by the control pipe.
pub static SCRIPT_PLAYERS: AtomicU32 = AtomicU32::new(0);
/// A replay holds player 1: the keyboard and player 1's pad are not read.
pub static REPLAYING: AtomicBool = AtomicBool::new(false);
static FOCUS: AtomicBool = AtomicBool::new(true);
static SUSPENDED: AtomicBool = AtomicBool::new(false);
/// A host prompt (controller support, Home quit) is open: the game reads no pads.
static PROMPT: AtomicBool = AtomicBool::new(false);
static RESCAN: AtomicBool = AtomicBool::new(true);
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// nn::hid::VibrationValue: amplitude and frequency of the low and high band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vib {
    pub amp_low: f32,
    pub freq_low: f32,
    pub amp_high: f32,
    pub freq_high: f32,
}
impl Default for Vib {
    fn default() -> Self {
        Vib { amp_low: 0.0, freq_low: 160.0, amp_high: 0.0, freq_high: 320.0 }
    }
}
fn clean(v: Vib) -> Vib {
    let a = |x: f32| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 };
    let f = |x: f32, d: f32| if x.is_finite() && x > 0.0 { x } else { d };
    Vib { amp_low: a(v.amp_low), freq_low: f(v.freq_low, 160.0), amp_high: a(v.amp_high), freq_high: f(v.freq_high, 320.0) }
}

struct Rumble {
    v: [[Vib; 2]; PLAYERS],
    dirty: [bool; PLAYERS],
}
static RUMBLE: Mutex<Rumble> = Mutex::new(Rumble { v: [[Vib { amp_low: 0.0, freq_low: 160.0, amp_high: 0.0, freq_high: 320.0 }; 2]; PLAYERS], dirty: [false; PLAYERS] });

/// The player's rumble settings (nxnative docs/settings.md, global): `rumble` 0 = off, `rumble_gain` 0..3 scales the
/// strength, `rumble_min` 0..0.9: levels below it stay silent and the rest is stretched over the full range (for
/// rotating motors a game's quiet HD rumble texture keeps running: Mario Kart 8 Deluxe 0.45).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RumbleSettings {
    pub on: bool,
    pub gain: f32,
    pub min: f32,
}

impl RumbleSettings {
    pub fn from_ini() -> RumbleSettings {
        let f = |k: &str, d: f32| nxconfig::string(k).trim().parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(d);
        RumbleSettings { on: nxconfig::string("rumble").trim() != "0", gain: f("rumble_gain", 1.0).clamp(0.0, 3.0), min: f("rumble_min", 0.0).clamp(0.0, 0.9) }
    }

    /// One amplitude as the pad gets it (nxnative haptics_apple.m `controller_level(x * gain)`).
    pub fn level(&self, x: f32) -> f32 {
        if !self.on {
            return 0.0;
        }
        let x = if x.is_finite() { (x * self.gain).clamp(0.0, 1.0) } else { 0.0 };
        if x <= self.min { 0.0 } else { (x - self.min) / (1.0 - self.min) }
    }

    pub fn apply(&self, v: Vib) -> Vib {
        Vib { amp_low: self.level(v.amp_low), amp_high: self.level(v.amp_high), ..v }
    }
}

/// The game's vibration for player p's device `side` (0 left / the whole pad, 1 right).
pub fn vibrate(p: usize, side: usize, v: Vib) {
    vibrate_with(p, side, v, false);
}

/// The same; `force` sends it even when the pad has that value already (a vibration device initialised idle).
pub fn vibrate_with(p: usize, side: usize, v: Vib, force: bool) {
    if p >= PLAYERS || side > 1 {
        return;
    }
    let v = RumbleSettings::from_ini().apply(clean(v));
    let mut r = RUMBLE.lock().unwrap();
    if r.v[p][side] != v || force {
        r.v[p][side] = v;
        r.dirty[p] = true;
    }
}

/// Motion streams: each player's pad, and each Joy-Con on its own.
pub struct Motion {
    pub pad: [Mutex<motion::Stream>; PLAYERS],
    pub joy: [[Mutex<motion::Stream>; 2]; PLAYERS],
}
pub fn motion() -> &'static Motion {
    static M: OnceLock<Motion> = OnceLock::new();
    M.get_or_init(|| Motion {
        pad: std::array::from_fn(|_| Mutex::new(motion::Stream::default())),
        joy: std::array::from_fn(|_| std::array::from_fn(|_| Mutex::new(motion::Stream::default()))),
    })
}
fn motion_gain() -> f32 {
    static G: OnceLock<f32> = OnceLock::new();
    *G.get_or_init(|| env_str("NXRS_MOTION_GAIN").and_then(|v| v.parse().ok()).unwrap_or(0.5))
}

/// The touch screen as the front end reports it (the mouse's left button over the window, or `touch` from the control
/// pipe), in the console's 1280x720 screen coordinates. A press shorter than `TOUCH_LATCH` is kept that long, so a
/// quick click is a tap the game samples at least a few times (nxnative hle_touch.c's short-press latch).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Touch {
    pub x: u32,
    pub y: u32,
    /// Counts presses: each new press is a new finger (the SDK tracks fingers by id).
    pub finger: u32,
    /// The press started since the last read (TouchAttribute Start).
    pub start: bool,
    /// The press ended (TouchAttribute End): the last sample of this finger.
    pub end: bool,
}

const TOUCH_LATCH: std::time::Duration = std::time::Duration::from_millis(60);
/// Fingers the host keeps (nxnative's gesture reads at most four).
pub const FINGERS: usize = 4;

#[derive(Clone, Copy)]
struct TouchSt {
    down: bool,
    x: u32,
    y: u32,
    finger: u32,
    since: Option<std::time::Instant>,
    started: bool,
    released_at: Option<std::time::Instant>,
}
const UP: TouchSt = TouchSt { down: false, x: 0, y: 0, finger: 0, since: None, started: false, released_at: None };
/// The fingers by slot (slot 0: the mouse or `touch`; more for `pinch` / `rotate` and the two-finger mouse) and the
/// last finger id handed out.
static TOUCH: Mutex<([TouchSt; FINGERS], u32)> = Mutex::new(([UP; FINGERS], 0));

/// The front end: the touch screen pressed at (x, y) (1280x720 space), or released.
pub fn set_touch(down: bool, x: u32, y: u32) {
    set_finger(0, down, x, y);
}

/// Finger `slot` (0..FINGERS) pressed at (x, y), moved there, or released.
pub fn set_finger(slot: usize, down: bool, x: u32, y: u32) {
    if slot >= FINGERS {
        return;
    }
    let mut g = TOUCH.lock().unwrap();
    let now = std::time::Instant::now();
    let fresh = down && (!g.0[slot].down || g.0[slot].released_at.is_some());
    if fresh {
        g.1 = g.1.wrapping_add(1);
    }
    let id = g.1;
    let t = &mut g.0[slot];
    if down {
        if fresh {
            t.finger = id;
            t.since = Some(now);
            t.started = false;
            crate::log_once!("touch first", "input: touch screen from the mouse / control pipe (first press at {x}, {y})");
        }
        t.down = true;
        t.released_at = None;
        t.x = x.min(1279);
        t.y = y.min(719);
    } else if t.down && t.released_at.is_none() {
        t.released_at = Some(now);
    }
}

/// Every finger down without taking a sample (the gesture recognizer's view): down, or released but still inside its
/// latch, in slot order.
pub fn touches_peek() -> Vec<(u32, u32)> {
    TOUCH.lock().unwrap().0.iter().filter(|t| t.down).map(|t| (t.x, t.y)).collect()
}

/// The first finger, as `touches_peek`.
pub fn touch_peek() -> Option<(u32, u32)> {
    touches_peek().first().copied()
}

/// The touches the game reads now, in slot order (each finger's Start on its first read, End on its last).
pub fn touches_now() -> Vec<Touch> {
    let mut g = TOUCH.lock().unwrap();
    let now = std::time::Instant::now();
    let mut v = vec![];
    for t in g.0.iter_mut().filter(|t| t.down) {
        let held_long_enough = t.since.is_some_and(|s| now.duration_since(s) >= TOUCH_LATCH);
        let end = t.released_at.is_some() && held_long_enough;
        let start = !t.started;
        t.started = true;
        if end {
            t.down = false;
        }
        v.push(Touch { x: t.x, y: t.y, finger: t.finger, start, end });
    }
    v
}

/// The first touch the game reads now (None: nothing on the screen).
pub fn touch_now() -> Option<Touch> {
    touches_now().first().copied()
}

/// The two-finger mouse: Ctrl + left drag puts a second finger mirrored about where the button went down (moving
/// away or toward it pinches, around it rotates); Shift + left drag keeps two fingers `ROTATE_R` px from that point,
/// turned by the mouse's angle around it (a rotation only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseTouch {
    One,
    Pinch,
    Rotate,
}
pub const ROTATE_R: f64 = 100.0;

/// The fingers for the mouse at `m`, the button having gone down at `anchor` (1280x720 space).
pub fn mouse_fingers(mode: MouseTouch, anchor: (i32, i32), m: (i32, i32)) -> Vec<(u32, u32)> {
    let c = |v: f64, hi: f64| v.round().clamp(0.0, hi) as u32;
    let (ax, ay) = (anchor.0 as f64, anchor.1 as f64);
    let (mx, my) = (m.0 as f64, m.1 as f64);
    match mode {
        MouseTouch::One => vec![(c(mx, 1279.0), c(my, 719.0))],
        MouseTouch::Pinch => vec![(c(mx, 1279.0), c(my, 719.0)), (c(2.0 * ax - mx, 1279.0), c(2.0 * ay - my, 719.0))],
        MouseTouch::Rotate => {
            let a = (my - ay).atan2(mx - ax);
            let (dx, dy) = (ROTATE_R * a.cos(), ROTATE_R * a.sin());
            vec![(c(ax + dx, 1279.0), c(ay + dy, 719.0)), (c(ax - dx, 1279.0), c(ay - dy, 719.0))]
        }
    }
}

/// Two fingers `r` px either side of (cx, cy) at `deg` degrees (the control pipe's `pinch` and `rotate`).
pub fn two_fingers(cx: f64, cy: f64, r: f64, deg: f64) -> [(u32, u32); 2] {
    let a = deg.to_radians();
    let (dx, dy) = (r * a.cos(), r * a.sin());
    let c = |v: f64, hi: f64| v.round().clamp(0.0, hi) as u32;
    [(c(cx + dx, 1279.0), c(cy + dy, 719.0)), (c(cx - dx, 1279.0), c(cy - dy, 719.0))]
}

/// The front end: player 1's keyboard and mouse state (guest buttons, sticks).
pub fn set_keyboard(buttons: u64, axes: [i32; 4]) {
    KEYS.set(buttons, axes);
}
/// The front end: whether the game's window is in front. Out of focus, pads read nothing and rumble stops (unless
/// NXRS_CONTROLLER_BACKGROUND=1).
pub fn set_focus(focused: bool) {
    FOCUS.store(focused, Ordering::Release);
}
/// The front end's F1 page is open: the game reads no pads.
pub fn set_suspended(on: bool) {
    SUSPENDED.store(on, Ordering::Release);
}
/// A host prompt opens or closes over the game (its pads are the prompt's meanwhile).
pub fn set_prompt(on: bool) {
    PROMPT.store(on, Ordering::Release);
}
/// A device arrived or left (WM_DEVICECHANGE): rescan now.
pub fn device_changed() {
    RESCAN.store(true, Ordering::Release);
}

fn active() -> bool {
    static BG: OnceLock<bool> = OnceLock::new();
    !SUSPENDED.load(Ordering::Acquire) && !PROMPT.load(Ordering::Acquire) && (FOCUS.load(Ordering::Acquire) || *BG.get_or_init(|| env_flag("NXRS_CONTROLLER_BACKGROUND")))
}

fn trace() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| env_flag("NXRS_CONTROLLER_TRACE"))
}

// ---------------------------------------------------------------- devices

enum Kind {
    X(u32),
    Sony(Box<SonyDev>),
    Nin(Box<NinDev>),
    Mm(winmm::Joy),
    S2(std::sync::Arc<switch2_ble::S2Dev>, u8),
}

struct SonyDev {
    hid: hidio::HidDev,
    model: u8,
    transport: u8,
    v2: bool,
    seq: u32,
    state: sony::SonyState,
    last_input: f64,
    failed: bool,
    writes: u32,
    errors: u32,
    leds: u8,
}

struct NinDev {
    hid: hidio::HidDev,
    side: u8,
    sticks: nintendo::Sticks,
    counter: u8,
    step: u32,
    next_step: f64,
    last_input: f64,
    rep: nintendo::Report,
    rumble: [u8; 8],
    usb: bool,
    leds: u8,
}

struct Dev {
    kind: Kind,
    key: String,
    name: String,
    /// The player slot, or None while it waits for one.
    slot: Option<usize>,
    /// For a Joy-Con: its side (0 left, 1 right).
    joy: Option<usize>,
    /// Positional buttons (before the layout), sticks.
    buttons: u64,
    axes: [i32; 4],
    home: bool,
    lights: Option<usize>,
    rumble_last: f64,
    rumble_sent: (u32, u32),
    selftest: Option<f64>,
    gone: bool,
}

impl Dev {
    fn rumble_capable(&self) -> bool {
        match &self.kind {
            Kind::X(_) => true,
            Kind::Sony(s) => s.hid.writable,
            Kind::Nin(n) => n.hid.writable,
            Kind::Mm(_) => false,
            Kind::S2(..) => true,
        }
    }

    fn poll(&mut self, now: f64) {
        match &mut self.kind {
            Kind::X(i) => match xinput::state(*i) {
                Some(g) => {
                    self.buttons = xinput::buttons(&g);
                    self.axes = [xinput::dz(g.lx as i32), xinput::dz(g.ly as i32), xinput::dz(g.rx as i32), xinput::dz(g.ry as i32)];
                    self.home = g.buttons & 0x0400 != 0;
                }
                None => self.gone = true,
            },
            Kind::Sony(s) => {
                for _ in 0..64 {
                    match s.hid.read() {
                        hidio::Read::Report(r) => {
                            let mut st = s.state;
                            if sony::input(s.model, s.transport, &r, &mut st) {
                                s.state = st;
                                s.last_input = now;
                                if st.imu {
                                    let (acc, rad) = sony::to_switch(&st);
                                    if let Some(p) = self.slot {
                                        motion().pad[p].lock().unwrap().add(now, acc, rad, motion_gain());
                                    }
                                }
                            } else if s.errors < 8 {
                                s.errors += 1;
                                ilog!("Sony input rejected id={:02x} bytes={} (length/report/CRC)", r.first().copied().unwrap_or(0), r.len());
                            }
                        }
                        hidio::Read::Empty => break,
                        hidio::Read::Gone(e) => {
                            ilog!("{} disconnected (error {e})", self.name);
                            self.gone = true;
                            return;
                        }
                    }
                }
                if now - s.last_input > 1.0 {
                    ilog!("{}: no input for a second; released", self.name);
                    self.gone = true;
                    return;
                }
                self.buttons = s.state.buttons;
                self.axes = [s.state.lx, s.state.ly, s.state.rx, s.state.ry];
                self.home = s.state.home;
            }
            Kind::Nin(n) => {
                nin_handshake(n, now);
                for _ in 0..64 {
                    match n.hid.read() {
                        hidio::Read::Report(r) => {
                            if r.first() == Some(&0x21) && n.sticks.spi_reply(&r) != 0 {
                                ilog!("{}: stick calibration read ({})", self.name, if n.sticks.cal.contains(&2) { "user" } else { "factory" });
                            }
                            if let Some(rep) = nintendo::decode(&mut n.sticks, &r, n.side) {
                                n.rep = rep;
                                n.last_input = now;
                                if rep.has_imu {
                                    let side = if n.side == 3 { 3 } else { 1 };
                                    for (k, s) in rep.imu.iter().enumerate() {
                                        let (acc, rad) = nintendo::imu_switch(s, side);
                                        let t = now - 0.005 * (2 - k) as f64;
                                        if let Some(p) = self.slot {
                                            let st = match self.joy {
                                                Some(j) => &motion().joy[p][j],
                                                None => &motion().pad[p],
                                            };
                                            st.lock().unwrap().add(t, acc, rad, motion_gain());
                                        }
                                    }
                                }
                            } else if r.first() == Some(&0x3f) {
                                n.last_input = now; // the simple report until the full mode answers
                            }
                        }
                        hidio::Read::Empty => break,
                        hidio::Read::Gone(e) => {
                            ilog!("{} disconnected (error {e})", self.name);
                            self.gone = true;
                            return;
                        }
                    }
                }
                if now - n.last_input > 2.0 {
                    ilog!("{}: no input for 2 s; released", self.name);
                    self.gone = true;
                    return;
                }
                self.buttons = n.rep.buttons;
                self.axes = n.rep.sticks;
                self.home = n.rep.home;
            }
            Kind::Mm(j) => match winmm::poll(j) {
                Some((b, a)) => {
                    self.buttons = b;
                    self.axes = a;
                }
                None => self.gone = true,
            },
            Kind::S2(d, _) => {
                if d.gone.load(Ordering::Acquire) {
                    self.gone = true;
                    return;
                }
                d.slot.store(self.slot.map(|p| p as i32).unwrap_or(-1), Ordering::Release);
                if let Some((b, a, home)) = d.state() {
                    self.buttons = b;
                    self.axes = a;
                    self.home = home;
                }
            }
        }
    }

    /// Lights for player p (None: off).
    fn set_lights(&mut self, p: Option<usize>) {
        if self.lights == p {
            return;
        }
        self.lights = p;
        let (low, high) = self.rumble_sent;
        match &mut self.kind {
            Kind::Sony(s) => {
                s.leds = p.map(sony_leds).unwrap_or(0);
                sony_write(s, &self.name, low, high, false);
            }
            Kind::S2(d, _) => d.lights(p),
            Kind::Nin(n) => {
                n.leds = p.map(nintendo::lights).unwrap_or(0);
                let c = n.counter;
                n.counter = n.counter.wrapping_add(1);
                let pkt = nintendo::subcommand(c, &nintendo::NEUTRAL_RUMBLE, 0x30, &[n.leds]);
                let _ = n.hid.write(&pkt);
            }
            _ => {}
        }
    }

    /// The pad's motors from the game's values (low, high in 0..255 for single-motor-pair pads; HD rumble per side).
    fn rumble(&mut self, v: [Vib; 2], now: f64) {
        let low = (v[0].amp_low.max(v[1].amp_low) * 255.0 + 0.5) as u32;
        let high = (v[0].amp_high.max(v[1].amp_high) * 255.0 + 0.5) as u32;
        let zero = low == 0 && high == 0;
        if !zero && now - self.rumble_last < 0.015 {
            return; // nonzero updates coalesce to about 60 Hz; a stop always goes out
        }
        self.rumble_last = now;
        let t = trace();
        match &mut self.kind {
            Kind::X(i) => {
                let r = xinput::rumble(*i, (low * 257) as u16, (high * 257) as u16);
                if r != 0 || t {
                    ilog!("XInput output user={i} low={low} high={high} status={r}");
                }
            }
            Kind::Sony(s) => sony_write(s, &self.name, low, high, false),
            Kind::Nin(n) => {
                let side = |x: &Vib| nintendo::encode_side(x.amp_low, x.freq_low, x.amp_high, x.freq_high);
                let (l, r) = match n.side {
                    2 => (side(&v[0]), [0x00, 0x01, 0x40, 0x40]),
                    3 => ([0x00, 0x01, 0x40, 0x40], side(&v[1])),
                    _ => (side(&v[0]), side(&v[1])),
                };
                n.rumble[..4].copy_from_slice(&l);
                n.rumble[4..].copy_from_slice(&r);
                let c = n.counter;
                n.counter = n.counter.wrapping_add(1);
                let e = n.hid.write(&nintendo::rumble_report(c, &n.rumble));
                if e != 0 || t {
                    ilog!("{} rumble {:02x?} error={e}", self.name, n.rumble);
                }
            }
            Kind::Mm(_) => {}
            Kind::S2(d, seq) => {
                *seq = seq.wrapping_add(1);
                // a Pro Controller 2's two grips take the left and right values, a Joy-Con 2 its own in both blocks
                let (a, b) = match d.joy() {
                    Some(0) => (v[0], v[0]),
                    Some(_) => (v[1], v[1]),
                    None => (v[0], v[1]),
                };
                d.rumble(&a, &b, *seq);
                if t {
                    ilog!("{} rumble low {low} high {high}", self.name);
                }
            }
        }
        self.rumble_sent = (low, high);
    }

    /// Rumble off and lights off, bounded (at exit, on focus loss, before closing).
    fn quiet(&mut self) {
        self.rumble([Vib::default(); 2], f64::MAX);
        self.set_lights(None);
    }
}

fn sony_leds(p: usize) -> u8 {
    // the DualSense's five player LEDs (hid-playstation's patterns); player 1 is the centre one
    [0x04, 0x0a, 0x15, 0x1b, 0x1f, 0x1f, 0x1f, 0x1f][p.min(7)]
}

fn sony_write(s: &mut SonyDev, name: &str, low: u32, high: u32, reset: bool) {
    if !s.hid.writable {
        return;
    }
    if s.failed && (low != 0 || high != 0) {
        return;
    }
    let mut pkt = [0u8; 78];
    let mut n = sony::output(s.model, s.transport, s.seq, low, high, s.leds != 0, reset, s.v2, &mut pkt);
    s.seq = s.seq.wrapping_add(1);
    if s.model == sony::DS5 && !reset && s.leds != 0 {
        let o = if s.transport == sony::BT { 3 } else { 1 };
        pkt[o + 43] = s.leds;
        if s.transport == sony::BT {
            let c = sony::crc(0xa2, &pkt[..74]);
            pkt[74..78].copy_from_slice(&c.to_le_bytes());
        }
    }
    if n == 48 && s.hid.info.output_len == 63 {
        n = 63;
    }
    if n == 0 {
        return;
    }
    let e = s.hid.write(&pkt[..n]);
    if e != 0 {
        s.failed = true;
        if s.errors < 8 {
            s.errors += 1;
            ilog!("{name}: output FAILED id={:02x} bytes={n} error={e}", pkt[0]);
        }
    } else if s.writes < 8 || trace() {
        s.writes += 1;
        ilog!("{name}: output sent id={:02x} bytes={n} low={low} high={high} leds={:#04x} reset={reset}", pkt[0], s.leds);
        if trace() {
            ilog!("{name}: report {}", pkt[..n].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
        }
    }
}

/// The Nintendo handshake, one step per call while due: USB (Pro Controller) start, full report mode, IMU, vibration,
/// the sticks' calibration, the player lights.
fn nin_handshake(n: &mut NinDev, now: f64) {
    if n.step >= 9 || now < n.next_step {
        return;
    }
    let neutral = nintendo::NEUTRAL_RUMBLE;
    let c = n.counter;
    let pkt: Vec<u8> = match n.step {
        0 if n.usb => vec![0x80, 0x02],
        1 if n.usb => vec![0x80, 0x03],
        2 if n.usb => vec![0x80, 0x02],
        3 if n.usb => vec![0x80, 0x04],
        0..=3 => {
            n.step = 4;
            return nin_handshake(n, now);
        }
        4 => nintendo::subcommand(c, &neutral, 0x03, &[0x30]),
        5 => nintendo::subcommand(c, &neutral, 0x40, &[0x01]),
        6 => nintendo::subcommand(c, &neutral, 0x48, &[0x01]),
        7 => {
            let a = nintendo::SPI_FACTORY.to_le_bytes();
            nintendo::subcommand(c, &neutral, 0x10, &[a[0], a[1], a[2], a[3], 18])
        }
        _ => {
            let a = nintendo::SPI_USER.to_le_bytes();
            nintendo::subcommand(c, &neutral, 0x10, &[a[0], a[1], a[2], a[3], 22])
        }
    };
    if n.step >= 4 {
        n.counter = n.counter.wrapping_add(1);
    }
    let e = n.hid.write(&pkt);
    if e != 0 && trace() {
        ilog!("Nintendo handshake step {} error {e}", n.step);
    }
    n.step += 1;
    n.next_step = now + 0.05;
}

// ---------------------------------------------------------------- slots

/// nxnative's pad slot rule: a reconnecting pad gets its player back, player 1 first, then a slot never held, then
/// the lowest free one.
#[derive(Default)]
pub struct Slots {
    pub owner: [Option<String>; PLAYERS],
    pub last: [String; PLAYERS],
}

impl Slots {
    pub fn of(&self, key: &str) -> Option<usize> {
        self.owner.iter().position(|o| o.as_deref() == Some(key))
    }
    pub fn take(&mut self, key: &str, name: &str) -> Option<usize> {
        if let Some(p) = self.of(key) {
            return Some(p);
        }
        let p = (0..PLAYERS)
            .find(|&i| self.owner[i].is_none() && !name.is_empty() && self.last[i] == name)
            .or_else(|| self.owner[0].is_none().then_some(0))
            .or_else(|| (0..PLAYERS).find(|&i| self.owner[i].is_none() && self.last[i].is_empty()))
            .or_else(|| (0..PLAYERS).find(|&i| self.owner[i].is_none()))?;
        self.owner[p] = Some(key.to_string());
        self.last[p] = name.to_string();
        Some(p)
    }
    /// The slot a new pad takes under the player's pad settings (nxnative pad_slots.h / docs/settings.md):
    /// `merge` every pad is player 1 (owning nothing); `lowest` (press to join) the lowest free player whatever the
    /// pad; `first` the first player a pad may take (1 when the keyboard is player 1 on its own).
    pub fn take_with(&mut self, key: &str, name: &str, merge: bool, lowest: bool, first: usize) -> Option<usize> {
        if merge {
            return Some(0);
        }
        if let Some(p) = self.of(key).filter(|&p| p >= first) {
            return Some(p);
        }
        let free = |s: &Self, i: usize| s.owner[i].is_none();
        let p = if lowest {
            (first..PLAYERS).find(|&i| free(self, i))?
        } else {
            (first..PLAYERS)
                .find(|&i| free(self, i) && !name.is_empty() && self.last[i] == name)
                .or_else(|| (first < PLAYERS && free(self, first)).then_some(first))
                .or_else(|| (first..PLAYERS).find(|&i| free(self, i) && self.last[i].is_empty()))
                .or_else(|| (first..PLAYERS).find(|&i| free(self, i)))?
        };
        self.owner[p] = Some(key.to_string());
        self.last[p] = name.to_string();
        Some(p)
    }
    pub fn free(&mut self, key: &str) -> Option<usize> {
        let p = self.of(key)?;
        self.owner[p] = None;
        Some(p)
    }
}

// ---------------------------------------------------------------- the input thread

struct Inputs {
    devs: Vec<Dev>,
    slots: Slots,
    map: layout::PadMap,
    /// Nintendo pads: by their printed labels whatever `button_layout` says (`layout::for_nintendo`).
    map_nin: layout::PadMap,
    map_desc: String,
    map_gen: u64,
    last_scan: f64,
    active: bool,
    record: Option<script::Recorder>,
    replay: Option<script::Replay>,
}

fn device_name(i: &hidio::HidInfo) -> String {
    let n = if i.product.is_empty() { "HID pad".to_string() } else { i.product.clone() };
    format!("{n} {:04x}:{:04x} {}", i.vid, i.pid, if i.bluetooth { "Bluetooth" } else { "USB" })
}

impl Inputs {
    fn has(&self, key: &str) -> bool {
        self.devs.iter().any(|d| d.key == key)
    }

    fn scan(&mut self, now: f64) {
        self.last_scan = now;
        // XInput users come and go every tick (cheap); HID and WinMM on the scan
        let hid = hidio::enumerate();
        let (no_sony, no_nin) = (env_flag("NXRS_NO_SONY_HID"), env_flag("NXRS_NO_NINTENDO_HID"));
        for info in hid.iter().filter(|i| i.is_gamepad() || nintendo::kind(i.vid, i.pid) != 0) {
            let key = info.path_string();
            if self.has(&key) {
                continue;
            }
            let model = sony::model(info.vid, info.pid);
            let nk = nintendo::kind(info.vid, info.pid);
            if model != 0 && !no_sony {
                let tr = sony::transport(model, info.input_len, info.output_len);
                if tr == 0 {
                    continue;
                }
                let Some(hid) = hidio::HidDev::open(info, true) else {
                    ilog!("{}: cannot open", device_name(info));
                    continue;
                };
                let mut firmware = 0;
                let mut v2 = false;
                if model == sony::DS5 {
                    let f = hid.feature(0x20, (info.feature_len as usize).clamp(46, 80));
                    v2 = sony::vibration_v2(info.pid, f.as_deref());
                    firmware = f.as_ref().map(|f| u16::from_le_bytes([f[44], f[45]])).unwrap_or(0);
                }
                let name = format!("{}{} {:04x}:{:04x} {}", if model == sony::DS5 { "DualSense" } else { "DualShock 4" }, if info.pid == 0x0df2 { " Edge" } else { "" }, info.vid, info.pid, if tr == sony::USB { "USB" } else { "Bluetooth" });
                ilog!(
                    "{name} backend=native-HID input={} output={} output-access={} firmware-update={firmware:04x} rumble={}",
                    info.input_len,
                    info.output_len,
                    if hid.writable { "read+write (lights+rumble)".to_string() } else { format!("read-only, open error {} (input only, no lights or rumble)", hid.open_error) },
                    if model == sony::DS5 { if v2 { "compatible-v2" } else { "compatible-v1" } } else { "classic" }
                );
                let mut s = SonyDev { hid, model, transport: tr, v2, seq: 0, state: Default::default(), last_input: now, failed: false, writes: 0, errors: 0, leds: 0 };
                if model == sony::DS5 {
                    sony_write(&mut s, &name, 0, 0, true); // release the firmware's lightbar animation
                }
                self.devs.push(Dev::new(Kind::Sony(Box::new(s)), key, name, None));
            } else if nk != 0 && !no_nin {
                let Some(hid) = hidio::HidDev::open(info, true) else {
                    ilog!("{}: cannot open", device_name(info));
                    continue;
                };
                let name = format!("{} {:04x}:{:04x} {}", match nk { 2 => "Joy-Con (L)", 3 => "Joy-Con (R)", _ => if info.pid == nintendo::PRO { "Pro Controller" } else { "Nintendo Online controller" } }, info.vid, info.pid, if info.bluetooth { "Bluetooth" } else { "USB" });
                ilog!("{name} backend=native-HID input={} output={} output-access={}", info.input_len, info.output_len, if hid.writable { "read+write (lights+HD rumble)" } else { "read-only" });
                let usb = !info.bluetooth && info.pid == nintendo::PRO;
                let n = NinDev { hid, side: nk, sticks: Default::default(), counter: 0, step: 0, next_step: now, last_input: now, rep: Default::default(), rumble: nintendo::NEUTRAL_RUMBLE, usb, leds: 0 };
                let joy = match nk {
                    2 => Some(0),
                    3 => Some(1),
                    _ => None,
                };
                self.devs.push(Dev::new(Kind::Nin(Box::new(n)), key, name, joy));
            }
        }
        for d in switch2_ble::DEVICES.lock().unwrap().iter() {
            let key = format!("s2:{:012x}", d.addr);
            if !self.has(&key) {
                let joy = d.joy();
                self.devs.push(Dev::new(Kind::S2(d.clone(), 0), key, d.name(), joy));
            }
        }
        // WinMM only when nothing else is there (it lists XInput and Sony pads as joysticks too)
        let others = self.devs.iter().any(|d| !matches!(d.kind, Kind::Mm(_)));
        if !others && !self.devs.iter().any(|d| matches!(d.kind, Kind::Mm(_))) {
            if let Some(j) = winmm::open(0) {
                let name = format!("joystick {:04x}:{:04x} ({} axes, {} buttons)", j.vid, j.pid, j.axes, j.buttons);
                ilog!("{name} backend=WinMM (no XInput or native-HID pad)");
                self.devs.push(Dev::new(Kind::Mm(j.clone()), format!("winmm:{}", j.id), name, None));
            }
        }
    }

    fn xinput(&mut self) {
        for i in 0..4 {
            let key = format!("xinput:{i}");
            if self.has(&key) {
                continue;
            }
            if xinput::state(i).is_some() {
                // a WinMM stand-in for the same pad goes
                self.devs.retain(|d| !matches!(d.kind, Kind::Mm(_)));
                let name = format!("XInput pad {}", i + 1);
                ilog!("{name} connected (backend=XInput {})", xinput::dll().unwrap_or("?"));
                self.devs.push(Dev::new(Kind::X(i), key, name, None));
            }
        }
    }

    /// Slots for new devices: a Joy-Con joins a lone Joy-Con of the other side, else everything takes a slot.
    fn assign(&mut self) {
        for i in 0..self.devs.len() {
            if self.devs[i].slot.is_some() {
                continue;
            }
            let mut slot = None;
            if let Some(side) = self.devs[i].joy {
                for p in 0..PLAYERS {
                    let holders: Vec<&Dev> = self.devs.iter().filter(|d| d.slot == Some(p)).collect();
                    if holders.len() == 1 && holders[0].joy == Some(1 - side) {
                        slot = Some(p);
                        break;
                    }
                }
            }
            if slot.is_none() {
                // the player's pad settings: pad_merge, pad_press_to_join (a pad is no player until one of its
                // buttons is pressed), keyboard_player own (the keyboard alone is player 1)
                let (merge, join) = (nxconfig::flag("pad_merge"), nxconfig::flag("pad_press_to_join") && !nxconfig::flag("pad_merge"));
                if join && self.devs[i].buttons == 0 {
                    continue;
                }
                let first = if nxconfig::string("keyboard_player").trim() == "own" && !merge { 1 } else { 0 };
                let (key, name) = (self.devs[i].key.clone(), self.devs[i].name.clone());
                slot = self.slots.take_with(&key, &name, merge, join, first);
            }
            if let Some(p) = slot {
                let d = &mut self.devs[i];
                d.slot = Some(p);
                ilog!("{} is player {}", d.name, p + 1);
                if env_flag("NXRS_CONTROLLER_SELFTEST") && d.rumble_capable() {
                    d.selftest = Some(0.0);
                }
            }
        }
    }

    fn drop_gone(&mut self) {
        let mut i = 0;
        while i < self.devs.len() {
            if self.devs[i].gone {
                let mut d = self.devs.remove(i);
                d.quiet();
                if let Some(p) = d.slot {
                    // a Joy-Con pair's slot stays with the other half
                    if !self.devs.iter().any(|o| o.slot == Some(p)) {
                        self.slots.free(&d.key);
                        PADS[p].clear();
                        motion().pad[p].lock().unwrap().last = 0.0;
                    } else if self.slots.of(&d.key).is_some() {
                        // hand the slot's ownership to the remaining half
                        let other = self.devs.iter().find(|o| o.slot == Some(p)).map(|o| o.key.clone()).unwrap();
                        self.slots.owner[p] = Some(other);
                    }
                    if let Some(s) = d.joy {
                        JOY[p][s].clear();
                    }
                    ilog!("{} (player {}) disconnected", d.name, p + 1);
                }
            } else {
                i += 1;
            }
        }
    }

    fn layout_update(&mut self) {
        let g = nxconfig::generation();
        if g == self.map_gen {
            return;
        }
        self.map_gen = g;
        let (m, bad, err) = layout::build(&nxconfig::string("button_layout"), &nxconfig::string("button_map"));
        if bad > 0 {
            ilog!("{err}; {bad} setting(s) ignored");
        }
        self.map = m;
        self.map_nin = layout::for_nintendo(&nxconfig::string("button_map"));
        let d = layout::describe(&m);
        if d != self.map_desc {
            ilog!("button layout {d}");
            self.map_desc = d;
        }
    }

    fn publish(&mut self, now: f64) {
        let act = active();
        if act != self.active {
            self.active = act;
            ilog!("input {} (focus={} suspended={})", if act { "enabled" } else { "suppressed" }, FOCUS.load(Ordering::Relaxed), SUSPENDED.load(Ordering::Relaxed));
        }
        let mut mask = 1u32 | SCRIPT_PLAYERS.load(Ordering::Relaxed);
        let mut sides = [0u32; PLAYERS];
        let mut merged = [(0u64, [0i32; 4]); PLAYERS];
        let mut joys = [[(0u64, [0i32; 4]); 2]; PLAYERS];
        for d in &self.devs {
            let Some(p) = d.slot else { continue };
            mask |= 1 << p;
            // a Nintendo pad by its printed labels, any other by button_layout
            let map = if matches!(d.kind, Kind::Nin(_) | Kind::S2(..)) { &self.map_nin } else { &self.map };
            let (b, a) = if act { (layout::apply(map, d.buttons), d.axes) } else { (0, [0; 4]) };
            let m = &mut merged[p];
            m.0 |= b & 0xffff;
            for k in 0..4 {
                m.1[k] = pick(m.1[k], a[k]);
            }
            if let Some(s) = d.joy {
                sides[p] |= 1 << s;
                let st = if s == 0 { [a[0], a[1], 0, 0] } else { [a[2], a[3], 0, 0] };
                joys[p][s] = (b, st);
            }
        }
        for p in 0..PLAYERS {
            let (b, a) = merged[p];
            let (ob, oa) = PADS[p].peek();
            if ob != b || oa != a {
                PADS[p].set(b, a);
                if trace() && p < 4 {
                    ilog!("player {} buttons={b:#06x} sticks={a:?}", p + 1);
                }
            }
            for s in 0..2 {
                if sides[p] >> s & 1 != 0 {
                    JOY[p][s].set(joys[p][s].0, joys[p][s].1);
                }
            }
            if JOY_SIDES[p].swap(sides[p], Ordering::AcqRel) != sides[p] {
                crate::services::hid::styles_changed();
            }
        }
        if PLAYERS_MASK.swap(mask, Ordering::AcqRel) != mask {
            crate::services::hid::players_changed(mask);
        }
        // lights: a pad shows its player while the game owns input
        for d in self.devs.iter_mut() {
            let want = if act { d.slot } else { None };
            d.set_lights(want);
        }
        // rumble
        let (vals, dirty) = {
            let mut r = RUMBLE.lock().unwrap();
            let v = r.v;
            let d = r.dirty;
            r.dirty = [false; PLAYERS];
            (v, d)
        };
        for d in self.devs.iter_mut() {
            let Some(p) = d.slot else { continue };
            if let Some(t0) = d.selftest {
                let t0 = if t0 == 0.0 {
                    ilog!("{}: self-test: lights on, low motor 0.5 s, high motor 0.5 s, stop", d.name);
                    d.selftest = Some(now);
                    now
                } else {
                    t0
                };
                let t = now - t0;
                let v = if t < 0.5 {
                    Vib { amp_low: 0.5, ..Default::default() }
                } else if t < 1.0 {
                    Vib { amp_high: 0.5, ..Default::default() }
                } else {
                    d.selftest = None;
                    ilog!("{}: self-test finished", d.name);
                    Vib::default()
                };
                d.rumble([v, v], now);
                continue;
            }
            if !act {
                if d.rumble_sent != (0, 0) {
                    d.rumble([Vib::default(); 2], now);
                }
                continue;
            }
            let mut v = vals[p];
            if let Some(s) = d.joy {
                // a Joy-Con takes its own side's value
                v = if s == 0 { [vals[p][0], Vib::default()] } else { [Vib::default(), vals[p][1]] };
            }
            let want_zero = v.iter().all(|x| x.amp_low == 0.0 && x.amp_high == 0.0);
            if dirty[p] || (!want_zero && now - d.rumble_last > 0.5) || (want_zero && d.rumble_sent != (0, 0)) {
                d.rumble(v, now);
            }
        }
    }
}

impl Dev {
    fn new(kind: Kind, key: String, name: String, joy: Option<usize>) -> Dev {
        Dev { kind, key, name, slot: None, joy, buttons: 0, axes: [0; 4], home: false, lights: None, rumble_last: 0.0, rumble_sent: (0, 0), selftest: None, gone: false }
    }
}

static INPUTS: Mutex<Option<Inputs>> = Mutex::new(None);

fn tick(inp: &mut Inputs) {
    let now = motion::now_s();
    inp.layout_update();
    inp.xinput();
    if RESCAN.swap(false, Ordering::AcqRel) || now - inp.last_scan > 2.0 {
        inp.scan(now);
    }
    for d in inp.devs.iter_mut() {
        d.poll(now);
    }
    inp.drop_gone();
    inp.assign();
    inp.publish(now);
    if let Some(r) = inp.record.as_mut() {
        let (kb, ka) = KEYS.peek();
        let (pb, pa) = PADS[0].peek();
        let a: [i32; 4] = std::array::from_fn(|i| pick(ka[i], pa[i]));
        r.note(now, kb | pb, a);
    }
    if let Some(r) = inp.replay.as_mut() {
        r.tick(now);
    }
}

/// Starts the input thread (once). The front end calls it at start; a run without a front end gets it from the hid
/// service's first use.
pub fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    *INPUTS.lock().unwrap() = Some(Inputs {
        devs: vec![],
        slots: Slots::default(),
        map: Default::default(),
        map_nin: Default::default(),
        map_desc: String::new(),
        map_gen: 0,
        last_scan: -10.0,
        active: true,
        record: env_str("NXRS_INPUT_RECORD").and_then(|p| script::Recorder::open(&p)),
        replay: env_str("NXRS_INPUT_REPLAY").map(script::Replay::new),
    });
    script::start_keys();
    switch2_ble::start();
    std::thread::Builder::new()
        .name("input".into())
        .spawn(|| {
            let period = std::time::Duration::from_millis(4);
            while !SHUTDOWN.load(Ordering::Acquire) {
                let t = std::time::Instant::now();
                if let Some(inp) = INPUTS.lock().unwrap().as_mut() {
                    tick(inp);
                }
                if let Some(rest) = period.checked_sub(t.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        })
        .expect("input thread");
}

/// Rumble off, lights off and the devices closed; waits at most ~200 ms for the input lock, so an exit never hangs
/// on it (Turbo's `backend_controller_shutdown`).
pub fn shutdown() {
    SHUTDOWN.store(true, Ordering::Release);
    for _ in 0..200 {
        if let Ok(mut g) = INPUTS.try_lock() {
            if let Some(inp) = g.as_mut() {
                for d in inp.devs.iter_mut() {
                    d.quiet();
                }
                inp.devs.clear();
                if let Some(r) = inp.record.as_mut() {
                    r.flush();
                }
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// The devices now: (name, player, buttons, sticks) for logs, the probe and the control pipe.
pub fn devices() -> Vec<(String, Option<usize>, u64, [i32; 4])> {
    INPUTS.lock().unwrap().as_ref().map(|i| i.devs.iter().map(|d| (d.name.clone(), d.slot, d.buttons, d.axes)).collect()).unwrap_or_default()
}

/// One synchronous pass (the probe, without the thread): scan, read, assign, publish.
pub fn probe_tick() {
    let mut g = INPUTS.lock().unwrap();
    let inp = g.get_or_insert_with(|| Inputs {
        devs: vec![],
        slots: Slots::default(),
        map: Default::default(),
        map_nin: Default::default(),
        map_desc: String::new(),
        map_gen: 0,
        last_scan: -10.0,
        active: true,
        record: None,
        replay: None,
    });
    tick(inp);
}

/// Player 1's view (keyboard, pad, script ORed), for the front end's own pages (F1) and applets.
pub fn peek_player1() -> (u64, [i32; 4]) {
    let (kb, ka) = KEYS.peek();
    let (pb, pa) = PADS[0].peek();
    let (sb, sa) = SCRIPT.peek();
    (kb | pb | sb, std::array::from_fn(|i| pick(pick(ka[i], pa[i]), sa[i])))
}

/// Whether a pad's Home button is held on any device (the front end's menu).
pub fn home_held() -> bool {
    INPUTS.lock().unwrap().as_ref().is_some_and(|i| i.devs.iter().any(|d| d.home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rumble_settings_as_nxnative() {
        let s = RumbleSettings { on: true, gain: 1.0, min: 0.0 };
        assert_eq!(s.level(0.5), 0.5);
        // gain 2: doubled, at most 1; gain 0 silent
        assert_eq!(RumbleSettings { gain: 2.0, ..s }.level(0.3), 0.6);
        assert_eq!(RumbleSettings { gain: 2.0, ..s }.level(0.8), 1.0);
        assert_eq!(RumbleSettings { gain: 0.0, ..s }.level(0.8), 0.0);
        // cutoff 0.45 (MK8D): the low texture silent, the rest stretched
        let m = RumbleSettings { min: 0.45, ..s };
        assert_eq!(m.level(0.4), 0.0);
        assert!((m.level(0.725) - 0.5).abs() < 1e-6 && m.level(1.0) == 1.0);
        // off: nothing; frequencies kept
        let v = RumbleSettings { on: false, ..s }.apply(Vib { amp_low: 0.9, freq_low: 100.0, amp_high: 0.9, freq_high: 300.0 });
        assert_eq!((v.amp_low, v.amp_high, v.freq_low, v.freq_high), (0.0, 0.0, 100.0, 300.0));
        assert_eq!(s.level(f32::NAN), 0.0);
    }

    /// nxnative hle_hid.c h_hid_InitVib: an initialised vibration device gets an idle value sent even when the pad
    /// already has it (a pad left vibrating stops); an unchanged ordinary value is not sent again.
    #[test]
    fn vibration_init_sends_idle() {
        let p = PLAYERS - 1;
        RUMBLE.lock().unwrap().dirty[p] = false;
        vibrate(p, 0, Vib::default());
        assert!(!RUMBLE.lock().unwrap().dirty[p], "the same value: nothing to send");
        vibrate_with(p, 0, Vib::default(), true);
        assert!(RUMBLE.lock().unwrap().dirty[p], "initialised: sent");
        RUMBLE.lock().unwrap().dirty[p] = false;
    }

    /// pad_merge, pad_press_to_join (the lowest free player), keyboard_player own (controllers from player 2).
    #[test]
    fn slot_rules_of_the_pad_settings() {
        let mut s = Slots::default();
        assert_eq!(s.take_with("a", "Pad", true, false, 0), Some(0));
        assert_eq!(s.take_with("b", "Pad", true, false, 0), Some(0), "merged: everyone is player 1");
        assert!(s.owner.iter().all(|o| o.is_none()));
        // keyboard own: the first pad is player 2
        let mut s = Slots::default();
        assert_eq!(s.take_with("a", "Pad A", false, false, 1), Some(1));
        assert_eq!(s.take_with("b", "Pad B", false, false, 1), Some(2));
        // press to join: the lowest free player, not the one a pad of that name held
        let mut s = Slots::default();
        s.take_with("a", "Pad A", false, false, 0);
        s.take_with("b", "Pad B", false, false, 0);
        s.free("a");
        s.free("b");
        assert_eq!(s.take_with("b2", "Pad B", false, false, 0), Some(1), "by default a reconnecting pad gets its player back");
        s.free("b2");
        assert_eq!(s.take_with("b3", "Pad B", false, true, 0), Some(0), "press to join: the lowest free player");
        // the default rule is nxnative's own
        let (mut x, mut y) = (Slots::default(), Slots::default());
        for (k, n) in [("a", "A"), ("b", "B"), ("c", "C")] {
            assert_eq!(x.take(k, n), y.take_with(k, n, false, false, 0));
        }
    }

    #[test]
    fn slots_follow_nxnative() {
        let mut s = Slots::default();
        assert_eq!(s.take("a", "Pad A"), Some(0));
        assert_eq!(s.take("b", "Pad B"), Some(1));
        assert_eq!(s.free("a"), Some(0));
        assert_eq!(s.take("c", "Pad C"), Some(0), "player 1 when free");
        assert_eq!(s.free("b"), Some(1));
        assert_eq!(s.take("d", "Pad D"), Some(2), "a slot never held before one a pad left");
        assert_eq!(s.take("b2", "Pad B"), Some(1), "the same name gets its player back");
    }

    #[test]
    fn latches() {
        let p = Path::new();
        p.set(1, [0; 4]);
        p.set(0, [0; 4]);
        assert_eq!(p.take().0, 1, "a press shorter than a read still reaches it");
        assert_eq!(p.take().0, 0);
        assert_eq!(unpack(pack([-32768, 32767, 5, -5])), [-32768, 32767, 5, -5]);
    }
}

#[cfg(test)]
mod touch_tests {
    /// The touch screen is one global: its tests take turns.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The touch screen the front end feeds: a press is one finger with Start on its first sample; a quick click
    /// stays down for the latch so the game samples it; End on the last sample; then nothing.
    #[test]
    fn touch_press_latch_and_release() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::set_touch(true, 100, 200);
        super::set_touch(false, 100, 200);
        let a = super::touch_now().unwrap();
        assert!(a.start && !a.end && (a.x, a.y) == (100, 200), "{a:?}");
        let b = super::touch_now().unwrap();
        assert!(!b.start && !b.end, "a quick click is latched: still down");
        std::thread::sleep(super::TOUCH_LATCH);
        let c = super::touch_now().unwrap();
        assert!(c.end && c.finger == a.finger, "{c:?}");
        assert_eq!(super::touch_now(), None);
        super::set_touch(true, 5000, 5000);
        let d = super::touch_now().unwrap();
        assert!(d.start && d.finger == a.finger + 1 && (d.x, d.y) == (1279, 719), "a new finger, clamped: {d:?}");
        super::set_touch(false, 0, 0);
    }

    /// Two fingers at once (slots 2 and 3 here): two touches with their own ids, released one by one; the gesture
    /// view sees both.
    #[test]
    fn two_fingers_read_together() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        super::set_finger(2, true, 100, 100);
        super::set_finger(3, true, 300, 100);
        let peek = super::touches_peek();
        assert!(peek.contains(&(100, 100)) && peek.contains(&(300, 100)));
        let v: Vec<_> = super::touches_now().into_iter().filter(|t| t.y == 100).collect();
        assert_eq!(v.len(), 2);
        assert!(v[0].start && v[1].start && v[0].finger != v[1].finger);
        super::set_finger(2, false, 100, 100);
        super::set_finger(3, false, 300, 100);
        std::thread::sleep(super::TOUCH_LATCH);
        let v: Vec<_> = super::touches_now().into_iter().filter(|t| t.y == 100).collect();
        assert!(v.len() == 2 && v.iter().all(|t| t.end));
        assert!(super::touches_now().iter().all(|t| t.y != 100));
    }

    /// Ctrl + drag mirrors a second finger about the press point (pinch / rotate); Shift + drag keeps both 100 px
    /// out, turned by the mouse's angle; the control pipe's two fingers either side of a centre.
    #[test]
    fn mouse_and_pipe_fingers() {
        use super::{mouse_fingers, two_fingers, MouseTouch};
        assert_eq!(mouse_fingers(MouseTouch::One, (0, 0), (50, 60)), vec![(50, 60)]);
        assert_eq!(mouse_fingers(MouseTouch::Pinch, (640, 360), (700, 360)), vec![(700, 360), (580, 360)]);
        assert_eq!(mouse_fingers(MouseTouch::Pinch, (10, 10), (100, 100)), vec![(100, 100), (0, 0)], "clamped to the screen");
        assert_eq!(mouse_fingers(MouseTouch::Rotate, (640, 360), (900, 360)), vec![(740, 360), (540, 360)]);
        assert_eq!(mouse_fingers(MouseTouch::Rotate, (640, 360), (640, 400)), vec![(640, 460), (640, 260)]);
        assert_eq!(two_fingers(640.0, 360.0, 50.0, 0.0), [(690, 360), (590, 360)]);
        assert_eq!(two_fingers(640.0, 360.0, 50.0, 90.0), [(640, 410), (640, 310)]);
    }
}
