//! Unity: UnityFS bundles (stored or LZ4 blocks) and serialized files (`*.assets`, `data.unity3d`'s members).
//! The Switch programs of a Shader object sit in its `compressedBlob` (LZ4 segments), found here by the shape of the
//! fields serialized right before it, so no type tree and no common string table is needed:
//! `platforms` (vector<u32>), `offsets`, `compressedLengths`, `decompressedLengths` (vector<vector<u32>> from 2019.3,
//! vector<u32> before), then the blob. A decoded segment lists its subprograms (u32 count, then {offset, length,
//! segment}, or {offset, length} before 2019.3); a subprogram holds its program code: from 2019.3 a 0x60 header of
//! four i32[6] arrays indexed by NVN stage (vertex, pixel, geometry, hull, domain, compute: unused, block offset from
//! +0x60 or -1, control length, code length) and one block per stage (control section + code area); before 2019.3 a
//! 0x10 header {?, ?, control length, code length} and one stage, its stage read from the code. Program code stored
//! uncompressed (ComputeShader kernels) is found by its length in front; GLSLC outputs in TextAssets are read too.

use super::glslc;
use crate::nvn::*;

pub const UNITYFS: &[u8; 8] = b"UnityFS\0";
const NV_STAGES: [Stage; 6] = [Stage::Vs, Stage::Ps, Stage::Gs, Stage::Hs, Stage::Ds, Stage::Cs];

fn be32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()))
}
fn be64(d: &[u8], o: usize) -> Option<u64> {
    d.get(o..o + 8).map(|b| u64::from_be_bytes(b.try_into().unwrap()))
}

/// A serialized file: big-endian header, format 17..22 (from 22 the sizes are 64-bit at +0x14).
pub fn is_serialized(d: &[u8]) -> bool {
    if d.len() < 48 {
        return false;
    }
    let ver = be32(d, 8).unwrap();
    if !(17..=22).contains(&ver) || d[16] > 1 {
        return false;
    }
    let (msz, fsz, doff) = if ver >= 22 { (be32(d, 20).unwrap() as u64, be64(d, 24).unwrap(), be64(d, 32).unwrap()) } else { (be32(d, 0).unwrap() as u64, be32(d, 4).unwrap() as u64, be32(d, 12).unwrap() as u64) };
    fsz == d.len() as u64 && 0 < msz && msz < fsz && doff <= fsz
}

fn cstr(d: &[u8], p: usize, wh: &str) -> R<(String, usize)> {
    let s = d.get(p..).ok_or_else(|| format!("{wh}: string at {p:#x} past the end"))?;
    let n = memchr::memchr(0, s).ok_or_else(|| format!("{wh}: unterminated string at {p:#x}"))?;
    Ok((String::from_utf8_lossy(&s[..n]).into_owned(), p + n + 1))
}

fn decomp(raw: &[u8], n: usize, kind: u32, wh: &str) -> R<Vec<u8>> {
    match kind {
        0 => Ok(raw.to_vec()),
        2 | 3 => {
            if n > 255 * raw.len().max(1) || n as u64 > super::archive::DECODE_LIMIT {
                return Err(format!("{wh}: an LZ4 block of {:#x} bytes cannot hold {n:#x}", raw.len()));
            }
            let d = lz4_flex::block::decompress(raw, n).map_err(|e| format!("{wh}: LZ4: {e}"))?;
            if d.len() != n {
                return Err(format!("{wh}: LZ4 block gives {:#x} bytes, not {n:#x}", d.len()));
            }
            Ok(d)
        }
        1 => {
            // Unity's LZMA: the 5 property bytes, then the raw stream (no size in it)
            if n as u64 > super::archive::DECODE_LIMIT {
                return Err(format!("{wh}: an LZMA block of {n:#x} bytes"));
            }
            let opts = lzma_rs::decompress::Options { unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(n as u64)), memlimit: None, allow_incomplete: false };
            let mut out = Vec::with_capacity(n);
            lzma_rs::lzma_decompress_with_options(&mut std::io::BufReader::new(raw), &mut out, &opts).map_err(|e| format!("{wh}: LZMA: {e:?}"))?;
            if out.len() != n {
                return Err(format!("{wh}: LZMA block gives {:#x} bytes, not {n:#x}", out.len()));
            }
            Ok(out)
        }
        k => Err(format!("{wh}: unknown compression {k}")),
    }
}

