//! Compression and archives shaders sit inside: Yaz0, SARC, zstd (with the game's dictionaries), gzip, size-prefixed
//! zstd / LZ4 frames, Oodle (Kraken, Mermaid, Selkie, Leviathan), CRI's CPK and CRILAYLA.

use crate::nvn::*;
use std::collections::HashMap;
use std::io::Read;

pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
pub const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4d, 0x18];
pub const ZSDIC_MAGIC: [u8; 4] = [0x37, 0xa4, 0x30, 0xec];
/// The most bytes one decoded stream may give (a damaged length cannot make the scan allocate more).
pub const DECODE_LIMIT: u64 = 1 << 32;

/// Yaz0: group header bits, 1 = a literal byte, 0 = a back-reference {length nibble, 12-bit distance + 1}; length 0
/// reads one more byte + 0x12, else + 2.
pub fn yaz0(s: &[u8], label: &str) -> R<Vec<u8>> {
    let n = u32::from_be_bytes(
        s.get(4..8)
            .ok_or_else(|| format!("{label}: a Yaz0 header cut short"))?
            .try_into()
            .unwrap(),
    ) as usize;
    if n as u64 > DECODE_LIMIT {
        return Err(format!("{label}: a Yaz0 stream of {n:#x} bytes"));
    }
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut si = 16usize;
    let cut = |len: usize| {
        format!(
            "{label}: the Yaz0 stream ends at {:#x} with {len:#x} of its {n:#x} bytes decoded",
            s.len()
        )
    };
    while out.len() < n {
        let mut h = *s.get(si).ok_or_else(|| cut(out.len()))?;
        si += 1;
        for _ in 0..8 {
            if h & 0x80 != 0 {
                out.push(*s.get(si).ok_or_else(|| cut(out.len()))?);
                si += 1;
            } else {
                let (b1, b2) = (
                    *s.get(si).ok_or_else(|| cut(out.len()))?,
                    *s.get(si + 1).ok_or_else(|| cut(out.len()))?,
                );
                si += 2;
                let dist = (((b1 & 15) as usize) << 8 | b2 as usize) + 1;
                let ln = if b1 >> 4 != 0 {
                    (b1 >> 4) as usize + 2
                } else {
                    let x = *s.get(si).ok_or_else(|| cut(out.len()))? as usize + 0x12;
                    si += 1;
                    x
                };
                let st = out.len().checked_sub(dist).ok_or_else(|| {
                    format!("{label}: a back-reference at {si:#x} reaches before the start")
                })?;
                for i in 0..ln {
                    out.push(out[st + i]);
                }
            }
            if out.len() >= n {
                break;
            }
            h <<= 1;
        }
    }
    out.truncate(n);
    Ok(out)
}

/// (name, bytes) of each member of a SARC. A member outside the data is a damaged archive.
pub fn sarc_members<'a>(d: &'a [u8], label: &str) -> R<Vec<(String, &'a [u8])>> {
    let hl = need16(d, 4, &format!("{label} SARC header"))? as usize;
    let doff = need32(d, 0xc, &format!("{label} SARC header"))? as usize;
    let n = need16(d, hl + 6, &format!("{label} SFAT header"))? as usize;
    let names = hl + 12 + 16 * n + 8;
    if !(names <= doff && doff <= d.len()) {
        return Err(format!("{label}: SARC data at {doff:#x}, its {n} entries and the names end at {names:#x}, the archive at {:#x}", d.len()));
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let e = hl + 12 + 16 * i;
        let (attr, s, en) = (
            u32_at(d, e + 4).unwrap(),
            u32_at(d, e + 8).unwrap() as usize,
            u32_at(d, e + 12).unwrap() as usize,
        );
        if !(s <= en && en <= d.len() - doff) {
            return Err(format!("{label}: SARC member {i} at {s:#x}..{en:#x} of the data lies outside its {:#x} bytes", d.len() - doff));
        }
        let nm = if attr >> 24 != 0 {
            let o = names + (attr & 0xffffff) as usize * 4;
            let tail = d.get(o..).unwrap_or(&[]);
            let z = memchr::memchr(0, tail).unwrap_or(tail.len());
            String::from_utf8_lossy(&tail[..z]).into_owned()
        } else {
            format!("#{i}")
        };
        out.push((nm, &d[doff + s..doff + en]));
    }
    Ok(out)
}

