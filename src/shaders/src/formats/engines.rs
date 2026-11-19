//! Engine containers: Snowdrop's BERG data slices (Mario + Rabbids), Heaps' PAK shader cache (Dead Cells), Hedgehog
//! Engine 2's PACx 403 archives and the Needle shader containers inside them (Sonic).

use super::{archive, glslc};
use crate::nvn::*;
use base64::Engine;

/// A Snowdrop shader record's 20-byte header starts {0xc5 or 0xc6, 0, 0xcd, 0xab}.
fn record_at(d: &[u8], h: usize) -> bool {
    d.len() >= h + 4 && (d[h] == 0xc5 || d[h] == 0xc6) && d[h + 1..h + 4] == [0, 0xcd, 0xab]
}

/// zstd frames in `d` whose decoded bytes start with a shader record: (start, end, decoded).
fn wrapped(d: &[u8]) -> Vec<(usize, usize, Vec<u8>)> {
    let mut out = vec![];
    for h in memchr::memmem::find_iter(d, &archive::ZSTD_MAGIC) {
        if out.last().is_some_and(|&(_, e, _): &(usize, usize, Vec<u8>)| h < e) {
            continue;
        }
        let Some(head) = archive::zstd_peek(&d[h..], 20) else { continue };
        if head.len() != 20 || !record_at(&head, 0) {
            continue;
        }
        if let Some((dec, n)) = archive::zstd_frame(&d[h..]) {
            out.push((h, h + n, dec));
        }
    }
    out
}

/// Snowdrop's BERG data slices: compressed shader records {0xabcd00c5 zlib / 0xabcd00c6 zstd, flags, 0, raw
/// length, stored length} then the data; inside, each control section is right before its code area (+0x20 and
/// +0x704 their lengths). Records can sit inside other zstd chunks.
pub fn snowdrop(d: &[u8], label: &str, nested: bool) -> R<Vec<Found>> {
    let chunks = if nested { vec![] } else { wrapped(d) };
    let mut out = vec![];
    let mut ci = 0;
    let mut h = 0;
    while let Some(p) = memchr::memmem::find(&d[h..], &[0, 0xcd, 0xab]) {
        let at = h + p;
        h = at + 1;
        if at == 0 || !record_at(d, at - 1) {
            continue;
        }
        let h0 = at - 1;
        while ci < chunks.len() && h0 >= chunks[ci].1 {
            ci += 1;
        }
        if ci < chunks.len() && h0 >= chunks[ci].0 {
            continue;
        }
        if h0 + 24 > d.len() {
            continue;
        }
        let zstd = d[h0] == 0xc6;
        if zstd {
            if d[h0 + 20..h0 + 24] != archive::ZSTD_MAGIC {
                continue;
            }
        } else if d[h0 + 20] & 0xf != 8 || u16::from_be_bytes([d[h0 + 20], d[h0 + 21]]) % 31 != 0 {
            continue;
        }
        let (nraw, nstored) = (u32_at(d, h0 + 12).unwrap() as usize, u32_at(d, h0 + 16).unwrap() as u64);
        let stored = span(d, h0 as u64 + 20, nstored, &format!("{label} Snowdrop record @{h0:#x}"))?;
        if nstored == 0 {
            return Err(format!("{label} Snowdrop record @{h0:#x}: no stored bytes"));
        }
        let w = format!("{label} Snowdrop record @{h0:#x}");
        let raw = if zstd { archive::unzstd(stored, &Default::default(), &w)? } else { archive::unzlib(stored, &w)? };
        if raw.len() != nraw {
            return Err(format!("{w}: expected {nraw} raw bytes, got {}", raw.len()));
        }
        for c in memchr::memmem::find_iter(&raw, &CTL_MAGIC) {
            if c + 0x708 > raw.len() {
                continue;
            }
            let (nc, nk) = (u32_at(&raw, c + 0x20).unwrap() as usize, u32_at(&raw, c + 0x704).unwrap() as usize);
            if nc < 0x760 || nc & 3 != 0 || nk < 0x34 || c + nc + nk > raw.len() {
                continue;
            }
            let (ctl, code) = (&raw[c..c + nc], &raw[c + nc..c + nc + nk]);
            if let Some(st) = glslc::stage(ctl, code).filter(|&st| check(st, ctl, code).is_none()) {
                out.push(Found::new(format!("{label}#SNOWDROP@{h0:#x}:{c:#x}"), st, ctl, code));
            }
        }
    }
    for (h, _, raw) in chunks {
        out.extend(snowdrop(&raw, &format!("{label}#SDFZSTD@{h:#x}"), true)?);
    }
    Ok(out)
}