/// A UnityFS bundle's files: (path, bytes).
pub fn bundle_files(d: &[u8], label: &str) -> R<Vec<(String, Vec<u8>)>> {
    if d.get(..8) != Some(&UNITYFS[..]) {
        return Err(format!("{label}: not a UnityFS bundle"));
    }
    let wh = format!("{label}: UnityFS");
    let fmt = be32(d, 8).ok_or_else(|| format!("{wh}: header"))?;
    let (_, p) = cstr(d, 12, &wh)?;
    let (_, mut p) = cstr(d, p, &wh)?;
    let h = span(d, p as u64, 20, &format!("{wh} header"))?;
    let (csz, usz, flags) = (be32(h, 8).unwrap() as usize, be32(h, 12).unwrap() as usize, be32(h, 16).unwrap());
    p += 20;
    if fmt >= 7 {
        p = (p + 15) & !15;
    }
    let info_raw = if flags & 0x80 != 0 {
        span(d, (d.len() as u64).saturating_sub(csz as u64), csz as u64, &format!("{wh} block info"))?
    } else {
        let s = span(d, p as u64, csz as u64, &format!("{wh} block info"))?;
        p += csz;
        s
    };
    let info = decomp(info_raw, usz, flags & 0x3f, &format!("{wh} block info"))?;
    let bad = || format!("{wh}: block info is cut short");
    let nb = be32(&info, 16).ok_or_else(bad)? as usize;
    let mut q = 20;
    let mut blocks = vec![];
    for _ in 0..nb {
        let b = info.get(q..q + 10).ok_or_else(bad)?;
        blocks.push((be32(b, 0).unwrap() as usize, be32(b, 4).unwrap() as usize, u16::from_be_bytes([b[8], b[9]]) as u32));
        q += 10;
    }
    let nn = be32(&info, q).ok_or_else(bad)? as usize;
    q += 4;
    let mut nodes = vec![];
    for _ in 0..nn {
        let b = info.get(q..q + 20).ok_or_else(bad)?;
        let (off, sz) = (be64(b, 0).unwrap(), be64(b, 8).unwrap());
        let (name, nq) = cstr(&info, q + 20, &wh)?;
        q = nq;
        nodes.push((off, sz, name));
    }
    if flags & 0x200 != 0 {
        p = (p + 15) & !15;
    }
    let total: u64 = blocks.iter().map(|b| b.0 as u64).sum();
    if total > super::archive::DECODE_LIMIT {
        return Err(format!("{wh}: blocks of {total:#x} bytes"));
    }
    let mut out = Vec::with_capacity(total as usize);
    for (i, &(u, c, f)) in blocks.iter().enumerate() {
        let raw = span(d, p as u64, c as u64, &format!("{wh} block {i}"))?;
        out.extend(decomp(raw, u, f & 0x3f, &format!("{wh} block {i}"))?);
        p += c;
    }
    nodes
        .into_iter()
        .map(|(off, sz, name)| Ok((name.clone(), span(&out, off, sz, &format!("{wh} file {name}"))?.to_vec())))
        .collect()
}

/// Program code → its stages (stage, control section, code area); None when the bytes are no program code.
pub fn program(code: &[u8]) -> Option<Vec<(Stage, &[u8], &[u8])>> {
    if code.len() >= 0x14 && code[0x10..0x14] == CTL_MAGIC {
        let (nc, nk) = (i32_at(code, 8)?, i32_at(code, 12)?);
        if nc < 0 || nk < 0 || 0x10 + nc as usize + nk as usize != code.len() {
            return None;
        }
        let (ctl, cd) = (&code[0x10..0x10 + nc as usize], &code[0x10 + nc as usize..]);
        let st = glslc::stage(ctl, cd)?;
        return check(st, ctl, cd).is_none().then(|| vec![(st, ctl, cd)]);
    }
    if code.len() < 0x60 {
        return None;
    }
    let h: Vec<i32> = (0..24).map(|i| i32_at(code, 4 * i).unwrap()).collect();
    let mut out = vec![];
    let mut end = 0x60;
    for (i, &st) in NV_STAGES.iter().enumerate() {
        let (off, nc, nk) = (h[6 + i], h[12 + i], h[18 + i]);
        if off == -1 {
            continue;
        }
        if off < 0 || nc < 0 || nk < 0 {
            return None;
        }
        let a = 0x60 + off as usize;
        let e = a + nc as usize + nk as usize;
        if e > code.len() || code.get(a..a + 4) != Some(&CTL_MAGIC[..]) {
            return None;
        }
        let (ctl, cd) = (&code[a..a + nc as usize], &code[a + nc as usize..e]);
        check_ok(st, ctl, cd)?;
        out.push((st, ctl, cd));
        end = end.max(e);
    }
    (!out.is_empty() && end == code.len()).then_some(out)
}

