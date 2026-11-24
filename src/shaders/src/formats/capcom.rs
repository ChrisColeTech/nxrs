//! Capcom: the RE Engine's PAK archives (KPKA) with their shader definitions (SDF) and OpenColorIO pipelines
//! (OCIO), and MT Framework's ARC archives with their shader package (SPK).

use super::archive;
use crate::nvn::*;

pub const KPKA: &[u8; 4] = b"KPKA";
const KPKA_BLOCKS: u16 = 0x20;
const KPKA_IN_BLOCKS: u64 = 0x100_0000;

/// One PAK entry: path hash, offset (the first block for one in blocks), stored length, length, attributes (bits
/// 0-3: 0 stored, 1 raw deflate, 2 zstd).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KpkaEntry {
    pub hash: u64,
    pub off: u64,
    pub stored: u64,
    pub len: u64,
    pub attr: u64,
}

/// A PAK's table: 'KPKA', u8 version 4, u8 minor, u16 features, u32 count, u32 fingerprint, then 0x30-byte entries
/// {path hash, offset, stored length, length, attributes, checksum}; with feature 0x20 a block table follows: u32 block
/// length, u32 count, a u64 per block (offset in bits 0-41, stored length from 42; a block of the block length is
/// stored, any other one zstd frame). `read(offset, length)` reads the PAK. → (entries, block length and blocks).
#[allow(clippy::type_complexity)]
pub fn kpka_entries(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    label: &str,
) -> R<(Vec<KpkaEntry>, Option<(u64, Vec<(u64, u64)>)>)> {
    let h = read(0, 16)?;
    if h.get(..4) != Some(&KPKA[..]) {
        return Err(format!("{label}: not a KPKA PAK"));
    }
    let (ver, feat, n) = (h[4], u16_at(&h, 6).unwrap(), u32_at(&h, 8).unwrap() as u64);
    if ver != 4 || feat & !KPKA_BLOCKS != 0 {
        return Err(format!("{label}: PAK version {ver}, features {feat:#x}: only version 4 with a plain entry table, or with a block table (0x20), is known"));
    }
    let t = read(16, 0x30 * n)?;
    let ents = t
        .chunks_exact(0x30)
        .map(|e| KpkaEntry {
            hash: u64_at(e, 0).unwrap(),
            off: u64_at(e, 8).unwrap(),
            stored: u64_at(e, 16).unwrap(),
            len: u64_at(e, 24).unwrap(),
            attr: u64_at(e, 32).unwrap(),
        })
        .collect();
    let blocks = if feat & KPKA_BLOCKS != 0 {
        let bh = read(16 + 0x30 * n, 8)?;
        let (bl, nb) = (
            u32_at(&bh, 0).unwrap() as u64,
            u32_at(&bh, 4).unwrap() as u64,
        );
        let b = read(24 + 0x30 * n, 8 * nb)?;
        Some((
            bl,
            b.chunks_exact(8)
                .map(|x| u64::from_le_bytes(x.try_into().unwrap()))
                .map(|x| (x & ((1 << 42) - 1), x >> 42))
                .collect(),
        ))
    } else {
        None
    };
    Ok((ents, blocks))
}