/// The files of a Heaps PAK (Dead Cells): 'PAK', version, i32 header size, i32 data size, (version 1: a 64-byte
/// hash), the root entry, then 'DATA'. An entry: u8 name length, name, u8 flags; a folder (bit 0) has i32 child count
/// and its children, a file its position (f64 with bit 1, else i32), i32 size, i32 checksum. → (path, offset, size).
pub fn heaps_pak_files(head: &[u8]) -> Option<Vec<(String, u64, u64)>> {
    if head.len() < 12 || &head[..3] != b"PAK" || head[3] > 1 {
        return None;
    }
    let size = i32_at(head, 4)? as i64;
    if !(12 < size && size <= 1 << 28) || i32_at(head, 8)? < 0 || head.len() < size as usize {
        return None;
    }
    let h = &head[12..size as usize];
    let mut out = vec![];
    fn entry(h: &[u8], mut p: usize, path: &str, size: u64, out: &mut Vec<(String, u64, u64)>, depth: usize) -> Option<usize> {
        if depth > 64 {
            return None;
        }
        let n = *h.get(p)? as usize;
        let name = std::str::from_utf8(h.get(p + 1..p + 1 + n)?).ok()?;
        let fl = *h.get(p + 1 + n)?;
        p += n + 2;
        let path = if name.is_empty() { path.to_string() } else { format!("{path}/{name}") };
        if fl & 1 != 0 {
            let cnt = i32_at(h, p)?;
            p += 4;
            for _ in 0..cnt.max(0) {
                p = entry(h, p, &path, size, out, depth + 1)?;
            }
            return Some(p);
        }
        let pos = if fl & 2 != 0 { f64::from_le_bytes(h.get(p..p + 8)?.try_into().ok()?) as u64 } else { i32_at(h, p)? as u64 };
        p += if fl & 2 != 0 { 8 } else { 4 };
        let n = i32_at(h, p)? as u64;
        out.push((path, size + pos, n));
        Some(p + 8)
    }
    let end = entry(h, if head[3] != 0 { 64 } else { 0 }, "", size as u64, &mut out, 0)?;
    (h.get(end..end + 4) == Some(b"DATA")).then_some(out)
}

pub const HEAPS_CACHE: &str = "shaders.cache.nx";

