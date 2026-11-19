//! Nintendo's own shader containers: BNSH (bfsha, effects, layouts, code modules), agl's BAHS (sharcb), NVN2 packs
//! (NDSR, NDVE, sead's primitive drawer), Shin'en's SHN! resources and MercurySteam's MSHD.

use crate::nvn::*;

fn hex(v: u64) -> String {
    format!("{v:#x}")
}

/// The shaders of the BNSH at `base` in `d`: every variation's program, its six classic stages, then NVN2's optional
/// ray-tracing (+0x38), task (+0x40) and mesh (+0x48) stages. Pointers are relative to the BNSH.
pub fn bnsh(d: &[u8], base: usize, label: &str) -> R<Vec<Found>> {
    let wh = format!("{label} BNSH @{}", hex(base as u64));
    if d.get(base + 0x60..base + 0x64) != Some(b"grsc") {
        return Err(format!("{wh} has no grsc block"));
    }
    let nvar = need32(d, base + 0x7c, &format!("{wh} grsc"))? as u64;
    let pvar = need64(d, base + 0x80, &format!("{wh} grsc"))?;
    let at = |rel: u64| -> Option<usize> { (base as u64).checked_add(rel).filter(|&a| a < usize::MAX as u64).map(|a| a as usize) };
    let mut out = vec![];
    for v in 0..nvar {
        let vw = format!("{wh} variation {v}");
        let pa = at(pvar.saturating_add(0x40 * v).saturating_add(0x10)).ok_or_else(|| format!("{vw}: pointer overflow"))?;
        let prog = need64(d, pa, &vw)?;
        if prog == 0 {
            continue;
        }
        let tag = format!("{label}#BNSH@{}#var{v}", hex(base as u64));
        let stage_ptrs = |sc: u64, what: &str| -> R<(u64, u64, u64, u64)> {
            let a = at(sc.saturating_add(8)).ok_or_else(|| format!("{vw} {what}: pointer overflow"))?;
            Ok((need64(d, a, &format!("{vw} {what}"))?, need64(d, a + 8, &format!("{vw} {what}"))?, need32(d, a + 16, &format!("{vw} {what}"))? as u64, need32(d, a + 20, &format!("{vw} {what}"))? as u64))
        };
        for (si, st) in Stage::BNSH.iter().enumerate() {
            let pa = at(prog.saturating_add(8 + 8 * si as u64)).ok_or_else(|| format!("{vw}: pointer overflow"))?;
            let sc = need64(d, pa, &format!("{vw} program"))?;
            if sc == 0 {
                continue;
            }
            let (pctl, pcode, ncode, nctl) = stage_ptrs(sc, st.name())?;
            let ctl = span(d, base as u64 + pctl, nctl, &format!("{vw} {st} control section"))?;
            let code = span(d, base as u64 + pcode, ncode, &format!("{vw} {st} code area"))?;
            if let Some(why) = check(*st, ctl, code) {
                return Err(format!("{wh} variation {v} {st}: {why}"));
            }
            out.push(Found::new(tag.clone(), *st, ctl, code).grouped(tag.clone()));
        }
        // NVN2's ray-tracing stage: admitted only when the pointer's control header says stage 8
        if let (Some(p38), true) = (at(prog.saturating_add(0x38)), at(prog.saturating_add(0x40)).is_some_and(|e| e <= d.len())) {
            let sc = u64_at(d, p38).unwrap_or(0);
            if sc != 0 && at(sc.saturating_add(0x20)).is_some_and(|e| e <= d.len()) {
                let (pctl, pcode, ncode, nctl) = stage_ptrs(sc, "rt")?;
                let prefix = d.get((base as u64 + pctl) as usize..).map(|s| &s[..s.len().min(0x57)]).unwrap_or(&[]);
                if prefix.len() == 0x57 && nvn2_ctl(prefix) && prefix[0x56] == 8 {
                    let ctl = span(d, base as u64 + pctl, nctl, &format!("{vw} rt control section"))?;
                    let code = span(d, base as u64 + pcode, ncode, &format!("{vw} rt code area"))?;
                    if let Some(why) = check(Stage::Rt, ctl, code) {
                        return Err(format!("{wh} variation {v} rt: {why}"));
                    }
                    out.push(Found::new(tag.clone(), Stage::Rt, ctl, code).grouped(tag.clone()));
                }
            }
        }
        // NVN2's task and mesh stages (Ocarina of Time's foliage and models); an older program ends before them
        for (off, st, sb) in [(0x40u64, Stage::Ts, 6u8), (0x48, Stage::Ms, 7)] {
            let Some(pp) = at(prog.saturating_add(off)).filter(|&p| p + 8 <= d.len()) else { continue };
            let sc = u64_at(d, pp).unwrap();
            if sc == 0 || !at(sc.saturating_add(0x20)).is_some_and(|e| e <= d.len()) {
                continue;
            }
            let Ok((pctl, pcode, ncode, nctl)) = stage_ptrs(sc, st.name()) else { continue };
            if !(0x48..0x10000).contains(&nctl) {
                continue;
            }
            let (Ok(ctl), Ok(code)) = (span(d, base as u64 + pctl, nctl, ""), span(d, base as u64 + pcode, ncode, "")) else { continue };
            if !nvn2_ctl(ctl) || ctl.len() <= 0x56 || ctl[0x56] != sb {
                continue;
            }
            if let Some(why) = check(st, ctl, code) {
                return Err(format!("{wh} variation {v} {st}: {why}"));
            }
            out.push(Found::new(tag.clone(), st, ctl, code).grouped(tag.clone()));
        }
    }
    Ok(out)
}