/// An entry's data, or only its first `head` bytes (as far as a cheap partial decode gives them).
pub fn kpka_data(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    e: &KpkaEntry,
    blocks: Option<&(u64, Vec<(u64, u64)>)>,
    head: Option<usize>,
    label: &str,
) -> R<Vec<u8>> {
    let want = head.map(|h| (h as u64).min(e.len)).unwrap_or(e.len);
    if want > archive::DECODE_LIMIT {
        return Err(format!("{label}: {want:#x} bytes"));
    }
    if e.attr & KPKA_IN_BLOCKS != 0 {
        let (bl, bs) = blocks.ok_or_else(|| {
            format!("{label}: its data is in blocks, but its PAK has no block table")
        })?;
        if e.attr & 0xf != 0 {
            return Err(format!(
                "{label}: compression {} inside blocks is not known",
                e.attr & 0xf
            ));
        }
        let mut out = vec![];
        let mut i = e.off as usize;
        while (out.len() as u64) < want {
            let &(off, n) = bs
                .get(i)
                .ok_or_else(|| format!("{label}: block {i} is past the block table"))?;
            let raw = read(off, n)?;
            out.extend(if n == *bl {
                raw
            } else {
                archive::unzstd(&raw, &Default::default(), label)?
            });
            i += 1;
        }
        out.truncate(want as usize);
        return Ok(out);
    }
    let raw = read(e.off, e.stored)?;
    match e.attr & 0xf {
        0 => Ok(raw[..(want as usize).min(raw.len())].to_vec()),
        1 => {
            let mut z = flate2::Decompress::new(false);
            let mut out = vec![0u8; want as usize];
            z.decompress(&raw, &mut out, flate2::FlushDecompress::Finish)
                .map_err(|x| format!("{label}: deflate: {x}"))?;
            out.truncate(z.total_out() as usize);
            Ok(out)
        }
        2 => match head {
            Some(h) => archive::zstd_peek(&raw, h)
                .ok_or_else(|| format!("{label}: zstd frame does not decode")),
            None => archive::unzstd(&raw, &Default::default(), label),
        },
        c => Err(format!("{label}: compression {c} is not known")),
    }
}

/// (first entry, entry length, the stage record's lengths at, its control section at): Monster Hunter Rise / Ghost
/// Trick, PRAGMATA, Resident Evil 4.
const SDF_LAYOUTS: [(usize, usize, usize, usize); 3] = [
    (0x10, 0x108, 0, 8),
    (0x18, 0x110, 8, 0x58),
    (0x18, 0x110, 8, 0x60),
];
const SDF_STAGES: [Stage; 6] = [
    Stage::Vs,
    Stage::Hs,
    Stage::Ds,
    Stage::Gs,
    Stage::Ps,
    Stage::Cs,
];

/// RE Engine shader definitions ('SDF\0'): u16 version 1, u16 entry count, entries in one of SDF_LAYOUTS (the one
/// whose every stage offset leads to a control section), each with six u64 stage offsets at +0x10 (vertex, hull,
/// domain, geometry, pixel, compute; 0 none) to a record {u32 control length, u32 code length} + control + code.
pub fn sdf(d: &[u8], label: &str) -> R<Vec<Found>> {
    let (ver, n) = (need16(d, 4, label)?, need16(d, 6, label)? as usize);
    if n == 0 {
        return Ok(vec![]);
    }
    if ver != 1 {
        return Err(format!("{label}: SDF version {ver}"));
    }
    let fits = |&(base, size, _, at): &(usize, usize, usize, usize)| {
        if base + size * n > d.len() {
            return false;
        }
        let offs: Vec<usize> = (0..n)
            .flat_map(|i| {
                (0..6).map(move |k| u64_at(d, base + size * i + 0x10 + 8 * k).unwrap() as usize)
            })
            .filter(|&o| o != 0)
            .collect();
        !offs.is_empty()
            && offs
                .iter()
                .all(|&o| d.get(o + at..o + at + 4) == Some(&CTL_MAGIC[..]))
    };
    let lays: Vec<_> = SDF_LAYOUTS.iter().filter(|l| fits(l)).collect();
    if lays.len() != 1 {
        return Err(format!(
            "{label}: the SDF entries fit {} of the {} known layouts",
            lays.len(),
            SDF_LAYOUTS.len()
        ));
    }
    let &(base, size, lens, at) = lays[0];
    let mut out = vec![];
    for i in 0..n {
        for (k, &slot) in SDF_STAGES.iter().enumerate() {
            let o = u64_at(d, base + size * i + 0x10 + 8 * k).unwrap() as usize;
            if o == 0 {
                continue;
            }
            let w = format!("{label} SDF entry {i} {}", slot.name());
            let (nc, nk) = (
                need32(d, o + lens, &w)? as u64,
                need32(d, o + lens + 4, &w)? as u64,
            );
            let b = span(d, (o + at) as u64, nc + nk, &w)?;
            let (ctl, code) = b.split_at(nc as usize);
            let mut st = slot;
            let mut why = check(st, ctl, code).map(|s| s.to_string());
            if why.is_none() && nvn2_ctl(ctl) {
                // NVN2: the stage from the control section or the SPH; PRAGMATA keeps mesh shaders in the hull slot
                let got = nvn2_stage(ctl, code);
                match got {
                    Some(g) if g == st || (st == Stage::Hs && g == Stage::Ms) => st = g,
                    _ => why = Some(format!("the stage ({got:?}) does not match the entry slot")),
                }
            } else if why.is_none() && st != Stage::Cs && sph_stage(code) != Some(st) {
                why = Some("the SPH stage does not match the entry slot".into());
            }
            if let Some(why) = why {
                return Err(format!("{w}: {why}"));
            }
            out.push(
                Found::new(format!("{label}#SDF#var{i}#{}", st.name()), st, ctl, code)
                    .grouped(format!("{label}#SDF#var{i}")),
            );
        }
    }
    Ok(out)
}

