//! nn::hid: the host's controllers (`crate::input`) as the game's npads, after nxnative's `hle_hid.c`.
//!
//! Player 1 is the Handheld npad in handheld mode (`oe::opmode() == 0`), else No1; player n is No n. Player 1 reads
//! the keyboard and mouse, the scripted path and its controller ORed (each stick axis the most deflected source);
//! players 2..8 read their own controller. A player whose npad the game does not support (SetSupportedNpadIdType) is
//! not reported: in a one-player game its controller feeds player 1, otherwise it is not used. A pad holding Joy-Cons
//! is shown in the style the game asked for: both of a pair as one JoyDual npad, or split into JoyLeft / JoyRight on
//! two npads under single assignment; a game that supports no Joy-Con style gets the pair's merged state as a
//! full-key pad. A full-key pad the game does not support is shown as a JoyDual pair (the one style with every
//! button), and each npad's style-set event is signalled when its style changes.
//!
//! State reads fill every history entry asked for with consecutive sampling numbers (games read all 16 whatever the
//! count says). Six-axis sensors report the controller's live motion while it sends some (DualSense / DS4 / Switch
//! Pro / Joy-Con), else a controller at rest lying flat. Vibration goes to the player's controller (`input::vibrate`).
//! Touch, gestures, keyboard, mouse and the debug pad read connected-but-idle states.

use crate::guest::{r32, w32, w64, w8, Args};
use crate::hle::{Registry, RES_OK};
use crate::input::{self, pick, PLAYERS};
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Mutex;

pub const NPAD_NO1: u32 = 0;
pub const NPAD_HANDHELD: u32 = 0x20;
pub const STYLE_FULLKEY: u32 = 1;
pub const STYLE_HANDHELD: u32 = 2;
pub const STYLE_JOYDUAL: u32 = 4;
pub const STYLE_JOYLEFT: u32 = 8;
pub const STYLE_JOYRIGHT: u32 = 0x10;
/// The Joy-Con 2 forms (Switch 2 SDKs).
pub const STYLE_JOYDUAL2: u32 = 1 << 12;
pub const STYLE_JOYLEFT2: u32 = 1 << 13;
pub const STYLE_JOYRIGHT2: u32 = 1 << 14;
const ATTR_CONNECTED: u32 = 1;
const ATTR_WIRED: u32 = 2;
const ATTR_LEFT: u32 = 4;
const ATTR_LEFT_WIRED: u32 = 8;
const ATTR_RIGHT: u32 = 0x10;
const ATTR_RIGHT_WIRED: u32 = 0x20;
const STICK: i32 = 0x4000;

static SAMPLING: AtomicI64 = AtomicI64::new(1 << 16);
static STYLES: AtomicU32 = AtomicU32::new(0xFFFF_FFFF);
/// Bit id for No1..No8, bit 8 Handheld, bit 9 Other: all until the game names its npads.
static IDS: AtomicU32 = AtomicU32::new(0x3ff);
static HOLD: AtomicU32 = AtomicU32::new(0);
/// Bit id: single Joy-Con assignment on npad No(id + 1); KEEP[id]: 1 the left one stays, 2 the right one.
static SINGLE: AtomicU32 = AtomicU32::new(0);
static KEEP: [AtomicU32; PLAYERS] = [const { AtomicU32::new(0) }; PLAYERS];
/// Players the controller applet left out, and whether its maximum was one.
static BENCHED: AtomicU32 = AtomicU32::new(0);
static BENCH_ONE: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------- players and npads

pub fn connected_id() -> u32 {
    if crate::services::oe::opmode() == 0 {
        NPAD_HANDHELD
    } else {
        NPAD_NO1
    }
}

fn id_bit(id: u32) -> u32 {
    if (id as usize) < PLAYERS {
        1 << id
    } else if id == NPAD_HANDHELD {
        1 << 8
    } else if id == 0x10 {
        1 << 9
    } else {
        0
    }
}

fn players() -> u32 {
    input::PLAYERS_MASK.load(Ordering::Acquire) | 1
}

fn reported(p: usize) -> bool {
    p == 0
        || (players() >> p & 1 != 0
            && IDS.load(Ordering::Acquire) & id_bit(p as u32) != 0
            && BENCHED.load(Ordering::Acquire) >> p & 1 == 0)
}

fn unsupported_players() -> u32 {
    (1..PLAYERS)
        .filter(|&p| players() >> p & 1 != 0 && !reported(p))
        .fold(0, |m, p| m | 1 << p)
}

fn one_player() -> bool {
    IDS.load(Ordering::Acquire) & ((1 << PLAYERS) - 2) == 0
}

/// Players whose controllers feed player 1.
fn merged_players() -> u32 {
    if one_player() {
        unsupported_players()
    } else if BENCH_ONE.load(Ordering::Acquire) != 0 {
        BENCHED.load(Ordering::Acquire) & players()
    } else {
        0
    }
}

fn player_of(id: u32) -> Option<usize> {
    if id == connected_id() {
        return Some(0);
    }
    (id > 0 && (id as usize) < PLAYERS && reported(id as usize)).then_some(id as usize)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    None,
    Pad,
    Dual,
    Left,
    Right,
}

fn joy_supported() -> u32 {
    let s = STYLES.load(Ordering::Acquire);
    s | (s >> 10 & (STYLE_JOYDUAL | STYLE_JOYLEFT | STYLE_JOYRIGHT))
}
fn joy_sides(p: usize) -> u32 {
    input::JOY_SIDES[p].load(Ordering::Acquire)
}
fn joy_active(p: usize) -> bool {
    p < PLAYERS && joy_sides(p) != 0 && !(p == 0 && crate::services::oe::opmode() == 0)
}
fn joy_split(p: usize) -> bool {
    joy_sides(p) == 3
        && SINGLE.load(Ordering::Acquire) >> p & 1 != 0
        && joy_supported() & (STYLE_JOYLEFT | STYLE_JOYRIGHT) == (STYLE_JOYLEFT | STYLE_JOYRIGHT)
}
/// The npad the other Joy-Con of a split pair on player p takes: the lowest supported No2..No8 with no player.
fn split_target(p: usize) -> Option<usize> {
    let mut taken = players() | 1;
    let mut mine = None;
    for q in 0..=p.min(PLAYERS - 1) {
        if joy_active(q) && joy_split(q) {
            let t = (1..PLAYERS).find(|&i| {
                taken >> i & 1 == 0 && IDS.load(Ordering::Acquire) & id_bit(i as u32) != 0
            });
            if let Some(t) = t {
                taken |= 1 << t;
            }
            if q == p {
                mine = t;
            }
        }
    }
    mine
}

