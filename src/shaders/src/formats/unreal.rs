//! Unreal Engine: pak files (versions 4, 8 and 11 with an unencrypted index; stored, Zlib, LZ4 or Oodle blocks),
//! shader code archives (`.ushaderbytecode` version 2: each shader stored, an LZ4 block or an Oodle stream) and the
//! shaders kept inline in cooked packages and global shader caches.
//!
//! A shader is an LSLG record ('LSLG', a stage tag SV / SH / SD / SG / SP / SC, a header with its source name, u32
//! code length + code area, u32 control length + control section, a short tail) or, on the Switch 2, an NVN2 record
//! with no tag (the same lengths around the code area and an NVN2 control section).
//!
//! Pak footer: legacy (version 4) the last 0x2d bytes {encrypted u8, magic, version, index offset u64, index size
//! u64, SHA-1}; versions 8 and 11 {key GUID 16, encrypted u8, magic, version, index offset, size, SHA-1, five 32-byte
//! compression method names}, the magic 0xc8 or 0xcc bytes from the end. Legacy index: mount point, count, then per
//! file its name and FPakEntry {offset, stored size, raw size (i64), compression (i32), SHA-1, compressed: i32 block
//! count + (start, end) i64 pairs, encrypted u8, block size u32}. Version 11: mount point, count, u64 seed, path hash
//! index (i32 flag, offset, size, SHA-1), full directory index (likewise), the encoded entries (i32 size + bytes), no
//! unencoded ones. An encoded entry's u32 bits: 0-5 block size >> 11 (0x3f: a u32 follows), 6-21 block count, 22
//! encrypted, 23-28 method (from 1), 29-31 stored size / raw size / offset fit 32 bits. A file's data follows a copy
//! of its FPakEntry (53 bytes, + 4 + 16 per block when compressed).

use super::archive;
use crate::nvn::*;

pub const PAK_MAGIC: u32 = 0x5A6F_12E1;

/// One file of a pak.
#[derive(Debug, Clone, PartialEq)]
pub struct PakEntry {
    pub name: String,
    pub off: u64,
    pub raw: u64,
    pub size: u64,
    pub method: String,
    pub encrypted: bool,
    pub block_size: u64,
    /// (start relative to `off`, stored length)
    pub blocks: Vec<(u64, u64)>,
}

struct Rd<'a> {
    b: &'a [u8],
    o: usize,
    what: &'a str,
}

impl<'a> Rd<'a> {
    fn raw(&mut self, n: usize) -> R<&'a [u8]> {
        let s = self
            .b
            .get(self.o..self.o.checked_add(n).unwrap_or(usize::MAX))
            .ok_or_else(|| {
                format!(
                    "{}: {n:#x} bytes at {:#x} do not fit in its {:#x}",
                    self.what,
                    self.o,
                    self.b.len()
                )
            })?;
        self.o += n;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.raw(1)?[0])
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.raw(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> R<i32> {
        Ok(self.u32()? as i32)
    }
    fn u64(&mut self) -> R<u64> {
        Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap()))
    }
    fn count(&mut self, max: usize) -> R<usize> {
        let n = self.i32()?;
        if n < 0 || n as usize > max {
            return Err(format!("{}: count {n} at {:#x}", self.what, self.o - 4));
        }
        Ok(n as usize)
    }
    fn fstr(&mut self) -> R<String> {
        let n = self.i32()?;
        if n < 0 {
            let b = self.raw(2 * n.unsigned_abs() as usize)?;
            let w: Vec<u16> = b
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            return Ok(String::from_utf16_lossy(&w[..w.len().saturating_sub(1)]));
        }
        let b = self.raw(n as usize)?;
        Ok(String::from_utf8_lossy(&b[..b.len().saturating_sub(1)]).into_owned())
    }
}

const LEGACY_METHODS: [&str; 4] = ["Zlib", "Gzip", "LZ5", "Custom"];

/// The files of a pak; `read(offset, length)` reads the pak.
/// A pak's footer: whether its index is encrypted, the index's offset and size, the compression method names, the
/// version.
pub struct PakFooter {
    pub encrypted: bool,
    pub index: (u64, u64),
    pub methods: Vec<String>,
    pub version: u32,
}

/// The footer of a pak (`read(offset, length)` reads it), or the reason it has none this reader knows.
pub fn pak_footer(read: &dyn Fn(u64, u64) -> R<Vec<u8>>, size: u64) -> R<PakFooter> {
    let tail_n = size.min(0xe1);
    let tail = read(size - tail_n, tail_n)?;
    let mut found = None;
    for (back, foot) in [(0x2cusize, 0x2dusize), (0xc8, 0xc8 + 21), (0xcc, 0xcc + 21)] {
        if tail.len() < foot {
            continue;
        }
        if u32_at(&tail, tail.len() - back) == Some(PAK_MAGIC) {
            found = Some((back, foot, u32_at(&tail, tail.len() - back + 4).unwrap()));
            break;
        }
    }
    let (back, foot, ver) = found.ok_or("no pak footer")?;
    let ft = &tail[tail.len() - foot..];
    let (enc, ioff, isz, methods): (bool, u64, u64, Vec<String>) = match (ver, back) {
        (4, 0x2c) => (ft[0] != 0, u64_at(ft, 9).unwrap(), u64_at(ft, 17).unwrap(), vec![]),
        (8 | 11, 0xc8 | 0xcc) => {
            let k = 21;
            let names = (0..5).map(|i| ft.get(k + 44 + 32 * i..(k + 76 + 32 * i).min(ft.len())).map(|s| String::from_utf8_lossy(s).trim_end_matches('\0').to_string()).unwrap_or_default()).collect();
            (ft[k - 1] != 0, u64_at(ft, k + 8).unwrap(), u64_at(ft, k + 16).unwrap(), names)
        }
        _ => return Err(format!("pak version {ver} (footer magic {back:#x} from the end); versions 4, 8 and 11 are read")),
    };
    if ioff.checked_add(isz).is_none_or(|e| e > size) {
        return Err(format!(
            "the index ({isz:#x} bytes at {ioff:#x}) runs past the end of the pak"
        ));
    }
    Ok(PakFooter {
        encrypted: enc,
        index: (ioff, isz),
        methods,
        version: ver,
    })
}

/// AES-256 ECB decryption in place (whole 16-byte blocks; a short tail is left as it is).
pub fn aes_decrypt(key: &[u8; 32], data: &mut [u8]) {
    use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
    let c = aes::Aes256::new(GenericArray::from_slice(key));
    for b in data.chunks_exact_mut(16) {
        c.decrypt_block(GenericArray::from_mut_slice(b));
    }
}

/// `n` bytes at `off`, decrypted with `key` when `enc` (read in whole 16-byte blocks).
fn read_at(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    off: u64,
    n: u64,
    enc: bool,
    key: Option<&[u8; 32]>,
) -> R<Vec<u8>> {
    if !enc {
        return read(off, n);
    }
    let key = key.ok_or("encrypted and no key")?;
    let mut b = read(off, (n + 15) & !15)?;
    aes_decrypt(key, &mut b);
    b.truncate(n as usize);
    Ok(b)
}

