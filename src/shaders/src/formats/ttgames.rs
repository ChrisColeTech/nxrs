//! TT Games' CC4 v2 DAT archives (LEGO Star Wars: The Skywalker Saga; nxnative `dm/gfx/tools/ttdat.py`): the DAT's
//! little-endian pointer names a big-endian `.CC40TAD` index whose name records form a folder tree, and FNV-1a hashes
//! of the full names map to the sorted file records. Only the `.nnx_shaders` members are read: stored, or in OODL
//! (Oodle) / DFLT (raw DEFLATE, the DLC's) chunks. A member holds the game's serialized programs: GLSLC outputs, and
//! control sections with their code areas beside them (`programs`).

use super::glslc;
use super::packs::Read;
use crate::nvn::*;

const MAGIC: &[u8; 8] = b".CC40TAD";
const MAX_TABLE: u64 = 64 << 20;
const MAX_MEMBER: u64 = 64 << 20;

/// A shader member: (name, offset, stored bytes, decoded bytes).
pub type TtEntry = (String, u64, u64, u64);

fn range(off: u64, n: u64, end: u64, what: &str) -> R<()> {
    if off > end || n > end - off {
        return Err(format!("{what}: range {off:#x}+{n:#x} exceeds {end:#x} bytes"));
    }
    Ok(())
}
fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(d[o..o + 4].try_into().unwrap())
}

/// The FNV-1a hash of a member's full name (backslashes, upper case, no leading separator).
pub fn name_hash(name: &str) -> u64 {
    let n = name.replace('/', "\\");
    let mut v = 0xcbf2_9ce4_8422_2325u64;
    for b in n.trim_start_matches('\\').to_uppercase().bytes() {
        v = (v ^ b as u64).wrapping_mul(1_099_511_628_211);
    }
    v
}