/// The game's zstd dictionaries by ID (Tears of the Kingdom ships them as `Pack/ZsDic.pack.zs`, a SARC of `.zsdic`).
#[derive(Default)]
pub struct ZstdDicts {
    pub raw: HashMap<u32, Vec<u8>>,
    prepared: HashMap<u32, zstd::dict::DecoderDictionary<'static>>,
}

impl ZstdDicts {
    pub fn from_pack(pack_zs: &[u8], label: &str) -> R<ZstdDicts> {
        let sarc = unzstd(pack_zs, &ZstdDicts::default(), label)?;
        let mut d = ZstdDicts::default();
        for (_, m) in sarc_members(&sarc, label)? {
            if m.len() >= 8 && m[..4] == ZSDIC_MAGIC {
                let id = u32_at(m, 4).unwrap();
                d.prepared
                    .insert(id, zstd::dict::DecoderDictionary::copy(m));
                d.raw.insert(id, m.to_vec());
            }
        }
        Ok(d)
    }
    pub fn len(&self) -> usize {
        self.raw.len()
    }
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
}

/// The zstd dictionary a frame names (0: none).
pub fn zstd_dict_id(d: &[u8]) -> u32 {
    if d.len() <= 5 || d[..4] != ZSTD_MAGIC {
        return 0;
    }
    let fhd = d[4];
    let n = [0usize, 1, 2, 4][(fhd & 3) as usize];
    let o = 6 - ((fhd >> 5) & 1) as usize;
    let mut v = 0u32;
    for i in 0..n {
        v |= (*d.get(o + i).unwrap_or(&0) as u32) << (8 * i);
    }
    v
}

fn read_limited(mut r: impl Read, label: &str) -> R<Vec<u8>> {
    let mut out = vec![];
    (&mut r)
        .take(DECODE_LIMIT + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("{label}: zstd: {e}"))?;
    if out.len() as u64 > DECODE_LIMIT {
        return Err(format!(
            "{label}: a zstd stream past {DECODE_LIMIT:#x} bytes"
        ));
    }
    Ok(out)
}

/// zstd frames (all of them, as the command does), with the dictionary the first frame names.
pub fn unzstd(d: &[u8], dicts: &ZstdDicts, label: &str) -> R<Vec<u8>> {
    let did = zstd_dict_id(d);
    if did == 0 {
        let mut dec = zstd::stream::read::Decoder::with_buffer(d)
            .map_err(|e| format!("{label}: zstd: {e}"))?;
        dec.window_log_max(31)
            .map_err(|e| format!("{label}: zstd: {e}"))?;
        return read_limited(dec, label);
    }
    let dict = dicts.prepared.get(&did).ok_or_else(|| {
        format!("{label}: a frame needs zstd dictionary {did}, which the game does not ship")
    })?;
    let mut dec = zstd::stream::read::Decoder::with_prepared_dictionary(d, dict)
        .map_err(|e| format!("{label}: zstd: {e}"))?;
    dec.window_log_max(31)
        .map_err(|e| format!("{label}: zstd: {e}"))?;
    read_limited(dec, label)
}

/// One zstd frame at the start of `d`, decoded, and its compressed length.
pub fn zstd_frame(d: &[u8]) -> Option<(Vec<u8>, usize)> {
    let n = zstd::zstd_safe::find_frame_compressed_size(d).ok()?;
    let mut dec = zstd::stream::read::Decoder::with_buffer(&d[..n])
        .ok()?
        .single_frame();
    dec.window_log_max(31).ok()?;
    let mut out = vec![];
    dec.take(DECODE_LIMIT).read_to_end(&mut out).ok()?;
    Some((out, n))
}

/// The first `n` decoded bytes of the zstd frame at the start of `d` (a peek: a damaged frame gives None).
pub fn zstd_peek(d: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut dec = zstd::stream::read::Decoder::with_buffer(d)
        .ok()?
        .single_frame();
    dec.window_log_max(31).ok()?;
    let mut out = vec![0u8; n];
    let mut got = 0;
    while got < n {
        match dec.read(&mut out[got..]) {
            Ok(0) => break,
            Ok(k) => got += k,
            Err(_) => return None,
        }
    }
    out.truncate(got);
    Some(out)
}