fn resolve(id: u32) -> (Option<usize>, Kind) {
    if let Some(pl) = player_of(id) {
        let mut k = Kind::Pad;
        if joy_active(pl) {
            let s = joy_sides(pl);
            k = if s == 3 {
                if joy_split(pl) {
                    if KEEP[pl].load(Ordering::Acquire) == 2 {
                        Kind::Right
                    } else {
                        Kind::Left
                    }
                } else if joy_supported() & STYLE_JOYDUAL != 0 {
                    Kind::Dual
                } else {
                    Kind::Pad
                }
            } else if s == 1 {
                Kind::Left
            } else {
                Kind::Right
            };
            if k == Kind::Left && joy_supported() & STYLE_JOYLEFT == 0
                || k == Kind::Right && joy_supported() & STYLE_JOYRIGHT == 0
            {
                k = Kind::Pad;
            }
        }
        return (Some(pl), k);
    }
    if id > 0 && (id as usize) < PLAYERS {
        for p in 0..PLAYERS {
            if joy_active(p) && joy_split(p) && split_target(p) == Some(id as usize) {
                return (
                    Some(p),
                    if KEEP[p].load(Ordering::Acquire) == 2 {
                        Kind::Left
                    } else {
                        Kind::Right
                    },
                );
            }
        }
    }
    (None, Kind::None)
}

fn pad_style(id: u32) -> u32 {
    if id == NPAD_HANDHELD {
        return STYLE_HANDHELD;
    }
    let s = STYLES.load(Ordering::Acquire);
    if s & STYLE_FULLKEY == 0 && s & STYLE_JOYDUAL != 0 {
        STYLE_JOYDUAL
    } else {
        STYLE_FULLKEY
    }
}

fn kind_style(k: Kind, id: u32) -> u32 {
    match k {
        Kind::Pad => pad_style(id),
        Kind::Dual => STYLE_JOYDUAL,
        Kind::Left => STYLE_JOYLEFT,
        Kind::Right => STYLE_JOYRIGHT,
        Kind::None => 0,
    }
}

/// The style set npad `id` reports.
pub fn style_of(id: u32) -> u32 {
    let (_, k) = resolve(id);
    kind_style(k, id)
}

// style-set events per npad (No1..No8, then Handheld): signalled when the style changes
const SLOTS: usize = PLAYERS + 1;
struct StyleEv {
    ev: [u64; SLOTS],
    told: [u32; SLOTS],
}
static STYLE_EV: Mutex<StyleEv> = Mutex::new(StyleEv {
    ev: [0; SLOTS],
    told: [0; SLOTS],
});

fn style_slot(id: u32) -> Option<usize> {
    if (id as usize) < PLAYERS {
        Some(id as usize)
    } else if id == NPAD_HANDHELD {
        Some(PLAYERS)
    } else {
        None
    }
}

fn styles_sync() {
    let mut g = STYLE_EV.lock().unwrap();
    for i in 0..SLOTS {
        let id = if i < PLAYERS { i as u32 } else { NPAD_HANDHELD };
        let st = style_of(id);
        if st == g.told[i] {
            continue;
        }
        g.told[i] = st;
        if g.ev[i] != 0 {
            crate::services::os::signal_bound(g.ev[i]);
        }
    }
}

/// The input layer: a pad's Joy-Cons changed.
pub fn styles_changed() {
    styles_sync();
}

/// The input layer: the players with a controller changed.
pub fn players_changed(mask: u32) {
    BENCHED.fetch_and(mask, Ordering::AcqRel);
    styles_sync();
    note_merged();
}

fn note_merged() {
    static LAST: AtomicU32 = AtomicU32::new(0);
    let m = unsupported_players();
    let key = m | (one_player() as u32) << 31 | BENCHED.load(Ordering::Relaxed) << 8;
    if LAST.swap(key, Ordering::Relaxed) == key {
        return;
    }
    for p in 1..PLAYERS {
        if m >> p & 1 != 0 {
            if one_player() {
                crate::log!("hid: player {}'s controller feeds player 1: the game supports one player (no npad {p})", p + 1);
            } else {
                crate::log!(
                    "hid: player {}'s controller is not used: the game supports no npad {p}",
                    p + 1
                );
            }
        }
    }
}

// ---------------------------------------------------------------- states

fn stick_dirs(a: &[i32; 4]) -> u64 {
    let mut b = 0;
    for (cond, bit) in [
        (a[0] < -STICK, 16),
        (a[1] > STICK, 17),
        (a[0] > STICK, 18),
        (a[1] < -STICK, 19),
        (a[2] < -STICK, 20),
        (a[3] > STICK, 21),
        (a[2] > STICK, 22),
        (a[3] < -STICK, 23),
    ] {
        if cond {
            b |= 1u64 << bit;
        }
    }
    b
}

/// The merged view of player p (buttons, sticks), latches taken.
fn player_input(p: usize) -> (u64, [i32; 4]) {
    let mut b = 0u64;
    let mut a = [0i32; 4];
    let mut add = |(pb, pa): (u64, [i32; 4])| {
        b |= pb;
        for i in 0..4 {
            a[i] = pick(a[i], pa[i]);
        }
    };
    if p == 0 {
        if !input::REPLAYING.load(Ordering::Acquire) {
            add(input::KEYS.take());
            let src = 1 | merged_players();
            for q in 0..PLAYERS {
                if src >> q & 1 != 0 {
                    add(input::PADS[q].take());
                }
            }
        }
        add(input::SCRIPT.take());
    } else {
        add(input::PADS[p].take());
        add(input::SCRIPT_P[p].take());
    }
    (b, a)
}

