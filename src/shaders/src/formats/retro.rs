//! Retro Studios' engine (Donkey Kong Country: Tropical Freeze, Metroid Prime Remastered, Metroid Prime 4: Beyond):
//! NVN programs in the SNVN chunks of RFRM forms: `MaterialArchive.arc` (one `MTRL` form), the `MTRL` resources of the
//! `.pak` archives (an RFRM `PACK` with an `ADIR` table), and the startup material archive compressed (LZSS) in main.

use crate::nvn::*;
use crate::formats::{archive, glslc};

pub const RFRM: &[u8; 4] = b"RFRM";
const SNVN: &[u8; 4] = b"SNVN";

fn u64_order(d: &[u8], o: usize, big: bool) -> Option<u64> {
    let b: [u8; 8] = d.get(o..o + 8)?.try_into().ok()?;
    Some(if big { u64::from_be_bytes(b) } else { u64::from_le_bytes(b) })
}

fn u32_order(d: &[u8], o: usize, big: bool) -> Option<u32> {
    let b: [u8; 4] = d.get(o..o + 4)?.try_into().ok()?;
    Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
}

/// The byte order of the form or chunk at `o`, from its u64 length at +4: big-endian in Tropical Freeze (the Wii U
/// engine), little-endian later. The reading that is a length is the smaller one. → true for big-endian.
fn big_endian(d: &[u8], o: usize) -> bool {
    match (u64_order(d, o + 4, false), u64_order(d, o + 4, true)) {
        (Some(le), Some(be)) => le >= be,
        _ => false,
    }
}

/// A chunk at `o`: fourcc, u64 data length, two u32 versions, its data from +0x18. → (data start, data end, big-endian).
fn chunk(d: &[u8], o: usize) -> Option<(usize, usize, bool)> {
    let big = big_endian(d, o);
    let n = u64_order(d, o + 4, big)? as usize;
    let a = o + 0x18;
    Some((a, a.checked_add(n)?.min(d.len()), big))
}

fn inflate_exact(s: &[u8]) -> Option<Vec<u8>> {
    let mut z = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(s.len() * 4);
    loop {
        out.reserve(1 << 16);
        let before = z.total_in();
        match z.decompress_vec(&s[z.total_in() as usize..], &mut out, flate2::FlushDecompress::Finish) {
            Ok(flate2::Status::StreamEnd) => return (z.total_in() as usize == s.len()).then_some(out),
            Ok(_) if z.total_in() == before && out.len() < out.capacity() => return None,
            Ok(_) => {}
            Err(_) => return None,
        }
        if out.len() > 1 << 28 {
            return None;
        }
    }
}

fn zstd_all(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![];
    let mut r = zstd::stream::read::Decoder::new(s).ok()?;
    std::io::Read::read_to_end(&mut r, &mut out).ok()?;
    Some(out)
}

