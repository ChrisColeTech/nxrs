//! Archives read through their own index, member by member, without reading the whole file: PlatinumGames' PKZ,
//! Rockstar's RPF6 (AES tables, the key from main), Camelot's CRA (index from the game's NRO) and NARC containers,
//! Blizzard's CASC and Diablo III packages, Kirby Air Riders' PDAT, zip archives (and Star Fox's chunked LZ4
//! members), Larian's LSPK, Unravel TWO's KIT, Virtuos' SRYK (VBF), UbiArt's IPK, Double Eleven's resource packs and
//! Frostbite's cas files.

use crate::formats::{archive, unreal};
use crate::nvn::*;

/// A reader of one file's bytes: (offset, length) → bytes.
pub type Read<'a> = &'a dyn Fn(u64, u64) -> R<Vec<u8>>;

fn inflate(s: &[u8], label: &str) -> R<Vec<u8>> {
    archive::unzlib(s, label)
}

fn inflate_raw(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![];
    std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(s), &mut out).ok()?;
    Some(out)
}

fn unzstd(s: &[u8], label: &str) -> R<Vec<u8>> {
    let mut v = vec![];
    let mut r = zstd::stream::read::Decoder::new(s).map_err(|e| format!("{label}: {e}"))?;
    std::io::Read::read_to_end(&mut r, &mut v).map_err(|e| format!("{label}: zstd: {e}"))?;
    Ok(v)
}

fn lz4_chained(d: &[u8], size: usize, chunk: usize, label: &str) -> R<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let mut pos = 0;
    while pos + 4 <= d.len() && out.len() < size {
        let stored = u32_at(d, pos).unwrap() as usize;
        pos += 4;
        let want = chunk.min(size - out.len());
        if stored == 0 || stored > d.len() - pos {
            return Err(format!("{label}: LZ4 block at {:#x} runs past the member", pos - 4));
        }
        if stored == want {
            out.extend_from_slice(&d[pos..pos + stored]);
        } else {
            let dict = &out[out.len().saturating_sub(chunk)..];
            let b = lz4_flex::block::decompress_with_dict(&d[pos..pos + stored], want, dict).map_err(|e| format!("{label}: LZ4 block at {:#x}: {e}", pos - 4))?;
            if b.len() != want {
                return Err(format!("{label}: LZ4 block makes {} of {want} bytes", b.len()));
            }
            out.extend(b);
        }
        pos += stored;
    }
    if pos != d.len() || out.len() != size {
        return Err(format!("{label}: the LZ4 blocks make {} of {size} bytes from {pos} of {}", out.len(), d.len()));
    }
    Ok(out)
}

// ---- PKZ (PlatinumGames) ----

pub const PKZL: &[u8; 4] = b"pkzl";

/// (name, compression, offset, stored length, length)
pub type PkzEntry = (String, String, u64, u64, u64);

pub fn pkz_entries(read: Read, label: &str) -> R<Vec<PkzEntry>> {
    let h = read(0, 0x20)?;
    if h.get(..4) != Some(&PKZL[..]) {
        return Err(format!("{label}: not a PKZ"));
    }
    let ver = u32_at(&h, 4).unwrap();
    if ver != 0x10000 && ver != 0x20000 {
        return Err(format!("{label}: PKZ version {ver:#x}: only 0x10000 and 0x20000 are known"));
    }
    let (n, eo, nl) = (u32_at(&h, 16).unwrap() as u64, u32_at(&h, 20).unwrap() as u64, u32_at(&h, 24).unwrap() as u64);
    let t = read(eo, 0x20 * n + nl)?;
    let nm = |o: u32| -> String {
        let a = 0x20 * n as usize + o as usize;
        let e = t.get(a..).and_then(|s| memchr::memchr(0, s)).map(|e| a + e).unwrap_or(t.len());
        String::from_utf8_lossy(t.get(a..e).unwrap_or(&[])).into_owned()
    };
    Ok((0..n as usize)
        .map(|i| {
            let o = 0x20 * i;
            (nm(u32_at(&t, o).unwrap_or(0)), nm(u32_at(&t, o + 4).unwrap_or(0)), u64_at(&t, o + 16).unwrap_or(0), u64_at(&t, o + 24).unwrap_or(0), u64_at(&t, o + 8).unwrap_or(0))
        })
        .collect())
}

pub fn pkz_read(read: Read, e: &PkzEntry, label: &str) -> R<Vec<u8>> {
    let c = read(e.2, e.3)?;
    let d = match e.1.as_str() {
        "None" => c,
        "OodleKraken" => archive::oodle(&c, e.4 as usize, label)?,
        "ZStandard" => unzstd(&c, label)?,
        x => return Err(format!("{label}: compression {x:?} is not known")),
    };
    if d.len() as u64 != e.4 {
        return Err(format!("{label}: {} bytes decoded, the entry says {}", d.len(), e.4));
    }
    Ok(d)
}

// ---- RPF6 (Rockstar, Double Eleven's Red Dead Redemption) ----

pub const RPF6: &[u8; 4] = b"RPF6";
const RPF_ENCRYPTED: u32 = 0xffff_fffd;