/// One NpadState (0x28 bytes: sampling, buttons, left and right stick, attributes).
fn fill(out: u64, id: u32, style: u32, sampling: i64) {
    crate::guest::zero(out, 0x28);
    w64(out, sampling as u64);
    let (pl, kind) = resolve(id);
    let Some(p) = pl else { return };
    if kind_style(kind, id) & style == 0 {
        return;
    }
    let (b, a, attr) = match kind {
        Kind::Dual | Kind::Left | Kind::Right => {
            let (l, r) = (kind != Kind::Right, kind != Kind::Left);
            let mut b = 0u64;
            let mut a = [0i32; 4];
            for side in 0..2 {
                if if side == 0 { l } else { r } {
                    let (jb, ja) = input::JOY[p][side].take();
                    b |= jb;
                    a[2 * side] = ja[0];
                    a[2 * side + 1] = ja[1];
                }
            }
            (
                b & 0x0f00_ffff,
                a,
                ATTR_CONNECTED | if l { ATTR_LEFT } else { 0 } | if r { ATTR_RIGHT } else { 0 },
            )
        }
        _ => {
            let (b, a) = player_input(p);
            let attr = if id == NPAD_HANDHELD {
                ATTR_CONNECTED
                    | ATTR_WIRED
                    | ATTR_LEFT
                    | ATTR_LEFT_WIRED
                    | ATTR_RIGHT
                    | ATTR_RIGHT_WIRED
            } else {
                ATTR_CONNECTED | ATTR_LEFT | ATTR_RIGHT
            };
            (b & 0xffff, a, attr)
        }
    };
    if p == 0 && input::env_flag("NXRS_CONTROLLER_TRACE") {
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if LAST.swap(b, Ordering::Relaxed) != b {
            crate::log!(
                "hid: the game read player 1 on npad {id:#x}: buttons {b:#06x} sticks {a:?}"
            );
        }
    }
    w64(out + 8, b | stick_dirs(&a));
    for (i, v) in a.iter().enumerate() {
        w32(out + 0x10 + 4 * i as u64, *v as u32);
    }
    w32(out + 0x20, attr);
}

fn states(out: u64, n: i32, id: u32, style: u32) -> u64 {
    if n <= 0 {
        return 0;
    }
    let s = SAMPLING.fetch_add(1, Ordering::Relaxed) + 1;
    fill(out, id, style, s);
    for i in 1..n as u64 {
        for k in 0..5u64 {
            w64(out + 0x28 * i + 8 * k, crate::guest::r64(out + 8 * k));
        }
        w64(out + 0x28 * i, (s - i as i64) as u64);
    }
    n as u64
}