/// Whether `key` decrypts an index's first block to a mount point: an FString of length 2..511 that starts with '.'
/// or '/' and is printable as far as the block holds it.
pub fn opens(key: &[u8; 32], first: &[u8; 16]) -> bool {
    let mut d = *first;
    aes_decrypt(key, &mut d);
    let n = i32::from_le_bytes(d[..4].try_into().unwrap());
    if !(2..512).contains(&n) || !matches!(d[4], b'.' | b'/') {
        return false;
    }
    let s = &d[4..(4 + n as usize).min(16)];
    let n = n as usize;
    s[..(n - 1).min(s.len())]
        .iter()
        .all(|&c| (0x20..0x7f).contains(&c))
        && (s.len() < n || s[n - 1] == 0)
}

/// 32-byte keys main's code stores from two 16-byte literals (`stp qA, qB, [x0]` after `ldr qA` / `ldr qB`, each
/// from an adrp page plus its offset, within 8 instructions before it): clang may split the key so.
pub fn code_keys(img: &[u8], lo: usize) -> Vec<[u8; 32]> {
    let n = lo.min(img.len()) / 4;
    let w = |i: usize| u32::from_le_bytes(img[4 * i..4 * i + 4].try_into().unwrap());
    let mut out = vec![];
    for i in 0..n {
        let x = w(i);
        if x & 0xFFFF_83E0 != 0xAD00_0000 {
            continue;
        }
        let mut page: std::collections::HashMap<u32, i64> = Default::default();
        let mut half: std::collections::HashMap<u32, i64> = Default::default();
        for j in i.saturating_sub(8)..i {
            let y = w(j);
            if y & 0x9F00_0000 == 0x9000_0000 {
                let imm = (((y >> 29) & 3) | ((y >> 5) & 0x7FFFF) << 2) as i64;
                let imm = if imm & (1 << 20) != 0 {
                    imm - (1 << 21)
                } else {
                    imm
                };
                page.insert(y & 31, ((j as i64 * 4) & !0xFFF) + (imm << 12));
            } else if y & 0xFFC0_0000 == 0x3DC0_0000 && page.contains_key(&((y >> 5) & 31)) {
                half.insert(
                    y & 31,
                    page[&((y >> 5) & 31)] + ((y >> 10) & 0xFFF) as i64 * 16,
                );
            } else {
                page.remove(&(y & 31));
                half.remove(&(y & 31));
            }
        }
        if let (Some(&a), Some(&b)) = (half.get(&(x & 31)), half.get(&((x >> 10) & 31))) {
            if a >= 0 && b >= 0 && (a as usize) + 16 <= img.len() && (b as usize) + 16 <= img.len()
            {
                let mut k = [0u8; 32];
                k[..16].copy_from_slice(&img[a as usize..a as usize + 16]);
                k[16..].copy_from_slice(&img[b as usize..b as usize + 16]);
                out.push(k);
            }
        }
    }
    out
}

/// The AES key (never printed or written) that opens an index whose first block is `first`: one of `known`, one
/// main's code stores from two literals, else one of main's 32-byte windows from `lo` (its read-only segment) on,
/// 4-aligned, with at least 20 distinct bytes.
pub fn find_key(img: &[u8], lo: usize, first: &[u8; 16], known: &[[u8; 32]]) -> Option<[u8; 32]> {
    if let Some(k) = known.iter().find(|k| opens(k, first)) {
        return Some(*k);
    }
    if let Some(k) = code_keys(img, lo).into_iter().find(|k| opens(k, first)) {
        return Some(k);
    }
    let mut a = lo & !3;
    while a + 32 <= img.len() {
        let k: [u8; 32] = img[a..a + 32].try_into().unwrap();
        let mut seen = [false; 256];
        let distinct = k
            .iter()
            .filter(|&&b| !std::mem::replace(&mut seen[b as usize], true))
            .count();
        if distinct >= 20 && opens(&k, first) {
            return Some(k);
        }
        a += 4;
    }
    None
}

/// The files of a pak; `read(offset, length)` reads the pak, `key` decrypts an encrypted index and entries.
pub fn pak_files(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    size: u64,
    key: Option<&[u8; 32]>,
) -> R<Vec<PakEntry>> {
    let PakFooter {
        encrypted: enc,
        index: (ioff, isz),
        methods,
        version: ver,
    } = pak_footer(read, size)?;
    if enc && key.is_none() {
        return Err("the index is AES-encrypted and no key in main opens it".into());
    }
    let idx = read_at(read, ioff, isz, enc, key)?;
    let mut r = Rd {
        b: &idx,
        o: 0,
        what: "the pak index",
    };
    r.fstr()?;
    let count = r.count(1 << 24)?;
    let method_name = |m: u32| -> String {
        if m == 0 {
            String::new()
        } else if ver < 8 {
            LEGACY_METHODS
                .get(m as usize - 1)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("flags {m}"))
        } else {
            methods
                .get(m as usize - 1)
                .cloned()
                .unwrap_or_else(|| format!("method {m}"))
        }
    };
    let mut out = vec![];
    if ver < 11 {
        for _ in 0..count {
            let name = r.fstr()?;
            let (off, sz, raw, meth) = (r.u64()?, r.u64()?, r.u64()?, r.u32()?);
            r.raw(20)?;
            let mut blocks = vec![];
            if meth != 0 {
                let n = r.count(1 << 20)?;
                for _ in 0..n {
                    let (s, e) = (r.u64()?, r.u64()?);
                    let s0 = if ver < 5 { s.wrapping_sub(off) } else { s };
                    blocks.push((s0, e.wrapping_sub(s)));
                }
            }
            let (e, bsz) = (r.u8()?, r.u32()?);
            if blocks.is_empty() {
                blocks.push((53, sz));
            }
            out.push(PakEntry {
                name,
                off,
                raw,
                size: sz,
                method: method_name(meth),
                encrypted: e != 0,
                block_size: bsz as u64,
                blocks,
            });
        }
        return Ok(out);
    }
    r.u64()?;
    if r.i32()? != 0 {
        r.u64()?;
        r.u64()?;
        r.raw(20)?;
    }
    if r.i32()? == 0 {
        return Err("no full directory index".into());
    }
    let (doff, dsz) = (r.u64()?, r.u64()?);
    r.raw(20)?;
    let n = r.count(1 << 30)?;
    let encoded = r.raw(n)?;
    if r.i32()? != 0 {
        return Err("unencoded entries are not read".into());
    }
    if doff.checked_add(dsz).is_none_or(|e| e > size) {
        return Err("the directory index runs past the end of the pak".into());
    }
    let dir = read_at(read, doff, dsz, enc, key)?;
    let mut d = Rd {
        b: &dir,
        o: 0,
        what: "the pak directory index",
    };
    let nd = d.count(1 << 24)?;
    for _ in 0..nd {
        let dn = d.fstr()?;
        let nf = d.count(1 << 24)?;
        for _ in 0..nf {
            let fname = format!("{dn}{}", d.fstr()?)
                .trim_start_matches('/')
                .to_string();
            let at = d.i32()?;
            if at < 0 {
                continue;
            }
            let mut e = Rd {
                b: encoded,
                o: at as usize,
                what: "an encoded pak entry",
            };
            let v = e.u32()?;
            let mut bsz = ((v & 0x3f) as u64) << 11;
            if v & 0x3f == 0x3f {
                bsz = e.u32()? as u64;
            }
            let (nblk, encr, meth) = ((v >> 6) & 0xffff, (v >> 22) & 1, (v >> 23) & 0x3f);
            let off = if (v >> 31) & 1 != 0 {
                e.u32()? as u64
            } else {
                e.u64()?
            };
            let raw = if (v >> 30) & 1 != 0 {
                e.u32()? as u64
            } else {
                e.u64()?
            };
            let sz = if meth != 0 {
                if (v >> 29) & 1 != 0 {
                    e.u32()? as u64
                } else {
                    e.u64()?
                }
            } else {
                raw
            };
            let mut cur = 53 + if meth != 0 { 4 + 16 * nblk as u64 } else { 0 };
            if nblk == 1 && bsz == 0 {
                bsz = raw;
            }
            let mut blocks = vec![];
            if nblk == 1 && encr == 0 || meth == 0 {
                blocks.push((cur, sz));
            } else {
                for _ in 0..nblk {
                    let s = e.u32()? as u64;
                    blocks.push((cur, s));
                    cur += if encr != 0 { (s + 15) & !15 } else { s };
                }
            }
            out.push(PakEntry {
                name: fname,
                off,
                raw,
                size: sz,
                method: method_name(meth),
                encrypted: encr != 0,
                block_size: bsz,
                blocks,
            });
        }
    }
    Ok(out)
}

