//! Intervals and limits, in milliseconds unless said.

/// Running games are looked for this often.
pub const POLL_MS: u64 = 2500;
/// `nxrs-build contract` may take this long.
pub const CONTRACT_MS: u64 = 10_000;
/// `nxrs-build doctor` may take this long.
pub const DOCTOR_MS: u64 = 60_000;
/// Linux: SIGKILL follows SIGTERM after this.
pub const KILL_GRACE_MS: u64 = 4000;
/// After the child exits, its pipe is drained for at most this long (a grandchild may hold it).
pub const DRAIN_MS: u64 = 3000;
/// Engine events handled per frame at most (a burst of lines never stalls a frame).
pub const EVENTS_PER_FRAME: usize = 400;
/// `--screenshot` waits this long by default; `--show-menu` opens the menu after this.
pub const SCREENSHOT_DELAY_MS: u64 = 1100;
pub const SHOW_MENU_MS: u64 = 300;
/// A headless verb waits for its run this long at most.
pub const HEADLESS_LIMIT_S: u64 = 6 * 60 * 60;
