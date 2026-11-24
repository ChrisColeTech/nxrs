//! Shader containers read from bytes in memory: Sumo Digital's `Ninty_NX` descriptors, `NVNsh` records and MuxFDMR
//! v2 streams; Firaxis' CIVBSF libraries; Diablo II: Resurrected's SHAD chunk trees; PhyreEngine effects and loose
//! programs; Star Fox's (Viper) shader binaries and VIPR containers; Larian's BSHD records; NintendoWare Bezel Engine
//! archives (BEA).

use crate::formats::{archive, glslc, raw};
use crate::nvn::*;

pub const SUMO_NX: &[u8; 16] = b"Ninty_NX\0\0\0\0\0\0\0\0";
pub const NVNSH: &[u8; 6] = b"NVNsh\0";
pub const SUMO_MUX: &[u8; 8] = b"MuxFDMR\x02";
pub const CIVBSF: &[u8; 8] = b"CIVBSF\0\0";
pub const SHAD: &[u8; 8] = b"SHADSHAD";
pub const SCNE: &[u8; 4] = b"SCNE";

const STAGES: [Stage; 6] = [
    Stage::Vs,
    Stage::Hs,
    Stage::Ds,
    Stage::Gs,
    Stage::Ps,
    Stage::Cs,
];

fn inflate_exact(s: &[u8], expected: usize) -> Option<Vec<u8>> {
    let mut z = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(expected + 1);
    match z.decompress_vec(s, &mut out, flate2::FlushDecompress::Finish) {
        Ok(flate2::Status::StreamEnd)
            if z.total_in() as usize == s.len() && out.len() == expected =>
        {
            Some(out)
        }
        _ => None,
    }
}