/// A pak file's bytes, its blocks decoded (stored, Zlib, LZ4, Oodle).
pub fn pak_read(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    e: &PakEntry,
    label: &str,
    key: Option<&[u8; 32]>,
) -> R<Vec<u8>> {
    if e.encrypted && key.is_none() {
        return Err(format!("{label}: encrypted in the pak and no key"));
    }
    if e.method.is_empty() {
        return read_at(read, e.off + 53, e.size, e.encrypted, key);
    }
    if e.raw > archive::DECODE_LIMIT || e.block_size == 0 {
        return Err(format!(
            "{label}: {:#x} raw bytes in blocks of {:#x}",
            e.raw, e.block_size
        ));
    }
    let mut out = Vec::with_capacity(e.raw as usize);
    let mut left = e.raw;
    for (i, &(s, n)) in e.blocks.iter().enumerate() {
        let k = left.min(e.block_size) as usize;
        let b = read_at(read, e.off + s, n, e.encrypted, key)?;
        let wh = format!("{label} block {i}");
        let d = match e.method.as_str() {
            "Zlib" => archive::unzlib(&b, &wh)?,
            "LZ4" => lz4_flex::block::decompress(&b, k).map_err(|x| format!("{wh}: LZ4: {x}"))?,
            "Oodle" => archive::oodle(&b, k, &wh)?,
            m => return Err(format!("{label}: {m}-compressed in the pak (no decoder)")),
        };
        if d.len() != k {
            return Err(format!("{wh}: decodes to {:#x} bytes, not {k:#x}", d.len()));
        }
        out.extend(d);
        left -= k as u64;
    }
    Ok(out)
}

/// The pak files worth reading for shaders: its shader code archives and global shader caches, or, with no shader
/// code archive (shaders inline), its cooked packages too.
pub fn shader_files(files: Vec<PakEntry>) -> Vec<PakEntry> {
    let archives = files.iter().any(|e| e.name.ends_with(".ushaderbytecode"));
    files
        .into_iter()
        .filter(|e| {
            e.name.ends_with(".ushaderbytecode")
                || e.name.contains("GlobalShaderCache")
                || (!archives && e.name.ends_with(".uexp"))
        })
        .collect()
}

fn ue_tag(t: &[u8]) -> Option<Stage> {
    Some(match t {
        b"SV" => Stage::Vs,
        b"SH" => Stage::Hs,
        b"SD" => Stage::Ds,
        b"SG" => Stage::Gs,
        b"SP" => Stage::Ps,
        b"SC" => Stage::Cs,
        _ => return None,
    })
}

/// An LSLG record → (stage tag, control section, code area).
pub fn lslg(s: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    if s.get(..4) != Some(b"LSLG") {
        return None;
    }
    let k = memchr::memmem::find(s, &CTL_MAGIC)?;
    if k < 8 || k + 0x708 > s.len() {
        return None;
    }
    let (nc, nk) = (u32_at(s, k - 4)? as usize, u32_at(s, k + 0x704)? as usize);
    let c = (k - 4).checked_sub(nk)?;
    if c < 4 || u32_at(s, c - 4)? as usize != nk || k + nc > s.len() {
        return None;
    }
    Some((&s[4..6], &s[k..k + nc], &s[c..c + nk]))
}

/// An NVN2 record (Switch 2, no tag): u32 code length + code area, u32 control length + control section.
pub fn nvn2_record(s: &[u8]) -> Option<(&[u8], &[u8])> {
    let k = memchr::memmem::find(s, &[0x34, 0x12, 0x76, 0x98, 0xc9, 0, 0, 0])?;
    if k < 12 || k + NVN2_KEY_CTL > s.len() {
        return None;
    }
    let (nc, nk) = (u32_at(s, k - 4)? as usize, u32_at(s, k + 0x44)? as usize);
    let c = (k - 4).checked_sub(nk)?;
    if c < 4 || u32_at(s, c - 4)? as usize != nk || k + nc > s.len() {
        return None;
    }
    Some((&s[k..k + nc], &s[c..c + nk]))
}

