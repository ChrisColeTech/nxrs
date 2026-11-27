//! The system's UI fonts: Segoe UI and its semibold, Cascadia Mono or Consolas (and Consolas Bold for
//! the title bar's wordmark), and a CJK face for
//! Japanese titles (Windows); Noto / DejaVu on Linux.

/// (bytes, face index in a collection) of each role; None when the system has none of its files.
pub struct FontFiles {
    pub ui: Option<(Vec<u8>, u32)>,
    pub semibold: Option<(Vec<u8>, u32)>,
    pub mono: Option<(Vec<u8>, u32)>,
    /// the title bar's wordmark
    pub mono_bold: Option<(Vec<u8>, u32)>,
    pub cjk: Option<(Vec<u8>, u32)>,
    /// marks the UI faces lack (the doctor's ✓ and ✗)
    pub symbols: Option<(Vec<u8>, u32)>,
}

fn first(candidates: &[String]) -> Option<(Vec<u8>, u32)> {
    candidates
        .iter()
        .find_map(|p| std::fs::read(p).ok().map(|b| (b, 0)))
}

pub fn load() -> FontFiles {
    if cfg!(windows) {
        let dir = std::env::var("WINDIR")
            .map(|w| format!("{w}\\Fonts"))
            .unwrap_or_else(|_| "C:\\Windows\\Fonts".into());
        let p = |names: &[&str]| {
            names
                .iter()
                .map(|n| format!("{dir}\\{n}"))
                .collect::<Vec<_>>()
        };
        FontFiles {
            ui: first(&p(&["segoeui.ttf"])),
            semibold: first(&p(&["seguisb.ttf", "segoeuib.ttf"])),
            mono: first(&p(&["CascadiaMono.ttf", "consola.ttf"])),
            mono_bold: first(&p(&["consolab.ttf"])),
            cjk: first(&p(&[
                "YuGothM.ttc",
                "meiryo.ttc",
                "msgothic.ttc",
                "malgun.ttf",
            ])),
            symbols: first(&p(&["seguisym.ttf"])),
        }
    } else {
        let p = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        FontFiles {
            ui: first(&p(&[
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            ])),
            semibold: first(&p(&[
                "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            ])),
            mono: first(&p(&["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"])),
            mono_bold: first(&p(&[
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
            ])),
            cjk: first(&p(&[
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            ])),
            symbols: first(&p(&["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"])),
        }
    }
}