/// Every gzip member, decoded.
pub fn ungzip(d: &[u8], label: &str) -> R<Vec<u8>> {
    read_limited(flate2::read::MultiGzDecoder::new(d), label).map_err(|e| e.replace("zstd", "gzip"))
}

pub fn unzlib(d: &[u8], label: &str) -> R<Vec<u8>> {
    read_limited(flate2::read::ZlibDecoder::new(d), label).map_err(|e| e.replace("zstd", "zlib"))
}

/// The uncompressed size a zstd or LZ4 frame header records.
fn frame_size(f: &[u8]) -> Option<u64> {
    if f.get(..4) == Some(&LZ4_FRAME_MAGIC[..]) {
        return if f.len() > 14 && f[4] & 8 != 0 {
            u64_at(f, 6)
        } else {
            None
        };
    }
    let fhd = *f.get(4)?;
    let single = ((fhd >> 5) & 1) as usize;
    let n = [single, 2, 4, 8][(fhd >> 6) as usize];
    if n == 0 {
        return None;
    }
    let o = 6 - single + [0usize, 1, 2, 4][(fhd & 3) as usize];
    let mut v = 0u64;
    for i in 0..n {
        v |= (*f.get(o + i)? as u64) << (8 * i);
    }
    Some(v + if n == 2 { 256 } else { 0 })
}

/// A size-prefixed frame (HAL's .cmp containers): a u32 size, then a zstd or LZ4 frame. Bytes whose frame records
/// another size are not such a container and come back as they are; so does a frame that does not open (reported).
pub fn size_prefixed<'a>(
    d: &'a [u8],
    dicts: &ZstdDicts,
    label: &str,
    problems: &mut Vec<String>,
) -> std::borrow::Cow<'a, [u8]> {
    let n = u32_at(d, 0).unwrap_or(0) as u64;
    let f = &d[4.min(d.len())..];
    match frame_size(f) {
        Some(s) if s != n => return d.into(),
        _ => {}
    }
    let dec = if f.get(..4) == Some(&LZ4_FRAME_MAGIC[..]) {
        let mut out = vec![];
        lz4_flex::frame::FrameDecoder::new(f)
            .take(DECODE_LIMIT)
            .read_to_end(&mut out)
            .map(|_| out)
            .map_err(|e| e.to_string())
    } else {
        unzstd(f, dicts, label)
    };
    match dec {
        Ok(v) if v.len() as u64 == n => v.into(),
        Ok(v) => {
            problems.push(format!("{label}: a size-prefixed frame that did not open (the size prefix says {n} bytes, the frame holds {}): the file is read as it is", v.len()));
            d.into()
        }
        Err(e) => {
            problems.push(format!(
                "{label}: a size-prefixed frame that did not open ({e}): the file is read as it is"
            ));
            d.into()
        }
    }
}

/// An Oodle stream (Kraken, Mermaid, Selkie or Leviathan blocks) of `n` decoded bytes, by a clean-room decoder.
/// LZNA, BitKnit and checksummed blocks are refused with the decoder's reason.
pub fn oodle(comp: &[u8], n: usize, label: &str) -> R<Vec<u8>> {
    if n as u64 > DECODE_LIMIT {
        return Err(format!("{label}: an Oodle stream of {n:#x} bytes"));
    }
    let mut out = vec![0u8; n];
    lztans::decode(comp, &mut out)
        .map_err(|e| format!("{label}: Oodle: the stream does not decode ({e})"))?;
    Ok(out)
}