/// A decoded record → (stage, control, code); `freq` the archive's frequency when it has one.
fn record<'a>(s: &'a [u8], freq: Option<u8>, wh: &str) -> R<(Stage, &'a [u8], &'a [u8])> {
    if let Some((tag, ctl, code)) = lslg(s) {
        let st = ue_tag(tag)
            .ok_or_else(|| format!("{wh}: stage tag {}", String::from_utf8_lossy(tag)))?;
        // EShaderFrequency before Unreal Engine 5: 0 vertex, 1 hull, 2 domain, 3 pixel, 4 geometry, 5 compute
        if let Some(f) = freq {
            let want = [
                Stage::Vs,
                Stage::Hs,
                Stage::Ds,
                Stage::Ps,
                Stage::Gs,
                Stage::Cs,
            ]
            .get(f as usize)
            .copied();
            if want != Some(st) {
                return Err(format!(
                    "{wh}: tag {} but frequency {f}",
                    String::from_utf8_lossy(tag)
                ));
            }
        }
        if let Some(why) = check(st, ctl, code) {
            return Err(format!("{wh} ({}): {why}", st.name()));
        }
        return Ok((st, ctl, code));
    }
    let (ctl, code) =
        nvn2_record(s).ok_or_else(|| format!("{wh}: neither an LSLG record nor an NVN2 one"))?;
    let st = nvn2_stage(ctl, code).ok_or_else(|| format!("{wh}: no stage"))?;
    // Unreal Engine 5's numbering: 1 mesh, 2 amplification (task)
    if let Some(f) = freq {
        let want = [
            Stage::Vs,
            Stage::Ms,
            Stage::Ts,
            Stage::Ps,
            Stage::Gs,
            Stage::Cs,
        ]
        .get(f as usize)
        .copied();
        if want != Some(st) {
            return Err(format!("{wh}: a {} shader with frequency {f}", st.name()));
        }
    }
    if let Some(why) = check(st, ctl, code) {
        return Err(format!("{wh} ({}): {why}", st.name()));
    }
    Ok((st, ctl, code))
}

/// A shader code archive (version 2): shader map hashes, shader hashes, shader map entries (16 bytes), shader
/// entries {u64 offset, u32 size, u32 raw size, u8 frequency}, preload entries (16), shader indices (u32), each a
/// u32 count and its elements; then the code. A shader whose size differs from its raw size is an LZ4 block or an
/// Oodle stream.
pub fn shader_archive(d: &[u8], label: &str) -> R<Vec<Found>> {
    let wh = format!("{label}: shader code archive");
    let mut r = Rd {
        b: d,
        o: 0,
        what: &wh,
    };
    if r.u32()? != 2 {
        return Err(format!("{wh}: not version 2"));
    }
    // shader map hashes, shader hashes, shader map entries
    for elem in [20usize, 20, 16] {
        let n = r.u32()? as usize;
        r.raw(
            elem.checked_mul(n)
                .ok_or_else(|| format!("{wh}: count {n}"))?,
        )?;
    }
    let n = r.u32()? as usize;
    let ents = r.raw(
        17usize
            .checked_mul(n)
            .ok_or_else(|| format!("{wh}: count {n}"))?,
    )?;
    let np = r.u32()? as usize;
    r.raw(
        16usize
            .checked_mul(np)
            .ok_or_else(|| format!("{wh}: count {np}"))?,
    )?;
    let ni = r.u32()? as usize;
    r.raw(
        4usize
            .checked_mul(ni)
            .ok_or_else(|| format!("{wh}: count {ni}"))?,
    )?;
    let code = r.o as u64;
    let mut out = vec![];
    for i in 0..n {
        let e = &ents[17 * i..17 * i + 17];
        let (off, size, raw, fr) = (
            u64_at(e, 0).unwrap(),
            u32_at(e, 8).unwrap() as usize,
            u32_at(e, 12).unwrap() as usize,
            e[16],
        );
        let w = format!("{label}#UE{i}");
        let c = span(d, code.saturating_add(off), size as u64, &w)?;
        let s = if size == raw {
            c.to_vec()
        } else if raw <= 255 * size
            && lz4_flex::block::decompress(c, raw)
                .is_ok_and(|x| x.len() == raw && (lslg(&x).is_some() || nvn2_record(&x).is_some()))
        {
            lz4_flex::block::decompress(c, raw).unwrap()
        } else {
            archive::oodle(c, raw, &w)?
        };
        let (st, ctl, cd) = record(&s, Some(fr), &w)?;
        out.push(Found::new(w, st, ctl, cd));
    }
    Ok(out)
}

/// Shaders inline in a cooked package or a global shader cache: LSLG records stored (their length in front),
/// zlib streams (their length in front) or LZ4 blocks (found by the record's first literals) that decode to one, and
/// the shader map tables whose code is in Oodle {count, 20-byte hashes, count, then {i32 length, code, i32 raw size,
/// u8 frequency}}.
pub fn inline(d: &[u8], label: &str) -> (Vec<Found>, Vec<String>) {
    let (mut out, mut problems) = (vec![], vec![]);
    let take = |s: &[u8], at: usize, out: &mut Vec<Found>, problems: &mut Vec<String>| {
        let w = format!("{label}#UE{at:x}");
        match record(s, None, &w) {
            Ok((st, ctl, cd)) => out.push(Found::new(w, st, ctl, cd)),
            Err(e) => problems.push(e),
        }
    };
    let len_before = |i: usize| {
        if i >= 4 {
            u32_at(d, i - 4).unwrap() as usize
        } else {
            0
        }
    };
    // zlib streams
    let mut i = 0;
    while let Some(p) = memchr::memchr(0x78, &d[i..]) {
        let at = i + p;
        i = at + 1;
        let Some(&b1) = d.get(at + 1) else { break };
        if (0x78u16 << 8 | b1 as u16) % 31 != 0 {
            continue;
        }
        let n = len_before(at);
        if n < 2 || n > d.len() - at {
            continue;
        }
        let mut z = flate2::Decompress::new(true);
        let mut head = vec![0u8; 4];
        if z.decompress(&d[at..at + n], &mut head, flate2::FlushDecompress::None)
            .is_err()
            || head != b"LSLG"
        {
            continue;
        }
        if let Ok(s) = archive::unzlib(&d[at..at + n], label) {
            if s.starts_with(b"LSLG") {
                take(&s, at, &mut out, &mut problems);
            }
        }
    }
    for at in memchr::memmem::find_iter(d, b"LSLG") {
        let n = len_before(at);
        if n >= 2 && n <= d.len() - at && lslg(&d[at..at + n]).is_some() {
            take(&d[at..at + n], at, &mut out, &mut problems);
            continue;
        }
        // an LZ4 block whose first literals start here: its token at j (and literal length bytes up to `at`)
        for j in (at.saturating_sub(5).max(4)..at).rev() {
            let (t, ext) = (d[j] >> 4, &d[j + 1..at]);
            if (t < 15 && !ext.is_empty())
                || (t == 15
                    && !(ext.last().is_some_and(|&x| x < 0xff)
                        && ext[..ext.len() - 1].iter().all(|&x| x == 0xff)))
            {
                continue;
            }
            let n = u32_at(d, j - 4).unwrap() as usize;
            if n < 8 || j + n + 5 > d.len() {
                continue;
            }
            let raw = u32_at(d, j + n).unwrap() as usize;
            if raw > 255 * n {
                continue;
            }
            if let Ok(s) = lz4_flex::block::decompress(&d[j..j + n], raw) {
                if s.len() == raw && s.starts_with(b"LSLG") {
                    take(&s, j, &mut out, &mut problems);
                    break;
                }
            }
        }
    }
    // shader map tables with Oodle code
    let mut end = 0;
    for i in memchr::memchr_iter(0x8c, d) {
        if i < end || i < 12 || !matches!(d.get(i + 1), Some(0x06 | 0x0a | 0x0b | 0x0c)) {
            continue;
        }
        let count = u32_at(d, i - 8).unwrap() as usize;
        if !(1..=1 << 20).contains(&count) {
            continue;
        }
        let Some(hashes) = (i - 8).checked_sub(20 * count) else {
            continue;
        };
        if hashes < 4 || u32_at(d, hashes - 4).unwrap() as usize != count {
            continue;
        }
        let mut rows = vec![];
        let mut p = i - 4;
        for _ in 0..count {
            let Some(n) = u32_at(d, p).map(|x| x as usize) else {
                break;
            };
            let q = p + 4 + n;
            if n == 0 || q + 5 > d.len() {
                break;
            }
            let (raw, f) = (u32_at(d, q).unwrap() as usize, d[q + 4]);
            if raw < n || raw > 1 << 28 || f > 15 {
                break;
            }
            rows.push((p + 4, n, raw, f));
            p = q + 5;
        }
        if rows.len() != count {
            continue;
        }
        // the first row proves the table: it decodes, or it is a stored record
        let dec = |&(at, n, raw, _): &(usize, usize, usize, u8)| -> Option<Vec<u8>> {
            if n == raw {
                let s = &d[at..at + n];
                (lslg(s).is_some() || nvn2_record(s).is_some()).then(|| s.to_vec())
            } else {
                archive::oodle(&d[at..at + n], raw, label).ok()
            }
        };
        if dec(&rows[0]).is_none() {
            continue;
        }
        end = p;
        for row in &rows {
            match dec(row) {
                // stored LSLG records were taken by the scan above
                Some(s) if s.starts_with(b"LSLG") && row.1 == row.2 => {}
                Some(s) => take(&s, row.0, &mut out, &mut problems),
                None => problems.push(format!(
                    "{label}#UE{:x}: the Oodle stream does not decode",
                    row.0
                )),
            }
        }
    }
    (out, problems)
}