/// The shader members of a DAT of `size` bytes; None for another DAT format.
pub fn entries(read: Read, size: u64, label: &str) -> R<Option<Vec<TtEntry>>> {
    if size < 8 {
        return Ok(None);
    }
    let h = read(0, 8)?;
    let (mut offset, length) = (u32::from_le_bytes(h[..4].try_into().unwrap()) as u64, u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64);
    if offset & 0x8000_0000 != 0 {
        offset = ((offset ^ 0xffff_ffff) + 1) << 8;
    }
    if offset < 8 || offset + 12 > size {
        return Ok(None);
    }
    let head = read(offset, 32.min(size - offset))?;
    if head.get(4..12) != Some(&MAGIC[..]) {
        return Ok(None);
    }
    if head.len() < 32 {
        return Err(format!("{label}: CC4 index: truncated header"));
    }
    range(offset, length, size, &format!("{label}: CC4 index"))?;
    if !(32..=MAX_TABLE).contains(&length) {
        return Err(format!("{label}: CC4 index: invalid length {length}"));
    }
    let (hlen, kind, version, count, names, strings) = (be32(&head, 0) as u64, be32(&head, 12) as i32, be32(&head, 16), be32(&head, 20) as usize, be32(&head, 24) as usize, be32(&head, 28) as usize);
    if version != 2 || kind != -13 {
        return Err(format!("{label}: unsupported CC4 version {version}, layout {kind}"));
    }
    if hlen + 4 != length {
        return Err(format!("{label}: CC4 index length disagrees with its header"));
    }
    let names_at = 32 + strings + 4;
    let files_at = names_at + 12 * names + 8;
    let hashes_at = files_at + 16 * count;
    let len = length as usize;
    range(32, strings as u64, length, &format!("{label}: CC4 name strings"))?;
    range(names_at as u64, (12 * names + 8) as u64, length, &format!("{label}: CC4 name records"))?;
    range(files_at as u64, (24 * count) as u64, length, &format!("{label}: CC4 files and hashes"))?;
    let data = read(offset, length)?;
    if data.len() != len {
        return Err(format!("{label}: truncated CC4 index"));
    }
    if (be32(&data, files_at - 8) as i32, be32(&data, files_at - 4) as usize) != (kind, count) {
        return Err(format!("{label}: CC4 file counts disagree"));
    }
    let mut hashes = std::collections::HashMap::new();
    for i in 0..count {
        let k = u64::from_be_bytes(data[hashes_at + 8 * i..hashes_at + 8 * i + 8].try_into().unwrap());
        if hashes.insert(k, i).is_some() {
            return Err(format!("{label}: duplicate CC4 file hash"));
        }
    }
    let mut folders: std::collections::HashMap<usize, String> = [(0usize, String::new())].into();
    let mut out = vec![];
    for i in 0..names {
        let r = names_at + 12 * i;
        let no = be32(&data, r);
        let parent = u16::from_be_bytes([data[r + 4], data[r + 5]]) as usize;
        let file_id = u16::from_be_bytes([data[r + 10], data[r + 11]]);
        if no == 0xffff_ffff {
            continue;
        }
        let no = no as usize;
        range(no as u64, 1, strings as u64, &format!("{label}: CC4 name"))?;
        let s = &data[32 + no..32 + strings];
        let end = s.iter().position(|&b| b == 0).ok_or_else(|| format!("{label}: unterminated CC4 name"))?;
        let parent_path = folders.get(&parent).ok_or_else(|| format!("{label}: CC4 name {i} has an unknown parent {parent}"))?;
        let full = format!("{parent_path}{}", String::from_utf8_lossy(&s[..end]));
        if full.len() > 4096 {
            return Err(format!("{label}: CC4 path exceeds 4096 characters"));
        }
        if file_id == 0 && i != names - 1 {
            folders.insert(i, full + "/");
            continue;
        }
        if !full.to_lowercase().ends_with(".nnx_shaders") {
            continue;
        }
        let &fi = hashes.get(&name_hash(&full)).ok_or_else(|| format!("{label}/{full}: no matching CC4 file hash"))?;
        let f = files_at + 16 * fi;
        let encoded = u64::from_be_bytes(data[f..f + 8].try_into().unwrap());
        let (stored, raw) = (be32(&data, f + 8) as u64, (be32(&data, f + 12) & 0x7fff_ffff) as u64);
        let at = encoded & 0x00ff_ffff_ffff_ffff; // the top byte carries the packing flags
        range(at, stored, offset, &format!("{label}/{full}"))?;
        if !(1..=MAX_MEMBER).contains(&raw) {
            return Err(format!("{label}/{full}: invalid shader member length {raw}"));
        }
        if !(1..=MAX_MEMBER * 2).contains(&stored) {
            return Err(format!("{label}/{full}: invalid stored shader length {stored}"));
        }
        out.push((full, at, stored, raw));
    }
    Ok(Some(out))
}

/// The DLC's DFLT chunks: one final DEFLATE block each, whose dynamic Huffman type is written as 0 instead of 2.
fn inflate_dflt(data: &[u8], size: usize, name: &str) -> R<Vec<u8>> {
    let mut d = data.to_vec();
    match d.first().map(|b| b & 7) {
        Some(1) => d[0] |= 4,
        Some(3) => {}
        h => return Err(format!("{name}: unsupported DFLT block header {h:?}")),
    }
    let mut out = Vec::with_capacity(size);
    let mut dec = flate2::Decompress::new(false);
    out.reserve(size + 1);
    let st = dec.decompress_vec(&d, &mut out, flate2::FlushDecompress::Finish).map_err(|_| format!("{name}: invalid DFLT stream"))?;
    if st != flate2::Status::StreamEnd || dec.total_in() as usize != d.len() || out.len() != size {
        return Err(format!("{name}: DFLT stream disagrees with its chunk length"));
    }
    Ok(out)
}