/// The program records of an SNVN chunk's data `d[a..b]`: each a u32 code length, u32 control length, a byte, u32 stream
/// length (the chunk's byte order), then a zlib stream (or zstd frames, Metroid Prime 4) that decodes to exactly the
/// two lengths: every stage's code area back to back, then their control sections in the same order. The records are
/// found by their own lengths. → [(offset, [(control, code)])]
pub fn snvn_records(d: &[u8], a: usize, b: usize, label: &str, big: bool) -> R<Vec<(usize, Vec<(Vec<u8>, Vec<u8>)>)>> {
    let mut out = vec![];
    let mut i = a + 12;
    loop {
        let from = i + 1;
        if from >= b {
            break;
        }
        let hit = d[from..b].iter().enumerate().find(|(k, &c)| c == 0x78 || (c == 0x28 && d[from + k..b].starts_with(&archive::ZSTD_MAGIC))).map(|(k, _)| from + k);
        let Some(at) = hit else { break };
        i = at;
        if at < 13 {
            continue;
        }
        let ncode = u32_order(d, at - 13, big).unwrap_or(0) as usize;
        let nctl = u32_order(d, at - 9, big).unwrap_or(0) as usize;
        let nz = u32_order(d, at - 4, big).unwrap_or(0) as usize;
        if !(nctl != 0 && nctl % 4 == 0 && nctl < 1 << 20 && ncode < 1 << 24 && nz > 0 && nz <= b - at) {
            continue;
        }
        let y = if d[at] == 0x78 { inflate_exact(&d[at..at + nz]) } else { zstd_all(&d[at..at + nz]) };
        let Some(y) = y else { continue };
        if y.len() != ncode + nctl {
            continue;
        }
        let (mut stages, mut k, mut c) = (vec![], 0usize, ncode);
        while c < y.len() {
            if y.get(c..c + 4) != Some(&CTL_MAGIC[..]) {
                return Err(format!("{label} SNVN record @{at:#x}: no control section at {c:#x}"));
            }
            let mut n = u32_at(&y, c + 0x20).unwrap_or(0) as usize;
            let nk = u32_at(&y, c + 0x704).unwrap_or(0) as usize;
            // data GLSLC put after the section, not counted in its own length (+0x7e8 offset, +0x7ec length)
            let (x, xn) = (u32_at(&y, c + 0x7e8).unwrap_or(0) as usize, u32_at(&y, c + 0x7ec).unwrap_or(0) as usize);
            if xn != 0 && x >= n {
                n = x + xn;
            }
            if n == 0 || c + n > y.len() || k + nk > ncode {
                return Err(format!("{label} SNVN record @{at:#x}: a control section at {c:#x} runs past the record"));
            }
            stages.push((y[c..c + n].to_vec(), y[k..k + nk].to_vec()));
            k += nk;
            c += n;
            if c < y.len() && y.get(c..c + 4) != Some(&CTL_MAGIC[..]) {
                // Metroid Prime 4: the next section starts on 16 bytes
                c = ncode + ((c - ncode + 15) & !15);
            }
        }
        if k != ncode {
            return Err(format!("{label} SNVN record @{at:#x}: the code areas end at {k:#x}, the control sections start at {ncode:#x}"));
        }
        out.push((at, stages));
        i = at + nz - 1;
    }
    Ok(out)
}

/// Every SNVN chunk of a form: each record is one program (its stages pair).
pub fn snvn_shaders(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(p) = memchr::memmem::find(&d[from..], SNVN).map(|x| x + from) {
        let Some((a, b, big)) = chunk(d, p) else { break };
        for (i, stages) in snvn_records(d, a, b, label, big)? {
            for (ctl, code) in stages {
                let st = glslc::stage(&ctl, &code);
                let why = match st {
                    Some(st) => check(st, &ctl, &code),
                    None => Some("no stage in the code area"),
                };
                if let Some(w) = why {
                    return Err(format!("{label} SNVN record @{i:#x}: {w}"));
                }
                out.push(Found::new(format!("{label}#SNVN@{i:#x}#var"), st.unwrap(), &ctl, &code).grouped(format!("{label}#SNVN@{i:#x}")));
            }
        }
        from = b.max(p + 4);
    }
    Ok(out)
}

/// Retro's LZSS on 8-bit units: a flag byte read from its high bit; a set bit is a back reference of two bytes (length
/// = high nibble + 3, distance = the other 12 bits), a clear one a literal. → the first `n` bytes, or None when the
/// stream runs out or points before its start.
pub fn lzss8(s: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut i, mut flags, mut left) = (0usize, 0u8, 0);
    while out.len() < n {
        if left == 0 {
            flags = *s.get(i)?;
            i += 1;
            left = 8;
        }
        if flags & 0x80 != 0 {
            let (b0, b1) = (*s.get(i)?, *s.get(i + 1)?);
            i += 2;
            let ln = (b0 >> 4) as usize + 3;
            let dist = ((b0 as usize & 0xf) << 8) | b1 as usize;
            if dist == 0 || dist > out.len() {
                return None;
            }
            for _ in 0..ln {
                out.push(out[out.len() - dist]);
            }
        } else {
            out.push(*s.get(i)?);
            i += 1;
        }
        flags <<= 1;
        left -= 1;
    }
    out.truncate(n);
    Some(out)
}

