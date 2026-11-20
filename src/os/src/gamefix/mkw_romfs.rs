//! Mario Kart World's internal render resolution as a RomFS overlay (mkw-recomp `runtime/sdk/fs/romfs_res.c` and
//! `tools/romfs/resolution.py`, MIT). MKW picks its 3D and screen resolutions from
//! `System/GymlArchive/general_format.Product_<version>.sarc.zs` ->
//! `Gyml/GamePerformanceGraphicsConfigGParam/30_Extendted.game__GamePerformanceGraphicsConfigGParam.bgyml` (BYML v7:
//! Resolution{Low,Middle}{For3D,Screen}[ForDiv,ForDiv4] = 1080/900/1440: 1.8.0 renders its scene at 2560x1440 lines
//! docked). With `NXRS_MKW_RESOLUTION=<lines>` every `*For3D*` integer becomes lines / 0.65 docked (the scene renders
//! at 65 % of For3D's lines docked whatever the GPU load) or lines in handheld mode, and every `*Screen*` integer
//! lines; `NXRS_MKW_DETAIL_DISTANCE=<x>` sets the float LODDistanceRatio (the console's 2: the distance at which
//! objects switch to their simpler models). The archive is compressed again and written to
//! `<data>\cache\resolution-<lines>[-handheld]\`, which the fs layers over the game's RomFS (`mods::add_overlay`).
//! Unset (the default): the console's own resolution.

use super::env_str;

const ARCHIVE_DIR: &str = "System/GymlArchive";
const MEMBER: &str = "Gyml/GamePerformanceGraphicsConfigGParam/30_Extendted.game__GamePerformanceGraphicsConfigGParam.bgyml";
const SCENE_SCALE: f64 = 0.65;