/// RE Engine OpenColorIO pipelines ('OCIO'): NVN2 stage records as in SDF files (a 0x58- or 0x60-byte header, the
/// lengths at +8), found by their control sections at any offset.
pub fn ocio(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for c in memchr::memmem::find_iter(d, &CTL_MAGIC) {
        for hl in [0x58usize, 0x60] {
            let Some(h) = c.checked_sub(hl) else { continue };
            let (nc, nk) = (
                u32_at(d, h + 8).unwrap() as usize,
                u32_at(d, h + 12).unwrap() as usize,
            );
            let (Some(ctl), Some(code)) = (d.get(c..c + nc), d.get(c + nc..c + nc + nk)) else {
                continue;
            };
            if nc == 0 || !nvn2_ctl(ctl) || u32_at(ctl, 0x44) != Some(nk as u32) {
                continue;
            }
            let st = nvn2_stage(ctl, code)
                .ok_or_else(|| format!("{label} OCIO record @{h:#x}: no stage"))?;
            if let Some(why) = check(st, ctl, code) {
                return Err(format!("{label} OCIO record @{h:#x}: {why}"));
            }
            out.push(Found::new(format!("{label}#OCIO@{h:x}"), st, ctl, code));
            break;
        }
    }
    Ok(out)
}

/// A PAK entry's bytes (scan.rs reads entries in groups): its shaders when it is an SDF or OCIO file.
pub fn kpka_member(
    read: &dyn Fn(u64, u64) -> R<Vec<u8>>,
    e: &KpkaEntry,
    blocks: Option<&(u64, Vec<(u64, u64)>)>,
    label: &str,
) -> R<Vec<Found>> {
    if e.len < 0x10 {
        return Ok(vec![]);
    }
    let head = kpka_data(read, e, blocks, Some(4), label)?;
    match head.as_slice() {
        b"SDF\0" => sdf(&kpka_data(read, e, blocks, None, label)?, label),
        b"OCIO" => ocio(&kpka_data(read, e, blocks, None, label)?, label),
        _ => Ok(vec![]),
    }
}

pub const MTARC: &[u8; 4] = b"ARC\0";