fn joaat(s: &str) -> u32 {
    let mut h: u32 = 0;
    for c in s.to_lowercase().bytes() {
        h = h.wrapping_add(c as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

fn rpf_decrypt(key: &[u8; 32], data: &[u8]) -> Vec<u8> {
    let mut d = data[..data.len() / 16 * 16].to_vec();
    for _ in 0..16 {
        unreal::aes_decrypt(key, &mut d);
    }
    d
}

fn rpf_is_root(d: &[u8]) -> bool {
    let be = |o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    d.len() >= 16 && be(0) == Some(0) && be(4) == Some(0) && be(8) == Some(0x8000_0001) && be(12).is_some_and(|c| c > 0)
}

/// (entry count, name table offset, encrypted), or None when `h` (the first 16 bytes) is no RPF6.
pub fn rpf_header(h: &[u8]) -> Option<(u32, u64, bool)> {
    if h.len() < 16 || &h[..4] != RPF6 {
        return None;
    }
    let be = |o: usize| u32::from_be_bytes(h[o..o + 4].try_into().unwrap());
    Some((be(4), be(8) as u64 * 8, be(12) == RPF_ENCRYPTED))
}

/// The key setup's first instructions: cmp w1, #0; mov w8, #-3; csel w8, w8, w1, eq; cmn w8, #3; str w8, [x0]
const RPF_SETUP: [u32; 5] = [0x7100_003f, 0x1280_0048, 0x1a81_0108, 0x3100_0d1f, 0xb900_0008];

/// Candidate keys the key setup in main's code builds: after its b.ne it stores 64 bytes of 16-byte constants (adrp,
/// ldr q) at x0+8; those are eight big-endian u32 key words zero-extended to 64 bits.
fn rpf_setup_keys(img: &[u8], text: (usize, usize)) -> Vec<[u8; 32]> {
    let setup: Vec<u8> = RPF_SETUP.iter().flat_map(|w| w.to_le_bytes()).collect();
    let (ts, tl) = text;
    let end = (ts + tl).min(img.len());
    let mut out = vec![];
    let mut from = ts;
    while let Some(at) = img.get(from..end).and_then(|s| memchr::memmem::find(s, &setup)).map(|p| p + from) {
        from = at + 4;
        let mut page: std::collections::HashMap<u32, i64> = Default::default();
        let mut ctx: std::collections::HashMap<u32, i64> = [(0u32, 0i64)].into_iter().collect();
        let mut q: std::collections::HashMap<u32, Vec<u8>> = Default::default();
        let mut mem: std::collections::HashMap<i64, Vec<u8>> = Default::default();
        let mut a = at + setup.len() + 4;
        while a < (at + 0x80).min(end) {
            let w = u32_at(img, a).unwrap_or(0);
            let (rd, rn) = (w & 31, (w >> 5) & 31);
            if w & 0x9f00_0000 == 0x9000_0000 {
                let imm = ((w >> 29) & 3) as i64 | ((((w >> 5) & 0x7ffff) as i64) << 2);
                let imm = if imm & (1 << 20) != 0 { imm - (1 << 21) } else { imm };
                page.insert(rd, ((a as i64) & !0xfff) + (imm << 12));
                ctx.remove(&rd);
            } else if w & 0xffe0_ffe0 == 0xaa00_03e0 && ctx.contains_key(&((w >> 16) & 31)) {
                let v = ctx[&((w >> 16) & 31)];
                ctx.insert(rd, v);
            } else if w & 0xffc0_0000 == 0x9100_0000 && ctx.contains_key(&rn) {
                let v = ctx[&rn] + ((w >> 10) & 0xfff) as i64;
                ctx.insert(rd, v);
            } else if w & 0xffc0_0000 == 0x3dc0_0000 && page.contains_key(&rn) {
                let o = (page[&rn] + ((w >> 10) & 0xfff) as i64 * 16) as usize;
                q.insert(rd, img.get(o..o + 16).unwrap_or(&[]).to_vec());
            } else if w & 0xffe0_0c00 == 0x3c80_0c00 && ctx.contains_key(&rn) && q.contains_key(&rd) {
                let imm = ((w >> 12) & 0x1ff) as i64;
                let v = ctx[&rn] + if imm & 0x100 != 0 { imm - 0x200 } else { imm };
                ctx.insert(rn, v);
                mem.insert(v, q[&rd].clone());
            } else if w & 0xffc0_0000 == 0x3d80_0000 && ctx.contains_key(&rn) && q.contains_key(&rd) {
                mem.insert(ctx[&rn] + ((w >> 10) & 0xfff) as i64 * 16, q[&rd].clone());
            } else if w & 0xffc0_0000 == 0xad00_0000 && ctx.contains_key(&rn) && q.contains_key(&rd) && q.contains_key(&((w >> 10) & 31)) {
                let imm = ((w >> 15) & 0x7f) as i64;
                let o = ctx[&rn] + if imm & 0x40 != 0 { imm - 0x80 } else { imm } * 16;
                mem.insert(o, q[&rd].clone());
                mem.insert(o + 16, q[&((w >> 10) & 31)].clone());
            } else if matches!(w & 0xfc00_0000, 0x1400_0000 | 0x9400_0000) || w & 0xff00_0010 == 0x5400_0000 {
                break;
            }
            a += 4;
        }
        if (0..4).all(|k| mem.get(&(8 + 16 * k)).is_some_and(|v| v.len() == 16)) {
            let words: Vec<u64> = (0..4).flat_map(|k| {
                let v = &mem[&(8 + 16 * k)];
                [u64::from_le_bytes(v[..8].try_into().unwrap()), u64::from_le_bytes(v[8..].try_into().unwrap())]
            }).collect();
            if words.iter().all(|v| v >> 32 == 0) {
                let mut k = [0u8; 32];
                for (i, v) in words.iter().enumerate() {
                    k[4 * i..4 * i + 4].copy_from_slice(&(*v as u32).to_be_bytes());
                }
                out.push(k);
            }
        }
    }
    out
}

/// The AES key that decrypts an archive's first entry (`first`, the 16 bytes at +0x10) to the root directory: the key
/// setup's candidates, then every 4-aligned 32-byte window (at least 24 distinct bytes) of main's read-only and data
/// segments. `segs`: main's (memory offset, size) of text, read-only data, data.
pub fn rpf_find_key(img: &[u8], segs: &[(usize, usize)], first: &[u8]) -> Option<[u8; 32]> {
    for k in rpf_setup_keys(img, segs[0]) {
        if rpf_is_root(&rpf_decrypt(&k, first)) {
            return Some(k);
        }
    }
    for &(mo, sz) in &segs[1..] {
        let end = (mo + sz).min(img.len());
        let mut a = mo;
        while a + 32 < end {
            let w: [u8; 32] = img[a..a + 32].try_into().unwrap();
            let mut seen = [false; 256];
            let distinct = w.iter().filter(|&&b| !std::mem::replace(&mut seen[b as usize], true)).count();
            if distinct >= 24 && rpf_is_root(&rpf_decrypt(&w, first)) {
                return Some(w);
            }
            a += 4;
        }
    }
    None
}

/// An RPF6 file: (path, offset, stored length, length (None for a resource), compressed, resource flags).
#[derive(Clone, Debug)]
pub struct RpfFile {
    pub name: String,
    pub off: u64,
    pub stored: u64,
    pub size: Option<u64>,
    pub comp: bool,
    pub rflags: Option<u32>,
}

pub fn rpf_files(read: Read, size: u64, key: Option<&[u8; 32]>, label: &str) -> R<Vec<RpfFile>> {
    let (n, names_at, enc) = rpf_header(&read(0, 16)?).ok_or_else(|| format!("{label}: no RPF6 archive"))?;
    if names_at > size {
        return Err(format!("{label}: the name table lies past the end"));
    }
    let mut t = read(16, (n as u64 * 20).div_ceil(16) * 16)?;
    let mut nt = read(names_at, size - names_at)?;
    if enc {
        let k = key.ok_or_else(|| format!("{label}: an encrypted RPF6 archive and no key"))?;
        t = rpf_decrypt(k, &t);
        nt = rpf_decrypt(k, &nt);
    }
    if !rpf_is_root(&t) {
        return Err(format!("{label}: the first entry is no root directory (wrong key?)"));
    }
    let be = |d: &[u8], o: usize| u32::from_be_bytes(d[o..o + 4].try_into().unwrap());
    let raw: Vec<[u32; 5]> = (0..n as usize).map(|i| [be(&t, 20 * i), be(&t, 20 * i + 4), be(&t, 20 * i + 8), be(&t, 20 * i + 12), be(&t, 20 * i + 16)]).collect();
    let mut names: std::collections::HashMap<u32, String> = Default::default();
    let mut s = 0;
    for (i, &b) in nt.iter().enumerate() {
        if b == 0 {
            if i > s && i - s <= 255 && nt[s..i].iter().all(|c| (0x20..=0x7e).contains(c)) {
                let st = String::from_utf8_lossy(&nt[s..i]).into_owned();
                names.insert(joaat(&st), st);
            }
            s = i + 1;
        } else if !(0x20..=0x7e).contains(&b) {
            s = i + 1;
        }
    }
    let mut files = vec![];
    fn walk(raw: &[[u32; 5]], names: &std::collections::HashMap<u32, String>, i: usize, prefix: &str, files: &mut Vec<RpfFile>, depth: usize, label: &str) -> R<()> {
        if depth > 64 {
            return Err(format!("{label}: directories nested too deep"));
        }
        let [h, a, b, c, _] = raw[i];
        let name = names.get(&h).cloned().unwrap_or_else(|| format!("#{h:08x}"));
        if b & 0x8000_0000 != 0 {
            let (first, count) = ((b & 0x7fff_ffff) as usize, c as usize);
            if first + count > raw.len() {
                return Err(format!("{label}: directory entry {i} runs past the table"));
            }
            let p = if i != 0 { format!("{prefix}{name}/") } else { String::new() };
            for j in first..first + count {
                walk(raw, names, j, &p, files, depth + 1, label)?;
            }
        } else if c & 0x8000_0000 != 0 {
            files.push(RpfFile { name: format!("{prefix}{name}"), off: (b & 0x7fff_ff00) as u64 * 8, stored: (a & 0x7fff_ffff) as u64, size: None, comp: c & 0x4000_0000 != 0, rflags: Some(c) });
        } else {
            files.push(RpfFile { name: format!("{prefix}{name}"), off: (b & 0x7fff_ffff) as u64 * 8, stored: (a & 0x7fff_ffff) as u64, size: Some((c & 0x3fff_ffff) as u64), comp: c & 0x4000_0000 != 0, rflags: None });
        }
        Ok(())
    }
    walk(&raw, &names, 0, "", &mut files, 0, label)?;
    Ok(files)
}

fn rpf_unpack(d: &[u8]) -> Option<Vec<u8>> {
    if d.starts_with(&archive::ZSTD_MAGIC) {
        return unzstd(d, "").ok();
    }
    if d.len() >= 2 && d[0] & 0xf == 8 && d[0] >> 4 <= 7 && ((d[0] as u32) << 8 | d[1] as u32) % 31 == 0 {
        let mut out = vec![];
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(d), &mut out).ok()?;
        return Some(out);
    }
    None
}

pub fn rpf_read(read: Read, e: &RpfFile, key: Option<&[u8; 32]>, label: &str) -> R<Vec<u8>> {
    let mut d = read(e.off, e.stored)?;
    if let Some(rf) = e.rflags {
        let h = d.get(..4).unwrap_or(&[]);
        let rsc85 = h == b"RSC\x85";
        if h != b"RSC\x05" && !rsc85 {
            return Err(format!("{label}: a resource entry, but no RSC5 / RSC85 header at {:#x}", e.off));
        }
        if u32_at(&d, 8) != Some(rf) {
            return Err(format!("{label}: the resource header's flags differ from the entry's"));
        }
        d = d[if rsc85 { 16 } else { 12 }..].to_vec();
    }
    if !e.comp && e.rflags.is_none() {
        return Ok(d);
    }
    let mut u = rpf_unpack(&d);
    if u.is_none() && e.comp && e.size.is_some() {
        u = inflate_raw(&d).filter(|x| Some(x.len() as u64) == e.size);
    }
    if u.is_none() {
        if let Some(k) = key {
            let mut x = rpf_decrypt(k, &d);
            x.extend_from_slice(&d[d.len() / 16 * 16..]);
            u = rpf_unpack(&x);
        }
    }
    let u = match u {
        Some(u) => u,
        None if !e.comp => return Ok(d),
        None => return Err(format!("{label}: compressed, but neither a zstd frame nor a zlib stream, also after decryption")),
    };
    if let Some(s) = e.size {
        if u.len() as u64 != s {
            return Err(format!("{label}: {} bytes decoded, the entry says {s}", u.len()));
        }
    }
    Ok(u)
}

// ---- Double Eleven's resource pack (.pkdswitch + .pkiswitch) ----

/// Its members (stored or one zlib stream each), from the index beside the data. → (name hash, data)
pub fn d11_members(idx: &[u8], d: &[u8], label: &str) -> R<Vec<(u32, Vec<u8>)>> {
    if idx.len() < 12 {
        return Err(format!("{label}: the pack index is cut short ({} bytes)", idx.len()));
    }
    let (ver, n) = (u32_at(idx, 0).unwrap(), u32_at(idx, 8).unwrap() as usize);
    if ver != 3 {
        return Err(format!("{label}: pack index version {ver}: only 3 is known"));
    }
    if 12 + 16 * n > idx.len() {
        return Err(format!("{label}: the pack index lists {n} members but holds {}", (idx.len() - 12) / 16));
    }
    let mut out = vec![];
    for k in 0..n {
        let o = 12 + 16 * k;
        let (size, stored, off, h) = (u32_at(idx, o).unwrap() as usize, u32_at(idx, o + 4).unwrap() as u64, u32_at(idx, o + 8).unwrap() as u64, u32_at(idx, o + 12).unwrap());
        let m = span(d, off, stored, &format!("{label}: member {k}"))?;
        let m = if stored as usize != size {
            let x = inflate(m, &format!("{label}: member {k} ({h:08x})"))?;
            if x.len() != size {
                return Err(format!("{label}: member {k} ({h:08x}) inflates to {} bytes, the index says {size}", x.len()));
            }
            x
        } else {
            m.to_vec()
        };
        out.push((h, m));
    }
    Ok(out)
}

// ---- UbiArt IPK ----

pub const UBIART_IPK: [u8; 4] = [0x50, 0xec, 0x12, 0xba];

/// The `.fxb` effect files of a version 7 IPK (not `shaders_dev/`). → (name, data)
pub fn ipk_fxb(read: Read, label: &str) -> R<Vec<(String, Vec<u8>)>> {
    let h = read(0, 0x38)?;
    if h.len() < 0x38 {
        return Err(format!("{label}: {} bytes, shorter than a UbiArt IPK header (0x38)", h.len()));
    }
    let be = |d: &[u8], o: usize| d.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let (ver, base, n) = (be(&h, 4).unwrap(), be(&h, 12).unwrap() as u64, be(&h, 16).unwrap() as usize);
    if ver != 7 {
        return Err(format!("{label}: UbiArt IPK version {ver} (only version 7 is read)"));
    }
    if base < 0x38 {
        return Err(format!("{label}: the IPK header puts its data at {base:#x}, outside the file"));
    }
    let t = read(0x38, base - 0x38)?;
    let mut p = 0usize;
    let mut out = vec![];
    for i in 0..n {
        let cut = || format!("{label}: entry {i} of {n} runs past the IPK table: not the layout of a version 7 archive");
        let (noff, size, csize) = (be(&t, p).ok_or_else(cut)? as usize, be(&t, p + 4).ok_or_else(cut)? as u64, be(&t, p + 8).ok_or_else(cut)? as u64);
        p += 20;
        let off = if noff != 0 { t.get(p..p + 8).map(|b| u64::from_be_bytes(b.try_into().unwrap())).ok_or_else(cut)? } else { 0 };
        p += 8 * noff;
        let pl = be(&t, p).ok_or_else(cut)? as usize;
        let dir: String = t.get(p + 4..p + 4 + pl).ok_or_else(cut)?.iter().map(|&c| c as char).collect();
        p += 4 + pl;
        let nl = be(&t, p).ok_or_else(cut)? as usize;
        let name: String = dir + &t.get(p + 4..p + 4 + nl).ok_or_else(cut)?.iter().map(|&c| c as char).collect::<String>();
        p += 4 + nl + 8;
        if !name.ends_with(".fxb") || format!("/{name}").contains("/shaders_dev/") {
            continue;
        }
        let want = if csize != 0 { csize } else { size };
        let mut b = read(base + off, want)?;
        if b.len() as u64 != want {
            return Err(format!("{label}/{name}: the entry at {:#x} runs past the end of the archive", base + off));
        }
        if csize != 0 {
            b = inflate(&b, &format!("{label}/{name}"))?;
        }
        if b.len() as u64 != size {
            return Err(format!("{label}/{name}: {} bytes decoded, the entry says {size}", b.len()));
        }
        out.push((name, b));
    }
    Ok(out)
}

// ---- Camelot's CRA archive and NARC containers ----

pub const CRA: &[u8; 4] = b"CRA.";

fn crc_tables(poly: u32) -> [[u32; 256]; 8] {
    let mut t = [[0u32; 256]; 8];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 != 0 { poly } else { 0 };
        }
        t[0][i as usize] = c;
    }
    for k in 1..8 {
        for i in 0..256 {
            t[k][i] = (t[k - 1][i] >> 8) ^ t[0][(t[k - 1][i] & 0xff) as usize];
        }
    }
    t
}

