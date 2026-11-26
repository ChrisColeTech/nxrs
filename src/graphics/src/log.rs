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

thread_local! {
    static MUTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Red lines of this thread are dropped while muted: a translation thrown away (a segment of a parallel translation
/// that gave up at the gate, `vk::gate`) reports what it skipped because it gave up, not anything wrong.
pub fn mute(on: bool) {
    MUTED.with(|m| m.set(on));
}

/// This thread's red lines are muted (a translation that will be thrown away): its anomalies are not real either.
pub fn muted() -> bool {
    MUTED.with(|m| m.get())
}

pub fn red(msg: &str) {
    if MUTED.with(|m| m.get()) {
        return;
    }
    line(&format!("[RED] {msg}"));
    // a fault capture of the frame it happened in (NXGFX_FAULT_SHOTS; nothing when off)
    crate::vk::shot::fault::note("red", msg);
}

/// A red line printed once per distinct text.
pub fn red_once(msg: &str) {
    if MUTED.with(|m| m.get()) {
        return;
    }
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