/// An Unreal package's tag; Unreal Engine 3 has a positive u16 file version after it.
pub const UE3_TAG: [u8; 4] = [0xc1, 0x83, 0x2a, 0x9e];

/// An Unreal Engine 3 global shader cache ('BMSG') or package.
pub fn is_ue3(head: &[u8]) -> bool {
    head.get(..4) == Some(b"BMSG")
        || (head.get(..4) == Some(&UE3_TAG[..])
            && u16_at(head, 4).is_some_and(|v| v > 0 && v < 0x8000))
}

/// An Unreal Engine 3 package with its compressed chunks decoded: the summary up to the first chunk tag, then chunks
/// {tag, u32 block size, u32 stored and raw size, per block u32 stored and raw size} and their blocks (stored when the
/// sizes are equal, zlib, else Oodle). A package without chunks is returned as it is.
pub fn ue3_body(d: &[u8], label: &str) -> R<Vec<u8>> {
    let Some(first) = memchr::memmem::find(&d[4.min(d.len())..], &UE3_TAG).map(|x| x + 4) else {
        return Ok(d.to_vec());
    };
    let mut out = d[..first].to_vec();
    let mut i = first;
    while i < d.len() {
        if d.get(i..i + 4) != Some(&UE3_TAG[..]) || i + 16 > d.len() {
            return Err(format!("{label}: no compressed chunk at {i:#x} (the chunks do not run to the end of the package)"));
        }
        let (bs, us) = (
            u32_at(d, i + 4).unwrap() as usize,
            u32_at(d, i + 12).unwrap() as usize,
        );
        if bs == 0 {
            return Err(format!("{label}: chunk at {i:#x} has block size 0"));
        }
        let nb = us.div_ceil(bs);
        let mut o = i + 16 + 8 * nb;
        for k in 0..nb {
            let w = format!("{label} chunk at {i:#x} block {k}");
            let (c, u) = (
                need32(d, i + 16 + 8 * k, &w)? as usize,
                need32(d, i + 20 + 8 * k, &w)? as usize,
            );
            let blk = span(d, o as u64, c as u64, &w)?;
            if out.len() + u > archive::DECODE_LIMIT as usize {
                return Err(format!("{w}: the package decodes past the limit"));
            }
            out.extend(if c == u {
                blk.to_vec()
            } else if blk[0] == 0x78 {
                archive::unzlib(blk, &w)?
            } else {
                archive::oodle(blk, u, &w)?
            });
            o += c;
        }
        i = o;
    }
    Ok(out)
}

/// Unreal Engine 3's shader records (Batman: Arkham Knight's global cache and combined shader cache): u32 record
/// length, u32 code length, u32 control length (and on the global cache one more u32), the code area right followed
/// by its control section. Found by the control section, its +0x704 length back to the code area's magic, and the
/// two lengths in front.
pub fn ue3_shaders(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for c in memchr::memmem::find_iter(d, &CTL_MAGIC) {
        if c + 0x708 > d.len() {
            continue;
        }
        let n = u32_at(d, c + 0x704).unwrap() as usize;
        let Some(a) = c.checked_sub(n).filter(|&a| a >= 12) else {
            continue;
        };
        let head = &d[a..a + 4];
        if head != GFX_MAGIC && head != CS_MAGIC {
            continue;
        }
        let nctl = [4usize, 8]
            .iter()
            .filter_map(|&k| a.checked_sub(k + 4))
            .find_map(|p| {
                let (ln, lc) = (u32_at(d, p)? as usize, u32_at(d, p + 4)? as usize);
                (ln == n && (0x760..=0x1000).contains(&lc)).then_some(lc)
            });
        let Some(nctl) = nctl.filter(|&x| c + x <= d.len()) else {
            continue;
        };
        let (ctl, code) = (&d[c..c + nctl], &d[a..c]);
        let st = if head == CS_MAGIC {
            Some(Stage::Cs)
        } else {
            sph_stage(code)
        };
        let st = st.ok_or_else(|| {
            format!("{label}: shader record at {c:#x}: no stage in the code area header")
        })?;
        if let Some(why) = check(st, ctl, code) {
            return Err(format!(
                "{label}: shader record at {c:#x} ({}): {why}",
                st.name()
            ));
        }
        out.push(Found::new(format!("{label}#UE3@{c:x}"), st, ctl, code));
    }
    Ok(out)
}

pub const TOC_MAGIC: &[u8; 16] = b"-==--==--==--==-";

/// An IoStore container's table (.utoc): chunk IDs, each chunk's range of the container's uncompressed stream, the
/// compression blocks (5-byte offset, 3-byte stored size, 3-byte raw size, method index; the data in the .ucas, or
/// `<name>_s<n>.ucas` for later partitions) and the method names. Header: magic, u8 version, ..., +0x14 header size,
/// entry count, block count, block entry size (12), method count, method name length, block size, directory index
/// size, partition count; +0x50 flags (2 encrypted); +0x54 perfect hash seed count (version 4 on); +0x58 partition
/// size (version 3 on); +0x60 chunks without a perfect hash (version 5 on). After it: the chunk IDs (12 bytes, type at
/// +11), offsets and lengths (5 + 5 bytes, big-endian), the seeds and that list (u32 each), the blocks, the names.
pub struct Toc {
    pub version: u8,
    pub encrypted: bool,
    pub block_size: u64,
    pub part_size: u64,
    pub ids: Vec<[u8; 12]>,
    pub ranges: Vec<(u64, u64)>,
    pub blocks: Vec<(u64, u32, u32, u8)>,
    pub methods: Vec<String>,
}

