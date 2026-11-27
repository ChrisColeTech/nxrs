//! nxos: the engine's OS services layer. It stands in for the Switch's system software around the translated game:
//! the SDK's service clients taken over by name (nxnative's HLE layer, ported), the IPC framework and `sm` with
//! the services behind it, the memory and IPC SVCs, the system counter, and the process boot. See README.md.
#![allow(clippy::missing_safety_doc)]

pub mod boot;
pub mod cheats;
pub mod clock;
#[cfg(target_os = "linux")]
pub mod desk;
pub mod fpslock;
pub mod fstep;
pub mod gamefix;
pub mod guest;
pub mod hle;
pub mod input;
pub mod ipc;
pub mod kobj;
pub mod log;
pub mod media;
pub mod mem;
pub mod mods;
pub mod nvn;
pub mod options;
pub mod profile;
pub mod sched;
pub mod services;
pub mod svc;
pub mod watch;
/// The web build's host side: the I/O worker's slots, browser saves, the audio ring (docs/35 W3).
#[cfg(target_os = "emscripten")]
pub mod web;
pub mod webdata;

/// `NXOS_TRACE=svc,ipc,hle,nvn,fs` (or `all`).
pub mod ui;
pub mod ui_text;
pub mod wwatch;
pub mod trace {
    use std::sync::OnceLock;
    static T: OnceLock<String> = OnceLock::new();
    pub fn on(what: &str) -> bool {
        let t = T.get_or_init(|| std::env::var("NXOS_TRACE").unwrap_or_default());
        !t.is_empty() && (t == "all" || t.split(',').any(|w| w == what))
    }
}
