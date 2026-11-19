//! The player's mods: code patch files (IPS, IPS32, IPSwitch `.pchtxt`) keyed by a module's build id, and RomFS
//! files layered over the game's, from `mods/<mod>/` beside a build. Parsing and the folder layout only; the build
//! (src/cli `mods.rs`) turns the patches into the translator's switchable sites and the run (src/os `mods.rs`)
//! applies data bytes and the RomFS layer.
//!
//! A mod folder (the layouts Atmosphère, Ryujinx and yuzu use, so a downloaded mod drops in unchanged):
//!
//!     mods/<mod>/mod.json                      optional: {"name": "...", "enabled": true, "description": "..."}
//!     mods/<mod>/exefs/<build id>.ips          IPS / IPS32 named by the module's build id
//!     mods/<mod>/exefs/*.pchtxt                IPSwitch text patches (`@nsobid-<build id>` inside)
//!     mods/<mod>/exefs_patches/<any>/<...>     Atmosphère's layout, same files
//!     mods/<mod>/*.ips | *.pchtxt              loose patch files in the mod folder
//!     mods/<mod>/romfs/...                     files that replace or add to the game's RomFS
//!
//! IPS offsets count from the start of the NSO file, whose header is 0x100 bytes: image offset = offset - 0x100.
//! `.pchtxt` offsets are as written plus `@flag offset_shift` (usually 0x100, so the written offsets are image
//! offsets), then the same 0x100 comes off.

use std::path::{Path, PathBuf};

/// The NSO header IPS offsets include.
pub const NSO_HEADER: u64 = 0x100;

/// Bytes written at an offset of the module's image (decompressed segments, as loaded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// The edits for one module build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFile {
    /// Lowercase hex, trailing zero bytes stripped (as `translator::loader::Nso::build_id`).
    pub build_id: String,
    pub edits: Vec<Edit>,
    /// The file it came from, for messages.
    pub source: String,
}

/// A build id in the one form patches are matched in: lowercase hex, trailing zero bytes stripped.
pub fn norm_build_id(s: &str) -> String {
    let mut h: String = s.trim().chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase();
    if h.len() % 2 == 1 {
        h.push('0');
    }
    while h.ends_with("00") {
        h.truncate(h.len() - 2);
    }
    h
}

fn be(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |a, &x| a << 8 | x as u64)
}

/// IPS (`PATCH` ... `EOF`, 24-bit offsets) or IPS32 (`IPS32` ... `EEOF`, 32-bit offsets); RLE records too.
/// Offsets are converted to image offsets (minus the 0x100 NSO header); a record inside the header is refused.
pub fn parse_ips(data: &[u8], build_id: &str, source: &str) -> Result<PatchFile, String> {
    let (wide, mut i) = if data.starts_with(b"IPS32") {
        (true, 5)
    } else if data.starts_with(b"PATCH") {
        (false, 5)
    } else {
        return Err(format!("{source}: not an IPS or IPS32 file"));
    };
    let (olen, eof): (usize, &[u8]) = if wide { (4, b"EEOF") } else { (3, b"EOF") };
    let mut edits = vec![];
    loop {
        if data.get(i..i + eof.len()) == Some(eof) && (i + eof.len() == data.len() || data.len() - (i + eof.len()) <= 3) {
            break;
        }
        let rec = data.get(i..i + olen + 2).ok_or_else(|| format!("{source}: cut off at byte {i}"))?;
        let off = be(&rec[..olen]);
        let n = be(&rec[olen..]) as usize;
        i += olen + 2;
        let bytes = if n == 0 {
            let r = data.get(i..i + 3).ok_or_else(|| format!("{source}: RLE record cut off at byte {i}"))?;
            i += 3;
            vec![r[2]; be(&r[..2]) as usize]
        } else {
            let b = data.get(i..i + n).ok_or_else(|| format!("{source}: record cut off at byte {i}"))?.to_vec();
            i += n;
            b
        };
        if off < NSO_HEADER {
            return Err(format!("{source}: a record at {off:#x} writes the NSO header"));
        }
        if !bytes.is_empty() {
            edits.push(Edit { offset: off - NSO_HEADER, bytes });
        }
    }
    Ok(PatchFile { build_id: norm_build_id(build_id), edits, source: source.to_string() })
}

