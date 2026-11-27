//! The page's keyboard and gamepads as player 1's controller (docs/35 W3), the page's in-game menu (WEB-W9) and its
//! settings mailbox.
//!
//! The page (`src/web/input.js`, `menu.js`) writes a published block of 32-bit words with Atomics: `[0]` sequence
//! (bumped at every change), `[1]` focus (1 visible and focused, 2 visible behind another window, 3 hidden; keys are
//! released when it is not 1), `[2..10)` 256 key bits by Windows virtual-key code, `[10]` gamepads connected (a bit
//! each), `[11]` the in-game menu is open, then 4 gamepads of 8 words from `[16]`: `[0]` connected, `[1]` buttons
//! (bit = the standard mapping's button index), `[2..6)` axes 0-3 scaled to ±32767; `[48..56)` key presses latched
//! by the page until this side takes them (a press shorter than a tick counts).
//!
//! The other way (WEB-RUMBLE): `[56..64)` the game's vibration as motor levels the page plays on its gamepads, written
//! here every tick: player p (0-3) low motor at `[56 + 2p]`, high motor at `[57 + 2p]`, 0..65535. They are the
//! standard path's values (`RUMBLE`, after `RumbleSettings`; HD rumble claims only native Nintendo pads, so on the
//! web every vibration takes this path), reduced to one low and one high motor as a native rotating-motor pad gets
//! them (the larger of the two sides), and 0 while the game does not own input (the menu open, the page not
//! focused). `NXRS_CONTROLLER_SELFTEST=1`: player 1's low motor 0.5 s, then the high one 0.5 s, once gamepad 0 is
//! connected.
//!
//! The settings mailbox follows at `[64]`: `[64]` a sequence lock (odd while the page writes), `[65]` the text's
//! length in bytes, then up to `MAIL_BYTES` bytes from `[68]`: `key=value` lines (the engine's ini format), applied
//! with `nxconfig::set` (the same listeners as a native hotkey), so a change in the menu applies live.
//!
//! The input thread reads it every tick: the keys through the native front's default keys (`front/src/keys.rs`, the
//! F1 page's table) with `key_map`'s changes, gamepad 0 by position through `button_layout` / `button_map`
//! (`layout.rs`, as every native pad) with the `stick_deadzone` dead zone, into player 1's keyboard path. While the
//! menu is open the game is paused: no input, the guest clock held, its next present waits (the last image stays).

use super::layout;

/// The native front's default keys (`front/src/keys.rs`): VK code to guest button bits (0..15), or the stick keys
/// (40..47).
pub fn keymap(vk: u32) -> u64 {
    match vk {
        0x58 | 0x20 => 1 << 0, // X, Space: A
        0x5A => 1 << 1,        // Z: B
        0x43 => 1 << 2,        // C: X
        0x56 => 1 << 3,        // V: Y
        0x51 => 1 << 6,        // Q: L
        0x45 => 1 << 7,        // E: R
        0x31 => 1 << 8,        // 1: ZL
        0x33 => 1 << 9,        // 3: ZR
        0x0D => 1 << 10,       // Enter: Plus
        0x08 => 1 << 11,       // Backspace: Minus
        0x25 => 1 << 12,       // arrows: D-pad left, up, right, down
        0x26 => 1 << 13,
        0x27 => 1 << 14,
        0x28 => 1 << 15,
        0x41 => 1 << 40, // A: left stick left
        0x57 => 1 << 41, // W: up
        0x44 => 1 << 42, // D: right
        0x53 => 1 << 43, // S: down
        0x4A => 1 << 44, // J: right stick left
        0x49 => 1 << 45, // I: up
        0x4C => 1 << 46, // L: right
        0x4B => 1 << 47, // K: down
        _ => 0,
    }
}

