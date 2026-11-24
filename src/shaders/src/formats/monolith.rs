//! Monolith Soft (Xenoblade Chronicles 3, Definitive Edition, X Definitive Edition): ARH1 / ARH2 indexes of an `.ard`
//! data file; entries and their XBC1 streams (stored, zlib, zstd) and Firehawk `cmp\0zlib` resources hold HCPS shader
//! programs (SLCT selectors of NVSD variants over control and code sections), standalone SLCTs in main, and whole
//! GLSLC outputs.

use crate::formats::{archive, glslc};
use crate::nvn::*;

pub fn is_arh(d: &[u8]) -> bool {
    matches!(d.get(..4), Some(b"arh1") | Some(b"arh2"))
}

/// An ARH1 / ARH2 index → (path, ARD offset, stored size).
pub fn arh_entries(d: &[u8], label: &str) -> R<Vec<(String, u64, u64)>> {
    if d.starts_with(b"arh2") {
        // paths replaced with hashes: (u64 hash, u32 disk size, u32 expanded size); offsets accumulate aligned sizes
        if d.len() < 16 {
            return Err(format!("{label}: ARH2: truncated header"));
        }
        let (count, align) = (u32_at(d, 4).unwrap() as usize, u32_at(d, 8).unwrap() as u64);
        if align == 0 || align & (align - 1) != 0 {
            return Err(format!("{label}: ARH2: invalid file alignment"));
        }
        if count > (d.len() - 16) / 16 {
            return Err(format!("{label}: ARH2: index table out of bounds"));
        }
        let mut out = vec![];
        let mut off = 0u64;
        for i in 0..count {
            let (key, size) = (
                u64_at(d, 16 + i * 16).unwrap(),
                u32_at(d, 24 + i * 16).unwrap() as u64,
            );
            if size != 0 {
                out.push((format!("/#{key:016x}"), off, size));
            }
            off += (size + align - 1) & !(align - 1);
        }
        return Ok(out);
    }
    if d.len() < 48 || !d.starts_with(b"arh1") {
        return Ok(vec![]);
    }
    let w: Vec<u32> = (0..9).map(|k| u32_at(d, 4 + 4 * k).unwrap()).collect();
    let (count, so, sn, no, nn, fo, fnn, key) = (
        w[1] as usize,
        w[2] as usize,
        w[3] as usize,
        w[4] as usize,
        w[5] as usize,
        w[6] as usize,
        w[7] as usize,
        w[8],
    );
    let oob = |o: usize, n: usize| o > d.len() || n > d.len() - o;
    if nn != count * 8 || oob(so, sn) || oob(no, nn) || oob(fo, fnn * 24) {
        return Err(format!("{label}: ARH1: index table out of bounds"));
    }
    let key = (key ^ 0xf3f3_5353).to_le_bytes();
    // whole u32 words are encrypted; a final partial word stays plain
    let dec = |o: usize, n: usize| -> Vec<u8> {
        d[o..o + n]
            .iter()
            .enumerate()
            .map(|(i, &v)| if i < n & !3 { v ^ key[i & 3] } else { v })
            .collect()
    };
    let strings = dec(so, sn);
    let nb = dec(no, nn);
    let nodes: Vec<(i32, i32)> = nb
        .chunks_exact(8)
        .map(|c| {
            (
                i32::from_le_bytes(c[..4].try_into().unwrap()),
                i32::from_le_bytes(c[4..].try_into().unwrap()),
            )
        })
        .collect();
    let mut names: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    for (i, &(nxt, prev)) in nodes.iter().enumerate() {
        if nxt >= 0 || prev < 0 {
            continue;
        }
        let off = (-(nxt as i64)) as usize;
        let end = strings
            .get(off..)
            .and_then(|s| memchr::memchr(0, s))
            .map(|e| e + off);
        let Some(end) = end.filter(|&e| e + 5 <= strings.len() && strings[off..e].is_ascii())
        else {
            return Err(format!("{label}: ARH1: invalid path suffix"));
        };
        let fid = u32_at(&strings, end + 1).unwrap();
        let mut prefix = vec![];
        let mut cur = i;
        let mut seen = std::collections::HashSet::new();
        while cur != 0 {
            if !seen.insert(cur) || cur >= nodes.len() {
                return Err(format!("{label}: ARH1: invalid path trie"));
            }
            let parent = nodes[cur].1;
            if parent < 0 || parent as usize >= nodes.len() {
                return Err(format!("{label}: ARH1: invalid path parent"));
            }
            let ch = cur as i64 ^ nodes[parent as usize].0 as i64;
            if !(0..128).contains(&ch) {
                return Err(format!("{label}: ARH1: invalid path character"));
            }
            prefix.push(ch as u8);
            cur = parent as usize;
        }
        prefix.reverse();
        prefix.extend(&strings[off..end]);
        names.insert(fid, String::from_utf8_lossy(&prefix).into_owned());
    }
    let mut out = vec![];
    for i in 0..fnn {
        let o = fo + i * 24;
        let (off, size, flags, fid) = (
            u64_at(d, o).unwrap(),
            u32_at(d, o + 8).unwrap() as u64,
            u32_at(d, o + 16).unwrap(),
            u32_at(d, o + 20).unwrap(),
        );
        // HasXbc1Header (bit 1): the stored size leaves out the 48-byte wrapper
        if size != 0 {
            out.push((
                names
                    .get(&fid)
                    .cloned()
                    .unwrap_or_else(|| format!("#{fid}")),
                off,
                size + if flags & 2 != 0 { 48 } else { 0 },
            ));
        }
    }
    Ok(out)
}