macro_rules! hidfn {
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

macro_rules! style_fns {
    ($one:ident, $many:ident, $style:expr) => {
        hidfn!($one, |a| {
            input::start();
            let s = SAMPLING.fetch_add(1, Ordering::Relaxed) + 1;
            fill(a.x(0), r32(a.x(1)), $style, s);
            0
        });
        hidfn!($many, |a| {
            input::start();
            states(a.x(0), a.i(1), r32(a.x(2)), $style)
        });
    };
}
style_fns!(state_fullkey, states_fullkey, STYLE_FULLKEY);
style_fns!(state_handheld, states_handheld, STYLE_HANDHELD);
style_fns!(state_joydual, states_joydual, STYLE_JOYDUAL);
style_fns!(state_joyleft, states_joyleft, STYLE_JOYLEFT);
style_fns!(state_joyright, states_joyright, STYLE_JOYRIGHT);

hidfn!(init_npad, |a| {
    input::start();
    0
});
hidfn!(nothing, |a| { 0 });
hidfn!(ok, |a| { RES_OK });
hidfn!(set_ids, |a| {
    let (ids, n) = (a.x(0), a.x(1));
    let mut m = 0u32;
    for i in 0..n.min(16) {
        m |= id_bit(r32(ids + 4 * i));
    }
    if IDS.swap(m, Ordering::AcqRel) != m {
        BENCHED.store(0, Ordering::Release);
        crate::log!("hid: supported npad ids {m:#x} (bit n: No n+1, bit 8 Handheld, bit 9 Other)");
        styles_sync();
        note_merged();
    }
    0
});
hidfn!(set_styles, |a| {
    let s = a.w(0);
    if STYLES.swap(s, Ordering::AcqRel) != s {
        crate::log!(
            "hid: supported npad styles {s:#x}; player 1 reports {:#x} on npad {:#x}",
            style_of(connected_id()),
            connected_id()
        );
        styles_sync();
    }
    0
});
hidfn!(get_styles, |a| { STYLES.load(Ordering::Acquire) as u64 });
hidfn!(get_style, |a| { style_of(r32(a.x(0))) as u64 });
hidfn!(hold_type_set, |a| {
    HOLD.store(a.w(0), Ordering::Relaxed);
    0
});
hidfn!(hold_type, |a| { HOLD.load(Ordering::Relaxed) as u64 });
fn joy_assign(id: u32, single: bool, keep: u32) {
    if id as usize >= PLAYERS {
        return;
    }
    KEEP[id as usize].store(keep, Ordering::Release);
    let was = SINGLE.load(Ordering::Acquire);
    let now = if single {
        was | 1 << id
    } else {
        was & !(1 << id)
    };
    if SINGLE.swap(now, Ordering::AcqRel) != now {
        crate::log!(
            "hid: npad {id}: {} Joy-Con assignment",
            if single { "single" } else { "dual" }
        );
        styles_sync();
    }
}
hidfn!(joy_get_assign, |a| {
    let id = r32(a.x(0));
    ((id as usize) < PLAYERS && SINGLE.load(Ordering::Acquire) >> id & 1 != 0) as u64
});
hidfn!(joy_single, |a| {
    joy_assign(r32(a.x(0)), true, 0);
    0
});
hidfn!(joy_single_dev, |a| {
    // (const NpadId&, NpadJoyDeviceType): 0 left stays, 1 right stays
    joy_assign(r32(a.x(0)), true, if a.w(1) == 1 { 2 } else { 1 });
    0
});
hidfn!(joy_single_out, |a| {
    // (NpadId* out, const NpadId&, NpadJoyDeviceType): the npad the other half went to, if any
    let id = r32(a.x(1));
    joy_assign(id, true, if a.w(2) == 1 { 2 } else { 1 });
    let t = split_target(id as usize);
    if let Some(t) = t {
        w32(a.x(0), t as u32);
    }
    t.is_some() as u64
});
hidfn!(joy_dual, |a| {
    joy_assign(r32(a.x(0)), false, 0);
    0
});
hidfn!(zero, |a| { 0 });
hidfn!(one, |a| { 1 });
hidfn!(led, |a| {
    let id = r32(a.x(0));
    let p = if id == NPAD_HANDHELD { 0 } else { id as usize };
    crate::input::nintendo::lights(p) as u64
});
hidfn!(color, |a| {
    w32(a.x(0), 0xFF32_3232);
    w32(a.x(0) + 4, 0xFFFF_FFFF);
    RES_OK
});
hidfn!(color2, |a| {
    for o in [a.x(0), a.x(1)] {
        w32(o, 0xFF32_3232);
        w32(o + 4, 0xFFFF_FFFF);
    }
    RES_OK
});
hidfn!(icon_color, |a| {
    // no host controller is a decorated Joy-Con (Switch 2 SDKs): ResultNoIconDecoration, or invalid npad
    let id = r32(a.x(1));
    if (id as usize) < 8 || id == 0x10 || id == 0x20 {
        0x4b6ca
    } else {
        0x58aca
    }
});
hidfn!(icon_color2, |a| {
    let id = r32(a.x(2));
    if (id as usize) < 8 || id == 0x10 || id == 0x20 {
        0x4b6ca
    } else {
        0x58aca
    }
});

fn players_reported() -> i32 {
    (0..PLAYERS)
        .map(|p| {
            reported(p) as i32
                + (reported(p) && joy_active(p) && joy_split(p) && split_target(p).is_some()) as i32
        })
        .sum()
}

const RES_CTRL_CANCELED: u64 = 202 | 3101 << 9;

/// The controller-support prompt over the game until it has its answer (true: enough players, done).
fn ctrl_prompt(min: i32, max: i32, joined: i32) -> bool {
    use crate::ui::{CpPhase, CtrlPrompt};
    let (a, b) = crate::ui::pads_ab();
    let mut q = CtrlPrompt::open(min, max, joined, a, b);
    crate::log!("hid: controller applet: {min} players wanted, {joined} joined: the prompt waits for controllers to join");
    crate::input::set_prompt(true);
    let mut shown = String::new();
    loop {
        let (a, b) = crate::ui::pads_ab();
        let (confirm, cancel) = crate::ui::keys();
        let phase = q.step(
            players_reported(),
            crate::ui::pads_waiting(),
            a,
            b,
            confirm,
            cancel,
        );
        if phase == CpPhase::Open {
            let (t, x) = q.text();
            if x != shown {
                crate::ui::show(&t, &x);
                shown = x;
            }
        } else {
            crate::ui::hide();
        }
        if phase == CpPhase::Idle {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    crate::input::set_prompt(false);
    crate::log!(
        "hid: controller prompt closed: {} ({} players)",
        if q.ok { "done" } else { "cancelled" },
        q.joined
    );
    q.ok
}
hidfn!(controller_support, |a| {
    // (ControllerSupportResultInfo* out, const ControllerSupportArg& {i8 min, i8 max, ...})
    let (out, arg) = (a.x(0), a.x(1));
    let (min, max) = if arg != 0 {
        (
            crate::guest::rd::<i8>(arg) as i32,
            crate::guest::rd::<i8>(arg + 1) as i32,
        )
    } else {
        (0, 0)
    };
    let mut count = players_reported();
    // more players wanted than reported: those left out before count again
    for p in 1..PLAYERS {
        if max <= 0 || count < max {
            if BENCHED.load(Ordering::Acquire) >> p & 1 != 0 {
                BENCHED.fetch_and(!(1 << p), Ordering::AcqRel);
                count = players_reported();
            }
        }
    }
    // too few: a pair of Joy-Cons held sideways is two
    for p in 0..PLAYERS {
        if count >= min {
            break;
        }
        if reported(p)
            && joy_active(p)
            && joy_sides(p) == 3
            && SINGLE.load(Ordering::Acquire) >> p & 1 == 0
        {
            joy_assign(p as u32, true, 0);
            if joy_split(p) && split_target(p).is_some() {
                count += 1;
            } else {
                joy_assign(p as u32, false, 0);
            }
        }
    }
    // still too few: the player screen a Switch shows, as a host prompt the game's thread waits on until enough
    // controllers joined or the player closes it (nxnative ctrl_prompt.h); the ini's controller_prompt=0 (or
    // NXOS_CTRL_PROMPT=0, scripted runs) answers at once
    let mut canceled = min > count;
    if canceled
        && nxconfig::flag("controller_prompt")
        && std::env::var("NXOS_CTRL_PROMPT").as_deref() != Ok("0")
    {
        let ok = crate::sched::without_core(|| ctrl_prompt(min, max, count));
        count = players_reported();
        canceled = !ok || min > count;
    }
    // more than the maximum: the highest players are left out
    let mut p = PLAYERS - 1;
    while max > 0 && p > 0 && count > max {
        if reported(p) {
            BENCHED.fetch_or(1 << p, Ordering::AcqRel);
            count = players_reported();
        }
        p -= 1;
    }
    BENCH_ONE.store((max == 1) as u32, Ordering::Release);
    styles_sync();
    note_merged();
    if max > 0 && count > max {
        count = max;
    }
    crate::log!(
        "hid: controller applet: {min} to {max} players wanted, {count} connected{}",
        if canceled {
            ": too few, closed as the player would (canceled)"
        } else {
            ""
        }
    );
    // the applet takes the focus while open (MK8D applies its answer only after the InFocus message 15)
    crate::services::oe::applet_enter("controller");
    crate::services::oe::applet_leave();
    if out != 0 {
        w8(out, count as u8);
        w32(out + 4, connected_id());
    }
    if canceled {
        RES_CTRL_CANCELED
    } else {
        RES_OK
    }
});
hidfn!(controller_support_noout, |a| {
    let arg = a.x(0);
    let min = if arg != 0 {
        crate::guest::rd::<i8>(arg) as i32
    } else {
        0
    };
    if min > players_reported() {
        RES_CTRL_CANCELED
    } else {
        RES_OK
    }
});
unsafe fn bind_style_event(c: *mut Cpu) {
    let (id, ev, mode) = (r32((*c).x[0]), (*c).x[1], (*c).x[2] as u32);
    crate::services::os::bind_system_event(c, ev, mode == 1);
    if let Some(s) = style_slot(id) {
        let mut g = STYLE_EV.lock().unwrap();
        g.ev[s] = ev;
        g.told[s] = style_of(id);
    }
    if style_of(id) != 0 {
        crate::services::os::signal_bound(ev);
    }
}

// ---------------------------------------------------------------- six-axis sensors

// SixAxisSensorHandle: {u8 style index, u8 npad id, u8 device index (0 left, 1 right), u8}.
hidfn!(six_handles, |a| {
    let (h, n, id, style) = (a.x(0), a.i(1), r32(a.x(2)), a.w(3));
    if n <= 0 {
        return 0;
    }
    let idx = match style {
        s if s & STYLE_FULLKEY != 0 => 3,
        s if s & STYLE_HANDHELD != 0 => 4,
        s if s & STYLE_JOYDUAL != 0 => 5,
        s if s & STYLE_JOYLEFT != 0 => 6,
        s if s & STYLE_JOYRIGHT != 0 => 7,
        _ => 3,
    };
    let k = if style & (STYLE_HANDHELD | STYLE_JOYDUAL) != 0 {
        2.min(n)
    } else {
        1
    };
    for i in 0..k as u64 {
        let dev = if style & STYLE_JOYRIGHT != 0 {
            1
        } else {
            i as u32
        };
        w32(h + 4 * i, idx | (id & 0xFF) << 8 | dev << 16);
    }
    k as u64
});

/// The live motion behind a handle at host time t, if any is fresh.
fn six_live(handle: u32, t: f64) -> Option<input::motion::Sample> {
    let id = handle >> 8 & 0xff;
    let dev = (handle >> 16 & 0xff) as usize;
    let style_idx = handle & 0xff;
    let (pl, kind) = resolve(id);
    let p = pl?;
    let m = input::motion();
    let joy = matches!(kind, Kind::Dual | Kind::Left | Kind::Right) || style_idx >= 5;
    if joy {
        let side = match kind {
            Kind::Right => 1,
            Kind::Left => 0,
            _ => dev.min(1),
        };
        let s = m.joy[p][side].lock().unwrap();
        if s.fresh(t) {
            return Some(s.at(t));
        }
    }
    // the player's pad, or in a one-player game whichever merged pad is fresh
    let src = if p == 0 { 1 | merged_players() } else { 1 << p };
    for q in 0..PLAYERS {
        if src >> q & 1 != 0 {
            let s = m.pad[q].lock().unwrap();
            if s.fresh(t) {
                return Some(s.at(t));
            }
        }
    }
    None
}

/// The player an npad id shows (a split Joy-Con pair's other half included), for the Joy-Con mouse.
pub fn player_on(id: u32) -> Option<usize> {
    resolve(id).0
}

/// The six-axis sample behind a handle at host time t: the live motion, else at rest.
pub fn six_sample_at(handle: u32, t: f64) -> input::motion::Sample {
    six_live(handle, t).unwrap_or_default()
}

/// Whether a live sensor stands behind a handle now.
pub fn six_sample_live(handle: u32) -> bool {
    six_live(handle, input::motion::now_s()).is_some()
}

fn six_fill(out: u64, sampling: i64, s: Option<input::motion::Sample>) {
    // SixAxisSensorState: delta time, sampling, acceleration, angular velocity, angle, direction (3x3), attributes
    crate::guest::zero(out, 0x60);
    w64(out, 5_000_000);
    w64(out + 8, sampling as u64);
    let s = s.unwrap_or_default();
    for i in 0..3u64 {
        crate::guest::wr::<f32>(out + 0x10 + 4 * i, s.acc[i as usize]);
        crate::guest::wr::<f32>(out + 0x1c + 4 * i, s.gyro[i as usize]);
        crate::guest::wr::<f32>(out + 0x28 + 4 * i, s.angle[i as usize]);
        for j in 0..3u64 {
            crate::guest::wr::<f32>(out + 0x34 + 12 * i + 4 * j, s.dir[i as usize][j as usize]);
        }
    }
    w32(out + 0x58, 1);
}
fn six_tick(t: f64) -> i64 {
    (t / 0.005) as i64
}
hidfn!(six_state, |a| {
    let t = input::motion::now_s();
    six_fill(a.x(0), six_tick(t), six_live(r32(a.x(1)), t));
    0
});
hidfn!(six_states, |a| {
    let (out, n, h) = (a.x(0), a.i(1), r32(a.x(2)));
    if n <= 0 {
        return 0;
    }
    let t = input::motion::now_s();
    let tick = six_tick(t);
    for i in 0..n as i64 {
        let ti = (tick - i) as f64 * 0.005;
        six_fill(out + 0x60 * i as u64, tick - i, six_live(h, ti));
    }
    n as u64
});
hidfn!(six_at_rest, |a| {
    let t = input::motion::now_s();
    match six_live(r32(a.x(0)), t) {
        Some(s) => (s.gyro.iter().all(|g| g.abs() < 0.01)) as u64,
        None => 1,
    }
});
hidfn!(fusion_params, |a| {
    crate::guest::out(a.x(0), 0.03f32);
    crate::guest::out(a.x(1), 0.4f32);
    0
});

// ---------------------------------------------------------------- vibration

hidfn!(vib_handles, |a| {
    // (handles*, n, const NpadId&, style): the left and right devices of the controller
    let (h, n, id, style) = (a.x(0), a.i(1), r32(a.x(2)), a.w(3));
    let k = if style & (STYLE_JOYLEFT | STYLE_JOYRIGHT) != 0 {
        1
    } else {
        n.clamp(0, 2) as u64
    };
    let ty = if id == NPAD_HANDHELD { 2 } else { 3 };
    for i in 0..k {
        let dev = if style & STYLE_JOYRIGHT != 0 {
            1
        } else {
            i as u32
        };
        w32(h + 4 * i, ty | (id & 0xFF) << 8 | dev << 16);
    }
    k
});
hidfn!(vib_info, |a| {
    // VibrationDeviceInfo {u32 type (1 linear resonant actuator), u32 position (1 left, 2 right)}
    let h = r32(a.x(1));
    w32(a.x(0), 1);
    w32(a.x(0) + 4, 1 + (h >> 16 & 1));
    0
});
fn vib_send(handle: u32, v: u64) {
    let rd = |o: u64| crate::guest::rd::<f32>(v + o);
    vib_route(
        handle,
        input::Vib {
            amp_low: rd(0),
            freq_low: rd(4),
            amp_high: rd(8),
            freq_high: rd(12),
        },
        false,
    );
}
/// A vibration value to the pad a handle names (`force`: sent even when it is the value the pad has).
fn vib_route(handle: u32, val: input::Vib, force: bool) {
    let id = handle >> 8 & 0xff;
    let side = (handle >> 16 & 1) as usize;
    let (pl, kind) = resolve(id);
    let Some(p) = pl else { return };
    let side = match kind {
        Kind::Left => 0,
        Kind::Right => 1,
        _ => side,
    };
    static LAST: Mutex<[[u32; 4]; 2 * PLAYERS]> = Mutex::new([[0; 4]; 2 * PLAYERS]);
    if input::env_flag("NXRS_CONTROLLER_TRACE") {
        let bits = [
            val.amp_low.to_bits(),
            val.freq_low.to_bits(),
            val.amp_high.to_bits(),
            val.freq_high.to_bits(),
        ];
        let mut l = LAST.lock().unwrap();
        if l[2 * p + side] != bits && (val.amp_low > 0.0 || val.amp_high > 0.0) {
            crate::log!(
                "hid: vibration player {} side {side}: low {:.3}@{:.0} Hz high {:.3}@{:.0} Hz",
                p + 1,
                val.amp_low,
                val.freq_low,
                val.amp_high,
                val.freq_high
            );
        }
        l[2 * p + side] = bits;
    }
    input::vibrate_with(p, side, val, force);
}
// InitializeVibrationDevice: the channel starts idle, an idle value sent to it (nxnative hle_hid.c h_hid_InitVib:
// {0, 160 Hz, 0, 320 Hz}), so a pad left vibrating by an earlier run or another program stops
hidfn!(vib_init, |a| {
    vib_route(r32(a.x(0)), input::Vib::default(), true);
    0
});
hidfn!(vib_one, |a| {
    vib_send(r32(a.x(0)), a.x(1));
    0
});
hidfn!(vib_many, |a| {
    let (hs, vs, n) = (a.x(0), a.x(1), a.i(2));
    for i in 0..n.clamp(0, 16) as u64 {
        vib_send(r32(hs + 4 * i), vs + 16 * i);
    }
    RES_OK
});
hidfn!(vib_actual, |a| {
    let o = a.x(0);
    crate::guest::zero(o, 16);
    crate::guest::wr::<f32>(o + 4, 160.0);
    crate::guest::wr::<f32>(o + 12, 320.0);
    0
});

// ---------------------------------------------------------------- other devices: connected, idle

// TouchScreenState<N> = { i64 sampling number; i32 count; i32 reserved; TouchState[N] (0x28 each) }: every
// template the SDK instantiates (N = 1..16) is taken over, each writing its own size (Donkey Kong Bananza reads
// GetTouchScreenState<1>, whose SDK version aborts with Result 0x52ca without the hid service). The touch comes from
// the front end (`input::touch_now`: the mouse's left button over the window, or the control pipe's `touch`).
// TouchState { u64 delta time; u32 attributes (1 Start, 2 End); u32 finger id; u32 x, y; u32 diameter x, y;
// u32 rotation; u32 reserved }.
fn touch_fill(out: u64, n: u64, count: i32) -> u64 {
    let stride = 0x10 + 0x28 * n;
    let num = SAMPLING.fetch_add(1, Ordering::Relaxed) as u64 + 1;
    let k = count.clamp(0, 1) as u64; // the newest sample
    let touches = if k > 0 {
        crate::input::touches_now()
    } else {
        vec![]
    };
    for i in 0..k {
        let s = out + i * stride;
        crate::guest::zero(s, stride);
        w64(s, num);
        // every finger on the screen (two for a pinch or a rotation), as many as the state holds
        let shown = touches.len().min(n as usize);
        w32(s + 8, shown as u32);
        for (j, t) in touches.iter().take(shown).enumerate() {
            let e = s + 0x10 + 0x28 * j as u64;
            w32(e + 8, t.start as u32 | (t.end as u32) << 1);
            w32(e + 0xc, t.finger);
            w32(e + 0x10, t.x);
            w32(e + 0x14, t.y);
            w32(e + 0x18, 15);
            w32(e + 0x1c, 15);
        }
    }
    k
}
macro_rules! touch_n {
    ($($n:literal $one:ident $many:ident),*) => {
        $(
            hidfn!($one, |a| { touch_fill(a.x(0), $n, 1); 0 });
            hidfn!($many, |a| { touch_fill(a.x(0), $n, a.i(1)) });
        )*
        const TOUCH_FNS: &[(u64, unsafe fn(*mut Cpu), unsafe fn(*mut Cpu))] = &[$(($n, $one, $many)),*];
    };
}
touch_n!(1 touch1 touches1, 2 touch2 touches2, 3 touch3 touches3, 4 touch4 touches4, 5 touch5 touches5, 6 touch6 touches6,
    7 touch7 touches7, 8 touch8 touches8, 9 touch9 touches9, 10 touch10 touches10, 11 touch11 touches11, 12 touch12 touches12,
    13 touch13 touches13, 14 touch14 touches14, 15 touch15 touches15, 16 touch16 touches16);
macro_rules! idle_dev {
    ($one:ident, $many:ident, $size:expr) => {
        hidfn!($one, |a| {
            crate::guest::zero(a.x(0), $size);
            w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) as u64 + 1);
            0
        });
        hidfn!($many, |a| {
            if a.i(1) <= 0 {
                return 0;
            }
            crate::guest::zero(a.x(0), $size);
            w64(a.x(0), SAMPLING.fetch_add(1, Ordering::Relaxed) as u64 + 1);
            1
        });
    };
}
idle_dev!(debugpad_state, debugpad_states, 0x20);
idle_dev!(keyboard_state, keyboard_states, 0x30); // KeyboardState is 0x30 bytes (nxnative, yuzu); 0x38 overran the caller's by 8
idle_dev!(mouse_state, mouse_states, 0x28);
// GetGestureStates(GestureState *out, int count): the newest entries of the gesture recognizer's history (newest
// first, 16 kept), built from the touch screen (`gesture.rs`, nxnative hle_touch.c / yuzu's rules).
hidfn!(gestures, |a| {
    let (out, n) = (a.x(0), a.i(1).clamp(0, 16) as usize);
    let entries =
        crate::services::gesture::fill(n, &crate::input::touches_peek(), crate::clock::now_ns());
    for (i, e) in entries.iter().enumerate() {
        crate::guest::write_bytes(out + 0x60 * i as u64, e);
    }
    entries.len() as u64
});
hidfn!(handheld_mode, |a| { 0 });
hidfn!(comm_mode, |a| { 0 });

pub fn register(r: &mut Registry) {
    const W: &str = "hid: the host's controllers (crate::input)";
    let p = "_ZN2nn3hid";
    for (n, f) in [
        ("14InitializeNpadEv", init_npad as unsafe fn(*mut Cpu)),
        ("22SetSupportedNpadIdTypeEPKjm", set_ids),
        ("24SetSupportedNpadStyleSetENS_4util10BitFlagSetILi32ENS0_12NpadStyleTagEEE", set_styles),
        ("24GetSupportedNpadStyleSetEv", get_styles),
        ("15GetNpadStyleSetERKj", get_style),
        ("12GetNpadStateEPNS0_16NpadFullKeyStateERKj", state_fullkey),
        ("12GetNpadStateEPNS0_17NpadHandheldStateERKj", state_handheld),
        ("12GetNpadStateEPNS0_16NpadJoyDualStateERKj", state_joydual),
        ("12GetNpadStateEPNS0_16NpadJoyLeftStateERKj", state_joyleft),
        ("12GetNpadStateEPNS0_17NpadJoyRightStateERKj", state_joyright),
        ("13GetNpadStatesEPNS0_16NpadFullKeyStateEiRKj", states_fullkey),
        ("13GetNpadStatesEPNS0_17NpadHandheldStateEiRKj", states_handheld),
        ("13GetNpadStatesEPNS0_16NpadJoyDualStateEiRKj", states_joydual),
        ("13GetNpadStatesEPNS0_16NpadJoyLeftStateEiRKj", states_joyleft),
        ("13GetNpadStatesEPNS0_17NpadJoyRightStateEiRKj", states_joyright),
        ("14DisconnectNpadERKj", nothing),
        ("18SetNpadJoyHoldTypeENS0_15NpadJoyHoldTypeE", hold_type_set),
        ("18GetNpadJoyHoldTypeEv", hold_type),
        ("20GetNpadJoyAssignmentERKj", joy_get_assign),
        ("28SetNpadJoyAssignmentModeDualERKj", joy_dual),
        ("30SetNpadJoyAssignmentModeSingleERKj", joy_single),
        ("30SetNpadJoyAssignmentModeSingleERKjNS0_17NpadJoyDeviceTypeE", joy_single_dev),
        ("30SetNpadJoyAssignmentModeSingleEPjRKjNS0_17NpadJoyDeviceTypeE", joy_single_out),
        ("23MergeSingleJoyAsDualJoyERKjS2_", ok),
        ("18SwapNpadAssignmentERKjS2_", nothing),
        ("21StartLrAssignmentModeEv", nothing),
        ("20StopLrAssignmentModeEv", nothing),
        ("24SetNpadCommunicationModeENS0_21NpadCommunicationModeE", nothing),
        ("24GetNpadCommunicationModeEv", comm_mode),
        ("29SetNpadHandheldActivationModeENS0_26NpadHandheldActivationModeE", nothing),
        ("29GetNpadHandheldActivationModeEv", handheld_mode),
        ("19GetPlayerLedPatternERKj", led),
        ("22GetNpadControllerColorEPNS0_19NpadControllerColorERKj", color),
        ("22GetNpadControllerColorEPNS0_19NpadControllerColorES2_RKj", color2),
        ("29GetNpadJoyIconDecorationColorEPNS0_26NpadJoyIconDecorationColorERKj", icon_color),
        ("29GetNpadJoyIconDecorationColorEPNS0_26NpadJoyIconDecorationColorES2_RKj", icon_color2),
        ("21ShowControllerSupportEPNS0_27ControllerSupportResultInfoERKNS0_20ControllerSupportArgE", controller_support),
        ("21ShowControllerSupportERKNS0_20ControllerSupportArgE", controller_support_noout),
        ("24ShowControllerStrapGuideEv", ok),
        ("28ShowControllerFirmwareUpdateERKNS0_27ControllerFirmwareUpdateArgE", ok),
        ("27BindNpadStyleSetUpdateEventERKjPNS_2os15SystemEventTypeENS3_14EventClearModeE", bind_style_event),
        ("41EnableUnintendedHomeButtonInputProtectionERKjb", nothing),
        ("44IsUnintendedHomeButtonInputProtectionEnabledERKj", zero),
        ("23GetSixAxisSensorHandlesEPNS0_19SixAxisSensorHandleEiRKjNS_4util10BitFlagSetILi32ENS0_12NpadStyleTagEEE", six_handles),
        ("18StartSixAxisSensorERKNS0_19SixAxisSensorHandleE", nothing),
        ("17StopSixAxisSensorERKNS0_19SixAxisSensorHandleE", nothing),
        ("21GetSixAxisSensorStateEPNS0_18SixAxisSensorStateERKNS0_19SixAxisSensorHandleE", six_state),
        ("22GetSixAxisSensorStatesEPNS0_18SixAxisSensorStateEiRKNS0_19SixAxisSensorHandleE", six_states),
        ("21IsSixAxisSensorAtRestERKNS0_19SixAxisSensorHandleE", six_at_rest),
        ("25EnableSixAxisSensorFusionERKNS0_19SixAxisSensorHandleEb", nothing),
        ("28IsSixAxisSensorFusionEnabledERKNS0_19SixAxisSensorHandleE", one),
        ("32SetSixAxisSensorFusionParametersERKNS0_19SixAxisSensorHandleEff", nothing),
        ("32GetSixAxisSensorFusionParametersEPfS1_RKNS0_19SixAxisSensorHandleE", fusion_params),
        ("34ResetSixAxisSensorFusionParametersERKNS0_19SixAxisSensorHandleE", nothing),
        ("25SetGyroscopeZeroDriftModeERKNS0_19SixAxisSensorHandleERKNS0_22GyroscopeZeroDriftModeE", nothing),
        ("25GetGyroscopeZeroDriftModeERKNS0_19SixAxisSensorHandleE", one),
        ("34IsSixAxisSensorDeviceNewlyAssignedERKNS0_19SixAxisSensorHandleE", zero),
        ("39ResetIsSixAxisSensorDeviceNewlyAssignedERKNS0_19SixAxisSensorHandleE", nothing),
        ("25GetVibrationDeviceHandlesEPNS0_21VibrationDeviceHandleEiRKjNS_4util10BitFlagSetILi32ENS0_12NpadStyleTagEEE", vib_handles),
        ("25InitializeVibrationDeviceERKNS0_21VibrationDeviceHandleE", vib_init),
        ("22GetVibrationDeviceInfoEPNS0_19VibrationDeviceInfoERKNS0_21VibrationDeviceHandleE", vib_info),
        ("24IsVibrationDeviceMountedERKNS0_21VibrationDeviceHandleE", one),
        ("29IsOunceVibrationDeviceMountedERKNS0_21VibrationDeviceHandleE", zero),
        ("18SendVibrationValueERKNS0_21VibrationDeviceHandleERKNS0_14VibrationValueE", vib_one),
        ("6detail19SendVibrationValuesEPKNS0_21VibrationDeviceHandleEPKNS0_14VibrationValueEi", vib_many),
        ("23GetActualVibrationValueEPNS0_14VibrationValueERKNS0_21VibrationDeviceHandleE", vib_actual),
        ("20IsVibrationPermittedEv", one),
        ("27BeginForcedVibrationSessionEv", nothing),
        ("25EndForcedVibrationSessionEv", nothing),
        ("21InitializeTouchScreenEv", nothing),
        ("17InitializeGestureEv", nothing),
        ("16GetGestureStatesEPNS0_12GestureStateEi", gestures),
        ("18InitializeDebugPadEv", nothing),
        ("16GetDebugPadStateEPNS0_13DebugPadStateE", debugpad_state),
        ("17GetDebugPadStatesEPNS0_13DebugPadStateEi", debugpad_states),
        ("18InitializeKeyboardEv", nothing),
        ("16GetKeyboardStateEPNS0_13KeyboardStateE", keyboard_state),
        ("17GetKeyboardStatesEPNS0_13KeyboardStateEi", keyboard_states),
        ("15InitializeMouseEv", nothing),
        ("13GetMouseStateEPNS0_10MouseStateE", mouse_state),
        ("14GetMouseStatesEPNS0_10MouseStateEi", mouse_states),
    ] {
        r.force(&format!("{p}{n}"), W, f);
    }
    for &(n, one, many) in TOUCH_FNS {
        r.force(
            &format!("{p}19GetTouchScreenStateILm{n}EEEvPNS0_16TouchScreenStateIXT_EEE"),
            W,
            one,
        );
        r.force(
            &format!("{p}20GetTouchScreenStatesILm{n}EEEiPNS0_16TouchScreenStateIXT_EEEi"),
            W,
            many,
        );
    }
    super::hid_ext::register(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npad_states_and_players() {
        STYLES.store(STYLE_FULLKEY | STYLE_HANDHELD, Ordering::Relaxed);
        input::set_keyboard(0b1, [0, 0x7000, 0, 0]);
        let mut buf = vec![0u8; 0x28 * 3];
        let a = buf.as_mut_ptr() as u64;
        assert_eq!(states(a, 3, connected_id(), style_of(connected_id())), 3);
        let s0 = crate::guest::r64(a);
        assert_eq!(
            crate::guest::r64(a + 0x28),
            s0 - 1,
            "consecutive sampling numbers, newest first"
        );
        assert_eq!(
            crate::guest::r64(a + 8),
            0b1 | 1 << 17,
            "the A button and the left stick's up direction"
        );
        assert_eq!(r32(a + 0x20) & ATTR_CONNECTED, 1);
        fill(a, 3, STYLE_FULLKEY, 1);
        assert_eq!(r32(a + 0x20), 0, "no controller on npad No4");
        // a second controller on player 2 in a game that supports No1..No4
        input::PLAYERS_MASK.store(0b11, Ordering::Relaxed);
        IDS.store(0b1111 | 1 << 8, Ordering::Relaxed);
        input::PADS[1].set(1 << 1, [0; 4]);
        fill(a, 1, STYLE_FULLKEY, 1);
        assert_eq!(
            crate::guest::r64(a + 8),
            1 << 1,
            "player 2 reads its own pad on No2"
        );
        // a one-player game: player 2's pad feeds player 1
        IDS.store(1 | 1 << 8, Ordering::Relaxed);
        input::PADS[1].set(1 << 2, [0; 4]);
        input::set_keyboard(0, [0; 4]);
        let s = style_of(connected_id());
        fill(a, connected_id(), s, 1);
        assert_eq!(
            crate::guest::r64(a + 8) & 0xffff,
            1 << 2,
            "player 2's pad merged into player 1"
        );
        input::PLAYERS_MASK.store(1, Ordering::Relaxed);
        input::PADS[1].clear();
        IDS.store(0x3ff, Ordering::Relaxed);
    }
}