/// CRILAYLA: 'CRILAYLA', u32 size, u32 compressed length, the compressed bits, then the output's first 0x100 bytes
/// stored. Bits are read from the end backwards, highest first; the output is written from its end backwards.
pub fn crilayla(c: &[u8]) -> R<Vec<u8>> {
    if c.get(..8) != Some(b"CRILAYLA") {
        return Err("not CRILAYLA".into());
    }
    let (usz, csz) = (
        u32_at(c, 8).unwrap_or(0) as usize,
        u32_at(c, 12).unwrap_or(0) as usize,
    );
    if 0x10 + csz + 0x100 > c.len() {
        return Err("CRILAYLA: the stream is cut short".into());
    }
    let mut out = vec![0u8; usz + 0x100];
    out[..0x100].copy_from_slice(&c[0x10 + csz..0x10 + csz + 0x100]);
    let src = &c[0x10..0x10 + csz];
    let total_bits = csz * 8;
    let mut p = 0usize;
    let bits = |p: &mut usize, n: usize| -> R<usize> {
        let mut v = 0usize;
        for _ in 0..n {
            if *p >= total_bits {
                return Err("CRILAYLA: the bits run out".into());
            }
            let byte = src[csz - 1 - *p / 8];
            v = v << 1 | ((byte >> (7 - (*p % 8))) & 1) as usize;
            *p += 1;
        }
        Ok(v)
    };
    let mut w = out.len() as isize - 1;
    while w >= 0x100 {
        if bits(&mut p, 1)? == 1 {
            let back = bits(&mut p, 13)? + 3;
            let mut v = bits(&mut p, 2)?;
            let mut ln = 3 + v;
            if v == 3 {
                v = bits(&mut p, 3)?;
                ln += v;
                if v == 7 {
                    v = bits(&mut p, 5)?;
                    ln += v;
                    if v == 31 {
                        loop {
                            v = bits(&mut p, 8)?;
                            ln += v;
                            if v != 255 {
                                break;
                            }
                        }
                    }
                }
            }
            for _ in 0..ln {
                let from = w as usize + back;
                if from >= out.len() || w < 0 {
                    return Err("CRILAYLA: a copy reaches past the output".into());
                }
                out[w as usize] = out[from];
                w -= 1;
            }
        } else {
            out[w as usize] = bits(&mut p, 8)? as u8;
            w -= 1;
        }
    }
    Ok(out)
}

/// CRI @UTF table → (table name, rows as name → value).
#[derive(Debug, Clone, PartialEq)]
pub enum UtfVal {
    Int(i128),
    Float(f32),
    Str(String),
    Data(Vec<u8>),
    None,
}

fn utf_unmask(b: &[u8]) -> Vec<u8> {
    let mut m: u32 = 0x655f;
    b.iter()
        .map(|&x| {
            let y = x ^ (m & 0xff) as u8;
            m = m.wrapping_mul(0x4115) & 0xffff;
            y
        })
        .collect()
}

pub fn utf_at(d: &[u8], o: usize, n: usize) -> Vec<u8> {
    let t = &d[o.min(d.len())..(o + n).min(d.len())];
    if t.get(..4) == Some(b"@UTF") {
        t.to_vec()
    } else {
        utf_unmask(t)
    }
}