fn unquote(s: &str) -> Result<Vec<u8>, String> {
    let mut out = vec![];
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            let mut b = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            continue;
        }
        match it.next() {
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('0') => out.push(0),
            Some('\\') => out.push(b'\\'),
            Some('"') => out.push(b'"'),
            Some('x') => {
                let h: String = it.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&h, 16).map_err(|_| format!("bad \\x escape in \"{s}\""))?);
            }
            o => return Err(format!("bad escape \\{} in \"{s}\"", o.unwrap_or(' '))),
        }
    }
    Ok(out)
}

/// IPSwitch text patches: `@nsobid-<hex>`, `@flag offset_shift <n>`, `@enabled` / `@disabled` (the patches that
/// follow are applied or not), `@stop` (the rest is ignored), `//` comments, and patch lines
/// `<hex offset> <hex bytes>` or `<hex offset> "string"`. One file may hold several build ids.
pub fn parse_pchtxt(text: &str, source: &str) -> Result<Vec<PatchFile>, String> {
    let mut out: Vec<PatchFile> = vec![];
    let mut shift: i64 = 0;
    let mut enabled = false;
    for (no, raw) in text.lines().enumerate() {
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = |m: String| format!("{source}:{}: {m}", no + 1);
        if let Some(rest) = line.strip_prefix('@') {
            let mut f = rest.split_whitespace();
            let word = f.next().unwrap_or("");
            if let Some(bid) = word.strip_prefix("nsobid-") {
                out.push(PatchFile { build_id: norm_build_id(bid), edits: vec![], source: source.to_string() });
                enabled = false;
            } else if word == "flag" {
                if f.next() == Some("offset_shift") {
                    let v = f.next().ok_or_else(|| at("offset_shift without a value".into()))?;
                    shift = parse_int(v).ok_or_else(|| at(format!("bad offset_shift {v}")))?;
                }
            } else if word == "enabled" {
                enabled = true;
            } else if word == "disabled" {
                enabled = false;
            } else if word == "stop" {
                break;
            }
            continue;
        }
        if !enabled {
            continue;
        }
        let (off_s, val) = line.split_once(char::is_whitespace).ok_or_else(|| at(format!("not a patch line: {line}")))?;
        let off = i64::from_str_radix(off_s.trim_start_matches("0x"), 16).map_err(|_| at(format!("bad offset {off_s}")))? + shift;
        let val = val.trim();
        let bytes = if let Some(q) = val.strip_prefix('"') {
            unquote(q.strip_suffix('"').ok_or_else(|| at("unterminated string".into()))?).map_err(at)?
        } else {
            let h: String = val.chars().filter(|c| !c.is_whitespace()).collect();
            if h.len() % 2 != 0 {
                return Err(at(format!("odd number of hex digits: {val}")));
            }
            (0..h.len()).step_by(2).map(|k| u8::from_str_radix(&h[k..k + 2], 16)).collect::<Result<Vec<u8>, _>>().map_err(|_| at(format!("bad hex: {val}")))?
        };
        if off < NSO_HEADER as i64 {
            return Err(at(format!("offset {off:#x} is inside the NSO header (missing `@flag offset_shift 0x100`?)")));
        }
        let pf = out.last_mut().ok_or_else(|| at("a patch before any @nsobid".into()))?;
        pf.edits.push(Edit { offset: off as u64 - NSO_HEADER, bytes });
    }
    out.retain(|p| !p.edits.is_empty());
    Ok(out)
}