fn check_ok(st: Stage, ctl: &[u8], cd: &[u8]) -> Option<()> {
    check(st, ctl, cd).is_none().then_some(())
}

/// The program code of a subprogram: version, program type, 12 bytes of statistics, (from 201608170) 4 more, the
/// keywords, (201806140 to 202012090) the local keywords, then the code (i32 length + bytes).
fn subprogram_code(b: &[u8]) -> Option<&[u8]> {
    let ver = u32_at(b, 0)?;
    let mut p = 8 + 12 + if ver >= 201608170 { 4 } else { 0 };
    let strs = |p: &mut usize| -> Option<()> {
        let n = u32_at(b, *p)?;
        if n > 1 << 16 {
            return None;
        }
        *p += 4;
        for _ in 0..n {
            let ln = u32_at(b, *p)? as usize;
            *p = (*p + 4 + ln + 3) & !3;
            if *p > b.len() {
                return None;
            }
        }
        Some(())
    };
    strs(&mut p)?;
    if (201806140..202012090).contains(&ver) {
        strs(&mut p)?;
    }
    let n = u32_at(b, p)? as usize;
    b.get(p + 4..p + 4 + n)
}

/// A Shader object's blob at `p` (its `platforms` count): the decoded segments of each platform, and where the
/// blob ends.
fn blob_at(x: &[u8], p: usize) -> Option<(Vec<(bool, Vec<Vec<u8>>)>, usize)> {
    let np = u32_at(x, p)? as usize;
    if !(1..=16).contains(&np) {
        return None;
    }
    for i in 0..np {
        if u32_at(x, p + 4 + 4 * i)? > 64 {
            return None;
        }
    }
    let mut q = p + 4 + 4 * np;
    if u32_at(x, q)? as usize != np {
        return None;
    }
    // nested (2019.3 and later): three vector<vector<u32>>; flat (before): three vector<u32>
    let nested = |mut q: usize| -> Option<(Vec<[Vec<u32>; 3]>, usize)> {
        let mut arrs: [Vec<Vec<u32>>; 3] = Default::default();
        for (k, arr) in arrs.iter_mut().enumerate() {
            if k > 0 {
                if u32_at(x, q)? as usize != np {
                    return None;
                }
            }
            q += 4;
            for _ in 0..np {
                let n = u32_at(x, q)? as usize;
                if !(1..=256).contains(&n) {
                    return None;
                }
                arr.push((0..n).map(|j| u32_at(x, q + 4 + 4 * j)).collect::<Option<_>>()?);
                q += 4 + 4 * n;
            }
        }
        let [o, c, u] = arrs;
        let per: Vec<[Vec<u32>; 3]> = o.into_iter().zip(c).zip(u).map(|((o, c), u)| [o, c, u]).collect();
        per.iter().all(|a| a[0].len() == a[1].len() && a[1].len() == a[2].len()).then_some((per, q))
    };
    let flat = |mut q: usize| -> Option<(Vec<[Vec<u32>; 3]>, usize)> {
        let mut arrs: [Vec<u32>; 3] = Default::default();
        for (k, arr) in arrs.iter_mut().enumerate() {
            if k > 0 && u32_at(x, q)? as usize != np {
                return None;
            }
            *arr = (0..np).map(|j| u32_at(x, q + 4 + 4 * j)).collect::<Option<_>>()?;
            q += 4 + 4 * np;
        }
        Some(((0..np).map(|i| [vec![arrs[0][i]], vec![arrs[1][i]], vec![arrs[2][i]]]).collect(), q))
    };
    let (per, q2, is_nested) = match nested(q) {
        Some((per, q2)) => (per, q2, true),
        None => {
            let (per, q2) = flat(q)?;
            (per, q2, false)
        }
    };
    q = q2;
    let nb = u32_at(x, q)? as usize;
    let blob = x.get(q + 4..q + 4 + nb)?;
    let mut out = vec![];
    for a in &per {
        let mut segs = vec![];
        for s in 0..a[0].len() {
            let (o, c, u) = (a[0][s] as usize, a[1][s] as usize, a[2][s] as usize);
            if c == 0 || u < 4 || u > 255 * c || u > 1 << 30 || o + c > nb {
                return None;
            }
            let d = lz4_flex::block::decompress(&blob[o..o + c], u).ok()?;
            if d.len() != u {
                return None;
            }
            segs.push(d);
        }
        out.push((is_nested, segs));
    }
    Some((out, q + 4 + nb))
}

