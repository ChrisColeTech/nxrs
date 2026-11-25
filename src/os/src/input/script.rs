//! Scripted input, for tests and the harness:
//! - `NXRS_INPUT_RECORD=<file>`: every change of player 1's physical input (keyboard, mouse, controller) appended as
//!   `<ms> <buttons hex> <lx> <ly> <rx> <ry>`, ms from the first sample.
//! - `NXRS_INPUT_REPLAY=<file>`: once the file exists (it may be written while the game runs) its lines replace
//!   player 1 from that moment, each state held until the next line's ms; after the last line the real input is back.
//!   Rewritten (another time or size), it plays again. `#` lines are comments.
//! - `NXRS_KEYS=<steps>` (or `@<file>`): "seconds:key[+key][~hold][*count/interval],..." as nxnative's DM_KEYS: keys
//!   A B X Y LS RS L R ZL ZR Plus Minus Left Up Right Down, LLeft LUp LRight LDown / RLeft... (a stick pushed), L<x>/<y>
//!   and R<x>/<y> (a stick part way, -1..1, y up); held 0.15 s by default.
//! - The control pipe's `press <keys>[~hold]` and `P<n> <keys>[~hold]` (player n).

use super::{ilog, Path, PLAYERS, REPLAYING, SCRIPT, SCRIPT_P, SCRIPT_PLAYERS};
use std::io::Write;
use std::sync::atomic::Ordering;

pub const KEY_NAMES: [&str; 16] = [
    "A", "B", "X", "Y", "LS", "RS", "L", "R", "ZL", "ZR", "Plus", "Minus", "Left", "Up", "Right",
    "Down",
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    pub at: f64,
    pub hold: f64,
    pub btn: u64,
    pub axes: [i32; 4],
}

/// "key[+key]" into buttons and sticks.
pub fn parse_keys(spec: &str) -> Result<(u64, [i32; 4]), String> {
    let mut b = 0u64;
    let mut a = [0i32; 4];
    for k in spec.split('+').filter(|k| !k.is_empty()) {
        if let Some(i) = KEY_NAMES.iter().position(|n| n.eq_ignore_ascii_case(k)) {
            b |= 1 << i;
            continue;
        }
        let dirs = [
            ("Left", -32767, 0),
            ("Up", 0, 32767),
            ("Right", 32767, 0),
            ("Down", 0, -32767),
        ];
        let mut found = false;
        for (stick, off) in [('L', 0usize), ('R', 2)] {
            for (d, x, y) in dirs {
                if k.len() == d.len() + 1 && k.starts_with(stick) && k[1..].eq_ignore_ascii_case(d)
                {
                    if x != 0 {
                        a[off] = x;
                    }
                    if y != 0 {
                        a[off + 1] = y;
                    }
                    found = true;
                }
            }
            if !found && k.starts_with(stick) && k.contains('/') {
                let (x, y) = k[1..].split_once('/').unwrap();
                if let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) {
                    if x.abs() <= 1.0 && y.abs() <= 1.0 {
                        a[off] = (x * 32767.0).round() as i32;
                        a[off + 1] = (y * 32767.0).round() as i32;
                        found = true;
                    }
                }
            }
        }
        if !found {
            return Err(format!("unknown key name {k}"));
        }
    }
    Ok((b, a))
}

/// A step list into steps, in time order.
pub fn parse_steps(spec: &str) -> (Vec<Step>, Vec<String>) {
    let mut out = vec![];
    let mut errs = vec![];
    for tok in spec
        .split([',', ' ', '\t', '\r', '\n'])
        .filter(|t| !t.is_empty())
    {
        let Some((t, rest)) = tok.split_once(':') else {
            errs.push(format!("step lacks 'seconds:': {tok}"));
            continue;
        };
        let Ok(at) = t.parse::<f64>() else {
            errs.push(format!("bad time in {tok}"));
            continue;
        };
        let (rest, rep) = match rest.split_once('*') {
            Some((r, x)) => (r, Some(x)),
            None => (rest, None),
        };
        let (keys, hold) = match rest.split_once('~') {
            Some((k, h)) => (k, h.parse::<f64>().unwrap_or(0.15)),
            None => (rest, 0.15),
        };
        let (count, every) = match rep {
            Some(x) => {
                let (c, e) = x.split_once('/').unwrap_or((x, "1"));
                (
                    c.parse::<u32>().unwrap_or(1).max(1),
                    e.parse::<f64>().unwrap_or(1.0),
                )
            }
            None => (1, 0.0),
        };
        match parse_keys(keys) {
            Ok((btn, axes)) => {
                for i in 0..count {
                    out.push(Step {
                        at: at + i as f64 * every,
                        hold,
                        btn,
                        axes,
                    });
                }
            }
            Err(e) => errs.push(e),
        }
    }
    out.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap());
    (out, errs)
}