/// Every BNSH in `d` (embedded anywhere: bfsha, effects, layouts, code modules).
pub fn bnsh_all(d: &[u8], label: &str, out: &mut Vec<Found>) -> R<()> {
    for m in memchr::memmem::find_iter(d, b"BNSH\0\0\0\0") {
        if m + 0x20 > d.len() {
            continue;
        }
        let size = u32_at(d, m + 0x1c).unwrap() as usize;
        if m.checked_add(size).is_some_and(|e| e <= d.len()) && d.get(m + 0x60..m + 0x64) == Some(b"grsc") {
            out.extend(bnsh(d, m, label)?);
        }
    }
    Ok(())
}

/// agl's BAHS (sharcb): every 16-byte program entry {code offset, 0, control length, signed control distance} that
/// passes the control magic + code magic + lengths check; entries inside a control section are not entries.
pub fn bahs(d: &[u8], label: &str) -> R<Vec<Found>> {
    let rsz = need32(d, 0x18, &format!("{label} BAHS header"))? as usize;
    let region = need32(d, 0x1c, &format!("{label} BAHS header"))? as usize;
    if region + rsz > d.len() {
        return Err(format!("{label} BAHS code region out of bounds"));
    }
    let mut found: Vec<(usize, usize, Found)> = vec![];
    let mut a = 0usize;
    while a + 16 < region {
        let off = u32_at(d, a).unwrap() as usize;
        let z = u32_at(d, a + 4).unwrap();
        let csz = u32_at(d, a + 8).unwrap() as usize;
        let rel = i32_at(d, a + 12).unwrap() as i64;
        let c = a as i64 + rel;
        let a0 = a;
        a += 4;
        let in_region = c >= 0 && (c as usize).checked_add(csz).is_some_and(|e| e <= region);
        let nvn2 = (0x48..0x10000).contains(&csz) && in_region && nvn2_ctl(&d[c as usize..c as usize + csz]);
        if z != 0 || !(nvn2 || csz == 0x880 || csz == 0x88c || (0x760..0x880).contains(&csz)) || off % 0x100 != 0 {
            continue;
        }
        if !in_region || d[c as usize..c as usize + 4] != CTL_MAGIC {
            continue;
        }
        let c = c as usize;
        let n = u32_at(d, c + if nvn2 { 0x44 } else { 0x704 }).unwrap_or(u32::MAX) as usize;
        if off + n > rsz {
            continue;
        }
        let code = &d[region + off..region + off + n];
        let ctl = &d[c..c + csz];
        let st = if nvn2 {
            nvn2_stage(ctl, code)
        } else if code.get(..4) == Some(&CS_MAGIC[..]) {
            Some(Stage::Cs)
        } else if code.get(..4) == Some(&GFX_MAGIC[..]) {
            sph_stage(code)
        } else {
            None
        };
        let Some(st) = st else { continue };
        if check(st, ctl, code).is_some() {
            continue;
        }
        found.push((a0, c, Found::new(format!("{label}#BAHS@{}", hex(a0 as u64)), st, ctl, code)));
    }
    // An entry never lies inside a control section (an NVN2 section's +0x38..+0x47 reads as a false entry)
    let mut spans: Vec<(usize, usize)> = found.iter().map(|(_, c, f)| (*c, *c + ctl_len(&f.ctl))).collect();
    spans.sort();
    let mut maxhi = Vec::with_capacity(spans.len());
    let mut m = 0;
    for &(_, hi) in &spans {
        m = m.max(hi);
        maxhi.push(m);
    }
    let inside = |a: usize| -> bool {
        let i = spans.partition_point(|&(lo, _)| lo <= a);
        i > 0 && maxhi[i - 1] > a
    };
    let mut out: Vec<(usize, Found)> = found.into_iter().filter(|(a, _, _)| !inside(*a)).map(|(a, _, f)| (a, f)).collect();
    bahs_variations(d, label, &mut out);
    Ok(out.into_iter().map(|(_, f)| f).collect())
}