fn crc64(t: &[[u32; 256]; 8], c: u64, v: u64) -> u64 {
    let x = c ^ v;
    (t[7][(x & 0xff) as usize] ^ t[6][(x >> 8 & 0xff) as usize] ^ t[5][(x >> 16 & 0xff) as usize] ^ t[4][(x >> 24 & 0xff) as usize] ^ t[3][(x >> 32 & 0xff) as usize] ^ t[2][(x >> 40 & 0xff) as usize] ^ t[1][(x >> 48 & 0xff) as usize] ^ t[0][(x >> 56) as usize]) as u64
}

struct Crc {
    c32: [[u32; 256]; 8],
    c32c: [[u32; 256]; 8],
}

fn crcs() -> &'static Crc {
    static C: std::sync::OnceLock<Crc> = std::sync::OnceLock::new();
    C.get_or_init(|| Crc { c32: crc_tables(0xedb8_8320), c32c: crc_tables(0x82f6_3b78) })
}

fn cra_stem(path: &str) -> u64 {
    let e = path.rsplit('/').next().unwrap_or("");
    let n = [e.find('.'), e.find('_'), Some(4)].into_iter().flatten().min().unwrap_or(4).min(e.len());
    e.bytes().take(n).fold(0u64, |a, b| a << 8 | b as u64)
}

fn cra_key(path: &str) -> u64 {
    let t = &crcs().c32;
    let k = crc64(t, cra_stem(path), u64::from_le_bytes(*b"UseUrSk1"));
    let a = crc64(t, crc64(t, 0xffff_ffff, u64::from_le_bytes(*b"Nintendo")), u64::from_le_bytes(*b"/CAMELOT"));
    (a ^ crc64(t, k, u64::from_le_bytes(*b"lls4G00d")) ^ 0x0205_2636) & 0xffff_ffff
}

const VARIANT_MASK: [u64; 8] = [0x1111_1111, 0x2222_2223, 0x3333_3333, 0x4444_4445, 0x5555_5555, 0x6666_6667, 0x7777_7777, 0x8888_8889];

/// A file's 16-byte blocks unscrambled, and the sum its checksum comes from.
fn cra_unscramble(path: &str, raw: &[u8]) -> (Vec<u8>, u64) {
    let w = cra_key(path);
    let v = (w & 7) as usize;
    let t = if v & 4 != 0 { &crcs().c32c } else { &crcs().c32 };
    let mul = ((w & 0xffff_fffe) ^ VARIANT_MASK[v]) << 32 | 1;
    let n = raw.len() / 16;
    let q: Vec<u64> = raw[..n * 16].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
    let mut out = vec![0u64; 2 * n];
    let (src, oth) = if v & 2 != 0 { (1, 0) } else { (0, 1) };
    let (a, b) = if v & 1 != 0 { (1, 0) } else { (0, 1) };
    let mut total = 0u64;
    for i in (0..2 * n).step_by(2) {
        let s = q[i + src];
        let x = s ^ w;
        let m = crc64(t, 0, x).wrapping_mul(mul);
        let e = q[i + oth] ^ (m >> 3);
        out[i + a] = !(m ^ (s << 32));
        out[i + b] = e;
        total = total.wrapping_add(e);
    }
    (out.iter().flat_map(|x| x.to_le_bytes()).collect(), total)
}

fn adrp_add(w: &[u32], i: usize, reg: u32) -> Option<usize> {
    let (a, b) = (*w.get(i)?, *w.get(i + 1)?);
    if a & 0x9f00_001f != 0x9000_0000 | reg || b & 0xffc0_03ff != 0x9100_0000 | reg << 5 | reg {
        return None;
    }
    let mut hi = (((a >> 5 & 0x7ffff) << 2) | (a >> 29 & 3)) as i64;
    if hi & 1 << 20 != 0 {
        hi -= 1 << 21;
    }
    Some(((i as i64 * 4 & !0xfff) + (hi << 12) + (b >> 10 & 0xfff) as i64) as usize)
}