/// RFRM forms compressed into main (a type byte 1 and three zero bytes, then LZSS): Tropical Freeze's startup material
/// archive, found by the stream's first literals 'RFRM'.
pub fn main_forms(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for p in memchr::memmem::find_iter(d, RFRM) {
        if p < 5 || d[p - 5] != 1 || d[p - 4..p - 1] != [0, 0, 0] {
            continue;
        }
        let Some(head) = lzss8(&d[p - 1..], 0x20) else { continue };
        if &head[..4] != RFRM || &head[0x14..0x18] != b"MTRL" {
            continue;
        }
        let Some(n) = u64_order(&head, 4, big_endian(&head, 0)).filter(|&n| n < 1 << 28) else { continue };
        if let Some(x) = lzss8(&d[p - 1..], 0x20 + n as usize) {
            out.extend(snvn_shaders(&x, &format!("{label}@{:#x}", p - 5))?);
        }
    }
    Ok(out)
}

/// A Retro RFRM file: a `PACK` archive (its `ADIR` resources, `MTRL` ones read) or a single form.
pub fn rfrm(d: &[u8], label: &str) -> R<Vec<Found>> {
    if d.get(0x14..0x18) != Some(b"PACK") {
        return snvn_shaders(d, label);
    }
    let a = memchr::memmem::find(d.get(0x20..).unwrap_or(&[]), b"ADIR").map(|x| x + 0x20).ok_or_else(|| format!("{label}: RFRM PACK without an ADIR chunk"))?;
    let (_, end, big) = chunk(d, a).ok_or_else(|| format!("{label}: ADIR chunk cut short"))?;
    let n = u32_order(d, a + 0x18, big).ok_or_else(|| format!("{label}: ADIR cut short"))? as usize;
    let size = if n != 0 { (end.saturating_sub(a + 0x1c)) / n } else { 36 };
    if !matches!(size, 36 | 52 | 60) || a + 0x1c + n * size != end {
        return Err(format!("{label}: ADIR of {n} entries in {:#x} bytes, neither 36, 52 nor 60 bytes each", end.saturating_sub(a + 0x1c)));
    }
    let mut out = vec![];
    for i in 0..n {
        let e = a + 0x1c + size * i;
        if d.get(e..e + 4) != Some(b"MTRL") {
            continue;
        }
        let guid: String = d[e + 4..e + 20].iter().map(|b| format!("{b:02x}")).collect();
        let at = e + if size == 36 { 20 } else { 28 };
        let (off, ln) = (u64_order(d, at, big).unwrap_or(0), u64_order(d, at + 8, big).unwrap_or(0));
        let stored = if size == 36 { ln } else { u64_order(d, e + 44, big).unwrap_or(0) };
        if stored != ln {
            return Err(format!("{label}: MTRL resource {guid} is stored compressed ({stored:#x} of {ln:#x} bytes), which this reader does not decode"));
        }
        let r = span(d, off, ln, &format!("{label}/{guid}"))?;
        if r.get(..4) != Some(&RFRM[..]) {
            return Err(format!("{label}: MTRL resource {guid} is not an RFRM form"));
        }
        let r: Vec<u8> = match r.get(0x20..0x24) {
            Some(x) if x[0] == 0x78 => archive::unzlib(&r[0x20..], label)?,
            Some(x) if x == archive::ZSTD_MAGIC => zstd_all(&r[0x20..]).ok_or_else(|| format!("{label}/{guid}: bad zstd stream"))?,
            _ => r.to_vec(),
        };
        out.extend(snvn_shaders(&r, &format!("{label}/{guid}"))?);
    }
    Ok(out)
}