/// An XBC1 stream at `off`: 48-byte header, stored / zlib / zstd payload. → (decoded, end), None for bytes that only
/// look like a header.
fn xbc(d: &[u8], off: usize, label: &str) -> R<Option<(Vec<u8>, usize)>> {
    if off + 48 > d.len() || &d[off..off + 4] != b"xbc1" {
        return Ok(None);
    }
    let (kind, raw, size) = (
        u32_at(d, off + 4).unwrap(),
        u32_at(d, off + 8).unwrap() as usize,
        u32_at(d, off + 12).unwrap() as usize,
    );
    if !matches!(kind, 0 | 1 | 3) || size == 0 || off + 48 + size > d.len() {
        return Ok(None);
    }
    let p = &d[off + 48..off + 48 + size];
    let data = match kind {
        1 => archive::unzlib(p, label)?,
        3 => {
            let mut v = vec![];
            let mut r = zstd::stream::read::Decoder::new(p).map_err(|e| format!("{label}: {e}"))?;
            std::io::Read::read_to_end(&mut r, &mut v)
                .map_err(|e| format!("{label}: XBC1 @{off:#x}: {e}"))?;
            v
        }
        _ => p.to_vec(),
    };
    if data.len() != raw {
        return Err(format!(
            "{label}: XBC1 @{off:#x}: expected {raw} bytes, decoded {}",
            data.len()
        ));
    }
    Ok(Some((data, off + 48 + size)))
}

/// A Firehawk `cmp\0zlib` resource at `off`: stored / raw sizes, a zlib payload.
fn cmp_zlib(d: &[u8], off: usize, label: &str) -> R<Option<(Vec<u8>, usize)>> {
    if off + 16 > d.len() || &d[off..off + 8] != b"cmp\0zlib" {
        return Ok(None);
    }
    let (size, raw) = (
        u32_at(d, off + 8).unwrap() as usize,
        u32_at(d, off + 12).unwrap() as usize,
    );
    if size == 0 || size > d.len() - off - 16 {
        return Ok(None);
    }
    let p = &d[off + 16..off + 16 + size];
    let mut z = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(raw);
    let st = z
        .decompress_vec(p, &mut out, flate2::FlushDecompress::Finish)
        .map_err(|e| format!("{label}: cmp zlib @{off:#x}: damaged payload ({e})"))?;
    if st != flate2::Status::StreamEnd || z.total_in() as usize != size || out.len() != raw {
        return Err(format!(
            "{label}: cmp zlib @{off:#x}: payload does not match stored/raw sizes"
        ));
    }
    Ok(Some((out, off + 16 + size)))
}