fn nro_words(nro: &[u8]) -> R<Vec<u32>> {
    if nro.get(0x10..0x14) != Some(b"NRO0") {
        return Err("not an NRO".into());
    }
    let end = (u32_at(nro, 0x24).unwrap_or(0) & !3) as usize;
    Ok(nro[..end.min(nro.len())].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
}

/// A CRA file: (index, path, size, offset, checksum, 2021 layout).
pub type CraEntry = (usize, String, u64, u64, u32, bool);

/// The file table an NRO's code reads: today's layout, else Mario Golf: Super Rush's (2021).
pub fn cra_index(nro: &[u8]) -> R<Vec<CraEntry>> {
    cra_index_now(nro).or_else(|_| cra_index_2021(nro))
}

fn cra_index_now(nro: &[u8]) -> R<Vec<CraEntry>> {
    let w = nro_words(nro)?;
    let (mut table, mut count, mut sums, mut names) = (None, 0usize, None, None);
    for i in 3..w.len().saturating_sub(3) {
        let c = w[i];
        if table.is_none() && c & 0xffe0_fc00 == 0x78e0_d800 && w[i - 1] & 0xffff_ffe0 == 0x5280_0240 && w[i + 1] & 0xffe0_8000 == 0x1b00_0000 && w[i + 2] & 0xffe0_0000 == 0x5280_0000 {
            if let Some(a) = adrp_add(&w, i - 3, c >> 5 & 31) {
                table = Some(a);
                count = ((w[i + 2] >> 5 & 0xffff) / 2) as usize;
            }
        } else if sums.is_none() && c & 0xffff_ffe0 == 0xd28e_6aa0 && w[i + 1] & 0xffff_ffe0 == 0xf2aa_aca0 {
            for k in i - 3..i {
                for reg in 0..31 {
                    if let Some(a) = adrp_add(&w, k, reg) {
                        sums = Some(a);
                    }
                }
            }
        }
    }
    let (Some(table), Some(sums)) = (table, sums) else { return Err("this NRO has no CRA file table (or another layout of it)".into()) };
    for i in 0..w.len().saturating_sub(5) {
        if w[i] & 0xffff_fc00 == 0x39c0_0c00 && w[i + 1] & 0xff00_0000 == 0x3700_0000 && w[i + 2] & 0xffff_fc00 == 0x9240_1c00 {
            for reg in 0..31 {
                if adrp_add(&w, i + 3, reg).is_some() && w.get(i + 5).is_some_and(|x| x & 0xffe0_fc00 == 0xf860_7800) {
                    names = adrp_add(&w, i + 3, reg);
                }
            }
        }
    }
    let d = nro;
    let rec = |i: usize| -> Option<usize> { Some((table as i64 + i16::from_le_bytes(d.get(table + 2 * i..table + 2 * i + 2)?.try_into().ok()?) as i64 + i as i64 * 18 + count as i64 * 2) as usize) };
    let name = |i: usize| -> Option<String> {
        let r = rec(i)?;
        let mut need = *d.get(r + 2)? as usize;
        let mut out = vec![0u8; need];
        if need != 0 {
            let mut k = i.checked_sub(u16_at(d, r)? as usize)?;
            loop {
                let s = rec(k)?;
                let have = need.min(*d.get(s + 2)? as usize);
                for j in 0..need - have {
                    out[have + j] = (k as u8).wrapping_sub(*d.get(s + 0xd + j)?) & 0x7f;
                }
                need = have;
                if need == 0 {
                    break;
                }
                k = k.checked_sub(u16_at(d, s)? as usize)?;
            }
        }
        let mut o = r + 0xd;
        while *d.get(o)? != 0 {
            out.push((i as u8).wrapping_sub(d[o]) & 0x7f);
            o += 1;
        }
        let sfx = *d.get(r + 3)?;
        if sfx < 0x80 {
            if let Some(nb) = names {
                let mut o = u64_at(d, nb + sfx as usize * 8)? as usize;
                while *d.get(o)? != 0 {
                    out.push(sfx.wrapping_sub(d[o]) & 0x7f);
                    o += 1;
                }
            }
        }
        let s: String = out.iter().map(|&c| c as char).collect();
        let mut parts: Vec<String> = vec![];
        for c in s.split('/') {
            let c = match parts.last() {
                Some(prev) => c.replace('$', prev),
                None => c.to_string(),
            };
            parts.push(c);
        }
        Some(parts.join("/"))
    };
    let mut out = vec![];
    for i in 0..count {
        let r = rec(i).ok_or("CRA: a record outside the NRO")?;
        let size = u32_at(d, r + 4).ok_or("CRA: a record outside the NRO")? ^ 0xff00_ff00;
        if size == 0 {
            continue;
        }
        let off = (u64_at(d, r + 8).ok_or("CRA: a record outside the NRO")? & 0xff_ffff_ffff) ^ 0xff_00ff_00ff;
        let sum = u32_at(d, sums + i * 4).ok_or("CRA: the checksums run past the NRO")?;
        out.push((i, name(i).ok_or_else(|| format!("CRA: file {i}'s name runs past the NRO"))?, size as u64, off, sum, false));
    }
    Ok(out)
}

fn rev8(b: u8) -> u8 {
    b.reverse_bits()
}

fn cra_index_2021(nro: &[u8]) -> R<Vec<CraEntry>> {
    let w = nro_words(nro)?;
    let mut table = None;
    'f: for i in 8..w.len().saturating_sub(1) {
        if w[i] & 0xffff_ffe0 != 0x5280_0000 | 0x2eca << 5 || w[i + 1] & 0xffff_ffe0 != 0x72a0_0000 | 0xce46 << 5 || (w[i] ^ w[i + 1]) & 31 != 0 {
            continue;
        }
        for k in i - 8..i {
            let reg = w[k] & 31;
            if let Some(a) = adrp_add(&w, k, reg) {
                if w[k + 2] & 0xffe0_fc00 == 0xb8a0_7800 && w[k + 2] >> 5 & 31 == reg {
                    table = Some(a);
                    break 'f;
                }
            }
        }
    }
    let table = table.ok_or("this NRO has no CRA file table (or another layout of it)")?;
    let d = nro;
    let first = i32::from_le_bytes(d.get(table..table + 4).ok_or("CRA 2021: table outside the NRO")?.try_into().unwrap());
    if first >= 0 || first % 4 != 0 {
        return Err("the 2021 file table does not start with its records' distance".into());
    }
    let count = (-first / 4) as usize;
    let rec = |i: usize| -> Option<usize> { (table as i64 - i32::from_le_bytes(d.get(table + i * 4..table + i * 4 + 4)?.try_into().ok()?) as i64).try_into().ok() };
    let chars = |i: usize, r: usize| -> Option<Vec<u8>> {
        let (mut o, k) = (r + 0x11, (i & 15) as u8 + 1);
        let mut out = vec![];
        while *d.get(o)? != 0 {
            out.push(rev8(d[o]) ^ k);
            o += 1;
        }
        Some(out)
    };
    let name = |i: usize| -> Option<String> {
        let mut r = rec(i)?;
        let mut need = *d.get(r + 2)? as usize;
        let mut out = vec![0u8; need];
        out.extend(chars(i, r)?);
        let mut k = i;
        while need != 0 {
            let back = u16_at(d, r)? as usize;
            if back == 0 {
                return None;
            }
            k = k.checked_sub(back)?;
            r = rec(k)?;
            let have = need.min(*d.get(r + 2)? as usize);
            let c = chars(k, r)?;
            out[have..need].copy_from_slice(c.get(..need - have)?);
            need = have;
        }
        Some(out.iter().map(|&c| c as char).collect())
    };
    let mut out = vec![];
    for i in 0..count {
        let r = rec(i).ok_or("CRA 2021: a record outside the NRO")?;
        let raw = u32_at(d, r + 4).ok_or("CRA 2021: a record outside the NRO")?;
        let size = raw ^ 0xce46_2eca;
        if raw == 0 || size == 0 {
            continue;
        }
        let off = (u64_at(d, r + 0xc).ok_or("CRA 2021: a record outside the NRO")? & 0xff_ffff_ffff) ^ 0x46_f636_762a;
        out.push((i, name(i).ok_or_else(|| format!("CRA 2021: file {i}'s name cannot be read"))?, size as u64, off, u32_at(d, r + 8).unwrap(), true));
    }
    Ok(out)
}

fn cra_keys_2021(path: &str) -> R<[u8; 16]> {
    let p = path.as_bytes();
    let st = path.rfind('/').map_or(0, |x| x + 1);
    if st >= p.len() {
        return Err(format!("{path}: no last path element to take the key from"));
    }
    let mut q = st;
    let mut kb = [0u8; 8];
    for x in &mut kb {
        *x = rev8(p[q]);
        q += 1;
        if q >= p.len() || p[q] == b'.' {
            q = st;
        }
    }
    let k = u64::from_be_bytes(kb);
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&(k ^ 0x4adf_c980_eae9_731b).to_le_bytes());
    out[8..].copy_from_slice(&(k ^ 0xfbcc_b4fa_c6ff_003e).to_le_bytes());
    Ok(out)
}

fn cra_plain_2021(path: &str, raw: &[u8]) -> R<Vec<u8>> {
    let ks = cra_keys_2021(path)?;
    Ok(raw.iter().enumerate().map(|(i, &b)| rev8(b ^ ks[i % 16])).collect())
}

/// A CRA file's first 16 bytes, unscrambled (its magic).
pub fn cra_head(read: Read, e: &CraEntry) -> R<Vec<u8>> {
    if e.5 {
        return cra_plain_2021(&e.1, &read(e.3, e.2.min(16))?);
    }
    Ok(cra_unscramble(&e.1, &read(e.3, 16)?).0)
}

/// A CRA file's bytes, unscrambled and checked against its checksum.
pub fn cra_read(read: Read, e: &CraEntry) -> R<Vec<u8>> {
    let (_, path, size, off, want, y2021) = e;
    if *y2021 {
        let plain = cra_plain_2021(path, &read(*off, *size)?)?;
        let mut h = flate2::Crc::new();
        h.update(&plain);
        if plain.len() as u64 != *size || h.sum() ^ 0x1234_5678 != *want {
            return Err(format!("{path}: the checksum differs"));
        }
        return Ok(plain);
    }
    let (mut plain, total) = cra_unscramble(path, &read(*off, (*size + 15) & !15)?);
    if (((total >> 32) ^ total) & 0xffff_ffff) as u32 != *want {
        return Err(format!("{path}: the checksum differs"));
    }
    plain.truncate(*size as usize);
    Ok(plain)
}