fn u16le(b: &[u8], at: usize) -> u32 {
    u16::from_le_bytes([b[at], b[at + 1]]) as u32
}
fn u24le(b: &[u8], at: usize) -> u32 {
    b[at] as u32 | (b[at + 1] as u32) << 8 | (b[at + 2] as u32) << 16
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The member's (offset, length) in a SARC.
fn sarc_find(s: &[u8], name: &str) -> Option<(usize, usize)> {
    let n = s.len();
    if n < 0x20 || &s[..4] != b"SARC" {
        return None;
    }
    let header = u16le(s, 4) as usize;
    let data_start = u32le(s, 12) as usize;
    if header + 12 > n {
        return None;
    }
    let count = u16le(s, header + 6) as usize;
    let names = header + 12 + count * 16 + 8;
    for i in 0..count {
        let e = header + 12 + i * 16;
        if e + 16 > n {
            return None;
        }
        let (attr, lo, hi) = (u32le(s, e + 4), u32le(s, e + 8) as usize, u32le(s, e + 12) as usize);
        let off = names + (attr & 0xffffff) as usize * 4;
        if off >= n || data_start + hi > n {
            return None;
        }
        let end = s[off..].iter().position(|&c| c == 0).map_or(n, |p| off + p);
        if &s[off..end] == name.as_bytes() {
            return Some((data_start + lo, hi - lo));
        }
    }
    None
}

/// Every integer field whose dotted name has For3D or Screen set to r3d / screen (0: kept), LODDistanceRatio to lod
/// when lod > 0. Returns how many changed.
#[allow(clippy::too_many_arguments)]
fn patch_dict(b: &mut [u8], keys_off: usize, node: usize, prefix: &str, r3d: u32, screen: u32, lod: f32, depth: u32) -> u32 {
    let n = b.len();
    if node + 4 > n || depth > 4 {
        return 0;
    }
    let count = u24le(b, node + 1) as usize;
    let nkeys = u24le(b, keys_off + 1);
    let mut changed = 0;
    for k in 0..count {
        let e = node + 4 + 8 * k;
        if e + 8 > n {
            break;
        }
        let (ki, ty) = (u24le(b, e), b[e + 3]);
        if ki >= nkeys {
            continue;
        }
        let ko = keys_off + u32le(b, keys_off + 4 + 4 * ki as usize) as usize;
        let kend = b[ko..].iter().position(|&c| c == 0).map_or(n, |p| ko + p);
        let name = format!("{prefix}{}", String::from_utf8_lossy(&b[ko..kend]));
        if ty == 0xC1 {
            let sub = u32le(b, e + 4) as usize;
            changed += patch_dict(b, keys_off, sub, &format!("{name}."), r3d, screen, lod, depth + 1);
        } else if ty == 0xD2 && lod > 0.0 && name == "LODDistanceRatio" {
            b[e + 4..e + 8].copy_from_slice(&lod.to_le_bytes());
            changed += 1;
        } else if ty == 0xD1 || ty == 0xD3 {
            let v = if name.contains("For3D") { r3d } else if name.contains("Screen") { screen } else { 0 };
            if v != 0 {
                b[e + 4..e + 8].copy_from_slice(&v.to_le_bytes());
                changed += 1;
            }
        }
    }
    changed
}

/// At start, before the game reads the archive: the overlay for NXRS_MKW_RESOLUTION / NXRS_MKW_DETAIL_DISTANCE.
pub(super) fn install() {
    let lines: u32 = env_str("NXRS_MKW_RESOLUTION").trim().parse().unwrap_or(0);
    let mut lod: f32 = env_str("NXRS_MKW_DETAIL_DISTANCE").trim().parse().unwrap_or(0.0);
    if lod == 2.0 {
        lod = 0.0; // the console's value writes nothing
    }
    if lines == 0 && lod <= 0.0 {
        return;
    }
    match write_overlay(lines, lod) {
        Ok(dir) => crate::mods::add_overlay(dir),
        Err(e) => crate::log!("[RED] gamefix: resolution: {e}: the game's own resolution"),
    }
}

fn write_overlay(lines: u32, lod: f32) -> Result<std::path::PathBuf, String> {
    let handheld = crate::services::oe::opmode() == 0;
    let r3d = if lines > 0 { (lines as f64 / if handheld { 1.0 } else { SCENE_SCALE }).ceil() as u32 } else { 0 };
    let fs = crate::services::fs::romfs().ok_or("no RomFS")?;
    let archive = fs
        .list(ARCHIVE_DIR)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|e| !e.is_dir && e.name.starts_with("general_format.Product_") && e.name.ends_with(".sarc.zs"))
        .map(|e| format!("{ARCHIVE_DIR}/{}", e.name))
        .ok_or(format!("no {ARCHIVE_DIR}/general_format.Product_*.sarc.zs"))?;
    let z = fs.read_file(&archive).map_err(|e| e.to_string())?;
    let mut s = zstd::decode_all(&z[..]).map_err(|e| format!("{archive} does not decompress: {e}"))?;
    let (at, len) = sarc_find(&s, MEMBER).ok_or(format!("no {MEMBER} in {archive}"))?;
    let changed = if len > 16 && &s[at..at + 2] == b"YB" {
        let m = &mut s[at..at + len];
        let (keys_off, root) = (u32le(m, 4) as usize, u32le(m, 12) as usize);
        patch_dict(m, keys_off, root, "", r3d, lines, lod, 0)
    } else {
        0
    };
    if changed == 0 {
        return Err(format!("no resolution fields in {MEMBER}"));
    }
    let out = zstd::encode_all(&s[..], 9).map_err(|e| e.to_string())?;
    let data = crate::boot::try_process().map(|p| p.data_dir.clone()).ok_or("no data folder")?;
    let name = if lod > 0.0 { format!("graphics-r{lines}{}-lod{lod}", if handheld { "-handheld" } else { "" }) } else { format!("resolution-{lines}{}", if handheld { "-handheld" } else { "" }) };
    let dir = data.join("cache").join(name);
    let path = dir.join(&archive);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("zs.part");
    std::fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    crate::log!(
        "gamefix: resolution: {lines} lines (3D {r3d}, screen {lines}, {}), detail distance {lod}: {changed} fields in {}",
        if handheld { "handheld" } else { "docked" },
        dir.display()
    );
    Ok(dir)
}