fn press(path: &'static Path, btn: u64, axes: [i32; 4], hold: f64) {
    path.set(btn, axes);
    std::thread::sleep(std::time::Duration::from_secs_f64(hold.max(0.0)));
    path.set(0, [0; 4]);
}

/// NXRS_KEYS on a thread of its own, and the control pipe's commands.
pub fn start_keys() {
    register_commands();
    let Some(spec) = super::env_str("NXRS_KEYS") else {
        return;
    };
    let text = match spec.strip_prefix('@') {
        Some(f) => match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(e) => {
                ilog!("[RED] NXRS_KEYS: cannot read {f}: {e}");
                return;
            }
        },
        None => spec,
    };
    let (steps, errs) = parse_steps(&text);
    for e in errs {
        ilog!("[RED] NXRS_KEYS: {e}");
    }
    if steps.is_empty() {
        return;
    }
    ilog!("NXRS_KEYS: {} steps", steps.len());
    std::thread::Builder::new()
        .name("NXRS_KEYS".into())
        .spawn(move || {
            let t0 = std::time::Instant::now();
            for s in steps {
                let el = t0.elapsed().as_secs_f64();
                if s.at > el {
                    std::thread::sleep(std::time::Duration::from_secs_f64(s.at - el));
                }
                crate::log!(
                    "NXRS_KEYS: {:.2} press {:#x} (hold {:.2} s)",
                    s.at,
                    s.btn,
                    s.hold
                );
                press(&SCRIPT, s.btn, s.axes, s.hold);
            }
        })
        .ok();
}

fn register_commands() {
    nxconfig::ctl::register(
        "press",
        "<keys>[~seconds]: presses player 1's keys (A+B, LUp, L0.5/-1 ...), 0.15 s by default",
        |a| {
            let spec = a.join("+");
            let (keys, hold) = match spec.split_once('~') {
                Some((k, h)) => (k.to_string(), h.parse().unwrap_or(0.15)),
                None => (spec.clone(), 0.15),
            };
            match parse_keys(&keys) {
                Ok((b, ax)) => {
                    crate::log!("ctl: press {b:#x} (hold {hold:.2} s)");
                    press(&SCRIPT, b, ax, hold);
                    format!("pressed {b:#x} for {hold:.2} s")
                }
                Err(e) => e,
            }
        },
    );
    for p in 1..PLAYERS {
        let name: &'static str = Box::leak(format!("P{}", p + 1).into_boxed_str());
        nxconfig::ctl::register(
            name,
            "<keys>[~seconds]: that player's scripted controller (connected from its first press)",
            move |a| {
                let spec = a.join("+");
                let (keys, hold) = match spec.split_once('~') {
                    Some((k, h)) => (k.to_string(), h.parse().unwrap_or(0.15)),
                    None => (spec.clone(), 0.15),
                };
                match parse_keys(&keys) {
                    Ok((b, ax)) => {
                        if SCRIPT_PLAYERS.fetch_or(1 << p, Ordering::AcqRel) >> p & 1 == 0 {
                            crate::log!("ctl: player {}'s scripted controller connected", p + 1);
                        }
                        press(&SCRIPT_P[p], b, ax, hold);
                        format!("player {} pressed {b:#x} for {hold:.2} s", p + 1)
                    }
                    Err(e) => e,
                }
            },
        );
    }
    nxconfig::ctl::register("pads", ": the controllers, their players and state", |_| {
        let mut s = String::new();
        for (n, p, b, a) in super::devices() {
            s.push_str(&format!(
                "{n}: player {} buttons {b:#06x} sticks {a:?}\n",
                p.map(|p| (p + 1).to_string()).unwrap_or("-".into())
            ));
        }
        if s.is_empty() {
            s = "no controllers\n".into();
        }
        s
    });
}