/// A NARC container's members (BTAF offsets into GMIF).
pub fn narc_members<'a>(d: &'a [u8], at: &str) -> R<Vec<&'a [u8]>> {
    if d.len() < 16 || &d[..4] != b"NARC" {
        return Err(format!("{at}: NARC header cut short"));
    }
    let (mut o, n) = (u16_at(d, 0xc).unwrap() as usize, u16_at(d, 0xe).unwrap() as usize);
    if o < 16 {
        return Err(format!("{at}: NARC sections overlap the header"));
    }
    let mut secs: std::collections::HashMap<[u8; 4], (usize, usize)> = Default::default();
    for _ in 0..n {
        if o + 8 > d.len() {
            return Err(format!("{at}: a section past the end"));
        }
        let s = u32_at(d, o + 4).unwrap() as usize;
        if s < 8 || o + s > d.len() {
            return Err(format!("{at}: invalid NARC section size at {o:#x}"));
        }
        secs.insert(d[o..o + 4].try_into().unwrap(), (o, s));
        o += s;
    }
    let (Some(&(fat, size)), Some(&(image, image_size))) = (secs.get(b"BTAF"), secs.get(b"GMIF")) else { return Err(format!("{at}: no BTAF or GMIF")) };
    if size < 12 {
        return Err(format!("{at}: NARC BTAF header cut short"));
    }
    let count = u32_at(d, fat + 8).unwrap() as usize;
    if count > (size - 12) / 8 {
        return Err(format!("{at}: NARC BTAF member table cut short"));
    }
    let data = image + 8;
    (0..count)
        .map(|i| {
            let (a, b) = (u32_at(d, fat + 12 + 8 * i).unwrap() as usize, u32_at(d, fat + 16 + 8 * i).unwrap() as usize);
            if !(a <= b && b <= image_size - 8) {
                return Err(format!("{at}: member {i} past the GMIF end"));
            }
            Ok(&d[data + a..data + b])
        })
        .collect()
}

fn cra_lz(d: &[u8], mut at: usize) -> R<Vec<u8>> {
    let mut out: Vec<u8> = vec![];
    let n = d.len();
    let cut = || "a packed member cut short".to_string();
    while at < n {
        let f = d[at];
        at += 1;
        if f == 0 {
            out.extend_from_slice(&d[at..(at + 8).min(n)]);
            at += 8;
            continue;
        }
        let f = f.wrapping_neg();
        for bit in (0..8).rev() {
            if f >> bit & 1 == 0 {
                out.push(*d.get(at).ok_or_else(cut)?);
                at += 1;
                continue;
            }
            let (lo, hi) = (*d.get(at).ok_or_else(cut)? as usize, *d.get(at + 1).ok_or_else(cut)? as usize);
            let dist = lo >> 4 | hi << 4;
            if dist == 0 {
                return Ok(out);
            }
            let mut ln = lo & 0xf;
            if ln > 1 {
                at += 2;
            } else if ln == 1 {
                ln = *d.get(at + 2).ok_or_else(cut)? as usize + 0x11 + ((*d.get(at + 3).ok_or_else(cut)? as usize) << 8) + 0xff;
                at += 4;
            } else {
                ln = *d.get(at + 2).ok_or_else(cut)? as usize + 0x10;
                at += 3;
            }
            let s = out.len().checked_sub(dist).ok_or("an LZ distance before the start")?;
            for k in 0..ln {
                out.push(out[s + k]);
            }
        }
    }
    Ok(out)
}

/// A NARC member unpacked by the kind in its first byte, as the game does; anything else is the member itself.
pub fn cra_undo(d: &[u8], depth: usize) -> R<Vec<u8>> {
    if d.is_empty() || depth > 16 {
        return Ok(d.to_vec());
    }
    let k = d[0];
    if k == 0x4d && d.starts_with(b"MMMM") {
        let n = u32_at(d, 8).ok_or("MMMM header cut short")? as usize;
        let mut at = (12 + 4 * n + 15) & !15;
        let mut out = vec![];
        for i in 0..n {
            let s = u32_at(d, 12 + 4 * i).ok_or("MMMM table cut short")? as usize;
            out.extend(cra_undo(d.get(at..at + s).ok_or("MMMM chunk past the end")?, depth + 1)?);
            at += (s + 15) & !15;
        }
        return Ok(out);
    }
    if k == 0x94 && d.len() >= 8 {
        return lz4_flex::block::decompress(&d[8..], u32_at(d, 4).unwrap() as usize).map_err(|e| format!("LZ4: {e}"));
    }
    if k == 0x5a && d.starts_with(b"Z\0\0\0") {
        return unzstd(&d[8..], "");
    }
    if k == 0x50 && d.len() >= 8 {
        return cra_lz(d, 8);
    }
    if k == 0x60 && d.len() >= 4 {
        return cra_lz(d, 4);
    }
    if (k == 0x70 || k == 0x80) && d.len() >= 0x10 {
        return Ok(d[0x10..].to_vec());
    }
    if matches!(k, 0 | 0x10 | 0x20 | 0x30) && d.len() >= 0x10 {
        let s = &d[0x10..];
        let mut out = vec![];
        let ok = if k == 0x30 {
            std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(s), &mut out).is_ok()
        } else {
            // zlib or gzip (zlib's wbits 47 takes either)
            if s.starts_with(&[0x1f, 0x8b]) {
                std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(s), &mut out).is_ok()
            } else {
                std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(s), &mut out).is_ok()
            }
        };
        return Ok(if ok { out } else { d.to_vec() });
    }
    Ok(d.to_vec())
}

// ---- CASC (Diablo II: Resurrected) ----

/// A CASC file: (file ID, archive name, offset, stored length, decoded length, encoding).
pub type CascEntry = (String, String, u64, u64, u64, String);

/// The current build's files from a folder's manifests: `build_key` the build_key.txt text, `manifests` every .psv's
/// (name, text).
pub fn casc_files(build_key: &str, manifests: &[(String, String)], label: &str) -> R<Vec<CascEntry>> {
    let want = build_key.trim();
    for (n, text) in manifests {
        if !(text.starts_with('#') || text.starts_with("FileId")) || !text[..text.len().min(4096)].contains("FileId!") {
            continue;
        }
        let (mut key, mut cols, mut rows): (Option<String>, Option<Vec<String>>, Vec<Vec<String>>) = (None, None, vec![]);
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            if let Some(c) = line.strip_prefix('#') {
                let c = c.trim_start_matches('#');
                if let Some((k, v)) = c.split_once('=') {
                    if k.trim() == "config-key" {
                        key = Some(v.trim().to_string());
                    }
                }
                continue;
            }
            let f: Vec<String> = line.split('|').map(str::to_string).collect();
            match &cols {
                None => {
                    if !f[0].contains('!') {
                        return Err(format!("{label}/{n}: no header line before the first file line"));
                    }
                    let c: Vec<String> = f.iter().map(|c| c.split('!').next().unwrap().to_string()).collect();
                    let missing: Vec<&str> = ["FileId", "Chunk", "Archive", "CSize", "EKey", "ESize", "Encoding"].into_iter().filter(|x| !c.iter().any(|y| y == x)).collect();
                    if !missing.is_empty() {
                        return Err(format!("{label}/{n}: manifest without the columns {}", missing.join(", ")));
                    }
                    cols = Some(c);
                }
                Some(c) => {
                    if f.len() != c.len() {
                        return Err(format!("{label}/{n}: {} fields where the header names {}", f.len(), c.len()));
                    }
                    rows.push(f);
                }
            }
        }
        if key.as_deref() != Some(want) {
            continue;
        }
        let cols = cols.ok_or_else(|| format!("{label}/{n}: no header line"))?;
        let ix = |name: &str| cols.iter().position(|c| c == name).unwrap();
        let mut out = vec![];
        for r in rows {
            let e: Vec<u8> = (0..r[ix("EKey")].len() / 2).filter_map(|i| u8::from_str_radix(&r[ix("EKey")][2 * i..2 * i + 2], 16).ok()).collect();
            let chunk: u8 = r[ix("Chunk")].parse().map_err(|_| format!("{label}/{n}: bad Chunk"))?;
            let arch: u8 = r[ix("Archive")].parse().map_err(|_| format!("{label}/{n}: bad Archive"))?;
            if e.len() != 16 || (e[9], e[10]) != (chunk, arch) {
                return Err(format!("{label}/{n}: {}: encoded key {} does not end in chunk {chunk}, archive {arch} and an offset", r[ix("FileId")], r[ix("EKey")]));
            }
            let off = e[11..].iter().fold(0u64, |a, &b| a << 8 | b as u64);
            let num = |c: &str| r[ix(c)].parse::<u64>().map_err(|_| format!("{label}/{n}: bad {c}"));
            out.push((r[ix("FileId")].clone(), format!("{chunk:02}-{arch:08}.data"), off, num("ESize")?, num("CSize")?, r[ix("Encoding")].clone()));
        }
        return Ok(out);
    }
    Err(format!("{label}: no manifest has the config key {want} that build_key.txt names"))
}

fn blte_chunk(c: &[u8], label: &str) -> R<Vec<u8>> {
    match c.first() {
        Some(b'N') => Ok(c[1..].to_vec()),
        Some(b'Z') => inflate(&c[1..], label),
        x => Err(format!("{label}: BLTE chunk mode {x:?}")),
    }
}