/// Heaps' shader cache: each compiled GLSL ends with `//BIN=<base64 of its GLSLC output>#`.
pub fn heaps_shaders(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for m in memchr::memmem::find_iter(d, b"//BIN=") {
        let s = &d[m + 6..];
        let n = s.iter().position(|&c| !(c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')).unwrap_or(s.len());
        if s.get(n) != Some(&b'#') {
            continue;
        }
        let Ok(bin) = base64::engine::general_purpose::STANDARD.decode(&s[..n]) else { continue };
        out.extend(glslc::shaders(&bin, &format!("{label}@{m:#x}")));
    }
    if out.is_empty() {
        return Err(format!("{label}: no GLSLC output in //BIN= comments"));
    }
    Ok(out)
}

pub const PACX4: &[u8; 8] = b"PACx403L";
/// Sonic X Shadow Generations' revision: the same layout, its root and blocks signed `\0\0\0\0405L`.
pub const PACX405: &[u8; 8] = b"PACx405L";
const PACX4_ROOT: &[u8; 8] = b"PACx402L";
const PACX405_ROOT: &[u8; 8] = b"\0\0\0\0405L";

/// A PACx 403 or 405 archive (Hedgehog Engine 2).
pub fn is_pacx4(d: &[u8]) -> bool {
    matches!(d.get(..8), Some(h) if h == PACX4 || h == PACX405)
}
pub const NEEDLE: &[u8; 8] = b"HNSHV002";

/// A PACx 403 part: its chunks (stored length, length), back to back from `at`; a chunk whose lengths are equal is
/// stored, the others are LZ4 blocks.
fn pacx4_unchunk(d: &[u8], mut at: u64, chunks: &[(u32, u32)], n: usize, wh: &str) -> R<Vec<u8>> {
    let mut out = Vec::with_capacity(n);
    for &(c, u) in chunks {
        let raw = span(d, at, c as u64, wh)?;
        if c == u {
            out.extend_from_slice(raw);
        } else {
            out.extend(lz4_flex::block::decompress(raw, u as usize).map_err(|e| format!("{wh}: LZ4: {e}"))?);
        }
        at += c as u64;
    }
    if out.len() != n {
        return Err(format!("{wh}: its chunks give {:#x} bytes, its length is {n:#x}", out.len()));
    }
    Ok(out)
}

/// Hedgehog Engine 2's PACx 403 archives (Sonic Frontiers) and 405 (Sonic X Shadow Generations): a root (a PACx 402
/// archive, or a `\0\0\0\0405L` one in 405; LZ4 chunks) whose block table names further blocks of the same kind.
/// Only an archive whose root names a shader type is opened. → (name, bytes) of the root and blocks.
pub fn pacx4_parts(d: &[u8], label: &str) -> R<Vec<(String, Vec<u8>)>> {
    let h = span(d, 0x10, 12, &format!("{label}: PACx 403 header"))?;
    let (roff, rstored, rlen) = (u32_at(h, 0).unwrap(), u32_at(h, 4).unwrap(), u32_at(h, 8).unwrap() as usize);
    let (deps, ctab) = (need32(d, 0x20, label)? as usize, need32(d, 0x24, label)? as usize);
    let n = need32(d, 0x30 + deps, &format!("{label}: PACx 403 root chunk table"))? as usize;
    if n == 0 || 4 + 8 * n > ctab {
        return Err(format!("{label}: PACx 403 root chunk count {n} in a table of {ctab:#x} bytes"));
    }
    let chunks: Vec<(u32, u32)> = (0..n).map(|i| Ok((need32(d, 0x34 + deps + 8 * i, label)?, need32(d, 0x38 + deps + 8 * i, label)?))).collect::<R<_>>()?;
    if chunks.iter().map(|c| c.0 as u64).sum::<u64>() != rstored as u64 {
        return Err(format!("{label}: the PACx 403 root chunks do not add up to its stored length {rstored:#x}"));
    }
    let root = pacx4_unchunk(d, roff as u64, &chunks, rlen, &format!("{label}: PACx 403 root"))?;
    let want = if d.get(..8) == Some(&PACX405[..]) { PACX405_ROOT } else { PACX4_ROOT };
    if root.get(..8) != Some(&want[..]) {
        return Err(format!("{label}: the PACx 4 root does not start with {:?}", String::from_utf8_lossy(want)));
    }
    if memchr::memmem::find(&root, b"Shader").is_none() {
        return Ok(vec![]);
    }
    let (trees, info) = (need32(&root, 0x10, label)? as usize, need32(&root, 0x14, label)? as usize);
    let mut blocks_out = vec![];
    if info != 0 {
        let t = span(&root, (0x30 + trees) as u64, 16, &format!("{label}: PACx 403 block table"))?;
        let (nb, tab) = (u64_at(t, 0).unwrap(), u64_at(t, 8).unwrap());
        let mut blocks = vec![];
        for i in 0..nb {
            let b = span(&root, tab + 0x20 * i, 0x20, &format!("{label}: PACx 403 block {i}"))?;
            blocks.push((u64_at(b, 0).unwrap(), u32_at(b, 8).unwrap(), u32_at(b, 12).unwrap() as usize, u32_at(b, 16).unwrap(), u32_at(b, 20).unwrap() as u64, u64_at(b, 24).unwrap()));
        }
        if 0x10 + blocks.iter().map(|b| 0x20 + 8 * b.4).sum::<u64>() != info as u64 {
            return Err(format!("{label}: the PACx 403 block table is not {info:#x} bytes"));
        }
        for (i, &(name, stored, ln, off, nch, cptr)) in blocks.iter().enumerate() {
            // 405's blocks are unnamed (name 0): their index instead
            let nm = root.get(name as usize..).filter(|_| name != 0).map(|s| String::from_utf8_lossy(&s[..memchr::memchr(0, s).unwrap_or(s.len())]).into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| i.to_string());
            let wh = format!("{label}/{nm}");
            let cks: Vec<(u32, u32)> = (0..nch)
                .map(|k| {
                    let c = span(&root, cptr + 8 * k, 8, &wh)?;
                    Ok((u32_at(c, 0).unwrap(), u32_at(c, 4).unwrap()))
                })
                .collect::<R<_>>()?;
            if cks.iter().map(|c| c.0 as u64).sum::<u64>() != stored as u64 {
                return Err(format!("{wh}: the chunks do not add up to the block's stored length {stored:#x}"));
            }
            blocks_out.push((nm, pacx4_unchunk(d, off as u64, &cks, ln, &wh)?));
        }
    }
    let mut out = vec![("root".to_string(), root)];
    out.extend(blocks_out);
    Ok(out)
}

/// Needle shader containers (HNSHV002, +8 the container's length, big-endian): each NVN payload has a section
/// descriptor right before its control section (0x90 bytes; 0x98 for NVN2): +0x30 control offset, +0x34 code offset,
/// +0x38 code length, +0x3c control length, relative to the descriptor.
pub fn needle(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for h in memchr::memmem::find_iter(d, NEEDLE) {
        let Some(sz) = d.get(h + 8..h + 12) else { continue };
        let size = u32::from_be_bytes(sz.try_into().unwrap()) as usize;
        if size < 0x98 + NVN2_KEY_CTL || size > d.len() - h {
            continue;
        }
        let end = h + size;
        let mut c = memchr::memmem::find(&d[h + 12..end], &CTL_MAGIC).map(|c| c + h + 12);
        while let Some(ci) = c {
            let nvn2 = nvn2_ctl(&d[ci..(ci + NVN2_KEY_CTL).min(end)]);
            let (desc, cmin, own) = if nvn2 { (0x98, NVN2_KEY_CTL, 0x30) } else { (0x90, 0x760, 0x20) };
            if size < desc + cmin {
                break;
            }
            let wh = format!("{label} NEEDLE @{h:#x}");
            if ci < h + 12 + desc || ci + cmin > end {
                return Err(format!("{wh}: incomplete NVN section at {ci:#x}"));
            }
            let s = ci - desc;
            let (co, ko, kn, cn) = (u32_at(d, s + 0x30).unwrap() as usize, u32_at(d, s + 0x34).unwrap() as usize, u32_at(d, s + 0x38).unwrap() as usize, u32_at(d, s + 0x3c).unwrap() as usize);
            if co != desc || cn < cmin || Some(cn as u32) != u32_at(d, ci + own) {
                return Err(format!("{wh}: invalid control offset or control length at {s:#x}"));
            }
            if ko < co + cn || kn == 0 || (co + cn).max(ko + kn) > end - s {
                return Err(format!("{wh}: NVN section at {s:#x} is outside its HNSHV002 container"));
            }
            let (ctl, code) = (&d[ci..ci + cn], &d[s + ko..s + ko + kn]);
            let st = glslc::stage(ctl, code).ok_or_else(|| format!("{wh}: NVN section at {s:#x}: unknown shader stage"))?;
            if let Some(why) = check(st, ctl, code) {
                return Err(format!("{wh}: NVN section at {s:#x}: {why}"));
            }
            out.push(Found::new(format!("{label}#NEEDLE@{s:#x}"), st, ctl, code));
            c = memchr::memmem::find(&d[s + ko + kn..end], &CTL_MAGIC).map(|x| x + s + ko + kn);
        }
    }
    Ok(out)
}

/// A PlatinumGames DAT archive: 'DAT\0', u32 file count, then the offsets of the file offset table (u32 each), the
/// extensions, the names (u32 name width, then one name of that width each), the file lengths (u32 each). → (name,
/// data); None when the tables do not fit (another format with the same four bytes).
pub fn pgdat_members(d: &[u8]) -> Option<Vec<(String, &[u8])>> {
    if d.get(..4) != Some(b"DAT\0") || d.len() < 0x18 {
        return None;
    }
    let (n, ot, nt, st) = (u32_at(d, 4)? as usize, u32_at(d, 8)? as usize, u32_at(d, 16)? as usize, u32_at(d, 20)? as usize);
    if n > d.len() / 4 || (ot + 4 * n).max(st + 4 * n).max(nt + 4) > d.len() {
        return None;
    }
    let w = u32_at(d, nt)? as usize;
    if w.checked_mul(n).is_none_or(|x| nt + 4 + x > d.len()) {
        return None;
    }
    let mut out = vec![];
    for i in 0..n {
        let (o, sz) = (u32_at(d, ot + 4 * i)? as usize, u32_at(d, st + 4 * i)? as usize);
        let m = d.get(o..o.checked_add(sz)?)?;
        let nm = &d[nt + 4 + w * i..nt + 4 + w * (i + 1)];
        out.push((String::from_utf8_lossy(&nm[..memchr::memchr(0, nm).unwrap_or(nm.len())]).into_owned(), m));
    }
    Some(out)
}

/// A PlatinumGames shader object's stage by its magic ('VSO\0', 'PSO\0', ...).
pub fn pg_stage(head: &[u8]) -> Option<Stage> {
    Some(match head.get(..4)? {
        b"VSO\0" => Stage::Vs,
        b"HSO\0" => Stage::Hs,
        b"DSO\0" => Stage::Ds,
        b"GSO\0" => Stage::Gs,
        b"PSO\0" => Stage::Ps,
        b"CSO\0" => Stage::Cs,
        _ => return None,
    })
}

/// A PlatinumGames shader object (Bayonetta 3): magic (the stage), 4 version bytes, u32 control length, u32 control
/// offset, u32 code length, u32 code offset. None when its control offset does not point at a control section.
pub fn pg_object(d: &[u8], label: &str) -> Option<R<Found>> {
    let st = pg_stage(d)?;
    let (nc, oc, nk, ok) = (u32_at(d, 8)? as usize, u32_at(d, 12)? as usize, u32_at(d, 16)? as usize, u32_at(d, 20)? as usize);
    if d.get(oc..oc + 4) != Some(&CTL_MAGIC[..]) {
        return None;
    }
    let bad = || format!("{label}: shader object sections out of bounds");
    let r = (|| {
        let ctl = d.get(oc..oc.checked_add(nc).ok_or_else(bad)?).ok_or_else(bad)?;
        let code = d.get(ok..ok.checked_add(nk).ok_or_else(bad)?).ok_or_else(bad)?;
        if let Some(why) = check(st, ctl, code) {
            return Err(format!("{label} ({}): {why}", st.name()));
        }
        if st != Stage::Cs && sph_stage(code) != Some(st) {
            return Err(format!("{label}: the SPH stage does not match the object's magic {}", st.name()));
        }
        Ok(Found::new(format!("{label}#PGSO"), st, ctl, code))
    })();
    Some(r)
}

/// PlatinumGames' binding-table form (NieR:Automata): u32 prefix length (with itself), bindings, u32 code length, the
/// code area, u32 control length, the control section, to the end. None when the bytes are not that.
pub fn pg_sized(d: &[u8], label: &str) -> Option<R<Found>> {
    let pre = u32_at(d, 0)? as usize;
    if pre < 4 || pre & 3 != 0 || pre + 8 > d.len() {
        return None;
    }
    let nk = u32_at(d, pre)? as usize;
    let (ko, cs) = (pre + 4, (pre + 4).checked_add(nk)?);
    let head = d.get(ko..ko + 4)?;
    if nk < 0x80 || cs + 4 > d.len() || (head != GFX_MAGIC && head != CS_MAGIC) {
        return None;
    }
    let nc = u32_at(d, cs)? as usize;
    if nc < 0x760 || cs + 4 + nc != d.len() || d[cs + 4..cs + 8] != CTL_MAGIC {
        return None;
    }
    let (ctl, code) = (&d[cs + 4..], &d[ko..cs]);
    let st = if head == CS_MAGIC { Some(Stage::Cs) } else { sph_stage(code) };
    Some(match st {
        None => Err(format!("{label}: binding-table shader has an unknown SPH stage")),
        Some(st) => match check(st, ctl, code) {
            Some(why) => Err(format!("{label} ({}): {why}", st.name())),
            None => Ok(Found::new(format!("{label}#PGSO"), st, ctl, code)),
        },
    })
}

pub const RGXD: &[u8; 4] = b"rgxd";

/// A RAGE effect ('rgxd', Red Dead Redemption): records {u32, u32 GLSLC magic, u32 code length, u32 control length},
/// the code area, the control section, packed. Control sections no record reaches go to `problems`.
pub fn rgxd(d: &[u8], label: &str, problems: &mut Vec<String>) -> Vec<Found> {
    let mut out = vec![];
    for m in memchr::memmem::find_iter(d, &glslc::MAGIC) {
        let Some(h) = m.checked_sub(4) else { continue };
        let (Some(nk), Some(nc)) = (u32_at(d, h + 8), u32_at(d, h + 12)) else { continue };
        let (nk, nc) = (nk as usize, nc as usize);
        let Some(code) = d.get(h + 16..h + 16 + nk) else { continue };
        let Some(ctl) = d.get(h + 16 + nk..h + 16 + nk + nc) else { continue };
        if ctl.len() < 0x760 || ctl[..4] != CTL_MAGIC {
            continue;
        }
        if let Some(st) = glslc::stage(ctl, code).filter(|&st| check(st, ctl, code).is_none()) {
            out.push(Found::new(format!("{label}#RGXD@{h:#x}"), st, ctl, code));
        }
    }
    let n = memchr::memmem::find_iter(d, &CTL_MAGIC).count();
    if n != out.len() {
        problems.push(format!("{label}: {n} control sections, {} shader records read; the rest are left out", out.len()));
    }
    out
}

pub const TFXO: &[u8; 11] = b".tfxo file\0";

/// A YEBIS effect file ('.tfxo file'): after its text header the shaders, each a control section (its own length at
/// +0x20) directly followed by its code area.
pub fn tfxo(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for a in memchr::memmem::find_iter(d, &CTL_MAGIC) {
        let Some(n) = u32_at(d, a + 0x20).map(|x| x as usize).filter(|n| (0x760..=0x880).contains(n)) else { continue };
        let ctl = span(d, a as u64, n as u64, &format!("{label}: the control section at {a:#x}"))?;
        let nk = u32_at(ctl, 0x704).unwrap() as usize;
        let code = d.get(a + n..a + n + nk).unwrap_or(&[]);
        let st = match code.get(..4) {
            Some(h) if h == CS_MAGIC => Some(Stage::Cs),
            Some(h) if h == GFX_MAGIC => sph_stage(code),
            _ => None,
        };
        let st = st.ok_or_else(|| format!("{label}: the control section at {a:#x} is not followed by a code area"))?;
        if let Some(why) = check(st, ctl, code) {
            return Err(format!("{label}: shader at {a:#x}: {why}"));
        }
        out.push(Found::new(format!("{label}#TFXO@{a:#x}"), st, ctl, code));
    }
    Ok(out)
}

/// YEBIS effect files linked into a module as zlib streams: a stream whose first bytes inflate to the '.tfxo file'
/// header.
pub fn tfxo_streams(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    let mut i = 0;
    while let Some(p) = memchr::memchr(0x78, &d[i..]) {
        let at = i + p;
        i = at + 1;
        if !matches!(d.get(at + 1), Some(0x01 | 0x5e | 0x9c | 0xda)) {
            continue;
        }
        let mut z = flate2::Decompress::new(true);
        let mut head = [0u8; 11];
        if z.decompress(&d[at..(at + 4096).min(d.len())], &mut head, flate2::FlushDecompress::None).is_err() || &head != TFXO {
            continue;
        }
        let x = archive::unzlib(&d[at..], label)?; // the stream ends where it ends; what follows is not read
        out.extend(tfxo(&x, &format!("{label}@{at:#x}"))?);
    }
    Ok(out)
}

pub const BND2: &[u8; 4] = b"bnd2";

/// Criterion's Bundle 2 (Need for Speed: Hot Pursuit Remastered's SHADERS.BNDL, Burnout Paradise Remastered): +4
/// version (2: 3 blocks per resource, 0x40-byte entries; 3: 4 blocks, 0x50), +0x10 entry count, +0x14 entry table,
/// +0x18 each block's data offset, then flags (bit 0: zlib). An entry: resource ID u64, import hash u64, then per block
/// the raw sizes, the stored sizes (alignment in the top nibble), the offsets. Every block holding GLSLC outputs is read.
pub fn bnd2(d: &[u8], label: &str) -> R<Vec<Found>> {
    let wh = format!("{label}: bnd2 header");
    let (ver, cnt, eoff) = (need32(d, 4, &wh)?, need32(d, 0x10, &wh)? as usize, need32(d, 0x14, &wh)? as usize);
    let (blocks, entry) = match ver {
        2 => (3usize, 0x40usize),
        3 => (4, 0x50),
        _ => return Err(format!("{label}: bnd2 version {ver} is not a layout this reader knows")),
    };
    let table_end = cnt.checked_mul(entry).and_then(|x| x.checked_add(eoff)).filter(|&e| eoff >= 0x30 && e <= d.len()).ok_or_else(|| format!("{label}: bnd2 entry table out of bounds"))?;
    let datoff: Vec<usize> = (0..blocks).map(|k| need32(d, 0x18 + 4 * k, &wh).map(|x| x as usize)).collect::<R<_>>()?;
    let flags = need32(d, 0x18 + 4 * blocks, &wh)?;
    let mut out = vec![];
    for i in 0..cnt {
        let e = eoff + i * entry;
        let rid = u64_at(d, e).unwrap();
        for k in 0..blocks {
            let f = |a: usize| u32_at(d, e + 0x10 + 4 * blocks * a + 4 * k).unwrap() as usize;
            let (raw_n, n, off) = (f(0) & 0x0fff_ffff, f(1) & 0x0fff_ffff, f(2));
            let w = format!("{label}: bnd2 resource {rid:016x} block {k}");
            if n == 0 {
                continue;
            }
            let start = datoff[k] + off;
            if start < table_end {
                return Err(format!("{w}: its data starts inside the resource table"));
            }
            let stored = span(d, start as u64, n as u64, &w)?;
            let data = if flags & 1 != 0 { archive::unzlib(stored, &w)? } else { stored.to_vec() };
            if data.len() != raw_n {
                return Err(format!("{w}: {} bytes, declared {raw_n}", data.len()));
            }
            if memchr::memmem::find(&data, &glslc::MAGIC).is_some() {
                out.extend(glslc::shaders(&data, &format!("{label}/{rid:016x}.{k}")));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    #[test]
    fn platinum_dat_objects() {
        let (c, k) = nvn1_ps(9);
        let mut o = b"PSO\0\0\0\x01\x10".to_vec();
        for x in [c.len() as u32, 0x18, k.len() as u32, 0x18 + c.len() as u32] {
            o.extend_from_slice(&x.to_le_bytes());
        }
        o.extend_from_slice(&c);
        o.extend_from_slice(&k);
        let mut sized = 8u32.to_le_bytes().to_vec();
        sized.extend_from_slice(&[0; 4]);
        sized.extend_from_slice(&(k.len() as u32).to_le_bytes());
        sized.extend_from_slice(&k);
        sized.extend_from_slice(&(c.len() as u32).to_le_bytes());
        sized.extend_from_slice(&c);
        // DAT, 2 files: offsets at 0x20, names at 0x28 (width 8), lengths at 0x3c
        let mut d = b"DAT\0".to_vec();
        for x in [2u32, 0x20, 0, 0x28, 0x3c, 0, 0] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        let a = 0x44u32;
        let b = a + o.len() as u32;
        d.extend_from_slice(&a.to_le_bytes());
        d.extend_from_slice(&b.to_le_bytes());
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(b"a.pso\0\0\0b.bin\0\0\0");
        d.extend_from_slice(&(o.len() as u32).to_le_bytes());
        d.extend_from_slice(&(sized.len() as u32).to_le_bytes());
        assert_eq!(d.len(), 0x44);
        d.extend_from_slice(&o);
        d.extend_from_slice(&sized);
        let mut sc = crate::formats::Scanned::default();
        crate::formats::scan_file(&Default::default(), &d, "/shader.dat", &mut sc);
        assert!(sc.problems.is_empty(), "{:?}", sc.problems);
        let labels: Vec<&str> = sc.found.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, vec!["/shader.dat/a.pso#PGSO", "/shader.dat/b.bin#PGSO"]);
        assert!(sc.found.iter().all(|f| f.key() == skey(&c, &k)));
    }

    #[test]
    fn bnd2_glslc() {
        let (c, k) = nvn1_ps(10);
        let out = crate::formats::glslc::tests::make_output(&c, &k);
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, &out).unwrap();
        let z = z.finish().unwrap();
        // version 2: the entry table at 0x30, one entry (0x40), block 1's data right after it
        let mut d = BND2.to_vec();
        for x in [2u32, 0, 0, 1, 0x30, 0, 0x70, 0, 1] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        d.resize(0x30, 0);
        let mut e = vec![0u8; 0x40];
        e[0..8].copy_from_slice(&7u64.to_le_bytes());
        e[0x14..0x18].copy_from_slice(&(out.len() as u32).to_le_bytes());
        e[0x20..0x24].copy_from_slice(&(z.len() as u32).to_le_bytes());
        d.extend_from_slice(&e);
        d.extend_from_slice(&z);
        let mut sc = crate::formats::Scanned::default();
        crate::formats::scan_file(&Default::default(), &d, "/SHADERS.BNDL", &mut sc);
        assert!(sc.problems.is_empty(), "{:?}", sc.problems);
        assert_eq!(sc.found.len(), 1);
        assert_eq!(sc.found[0].key(), skey(&c, &k));
    }

    #[test]
    fn snowdrop_records() {
        let (mut c, k) = nvn1_ps(4);
        c[0x20..0x24].copy_from_slice(&0x880u32.to_le_bytes());
        let mut raw = vec![0u8; 8];
        raw.extend_from_slice(&c);
        raw.extend_from_slice(&k);
        let stored = zstd::bulk::compress(&raw, 3).unwrap();
        let mut d = b"BERG".to_vec();
        d.resize(0x20, 0);
        d.extend_from_slice(&[0xc6, 0, 0xcd, 0xab]);
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        d.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        d.extend_from_slice(&stored);
        let out = snowdrop(&d, "/a.sdf", false).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key(), skey(&c, &k));
    }

    #[test]
    fn needle_payload() {
        let (c, k) = nvn2(Stage::Ps, 6);
        let mut d = b"HNSHV002".to_vec();
        d.resize(0x20, 0);
        let s = d.len();
        d.resize(s + 0x98, 0);
        let co = 0x98u32;
        let ko = co + c.len() as u32;
        d[s + 0x30..s + 0x34].copy_from_slice(&co.to_le_bytes());
        d[s + 0x34..s + 0x38].copy_from_slice(&ko.to_le_bytes());
        d[s + 0x38..s + 0x3c].copy_from_slice(&(k.len() as u32).to_le_bytes());
        d[s + 0x3c..s + 0x40].copy_from_slice(&(c.len() as u32).to_le_bytes());
        d.extend_from_slice(&c);
        d.extend_from_slice(&k);
        let n = d.len() as u32;
        d[8..12].copy_from_slice(&n.to_be_bytes());
        let f = needle(&d, "/raw/NeedleShader.pac").unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].key(), skey(&c, &k));
    }

    #[test]
    fn heaps_cache() {
        let (c, k) = nvn1_ps(5);
        let out = crate::formats::glslc::tests::make_output(&c, &k);
        let mut d = b"void main() {}\n//BIN=".to_vec();
        d.extend_from_slice(base64::engine::general_purpose::STANDARD.encode(&out).as_bytes());
        d.extend_from_slice(b"#\n");
        let f = heaps_shaders(&d, "/res.pak/shaders.cache.nx").unwrap();
        assert_eq!(f.len(), 1);
        // the PAK index
        let mut pak = b"PAK\0".to_vec();
        let mut idx = vec![0u8, 1]; // root: empty name, folder
        idx.extend_from_slice(&1i32.to_le_bytes());
        idx.push(HEAPS_CACHE.len() as u8);
        idx.extend_from_slice(HEAPS_CACHE.as_bytes());
        idx.push(0);
        idx.extend_from_slice(&0i32.to_le_bytes());
        idx.extend_from_slice(&(d.len() as i32).to_le_bytes());
        idx.extend_from_slice(&0i32.to_le_bytes());
        idx.extend_from_slice(b"DATA");
        let hsize = 12 + idx.len();
        pak.extend_from_slice(&(hsize as i32).to_le_bytes());
        pak.extend_from_slice(&(d.len() as i32).to_le_bytes());
        pak.extend_from_slice(&idx);
        let files = heaps_pak_files(&pak).unwrap();
        assert_eq!(files, vec![(format!("/{HEAPS_CACHE}"), hsize as u64, d.len() as u64)]);
    }

    #[test]
    fn pacx405_archive() {
        // a block holding a Needle container, and a root (stored chunks) whose block table names it
        let (mut c, k) = nvn1_ps(8);
        let cl = c.len() as u32;
        c[0x20..0x24].copy_from_slice(&cl.to_le_bytes());
        let mut blk = b"HNSHV002".to_vec();
        blk.resize(0x20, 0);
        let s = blk.len();
        blk.resize(s + 0x90, 0);
        let co = 0x90u32;
        let ko = co + c.len() as u32;
        blk[s + 0x30..s + 0x34].copy_from_slice(&co.to_le_bytes());
        blk[s + 0x34..s + 0x38].copy_from_slice(&ko.to_le_bytes());
        blk[s + 0x38..s + 0x3c].copy_from_slice(&(k.len() as u32).to_le_bytes());
        blk[s + 0x3c..s + 0x40].copy_from_slice(&(c.len() as u32).to_le_bytes());
        blk.extend_from_slice(&c);
        blk.extend_from_slice(&k);
        let n = blk.len() as u32;
        blk[8..12].copy_from_slice(&n.to_be_bytes());
        let mut root = PACX405_ROOT.to_vec();
        root.resize(0x30, 0);
        root[0x14..0x18].copy_from_slice(&0x38u32.to_le_bytes()); // trees 0, block info 0x38
        root.extend_from_slice(&1u64.to_le_bytes());
        root.extend_from_slice(&0x40u64.to_le_bytes());
        let mut e = 0u64.to_le_bytes().to_vec(); // unnamed
        for x in [n, n, 0x40, 1] {
            e.extend_from_slice(&x.to_le_bytes());
        }
        e.extend_from_slice(&0x60u64.to_le_bytes());
        root.extend_from_slice(&e);
        root.extend_from_slice(&n.to_le_bytes());
        root.extend_from_slice(&n.to_le_bytes());
        root.extend_from_slice(b"Shader\0");
        let mut d = PACX405.to_vec();
        d.resize(0x40, 0);
        let roff = 0x40 + n;
        let rl = root.len() as u32;
        for (o, x) in [(0x10, roff), (0x14, rl), (0x18, rl), (0x24, 0x20), (0x30, 1), (0x34, rl), (0x38, rl)] {
            d[o..o + 4].copy_from_slice(&x.to_le_bytes());
        }
        d.extend_from_slice(&blk);
        d.extend_from_slice(&root);
        let mut sc = crate::formats::Scanned::default();
        crate::formats::scan_file(&Default::default(), &d, "/raw/NeedleShader.pac", &mut sc);
        assert!(sc.problems.is_empty(), "{:?}", sc.problems);
        assert_eq!(sc.found.len(), 1);
        assert_eq!(sc.found[0].label, format!("/raw/NeedleShader.pac/0#NEEDLE@{:#x}", 0x20));
        assert_eq!(sc.found[0].key(), skey(&c, &k));
    }

    #[test]
    fn rgxd_and_tfxo() {
        let (c, k) = nvn1_ps(21);
        let mut d = b"rgxd\0\0\0names".to_vec();
        d.extend_from_slice(&0x2df8u32.to_le_bytes());
        d.extend_from_slice(&crate::formats::glslc::MAGIC);
        d.extend_from_slice(&(k.len() as u32).to_le_bytes());
        d.extend_from_slice(&(c.len() as u32).to_le_bytes());
        d.extend_from_slice(&k);
        d.extend_from_slice(&c);
        let mut sc = crate::formats::Scanned::default();
        crate::formats::scan_file(&Default::default(), &d, "/shaders.rpf/a.nvn", &mut sc);
        assert!(sc.problems.is_empty(), "{:?}", sc.problems);
        assert_eq!(sc.found.len(), 1);
        assert_eq!(sc.found[0].key(), skey(&c, &k));
        // a .tfxo file in a zlib stream inside a module
        let mut c2 = c.clone();
        let n = c2.len() as u32;
        c2[0x20..0x24].copy_from_slice(&n.to_le_bytes());
        let mut t = TFXO.to_vec();
        t.extend_from_slice(b"1 hlsl TexUtil_NVN_NX\0");
        t.extend_from_slice(&c2);
        t.extend_from_slice(&k);
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, &t).unwrap();
        let mut img = vec![0u8; 0x40];
        img.extend(z.finish().unwrap());
        img.extend_from_slice(&[0x55; 32]);
        let f = tfxo_streams(&img, "main").unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].label, "main@0x40#TFXO@0x21");
    }
}
