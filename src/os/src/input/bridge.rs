//! `nxrs-build pad-bridge` (WEB-PADBRIDGE): a web game played through the launcher sends its pads' output (the
//! input block's motor words and whether the game owns input) to the launcher's server, which writes it here, one
//! line each, on stdin. This process drives the real pads with the native input layer, exactly as a native run's
//! input thread does: its own scan and player slots, the player lights while input is active (`set_lights`), the
//! motors through `Dev::rumble` (DualSense / DualShock 4 output reports, XInput, Nintendo standard rumble).
//!
//! Line: `pad <active 0|1> <low 1> <high 1> <low 2> <high 2> <low 3> <high 3> <low 4> <high 4>`, levels 0..65535
//! (the page's levels already carry the rumble settings). Input stays the browser's: bridge mode opens every pad
//! shared, leaves Sony pads on Bluetooth and Switch 2 pads alone (mod.rs) and turns HD rumble off (the page carries
//! the standard path's two levels). Watchdog: no line for `WATCHDOG` -> motors stop and the lights go out until
//! the next line. End of stdin: everything quiet (`shutdown`), exit 0.

use super::{ilog, set_focus, set_levels, shutdown, start, BRIDGE};
use std::io::BufRead;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

/// The page sends a line at every change and every 250 ms; four missed ones mean it is gone or frozen.
pub const WATCHDOG: Duration = Duration::from_millis(1000);
pub const PLAYERS: usize = 4;

/// One line from the page.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PadLine {
    pub active: bool,
    /// (low, high) per player, 0..65535
    pub levels: [(u16, u16); PLAYERS],
}

pub fn parse(line: &str) -> Option<PadLine> {
    let mut it = line.split_ascii_whitespace();
    if it.next()? != "pad" {
        return None;
    }
    let active = match it.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let mut levels = [(0u16, 0u16); PLAYERS];
    for l in levels.iter_mut() {
        *l = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    }
    it.next().is_none().then_some(PadLine { active, levels })
}

/// The line put into the input layer: focus = active (as the window's focus natively), each player's levels.
fn apply(l: &PadLine) {
    set_focus(l.active);
    for (p, &(low, high)) in l.levels.iter().enumerate() {
        set_levels(p, low as f32 / 65535.0, high as f32 / 65535.0);
    }
}

/// Runs the bridge until stdin ends; the exit code.
pub fn run() -> i32 {
    BRIDGE.store(true, Ordering::Release);
    nxconfig::apply_arg("hd_rumble=0");
    set_focus(false);
    ilog!(
        "pad bridge: driving the pads' rumble and lights for a web page (watchdog {} ms)",
        WATCHDOG.as_millis()
    );
    start();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            if tx.send(l).is_err() {
                return;
            }
        }
    });
    let (mut last, mut live, mut bad, mut noted) = (PadLine::default(), false, 0u32, 0u32);
    loop {
        match rx.recv_timeout(WATCHDOG) {
            Ok(text) => match parse(&text) {
                Some(l) => {
                    if !live || l != last {
                        if noted < 1000 {
                            noted += 1;
                            ilog!(
                                "pad bridge: from the page: active={} levels {:?}",
                                l.active as u8,
                                l.levels
                            );
                        }
                        apply(&l);
                    }
                    last = l;
                    live = true;
                }
                None => {
                    if bad < 8 {
                        bad += 1;
                        ilog!(
                            "pad bridge: not a pad line: {:?}",
                            text.chars().take(80).collect::<String>()
                        );
                    }
                }
            },
            Err(RecvTimeoutError::Timeout) => {
                if live {
                    live = false;
                    ilog!(
                        "pad bridge: no line from the page for {} ms: motors stopped, lights off",
                        WATCHDOG.as_millis()
                    );
                    apply(&PadLine::default());
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    ilog!("pad bridge: the page's connection ended: motors stopped, lights off");
    apply(&PadLine::default());
    // one input tick (4 ms) carries the zeros and the lights off before the devices close
    std::thread::sleep(Duration::from_millis(20));
    shutdown();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_lines() {
        let l = parse("pad 1 32768 0 0 0 0 0 0 65535").unwrap();
        assert!(l.active);
        assert_eq!(l.levels[0], (32768, 0));
        assert_eq!(l.levels[3], (0, 65535));
        assert_eq!(parse("pad 0 0 0 0 0 0 0 0 0"), Some(PadLine::default()));
        assert_eq!(parse("pad 2 0 0 0 0 0 0 0 0"), None, "active is 0 or 1");
        assert_eq!(parse("pad 1 0 0 0 0 0 0 0"), None, "eight levels");
        assert_eq!(parse("pad 1 0 0 0 0 0 0 0 0 9"), None, "nothing after them");
        assert_eq!(parse("pad 1 65536 0 0 0 0 0 0 0"), None, "16-bit levels");
        assert_eq!(parse("rumble 1 0 0 0 0 0 0 0 0"), None);
    }
}