fn parse_int(s: &str) -> Option<i64> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => i64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// One mod folder.
#[derive(Debug, Clone)]
pub struct Mod {
    /// The folder's name (the key of its switch and of its `disabled` marker).
    pub folder: String,
    pub name: String,
    pub dir: PathBuf,
    /// On unless mod.json says `"enabled": false`.
    pub default_on: bool,
    pub patches: Vec<PatchFile>,
    pub romfs: Option<PathBuf>,
    /// Files that could not be read or parsed (the mod still loads without them).
    pub problems: Vec<String>,
}

impl Mod {
    /// The patch set's name in the translator's table.
    pub fn set_name(&self) -> String {
        format!("mod_{}", ident(&self.folder).to_ascii_lowercase())
    }
    /// The environment switch: 0 turns the mod off for a run, 1 on (over the `disabled` marker and mod.json).
    pub fn env(&self) -> String {
        format!("NXRS_MOD_{}", ident(&self.folder).to_ascii_uppercase())
    }
}

/// The folder name as an identifier: ASCII letters and digits, the rest `_`.
pub fn ident(s: &str) -> String {
    let v: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if v.is_empty() { "_".into() } else { v }
}

fn json_field<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a serde_json::Value> {
    v.as_object().and_then(|o| o.get(k))
}

