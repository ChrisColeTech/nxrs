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
const PACX4_ROOT: &[u8; 8] = b"PACx402L";
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

/// Hedgehog Engine 2's PACx 403 archives: a root (a PACx 402 archive, LZ4 chunks) whose block table names further
/// PACx 402 blocks. Only an archive whose root names a shader type is opened. → (name, bytes) of the root and blocks.
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
    if root.get(..8) != Some(&PACX4_ROOT[..]) {
        return Err(format!("{label}: the PACx 403 root is not a PACx 402 archive"));
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
            let nm = root.get(name as usize..).map(|s| String::from_utf8_lossy(&s[..memchr::memchr(0, s).unwrap_or(s.len())]).into_owned()).unwrap_or_else(|| i.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

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
}