/// MT Framework's archive: 'ARC\0', u16 version 7, u16 count, then per file {path (128 or 64 bytes), u32 type hash,
/// u32 stored length, u32 length (low 29 bits), u32 offset}; a member is a zlib stream, or stored when the lengths are
/// equal. The path width is the one whose every member lies after the table and inside the archive.
pub fn mtarc_members(d: &[u8], label: &str) -> R<Vec<(String, Vec<u8>)>> {
    let (ver, n) = (need16(d, 4, label)?, need16(d, 6, label)? as usize);
    if ver != 7 {
        return Err(format!(
            "{label}: ARC version {ver}: only version 7 is known"
        ));
    }
    let rank = |width: usize| -> i32 {
        let step = width + 16;
        let end = 8 + n * step;
        if end > d.len() {
            return -1;
        }
        let mut data = false;
        for o in (8..end).step_by(step) {
            let (st, sz, off) = (
                u32_at(d, o + width + 4).unwrap() as usize,
                u32_at(d, o + width + 8).unwrap() & 0x1fff_ffff,
                u32_at(d, o + width + 12).unwrap() as usize,
            );
            if st == 0 && sz == 0 && off == 0 {
                continue;
            }
            if off < end || off + st > d.len() {
                return -1;
            }
            data |= st != 0;
        }
        data as i32
    };
    let wide = rank(128);
    let width = if wide == 1 {
        128
    } else {
        let short = rank(64);
        if wide < 0 && short < 0 {
            return Err(format!(
                "{label}: ARC v7 has no complete directory with in-bounds members"
            ));
        }
        if wide >= short {
            128
        } else {
            64
        }
    };
    let mut out = vec![];
    for i in 0..n {
        let o = 8 + i * (width + 16);
        let p = &d[o..o + width];
        let path =
            String::from_utf8_lossy(&p[..memchr::memchr(0, p).unwrap_or(width)]).replace('\\', "/");
        let (st, sz, off) = (
            u32_at(d, o + width + 4).unwrap() as usize,
            (u32_at(d, o + width + 8).unwrap() & 0x1fff_ffff) as usize,
            u32_at(d, o + width + 12).unwrap() as usize,
        );
        let w = format!("{label}#{path}");
        let raw = span(d, off as u64, st as u64, &w)?;
        let data = if st != sz {
            archive::unzlib(raw, &w)?
        } else {
            raw.to_vec()
        };
        if data.len() != sz {
            return Err(format!(
                "{w}: {:#x} bytes, the entry says {sz:#x}",
                data.len()
            ));
        }
        out.push((path, data));
    }
    Ok(out)
}

/// MT Framework's shader package ('SPK\0'): after its tables the shaders lie back to back to the end, each a code
/// area and its control section (code first in The Great Ace Attorney Chronicles, control first in Ultra Street
/// Fighter II); every one starts where the previous one ended.
pub fn mtspk(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut c = memchr::memmem::find(d, &CTL_MAGIC)
        .ok_or_else(|| format!("{label} SPK: no shader control section"))?;
    let nk = need32(d, c + 0x704, label)? as usize;
    let control_first = c < nk || d.get(c - nk..c - nk + 4) != Some(&GFX_MAGIC[..]);
    let mut out = vec![];
    let mut end: Option<usize> = None;
    loop {
        if c + 0x708 > d.len() || d[c..c + 4] != CTL_MAGIC {
            return Err(format!("{label} SPK: no complete control header at {c:#x}"));
        }
        let (nc, nk) = (
            u32_at(d, c + 0x20).unwrap() as usize,
            u32_at(d, c + 0x704).unwrap() as usize,
        );
        let g = if control_first {
            Some(c + nc)
        } else {
            c.checked_sub(nk)
        };
        let Some(g) = g.filter(|&g| {
            nc >= 0x708
                && c + nc <= d.len()
                && g + nk <= d.len()
                && d.get(g..g + 4) == Some(&GFX_MAGIC[..])
        }) else {
            return Err(format!(
                "{label} SPK: no matching graphics code area for control at {c:#x}"
            ));
        };
        let start = if control_first { c } else { g };
        if end.is_some_and(|e| e != start) {
            return Err(format!("{label} SPK: noncontiguous shader at {start:#x}"));
        }
        let (ctl, code) = (&d[c..c + nc], &d[g..g + nk]);
        let st = sph_stage(code)
            .ok_or_else(|| format!("{label} SPK @{start:#x}: no stage in the SPH"))?;
        if let Some(why) = check(st, ctl, code) {
            return Err(format!("{label} SPK @{start:#x}: {why}"));
        }
        out.push(Found::new(
            format!("{label}#MTSPK@{start:#x}"),
            st,
            ctl,
            code,
        ));
        let e = if control_first { g + nk } else { c + nc };
        if e == d.len() {
            return Ok(out);
        }
        end = Some(e);
        c = if control_first {
            e
        } else {
            memchr::memmem::find(&d[e..], &CTL_MAGIC)
                .map(|x| x + e)
                .ok_or_else(|| {
                    format!("{label} SPK: the last shader ends at {e:#x}, before the end")
                })?
        };
    }
}