/// The shaders of one serialized file (or any bytes holding Unity objects).
pub fn serialized(x: &[u8], label: &str) -> Vec<Found> {
    let mut out = vec![];
    let mut add = |where_: String, progs: Vec<(Stage, &[u8], &[u8])>| {
        for (st, ctl, cd) in progs {
            out.push(Found::new(format!("{where_}#{}", st.name()), st, ctl, cd).grouped(where_.clone()));
        }
    };
    // Shader objects' compressed blobs
    let mut p = 0;
    while p + 8 <= x.len() {
        let Some((plats, end)) = blob_at(x, p) else {
            p += 4;
            continue;
        };
        for (pi, (nested, segs)) in plats.iter().enumerate() {
            let s0 = &segs[0];
            let n = u32_at(s0, 0).unwrap_or(0) as usize;
            let w = if *nested { 12 } else { 8 };
            for k in 0..n.min((s0.len().saturating_sub(4)) / w) {
                let (off, ln) = (u32_at(s0, 4 + w * k).unwrap() as usize, u32_at(s0, 8 + w * k).unwrap() as usize);
                let seg = if *nested { u32_at(s0, 12 + w * k).unwrap() as usize } else { 0 };
                let Some(sp) = segs.get(seg).and_then(|s| s.get(off..off + ln)) else { continue };
                // Unity 2022 also keeps parameter entries in the blob: no program code, left out
                if let Some(progs) = subprogram_code(sp).and_then(program) {
                    add(format!("{label}#UNITY@{p:#x}.{pi}.{k}"), progs);
                }
            }
        }
        p = (end + 3) & !3;
    }
    // program code stored as it is (ComputeShader kernels): its length right before a 0x60 header whose first
    // block's control section follows it, or before a 0x10 header
    for c in memchr::memmem::find_iter(x, &CTL_MAGIC) {
        for back in [0x64usize, 0x14] {
            let Some(at) = c.checked_sub(back) else { continue };
            let Some(n) = u32_at(x, at) else { continue };
            let Some(code) = x.get(at + 4..at + 4 + n as usize) else { continue };
            if let Some(progs) = program(code) {
                add(format!("{label}#UNITY@{at:#x}"), progs);
                break;
            }
        }
    }
    // a game's own GLSLC outputs in a TextAsset
    if memchr::memmem::find(x, &glslc::MAGIC).is_some() {
        out.extend(glslc::shaders(x, label));
    }
    out
}

