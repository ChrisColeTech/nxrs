//! Log lines in the engine's format, `[   t.tt] [vk] ...` (seconds since the process's first line), which the
//! launcher's pacing parser reads. Red lines (something unimplemented or wrong) start with `[RED] `.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

type Sink = Box<dyn Fn(&str) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();
static T0: OnceLock<Instant> = OnceLock::new();

/// Where log lines go (the runtime's `<name>.log`); stderr until set.
pub fn set_sink(f: impl Fn(&str) + Send + Sync + 'static) {
    let _ = SINK.set(Box::new(f));
}

pub fn elapsed() -> f64 {
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

pub fn line(msg: &str) {
    let l = format!("[{:7.2}] {msg}", elapsed());
    match SINK.get() {
        Some(f) => f(&l),
        None => eprintln!("{l}"),
    }
}

pub fn red(msg: &str) {
    line(&format!("[RED] {msg}"));
}

/// A red line printed once per distinct text.
pub fn red_once(msg: &str) {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap();
    if g.get_or_insert_with(HashSet::new).insert(msg.to_string()) {
        drop(g);
        red(msg);
    }
}

#[macro_export]
macro_rules! vklog {
    ($($t:tt)*) => { $crate::log::line(&format!("[vk] {}", format_args!($($t)*))) };
}
#[macro_export]
macro_rules! nvnlog {
    ($($t:tt)*) => { $crate::log::line(&format!("[nvn] {}", format_args!($($t)*))) };
}
