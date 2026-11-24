//! The player's mods at run time (`<data>\mods.tsv`, written by nxrs-build's `mods.rs`): a mod is on unless its switch
//! `NXRS_MOD_<NAME>=0` says otherwise or a `disabled` file is in its folder (`<exe folder>\mods\<folder>\`; the
//! switch set to 1 wins over the file), else as its mod.json says. An enabled mod's data bytes are written into the
//! loaded modules before any guest code runs (its code patches are switchable sites in the translated code, chosen by
//! the same rule in modules.c), and its `romfs\` files replace or join the game's RomFS (fs).
//!
//! mods.tsv lines: `mod <switch> <default 0|1> <folder> <romfs folder or ->` and `bytes <switch> <module> <offset> <hex>`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct ModRow {
    env: String,
    on: bool,
    folder: String,
    romfs: Option<PathBuf>,
}

struct Mods {
    mods: Vec<ModRow>,
    bytes: Vec<(String, String, u64, u8)>,
}

static MODS: OnceLock<Mods> = OnceLock::new();

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The on / off rule for one mod.
pub fn enabled_by(env: Option<&str>, marker: bool, default_on: bool) -> bool {
    match env.filter(|v| !v.is_empty()) {
        Some(v) => !v.starts_with('0'),
        None => default_on && !marker,
    }
}

fn parse(text: &str, exe: &Path) -> Mods {
    let mut m = Mods {
        mods: vec![],
        bytes: vec![],
    };
    for l in text.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        match f.as_slice() {
            ["mod", env, def, folder, romfs] => {
                let on = enabled_by(
                    std::env::var(env).ok().as_deref(),
                    exe.join("mods").join(folder).join("disabled").exists(),
                    *def == "1",
                );
                m.mods.push(ModRow {
                    env: env.to_string(),
                    on,
                    folder: folder.to_string(),
                    romfs: (*romfs != "-").then(|| PathBuf::from(romfs)),
                });
            }
            ["bytes", env, module, off, hex] => {
                if let (Ok(o), Ok(b)) = (
                    u64::from_str_radix(off.trim_start_matches("0x"), 16),
                    u8::from_str_radix(hex, 16),
                ) {
                    m.bytes.push((env.to_string(), module.to_string(), o, b));
                }
            }
            _ => {}
        }
    }
    m
}

/// Reads `<data>\mods.tsv` once and says which mods are on.
pub fn init(data: &Path) -> bool {
    let m = MODS.get_or_init(|| {
        parse(
            &std::fs::read_to_string(data.join("mods.tsv")).unwrap_or_default(),
            &exe_dir(),
        )
    });
    for r in &m.mods {
        crate::log!(
            "mods: {} {} ({}{})",
            r.folder,
            if r.on { "on" } else { "off" },
            r.env,
            if r.romfs.is_some() {
                ", RomFS files layered"
            } else {
                ""
            }
        );
    }
    !m.mods.is_empty()
}

fn on(env: &str) -> bool {
    MODS.get()
        .is_some_and(|m| m.mods.iter().any(|r| r.env == env && r.on))
}