/// HCPS shader programs at `base`: SLCT selectors give control / code ranges and NVSD variants; each NVSD's last
/// stage record gives the sizes. A variant's stages pair.
pub fn hcps(d: &[u8], base: usize, label: &str) -> R<Vec<Found>> {
    let at = format!("{label} HCPS @{base:#x}");
    let sp = |off: usize, n: usize| -> R<&[u8]> {
        if off < base || off > d.len() || n > d.len() - off {
            return Err(format!("{at}: section out of bounds"));
        }
        Ok(&d[off..off + n])
    };
    sp(base, 72)?;
    let (table, count) = (
        u32_at(d, base + 8).unwrap() as usize,
        u32_at(d, base + 12).unwrap() as usize,
    );
    let w: Vec<usize> = (0..6)
        .map(|k| u32_at(d, base + 24 + 4 * k).unwrap() as usize)
        .collect();
    sp(base + table, count * 8)?;
    let slcts = sp(base + w[0], w[1])?;
    let codes = sp(base + w[2], w[3])?;
    let ctls = sp(base + w[4], w[5])?;
    if w[3] == 0 && w[5] == 0 {
        return Ok(vec![]); // source-only GLSL programs
    }
    let mut out = vec![];
    for i in 0..count {
        let s = u32_at(d, base + table + 8 * i).unwrap() as usize;
        if s + 72 > slcts.len() || &slcts[s..s + 4] != b"SLCT" {
            return Err(format!("{at}: invalid SLCT {i}"));
        }
        let (num, ptr) = (
            u32_at(slcts, s + 16).unwrap() as usize,
            u32_at(slcts, s + 20).unwrap() as usize,
        );
        let (mut c, cn, mut k, kn) = (
            u32_at(slcts, s + 40).unwrap() as usize,
            u32_at(slcts, s + 44).unwrap() as usize,
            u32_at(slcts, s + 48).unwrap() as usize,
            u32_at(slcts, s + 52).unwrap() as usize,
        );
        let (ce, ke) = (c + cn, k + kn);
        if s + ptr + num * 8 > slcts.len() || ce > ctls.len() || ke > codes.len() {
            return Err(format!("{at} SLCT {i}: variant range out of bounds"));
        }
        for j in 0..num {
            let (po, pn) = (
                u32_at(slcts, s + ptr + j * 8).unwrap() as usize,
                u32_at(slcts, s + ptr + j * 8 + 4).unwrap() as usize,
            );
            let p = s + po;
            if pn < 32 || p + pn > slcts.len() {
                return Err(format!("{at}: invalid NVSD"));
            }
            let (vo, vn) = (
                u32_at(slcts, p + 24).unwrap() as usize,
                u32_at(slcts, p + 28).unwrap() as usize,
            );
            if vn == 0 || vo + vn * 20 > pn {
                return Err(format!("{at}: invalid NVSD stage records"));
            }
            let r = p + vo + (vn - 1) * 20;
            let (vs, ps, vc, pc) = (
                u32_at(slcts, r + 4).unwrap() as usize,
                u32_at(slcts, r + 8).unwrap() as usize,
                u32_at(slcts, r + 12).unwrap() as usize,
                u32_at(slcts, r + 16).unwrap() as usize,
            );
            let (pend, kend) = (c + vc + pc, k + vs + ps);
            if pend > ce || kend > ke {
                return Err(format!("{at}: program exceeds SLCT range"));
            }
            // a vertex bundle may hold a geometry stage too; each control gives its own code size, and one asset may
            // hold Maxwell and NVN2 programs
            while c < pend {
                if c + NVN2_KEY_CTL > pend {
                    return Err(format!("{at}: truncated control"));
                }
                let n2 = nvn2_ctl(&ctls[c..c + NVN2_KEY_CTL]);
                if !n2 && c + 0x708 > pend {
                    return Err(format!("{at}: truncated control"));
                }
                let nc = u32_at(ctls, c + if n2 { 0x30 } else { 0x20 }).unwrap() as usize;
                let nk = u32_at(ctls, c + if n2 { 0x44 } else { 0x704 }).unwrap() as usize;
                if nc < if n2 { NVN2_KEY_CTL } else { 0x760 }
                    || c + nc > pend
                    || nk == 0
                    || k + nk > kend
                {
                    return Err(format!("{at}: shader exceeds program range"));
                }
                let (ctl, code) = (&ctls[c..c + nc], &codes[k..k + nk]);
                c += (nc + 7) & !7;
                k += nk;
                let st = if n2 {
                    nvn2_stage(ctl, code)
                } else {
                    glslc::stage(ctl, code)
                };
                let why = match st {
                    Some(st) => check(st, ctl, code),
                    None => Some("unknown stage"),
                };
                if let Some(w) = why {
                    return Err(format!("{at} SLCT {i} variant {j}: {w}"));
                }
                let tag = format!("{label}#HCPS@{base:#x}#var{i}.{j}");
                out.push(Found::new(tag.clone(), st.unwrap(), ctl, code).grouped(tag));
            }
            if k != kend {
                return Err(format!("{at}: unused program code bytes"));
            }
        }
        if c != ce || k != ke {
            return Err(format!("{at} SLCT {i}: unused control/code bytes"));
        }
    }
    Ok(out)
}

