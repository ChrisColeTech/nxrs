//! The browser that plays web builds (docs/35: Chrome is the target): the default browser when it is
//! Chromium-based, else Chrome found by path. None found: Play in browser is not offered.

use super::files::is_file;
use super::platform;
use crate::constants::{web as c, words_web as w};
use crate::model::path_text::{file_name, join};
use crate::model::web_build::chromium_browser;
use std::process::Command;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Browser {
    /// for the status line: Chrome, Edge, …
    pub name: String,
    /// the program to start with the address; empty: the system's default browser
    pub program: String,
}

/// `NXRS_BROWSER` (development: a program given the address), else the default browser if it is
/// Chromium-based, else Chrome by path.
pub fn find() -> Option<Browser> {
    if let Some(program) = std::env::var(c::ENV_BROWSER).ok().filter(|p| is_file(p)) {
        return Some(Browser {
            name: file_name(&program),
            program,
        });
    }
    let default = platform::registry_text(false, c::USER_CHOICE_KEY, "ProgId")
        .and_then(|id| chromium_browser(&id));
    if let Some(name) = default {
        return Some(Browser {
            name: name.into(),
            program: String::new(),
        });
    }
    chrome_path().map(|program| Browser {
        name: c::CHROME.into(),
        program,
    })
}

#[cfg(windows)]
fn chrome_path() -> Option<String> {
    let registered = [true, false]
        .into_iter()
        .filter_map(|machine| platform::registry_text(machine, c::CHROME_APP_PATH_KEY, ""));
    let usual = c::CHROME_BASE_VARS
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|base| join(&base, c::CHROME_EXE));
    registered
        .map(|p| p.trim_matches('"').to_string())
        .chain(usual)
        .find(|p| is_file(p))
}

#[cfg(not(windows))]
fn chrome_path() -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    c::LINUX_CHROMES.iter().find_map(|name| {
        path.split(':')
            .map(|dir| join(dir, name))
            .find(|p| is_file(p))
    })
}

/// Opens `url` in the browser (a tab; Chrome's "Install as app" makes it an app window).
pub fn open(b: &Browser, url: &str) -> Result<(), String> {
    let started = if b.program.is_empty() {
        open::that_detached(url)
    } else {
        Command::new(&b.program).arg(url).spawn().map(|_| ())
    };
    started.map_err(|e| format!("{}{e}", w::NO_BROWSER))
}