/// A CASC file's stored bytes decoded by its encoding ('n' stored, 'z' zlib, 'b:…' BLTE).
pub fn casc_decode(d: &[u8], enc: &str, size: u64, label: &str) -> R<Vec<u8>> {
    let x = if enc == "n" {
        d.to_vec()
    } else if enc == "z" {
        inflate(d, label)?
    } else if enc.starts_with("b:") {
        if d.get(..4) != Some(b"BLTE") {
            return Err(format!("{label}: not BLTE"));
        }
        let hl = u32::from_be_bytes(d[4..8].try_into().unwrap()) as usize;
        if hl == 0 {
            blte_chunk(&d[8..], label)?
        } else {
            if d.get(8) != Some(&0x0f) {
                return Err(format!("{label}: BLTE chunk table flags {:?}", d.get(8)));
            }
            let n = u32::from_be_bytes([0, d[9], d[10], d[11]]) as usize;
            if 12 + 24 * n != hl {
                return Err(format!("{label}: BLTE header of {hl} bytes for {n} chunks"));
            }
            let mut at = hl;
            let mut out = vec![];
            for i in 0..n {
                let o = 12 + 24 * i;
                let (stored, sz) = (u32::from_be_bytes(d[o..o + 4].try_into().unwrap()) as usize, u32::from_be_bytes(d[o + 4..o + 8].try_into().unwrap()) as usize);
                let x = blte_chunk(d.get(at..at + stored).ok_or_else(|| format!("{label}: BLTE chunk {i} past the end"))?, label)?;
                if x.len() != sz {
                    return Err(format!("{label}: BLTE chunk {i}: {} bytes where the table says {sz}", x.len()));
                }
                out.extend(x);
                at += stored;
            }
            if at != d.len() {
                return Err(format!("{label}: BLTE chunks end at {at}, the container at {}", d.len()));
            }
            out
        }
    } else {
        return Err(format!("{label}: encoding {enc:?}"));
    };
    if x.len() as u64 != size {
        return Err(format!("{label}: {} bytes where the manifest says {size}", x.len()));
    }
    Ok(x)
}

// ---- Diablo III packages ----

pub const D3CPK_MAGIC: [u8; 4] = 0xa1b2_c3d4u32.to_le_bytes();
pub const D3_SPAN: u64 = 6 + 0xffff;
const D3_HEAD: usize = 0x48;

pub fn is_d3cpk(h: &[u8]) -> bool {
    h.len() >= 8 && h[..4] == D3CPK_MAGIC && matches!(u32_at(h, 4), Some(6 | 7))
}

/// The zlib streams whose 6-byte record {u32 decoded length, u16 stored length} starts in d[start..end] (`d` holds the
/// file from `base`). → (record offset, stored length, data)
pub fn d3_streams(d: &[u8], start: usize, end: usize, base: u64) -> Vec<(usize, usize, Vec<u8>)> {
    let mut out = vec![];
    let mut last = 0usize;
    let lo = (start + 6).max((D3_HEAD as i64 - base as i64 + 6).max(0) as usize);
    let hi = (end + 6).min(d.len());
    let mut i = lo;
    while i + 1 < hi {
        if !(d[i] == 0x78 && matches!(d[i + 1], 0x01 | 0x5e | 0x9c | 0xda)) {
            i += 1;
            continue;
        }
        let p = i - 6;
        i += 1;
        if p < last {
            continue;
        }
        let (n, stored) = (u32_at(d, p).unwrap() as usize, u16_at(d, p + 4).unwrap() as usize);
        if n == 0 || stored < 3 || p + 6 + stored > d.len() {
            continue;
        }
        let mut z = flate2::Decompress::new(true);
        let mut x = Vec::with_capacity(n + 1);
        let ok = matches!(z.decompress_vec(&d[p + 6..p + 6 + stored], &mut x, flate2::FlushDecompress::Finish), Ok(flate2::Status::StreamEnd));
        if !ok || z.total_in() as usize != stored || x.len() != n {
            continue;
        }
        last = p + 6 + stored;
        out.push((p, stored, x));
    }
    out
}

/// A byte range of a Diablo III package: control sections each directly followed by their code area in its streams.
pub fn d3_shaders(d: &[u8], start: u64, end: u64, label: &str, problems: &mut Vec<String>) -> Vec<Found> {
    let mut out = vec![];
    let mut lost = vec![];
    for (p, _, x) in d3_streams(d, 0, (end - start) as usize, start) {
        let lb = format!("{label}@{:#x}", start + p as u64);
        for c in memchr::memmem::find_iter(&x, &CTL_MAGIC) {
            if c + 0x708 > x.len() {
                continue;
            }
            let (n, k) = (u32_at(&x, c + 0x20).unwrap() as usize, u32_at(&x, c + 0x704).unwrap() as usize);
            if !(0x760..=0x880).contains(&n) {
                continue;
            }
            let (ctl, code) = (&x[c..(c + n).min(x.len())], x.get(c + n..c + n + k).unwrap_or(&[]));
            let st = if code.len() == k && ctl.len() == n { crate::formats::glslc::stage(ctl, code) } else { None };
            match st {
                Some(st) if check(st, ctl, code).is_none() => out.push(Found::new(format!("{lb}#D3@{c:#x}"), st, ctl, code)),
                _ => lost.push(format!("{lb}+{c:#x}")),
            }
        }
    }
    if !lost.is_empty() {
        problems.push(format!("{label} {start:#x}..{end:#x}: {} control section(s) not followed by their code area in the same stream, left out (first: {})", lost.len(), lost[0]));
    }
    out
}

// ---- PDAT (Kirby Air Riders) ----

pub const PDAT: &[u8; 4] = b"PDAT";
pub const PDAT_SPAN: u64 = 1 << 20;

/// A byte range of a PDAT archive (`d` holds it and PDAT_SPAN bytes more): every PMAP record (a zstd frame, or stored),
/// its control sections (NVN2) each directly followed by its code area.
pub fn pdat_shaders(d: &[u8], start: u64, end: u64, label: &str) -> R<Vec<Found>> {
    let n = (end - start) as usize;
    let mut out = vec![];
    let ctl_tag = [CTL_MAGIC[0], CTL_MAGIC[1], CTL_MAGIC[2], CTL_MAGIC[3], 0xc9, 0, 0, 0];
    let program = |x: &[u8], at: u64, lb: &str, out: &mut Vec<Found>| -> R<()> {
        for c in memchr::memmem::find_iter(x, &ctl_tag) {
            if c & 7 != 0 || c + NVN2_KEY_CTL > x.len() {
                continue;
            }
            let (nctl, ncode) = (u32_at(x, c + 0x30).unwrap() as usize, u32_at(x, c + 0x44).unwrap() as usize);
            if nctl < NVN2_KEY_CTL || c + nctl + ncode > x.len() {
                return Err(format!("{lb}: the control section at {c:#x} runs past its record"));
            }
            let (ctl, code) = (&x[c..c + nctl], &x[c + nctl..c + nctl + ncode]);
            let st = nvn2_stage(ctl, code);
            let why = match st {
                Some(st) => check(st, ctl, code),
                None => Some("no stage"),
            };
            if let Some(w) = why {
                return Err(format!("{lb}: the shader at {c:#x} is not one: {w}"));
            }
            out.push(Found::new(format!("{lb}@{at:#x}#var0"), st.unwrap(), ctl, code).grouped(format!("{lb}@{at:#x}#var0")));
        }
        Ok(())
    };
    let lb = format!("{label}#PMAP");
    let mut frames: Vec<(usize, usize)> = vec![];
    for p in memchr::memmem::find_iter(&d[..n.min(d.len())], &archive::ZSTD_MAGIC) {
        let head = archive::zstd_peek(&d[p..(p + 0x21000).min(d.len())], 8).unwrap_or_default();
        if head.get(4..8) != Some(b"PMAP") {
            continue;
        }
        let lim = (p + PDAT_SPAN as usize).min(d.len());
        let Some((x, used)) = archive::zstd_frame(&d[p..lim]) else {
            return Err(format!("{label}: the PMAP frame at {:#x} is longer than {PDAT_SPAN:#x} bytes", start + p as u64));
        };
        frames.push((p, p + used));
        program(&x, start + p as u64, &lb, &mut out)?;
    }
    for c in memchr::memmem::find_iter(&d[..n.min(d.len())], &ctl_tag) {
        if frames.iter().any(|&(a, b)| a <= c && c < b) {
            continue;
        }
        let k = (u32_at(d, c + 0x30).unwrap_or(0) + u32_at(d, c + 0x44).unwrap_or(0)) as usize;
        program(d.get(c..c + k).unwrap_or(&d[c..]), start + c as u64, &lb, &mut out)?;
    }
    Ok(out)
}

// ---- zip archives ----

pub const ZIP: &[u8; 4] = b"PK\x03\x04";
pub const VZIP: &[u8; 4] = b"VZIP";

/// A zip member: (name, local header offset, method, stored size, size, CRC).
#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub name: String,
    pub header: u64,
    pub method: u16,
    pub stored: u64,
    pub size: u64,
    pub crc: u32,
}