/// A member's bytes: stored, or its OODL / DFLT chunks decoded.
pub fn read_member(read: Read, e: &TtEntry, label: &str) -> R<Vec<u8>> {
    let (name, offset, stored, size) = (label, e.1, e.2, e.3 as usize);
    let tag = read(offset, 4.min(stored))?;
    if tag != b"OODL" && tag != b"DFLT" {
        if stored != size as u64 {
            return Err(format!("{name}: unsupported TT Games compression {tag:?}"));
        }
        let d = read(offset, stored)?;
        return if d.len() == size { Ok(d) } else { Err(format!("{name}: truncated stored member")) };
    }
    let all = read(offset, stored)?;
    if all.len() as u64 != stored {
        return Err(format!("{name}: truncated member"));
    }
    let codec = String::from_utf8_lossy(&tag).to_string();
    let (mut at, mut out) = (0usize, Vec::with_capacity(size));
    while at < all.len() {
        let h = all.get(at..at + 12).ok_or_else(|| format!("{name}: truncated {codec} header"))?;
        let (packed, raw) = (u32::from_le_bytes(h[4..8].try_into().unwrap()) as usize, u32::from_le_bytes(h[8..12].try_into().unwrap()) as usize);
        if h[..4] != tag[..] || packed == 0 || raw == 0 || packed > (all.len() - at - 12).min(MAX_MEMBER as usize) || raw > size - out.len() {
            return Err(format!("{name}: invalid {codec} chunk header"));
        }
        let d = &all[at + 12..at + 12 + packed];
        let chunk = if packed == raw {
            d.to_vec()
        } else if tag == b"DFLT" {
            inflate_dflt(d, raw, name)?
        } else {
            super::archive::oodle(d, raw, name)?
        };
        if chunk.len() != raw {
            return Err(format!("{name}: Oodle returned the wrong length"));
        }
        out.extend_from_slice(&chunk);
        at += 12 + packed;
    }
    if out.len() != size {
        return Err(format!("{name}: {codec} decoded length disagrees with the index"));
    }
    Ok(out)
}

/// An unreferenced control section's unique adjacent code area: right after it (as is or 16-aligned) or right
/// before it, within [start, end). Two headerless compute neighbours are told apart by a well-formed Maxwell
/// scheduling stream with an EXIT in an instruction slot; an ambiguous pair is left out.
fn adjacent(d: &[u8], c: usize, start: usize, end: usize) -> Option<(Stage, Vec<u8>, Vec<u8>)> {
    if c < start || c + 0x760 > end {
        return None;
    }
    let (cn, kn) = (u32_at(d, c + 0x20)? as usize, u32_at(d, c + 0x704)? as usize);
    if cn < 0x760 || c + cn > end || kn < 8 {
        return None;
    }
    let ctl = &d[c..c + cn];
    let mut ks = vec![c + cn, (c + cn + 15) & !15];
    if let Some(k) = c.checked_sub(kn) {
        ks.push(k);
    }
    ks.dedup();
    let mut cands = vec![];
    for k in ks {
        if k < start || k + kn > end {
            continue;
        }
        let code = &d[k..k + kn];
        if memchr::memmem::find(code, &CTL_MAGIC).is_some() {
            continue;
        }
        let Some(st) = glslc::stage(ctl, code) else { continue };
        if st == Stage::Cs && code[..4] != CS_MAGIC && code.iter().all(|&b| b == 0) {
            continue;
        }
        if check(st, ctl, code).is_none() {
            cands.push((st, ctl.to_vec(), code.to_vec()));
        }
    }
    if cands.len() > 1 {
        let headed: Vec<_> = cands.iter().filter(|v| v.2[..4] == GFX_MAGIC || v.2[..4] == CS_MAGIC).cloned().collect();
        if headed.len() == 1 {
            return headed.into_iter().next();
        }
        let exits = |v: &(Stage, Vec<u8>, Vec<u8>)| -> bool {
            let code = &v.2;
            let s = if code[..4] == CS_MAGIC { 0x100 } else { 0 };
            let c1 = u32_at(ctl, 0x700).unwrap_or(0) as usize;
            let e = if s < c1 && c1 <= code.len() { c1 } else { code.len() };
            let q = |i: usize| u64_at(code, i).unwrap_or(0);
            v.0 == Stage::Cs && (s..e.saturating_sub(7)).step_by(32).all(|i| q(i) >> 63 == 0) && (s..e.saturating_sub(7)).step_by(8).any(|i| (i - s) % 32 != 0 && q(i) >> 52 == 0xe30)
        };
        cands.retain(exits);
    }
    (cands.len() == 1).then(|| cands.pop().unwrap())
}