/// Groups agl BAHS entries into their programs' variations (the n-th vertex binary and the n-th fragment binary of a
/// program's range): a binary array of records (u32 size, u32 kind 0 vertex / 1 fragment / 3 compute, two u32, the
/// 16-byte entry) and right after it a program array (u32 size, u32 name length, u32 stage mask, u32 first binary).
/// A file whose arrays do not read back gives no groups.
fn bahs_variations(d: &[u8], label: &str, entries: &mut [(usize, Found)]) {
    if entries.is_empty() {
        return;
    }
    let taken: std::collections::HashSet<usize> = entries.iter().map(|(a, _)| *a).collect();
    let slack = (taken.len() / 100).max(1);
    let Some(arr) = taken.iter().min().unwrap().checked_sub(0x18) else { return };
    if arr < 0x20 {
        return;
    }
    let (Some(asz), Some(acnt)) = (u32_at(d, arr), u32_at(d, arr + 4)) else { return };
    let (asz, acnt) = (asz as usize, acnt as usize);
    if arr + asz + 8 > d.len() || acnt + slack < taken.len() {
        return;
    }
    let mut o = arr + 8;
    let mut recs = vec![];
    for _ in 0..acnt {
        if o + 0x20 > arr + asz {
            return;
        }
        let (rs, kind) = (u32_at(d, o).unwrap() as usize, u32_at(d, o + 4).unwrap());
        if rs < 0x20 {
            return;
        }
        recs.push((o + 0x10, kind));
        o += rs;
    }
    let in_recs: std::collections::HashSet<usize> = recs.iter().map(|r| r.0).collect();
    if o != arr + asz || taken.iter().filter(|a| !in_recs.contains(a)).count() > slack {
        return;
    }
    let (Some(psz), Some(pcnt)) = (u32_at(d, o), u32_at(d, o + 4)) else { return };
    let (psz, pcnt) = (psz as usize, pcnt as usize);
    if o + psz > d.len() || pcnt == 0 {
        return;
    }
    let mut q = o + 8;
    let mut bases: Vec<usize> = vec![];
    for _ in 0..pcnt {
        if q + 16 > o + psz {
            return;
        }
        let rs = u32_at(d, q).unwrap() as usize;
        let base = u32_at(d, q + 12).unwrap() as usize;
        if rs < 16 || base > acnt || bases.last().is_some_and(|&l| base < l) {
            return;
        }
        bases.push(base);
        q += rs;
    }
    if q != o + psz {
        return;
    }
    bases.push(acnt);
    let mut group: std::collections::HashMap<usize, String> = Default::default();
    for p in 0..pcnt {
        let mut seen = [0usize; 2];
        for &(a, k) in &recs[bases[p]..bases[p + 1]] {
            if k > 1 {
                continue;
            }
            if taken.contains(&a) {
                group.insert(a, format!("{label}#BAHS#prog{p}#var{}", seen[k as usize]));
            }
            seen[k as usize] += 1;
        }
    }
    for (a, f) in entries.iter_mut() {
        if let Some(g) = group.remove(a) {
            f.group = Some(g);
        }
    }
}