/// The central directory's members (zip64 included), directories left out.
pub fn zip_entries(read: Read, file_size: u64, label: &str) -> R<Vec<ZipEntry>> {
    let tail_n = file_size.min(0x10000 + 22);
    let tail = read(file_size - tail_n, tail_n)?;
    let eocd = (0..tail.len().saturating_sub(21)).rev().find(|&i| tail[i..].starts_with(b"PK\x05\x06")).ok_or_else(|| format!("{label}: no end of central directory"))?;
    let mut count = u16_at(&tail, eocd + 10).unwrap() as u64;
    let mut cd_size = u32_at(&tail, eocd + 12).unwrap() as u64;
    let mut cd_off = u32_at(&tail, eocd + 16).unwrap() as u64;
    if (count == 0xffff || cd_off == 0xffff_ffff || cd_size == 0xffff_ffff) && eocd >= 20 && tail[eocd - 20..].starts_with(b"PK\x06\x07") {
        let z64 = u64_at(&tail, eocd - 12).unwrap();
        let r = read(z64, 56)?;
        if !r.starts_with(b"PK\x06\x06") {
            return Err(format!("{label}: zip64 end of central directory missing"));
        }
        count = u64_at(&r, 32).unwrap();
        cd_size = u64_at(&r, 40).unwrap();
        cd_off = u64_at(&r, 48).unwrap();
    }
    // a prefix before the archive (Star Fox's VZIP index): the directory sits where its size says, before the EOCD
    let eocd_abs = file_size - tail_n + eocd as u64;
    let shift = eocd_abs.saturating_sub(cd_size).saturating_sub(cd_off);
    let shift = if read(cd_off, 4).ok().as_deref() == Some(b"PK\x01\x02") { 0 } else { shift };
    let cd = read(cd_off + shift, cd_size)?;
    let mut out = vec![];
    let mut p = 0usize;
    for _ in 0..count {
        if cd.get(p..p + 4) != Some(b"PK\x01\x02") {
            return Err(format!("{label}: central directory entry at {p:#x} damaged"));
        }
        let method = u16_at(&cd, p + 10).unwrap();
        let crc = u32_at(&cd, p + 16).unwrap();
        let (mut stored, mut size) = (u32_at(&cd, p + 20).unwrap() as u64, u32_at(&cd, p + 24).unwrap() as u64);
        let (nl, xl, cl) = (u16_at(&cd, p + 28).unwrap() as usize, u16_at(&cd, p + 30).unwrap() as usize, u16_at(&cd, p + 32).unwrap() as usize);
        let mut header = u32_at(&cd, p + 42).unwrap() as u64;
        let name = String::from_utf8_lossy(cd.get(p + 46..p + 46 + nl).ok_or_else(|| format!("{label}: central directory cut short"))?).into_owned();
        // zip64 extra field
        let mut x = p + 46 + nl;
        let xe = x + xl;
        while x + 4 <= xe.min(cd.len()) {
            let (id, len) = (u16_at(&cd, x).unwrap(), u16_at(&cd, x + 2).unwrap() as usize);
            if id == 1 {
                let mut q = x + 4;
                if size == 0xffff_ffff {
                    size = u64_at(&cd, q).unwrap_or(size);
                    q += 8;
                }
                if stored == 0xffff_ffff {
                    stored = u64_at(&cd, q).unwrap_or(stored);
                    q += 8;
                }
                if header == 0xffff_ffff {
                    header = u64_at(&cd, q).unwrap_or(header);
                }
            }
            x += 4 + len;
        }
        p = xe + cl;
        if !name.ends_with('/') {
            out.push(ZipEntry { name, header: header + shift, method, stored, size, crc });
        }
    }
    Ok(out)
}

/// A zip member's data (stored or deflated), or Star Fox's chained LZ4 chunks for a member the directory calls stored
/// at another size than its file size; the CRC is checked.
pub fn zip_read(read: Read, e: &ZipEntry, label: &str) -> R<Vec<u8>> {
    let h = read(e.header, 30)?;
    if !h.starts_with(ZIP) {
        return Err(format!("{label}: no local header at {:#x}", e.header));
    }
    let (nl, xl) = (u16_at(&h, 26).unwrap() as u64, u16_at(&h, 28).unwrap() as u64);
    let d = read(e.header + 30 + nl + xl, e.stored)?;
    let out = match e.method {
        0 if e.stored != e.size => lz4_chained(&d, e.size as usize, 0x10000, label)?,
        0 => d,
        8 => inflate_raw(&d).ok_or_else(|| format!("{label}: damaged deflate stream"))?,
        m => return Err(format!("{label}: zip compression method {m} is not read")),
    };
    if out.len() as u64 != e.size {
        return Err(format!("{label}: {} bytes decoded, the directory says {}", out.len(), e.size));
    }
    let mut c = flate2::Crc::new();
    c.update(&out);
    if c.sum() != e.crc {
        return Err(format!("{label}: CRC mismatch"));
    }
    Ok(out)
}

// ---- Larian LSPK v13 ----

pub const LSPK: &[u8; 4] = b"LSPK";

/// An LSPK member: (name, part, offset, stored length, decoded length, method).
pub type LspkEntry = (String, usize, u64, u64, u64, u32);

fn lspk_decode(data: &[u8], size: usize, method: u32, what: &str) -> R<Vec<u8>> {
    if size > 256 << 20 {
        return Err(format!("{what}: decoded size exceeds {} bytes", 256 << 20));
    }
    let out = match method {
        0 => data.to_vec(),
        1 => inflate(data, what)?,
        2 => lz4_flex::block::decompress(data, size).map_err(|e| format!("{what}: damaged LZ4 block: {e}"))?,
        m => return Err(format!("{what}: unsupported compression {m}")),
    };
    if out.len() != size {
        return Err(format!("{what}: decoded size {} differs from {size}", out.len()));
    }
    Ok(out)
}

/// An LSPK package by its footer (40-byte header, its size, 'LSPK'): its part count and members. None for another
/// format. `part_len(i)` the length of part i (`<stem>_<i><ext>`; part 0 the package itself).
pub fn lspk_package(read: Read, size: u64, label: &str) -> R<Option<(usize, Vec<LspkEntry>)>> {
    if size < 8 {
        return Ok(None);
    }
    let f = read(size - 8, 8)?;
    if &f[4..8] != LSPK {
        return Ok(None);
    }
    let hsize = u32_at(&f, 0).unwrap() as u64;
    if hsize != 40 || size < hsize {
        return Err(format!("{label}: invalid LSPK header size {hsize}"));
    }
    let header = size - hsize;
    let h = read(header, 16)?;
    let (version, offset, stored, parts, flags) = (u32_at(&h, 0).unwrap(), u32_at(&h, 4).unwrap() as u64, u32_at(&h, 8).unwrap() as u64, u16_at(&h, 12).unwrap() as usize, h[14]);
    if version != 13 {
        return Err(format!("{label}: unsupported LSPK version {version}"));
    }
    if parts == 0 || flags & 4 != 0 {
        return Err(format!("{label}: invalid part count or unsupported solid LSPK"));
    }
    if offset > header || stored > header - offset || stored < 4 || stored > 256 << 20 {
        return Err(format!("{label}: invalid LSPK file table"));
    }
    let t = read(offset, stored)?;
    let count = u32_at(&t, 0).unwrap() as usize;
    if count > (256 << 20) / 280 {
        return Err(format!("{label}: LSPK file count {count} is too large"));
    }
    let table = lspk_decode(&t[4..], count * 280, 2, &format!("{label}: LSPK file table"))?;
    let mut out = vec![];
    for i in 0..count {
        let row = &table[i * 280..(i + 1) * 280];
        let end = memchr::memchr(0, &row[..256]).unwrap_or(0);
        if end == 0 {
            return Err(format!("{label}: LSPK member {i} has an invalid name"));
        }
        let name = String::from_utf8_lossy(&row[..end]).into_owned();
        let w: Vec<u32> = (0..6).map(|k| u32_at(row, 256 + 4 * k).unwrap()).collect();
        let (at, n, raw, part, comp) = (w[0] as u64, w[1] as u64, w[2] as u64, w[3] as usize, w[4]);
        if part >= parts {
            return Err(format!("{label}/{name}: invalid LSPK part {part}"));
        }
        if part == 0 && (at > offset || n > offset - at) {
            return Err(format!("{label}/{name}: range {at:#x}+{n:#x} exceeds {offset:#x} bytes"));
        }
        out.push((name, part, at, n, if comp & 15 != 0 { raw } else { n }, comp & 15));
    }
    Ok(Some((parts, out)))
}

pub fn lspk_read(read: Read, e: &LspkEntry, label: &str) -> R<Vec<u8>> {
    if e.3 > 256 << 20 {
        return Err(format!("{label}: stored size exceeds {} bytes", 256 << 20));
    }
    let d = read(e.2, e.3)?;
    if d.len() as u64 != e.3 {
        return Err(format!("{label}: truncated LSPK member"));
    }
    lspk_decode(&d, e.4 as usize, e.5, label)
}

/// The part file names of an LSPK package: `<stem>_<i><ext>` for parts after the first.
pub fn lspk_part(path: &str, i: usize) -> String {
    if i == 0 {
        return path.to_string();
    }
    match path.rfind('.').filter(|&d| d > path.rfind('/').map_or(0, |s| s + 1)) {
        Some(d) => format!("{}_{i}{}", &path[..d], &path[d..]),
        None => format!("{path}_{i}"),
    }
}

// ---- KIT (Unravel TWO) ----

pub const KIT: &[u8; 4] = b"kit!";

/// A KIT member: (record, part, offset, length there, decoded size, chained LZ4, SHA-1).
pub type KitEntry = (usize, u16, u64, u64, u64, bool, [u8; 20]);