/// A large RFRM file read by range (Metroid Prime 4's packs reach 3.9 GB): its ADIR table, then only the MTRL
/// resources.
pub fn rfrm_read(read: &dyn Fn(u64, u64) -> R<Vec<u8>>, size: u64, label: &str) -> R<Vec<Found>> {
    let head = read(0, size.min(16 << 20))?;
    if head.get(0x14..0x18) != Some(b"PACK") {
        return rfrm(&read(0, size)?, label);
    }
    let a = memchr::memmem::find(head.get(0x20..).unwrap_or(&[]), b"ADIR").map(|x| x + 0x20).ok_or_else(|| format!("{label}: RFRM PACK without an ADIR chunk in its first 16 MB"))?;
    let big = big_endian(&head, a);
    let n = u64_order(&head, a + 4, big).ok_or_else(|| format!("{label}: ADIR cut short"))?;
    let adir = read(a as u64, 0x18 + n)?;
    // the table as a small form of its own: the same chunk at offset 0x20 of a PACK header
    let mut d = head[..0x20].to_vec();
    d.extend(&adir);
    let end = 0x20 + adir.len();
    let cnt = u32_order(&d, 0x20 + 0x18, big).ok_or_else(|| format!("{label}: ADIR cut short"))? as usize;
    let esz = if cnt != 0 { (end - 0x20 - 0x1c) / cnt } else { 36 };
    if !matches!(esz, 36 | 52 | 60) || 0x20 + 0x1c + cnt * esz != end {
        return Err(format!("{label}: ADIR of {cnt} entries, neither 36, 52 nor 60 bytes each"));
    }
    let mut out = vec![];
    for i in 0..cnt {
        let e = 0x20 + 0x1c + esz * i;
        if &d[e..e + 4] != b"MTRL" {
            continue;
        }
        let guid: String = d[e + 4..e + 20].iter().map(|b| format!("{b:02x}")).collect();
        let at = e + if esz == 36 { 20 } else { 28 };
        let (off, ln) = (u64_order(&d, at, big).unwrap_or(0), u64_order(&d, at + 8, big).unwrap_or(0));
        let stored = if esz == 36 { ln } else { u64_order(&d, e + 44, big).unwrap_or(0) };
        if stored != ln {
            return Err(format!("{label}: MTRL resource {guid} is stored compressed ({stored:#x} of {ln:#x} bytes), which this reader does not decode"));
        }
        let r = read(off, ln)?;
        if r.get(..4) != Some(&RFRM[..]) {
            return Err(format!("{label}: MTRL resource {guid} is not an RFRM form"));
        }
        let r: Vec<u8> = match r.get(0x20..0x24) {
            Some(x) if x[0] == 0x78 => archive::unzlib(&r[0x20..], label)?,
            Some(x) if x == archive::ZSTD_MAGIC => zstd_all(&r[0x20..]).ok_or_else(|| format!("{label}/{guid}: bad zstd stream"))?,
            _ => r,
        };
        out.extend(snvn_shaders(&r, &format!("{label}/{guid}"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;
    use std::io::Write;

    fn record(stages: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
        let mut y = vec![];
        for (_, k) in stages {
            y.extend(k);
        }
        let ncode = y.len();
        for (c, _) in stages {
            y.extend(c);
        }
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        z.write_all(&y).unwrap();
        let s = z.finish().unwrap();
        let mut r = vec![];
        r.extend((ncode as u32).to_be_bytes());
        r.extend(((y.len() - ncode) as u32).to_be_bytes());
        r.push(0);
        r.extend((s.len() as u32).to_be_bytes());
        r.extend(s);
        r
    }

    #[test]
    fn snvn_form_and_lzss() {
        let rec = record(&[nvn1_vs(1), nvn1_ps(2)]);
        let mut chunk_data = vec![0u8; 16];
        chunk_data.extend(&rec);
        let mut form = vec![];
        form.extend(b"RFRM");
        form.extend(((0x18 + chunk_data.len()) as u64).to_be_bytes());
        form.extend([0u8; 8]);
        form.extend(b"MTRL");
        form.extend([0u8; 8]);
        form.extend(b"SNVN");
        form.extend((chunk_data.len() as u64).to_be_bytes());
        form.extend([0u8; 12]);
        form.extend(&chunk_data);
        let f = rfrm(&form, "/MaterialArchive.arc").unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].stage, Stage::Vs);
        assert_eq!(f[1].stage, Stage::Ps);
        // LZSS: all literals, flag bytes of 0
        let mut s = vec![];
        for c in form.chunks(8) {
            s.push(0);
            s.extend(c);
        }
        assert_eq!(lzss8(&s, form.len()).unwrap(), form);
        let mut main = vec![0u8; 3];
        main.extend([1, 0, 0, 0]);
        main.extend(&s);
        assert_eq!(main_forms(&main, "exefs/main").unwrap().len(), 2);
        // a back reference: "ab" then copy 4 from distance 2
        assert_eq!(lzss8(&[0b0010_0000, b'a', b'b', 0x10, 0x02], 6).unwrap(), b"ababab");
    }
}