pub const IO_EXPORT_BUNDLE: u8 = 1;
pub const IO_EXPORT_BUNDLE_427: u8 = 2;
pub const IO_SHADER_LIBRARY: u8 = 8;

impl Toc {
    pub fn parse(b: &[u8], label: &str) -> R<Toc> {
        if b.get(..16) != Some(&TOC_MAGIC[..]) {
            return Err(format!("{label}: not an IoStore TOC"));
        }
        let wh = format!("{label}: TOC header");
        let w = |o: usize| need32(b, o, &wh).map(|x| x as usize);
        let version = b[16];
        let (hsz, n, nblk, bsz, nmeth, mlen, block_size, _dsz, nparts) = (
            w(20)?,
            w(24)?,
            w(28)?,
            w(32)?,
            w(36)?,
            w(40)?,
            w(44)? as u64,
            w(48)?,
            w(52)?,
        );
        if bsz != 12 || block_size == 0 {
            return Err(format!(
                "{label}: compression block entries of {bsz} bytes, blocks of {block_size}"
            ));
        }
        let flags = *b.get(80).ok_or_else(|| format!("{wh}: cut short"))?;
        let nseeds = if version >= 4 { w(84)? } else { 0 };
        let part_size = if version >= 3 && nparts > 1 {
            need64(b, 88, &wh)?
        } else {
            u64::MAX
        };
        let nover = if version >= 5 { w(96)? } else { 0 };
        let total = hsz + 22 * n + 4 * (nseeds + nover) + 12 * nblk + mlen * nmeth;
        if total > b.len() || n > b.len() || nblk > b.len() {
            return Err(format!("{label}: the TOC's tables run past its end"));
        }
        let mut o = hsz;
        let ids = (0..n)
            .map(|i| b[o + 12 * i..o + 12 * i + 12].try_into().unwrap())
            .collect();
        o += 12 * n;
        let be5 = |s: &[u8]| s.iter().fold(0u64, |a, &x| a << 8 | x as u64);
        let le = |s: &[u8]| s.iter().rev().fold(0u64, |a, &x| a << 8 | x as u64);
        let ranges = (0..n)
            .map(|i| {
                (
                    be5(&b[o + 10 * i..o + 10 * i + 5]),
                    be5(&b[o + 10 * i + 5..o + 10 * i + 10]),
                )
            })
            .collect();
        o += 10 * n + 4 * (nseeds + nover);
        let blocks = (0..nblk)
            .map(|i| {
                let e = &b[o + 12 * i..o + 12 * i + 12];
                (
                    le(&e[..5]),
                    le(&e[5..8]) as u32,
                    le(&e[8..11]) as u32,
                    e[11],
                )
            })
            .collect();
        o += 12 * nblk;
        let methods = (0..nmeth)
            .map(|i| {
                String::from_utf8_lossy(&b[o + mlen * i..o + mlen * (i + 1)])
                    .trim_end_matches('\0')
                    .to_string()
            })
            .collect();
        Ok(Toc {
            version,
            encrypted: flags & 2 != 0,
            block_size,
            part_size,
            ids,
            ranges,
            blocks,
            methods,
        })
    }

    /// The chunks of the cooked packages (their export bundles): type 1 from version 4 (Unreal Engine 5), 2 before.
    pub fn export_bundles(&self) -> Vec<usize> {
        let t = if self.version >= 4 {
            IO_EXPORT_BUNDLE
        } else {
            IO_EXPORT_BUNDLE_427
        };
        (0..self.ids.len())
            .filter(|&i| self.ids[i][11] == t)
            .collect()
    }

    /// The shader library chunks (Unreal Engine 5).
    pub fn shader_libraries(&self) -> Vec<usize> {
        if self.version < 4 {
            return vec![];
        }
        (0..self.ids.len())
            .filter(|&i| self.ids[i][11] == IO_SHADER_LIBRARY)
            .collect()
    }

    /// Chunk `i`'s bytes; `read(partition, offset, length)` reads the .ucas files.
    pub fn read(
        &self,
        i: usize,
        read: &dyn Fn(u64, u64, u64) -> R<Vec<u8>>,
        key: Option<&[u8; 32]>,
        label: &str,
    ) -> R<Vec<u8>> {
        if self.encrypted && key.is_none() {
            return Err(format!(
                "{label}: the container is encrypted and no key opens it"
            ));
        }
        let (off, ln) = self.ranges[i];
        if ln > archive::DECODE_LIMIT {
            return Err(format!("{label}: chunk of {ln:#x} bytes"));
        }
        let first = off / self.block_size;
        let last = if ln > 0 {
            (off + ln - 1) / self.block_size + 1
        } else {
            first
        };
        let mut out = vec![];
        for k in first..last {
            let &(at, size, raw, m) = self
                .blocks
                .get(k as usize)
                .ok_or_else(|| format!("{label}: block {k} is not in the TOC"))?;
            let (part, po) = (at / self.part_size, at % self.part_size);
            let mut d = read(
                part,
                po,
                if self.encrypted {
                    (size as u64 + 15) & !15
                } else {
                    size as u64
                },
            )?;
            if self.encrypted {
                aes_decrypt(key.unwrap(), &mut d);
                d.truncate(size as usize);
            }
            let wh = format!("{label} block {k}");
            let meth = if m == 0 {
                ""
            } else {
                self.methods
                    .get(m as usize - 1)
                    .map(|s| s.as_str())
                    .unwrap_or("?")
            };
            out.extend(match meth {
                "" => d[..(raw as usize).min(d.len())].to_vec(),
                "Oodle" => archive::oodle(&d, raw as usize, &wh)?,
                "Zlib" => archive::unzlib(&d, &wh)?,
                "LZ4" => lz4_flex::block::decompress(&d, raw as usize)
                    .map_err(|e| format!("{wh}: LZ4: {e}"))?,
                m => return Err(format!("{wh}: compression method {m} is not read")),
            });
        }
        let s = (off - first * self.block_size) as usize;
        out.get(s..s + ln as usize)
            .map(|x| x.to_vec())
            .ok_or_else(|| format!("{label}: chunk {i} is shorter than its range"))
    }
}