/// A standalone SLCT (in main): its control / code offsets relative to itself, read as a one-selector HCPS.
pub fn slct(d: &[u8], base: usize, label: &str) -> R<Vec<Found>> {
    if base + 72 > d.len() {
        return Ok(vec![]);
    }
    let (c, cn, k, kn) = (
        u32_at(d, base + 40).unwrap() as usize,
        u32_at(d, base + 44).unwrap() as usize,
        u32_at(d, base + 48).unwrap() as usize,
        u32_at(d, base + 52).unwrap() as usize,
    );
    let size = (c + cn).max(k + kn);
    if c.min(k) < 72 || size > d.len() - base {
        return Ok(vec![]);
    }
    let mut h = vec![0u8; 80];
    h[..4].copy_from_slice(b"HCPS");
    h[8..12].copy_from_slice(&72u32.to_le_bytes());
    h[12..16].copy_from_slice(&1u32.to_le_bytes());
    for (q, v) in [80u32, size as u32, 80, size as u32, 80, size as u32]
        .iter()
        .enumerate()
    {
        h[24 + 4 * q..28 + 4 * q].copy_from_slice(&v.to_le_bytes());
    }
    h.extend(&d[base..base + size]);
    hcps(&h, 0, &format!("{label}#SLCT@{base:#x}"))
}

/// Standalone SLCTs (main): every `SLCT\x03\x10\0\0`.
pub fn slcts(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for p in memchr::memmem::find_iter(d, b"SLCT\x03\x10\0\0") {
        out.extend(slct(d, p, label)?);
    }
    Ok(out)
}

/// Whether `d` starts as a Monolith container this reader takes whole.
pub fn is_monolith(d: &[u8]) -> bool {
    matches!(d.get(..4), Some(b"HCPS") | Some(b"xbc1") | Some(b"DRSM"))
        || d.starts_with(b"cmp\0zlib")
}

