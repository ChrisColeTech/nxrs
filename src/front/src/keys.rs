//! The keyboard and mouse as player 1's controller (Turbo's default keys, shown on the F1 page):
//!
//! | Keys | Controller |
//! |---|---|
//! | arrows | D-pad |
//! | W A S D | left stick |
//! | I J K L (or the mouse, `mouse_right_stick=1`) | right stick |
//! | X / Space, Z, C, V | A, B, X, Y |
//! | Q, E, 1, 3 | L, R, ZL, ZR |
//! | Enter, Backspace | Plus, Minus |
//!
//! The mouse: raw motion summed between reads, turned into a stick deflection by `mouse_sensitivity` on Turbo's curve
//! (10% = 5x, 100% about 14x, 500% = 1250x of a count per 0.5 ms) and `mouse_invert` (1 horizontal, 2 vertical, 3 both).

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

/// Held keys as bits: 0..15 controller buttons, 40..47 the stick keys.
pub static HELD: AtomicU64 = AtomicU64::new(0);
pub static MOUSE_DX: AtomicI32 = AtomicI32::new(0);
pub static MOUSE_DY: AtomicI32 = AtomicI32::new(0);

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
        0x25 => 1 << 12,
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

/// Turbo's sensitivity curve: percent 10..500 → speed factor 5..1250.
pub fn mouse_factor(percent: f64) -> f64 {
    5.0 * 250f64.powf((percent - 10.0) / 490.0)
}

pub struct Mouse {
    last: Option<std::time::Instant>,
    rx: i32,
    ry: i32,
}

impl Mouse {
    pub const fn new() -> Mouse {
        Mouse {
            last: None,
            rx: 0,
            ry: 0,
        }
    }
    /// The right stick from the mouse motion since the last read (every 8 ms at most).
    pub fn stick(&mut self, focused: bool) -> (i32, i32) {
        let now = std::time::Instant::now();
        if !focused {
            self.rx = 0;
            self.ry = 0;
            self.last = Some(now);
            MOUSE_DX.store(0, Ordering::Relaxed);
            MOUSE_DY.store(0, Ordering::Relaxed);
            return (0, 0);
        }
        let last = *self.last.get_or_insert(now);
        let el = now.duration_since(last).as_nanos() as f64;
        if el >= 8_000_000.0 {
            let dx = MOUSE_DX.swap(0, Ordering::Relaxed) as f64;
            let dy = MOUSE_DY.swap(0, Ordering::Relaxed) as f64;
            let pct = (nxconfig::f32("mouse_sensitivity") as f64 * 100.0).clamp(10.0, 500.0);
            let scale = 32767.0 * 500_000.0 * mouse_factor(pct) / el;
            self.rx = (dx * scale).clamp(-32767.0, 32767.0) as i32;
            self.ry = (-dy * scale).clamp(-32767.0, 32767.0) as i32;
            self.last = Some(now);
            // the first deflections of each burst of motion, logged (a run shows the mouse reaching the stick)
            static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if (dx != 0.0 || dy != 0.0) && SAID.fetch_add(1, Ordering::Relaxed) < 24 {
                crate::flog!(
                    "input: mouse raw motion {dx:+} {dy:+} over {:.1} ms -> right stick {} {} (sensitivity {pct:.0}%; invert {} applied after)",
                    el / 1e6,
                    self.rx,
                    self.ry,
                    nxconfig::int("mouse_invert") & 3
                );
            }
        }
        let inv = nxconfig::int("mouse_invert") & 3;
        (
            if inv & 1 != 0 { -self.rx } else { self.rx },
            if inv & 2 != 0 { -self.ry } else { self.ry },
        )
    }
}

impl Default for Mouse {
    fn default() -> Self {
        Mouse::new()
    }
}

/// Player 1's keyboard (and mouse) state: guest buttons and sticks.
pub fn state(held: u64, mouse: Option<(i32, i32)>) -> (u64, [i32; 4]) {
    let b = held & 0xffff;
    let mut a = [
        axis(held, 40, 42),
        axis(held, 43, 41),
        axis(held, 44, 46),
        axis(held, 47, 45),
    ];
    if let Some((x, y)) = mouse {
        a[2] = x;
        a[3] = y;
    }
    (b, a)
}

/// The ini's hidden `script` (Turbo backend_win32.c, test input as the X11 backend has it): `ms:hex,ms:hex,...`, each
/// entry holds those controller buttons (hex, as the guest's button bits) for 300 ms from that many milliseconds after
/// start. The buttons held at `elapsed_ms`.
pub fn script_buttons(spec: &str, elapsed_ms: u64) -> u64 {
    let mut b = 0;
    for e in spec.split(',') {
        let Some((t, m)) = e.split_once(':') else {
            break;
        };
        let Ok(t) = t.trim().parse::<u64>() else {
            break;
        };
        let m = m.trim();
        let m = u64::from_str_radix(
            m.strip_prefix("0x")
                .or_else(|| m.strip_prefix("0X"))
                .unwrap_or(m),
            16,
        )
        .unwrap_or(0);
        if elapsed_ms >= t && elapsed_ms < t + 300 {
            b |= m;
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turbo's script: "ms:hex" entries, each held 300 ms.
    #[test]
    fn script_holds_buttons_300_ms() {
        let s = "1000:1,1200:0x10,5000:400";
        assert_eq!(script_buttons(s, 999), 0);
        assert_eq!(script_buttons(s, 1000), 1);
        assert_eq!(script_buttons(s, 1250), 0x11, "both held");
        assert_eq!(script_buttons(s, 1300), 0x10);
        assert_eq!(script_buttons(s, 5299), 0x400);
        assert_eq!(script_buttons(s, 5300), 0);
        assert_eq!(script_buttons("", 0), 0);
        assert_eq!(
            script_buttons("x:1,0:1", 100),
            0,
            "stops at the first entry it cannot read, as strtoull"
        );
    }
    #[test]
    fn keyboard() {
        let h = keymap(0x58) | keymap(0x57) | keymap(0x4C);
        assert_eq!(state(h, None), (1, [0, 32767, 32767, 0]));
        assert_eq!(
            state(h, Some((-5, 7))).1,
            [0, 32767, -5, 7],
            "the mouse drives the right stick"
        );
        assert!(
            (mouse_factor(10.0) - 5.0).abs() < 1e-9 && (mouse_factor(500.0) - 1250.0).abs() < 1e-6
        );
        assert!((mouse_factor(100.0) - 13.7).abs() < 0.2);
    }
}