/// sead's primitive drawer for NVN2 (Donkey Kong Bananza): four u32 offsets (vertex control, pixel control, vertex
/// code, pixel code); each control section names its own length (+0x30) and its code area's (+0x44).
pub fn sead_prim(d: &[u8], label: &str) -> Vec<Found> {
    if d.len() < 0x10 {
        return vec![];
    }
    let offs: Vec<usize> = (0..4).map(|i| u32_at(d, 4 * i).unwrap() as usize).collect();
    let mut out = vec![];
    for (st, c, k) in [(Stage::Vs, offs[0], offs[2]), (Stage::Ps, offs[1], offs[3])] {
        if !(0x10 <= c && c + 0x48 <= d.len() && nvn2_ctl(&d[c..c + 0x48])) {
            return vec![];
        }
        let own = u32_at(d, c + 0x30).unwrap() as usize;
        let ctl = &d[c..(c + own.max(0x48)).min(d.len())];
        let n = u32_at(ctl, 0x44).unwrap() as usize;
        let Some(code) = d.get(k..k.saturating_add(n)) else { return vec![] };
        if nvn2_stage(ctl, code) != Some(st) || check(st, ctl, code).is_some() {
            return vec![];
        }
        let tag = format!("{label}#SEADPRIM");
        out.push(Found::new(format!("{label}#SEADPRIM@{}", hex(c as u64)), st, ctl, code).grouped(tag));
    }
    out
}

/// agl's NDSR pack (the dynamic super-resolution upscaler): 16-byte records {control offset, control length << 4 |
/// type, code offset, code length / 16 << 8 | type} up to a zero one, from +0x10 or +0x14 (whichever start points at
/// more control sections). A vertex record is followed by its pixel record.
pub fn ndsr(d: &[u8], label: &str) -> Vec<Found> {
    if d.len() < 0x20 || &d[..4] != b"NDSR" {
        return vec![];
    }
    let records_from = |o: usize| -> usize {
        let count = 16.min(d.len().saturating_sub(o) / 16);
        (0..count).filter(|i| {
            let c = u32_at(d, o + 16 * i).unwrap() as usize;
            c + 0x48 <= d.len() && nvn2_ctl(&d[c..c + 0x48])
        }).count()
    };
    let mut o = if records_from(0x14) > records_from(0x10) { 0x14 } else { 0x10 };
    let mut out: Vec<Found> = vec![];
    let mut prog: Option<usize> = None;
    while o + 16 <= d.len() {
        let w: Vec<u32> = (0..4).map(|i| u32_at(d, o + 4 * i).unwrap()).collect();
        if w.iter().all(|&x| x == 0) {
            break;
        }
        let (c, cl, k) = (w[0] as usize, w[1] as usize, w[2] as usize);
        if !(c + 0x48 <= d.len() && nvn2_ctl(&d[c..c + 0x48])) {
            o += 8;
            continue;
        }
        o += 16;
        let ctl = &d[c..(c + (cl >> 4).max(0x48)).min(d.len())];
        let n = u32_at(ctl, 0x44).unwrap() as usize;
        let code = &d[k.min(d.len())..k.saturating_add(n).min(d.len())];
        let Some(st) = nvn2_stage(ctl, code) else { continue };
        if check(st, ctl, code).is_some() {
            continue;
        }
        out.push(Found::new(format!("{label}#NDSR@{}", hex(c as u64)), st, ctl, code));
        let i = out.len() - 1;
        if st == Stage::Vs {
            prog = Some(i);
        } else if st == Stage::Ps {
            if let Some(p) = prog.take() {
                let g = format!("{label}#NDSR#{}", hex(u64::from_str_radix(out[p].label.rsplit("@0x").next().unwrap(), 16).unwrap_or(0)));
                out[p].group = Some(g.clone());
                out[i].group = Some(g);
            }
        }
    }
    out
}