/// A `key_map` key name to its Windows virtual-key code.
pub fn key_vk(name: &str) -> Option<u32> {
    let b = name.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(d) = name.strip_prefix("num") {
        if let Ok(n) = d.parse::<u32>() {
            return (n < 10).then_some(0x60 + n);
        }
    }
    Some(match name {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0D,
        "shift" | "lshift" => 0xA0,
        "rshift" => 0xA1,
        "ctrl" | "lctrl" => 0xA2,
        "rctrl" => 0xA3,
        "alt" | "lalt" => 0xA4,
        "ralt" => 0xA5,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "semicolon" => 0xBA,
        "equal" => 0xBB,
        "comma" => 0xBC,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "backquote" => 0xC0,
        "bracketleft" => 0xDB,
        "backslash" => 0xDC,
        "bracketright" => 0xDD,
        "quote" => 0xDE,
        _ => return None,
    })
}

/// A `key_map` guest name to its bits: a button (`layout::GUEST`), a stick direction (40..47), or none (0).
pub fn guest_bits(name: &str) -> Option<u64> {
    const STICK: [&str; 8] = [
        "lsleft", "lsup", "lsright", "lsdown", "rsleft", "rsup", "rsright", "rsdown",
    ];
    if name == "none" {
        return Some(0);
    }
    if let Some(i) = layout::GUEST.iter().position(|g| *g == name) {
        return Some(1 << i);
    }
    STICK.iter().position(|s| *s == name).map(|i| 1 << (40 + i))
}

/// The keyboard's table (VK code to bits): the default keys with `key_map`'s `key:guest` changes; the count of
/// entries not understood.
pub fn key_table(map: &str) -> (Vec<u64>, u32) {
    let mut t: Vec<u64> = (0..256).map(keymap).collect();
    let mut bad = 0;
    for tok in map.split([' ', ',', ';', '\t']).filter(|t| !t.is_empty()) {
        let tok = tok.to_ascii_lowercase();
        match tok
            .split_once(':')
            .and_then(|(k, g)| Some((key_vk(k)?, guest_bits(g)?)))
        {
            Some((vk, bits)) => t[vk as usize] = bits,
            None => bad += 1,
        }
    }
    (t, bad)
}

fn axis(b: u64, neg: u32, pos: u32) -> i32 {
    (if b >> pos & 1 != 0 { 32767 } else { 0 }) - (if b >> neg & 1 != 0 { 32767 } else { 0 })
}

/// Held keys (as [`keymap`] bits) to guest buttons and sticks.
pub fn keys_state(held: u64) -> (u64, [i32; 4]) {
    (
        held & 0xffff,
        [
            axis(held, 40, 42),
            axis(held, 43, 41),
            axis(held, 44, 46),
            axis(held, 47, 45),
        ],
    )
}

/// The standard gamepad mapping's buttons to positions in the Switch bit order `layout.rs` takes (0 right, 1 bottom,
/// 2 top, 3 left, stick clicks, L, R, ZL, ZR, Plus, Minus, D-pad left, up, right, down).
pub fn pad_positions(std_bits: u32) -> u64 {
    const POS: [u8; 16] = [
        1,  // 0 bottom
        0,  // 1 right
        3,  // 2 left
        2,  // 3 top
        6,  // 4 left bumper: L
        7,  // 5 right bumper: R
        8,  // 6 left trigger: ZL
        9,  // 7 right trigger: ZR
        11, // 8 back / view: Minus
        10, // 9 start / menu: Plus
        4,  // 10 left stick press
        5,  // 11 right stick press
        13, // 12 D-pad up
        15, // 13 down
        12, // 14 left
        14, // 15 right
    ];
    (0..16)
        .filter(|i| std_bits >> i & 1 != 0)
        .fold(0, |b, i| b | 1 << POS[i as usize])
}

/// The standard mapping's buttons to guest buttons through the player's layout and map.
pub fn pad_buttons(map: &layout::PadMap, std_bits: u32) -> u64 {
    layout::apply(map, pad_positions(std_bits))
}

/// The standard mapping's axes (x right, y down) to guest sticks (y up), with a dead zone of `dz_percent`.
pub fn pad_sticks(axes: [i32; 4], dz_percent: i64) -> [i32; 4] {
    let edge = (32767 * dz_percent.clamp(0, 30) / 100) as i32;
    let dz = |v: i32| {
        if v.abs() < edge.max(1) {
            0
        } else {
            v.clamp(-32767, 32767)
        }
    };
    [dz(axes[0]), -dz(axes[1]), dz(axes[2]), -dz(axes[3])]
}

