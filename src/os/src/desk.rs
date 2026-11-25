//! The desktop's own dialogs on Linux, where Windows draws a message box, a text box or a small prompt window
//! itself: zenity (GNOME and most desktops) or kdialog (KDE, the Steam Deck's desktop), and `xdg-open` for files the browser opens. Without either dialog tool, or
//! without a display, nothing opens and the callers log what they would have shown.

use std::process::{Child, Command, Stdio};

/// A display to open a window on (X11 or Wayland).
pub fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

fn have(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
}

/// Which dialog tool there is: "zenity" or "kdialog".
pub fn tool() -> Option<&'static str> {
    if !has_display() {
        return None;
    }
    ["zenity", "kdialog"].into_iter().find(|t| have(t))
}

/// A message box (warning or information) beside the game; returns at once, the box stays until it is closed.
/// False when none could be shown.
pub fn message(title: &str, text: &str, warning: bool) -> bool {
    let mut c = match tool() {
        Some("zenity") => {
            let mut c = Command::new("zenity");
            c.args([
                if warning { "--warning" } else { "--info" },
                "--no-markup",
                "--no-wrap",
                "--title",
                title,
                "--text",
                text,
            ]);
            c
        }
        Some(_) => {
            let mut c = Command::new("kdialog");
            c.args([
                "--title",
                title,
                if warning { "--sorry" } else { "--msgbox" },
                text,
            ]);
            c
        }
        None => return false,
    };
    match c
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut ch) => {
            std::thread::spawn(move || ch.wait());
            true
        }
        Err(_) => false,
    }
}

/// A text box: `title`, a prompt line, the default text. Blocks until it is answered: Some(text) on OK, None on
/// Cancel / Escape; Err when no dialog could be shown.
pub fn entry(title: &str, prompt: &str, default: &str) -> Result<Option<String>, String> {
    let out = match tool() {
        Some("zenity") => Command::new("zenity")
            .args([
                "--entry",
                "--title",
                title,
                "--text",
                prompt,
                "--entry-text",
                default,
            ])
            .stdin(Stdio::null())
            .output(),
        Some(_) => Command::new("kdialog")
            .args(["--title", title, "--inputbox", prompt, default])
            .stdin(Stdio::null())
            .output(),
        None => return Err("no display or no zenity / kdialog".into()),
    }
    .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Ok(None);
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Ok(Some(s.strip_suffix('\n').unwrap_or(&s).to_string()))
}

/// Opens a file or URL with the desktop's handler (the browser for a page).
pub fn open(path: &std::path::Path) -> bool {
    Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut c| {
            std::thread::spawn(move || c.wait());
        })
        .is_ok()
}

/// A question dialog that stays open until it is answered or closed from here: the prompt window of `ui.rs`.
pub struct Question {
    child: Child,
}

impl Question {
    /// Opens it: OK answers "confirm" (Return), Cancel "cancel" (Escape).
    pub fn open(title: &str, text: &str) -> Option<Question> {
        let mut c = match tool()? {
            "zenity" => {
                let mut c = Command::new("zenity");
                c.args([
                    "--question",
                    "--no-markup",
                    "--no-wrap",
                    "--title",
                    title,
                    "--text",
                    text,
                    "--ok-label",
                    "OK",
                    "--cancel-label",
                    "Cancel",
                ]);
                c
            }
            _ => {
                let mut c = Command::new("kdialog");
                c.args([
                    "--title",
                    title,
                    "--yesno",
                    text,
                    "--yes-label",
                    "OK",
                    "--no-label",
                    "Cancel",
                ]);
                c
            }
        };
        c.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .map(|child| Question { child })
    }

    /// The answer once the player gave one: Some(true) confirm, Some(false) cancel, None still open.
    pub fn answer(&mut self) -> Option<bool> {
        match self.child.try_wait() {
            Ok(Some(st)) => Some(st.success()),
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }

    /// Closes it (the pads answered): our own child, ended by its PID.
    pub fn close(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_display_no_dialog() {
        if super::has_display() {
            return;
        }
        assert!(super::tool().is_none());
        assert!(!super::message("t", "x", false));
        assert!(super::entry("t", "p", "d").is_err());
    }
}