/// nn::ndve's movie enhancement file (Ocarina of Time's movie upscaler): 'NDVE', +0xc the NDSR pack's length, +0x14 a
/// model count (8-byte records), the NDSR pack, then the networks, whose control sections each name their own length
/// (+0x30) and their code area's (+0x44), the code area right behind.
pub fn ndve(d: &[u8], label: &str) -> Vec<Found> {
    if d.len() < 0x18 {
        return vec![];
    }
    let pack = 0x18 + 8 * u32_at(d, 0x14).unwrap() as usize;
    let nets = pack.saturating_add(u32_at(d, 0xc).unwrap() as usize);
    let mut out = if pack <= nets && nets <= d.len() { ndsr(&d[pack..nets], label) } else { vec![] };
    if nets >= d.len() {
        return out;
    }
    for c in memchr::memmem::find_iter(&d[nets..], &CTL_MAGIC).map(|c| c + nets) {
        if c & 3 != 0 || c + 0x48 > d.len() || !nvn2_ctl(&d[c..c + 0x48]) {
            continue;
        }
        let own = u32_at(d, c + 0x30).unwrap() as usize;
        let n = u32_at(d, c + 0x44).unwrap() as usize;
        let (Some(ctl), Some(code)) = (d.get(c..c.saturating_add(own)), d.get(c.saturating_add(own)..c.saturating_add(own).saturating_add(n))) else { continue };
        if let Some(st) = nvn2_stage(ctl, code) {
            if check(st, ctl, code).is_none() {
                out.push(Found::new(format!("{label}#NDVE@{}", hex(c as u64)), st, ctl, code));
            }
        }
    }
    out
}

/// Shin'en's SHN! resources: three {offset, stored length, length} triples (metadata at +4, GPU data at +0x24), raw
/// or LZ4 blocks; SHDROUNC objects in the metadata have nine 0x50-byte stage records per variant.
pub fn shn(d: &[u8], label: &str) -> R<Vec<Found>> {
    span(d, 0, 0x44, &format!("{label} SHN! header"))?;
    let block = |at: usize| -> R<Vec<u8>> {
        let (off, stored, size) = (need32(d, at, label)? as usize, need32(d, at + 4, label)? as usize, need32(d, at + 8, label)? as usize);
        if stored != 0 && off < 0x44 {
            return Err(format!("{label} SHN! block at {at:#x}: data overlaps the header"));
        }
        let raw = span(d, off as u64, stored as u64, &format!("{label} SHN! block at {at:#x}"))?;
        if stored == size {
            return Ok(raw.to_vec());
        }
        if stored == 0 || size > 255 * stored {
            return Err(format!("{label} SHN! block at {at:#x}: impossible LZ4 length {size:#x} for {stored:#x} bytes"));
        }
        let out = lz4_flex::block::decompress(raw, size).map_err(|e| format!("{label} SHN! block at {at:#x}: invalid LZ4 data: {e}"))?;
        if out.len() != size {
            return Err(format!("{label} SHN! block at {at:#x}: decoded {:#x} bytes, expected {size:#x}", out.len()));
        }
        Ok(out)
    };
    let meta = block(4)?;
    let objects: Vec<usize> = memchr::memmem::find_iter(&meta, b"SHDROUNC").collect();
    if objects.is_empty() {
        return Ok(vec![]);
    }
    let data = block(0x24)?;
    let mut out = vec![];
    for h in objects {
        let count = need32(&meta, h + 0xc, &format!("{label} SHDROUNC header"))? as u64;
        let table = need64(&meta, h + 0x30, &format!("{label} SHDROUNC header"))?;
        span(&meta, table, count * 9 * 0x50, &format!("{label} SHDROUNC stage table"))?;
        for v in 0..count {
            for slot in 0..9u64 {
                let r = (table + (v * 9 + slot) * 0x50) as usize;
                let (k, c) = (u64_at(&meta, r).unwrap(), u64_at(&meta, r + 8).unwrap());
                if c == 0 {
                    continue;
                }
                let (nk, nc) = (u32_at(&meta, r + 0x40).unwrap() as u64, u32_at(&meta, r + 0x44).unwrap() as u64);
                let ctl = span(&meta, c, nc, &format!("{label} SHDROUNC control section"))?;
                let code = span(&data, k, nk, &format!("{label} SHDROUNC code area"))?;
                if !nvn2_ctl(ctl) {
                    return Err(format!("{label} SHDROUNC: invalid NVN2 control section at {c:#x}"));
                }
                let st = nvn2_stage(ctl, code).ok_or_else(|| format!("{label} SHDROUNC variant {v} slot {slot}: unknown shader stage"))?;
                if let Some(why) = check(st, ctl, code) {
                    return Err(format!("{label} SHDROUNC variant {v} slot {slot}: {why}"));
                }
                let tag = format!("{label}#SHN@{h:#x}#var{v}");
                out.push(Found::new(tag.clone(), st, ctl, code).grouped(tag));
            }
        }
    }
    Ok(out)
}