/// TT's serialized programs in a member: GLSLC outputs, then control sections outside them with their code beside
/// them (newer code), or, in older programs, a unique headed code area between the control section and the next one
/// (reflection data lies between).
pub fn programs(data: &[u8], label: &str, problems: &mut Vec<String>) -> Vec<Found> {
    let mut found = glslc::shaders_skip(data, label, &[]);
    let mut ranges: Vec<(usize, usize)> = vec![];
    for m in memchr::memmem::find_iter(data, &glslc::MAGIC) {
        let (size, _, _, _) = glslc::layout(data, m);
        if size > 0 && size <= data.len() - m {
            ranges.push((m, m + size));
        }
    }
    ranges.sort();
    let controls: Vec<usize> = memchr::memmem::find_iter(data, &CTL_MAGIC).collect();
    let (mut ri, mut previous_stop) = (0usize, 0usize);
    let next_range = |ri: usize| ranges.get(ri).copied().unwrap_or((data.len(), data.len()));
    for (i, &c) in controls.iter().enumerate() {
        let (mut start, mut stop) = next_range(ri);
        while c >= stop {
            previous_stop = previous_stop.max(stop);
            ri += 1;
            (start, stop) = next_range(ri);
        }
        if start <= c {
            continue; // inside a GLSLC output
        }
        if c + 0x760 > data.len() {
            problems.push(format!("{label}: truncated control section @{c:#x}"));
            continue;
        }
        let (cn, kn) = (u32_at(data, c + 0x20).unwrap_or(0) as usize, u32_at(data, c + 0x704).unwrap_or(0) as usize);
        let end = start.min(controls.get(i + 1).copied().unwrap_or(data.len()));
        let mut cand = adjacent(data, c, previous_stop, end);
        if cand.is_none() && cn >= 0x760 && c + cn <= data.len() && kn >= 0x80 {
            let ctl = &data[c..c + cn];
            let mut v = vec![];
            let from = c + cn;
            if from < end {
                for k in (from..end.saturating_sub(3)).filter(|&k| (data[k] == b'x' || data[k] == b'y') && data[k + 1..k + 4] == [0x56, 0x34, 0x12]) {
                    if k + kn > end {
                        continue;
                    }
                    let code = &data[k..k + kn];
                    if let Some(st) = glslc::stage(ctl, code).filter(|&st| check(st, ctl, code).is_none()) {
                        v.push((st, ctl.to_vec(), code.to_vec()));
                    }
                }
            }
            if v.len() == 1 {
                cand = v.pop();
            }
        }
        match cand {
            Some((st, ctl, code)) => found.push(Found::new(format!("{label}#RAW@{c:#x}"), st, &ctl, &code)),
            None => problems.push(format!("{label} control section @{c:#x}: no unique code area in its program")),
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CC4 v2 DAT built by hand: one folder, a shader member stored and one in a DFLT chunk, a texture skipped.
    fn dat(members: &[(&str, Vec<u8>, bool)]) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut name_recs: Vec<(u32, u16, u16)> = vec![(0, 0, 0)]; // record 0: the root folder (no name)
        strings.extend_from_slice(b"\0");
        let folder_no = strings.len() as u32;
        strings.extend_from_slice(b"SHADERS\0");
        name_recs.push((folder_no, 0, 0));
        let mut body = vec![0u8; 16];
        let mut files: Vec<(u64, u64, u32, u32)> = vec![];
        for (i, (n, data, deflate)) in members.iter().enumerate() {
            let no = strings.len() as u32;
            strings.extend_from_slice(n.as_bytes());
            strings.push(0);
            name_recs.push((no, 1, (i + 1) as u16));
            let at = body.len() as u64;
            if *deflate {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
                std::io::Write::write_all(&mut e, data).unwrap();
                let mut c = e.finish().unwrap();
                // the DLC's writer stores a dynamic block's type as 0
                if c[0] & 7 == 5 {
                    c[0] &= !4;
                }
                body.extend_from_slice(b"DFLT");
                body.extend_from_slice(&(c.len() as u32).to_le_bytes());
                body.extend_from_slice(&(data.len() as u32).to_le_bytes());
                body.extend_from_slice(&c);
            } else {
                body.extend_from_slice(data);
            }
            files.push((name_hash(&format!("SHADERS/{n}")), at, (body.len() as u64 - at) as u32, data.len() as u32));
        }
        // the last name record must not be a folder with file id 0 unless it is the last: fine
        files.sort_by_key(|f| f.0);
        let count = files.len();
        let names = name_recs.len();
        let mut idx = Vec::new();
        let strings_len = strings.len();
        let mut tail = Vec::new();
        tail.extend_from_slice(&strings);
        tail.extend_from_slice(&[0, 0, 0, 0]);
        for (no, parent, fid) in &name_recs {
            tail.extend_from_slice(&no.to_be_bytes());
            tail.extend_from_slice(&parent.to_be_bytes());
            tail.extend_from_slice(&[0, 0, 0, 0]);
            tail.extend_from_slice(&fid.to_be_bytes());
        }
        tail.extend_from_slice(&(-13i32).to_be_bytes());
        tail.extend_from_slice(&(count as u32).to_be_bytes());
        for f in &files {
            tail.extend_from_slice(&f.1.to_be_bytes());
            tail.extend_from_slice(&f.2.to_be_bytes());
            tail.extend_from_slice(&f.3.to_be_bytes());
        }
        for f in &files {
            tail.extend_from_slice(&f.0.to_be_bytes());
        }
        let length = 32 + tail.len();
        idx.extend_from_slice(&((length - 4) as u32).to_be_bytes());
        idx.extend_from_slice(MAGIC);
        idx.extend_from_slice(&(-13i32).to_be_bytes());
        idx.extend_from_slice(&2u32.to_be_bytes());
        idx.extend_from_slice(&(count as u32).to_be_bytes());
        idx.extend_from_slice(&(names as u32).to_be_bytes());
        idx.extend_from_slice(&(strings_len as u32).to_be_bytes());
        idx.extend_from_slice(&tail);
        let offset = body.len() as u32;
        body[..4].copy_from_slice(&offset.to_le_bytes());
        body[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        body.extend_from_slice(&idx);
        body
    }

    #[test]
    fn cc4_dat_members_and_adjacent_programs() {
        let (ctl, code) = synth::nvn1_ps(3);
        let mut program = vec![0u8; 32];
        program.extend_from_slice(&ctl);
        while program.len() % 16 != 0 {
            program.push(0);
        }
        program.extend_from_slice(&code);
        let d = dat(&[("A.NNX_SHADERS", program.clone(), false), ("B.nnx_shaders", program.clone(), true), ("C.TEX", vec![1, 2, 3], false)]);
        let read = |o: u64, n: u64| -> R<Vec<u8>> { Ok(d.get(o as usize..(o + n) as usize).map(|s| s.to_vec()).unwrap_or_else(|| d[o as usize..].to_vec())) };
        let es = entries(&read, d.len() as u64, "t.dat").unwrap().expect("a CC4 DAT");
        assert_eq!(es.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(), ["SHADERS/A.NNX_SHADERS", "SHADERS/B.nnx_shaders"]);
        for e in &es {
            let m = read_member(&read, e, &e.0).unwrap();
            assert_eq!(m, program, "{} decodes", e.0);
            let mut probs = vec![];
            let f = programs(&m, &e.0, &mut probs);
            assert_eq!(f.len(), 1, "{probs:?}");
            assert_eq!((f[0].stage, f[0].ctl.as_slice(), f[0].code.as_slice()), (Stage::Ps, &ctl[..], &code[..]));
        }
        // another DAT format is not taken
        let other = vec![0u8; 64];
        let r2 = |o: u64, n: u64| -> R<Vec<u8>> { Ok(other[o as usize..(o + n) as usize].to_vec()) };
        assert!(entries(&r2, 64, "x.dat").unwrap().is_none());
        assert_eq!(name_hash("a/b"), name_hash("\\A\\B"));
    }
}