/// An IoStore shader library (FIoStoreShaderCodeArchive, version 1): shader map hashes (20), shader hashes (20), the
/// groups' chunk IDs (12), shader map entries {first, count}, shader entries (u64: frequency bits 0-3, group 4-33,
/// offset in the group's decoded bytes from 34), groups {first index, count, raw size, stored size}, shader indices;
/// each a u32 count and its elements. A group is one chunk holding its shaders end to end, Oodle-compressed as a
/// whole when stored < raw. `chunk(id)` reads a group's chunk.
pub fn io_library(
    lib: &[u8],
    chunk: &dyn Fn(&[u8; 12]) -> R<Vec<u8>>,
    label: &str,
) -> R<Vec<Found>> {
    let mut r = Rd {
        b: lib,
        o: 0,
        what: label,
    };
    if r.u32()? != 1 {
        return Err(format!("{label}: shader library is not version 1"));
    }
    let nm = r.u32()? as usize;
    r.raw(20usize.checked_mul(nm).ok_or("count")?)?;
    let nsh = r.u32()? as usize;
    r.raw(20usize.checked_mul(nsh).ok_or("count")?)?;
    let ng = r.u32()? as usize;
    let gids: Vec<[u8; 12]> = (0..ng)
        .map(|_| r.raw(12).map(|s| s.try_into().unwrap()))
        .collect::<R<_>>()?;
    let nmap = r.u32()? as usize;
    r.raw(8usize.checked_mul(nmap).ok_or("count")?)?;
    let n = r.u32()? as usize;
    let ents: Vec<u64> = (0..n).map(|_| r.u64()).collect::<R<_>>()?;
    let ngr = r.u32()? as usize;
    let groups: Vec<[u32; 4]> = (0..ngr)
        .map(|_| Ok([r.u32()?, r.u32()?, r.u32()?, r.u32()?]))
        .collect::<R<_>>()?;
    let ni = r.u32()? as usize;
    let idx: Vec<u32> = (0..ni).map(|_| r.u32()).collect::<R<_>>()?;
    if n != nsh || gids.len() != groups.len() {
        return Err(format!("{label}: shader library does not parse"));
    }
    let mut out = vec![];
    for (g, (gid, &[first, cnt, raw, stored])) in gids.iter().zip(&groups).enumerate() {
        let wh = format!("{label}: shader group {g}");
        let mut d = chunk(gid).map_err(|e| format!("{wh}: {e}"))?;
        if d.len() != stored as usize {
            return Err(format!(
                "{wh} is {} bytes, its entry says {stored}",
                d.len()
            ));
        }
        if stored < raw {
            d = archive::oodle(&d, raw as usize, &wh)?;
        }
        let sel = idx
            .get(first as usize..(first + cnt) as usize)
            .ok_or_else(|| format!("{wh}: indices out of range"))?;
        let mut mine: Vec<(u64, usize)> = sel
            .iter()
            .filter_map(|&s| ents.get(s as usize).map(|e| (e >> 34, s as usize)))
            .collect();
        mine.sort();
        for (j, &(at, s)) in mine.iter().enumerate() {
            let nxt = mine.get(j + 1).map(|x| x.0).unwrap_or(raw as u64);
            let c = d
                .get(at as usize..nxt as usize)
                .ok_or_else(|| format!("{wh}: shader {s} out of range"))?;
            let w = format!("{label}#UE{s}");
            let (st, ctl, cd) = record(c, Some((ents[s] & 0xf) as u8), &w)?;
            out.push(Found::new(w, st, ctl, cd));
        }
    }
    Ok(out)
}