/// Writes the enabled mods' data bytes into the loaded modules (name, base, size).
pub fn apply_bytes(loaded: &[(String, u64, u64)]) {
    let Some(m) = MODS.get() else { return };
    let mut n = 0;
    for (env, module, off, b) in &m.bytes {
        if !on(env) {
            continue;
        }
        let Some((_, base, size)) = loaded.iter().find(|(nm, _, _)| nm == module) else {
            continue;
        };
        if *off >= *size {
            continue;
        }
        let a = base + off;
        #[cfg(target_os = "linux")]
        {
            let old = match nxrt::linux::query(a) {
                Some((true, p, _, _)) => p,
                _ => {
                    crate::log!("[RED] mods: {module}+{off:#x} cannot be written");
                    continue;
                }
            };
            let page = a & !0xfff;
            let prot = |p: u8| match p {
                0 => nxrt::mem::Prot::None,
                1 => nxrt::mem::Prot::Read,
                _ => nxrt::mem::Prot::ReadWrite,
            };
            if !nxrt::mem::protect(page, 0x1000, nxrt::mem::Prot::ReadWrite) {
                crate::log!("[RED] mods: {module}+{off:#x} cannot be written");
                continue;
            }
            unsafe { *(a as *mut u8) = *b };
            nxrt::mem::protect(page, 0x1000, prot(old));
        }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_READWRITE};
            let mut old = 0u32;
            if VirtualProtect(a as _, 1, PAGE_READWRITE, &mut old) == 0 {
                crate::log!("[RED] mods: {module}+{off:#x} cannot be written");
                continue;
            }
            *(a as *mut u8) = *b;
            let mut x = 0u32;
            VirtualProtect(a as _, 1, old, &mut x);
        }
        n += 1;
    }
    if n > 0 {
        crate::log!("mods: {n} data bytes written into the modules");
    }
}

/// The enabled mods' RomFS folders, the first given the most weight (folder order reversed: a later folder name wins
/// as in Ryujinx's load order).
fn layers() -> Vec<&'static Path> {
    let mut v: Vec<&'static Path> = OVERLAYS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    v.extend(MODS.get().map_or(vec![], |m| {
        m.mods
            .iter()
            .rev()
            .filter(|r| r.on)
            .filter_map(|r| r.romfs.as_deref())
            .collect::<Vec<_>>()
    }));
    v
}

/// RomFS folders a game fix writes at start (os `gamefix`: Mario Kart World's resolution overlay), layered over the
/// game's RomFS and the player's mods, the latest added first.
static OVERLAYS: std::sync::Mutex<Vec<&'static Path>> = std::sync::Mutex::new(Vec::new());

/// Layers `dir` (a RomFS-shaped folder) over the game's RomFS, above every mod.
pub fn add_overlay(dir: PathBuf) {
    let d: &'static Path = Box::leak(dir.into_boxed_path());
    OVERLAYS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(0, d);
}

/// A mod's file for this path of the game's RomFS (`a/b.bin`), if one replaces or adds it.
pub fn romfs_file(path: &str) -> Option<PathBuf> {
    layers()
        .into_iter()
        .map(|d| d.join(path))
        .find(|p| p.is_file())
}

/// The mods' entries of a RomFS directory: (name, is dir, size).
pub fn romfs_dir(path: &str) -> Vec<(String, bool, u64)> {
    let mut v: Vec<(String, bool, u64)> = vec![];
    for d in layers() {
        let Ok(rd) = std::fs::read_dir(d.join(path)) else {
            continue;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if v.iter().any(|x| x.0 == name) {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            v.push((name, md.is_dir(), if md.is_dir() { 0 } else { md.len() }));
        }
    }
    v
}

/// Whether a mod adds this directory to the RomFS.
pub fn romfs_has_dir(path: &str) -> bool {
    layers().into_iter().any(|d| d.join(path).is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_and_table() {
        assert!(enabled_by(None, false, true));
        assert!(!enabled_by(None, true, true), "the disabled file");
        assert!(!enabled_by(None, false, false), "mod.json off");
        assert!(enabled_by(Some("1"), true, false), "the switch wins");
        assert!(!enabled_by(Some("0"), false, true));
        let m = parse("# x\nmod\tNXRS_MOD_T_A\t1\tA\t-\nmod\tNXRS_MOD_T_B\t0\tB\tC:\\r\nbytes\tNXRS_MOD_T_A\tmain\t0x10\tff\n", Path::new("."));
        assert_eq!(m.mods.len(), 2);
        assert!(m.mods[0].on && !m.mods[1].on);
        assert_eq!(
            m.bytes,
            vec![("NXRS_MOD_T_A".into(), "main".into(), 0x10, 0xff)]
        );
    }
}
