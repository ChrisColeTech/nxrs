//! Host UI for two library applets a PC player needs (owner rule: host UI only where it serves the player):
//!
//! * The error applet (nn::err): the game's error, shown in a message box beside the game, which goes on (the box
//!   runs on a thread of its own). Option `error_notice` (1).
//! * Offline HTML pages (nn::web's ShowOfflineHtmlPage / OfflineWebSession): the game's own page, from its manual
//!   NCA's `html-document/` tree (`nxrs-build` stages it in the data folder), opened in the PC's browser. Option
//!   `offline_pages_in_browser` (1).
//!
//! Neither opens during harness runs (`NXRS_NO_ACTIVATE`, the owner at the PC) or headless: they are logged.

use std::path::{Path, PathBuf};

/// Whether a host window may open now (not under the harness, not headless).
fn may_show() -> bool {
    std::env::var("NXRS_NO_ACTIVATE").map_or(true, |v| v.is_empty() || v == "0") && std::env::var("NXGFX_HEADLESS").is_err()
}

/// The error a game shows: (title, text).
pub enum Shown<'a> {
    /// nn::Result
    Result(u32),
    /// nn::err::ErrorCode {category, number}
    Code(u32, u32),
    /// nn::err::ApplicationErrorArg's message, if it has a readable one
    Application(Option<&'a str>),
    /// a fixed error of the SDK (a version / add-on content mismatch, ...)
    Other(&'a str),
}

pub fn error_text(e: &Shown) -> (String, String) {
    let title = "The game reports an error".to_string();
    let text = match e {
        Shown::Result(r) => format!("Error {:04}-{:04} (Result {r:#x}).", 2000 + (r & 0x1FF), (r >> 9) & 0x1FFF),
        Shown::Code(cat, num) => format!("Error {cat:04}-{num:04}."),
        Shown::Application(Some(m)) => m.trim().to_string(),
        Shown::Application(None) => "The game reports an error of its own.".into(),
        Shown::Other(w) => {
            let mut t = w.to_string();
            if let Some(c) = t.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            format!("{t}.")
        }
    };
    (title, format!("{text}\n\nThe game goes on; this is what it would show on its error screen."))
}

/// The game's error: logged, and shown in a message box when the option is on.
pub fn error(e: Shown) {
    let (title, text) = error_text(&e);
    let first = text.lines().next().unwrap_or("").to_string();
    if !nxconfig::flag("error_notice") || !may_show() {
        crate::log!("[RED] error applet: {first} (not shown: error_notice off, or a harness run)");
        return;
    }
    crate::log!("[RED] error applet: {first} (shown in a message box)");
    std::thread::spawn(move || {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK, MB_TOPMOST};
        let w = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (t, x) = (w(&title), w(&text));
        unsafe { MessageBoxW(std::ptr::null_mut(), x.as_ptr(), t.as_ptr(), MB_OK | MB_ICONWARNING | MB_TOPMOST) };
    });
}

/// The file an offline page's document path names in the data folder's `html-document/` (its query and anchor cut
/// off; None outside that folder or missing).
pub fn page_file(data: &Path, doc: &str) -> Option<PathBuf> {
    let doc = doc.split(['#', '?']).next().unwrap_or("").trim_start_matches('/');
    let doc = doc.strip_prefix("html-document/").unwrap_or(doc);
    if doc.is_empty() || doc.split('/').any(|c| c == ".." || c.is_empty()) || doc.contains(':') || doc.contains('\\') {
        return None;
    }
    let p = data.join("html-document").join(doc);
    p.is_file().then_some(p)
}

/// An offline page: opened in the PC's browser when the build has it and the option is on.
pub fn offline_page(doc: &str) {
    let Some(data) = crate::boot::try_process().map(|p| p.data_dir.clone()) else { return };
    let Some(file) = page_file(&data, doc) else {
        crate::log!("web: offline page {doc:?}: not in this build's html-document/ (rebuild with nxrs-build to stage the game's manual)");
        return;
    };
    if !nxconfig::flag("offline_pages_in_browser") || !may_show() {
        crate::log!("web: offline page {}: not opened (offline_pages_in_browser off, or a harness run)", file.display());
        return;
    }
    crate::log!("web: offline page {} opened in the PC's browser", file.display());
    let wide: Vec<u16> = file.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
    let open: Vec<u16> = "open".encode_utf16().chain([0]).collect();
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_texts() {
        assert!(error_text(&Shown::Result(0x2a2)).1.starts_with("Error 2162-0001 (Result 0x2a2)."), "{}", error_text(&Shown::Result(0x2a2)).1);
        assert!(error_text(&Shown::Code(2306, 1)).1.starts_with("Error 2306-0001."));
        assert!(error_text(&Shown::Application(Some(" Could not connect. "))).1.starts_with("Could not connect.\n"));
        assert!(error_text(&Shown::Other("game version mismatch")).1.starts_with("Game version mismatch."));
    }

    #[test]
    fn offline_page_files() {
        let d = std::env::temp_dir().join(format!("nxrs-notice-{}", std::process::id()));
        std::fs::create_dir_all(d.join("html-document/manual")).unwrap();
        std::fs::write(d.join("html-document/manual/index.html"), "<p>").unwrap();
        let want = Some(d.join("html-document").join("manual/index.html"));
        assert_eq!(page_file(&d, "manual/index.html#top"), want);
        assert_eq!(page_file(&d, "html-document/manual/index.html?x=1"), want);
        assert_eq!(page_file(&d, "../secret"), None);
        assert_eq!(page_file(&d, "manual/missing.html"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