/// Decoded entries and their XBC1 / cmp-zlib streams, read without relying on extensions.
pub fn shaders(d: &[u8], label: &str, out: &mut Vec<Found>, depth: usize) -> R<()> {
    if depth > crate::formats::NEST {
        return Err(format!(
            "{label}: streams nested more than {} deep",
            crate::formats::NEST
        ));
    }
    let mut markers: Vec<(usize, bool, usize)> = memchr::memmem::find_iter(d, b"xbc1")
        .map(|o| (o, true, 4))
        .chain(memchr::memmem::find_iter(d, b"cmp\0zlib").map(|o| (o, false, 8)))
        .collect();
    markers.sort();
    let mut pos = 0;
    let mut compressed = vec![];
    for (off, is_xbc, size) in markers {
        if off < pos {
            continue;
        }
        let r = if is_xbc {
            xbc(d, off, label)?
        } else {
            cmp_zlib(d, off, label)?
        };
        match r {
            None => pos = off + size,
            Some((data, end)) => {
                pos = end;
                compressed.push((off, end));
                shaders(
                    &data,
                    &format!(
                        "{label}#{}@{off:#x}",
                        if is_xbc { "XBC1" } else { "cmp-zlib" }
                    ),
                    out,
                    depth + 1,
                )?;
            }
        }
    }
    let mut ci = 0;
    for base in memchr::memmem::find_iter(d, b"HCPS") {
        while ci < compressed.len() && compressed[ci].1 <= base {
            ci += 1;
        }
        if ci < compressed.len() && compressed[ci].0 <= base {
            continue; // inside a compressed stream: read once decoded
        }
        if matches!(u32_at(d, base + 4), Some(1000 | 1010)) {
            out.extend(hcps(d, base, label)?);
        }
    }
    // effect packs keep whole GLSLC outputs beside HCPS programs
    out.extend(glslc::shaders_skip(d, label, &compressed));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    /// A standalone SLCT of one variant with a vertex and a pixel stage.
    fn slct_bytes() -> Vec<u8> {
        let (c1, k1) = nvn1_vs(1);
        let (c2, k2) = nvn1_ps(2);
        let ctls = [c1.clone(), c2.clone()].concat();
        let codes = [k1.clone(), k2.clone()].concat();
        // SLCT header 72 bytes, then the variant pointer (8), the NVSD (32 + one 20-byte record), then controls, codes
        let nvsd_at = 80usize;
        let nvsd_len = 32 + 20;
        let c_at = (nvsd_at + nvsd_len + 7) & !7;
        let k_at = c_at + ctls.len();
        let mut s = vec![0u8; k_at + codes.len()];
        s[..8].copy_from_slice(b"SLCT\x03\x10\0\0");
        s[16..20].copy_from_slice(&1u32.to_le_bytes());
        s[20..24].copy_from_slice(&72u32.to_le_bytes());
        for (q, v) in [c_at, ctls.len(), k_at, codes.len()].iter().enumerate() {
            s[40 + 4 * q..44 + 4 * q].copy_from_slice(&(*v as u32).to_le_bytes());
        }
        s[72..76].copy_from_slice(&(nvsd_at as u32).to_le_bytes());
        s[76..80].copy_from_slice(&(nvsd_len as u32).to_le_bytes());
        s[nvsd_at + 24..nvsd_at + 28].copy_from_slice(&32u32.to_le_bytes());
        s[nvsd_at + 28..nvsd_at + 32].copy_from_slice(&1u32.to_le_bytes());
        let r = nvsd_at + 32;
        for (q, v) in [0usize, k1.len(), k2.len(), c1.len(), c2.len()]
            .iter()
            .enumerate()
        {
            s[r + 4 * q..r + 4 + 4 * q].copy_from_slice(&(*v as u32).to_le_bytes());
        }
        s[c_at..k_at].copy_from_slice(&ctls);
        s[k_at..].copy_from_slice(&codes);
        s
    }

    #[test]
    fn standalone_slct_and_xbc1() {
        let s = slct_bytes();
        let mut main = vec![0u8; 16];
        main.extend(&s);
        let f = slcts(&main, "exefs/main").unwrap();
        assert_eq!(
            f.iter().map(|x| x.stage).collect::<Vec<_>>(),
            vec![Stage::Vs, Stage::Ps]
        );
        // an xbc1 (zlib) around a GLSLC-free HCPS-free blob holding nothing; then around a GLSLC output
        let g = crate::formats::glslc::tests::make_output(&nvn1_ps(3).0, &nvn1_ps(3).1);
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, &g).unwrap();
        let zc = z.finish().unwrap();
        let mut x = b"xbc1".to_vec();
        x.extend(1u32.to_le_bytes());
        x.extend((g.len() as u32).to_le_bytes());
        x.extend((zc.len() as u32).to_le_bytes());
        x.extend([0u8; 32]);
        x.extend(&zc);
        let mut out = vec![];
        shaders(&x, "/bf3.ard/x.wismt", &mut out, 0).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].label.contains("#XBC1@0x0"));
    }

    #[test]
    fn arh2_offsets() {
        let mut d = b"arh2".to_vec();
        d.extend(2u32.to_le_bytes());
        d.extend(0x10u32.to_le_bytes());
        d.extend([0u8; 4]);
        for (k, n) in [(1u64, 0x18u32), (2, 0x20)] {
            d.extend(k.to_le_bytes());
            d.extend(n.to_le_bytes());
            d.extend(n.to_le_bytes());
        }
        let e = arh_entries(&d, "t").unwrap();
        assert_eq!(
            e,
            vec![
                ("/#0000000000000001".into(), 0, 0x18),
                ("/#0000000000000002".into(), 0x20, 0x20)
            ]
        );
    }
}