/// The input recording.
pub struct Recorder {
    f: std::io::BufWriter<std::fs::File>,
    t0: Option<f64>,
    last: Option<(u64, [i32; 4])>,
}

impl Recorder {
    pub fn open(path: &str) -> Option<Recorder> {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(f) => {
                ilog!("input: recording to {path}");
                Some(Recorder {
                    f: std::io::BufWriter::new(f),
                    t0: None,
                    last: None,
                })
            }
            Err(e) => {
                ilog!("[RED] input: cannot record to {path}: {e}");
                None
            }
        }
    }
    pub fn note(&mut self, now: f64, b: u64, a: [i32; 4]) {
        if self.last == Some((b, a)) {
            return;
        }
        let t0 = *self.t0.get_or_insert(now);
        self.last = Some((b, a));
        let _ = writeln!(
            self.f,
            "{} {:x} {} {} {} {}",
            ((now - t0) * 1000.0) as u64,
            b & 0xffff,
            a[0],
            a[1],
            a[2],
            a[3]
        );
        let _ = self.f.flush();
    }
    pub fn flush(&mut self) {
        let _ = self.f.flush();
    }
}

/// The input replay.
pub struct Replay {
    path: String,
    stamp: Option<(std::time::SystemTime, u64)>,
    lines: Vec<(u64, u64, [i32; 4])>,
    start: f64,
    at: usize,
    next_check: f64,
}

pub fn parse_record(text: &str) -> Vec<(u64, u64, [i32; 4])> {
    let mut v = vec![];
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        let (Ok(ms), Ok(b)) = (
            f[0].parse::<u64>(),
            u64::from_str_radix(f[1].trim_start_matches("0x"), 16),
        ) else {
            continue;
        };
        let a: [i32; 4] = std::array::from_fn(|i| f[2 + i].parse().unwrap_or(0));
        v.push((ms, b, a));
    }
    v
}

impl Replay {
    pub fn new(path: String) -> Replay {
        ilog!("input: replaying {path} when it appears");
        Replay {
            path,
            stamp: None,
            lines: vec![],
            start: 0.0,
            at: 0,
            next_check: 0.0,
        }
    }
    pub fn tick(&mut self, now: f64) {
        if now >= self.next_check {
            self.next_check = now + 0.2;
            if let Ok(m) = std::fs::metadata(&self.path) {
                let st = (m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len());
                if self.stamp != Some(st) {
                    self.stamp = Some(st);
                    self.lines =
                        parse_record(&std::fs::read_to_string(&self.path).unwrap_or_default());
                    self.start = now;
                    self.at = 0;
                    ilog!(
                        "input: replay of {} ({} states) starts",
                        self.path,
                        self.lines.len()
                    );
                    REPLAYING.store(!self.lines.is_empty(), Ordering::Release);
                }
            }
        }
        if self.lines.is_empty() {
            return;
        }
        let ms = ((now - self.start) * 1000.0) as u64;
        while self.at + 1 < self.lines.len() && self.lines[self.at + 1].0 <= ms {
            self.at += 1;
        }
        if self.at + 1 >= self.lines.len() && ms > self.lines[self.at].0 + 100 {
            ilog!(
                "input: replay of {} finished; the real input is back",
                self.path
            );
            self.lines.clear();
            SCRIPT.set(0, [0; 4]);
            REPLAYING.store(false, Ordering::Release);
            return;
        }
        let (_, b, a) = self.lines[self.at];
        if SCRIPT.peek() != (b, a) {
            SCRIPT.set(b, a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn steps() {
        let (s, e) = parse_steps("8:A,12:A+B~0.5, 15:LUp 20:L0.5/-1*3/2");
        assert!(e.is_empty(), "{e:?}");
        assert_eq!(s.len(), 6);
        assert_eq!(s[1].btn, 3);
        assert_eq!(s[1].hold, 0.5);
        assert_eq!(s[2].axes, [0, 32767, 0, 0]);
        assert_eq!(s[3].axes[..2], [16384, -32767]);
        assert_eq!(s[5].at, 24.0);
        assert!(parse_keys("Q").is_err());
    }
    #[test]
    fn record_lines() {
        let r = parse_record("# x\n0 1 0 0 0 0\n250 0 0 32767 0 0\n");
        assert_eq!(r, vec![(0, 1, [0; 4]), (250, 0, [0, 32767, 0, 0])]);
    }
}