fn patch_files_in(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    v.sort();
    for p in v {
        if p.is_dir() {
            if depth > 0 {
                patch_files_in(&p, out, depth - 1);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ips") || x.eq_ignore_ascii_case("pchtxt")) {
            out.push(p);
        }
    }
}

/// Reads one mod folder.
pub fn read_mod(dir: &Path) -> Mod {
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut m = Mod { folder: folder.clone(), name: folder, dir: dir.to_path_buf(), default_on: true, patches: vec![], romfs: None, problems: vec![] };
    if let Ok(t) = std::fs::read_to_string(dir.join("mod.json")) {
        match serde_json::from_str::<serde_json::Value>(&t) {
            Ok(v) => {
                if let Some(n) = json_field(&v, "name").and_then(|x| x.as_str()) {
                    m.name = n.to_string();
                }
                if let Some(e) = json_field(&v, "enabled").and_then(|x| x.as_bool()) {
                    m.default_on = e;
                }
            }
            Err(e) => m.problems.push(format!("mod.json: {e}")),
        }
    }
    let mut files = vec![];
    patch_files_in(dir, &mut files, 0);
    patch_files_in(&dir.join("exefs"), &mut files, 0);
    patch_files_in(&dir.join("exefs_patches"), &mut files, 2);
    for f in files {
        let src = f.strip_prefix(dir).unwrap_or(&f).display().to_string();
        let r = match std::fs::read(&f) {
            Err(e) => Err(format!("{src}: {e}")),
            Ok(d) if f.extension().is_some_and(|x| x.eq_ignore_ascii_case("ips")) => {
                let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                parse_ips(&d, &stem, &src).map(|p| vec![p])
            }
            Ok(d) => parse_pchtxt(&String::from_utf8_lossy(&d), &src),
        };
        match r {
            Ok(v) => m.patches.extend(v),
            Err(e) => m.problems.push(e),
        }
    }
    let r = dir.join("romfs");
    if r.is_dir() {
        m.romfs = Some(r);
    }
    m
}

/// Every mod folder under `mods_dir`, by name.
pub fn scan(mods_dir: &Path) -> Vec<Mod> {
    let Ok(rd) = std::fs::read_dir(mods_dir) else { return vec![] };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.iter().map(|d| read_mod(d)).collect()
}

/// Applies edits to a copy of an image (bytes past its end are left out).
pub fn apply(image: &[u8], edits: &[&Edit]) -> Vec<u8> {
    let mut v = image.to_vec();
    for e in edits {
        for (k, b) in e.bytes.iter().enumerate() {
            if let Some(x) = v.get_mut(e.offset as usize + k) {
                *x = *b;
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ips_and_ips32() {
        // two records: 4 bytes at file 0x104 (image 4), RLE 3 x 0xAA at file 0x200
        let mut d = b"PATCH".to_vec();
        d.extend_from_slice(&[0x00, 0x01, 0x04, 0x00, 0x04, 0x1f, 0x20, 0x03, 0xd5]);
        d.extend_from_slice(&[0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x03, 0xaa]);
        d.extend_from_slice(b"EOF");
        let p = parse_ips(&d, "ABCD0000", "t.ips").unwrap();
        assert_eq!(p.build_id, "abcd");
        assert_eq!(p.edits, vec![Edit { offset: 4, bytes: vec![0x1f, 0x20, 0x03, 0xd5] }, Edit { offset: 0x100, bytes: vec![0xaa; 3] }]);
        let mut d = b"IPS32".to_vec();
        d.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x01, 0x42]);
        d.extend_from_slice(b"EEOF");
        let p = parse_ips(&d, "ff", "t.ips").unwrap();
        assert_eq!(p.edits, vec![Edit { offset: 0x0100_0000, bytes: vec![0x42] }]);
        assert!(parse_ips(b"PATCH\x00\x00\x10\x00\x01\x00EOF", "ff", "h.ips").is_err(), "header write refused");
        assert!(parse_ips(b"NOPE", "ff", "x.ips").is_err());
    }

    #[test]
    fn pchtxt() {
        let t = "@nsobid-B1F12FD919EAE86AB8A978317677E64B\n@flag offset_shift 0x100\n// 60 FPS\n@enabled\n01b296d8 f4031f2a // mov w20, wzr\n00000010 \"ok\\x00\"\n@disabled\n00000020 00000000\n@enabled\n00000030 20 00 80 52\n@stop\n00000040 ffff\n";
        let v = parse_pchtxt(t, "t.pchtxt").unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].build_id, "b1f12fd919eae86ab8a978317677e64b");
        assert_eq!(
            v[0].edits,
            vec![
                Edit { offset: 0x1b296d8, bytes: vec![0xf4, 0x03, 0x1f, 0x2a] },
                Edit { offset: 0x10, bytes: b"ok\0".to_vec() },
                Edit { offset: 0x30, bytes: vec![0x20, 0x00, 0x80, 0x52] }
            ]
        );
        assert!(parse_pchtxt("@nsobid-aa\n@enabled\n00000010 00\n", "n.pchtxt").is_err(), "no offset_shift: inside the header");
    }

    #[test]
    fn folder() {
        let d = std::env::temp_dir().join(format!("nxrs-mods-{}", std::process::id()));
        let m = d.join("My 60 FPS!");
        std::fs::create_dir_all(m.join("exefs")).unwrap();
        std::fs::create_dir_all(m.join("romfs/a")).unwrap();
        std::fs::write(m.join("mod.json"), r#"{"name": "Sixty", "enabled": false}"#).unwrap();
        std::fs::write(m.join("exefs/aabb.ips"), b"PATCH\x00\x01\x00\x00\x01\x07EOF").unwrap();
        std::fs::write(m.join("x.pchtxt"), "@nsobid-ccdd\n@flag offset_shift 0x100\n@enabled\n10 0102\n").unwrap();
        let v = scan(&d);
        assert_eq!(v.len(), 1);
        let m0 = &v[0];
        assert_eq!((m0.name.as_str(), m0.default_on, m0.set_name(), m0.env()), ("Sixty", false, "mod_my_60_fps_".into(), "NXRS_MOD_MY_60_FPS_".into()));
        assert_eq!(m0.patches.len(), 2, "{:?}", m0.problems);
        assert!(m0.romfs.is_some());
        assert_eq!(apply(&[0u8; 4], &[&Edit { offset: 1, bytes: vec![9, 9, 9, 9] }]), vec![0, 9, 9, 9]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