/// A UnityFS bundle (each serialized file in it) or a serialized file.
pub fn unity(d: &[u8], label: &str, out: &mut super::Scanned) {
    if d.get(..8) == Some(&UNITYFS[..]) {
        match bundle_files(d, label) {
            Ok(files) => {
                for (name, x) in files {
                    out.found.extend(serialized(&x, &format!("{label}:{name}")));
                }
            }
            Err(e) => out.problems.push(e),
        }
        return;
    }
    out.found.extend(serialized(d, label));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    fn program_code(stages: &[(usize, &(Vec<u8>, Vec<u8>))]) -> Vec<u8> {
        let mut h = [0i32; 24];
        for i in 0..6 {
            h[6 + i] = -1;
        }
        let mut body = vec![];
        for &(i, (c, k)) in stages {
            h[6 + i] = body.len() as i32;
            h[12 + i] = c.len() as i32;
            h[18 + i] = k.len() as i32;
            body.extend_from_slice(c);
            body.extend_from_slice(k);
        }
        let mut v: Vec<u8> = h.iter().flat_map(|x| x.to_le_bytes()).collect();
        v.extend(body);
        v
    }

    fn u32s(v: &mut Vec<u8>, xs: &[u32]) {
        for x in xs {
            v.extend_from_slice(&x.to_le_bytes());
        }
    }

    #[test]
    fn lzma_block() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut lz = vec![];
        lzma_rs::lzma_compress(&mut std::io::Cursor::new(&data), &mut lz).unwrap();
        // .lzma: 5 property bytes, u64 size, the stream; Unity's leaves the size out
        let mut unity = lz[..5].to_vec();
        unity.extend_from_slice(&lz[13..]);
        assert_eq!(decomp(&unity, data.len(), 1, "t").unwrap(), data);
    }

    #[test]
    fn shader_blob_and_bundle() {
        let (vs, ps, cs) = (nvn1_vs(3), nvn1_ps(4), nvn1_ps(7));
        let code = program_code(&[(0, &vs), (1, &ps)]);
        // a subprogram (version 201806140: keywords and local keywords, both empty)
        let mut sp = vec![];
        u32s(&mut sp, &[201806140, 0, 0, 0, 0, 0, 0, 0, code.len() as u32]);
        sp.extend_from_slice(&code);
        let mut seg = vec![];
        u32s(&mut seg, &[1, 16, sp.len() as u32, 0]);
        seg.extend_from_slice(&sp);
        let comp = lz4_flex::block::compress(&seg);
        // platforms, offsets, compressedLengths, decompressedLengths (nested), the blob
        let mut x = vec![0u8; 0x20];
        u32s(&mut x, &[1, 25, 1, 1, 0, 1, 1, comp.len() as u32, 1, 1, seg.len() as u32, comp.len() as u32]);
        x.extend_from_slice(&comp);
        while x.len() % 4 != 0 {
            x.push(0);
        }
        let f = serialized(&x, "/Data/sharedassets0.assets");
        assert_eq!(f.len(), 2, "{:?}", f.iter().map(|f| &f.label).collect::<Vec<_>>());
        assert_eq!((f[0].stage, f[1].stage), (Stage::Vs, Stage::Ps));
        assert_eq!(f[1].key(), skey(&ps.0, &ps.1));
        assert_eq!(f[0].group, f[1].group);
        // program code stored as it is, its length in front (as a ComputeShader kernel's)
        let k = program_code(&[(1, &cs)]);
        let mut y = x.clone();
        u32s(&mut y, &[k.len() as u32]);
        y.extend_from_slice(&k);
        assert_eq!(serialized(&y, "/x").len(), 3);
        // the same file in a UnityFS bundle, one LZ4 block
        let blk = lz4_flex::block::compress(&y);
        let mut info = vec![0u8; 16];
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&(y.len() as u32).to_be_bytes());
        info.extend_from_slice(&(blk.len() as u32).to_be_bytes());
        info.extend_from_slice(&3u16.to_be_bytes());
        info.extend_from_slice(&1u32.to_be_bytes());
        info.extend_from_slice(&0u64.to_be_bytes());
        info.extend_from_slice(&(y.len() as u64).to_be_bytes());
        info.extend_from_slice(&4u32.to_be_bytes());
        info.extend_from_slice(b"CAB-1\0");
        let mut b = UNITYFS.to_vec();
        b.extend_from_slice(&6u32.to_be_bytes());
        b.extend_from_slice(b"5.x.x\x002019.4.0f1\0");
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes());
        b.extend_from_slice(&(info.len() as u32).to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&info);
        b.extend_from_slice(&blk);
        let mut s = super::super::Scanned::default();
        unity(&b, "/Data/a.bundle", &mut s);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.found.len(), 3);
        assert!(s.found[0].label.starts_with("/Data/a.bundle:CAB-1#UNITY@"));
    }
}