pub fn utf(d: &[u8]) -> R<(String, Vec<HashMap<String, UtfVal>>)> {
    if d.get(..4) != Some(b"@UTF") {
        return Err("no @UTF table".into());
    }
    let be16 = |o: usize| -> R<usize> {
        d.get(o..o + 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]) as usize)
            .ok_or_else(|| "@UTF cut short".to_string())
    };
    let be32 = |o: usize| -> R<usize> {
        d.get(o..o + 4)
            .map(|s| u32::from_be_bytes(s.try_into().unwrap()) as usize)
            .ok_or_else(|| "@UTF cut short".to_string())
    };
    let (ro, so, dof, tn, nc, rw, nr) = (
        be16(10)?,
        be32(12)?,
        be32(16)?,
        be32(20)?,
        be16(24)?,
        be16(26)?,
        be32(28)?,
    );
    let b = 8;
    let sz = |off: usize| -> R<String> {
        let s = d.get(b + so + off..).ok_or("@UTF string out of range")?;
        let z = memchr::memchr(0, s).unwrap_or(s.len());
        Ok(String::from_utf8_lossy(&s[..z]).into_owned())
    };
    let val = |t: u8, p: usize| -> R<(UtfVal, usize)> {
        let g = |n: usize| {
            d.get(p..p + n)
                .ok_or_else(|| "@UTF value out of range".to_string())
        };
        Ok(match t {
            0 => (UtfVal::Int(g(1)?[0] as i128), 1),
            1 => (UtfVal::Int(g(1)?[0] as i8 as i128), 1),
            2 => (
                UtfVal::Int(u16::from_be_bytes(g(2)?.try_into().unwrap()) as i128),
                2,
            ),
            3 => (
                UtfVal::Int(i16::from_be_bytes(g(2)?.try_into().unwrap()) as i128),
                2,
            ),
            4 => (
                UtfVal::Int(u32::from_be_bytes(g(4)?.try_into().unwrap()) as i128),
                4,
            ),
            5 => (
                UtfVal::Int(i32::from_be_bytes(g(4)?.try_into().unwrap()) as i128),
                4,
            ),
            6 => (
                UtfVal::Int(u64::from_be_bytes(g(8)?.try_into().unwrap()) as i128),
                8,
            ),
            7 => (
                UtfVal::Int(i64::from_be_bytes(g(8)?.try_into().unwrap()) as i128),
                8,
            ),
            8 => (
                UtfVal::Float(f32::from_be_bytes(g(4)?.try_into().unwrap())),
                4,
            ),
            0xa => (
                UtfVal::Str(sz(u32::from_be_bytes(g(4)?.try_into().unwrap()) as usize)?),
                4,
            ),
            0xb => {
                let (a, n) = (
                    u32::from_be_bytes(g(4)?.try_into().unwrap()) as usize,
                    u32::from_be_bytes(d.get(p + 4..p + 8).ok_or("@UTF")?.try_into().unwrap())
                        as usize,
                );
                (
                    UtfVal::Data(
                        d.get(b + dof + a..b + dof + a + n)
                            .ok_or("@UTF data out of range")?
                            .to_vec(),
                    ),
                    8,
                )
            }
            _ => return Err(format!("@UTF column type {t:#x}")),
        })
    };
    enum Col {
        Const(UtfVal),
        Row(u8),
        Zero,
    }
    let mut cols = vec![];
    let mut p = 0x20;
    for _ in 0..nc {
        let fl = *d.get(p).ok_or("@UTF cut short")?;
        let name = sz(be32(p + 1)?)?;
        p += 5;
        match fl & 0xf0 {
            0x30 => {
                let (v, n) = val(fl & 0xf, p)?;
                p += n;
                cols.push((name, Col::Const(v)));
            }
            0x50 => cols.push((name, Col::Row(fl & 0xf))),
            _ => cols.push((name, Col::Zero)),
        }
    }
    let mut rows = vec![];
    for i in 0..nr {
        let mut p = b + ro + i * rw;
        let mut r = HashMap::new();
        for (name, c) in &cols {
            let v = match c {
                Col::Row(t) => {
                    let (v, n) = val(*t, p)?;
                    p += n;
                    v
                }
                Col::Const(v) => v.clone(),
                Col::Zero => UtfVal::None,
            };
            r.insert(name.clone(), v);
        }
        rows.push(r);
    }
    Ok((sz(tn)?, rows))
}