/// The first word of the motor levels and the players they cover.
pub const RUMBLE_WORD: usize = 56;
pub const RUMBLE_PLAYERS: usize = 4;

/// A player's two sides to one low and one high motor level (0..1), as a native pad with two rotating motors gets
/// them (`Dev::rumble`: the larger side of each).
pub fn two_motors(v: [super::Vib; 2]) -> (f32, f32) {
    (
        v[0].amp_low.max(v[1].amp_low),
        v[0].amp_high.max(v[1].amp_high),
    )
}

/// A motor level (0..1) as the block's word (0..65535).
pub fn motor_word(x: f32) -> u32 {
    if x.is_finite() {
        (x.clamp(0.0, 1.0) * 65535.0 + 0.5) as u32
    } else {
        0
    }
}

/// The block's motor words for the first `RUMBLE_PLAYERS` players: `[low, high]` per player, all 0 when `on` is
/// false (the game does not own input, or `rumble` is 0).
pub fn motor_words(v: &[[super::Vib; 2]], on: bool) -> [u32; 2 * RUMBLE_PLAYERS] {
    let mut w = [0; 2 * RUMBLE_PLAYERS];
    if on {
        for (p, s) in v.iter().take(RUMBLE_PLAYERS).enumerate() {
            let (low, high) = two_motors(*s);
            w[2 * p] = motor_word(low);
            w[2 * p + 1] = motor_word(high);
        }
    }
    w
}

/// The self-test's levels (low, high) `t` seconds after it began: low 0.5 s, high 0.5 s, then over (None).
pub fn selftest_levels(t: f64) -> Option<(f32, f32)> {
    if t < 0.5 {
        Some((0.5, 0.0))
    } else if t < 1.0 {
        Some((0.0, 0.5))
    } else {
        None
    }
}

/// The settings mailbox's text to the pairs that differ from the current values.
pub fn mail_changes(text: &str, current: impl Fn(&str) -> String) -> Vec<(String, String)> {
    nxconfig::parse(text)
        .into_iter()
        .filter(|(k, v)| {
            let k = nxconfig::options::canonical(k);
            nxconfig::options::normalise(k, v).is_some_and(|n| n != current(k))
        })
        .collect()
}

#[cfg(target_os = "emscripten")]
mod block {
    use std::sync::atomic::{AtomicU32, Ordering::*};
    use std::sync::{Mutex, OnceLock};

    const MAIL: usize = 64;
    const MAIL_BYTES: usize = 4096;
    const WORDS: usize = MAIL + 4 + MAIL_BYTES / 4;

