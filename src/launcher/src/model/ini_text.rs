//! A game's INI as text: setting one key keeps every other line, comment and the line ending
//! (`writeIniKey`); reading takes the last value.

/// Sets `key=value`: every `key=` line anywhere is replaced; else it goes right after `[section]`;
/// else `[section]`, the line and an empty line are appended.
pub fn set_key(text: &str, section: &str, key: &str, value: &str) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let unix = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = unix.split('\n').map(str::to_string).collect();
    let line = format!("{key}={value}");
    let header = format!("[{section}]");
    let mut found = false;
    let mut at: Option<usize> = None;
    for (i, l) in lines.iter_mut().enumerate() {
        if l.trim() == header {
            at = Some(i);
        }
        let is_key = l
            .trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='));
        if is_key {
            *l = line.clone();
            found = true;
        }
    }
    if !found {
        match at {
            Some(i) => lines.insert(i + 1, line),
            None => lines.extend([header, line, String::new()]),
        }
    }
    lines.join(newline)
}

/// The last `key=` value in the text.
pub fn value(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    text.split('\n')
        .filter_map(|l| l.trim().strip_prefix(&prefix).map(|v| v.trim().to_string()))
        .next_back()
}

/// The INI keys the launcher owns.
pub const RENDERER: (&str, &str) = ("Graphics", "renderer");
pub const BUTTON_LAYOUT: (&str, &str) = ("Controls", "button_layout");
pub const PAUSE: (&str, &str) = ("Controls", "pause_on_focus_loss");
pub const MUTE: (&str, &str) = ("Controls", "mute_on_focus_loss");
pub const ASK_USER: (&str, &str) = ("Controls", "ask_user");
pub const DOCKED: (&str, &str) = ("Display", "docked");
pub const SETTINGS_VERSION: (&str, &str) = ("Other", "settings_version");