/// An MT Framework archive's shader packages.
pub fn mtarc(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for (path, m) in mtarc_members(d, label)? {
        if m.starts_with(b"SPK\0") {
            out.extend(mtspk(&m, &format!("{label}#{path}"))?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    fn with_len(mut c: Vec<u8>) -> Vec<u8> {
        let n = c.len() as u32;
        c[0x20..0x24].copy_from_slice(&n.to_le_bytes());
        c
    }

    #[test]
    fn kpka_sdf() {
        let (vs, ps) = (nvn1_vs(15), nvn1_ps(16));
        // SDF, Monster Hunter Rise's layout: one entry at 0x10 (0x108 bytes), records after it
        let mut s = b"SDF\0".to_vec();
        s.extend_from_slice(&1u16.to_le_bytes());
        s.extend_from_slice(&1u16.to_le_bytes());
        s.resize(0x10 + 0x108, 0);
        for (slot, (c, k)) in [(0usize, &vs), (4, &ps)] {
            let o = s.len();
            s[0x20 + 8 * slot..0x28 + 8 * slot].copy_from_slice(&(o as u64).to_le_bytes());
            s.extend_from_slice(&(c.len() as u32).to_le_bytes());
            s.extend_from_slice(&(k.len() as u32).to_le_bytes());
            s.extend_from_slice(c);
            s.extend_from_slice(k);
        }
        let z = zstd::bulk::compress(&s, 3).unwrap();
        let mut pak = KPKA.to_vec();
        pak.extend_from_slice(&[4, 0, 0, 0]);
        pak.extend_from_slice(&1u32.to_le_bytes());
        pak.extend_from_slice(&0u32.to_le_bytes());
        for x in [0x1234u64, 0x40, z.len() as u64, s.len() as u64, 2, 0] {
            pak.extend_from_slice(&x.to_le_bytes());
        }
        pak.extend_from_slice(&z);
        let read = |o: u64, n: u64| Ok(pak[o as usize..(o + n) as usize].to_vec());
        let (ents, blocks) = kpka_entries(&read, "/re_chunk_000.pak").unwrap();
        assert!(blocks.is_none());
        let f = kpka_member(&read, &ents[0], None, "/re_chunk_000.pak#1234").unwrap();
        assert_eq!(
            f.iter().map(|f| (f.stage, f.key())).collect::<Vec<_>>(),
            vec![
                (Stage::Vs, skey(&vs.0, &vs.1)),
                (Stage::Ps, skey(&ps.0, &ps.1))
            ]
        );
        assert_eq!(f[0].group, f[1].group);
    }

    #[test]
    fn mt_arc_spk() {
        let ((c1, k1), (c2, k2)) = (nvn1_vs(17), nvn1_ps(18));
        let (vs, ps) = ((with_len(c1), k1), (with_len(c2), k2));
        let mut spk = b"SPK\0".to_vec();
        spk.resize(0x20, 0);
        for (c, k) in [&vs, &ps] {
            spk.extend_from_slice(k);
            spk.extend_from_slice(c);
        }
        let mut z = flate2::write::ZlibEncoder::new(vec![], flate2::Compression::default());
        std::io::Write::write_all(&mut z, &spk).unwrap();
        let z = z.finish().unwrap();
        let mut d = MTARC.to_vec();
        d.extend_from_slice(&7u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        let mut p = b"sc\\NX_64\\root".to_vec();
        p.resize(128, 0);
        d.extend_from_slice(&p);
        let off = d.len() + 16;
        for x in [0u32, z.len() as u32, spk.len() as u32, off as u32] {
            d.extend_from_slice(&x.to_le_bytes());
        }
        d.extend_from_slice(&z);
        let f = mtarc(&d, "/root.arc").unwrap();
        assert_eq!(
            f.iter().map(|f| f.key()).collect::<Vec<_>>(),
            vec![skey(&vs.0, &vs.1), skey(&ps.0, &ps.1)]
        );
    }
}