    fn block() -> &'static [AtomicU32] {
        static B: OnceLock<&'static [AtomicU32]> = OnceLock::new();
        B.get_or_init(|| {
            let b = crate::web::block(WORDS);
            // SAFETY: the block lives for the process
            unsafe { crate::web::nxweb_input_publish(b.as_ptr() as *const u32, WORDS as u32) };
            crate::log!(
                "input: the page's keyboard, gamepads, menu and settings through the input block at {:#x}",
                b.as_ptr() as usize
            );
            b
        })
    }

    /// The mailbox's text when the page wrote a new one since `last` (a consistent copy: the lock was even and
    /// unchanged around it).
    fn mail(b: &[AtomicU32], last: &mut u32) -> Option<String> {
        let s1 = b[MAIL].load(Acquire);
        if s1 == *last || s1 & 1 != 0 {
            return None;
        }
        let n = (b[MAIL + 1].load(Acquire) as usize).min(MAIL_BYTES);
        let mut bytes = Vec::with_capacity(n);
        for i in 0..n {
            let w = b[MAIL + 4 + i / 4].load(Relaxed);
            bytes.push((w >> (8 * (i % 4))) as u8);
        }
        if b[MAIL].load(Acquire) != s1 {
            return None;
        }
        *last = s1;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    struct Last {
        seq: u32,
        btn: u64,
        axes: [i32; 4],
        logged: u32,
        mail: u32,
        menu: bool,
        focus: u32,
        gen: u64,
        keys: Vec<u64>,
        dz: i64,
        rumble_on: bool,
        rlogged: u32,
        motors: [u32; 2 * super::RUMBLE_PLAYERS],
        /// the self-test: None when not asked for, Some(None) waiting for gamepad 0, Some(Some(t0)) running
        test: Option<Option<f64>>,
    }

    /// The game's vibration as motor levels into the block (words `[56..64)`), logged when they change.
    fn motors(b: &[AtomicU32], l: &mut Last, pad: bool) {
        let v = crate::input::RUMBLE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .v;
        let mut w = super::motor_words(&v, l.rumble_on && crate::input::active());
        if let Some(t) = l.test {
            let now = crate::input::motion::now_s();
            match t {
                None if pad => {
                    crate::log!(
                        "input: rumble self-test: player 1 low motor 0.5 s, high motor 0.5 s, stop"
                    );
                    l.test = Some(Some(now));
                }
                Some(t0) => match super::selftest_levels(now - t0) {
                    Some((low, high)) => {
                        w[0] = super::motor_word(low);
                        w[1] = super::motor_word(high);
                    }
                    None => {
                        crate::log!("input: rumble self-test finished");
                        l.test = None;
                    }
                },
                None => {}
            }
        }
        for (i, x) in w.iter().enumerate() {
            b[super::RUMBLE_WORD + i].store(*x, Release);
        }
        if w != l.motors {
            if crate::input::trace() || l.rlogged < 200 {
                l.rlogged += 1;
                crate::log!(
                    "input: rumble to the page: player 1 low {} high {} (of 65535)",
                    w[0],
                    w[1]
                );
            }
            l.motors = w;
        }
    }

    /// One read of the block into player 1's keyboard path (the input thread's tick).
    pub fn poll(map: &crate::input::layout::PadMap) {
        static LAST: Mutex<Option<Last>> = Mutex::new(None);
        let b = block();
        let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let l = g.get_or_insert_with(|| Last {
            seq: u32::MAX,
            btn: 0,
            axes: [0; 4],
            logged: 0,
            mail: 0,
            menu: false,
            focus: 0,
            gen: 0,
            keys: vec![],
            dz: 10,
            rumble_on: true,
            rlogged: 0,
            motors: [0; 2 * super::RUMBLE_PLAYERS],
            test: crate::input::env_flag("NXRS_CONTROLLER_SELFTEST").then_some(None),
        });
        // the menu's settings, applied as a native hotkey would
        if let Some(text) = mail(b, &mut l.mail) {
            let changes = super::mail_changes(&text, nxconfig::string);
            for (k, v) in &changes {
                nxconfig::set(k, v);
            }
            if !changes.is_empty() {
                let list: Vec<String> = changes.iter().map(|(k, v)| format!("{k}={v}")).collect();
                crate::log!("settings from the page: {}", list.join(" "));
            }
        }
        let gen = nxconfig::generation();
        let gen_changed = gen != l.gen;
        if gen_changed {
            l.gen = gen;
            let (t, bad) = super::key_table(&nxconfig::string("key_map"));
            if bad > 0 {
                crate::log!(
                    "input: key_map: {bad} entries not understood (key:guest, e.g. space:b)"
                );
            }
            l.keys = t;
            l.dz = nxconfig::int("stick_deadzone");
            // the menu's Rumble switch stops a vibration the game holds at once (the strength applies to its next)
            l.rumble_on = nxconfig::string("rumble").trim() != "0";
        }
        // the focus as the game sees it (pause_on_focus_loss) and the sound (mute_on_focus_loss)
        let focus = b[1].load(Acquire);
        crate::input::set_focus(focus == 1);
        if focus != l.focus || gen_changed {
            l.focus = focus;
            crate::services::oe::set_focus(if focus == 0 { 1 } else { focus as i32 });
        }
        // the menu pauses the game
        let menu = b[11].load(Acquire) != 0;
        if menu != l.menu {
            l.menu = menu;
            crate::clock::set_hold(menu);
            nxgfx::nvn::window::set_held(menu);
            crate::input::set_suspended(menu);
            crate::log!(
                "input: the page's menu {}",
                if menu {
                    "opened: the game is paused"
                } else {
                    "closed: the game runs"
                }
            );
        }
        let mut held = 0u64;
        for w in 0..8usize {
            // held now, or pressed since the last read (a press shorter than a tick)
            let bits = b[2 + w].load(Relaxed) | b[48 + w].swap(0, AcqRel);
            for i in 0..32u32 {
                if bits >> i & 1 != 0 {
                    held |= l
                        .keys
                        .get((w as u32 * 32 + i) as usize)
                        .copied()
                        .unwrap_or(0);
                }
            }
        }
        let (mut btn, mut axes) = super::keys_state(held);
        if b[16].load(Relaxed) != 0 {
            btn |= super::pad_buttons(map, b[17].load(Relaxed));
            let raw: [i32; 4] = std::array::from_fn(|i| b[18 + i].load(Relaxed) as i32);
            let p = super::pad_sticks(raw, l.dz);
            for i in 0..4 {
                axes[i] = crate::input::pick(axes[i], p[i]);
            }
        }
        if menu {
            (btn, axes) = (0, [0; 4]);
        }
        crate::input::set_keyboard(btn, axes);
        motors(b, l, b[16].load(Relaxed) != 0);
        let seq = b[0].load(Acquire);
        if seq != l.seq && (btn, axes) != (l.btn, l.axes) && l.logged < 64 {
            l.logged += 1;
            crate::log!("input: page keys/gamepad -> player 1 buttons {btn:#06x} sticks {axes:?} (focus {focus})");
        }
        l.seq = seq;
        l.btn = btn;
        l.axes = axes;
    }
}
#[cfg(target_os = "emscripten")]
pub use block::poll;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_pad_map_like_the_front() {
        // Space + W + right arrow: A, left stick up, D-pad right
        let held = keymap(0x20) | keymap(0x57) | keymap(0x27);
        assert_eq!(keys_state(held), (1 | 1 << 14, [0, 32767, 0, 0]));
        // standard bottom + right + start + D-pad down, by position (the switch layout)
        let sw = layout::build("switch", "").0;
        assert_eq!(
            pad_buttons(&sw, 1 | 1 << 1 | 1 << 9 | 1 << 13),
            1 << 1 | 1 << 0 | 1 << 10 | 1 << 15
        );
        assert_eq!(
            pad_sticks([1000, -32767, 32767, 40000], 10),
            [0, 32767, 32767, -32767]
        );
    }

    /// The web pad goes through button_layout and button_map as every native pad does.
    #[test]
    fn pad_follows_button_layout_and_map() {
        let xbox = layout::build("xbox", "").0;
        assert_eq!(pad_buttons(&xbox, 1), 1, "bottom is A by the Xbox labels");
        assert_eq!(pad_buttons(&xbox, 1 << 1), 1 << 1, "right is B");
        assert_eq!(pad_buttons(&xbox, 1 << 2), 1 << 2, "left is X");
        let swap = layout::build("switch", "south:a east:b l:zl").0;
        assert_eq!(pad_buttons(&swap, 1), 1, "south (bottom) to A");
        assert_eq!(pad_buttons(&swap, 1 << 1), 1 << 1, "east (right) to B");
        assert_eq!(pad_buttons(&swap, 1 << 4), 1 << 8, "left bumper to ZL");
    }

    #[test]
    fn dead_zone_follows_the_setting() {
        assert_eq!(pad_sticks([5000, 0, 0, 0], 10), [5000, 0, 0, 0]);
        assert_eq!(pad_sticks([5000, 0, 0, 0], 20), [0, 0, 0, 0]);
        assert_eq!(pad_sticks([200, 0, 0, 0], 0), [200, 0, 0, 0]);
        assert_eq!(
            pad_sticks([9000, 0, 0, 0], 99),
            [0, 0, 0, 0],
            "clamped to 30%"
        );
    }

    /// key_map changes the default keys: a key to another button, a key off, a stick direction.
    #[test]
    fn key_map_changes_the_keys() {
        let (t, bad) = key_table("space:b Z:none q:lsleft num4:rsdown f:zr nonsense x:what");
        assert_eq!(bad, 2);
        assert_eq!(t[0x20], 1 << 1, "space to B");
        assert_eq!(t[0x5A], 0, "Z off");
        assert_eq!(t[0x51], 1 << 40, "Q to the left stick's left");
        assert_eq!(t[0x64], 1 << 47);
        assert_eq!(t[0x46], 1 << 9);
        assert_eq!(t[0x58], 1, "X keeps its default A");
        let (d, _) = key_table("");
        assert!((0..256).all(|vk| d[vk] == keymap(vk as u32)));
        assert_eq!(key_vk("bracketleft"), Some(0xDB));
        assert_eq!(key_vk("7"), Some(0x37));
        assert_eq!(key_vk("num10"), None);
    }

    fn vib(low: f32, high: f32) -> crate::input::Vib {
        crate::input::Vib {
            amp_low: low,
            amp_high: high,
            ..Default::default()
        }
    }

    /// Two sides to two motors: the larger of each, as a native rotating-motor pad.
    #[test]
    fn two_sides_make_two_motors() {
        assert_eq!(two_motors([vib(0.2, 0.7), vib(0.6, 0.1)]), (0.6, 0.7));
        assert_eq!(two_motors([Default::default(); 2]), (0.0, 0.0));
    }

    /// Levels to the block's words: 0..65535, clamped, a bad value silent; players at [56 + 2p], [57 + 2p].
    #[test]
    fn motor_levels_encode_into_the_block() {
        assert_eq!(motor_word(0.0), 0);
        assert_eq!(motor_word(1.0), 65535);
        assert_eq!(motor_word(0.5), 32768);
        assert_eq!(motor_word(2.0), 65535);
        assert_eq!(motor_word(-1.0), 0);
        assert_eq!(motor_word(f32::NAN), 0);
        let v = [
            [vib(1.0, 0.0), vib(0.0, 0.0)],
            [vib(0.0, 0.0), vib(0.0, 0.25)],
            [Default::default(); 2],
            [vib(0.5, 0.5); 2],
            [vib(1.0, 1.0); 2], // player 5: not on the page
        ];
        let w = motor_words(&v, true);
        assert_eq!(w, [65535, 0, 0, 16384, 0, 0, 32768, 32768]);
        assert_eq!(
            motor_words(&v, false),
            [0; 8],
            "the game does not own input: silent"
        );
        // the words sit between the key latches [48..56) and the mailbox [64..)
        assert_eq!(RUMBLE_WORD, 56);
        assert!(RUMBLE_WORD + 2 * RUMBLE_PLAYERS <= 64);
    }

    /// The self-test: low motor 0.5 s, high motor 0.5 s, then over.
    #[test]
    fn selftest_runs_low_then_high() {
        assert_eq!(selftest_levels(0.0), Some((0.5, 0.0)));
        assert_eq!(selftest_levels(0.49), Some((0.5, 0.0)));
        assert_eq!(selftest_levels(0.5), Some((0.0, 0.5)));
        assert_eq!(selftest_levels(0.99), Some((0.0, 0.5)));
        assert_eq!(selftest_levels(1.0), None);
    }

    /// The mailbox applies only what differs, by the canonical key and its normalised value.
    #[test]
    fn mail_changes_only_what_differs() {
        let cur = |k: &str| match k {
            "resolution" => "1".to_string(),
            "docked" => "1".to_string(),
            _ => String::new(),
        };
        let c = mail_changes(
            "[Display]\ndocked=1\n[Graphics]\nresolution=0.50\nresolucion=1\nkey_map=Space:B\nvsync=1\n",
            cur,
        );
        assert_eq!(
            c,
            vec![
                ("resolution".to_string(), "0.50".to_string()),
                ("key_map".to_string(), "Space:B".to_string())
            ]
        );
    }
}