pub fn kit_entries(d: &[u8], part_size: &dyn Fn(u16) -> Option<u64>, label: &str) -> R<Vec<KitEntry>> {
    if d.len() < 0x25 || &d[..4] != KIT {
        return Err(format!("{label}: incomplete KIT header"));
    }
    let base = 0x25 + u64_at(d, 0x10).unwrap() as usize;
    let count = u32_at(d, 0x18).unwrap() as usize;
    if base > d.len() || count > (d.len() - base) / 56 {
        return Err(format!("{label}: KIT directory is outside the index"));
    }
    let mut out = vec![];
    for i in 0..count {
        let o = base + 56 * i;
        let (flags, part, off, high, stored, size) = (u32_at(d, o).unwrap(), u16_at(d, o + 4).unwrap(), u32_at(d, o + 6).unwrap() as u64, u16_at(d, o + 10).unwrap(), u64_at(d, o + 12).unwrap(), u64_at(d, o + 20).unwrap());
        let sha: [u8; 20] = d[o + 28..o + 48].try_into().unwrap();
        if high != 0 {
            return Err(format!("{label}: KIT entry {i} has an unsupported high offset word"));
        }
        let psize = part_size(part).ok_or_else(|| format!("{label}: KIT entry {i} has no part {part}"))?;
        let n = if flags & 8 != 0 { stored } else { size };
        if off > psize || n > psize - off {
            return Err(format!("{label}: KIT entry {i} is outside part {part}"));
        }
        out.push((i, part, off, n, size, flags & 8 != 0, sha));
    }
    Ok(out)
}

pub fn kit_read(read: Read, e: &KitEntry, label: &str) -> R<Vec<u8>> {
    let d = read(e.2, e.3)?;
    if d.len() as u64 != e.3 {
        return Err(format!("{label}: truncated KIT member"));
    }
    let d = if e.5 { lz4_chained(&d, e.4 as usize, 0x10000, label)? } else { d };
    use sha1::Digest;
    if sha1::Sha1::digest(&d)[..] != e.6[..] {
        return Err(format!("{label}: KIT SHA1 mismatch"));
    }
    Ok(d)
}

// ---- SRYK (VBF, Virtuos) ----

/// A VBF member: (name, size, offset, block lengths).
pub type VbfEntry = (String, u64, u64, Vec<u16>);

pub fn vbf_entries(read: Read, label: &str) -> R<Vec<VbfEntry>> {
    let h0 = read(0, 16)?;
    if h0.len() != 16 || &h0[..4] != b"SRYK" {
        return Err(format!("{label}: not a VBF archive"));
    }
    let (hlen, count) = (u32_at(&h0, 4).unwrap() as u64, u64_at(&h0, 8).unwrap());
    if !(16 + count * 64 + 4 <= hlen && hlen <= 256 << 20) {
        return Err(format!("{label}: invalid VBF header length"));
    }
    let head = read(0, hlen)?;
    let count = count as usize;
    let strings = 16 + count * 64;
    let slen = u32_at(&head, strings).ok_or_else(|| format!("{label}: truncated VBF header"))? as usize;
    let blocks = strings + slen;
    if slen < 4 || blocks > hlen as usize {
        return Err(format!("{label}: invalid VBF string table"));
    }
    let mut out = vec![];
    for i in 0..count {
        let o = 16 + count * 32 + i * 32;
        let (bi, size, offset, name) = (u32_at(&head, o).unwrap() as usize, u64_at(&head, o + 8).unwrap(), u64_at(&head, o + 16).unwrap(), u64_at(&head, o + 24).unwrap() as usize);
        if offset == u64::MAX {
            continue;
        }
        let start = strings + 4 + name;
        if !(strings + 4 <= start && start < blocks) {
            return Err(format!("{label}: invalid VBF name offset"));
        }
        let end = memchr::memchr(0, &head[start..blocks]).map(|e| start + e).ok_or_else(|| format!("{label}: unterminated VBF name"))?;
        let n = size.div_ceil(65536) as usize;
        if blocks + (bi + n) * 2 > hlen as usize || offset < hlen {
            return Err(format!("{label}: invalid VBF block range"));
        }
        let lengths = (0..n).map(|k| u16_at(&head, blocks + (bi + k) * 2).unwrap()).collect();
        out.push((String::from_utf8_lossy(&head[start..end]).into_owned(), size, offset, lengths));
    }
    Ok(out)
}

pub fn vbf_read(read: Read, e: &VbfEntry, label: &str) -> R<Vec<u8>> {
    let (name, mut size, offset, lengths) = (&e.0, e.1, e.2, &e.3);
    let mut at = offset;
    let mut out = Vec::with_capacity(size as usize);
    for &l in lengths {
        let n = size.min(65536) as usize;
        let len = if l == 0 { 65536 } else { l as usize };
        let data = read(at, len as u64)?;
        at += len as u64;
        if data.len() != len {
            return Err(format!("{label}: {name}: truncated VBF block"));
        }
        let data = if len != n {
            let mut z = flate2::Decompress::new(true);
            let mut o = Vec::with_capacity(n + 1);
            match z.decompress_vec(&data, &mut o, flate2::FlushDecompress::Finish) {
                Ok(flate2::Status::StreamEnd) if o.len() == n => o,
                _ => lz4_flex::block::decompress(&data, n).map_err(|_| format!("{label}: {name}: invalid VBF compressed block (neither zlib nor LZ4)"))?,
            }
        } else {
            data
        };
        if data.len() != n {
            return Err(format!("{label}: {name}: VBF block size mismatch"));
        }
        out.extend(data);
        size -= n as u64;
    }
    Ok(out)
}

/// Whether a VBF member may hold shader programs (compiled effects and loose GPU programs).
pub fn vbf_wanted(name: &str) -> bool {
    name.contains(".fx") || name.contains("/shaders/") || name.ends_with(".vs.bin") || name.ends_with(".ps.bin") || name.ends_with(".fs.bin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpf_joaat_and_zip_and_narc() {
        assert_eq!(joaat("a"), 0xca2e9442);
        // a stored zip member
        let data = b"hello zip member".to_vec();
        let mut c = flate2::Crc::new();
        c.update(&data);
        let mut z = vec![];
        z.extend(b"PK\x03\x04");
        z.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend(c.sum().to_le_bytes());
        z.extend((data.len() as u32).to_le_bytes());
        z.extend((data.len() as u32).to_le_bytes());
        z.extend(1u16.to_le_bytes());
        z.extend(0u16.to_le_bytes());
        z.extend(b"a");
        z.extend(&data);
        let cd_off = z.len();
        z.extend(b"PK\x01\x02");
        z.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        z.extend(c.sum().to_le_bytes());
        z.extend((data.len() as u32).to_le_bytes());
        z.extend((data.len() as u32).to_le_bytes());
        z.extend(1u16.to_le_bytes());
        z.extend([0u8; 12]);
        z.extend(0u32.to_le_bytes());
        z.extend(b"a");
        let cd_size = z.len() - cd_off;
        z.extend(b"PK\x05\x06");
        z.extend([0u8; 4]);
        z.extend(1u16.to_le_bytes());
        z.extend(1u16.to_le_bytes());
        z.extend((cd_size as u32).to_le_bytes());
        z.extend((cd_off as u32).to_le_bytes());
        z.extend(0u16.to_le_bytes());
        let read = |o: u64, n: u64| Ok(z[o as usize..(o + n).min(z.len() as u64) as usize].to_vec());
        let e = zip_entries(&read, z.len() as u64, "t").unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(zip_read(&read, &e[0], "t").unwrap(), data);
        // NARC with one member, and an undo of kind 0x70 (stored after 0x10 bytes)
        let mut m = vec![0x70u8];
        m.extend([0u8; 15]);
        m.extend(b"BNSH");
        let mut narc = b"NARC".to_vec();
        narc.extend([0u8; 8]);
        narc.extend(16u16.to_le_bytes());
        narc.extend(2u16.to_le_bytes());
        narc.extend(b"BTAF");
        narc.extend(20u32.to_le_bytes());
        narc.extend(1u32.to_le_bytes());
        narc.extend(0u32.to_le_bytes());
        narc.extend((m.len() as u32).to_le_bytes());
        narc.extend(b"GMIF");
        narc.extend((8 + m.len() as u32).to_le_bytes());
        narc.extend(&m);
        let mem = narc_members(&narc, "t").unwrap();
        assert_eq!(cra_undo(mem[0], 0).unwrap(), b"BNSH");
    }

    #[test]
    fn casc_manifest_and_d3_streams() {
        let text = "## config-key = abc\nFileId!STRING:0|KeyLayout!DEC:4|Chunk!DEC:8|Archive!DEC:8|CKey!HEX:16|CSize!DEC:4|EKey!HEX:16|ESize!DEC:4|Encoding!STRING:0|Path!STRING:0|Flags!STRING:0\n\
f1|0|1|2|00|5|00000000000000000001020000001000|5|n|x|\n";
        let e = casc_files("abc\n", &[("build_manifest.psv".into(), text.into())], "data").unwrap();
        assert_eq!(e[0].1, "01-00000002.data");
        assert_eq!(e[0].2, 0x1000);
        // a D3 stream record
        let x = b"payload payload payload".to_vec();
        let mut zz = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut zz, &x).unwrap();
        let s = zz.finish().unwrap();
        let mut d = vec![0u8; 0x48];
        d.extend((x.len() as u32).to_le_bytes());
        d.extend((s.len() as u16).to_le_bytes());
        d.extend(&s);
        let st = d3_streams(&d, 0, d.len(), 0);
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].2, x);
    }
}
