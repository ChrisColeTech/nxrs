//! The page's keyboard and gamepads as player 1's controller (docs/35 W3).
//!
//! The page (`src/web/input.js`) writes a published block of 32-bit words with Atomics: `[0]` sequence (bumped at
//! every change), `[1]` focus (1 while the page is visible and focused; keys are released on blur), `[2..10)` 256 key
//! bits by Windows virtual-key code, `[10]` gamepads connected (a bit each), then 4 gamepads of 8 words from `[16]`:
//! `[0]` connected, `[1]` buttons (bit = the standard mapping's button index), `[2..6)` axes 0-3 scaled to ±32767;
//! `[40..48)` key presses latched by the page until this side takes them (a press shorter than a tick counts).
//! The input thread reads it every tick: the keys through the native front's default keys (`front/src/keys.rs`, the
//! F1 page's table), gamepad 0 by position like every other pad, into player 1's keyboard path.

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

/// The standard gamepad mapping's buttons by position to guest buttons (Switch positions: bottom B, right A, left Y,
/// top X).
pub fn pad_buttons(std_bits: u32) -> u64 {
    const MAP: [u64; 16] = [
        1 << 1,  // 0 bottom: B
        1 << 0,  // 1 right: A
        1 << 3,  // 2 left: Y
        1 << 2,  // 3 top: X
        1 << 6,  // 4 left bumper: L
        1 << 7,  // 5 right bumper: R
        1 << 8,  // 6 left trigger: ZL
        1 << 9,  // 7 right trigger: ZR
        1 << 11, // 8 back / view: Minus
        1 << 10, // 9 start / menu: Plus
        1 << 4,  // 10 left stick press
        1 << 5,  // 11 right stick press
        1 << 13, // 12 D-pad up
        1 << 15, // 13 down
        1 << 12, // 14 left
        1 << 14, // 15 right
    ];
    (0..16)
        .filter(|i| std_bits >> i & 1 != 0)
        .fold(0, |b, i| b | MAP[i as usize])
}

/// The standard mapping's axes (x right, y down) to guest sticks (y up), with a 10% dead zone.
pub fn pad_sticks(axes: [i32; 4]) -> [i32; 4] {
    let dz = |v: i32| {
        if v.abs() < 3277 {
            0
        } else {
            v.clamp(-32767, 32767)
        }
    };
    [dz(axes[0]), -dz(axes[1]), dz(axes[2]), -dz(axes[3])]
}

#[cfg(target_os = "emscripten")]
mod block {
    use std::sync::atomic::{AtomicU32, Ordering::*};
    use std::sync::{Mutex, OnceLock};

    const WORDS: usize = 64;

    fn block() -> &'static [AtomicU32] {
        static B: OnceLock<&'static [AtomicU32]> = OnceLock::new();
        B.get_or_init(|| {
            let b = crate::web::block(WORDS);
            // SAFETY: the block lives for the process
            unsafe { crate::web::nxweb_input_publish(b.as_ptr() as *const u32, WORDS as u32) };
            crate::log!(
                "input: the page's keyboard and gamepads through the input block at {:#x}",
                b.as_ptr() as usize
            );
            b
        })
    }

    /// One read of the block into player 1's keyboard path (the input thread's tick).
    pub fn poll() {
        static LAST: Mutex<(u32, u64, [i32; 4], u32)> = Mutex::new((u32::MAX, 0, [0; 4], 0));
        let b = block();
        let focus = b[1].load(Acquire) != 0;
        crate::input::set_focus(focus);
        let mut held = 0u64;
        for w in 0..8usize {
            // held now, or pressed since the last read (a press shorter than a tick)
            let bits = b[2 + w].load(Relaxed) | b[40 + w].swap(0, AcqRel);
            for i in 0..32u32 {
                if bits >> i & 1 != 0 {
                    held |= super::keymap(w as u32 * 32 + i);
                }
            }
        }
        let (mut btn, mut axes) = super::keys_state(held);
        if b[16].load(Relaxed) != 0 {
            btn |= super::pad_buttons(b[17].load(Relaxed));
            let raw: [i32; 4] = std::array::from_fn(|i| b[18 + i].load(Relaxed) as i32);
            let p = super::pad_sticks(raw);
            for i in 0..4 {
                axes[i] = crate::input::pick(axes[i], p[i]);
            }
        }
        crate::input::set_keyboard(btn, axes);
        let seq = b[0].load(Acquire);
        let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if seq != l.0 && (btn, axes) != (l.1, l.2) && l.3 < 64 {
            l.3 += 1;
            crate::log!("input: page keys/gamepad -> player 1 buttons {btn:#06x} sticks {axes:?} (focus {focus})");
        }
        l.0 = seq;
        l.1 = btn;
        l.2 = axes;
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
        // standard bottom + right + start + D-pad down
        assert_eq!(
            pad_buttons(1 | 1 << 1 | 1 << 9 | 1 << 13),
            1 << 1 | 1 << 0 | 1 << 10 | 1 << 15
        );
        assert_eq!(
            pad_sticks([1000, -32767, 32767, 40000]),
            [0, 32767, 32767, -32767]
        );
    }
}