/// MercurySteam's MSHD (Metroid Dread's .bshdat): stages in vertex / pixel pairs, each a control section (+0x20 its
/// length) and the code area at the next code header.
pub fn mshd(d: &[u8], label: &str) -> R<Vec<Found>> {
    let mut out: Vec<Found> = vec![];
    let mut c = memchr::memmem::find(d, &CTL_MAGIC);
    while let Some(ci) = c {
        let nctl = need32(d, ci + 0x20, &format!("{label} MSHD control section @{ci:#x}"))? as usize;
        let g = d.get(ci + nctl..).and_then(|s| memchr::memmem::find(s, &GFX_MAGIC)).map(|g| g + ci + nctl);
        let Some(g) = g.filter(|&g| nctl >= 0x880 && g - ci - nctl <= 0x400) else {
            return Err(format!("{label} MSHD control section @{ci:#x}: length {nctl:#x}, no code area header after it"));
        };
        let ncode = need32(d, ci + 0x704, label)? as u64;
        let ctl = span(d, ci as u64, nctl as u64, &format!("{label} MSHD @{ci:#x} control section"))?;
        let code = span(d, g as u64, ncode, &format!("{label} MSHD @{ci:#x} code area"))?;
        if ncode < 0x34 {
            return Err(format!("{label} MSHD @{ci:#x}: a code area of {ncode:#x} bytes, shorter than its header"));
        }
        let want = if out.len() % 2 == 0 { Stage::Vs } else { Stage::Ps };
        let st = sph_stage(code);
        if st != Some(want) {
            return Err(format!("{label} MSHD @{ci:#x}: stage {} is {st:?}, not alternating vertex / pixel", out.len()));
        }
        if let Some(why) = check(want, ctl, code) {
            return Err(format!("{label} MSHD @{ci:#x} {want}: {why}"));
        }
        let tag = format!("{label}#MSHD#var{}", out.len() / 2);
        out.push(Found::new(tag.clone(), want, ctl, code).grouped(tag));
        c = d.get(g + ncode as usize..).and_then(|s| memchr::memmem::find(s, &CTL_MAGIC)).map(|x| x + g + ncode as usize);
    }
    if out.len() % 2 != 0 {
        return Err(format!("{label} MSHD: {} stages, not in pairs", out.len()));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::nvn::synth::*;

    /// A BNSH with one variation holding a vertex and a pixel shader.
    pub fn make_bnsh(vs: (Vec<u8>, Vec<u8>), ps: (Vec<u8>, Vec<u8>)) -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        d[..8].copy_from_slice(b"BNSH\0\0\0\0");
        d[0x60..0x64].copy_from_slice(b"grsc");
        d[0x7c..0x80].copy_from_slice(&1u32.to_le_bytes());
        d[0x80..0x88].copy_from_slice(&0x100u64.to_le_bytes()); // variation table
        d[0x110..0x118].copy_from_slice(&0x140u64.to_le_bytes()); // program
        // program: stage pointers at +8 (vs) and +0x28 (ps)
        d[0x148..0x150].copy_from_slice(&0x180u64.to_le_bytes());
        d[0x168..0x170].copy_from_slice(&0x1a0u64.to_le_bytes());
        let put = |d: &mut Vec<u8>, desc: usize, (ctl, code): (Vec<u8>, Vec<u8>)| {
            let pc = d.len();
            d.extend_from_slice(&ctl);
            let pk = d.len();
            d.extend_from_slice(&code);
            d[desc + 8..desc + 16].copy_from_slice(&(pc as u64).to_le_bytes());
            d[desc + 16..desc + 24].copy_from_slice(&(pk as u64).to_le_bytes());
            d[desc + 24..desc + 28].copy_from_slice(&(code.len() as u32).to_le_bytes());
            d[desc + 28..desc + 32].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
        };
        put(&mut d, 0x180, vs);
        put(&mut d, 0x1a0, ps);
        let n = d.len() as u32;
        d[0x1c..0x20].copy_from_slice(&n.to_le_bytes());
        d
    }

    #[test]
    fn bnsh_variation() {
        let d = make_bnsh(nvn1_vs(1), nvn1_ps(2));
        let mut prefixed = vec![0xaau8; 0x40];
        prefixed.extend_from_slice(&d);
        let mut out = vec![];
        bnsh_all(&prefixed, "/x.bfsha", &mut out).unwrap();
        assert_eq!(out.iter().map(|f| f.stage).collect::<Vec<_>>(), vec![Stage::Vs, Stage::Ps]);
        assert_eq!(out[0].group, out[1].group);
        assert_eq!(out[0].label, "/x.bfsha#BNSH@0x40#var0");
        // a cut BNSH is refused, naming the place
        let cut = &d[..d.len() - 8];
        let mut big = cut.to_vec();
        big[0x1c..0x20].copy_from_slice(&(cut.len() as u32).to_le_bytes());
        assert!(bnsh(&big, 0, "/cut").is_err());
    }

    #[test]
    fn bahs_entries() {
        // header, an entry at 0x40 pointing at a control section at 0x100, the code region at 0x1000
        let (ctl, code) = nvn1_ps(3);
        let region = 0x1000usize;
        let mut d = vec![0u8; region + 0x100 + code.len()];
        d[..4].copy_from_slice(b"BAHS");
        d[0x18..0x1c].copy_from_slice(&((0x100 + code.len()) as u32).to_le_bytes());
        d[0x1c..0x20].copy_from_slice(&(region as u32).to_le_bytes());
        let a = 0x40usize;
        d[a..a + 4].copy_from_slice(&0x100u32.to_le_bytes());
        d[a + 8..a + 12].copy_from_slice(&0x880u32.to_le_bytes());
        d[a + 12..a + 16].copy_from_slice(&((0x100 - a as i32)).to_le_bytes());
        d[0x100..0x980].copy_from_slice(&ctl);
        d[region + 0x100..].copy_from_slice(&code);
        let out = bahs(&d, "/agl.sharcb").unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stage, Stage::Ps);
        assert_eq!(out[0].key(), skey(&ctl, &code));
    }

    #[test]
    fn sead_prim_pair() {
        let (vc, vk) = nvn2(Stage::Vs, 1);
        let (pc, pk) = nvn2(Stage::Ps, 2);
        let mut d = vec![0u8; 0x10];
        let o_vc = d.len();
        d.extend_from_slice(&vc);
        let o_pc = d.len();
        d.extend_from_slice(&pc);
        let o_vk = d.len();
        d.extend_from_slice(&vk);
        let o_pk = d.len();
        d.extend_from_slice(&pk);
        for (i, o) in [o_vc, o_pc, o_vk, o_pk].into_iter().enumerate() {
            d[4 * i..4 * i + 4].copy_from_slice(&(o as u32).to_le_bytes());
        }
        let out = sead_prim(&d, "/prim.bin");
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].stage, out[1].stage), (Stage::Vs, Stage::Ps));
    }

    #[test]
    fn ndsr_records() {
        let (vc, vk) = nvn2(Stage::Vs, 1);
        let (pc, pk) = nvn2(Stage::Ps, 2);
        let mut d = vec![0u8; 0x60];
        d[..4].copy_from_slice(b"NDSR");
        let rec = |d: &mut Vec<u8>, at: usize, ctl: &[u8], code: &[u8]| {
            let c = d.len();
            d.extend_from_slice(ctl);
            let k = d.len();
            d.extend_from_slice(code);
            d[at..at + 4].copy_from_slice(&(c as u32).to_le_bytes());
            d[at + 4..at + 8].copy_from_slice(&((ctl.len() as u32) << 4).to_le_bytes());
            d[at + 8..at + 12].copy_from_slice(&(k as u32).to_le_bytes());
        };
        rec(&mut d, 0x10, &vc, &vk);
        rec(&mut d, 0x20, &pc, &pk);
        let out = ndsr(&d, "/agl_ndsr.pck");
        assert_eq!(out.len(), 2);
        assert!(out[0].group.is_some() && out[0].group == out[1].group);
    }

    #[test]
    fn mshd_pairs() {
        let (vc, vk) = nvn1_vs(1);
        let (pc, pk) = nvn1_ps(2);
        let mut d = b"MSHD".to_vec();
        d.resize(0x10, 0);
        for (c, k) in [(&vc, &vk), (&pc, &pk)] {
            let mut c = c.clone();
            c[0x20..0x24].copy_from_slice(&0x880u32.to_le_bytes());
            d.extend_from_slice(&c);
            d.extend_from_slice(k);
        }
        let out = mshd(&d, "/a.bshdat").unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].group, out[1].group);
    }
}