/// MuxFDMR v2: two buffer lengths at +24 / +32, one or two zlib streams whose stored lengths start at +40.
pub fn sumo_members(d: &[u8], label: &str) -> R<Vec<(String, Vec<u8>)>> {
    if d.len() < 44 {
        return Err(format!("{label}: truncated MuxFDMR header"));
    }
    let (mut head, n) = (u32_at(d, 8).unwrap() as usize, d[18] as usize);
    if d[17] != 2 || !(1..=2).contains(&n) || head != 44 + 4 * n || head > d.len() {
        return Err(format!("{label}: unsupported MuxFDMR layout"));
    }
    let sizes: Vec<usize> = (0..n)
        .map(|i| u32_at(d, 40 + 4 * i).unwrap() as usize)
        .collect();
    if head + sizes.iter().sum::<usize>() != d.len() {
        return Err(format!("{label}: MuxFDMR streams do not fit the file"));
    }
    let mut out = vec![];
    for (i, &size) in sizes.iter().enumerate() {
        let expected = u32_at(d, 24 + 8 * i).unwrap() as usize;
        let raw = inflate_exact(&d[head..head + size], expected).ok_or_else(|| {
            format!("{label}: MuxFDMR stream {i} length does not match its header")
        })?;
        out.push((format!("#MUX{i}"), raw));
        head += size;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn sumo_program(
    d: &[u8],
    base: usize,
    size: usize,
    co: usize,
    ko: usize,
    cn: usize,
    kn: usize,
    label: String,
    stage: Option<Stage>,
) -> R<Found> {
    let disjoint = co + cn <= ko || ko + kn <= co;
    if size < 0x34
        || base + size > d.len()
        || cn < 0x760
        || kn < 0x40
        || co.min(ko) < 0x34
        || (co + cn).max(ko + kn) > size
        || !disjoint
    {
        return Err(format!("{label}: shader ranges do not fit the container"));
    }
    let (ctl, code) = (&d[base + co..base + co + cn], &d[base + ko..base + ko + kn]);
    let st = glslc::stage(ctl, code)
        .filter(|st| stage.is_none_or(|s| s == *st))
        .ok_or_else(|| format!("{label}: invalid shader stage"))?;
    if let Some(w) = check(st, ctl, code) {
        return Err(format!("{label}: {w}"));
    }
    Ok(Found::new(label, st, ctl, code))
}

/// Sumo's `Ninty_NX` descriptors (container-relative ranges) and `NVNsh` v1 records (one stage each). A magic whose
/// container does not fit the data is a literal in code, not a container.
pub fn sumo(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out = vec![];
    for m in memchr::memmem::find_iter(d, SUMO_NX) {
        let Some(r) = m.checked_sub(0x30) else {
            continue;
        };
        let Some(base) = u64_at(d, r + 0x28)
            .and_then(|o| (r as u64).checked_sub(o))
            .map(|b| b as usize)
        else {
            continue;
        };
        if r < base + 0x38 {
            continue;
        }
        let Some(size) = u64_at(d, base + 8).map(|s| s as usize) else {
            continue;
        };
        if r + 0x40 > base.saturating_add(size) || base.saturating_add(size) > d.len() {
            continue;
        }
        if u32_at(d, base + 4) != Some(1) {
            return Err(format!("{label}: unsupported Ninty_NX container version"));
        }
        let (co, ko, cn, kn) = (
            u64_at(d, r).unwrap() as usize,
            u64_at(d, r + 8).unwrap() as usize,
            u32_at(d, r + 16).unwrap() as usize,
            u32_at(d, r + 20).unwrap() as usize,
        );
        out.push(sumo_program(
            d,
            base,
            size,
            co,
            ko,
            cn,
            kn,
            format!("{label}#SUMO@{r:#x}"),
            None,
        )?);
    }
    for h in memchr::memmem::find_iter(d, NVNSH) {
        let Some(size) = u32_at(d, h + 8).map(|s| s as usize) else {
            continue;
        };
        if h + 0x34 > d.len() || !(0x34..=d.len() - h).contains(&size) {
            continue;
        }
        if u16_at(d, h + 6) != Some(1) {
            return Err(format!("{label}: unsupported NVNsh version"));
        }
        let stage = u32_at(d, h + 28).unwrap() as usize;
        let st = *STAGES
            .get(stage)
            .ok_or_else(|| format!("{label}: invalid NVNsh stage {stage}"))?;
        let (co, ko, kn, cn) = (
            u32_at(d, h + 32).unwrap() as usize,
            u32_at(d, h + 36).unwrap() as usize,
            u32_at(d, h + 40).unwrap() as usize,
            u32_at(d, h + 44).unwrap() as usize,
        );
        out.push(sumo_program(
            d,
            h,
            size,
            co,
            ko,
            cn,
            kn,
            format!("{label}#NVNSH@{h:#x}"),
            Some(st),
        )?);
    }
    Ok(out)
}

/// Firaxis' shader library: revision 0x1118 (NVBC: GLSLC output + reflection per platform descriptor 3) or 0x1218
/// ({u32 0xff00ff00, code length, control length, flags}, code, control). One stage each.
pub fn civbsf(d: &[u8], label: &str) -> R<Vec<Found>> {
    let at = format!("{label} CIVBSF");
    span(d, 0, 0x90, &at)?;
    let version = u32_at(d, 8).unwrap();
    let stride = match version {
        0x1118 => 0x48,
        0x1218 => 0x50,
        v => return Err(format!("{at}: unknown revision {v:#x}")),
    };
    if u32_at(d, 0xc).unwrap() as usize != d.len() {
        return Err(format!(
            "{at}: file size {:#x} does not match {:#x} bytes",
            u32_at(d, 0xc).unwrap(),
            d.len()
        ));
    }
    let (table, count) = (u64_at(d, 0x20).unwrap(), u32_at(d, 0x68).unwrap() as u64);
    if table < 0x90 {
        return Err(format!("{at}: shader table overlaps the header"));
    }
    span(d, table, count * stride, &format!("{at} shader table"))?;
    let mut out = vec![];
    for i in 0..count {
        let entry = (table + i * stride) as usize;
        let src = format!("{at} shader {i}");
        let desc = need64(d, entry + 8, &src)?;
        if version == 0x1118 {
            span(d, desc, 0x40, &format!("{src} platform descriptors"))?;
            for platform in 0..4u64 {
                let o = (desc + platform * 16) as usize;
                let (offset, length, kind) = (
                    u64_at(d, o).unwrap(),
                    u32_at(d, o + 8).unwrap() as u64,
                    u32_at(d, o + 12).unwrap() as u64,
                );
                if kind != platform {
                    return Err(format!(
                        "{src}: platform descriptor {platform} has kind {kind}"
                    ));
                }
                let payload = span(
                    d,
                    offset,
                    length,
                    &format!("{src} platform {platform} data"),
                )?;
                if platform != 3 {
                    continue;
                }
                span(payload, 0, 0x28, &format!("{src} NVBC"))?;
                if &payload[..8] != b"NVBC\0\0\0\0" {
                    return Err(format!("{src}: unknown NVBC tag"));
                }
                let (nout, nrefl) = (u64_at(payload, 8).unwrap(), u64_at(payload, 16).unwrap());
                if 0x28 + nout + nrefl != length {
                    return Err(format!(
                        "{src}: NVBC output and reflection lengths do not match the data size"
                    ));
                }
                let output = &payload[0x28..0x28 + nout as usize];
                if output.get(..4) != Some(&glslc::MAGIC[..])
                    || glslc::layout(output, 0).0 as u64 != nout
                {
                    return Err(format!("{src}: invalid NVBC GLSLC output length or tag"));
                }
                out.extend(glslc::shaders(
                    output,
                    &format!("{label}#CIVBSF@{entry:#x}"),
                ));
            }
            continue;
        }
        let (offset, length) = (
            need64(d, desc as usize, &src)?,
            need64(d, desc as usize + 8, &src)?,
        );
        let payload = span(d, offset, length, &format!("{src} data"))?;
        span(payload, 0, 16, &src)?;
        let (tag, ncode, nctl) = (
            u32_at(payload, 0).unwrap(),
            u32_at(payload, 4).unwrap() as u64,
            u32_at(payload, 8).unwrap() as u64,
        );
        if tag != 0xff00_ff00 {
            return Err(format!("{src}: unknown data tag {tag:#x}"));
        }
        if 16 + ncode + nctl != length {
            return Err(format!(
                "{src}: code and control lengths do not match the data size"
            ));
        }
        let (code, ctl) = (
            &payload[16..16 + ncode as usize],
            &payload[16 + ncode as usize..],
        );
        let st = glslc::stage(ctl, code).ok_or_else(|| format!("{src}: unknown shader stage"))?;
        if let Some(w) = check(st, ctl, code) {
            return Err(format!("{src}: {w}"));
        }
        out.push(Found::new(
            format!("{label}#CIVBSF@{entry:#x}"),
            st,
            ctl,
            code,
        ));
    }
    Ok(out)
}

/// Diablo II: Resurrected's shader container: a tree of chunks {tag, second tag, count, -1 or 0, header length, data
/// start, length}; SHAD and SHMD hold chunks, PRM[ a table of offsets to BTCD chunks, a BTCD's data the record
/// `raw::shad_record` reads.
pub fn shad(d: &[u8], label: &str) -> R<Vec<Found>> {
    fn walk(
        d: &[u8],
        label: &str,
        mut at: usize,
        end: usize,
        depth: usize,
        out: &mut Vec<Found>,
    ) -> R<()> {
        if depth > 32 {
            return Err(format!("{label}: SHAD chunks nested too deep"));
        }
        while at < end {
            if at + 28 > end {
                return Err(format!(
                    "{label}: chunk header at {at:#x} runs past {end:#x}"
                ));
            }
            let tag = &d[at..at + 4];
            let cnt = u32_at(d, at + 8).unwrap() as usize;
            let (hl, ds, size) = (
                u32_at(d, at + 16).unwrap() as usize,
                u32_at(d, at + 20).unwrap() as usize,
                u32_at(d, at + 24).unwrap() as usize,
            );
            if !(28 <= hl && hl <= ds && ds <= size) || at + size > end {
                return Err(format!("{label}: chunk {:?} at {at:#x} (header {hl:#x}, data {ds:#x}, length {size:#x}) does not fit {end:#x}", String::from_utf8_lossy(tag)));
            }
            match tag {
                b"BTCD" => {
                    let (ctl, code) = raw::shad_record(d, at + hl, at + size).ok_or_else(|| {
                        format!("{label}: BTCD at {at:#x} does not hold a record of its length")
                    })?;
                    let wh = format!("{label}#SHAD@{at:#x}");
                    let st = glslc::stage(ctl, code);
                    let why = match st {
                        Some(st) => check(st, ctl, code),
                        None => Some("no stage in the code area header"),
                    };
                    if let Some(w) = why {
                        return Err(format!("{wh}: {w}"));
                    }
                    out.push(Found::new(wh, st.unwrap(), ctl, code));
                }
                b"PRM[" => {
                    if hl + 4 * cnt > ds {
                        return Err(format!("{label}: PRM[ at {at:#x}: {cnt} offsets do not fit before its data at {ds:#x}"));
                    }
                    for k in 0..cnt {
                        let o = u32_at(d, at + hl + 4 * k).unwrap() as usize;
                        walk(d, label, at + ds + o, at + size, depth + 1, out)?;
                    }
                }
                b"SHAD" | b"SHMD" => walk(d, label, at + ds, at + size, depth + 1, out)?,
                _ => {}
            }
            at += size;
            if tag == b"BTCD" && depth != 0 {
                return Ok(()); // a PRM[ offset names one chunk
            }
        }
        Ok(())
    }
    let mut out = vec![];
    walk(d, label, 0, d.len(), 0, &mut out)?;
    Ok(out)
}

/// PhyreEngine: the NVN code area immediately before its control section (unaligned in RYHP effects; loose programs
/// prefix their two lengths). Headerless compute code is taken only in an RYHP container.
pub fn phyre(d: &[u8], label: &str) -> Vec<Found> {
    let mut out = vec![];
    for c in memchr::memmem::find_iter(d, &CTL_MAGIC) {
        if c + 0x708 > d.len() {
            continue;
        }
        let (cn, kn) = (
            u32_at(d, c + 0x20).unwrap() as usize,
            u32_at(d, c + 0x704).unwrap() as usize,
        );
        let Some(k) = c.checked_sub(kn) else { continue };
        if kn < 0x80 || cn < 0x760 || c + cn > d.len() {
            continue;
        }
        let (ctl, code) = (&d[c..c + cn], &d[k..c]);
        let st = glslc::stage(ctl, code);
        if st == Some(Stage::Cs)
            && code.get(..4) != Some(&CS_MAGIC[..])
            && (!d.starts_with(b"RYHP") || memchr::memmem::find(code, &CTL_MAGIC).is_some())
        {
            continue;
        }
        if let Some(st) = st.filter(|&st| check(st, ctl, code).is_none()) {
            out.push(Found::new(format!("{label}#PHYRE@{k:#x}"), st, ctl, code));
        }
    }
    out
}

/// Whether `d` may be a Phyre container or loose program (scan_blob's test).
pub fn is_phyre(d: &[u8]) -> bool {
    d.starts_with(b"RYHP") || d.get(8..12) == Some(&GFX_MAGIC[..])
}

/// Star Fox's shader binary: the GLSLC output magic, u32 code length, u32 control length, u32 0, a 64-bit hash, the code
/// area and the NVN2 control section. → None when `d` is not one.
pub fn viper(d: &[u8], label: &str) -> Option<R<Vec<Found>>> {
    if d.len() < 0x18 + NVN2_KEY_CTL || d[..4] != glslc::MAGIC {
        return None;
    }
    let (ncode, nctl) = (u32_at(d, 4)? as usize, u32_at(d, 8)? as usize);
    if 0x18 + ncode + nctl != d.len() || ncode == 0 || nctl == 0 {
        return None;
    }
    let (code, ctl) = (&d[0x18..0x18 + ncode], &d[0x18 + ncode..]);
    if !nvn2_ctl(ctl) {
        return None;
    }
    let Some(st) = nvn2_stage(ctl, code) else {
        return Some(Err(format!(
            "{label}: a shader binary of no known stage (control +0x56 {:?})",
            ctl.get(0x56)
        )));
    };
    if let Some(w) = check(st, ctl, code) {
        return Some(Err(format!("{label} ({}): {w}", st.name())));
    }
    Some(Ok(vec![Found::new(
        format!("{label}#VIPER"),
        st,
        ctl,
        code,
    )]))
}

/// A VIPR resource container: the NDSR networks it wraps.
pub fn vipr(d: &[u8], label: &str) -> Vec<Found> {
    let mut out = vec![];
    for p in memchr::memmem::find_iter(d, b"NDSR") {
        if p + 0x20 <= d.len() && d[p + 4..p + 6] == [0, 1] {
            out.extend(crate::formats::nvnfmt::ndsr(
                &d[p..],
                &format!("{label}#NDSR@{p:#x}"),
            ));
        }
    }
    out
}

/// Larian's BSHD v1 record: a big-endian length (top bit: zlib), then {0xffffffff, 1, count, {u8 stage, u32 offset,
/// u32 size} each}. NX stages are GLSLC outputs (stage 1 vertex, 3 pixel, 4 compute); other platforms' are left out.
pub fn bshd(data: &[u8], label: &str) -> R<Vec<Found>> {
    if data.len() < 4 {
        return Err(format!("{label}: truncated BSHD length"));
    }
    let word = u32::from_be_bytes(data[..4].try_into().unwrap());
    let n = (word & 0x7fff_ffff) as usize;
    let d = if word >> 31 != 0 {
        inflate_exact(&data[4..], n).ok_or_else(|| format!("{label}: damaged BSHD zlib stream"))?
    } else {
        data[4..].to_vec()
    };
    if d.len() != n {
        return Err(format!(
            "{label}: BSHD of {} bytes, its length says {n}",
            d.len()
        ));
    }
    if d.len() < 6 || d[..5] != [0xff, 0xff, 0xff, 0xff, 1] {
        return Err(format!("{label}: unsupported BSHD header"));
    }
    let count = d[5] as usize;
    let start = 6 + 9 * count;
    if count == 0 || start > d.len() {
        return Err(format!("{label}: truncated BSHD stage table"));
    }
    let mut out = vec![];
    for i in 0..count {
        let (stage, offset, size) = (
            d[6 + 9 * i],
            u32_at(&d, 7 + 9 * i).unwrap() as usize,
            u32_at(&d, 11 + 9 * i).unwrap() as usize,
        );
        if offset < start || size == 0 || offset + size > d.len() {
            return Err(format!("{label}: invalid BSHD stage {i} range"));
        }
        let program = &d[offset..offset + size];
        if program.get(..4) != Some(&glslc::MAGIC[..]) {
            continue;
        }
        let expected = match stage {
            1 => Some(Stage::Vs),
            3 => Some(Stage::Ps),
            4 => Some(Stage::Cs),
            _ => None,
        };
        let st = glslc::shaders(program, label);
        if st.len() != 1 || Some(st[0].stage) != expected {
            return Err(format!(
                "{label}: BSHD stage {stage} disagrees with its GLSLC program"
            ));
        }
        let f = &st[0];
        out.push(
            Found::new(format!("{label}#BSHD#var"), f.stage, &f.ctl, &f.code)
                .grouped(format!("{label}#BSHD#var")),
        );
    }
    Ok(out)
}

/// A NintendoWare Bezel Engine archive ('SCNE'): +0x20 the entry count, +0x28 the entry pointer array (or, Bezel
/// Engine 2.9, the 'ASST' entries themselves back to back); each entry's data (one zstd frame or stored) and path.
pub fn bea_members(d: &[u8], label: &str) -> R<Vec<(String, Vec<u8>)>> {
    let (n, arr) = (
        need64(d, 0x20, label)? as usize,
        need64(d, 0x28, label)? as usize,
    );
    let inline = d.get(arr..arr + 4) == Some(b"ASST");
    let mut o = arr;
    let mut out = vec![];
    for i in 0..n {
        if !inline {
            o = need64(d, arr + 8 * i, label)? as usize;
        }
        if d.get(o..o + 4) != Some(b"ASST") {
            return Err(format!("{label}: entry {i} @{o:#x} is not an ASST entry"));
        }
        let size = if inline {
            need32(d, o + 4, label)? as usize
        } else {
            0x30
        };
        let (csz, usz) = (
            need32(d, o + 0x14, label)? as u64,
            need32(d, o + 0x18, label)? as usize,
        );
        let (off, nm) = (
            need64(d, o + size - 0x10, label)?,
            need64(d, o + size - 8, label)? as usize,
        );
        if inline {
            o += size.max(4);
        }
        let nl = need16(d, nm, label)? as usize;
        let path = String::from_utf8_lossy(span(d, nm as u64 + 2, nl as u64, label)?).into_owned();
        let raw = span(d, off, csz, &format!("{label}/{path}"))?;
        let data = if raw.starts_with(&archive::ZSTD_MAGIC) {
            let mut v = Vec::with_capacity(usz);
            let mut r = zstd::stream::read::Decoder::new(raw)
                .map_err(|e| format!("{label}/{path}: {e}"))?;
            std::io::Read::read_to_end(&mut r, &mut v)
                .map_err(|e| format!("{label}/{path}: {e}"))?;
            v
        } else {
            raw.to_vec()
        };
        if data.len() != usz {
            return Err(format!(
                "{label}/{path}: {:#x} bytes, the entry says {usz:#x}",
                data.len()
            ));
        }
        out.push((path, data));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    #[test]
    fn nvnsh_and_viper_and_phyre() {
        let (c, k) = nvn1_ps(1);
        // NVNsh: 0x34 header, then control, then code
        let co = 0x34usize;
        let ko = co + c.len();
        let size = ko + k.len();
        let mut h = vec![0u8; 0x34];
        h[..6].copy_from_slice(NVNSH);
        h[6..8].copy_from_slice(&1u16.to_le_bytes());
        h[8..12].copy_from_slice(&(size as u32).to_le_bytes());
        h[28..32].copy_from_slice(&4u32.to_le_bytes());
        for (q, v) in [co, ko, k.len(), c.len()].iter().enumerate() {
            h[32 + 4 * q..36 + 4 * q].copy_from_slice(&(*v as u32).to_le_bytes());
        }
        h.extend(&c);
        h.extend(&k);
        let f = sumo(&h, "/x.bin").unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].stage, Stage::Ps);
        // Viper
        let (c2, k2) = nvn2(Stage::Cs, 2);
        let mut v = glslc::MAGIC.to_vec();
        v.extend((k2.len() as u32).to_le_bytes());
        v.extend((c2.len() as u32).to_le_bytes());
        v.extend([0u8; 12]);
        v.extend(&k2);
        v.extend(&c2);
        assert_eq!(
            viper(&v, "/data.zip/a.shb").unwrap().unwrap()[0].stage,
            Stage::Cs
        );
        // Phyre: code immediately before control
        let mut p = b"RYHP".to_vec();
        p.extend([0u8; 3]);
        p.extend(&k);
        p.extend(&c);
        assert_eq!(phyre(&p, "/x.phyre").len(), 1);
    }

    #[test]
    fn shad_tree_and_bshd() {
        let (c, k) = nvn1_vs(3);
        let mut rec = vec![];
        for w in [
            5u32,
            c.len() as u32,
            k.len() as u32 + 0xc,
            k.len() as u32,
            4,
        ] {
            rec.extend(w.to_le_bytes());
        }
        rec.extend(&k);
        rec.extend(&c);
        let chunk = |tag: &[u8; 4], cnt: u32, hl: u32, body: &[u8]| -> Vec<u8> {
            let mut v = tag.to_vec();
            v.extend(tag);
            v.extend(cnt.to_le_bytes());
            v.extend(0u32.to_le_bytes());
            v.extend(hl.to_le_bytes());
            v.extend(hl.to_le_bytes());
            v.extend((hl + body.len() as u32).to_le_bytes());
            v.extend(vec![0u8; hl as usize - 28]);
            v.extend(body);
            v
        };
        let btcd = chunk(b"BTCD", 0, 0x24, &rec);
        let mut prm_body = 0u32.to_le_bytes().to_vec();
        prm_body.extend(&btcd);
        let mut prm = chunk(b"PRM[", 1, 28, &[]);
        // PRM[: header 28, the offset table (4), data starts after it
        prm.truncate(0);
        prm.extend(b"PRM[PRM[");
        prm.extend(1u32.to_le_bytes());
        prm.extend(0u32.to_le_bytes());
        prm.extend(28u32.to_le_bytes());
        prm.extend(32u32.to_le_bytes());
        prm.extend((32 + btcd.len() as u32).to_le_bytes());
        prm.extend(0u32.to_le_bytes());
        prm.extend(&btcd);
        let shmd = chunk(b"SHMD", 0, 28, &prm);
        let shad_c = chunk(b"SHAD", 0, 28, &shmd);
        let f = shad(&shad_c, "/data/hd/shaders/vs_x.bin").unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].stage, Stage::Vs);
        // BSHD: one stored record with a GLSLC output
        let g = glslc::tests::make_output(&nvn1_ps(4).0, &nvn1_ps(4).1);
        let mut body = vec![0xff, 0xff, 0xff, 0xff, 1, 1];
        let off = 6 + 9;
        body.push(3);
        body.extend((off as u32).to_le_bytes());
        body.extend((g.len() as u32).to_le_bytes());
        body.extend(&g);
        let mut b = (body.len() as u32).to_be_bytes().to_vec();
        b.extend(&body);
        assert_eq!(bshd(&b, "/Shared.pak/x.bshd").unwrap()[0].stage, Stage::Ps);
    }
}