/// A CPK's files: (path, offset, stored length, length), by its TOC.
pub fn cpk_files(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    file_size: u64,
) -> R<Vec<(String, u64, u64, u64)>> {
    let h = read(0, 0x800.min(file_size))?;
    if h.get(..4) != Some(b"CPK ") {
        return Err("not a CPK".into());
    }
    let n = need32(&h, 8, "CPK header")? as usize;
    let h = if 0x10 + n > h.len() {
        read(0, (0x10 + n) as u64)?
    } else {
        h
    };
    let (_, hd) = utf(&utf_at(&h, 0x10, n))?;
    let hd = hd.into_iter().next().ok_or("CPK header has no row")?;
    let int = |r: &HashMap<String, UtfVal>, k: &str| match r.get(k) {
        Some(UtfVal::Int(v)) => Some(*v as u64),
        _ => None,
    };
    let toc = int(&hd, "TocOffset")
        .filter(|&t| t != 0)
        .ok_or("no TOC (ITOC only is not read)")?;
    let ctt = int(&hd, "ContentOffset");
    let t = read(toc, 0x10)?;
    if t.get(..4) != Some(b"TOC ") {
        return Err(format!("no TOC at {toc:#x}"));
    }
    let n = need32(&t, 8, "TOC header")? as u64;
    let t = read(toc + 0x10, n)?;
    let t = utf_at(&t, 0, n as usize);
    let toc_base = toc.min(0x800);
    let base = match ctt {
        Some(c) if c != 0 => c.min(toc_base),
        _ => toc_base,
    };
    let mut out = vec![];
    for r in utf(&t)?.1 {
        let s = |k: &str| match r.get(k) {
            Some(UtfVal::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let dir = s("DirName");
        let nm = if dir.is_empty() {
            s("FileName")
        } else {
            format!("{dir}/{}", s("FileName"))
        };
        let off = int(&r, "FileOffset").ok_or("TOC row without FileOffset")?;
        let csz = int(&r, "FileSize").ok_or("TOC row without FileSize")?;
        let usz = int(&r, "ExtractSize").unwrap_or(csz);
        out.push((nm, base + off, csz, usz));
    }
    Ok(out)
}

/// A CPK entry's bytes: CRILAYLA-decoded when compressed.
pub fn cpk_entry(raw: Vec<u8>, usz: u64, label: &str) -> R<Vec<u8>> {
    let d = if raw.len() as u64 != usz || raw.get(..8) == Some(b"CRILAYLA") {
        crilayla(&raw).map_err(|e| format!("{label}: {e}"))?
    } else {
        raw
    };
    if d.len() as u64 != usz {
        return Err(format!("{label}: {} bytes, the TOC says {usz}", d.len()));
    }
    Ok(d)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A SARC of the given members (names in SFNT).
    pub fn make_sarc(members: &[(&str, &[u8])]) -> Vec<u8> {
        let hl = 0x14usize;
        let n = members.len();
        let mut names = vec![];
        let mut name_offs = vec![];
        for (nm, _) in members {
            name_offs.push(names.len() / 4);
            names.extend_from_slice(nm.as_bytes());
            names.push(0);
            while names.len() % 4 != 0 {
                names.push(0);
            }
        }
        let sfnt = hl + 12 + 16 * n;
        let doff = (sfnt + 8 + names.len() + 0xff) & !0xff;
        let mut d = vec![0u8; doff];
        d[..4].copy_from_slice(b"SARC");
        d[4..6].copy_from_slice(&(hl as u16).to_le_bytes());
        d[0xc..0x10].copy_from_slice(&(doff as u32).to_le_bytes());
        d[hl..hl + 4].copy_from_slice(b"SFAT");
        d[hl + 6..hl + 8].copy_from_slice(&(n as u16).to_le_bytes());
        d[sfnt..sfnt + 4].copy_from_slice(b"SFNT");
        d[sfnt + 8..sfnt + 8 + names.len()].copy_from_slice(&names);
        let mut data = vec![];
        for (i, (_, m)) in members.iter().enumerate() {
            let e = hl + 12 + 16 * i;
            d[e + 4..e + 8].copy_from_slice(&((1u32 << 24) | name_offs[i] as u32).to_le_bytes());
            d[e + 8..e + 12].copy_from_slice(&(data.len() as u32).to_le_bytes());
            data.extend_from_slice(m);
            d[e + 12..e + 16].copy_from_slice(&(data.len() as u32).to_le_bytes());
        }
        d.extend(data);
        let total = d.len() as u32;
        d[8..12].copy_from_slice(&total.to_le_bytes());
        d
    }

    /// Yaz0 with literals only.
    pub fn make_yaz0(data: &[u8]) -> Vec<u8> {
        let mut d = b"Yaz0".to_vec();
        d.extend_from_slice(&(data.len() as u32).to_be_bytes());
        d.resize(16, 0);
        for ch in data.chunks(8) {
            d.push(0xff);
            d.extend_from_slice(ch);
        }
        d
    }

    #[test]
    fn yaz0_literals_and_backrefs() {
        assert_eq!(
            yaz0(&make_yaz0(b"hello world!"), "t").unwrap(),
            b"hello world!"
        );
        // "ab" then a back-reference of 6 bytes at distance 2: "abababab"
        let mut d = b"Yaz0".to_vec();
        d.extend_from_slice(&8u32.to_be_bytes());
        d.resize(16, 0);
        d.extend_from_slice(&[0b1100_0000, b'a', b'b', 0x40, 0x01]);
        assert_eq!(yaz0(&d, "t").unwrap(), b"abababab");
        assert!(yaz0(&d[..18], "t").is_err());
    }

    #[test]
    fn sarc_names() {
        let s = make_sarc(&[("a/b.bin", b"xyz"), ("c", b"12345")]);
        let m = sarc_members(&s, "t").unwrap();
        assert_eq!(
            m,
            vec![
                ("a/b.bin".to_string(), &b"xyz"[..]),
                ("c".to_string(), &b"12345"[..])
            ]
        );
    }

    #[test]
    fn zstd_with_and_without_dictionary() {
        let plain = zstd::bulk::compress(b"shader shader shader", 3).unwrap();
        assert_eq!(
            unzstd(&plain, &ZstdDicts::default(), "t").unwrap(),
            b"shader shader shader"
        );
        // a dictionary-compressed frame names its dictionary; without it the frame is refused
        let samples: Vec<Vec<u8>> = (0..64)
            .map(|i| format!("control section {i} code area {}", i * 7).into_bytes())
            .collect();
        let dict = zstd::dict::from_samples(&samples, 4096).unwrap();
        let mut c = zstd::bulk::Compressor::with_dictionary(3, &dict).unwrap();
        let framed = c.compress(b"control section 5 code area 35").unwrap();
        let id = zstd_dict_id(&framed);
        assert_ne!(id, 0);
        assert!(unzstd(&framed, &ZstdDicts::default(), "t").is_err());
        let mut zsdic = ZSDIC_MAGIC.to_vec();
        zsdic.extend_from_slice(&dict[4..]);
        assert_eq!(
            &zsdic, &dict,
            "a zstd dictionary starts with the .zsdic magic"
        );
        let pack = zstd::bulk::compress(&make_sarc(&[("x.zsdic", &dict)]), 3).unwrap();
        let dicts = ZstdDicts::from_pack(&pack, "ZsDic").unwrap();
        assert_eq!(dicts.len(), 1);
        assert_eq!(
            unzstd(&framed, &dicts, "t").unwrap(),
            b"control section 5 code area 35"
        );
    }

    #[test]
    fn size_prefixed_frames() {
        let body = b"BNSH inside a HAL container".to_vec();
        let mut d = (body.len() as u32).to_le_bytes().to_vec();
        d.extend(zstd::bulk::compress(&body, 3).unwrap());
        let mut p = vec![];
        assert_eq!(
            &*size_prefixed(&d, &ZstdDicts::default(), "t", &mut p),
            &body[..]
        );
        // a prefix that is not the frame's size: the bytes as they are
        d[0] ^= 1;
        assert_eq!(
            &*size_prefixed(&d, &ZstdDicts::default(), "t", &mut p),
            &d[..]
        );
    }

    #[test]
    fn crilayla_literals() {
        // all-literal stream: for each output byte from the end, bit 0 then 8 bits
        let payload: Vec<u8> = (0..16u8).map(|i| i * 3 + 1).collect();
        let mut bitsv = vec![];
        for &b in payload.iter().rev() {
            bitsv.push(0u8);
            for k in (0..8).rev() {
                bitsv.push((b >> k) & 1);
            }
        }
        while bitsv.len() % 8 != 0 {
            bitsv.push(0);
        }
        let bytes: Vec<u8> = bitsv
            .chunks(8)
            .map(|c| c.iter().fold(0, |a, &x| a << 1 | x))
            .collect();
        // bits are read from the last byte backwards
        let comp: Vec<u8> = bytes.iter().rev().cloned().collect();
        let mut c = b"CRILAYLA".to_vec();
        c.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        c.extend_from_slice(&(comp.len() as u32).to_le_bytes());
        c.extend_from_slice(&comp);
        c.extend(vec![0x55u8; 0x100]);
        let out = crilayla(&c).unwrap();
        assert_eq!(&out[..0x100], &[0x55u8; 0x100][..]);
        assert_eq!(&out[0x100..], &payload[..]);
    }

    #[test]
    fn gzip_members() {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
        e.write_all(b"glslc").unwrap();
        let mut g = e.finish().unwrap();
        let mut e = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
        e.write_all(b" output").unwrap();
        g.extend(e.finish().unwrap());
        assert_eq!(ungzip(&g, "t").unwrap(), b"glslc output");
    }
}