/// A file of a pak (or a loose one) by its name: a shader code archive, or shaders inline.
pub fn pak_member(d: &[u8], name: &str, label: &str, out: &mut super::Scanned) {
    if name.ends_with(".ushaderbytecode") {
        match shader_archive(d, label) {
            Ok(f) => out.found.extend(f),
            Err(e) => out.problems.push(e),
        }
    } else {
        let (f, p) = inline(d, label);
        out.found.extend(f);
        out.problems.extend(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    fn lslg_rec(tag: &[u8; 2], c: &[u8], k: &[u8]) -> Vec<u8> {
        let mut v = b"LSLG".to_vec();
        v.extend_from_slice(tag);
        v.extend_from_slice(b"\0\0name.usf\0\0");
        v.extend_from_slice(&(k.len() as u32).to_le_bytes());
        v.extend_from_slice(k);
        v.extend_from_slice(&(c.len() as u32).to_le_bytes());
        v.extend_from_slice(c);
        v.extend_from_slice(&[0; 24]);
        v
    }

    fn fstr(v: &mut Vec<u8>, s: &str) {
        v.extend_from_slice(&(s.len() as i32 + 1).to_le_bytes());
        v.extend_from_slice(s.as_bytes());
        v.push(0);
    }

    #[test]
    fn archive_in_legacy_pak_and_inline() {
        let (vs, ps) = (nvn1_vs(1), nvn1_ps(2));
        let (rv, rp) = (lslg_rec(b"SV", &vs.0, &vs.1), lslg_rec(b"SP", &ps.0, &ps.1));
        let rpc = lz4_flex::block::compress(&rp);
        // a version 2 archive: no maps, two shaders (stored vertex, LZ4 pixel)
        let mut a = vec![];
        for x in [2u32, 0, 0, 0, 2] {
            a.extend_from_slice(&x.to_le_bytes());
        }
        for (off, size, raw, f) in [
            (0u64, rv.len(), rv.len(), 0u8),
            (rv.len() as u64, rpc.len(), rp.len(), 3),
        ] {
            a.extend_from_slice(&off.to_le_bytes());
            a.extend_from_slice(&(size as u32).to_le_bytes());
            a.extend_from_slice(&(raw as u32).to_le_bytes());
            a.push(f);
        }
        a.extend_from_slice(&[0; 8]);
        a.extend_from_slice(&rv);
        a.extend_from_slice(&rpc);
        let f = shader_archive(&a, "/x.ushaderbytecode").unwrap();
        assert_eq!(
            f.iter().map(|f| (f.stage, f.key())).collect::<Vec<_>>(),
            vec![
                (Stage::Vs, skey(&vs.0, &vs.1)),
                (Stage::Ps, skey(&ps.0, &ps.1))
            ]
        );
        // the archive in a version 4 pak, stored
        let mut pak = vec![0u8; 53];
        pak.extend_from_slice(&a);
        let ioff = pak.len() as u64;
        let mut idx = vec![];
        fstr(&mut idx, "../../../");
        idx.extend_from_slice(&1i32.to_le_bytes());
        fstr(
            &mut idx,
            "Game/Content/ShaderArchive-Global-SF_GLSL_SWITCH.ushaderbytecode",
        );
        for x in [0u64, a.len() as u64, a.len() as u64] {
            idx.extend_from_slice(&x.to_le_bytes());
        }
        idx.extend_from_slice(&0u32.to_le_bytes());
        idx.extend_from_slice(&[0; 20]);
        idx.push(0);
        idx.extend_from_slice(&0u32.to_le_bytes());
        pak.extend_from_slice(&idx);
        pak.push(0);
        pak.extend_from_slice(&PAK_MAGIC.to_le_bytes());
        pak.extend_from_slice(&4u32.to_le_bytes());
        pak.extend_from_slice(&ioff.to_le_bytes());
        pak.extend_from_slice(&(idx.len() as u64).to_le_bytes());
        pak.extend_from_slice(&[0; 20]);
        let read = |o: u64, n: u64| Ok(pak[o as usize..(o + n) as usize].to_vec());
        let files = shader_files(pak_files(&read, pak.len() as u64, None).unwrap());
        assert_eq!(files.len(), 1);
        let d = pak_read(&read, &files[0], "/p.pak", None).unwrap();
        assert_eq!(d, a);
        // inline: a stored record and a zlib one, each with its length in front
        let mut u = vec![0u8; 16];
        u.extend_from_slice(&(rv.len() as u32).to_le_bytes());
        u.extend_from_slice(&rv);
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, &rp).unwrap();
        let zc = z.finish().unwrap();
        u.extend_from_slice(&(zc.len() as u32).to_le_bytes());
        u.extend_from_slice(&zc);
        let (f, p) = inline(&u, "/m.uexp");
        assert!(p.is_empty(), "{p:?}");
        let mut st: Vec<Stage> = f.iter().map(|f| f.stage).collect();
        st.sort_by_key(|s| s.name());
        assert_eq!(st, vec![Stage::Ps, Stage::Vs]);
    }

    #[test]
    fn key_from_main_window() {
        use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
        let key: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11));
        let mut first = [0u8; 16];
        first[..4].copy_from_slice(&10i32.to_le_bytes());
        first[4..13].copy_from_slice(b"../../../");
        aes::Aes256::new(GenericArray::from_slice(&key))
            .encrypt_block(GenericArray::from_mut_slice(&mut first));
        let mut img = vec![0u8; 0x200];
        img[0x140..0x160].copy_from_slice(&key);
        assert_eq!(find_key(&img, 0x100, &first, &[]), Some(key));
        assert_eq!(find_key(&img[..0x150], 0x100, &first, &[]), None);
    }

    #[test]
    fn iostore_library() {
        let (vs, ps) = (nvn1_vs(5), nvn1_ps(6));
        let (rv, rp) = (lslg_rec(b"SV", &vs.0, &vs.1), lslg_rec(b"SP", &ps.0, &ps.1));
        let mut group = rv.clone();
        group.extend_from_slice(&rp);
        let gid: [u8; 12] = [7, 7, 7, 7, 7, 7, 7, 7, 0, 0, 0, 9];
        let mut lib = vec![];
        let u = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        u(&mut lib, 1);
        u(&mut lib, 0);
        u(&mut lib, 2);
        lib.extend_from_slice(&[0; 40]);
        u(&mut lib, 1);
        lib.extend_from_slice(&gid);
        u(&mut lib, 0);
        u(&mut lib, 2);
        lib.extend_from_slice(&((0u64 << 34) | 0).to_le_bytes());
        lib.extend_from_slice(&(((rv.len() as u64) << 34) | 3).to_le_bytes());
        u(&mut lib, 1);
        for x in [0, 2, group.len() as u32, group.len() as u32] {
            u(&mut lib, x);
        }
        u(&mut lib, 2);
        u(&mut lib, 0);
        u(&mut lib, 1);
        // the container: chunk 0 the library (type 8), chunk 1 the group; one stored block each
        let ucas: Vec<u8> = [lib.clone(), group.clone()].concat();
        let bs = 0x10000u32;
        let mut t = TOC_MAGIC.to_vec();
        t.push(4);
        t.resize(20, 0);
        for x in [0x90u32, 2, 2, 12, 0, 0, bs, 0, 1] {
            u(&mut t, x);
        }
        t.resize(0x90, 0);
        let mut lib_id = [1u8; 12];
        lib_id[11] = IO_SHADER_LIBRARY;
        t.extend_from_slice(&lib_id);
        t.extend_from_slice(&gid);
        let be5 = |v: &mut Vec<u8>, x: u64| v.extend_from_slice(&x.to_be_bytes()[3..]);
        be5(&mut t, 0);
        be5(&mut t, lib.len() as u64);
        be5(&mut t, bs as u64);
        be5(&mut t, group.len() as u64);
        for (at, n) in [
            (0u64, lib.len() as u64),
            (lib.len() as u64, group.len() as u64),
        ] {
            t.extend_from_slice(&at.to_le_bytes()[..5]);
            t.extend_from_slice(&n.to_le_bytes()[..3]);
            t.extend_from_slice(&n.to_le_bytes()[..3]);
            t.push(0);
        }
        let toc = Toc::parse(&t, "/x.utoc").unwrap();
        assert_eq!(toc.shader_libraries(), vec![0]);
        let read = |_p: u64, o: u64, n: u64| Ok(ucas[o as usize..(o + n) as usize].to_vec());
        let l = toc.read(0, &read, None, "/x.utoc").unwrap();
        assert_eq!(l, lib);
        let chunk = |id: &[u8; 12]| {
            assert_eq!(id, &gid);
            toc.read(1, &read, None, "/x.utoc")
        };
        let f = io_library(&l, &chunk, "/x.utoc#lib").unwrap();
        assert_eq!(
            f.iter().map(|f| (f.stage, f.key())).collect::<Vec<_>>(),
            vec![
                (Stage::Vs, skey(&vs.0, &vs.1)),
                (Stage::Ps, skey(&ps.0, &ps.1))
            ]
        );
    }

    #[test]
    fn ue3_package() {
        let (ps, cs) = (nvn1_ps(13), nvn1_ps(14));
        let mut rec = vec![];
        for (c, k) in [&ps, &cs] {
            rec.extend_from_slice(&((8 + k.len() + c.len()) as u32).to_le_bytes());
            rec.extend_from_slice(&(k.len() as u32).to_le_bytes());
            rec.extend_from_slice(&(c.len() as u32).to_le_bytes());
            rec.extend_from_slice(k);
            rec.extend_from_slice(c);
        }
        // a package: summary, then one chunk of two blocks (zlib, stored)
        let half = rec.len() / 2;
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, &rec[..half]).unwrap();
        let z = z.finish().unwrap();
        let mut d = UE3_TAG.to_vec();
        d.extend_from_slice(&0x360u16.to_le_bytes());
        d.resize(0x40, 0);
        d.extend_from_slice(&UE3_TAG);
        for x in [
            half as u32,
            (z.len() + rec.len() - half) as u32,
            rec.len() as u32,
            z.len() as u32,
            half as u32,
            (rec.len() - half) as u32,
            (rec.len() - half) as u32,
        ] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        d.extend_from_slice(&z);
        d.extend_from_slice(&rec[half..]);
        let mut sc = crate::formats::Scanned::default();
        crate::formats::scan_file(
            &Default::default(),
            &d,
            "/CookedSwitch/CombinedShaderCache.xxx",
            &mut sc,
        );
        assert!(sc.problems.is_empty(), "{:?}", sc.problems);
        assert_eq!(
            sc.found.iter().map(|f| f.key()).collect::<Vec<_>>(),
            vec![skey(&ps.0, &ps.1), skey(&cs.0, &cs.1)]
        );
    }
}
