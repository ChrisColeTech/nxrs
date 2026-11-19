//! Next Level Games' archives (Luigi's Mansion 3; revision 4: Mario Strikers: Battle League): `<name>.dict` indexes
//! `<name>.data` (and `.debug`, `.nxpc`, which games do not ship), an update ships `<name>.patch` beside them. The
//! shaders are control sections back to back in one section of a variant and their code areas back to back in
//! another, one program per stage.

use crate::formats::{archive, glslc, Ctx, Scanned};
use crate::nvn::*;

pub const DICT: [u8; 4] = [0x58, 0x24, 0xf3, 0xa9];
const DICT4: [u8; 2] = [4, 4];
const PATCH4: [u8; 2] = [4, 5];
/// (kind, number) of a revision-4 shader's four table entries
const GROUP4: [(u32, u32); 4] = [(1, 1), (2, 2), (1, 3), (2, 4)];

/// Sibling files a `.dict` is read with: the extensions after the archive's stem.
pub const SIBLINGS: [&str; 4] = [".data", ".patch", ".debug", ".nxpc"];

struct Dict {
    comp: bool,
    recs: Vec<(u16, [u8; 16])>,
    /// (offset, length, stored length, file)
    chunks: Vec<(u32, u32, u32, u8)>,
    names: Vec<String>,
}

fn dict(d: &[u8], label: &str) -> R<Dict> {
    let (nchunk, nrec) = if d.len() >= 0x10 { (d[0xc] as usize, d[0xd] as usize) } else { (0, 0) };
    let co = 0x10 + 0x18 * nrec;
    let bad = d.len() < co + 0x10 * nchunk || nrec > nchunk || (0..nrec).any(|i| d[0x18 + 0x18 * i..0x28 + 0x18 * i].iter().any(|&c| c as usize >= nchunk));
    if bad {
        return Err(format!("{label}: the index ({:#x} bytes, header {:02x?}) is not in the layout of Luigi's Mansion 3's archives", d.len(), d.get(4..0x10).unwrap_or(&[])));
    }
    let recs = (0..nrec).map(|i| (u16_at(d, 0x14 + 0x18 * i).unwrap(), d[0x18 + 0x18 * i..0x28 + 0x18 * i].try_into().unwrap())).collect();
    let chunks = (0..nchunk).map(|i| {
        let o = co + 0x10 * i;
        (u32_at(d, o).unwrap(), u32_at(d, o + 4).unwrap(), u32_at(d, o + 8).unwrap(), d[o + 0xe])
    }).collect();
    let names = d[co + 0x10 * nchunk..].split(|&b| b == 0).take(d[0xe] as usize).map(|n| n.iter().map(|&c| c as char).collect()).collect();
    Ok(Dict { comp: d[6] != 0, recs, chunks, names })
}

/// The plain entries of a variant's table in the game's order (lists inline, depth first).
fn walk(tab: &[u8], start: u32, count: u32, out: &mut Vec<usize>, depth: usize) {
    if depth > 64 {
        return;
    }
    for i in start..start.saturating_add(count) {
        let i = i as usize;
        let (Some(w0), Some(w1), Some(w2)) = (u32_at(tab, 12 * i), u32_at(tab, 12 * i + 4), u32_at(tab, 12 * i + 8)) else { return };
        if w0 >> 31 != 0 {
            walk(tab, w2, w1, out, depth + 1);
        } else {
            out.push(i);
        }
    }
}

/// An update's section `s`, rebuilt as the game does after loading the old one: the entries of section s in table
/// order are packed from the section's start at their alignment, each taking its data from the old section (offsets
/// below its length) or from the update's body (counted from the old length); old[cut..end] is kept aside first, as
/// the game copies it before the packing overwrites it.
#[allow(clippy::too_many_arguments)]
fn patch_section(tab: &mut [u8], order: &[usize], s: u32, old: &[u8], body: &[u8], cut: usize, end: usize, size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; old.len().max(size)];
    buf[..old.len()].copy_from_slice(old);
    let keep: Vec<u8> = if end != 0 { old.get(cut..end.min(old.len())).unwrap_or(&[]).to_vec() } else { vec![] };
    let mut cur = 0usize;
    let put = |buf: &mut Vec<u8>, at: usize, src: &[u8]| {
        if buf.len() < at + src.len() {
            buf.resize(at + src.len(), 0);
        }
        buf[at..at + src.len()].copy_from_slice(src);
    };
    for &i in order {
        let (w0, n, src) = (u32_at(tab, 12 * i).unwrap(), u32_at(tab, 12 * i + 4).unwrap() as usize, u32_at(tab, 12 * i + 8).unwrap() as usize);
        if (w0 >> 27) & 0xf != s {
            continue;
        }
        let al = if w0 & 0x7c0_0000 != 0 { 1usize << ((w0 >> 22) & 0x1f) } else { 1 };
        cur = (cur + al - 1) & !(al - 1);
        let piece: Vec<u8> = if src >= old.len() {
            let a = src - old.len();
            body.get(a..(a + n).min(body.len())).unwrap_or(&[]).to_vec()
        } else if end != 0 && cut <= src && src < end {
            let a = src - cut;
            keep.get(a..(a + n).min(keep.len())).unwrap_or(&[]).to_vec()
        } else {
            buf.get(src..(src + n).min(buf.len())).unwrap_or(&[]).to_vec()
        };
        put(&mut buf, cur, &piece);
        tab[12 * i + 8..12 * i + 12].copy_from_slice(&(cur as u32).to_le_bytes());
        cur += n;
    }
    buf.truncate(size);
    buf.resize(size, 0);
    buf
}

/// A Next Level Games archive's shaders: `index` is the `.dict`, `sibling(ext)` reads `<stem><ext>` when the RomFS has
/// it. BNSH containers in a revision-4 archive's data go through `scan_blob` as everywhere else.
pub fn nlg(ctx: &Ctx, index: &[u8], sibling: &dyn Fn(&str) -> Option<Vec<u8>>, label: &str, out: &mut Scanned) -> R<()> {
    if index.get(4..6) == Some(&DICT4[..]) {
        return nlg4(ctx, index, sibling, label, out);
    }
    let dc = dict(index, label)?;
    let files: std::collections::HashMap<u8, Vec<u8>> = dc.names.iter().enumerate().filter_map(|(i, n)| sibling(n).map(|b| (i as u8, b))).collect();
    let patch = sibling(".patch");
    let mut cache: std::collections::HashMap<usize, Vec<u8>> = std::collections::HashMap::new();
    let mut chunk = |ci: usize| -> R<Vec<u8>> {
        if let Some(c) = cache.get(&ci) {
            return Ok(c.clone());
        }
        let (off, n, stored, f) = dc.chunks[ci];
        let file = files.get(&f).ok_or_else(|| format!("{label}: chunk {ci} is in a file the game does not ship"))?;
        let raw = span(file, off as u64, if dc.comp { stored } else { n } as u64, &format!("{label}: chunk {ci}"))?;
        let c = if dc.comp { archive::unzlib(raw, label)? } else { raw.to_vec() };
        if c.len() != n as usize {
            return Err(format!("{label}: chunk {ci} inflates to {:#x} bytes, the index says {n:#x}", c.len()));
        }
        cache.insert(ci, c.clone());
        Ok(c)
    };
    let (mut mask, mut body, mut bsec) = (0u64, vec![], vec![0u32; 16]);
    if let Some(p) = &patch {
        mask = u64_at(p, 0).ok_or_else(|| format!("{label}: patch cut short"))?;
        let (boff, bstored, bn) = (need32(p, 8, "patch")? as u64, need32(p, 12, "patch")? as u64, need32(p, 16, "patch")? as usize);
        body = archive::unzlib(span(p, boff, bstored, &format!("{label}: patch body"))?, label)?;
        if body.len() != bn {
            return Err(format!("{label}: the patch body inflates to {:#x} bytes, its header says {bn:#x}", body.len()));
        }
        bsec = (0..16).map(|k| u32_at(&body, 4 * k).unwrap_or(0)).collect();
    }
    for (v, (top, secs)) in dc.recs.iter().enumerate() {
        let used: Vec<usize> = std::iter::once(v).chain(secs.iter().filter(|&&c| c != 0).map(|&c| c as usize)).collect();
        if used.iter().any(|&ci| !files.contains_key(&dc.chunks[ci].3)) {
            continue; // a variant of a file the game does not ship
        }
        let _ = top;
        let mut sections: std::collections::HashMap<u32, Vec<u8>> = std::collections::HashMap::new();
        let live: Vec<u32>;
        if v < 64 && mask >> v & 1 != 0 {
            let p = patch.as_ref().unwrap();
            let at = 0x18 + 0xd0 * (mask & ((1u64 << v) - 1)).count_ones() as usize;
            let r: Vec<u32> = (0..52).map(|k| u32_at(p, at + 4 * k).unwrap_or(0)).collect();
            let mut tab = archive::unzlib(span(p, r[0] as u64, r[1] as u64, &format!("{label}: variant {v} patch table"))?, label)?;
            if tab.len() != 12 * r[2] as usize {
                return Err(format!("{label}: variant {v} patch table has {} bytes for {} entries", tab.len(), r[2]));
            }
            let mut order = vec![];
            walk(&tab, 0, r[3] & 0xffff, &mut order, 0);
            live = (0..16).filter(|&s| r[4 + s as usize] != 0).collect();
            for &s in &live {
                let old = if secs[s as usize] != 0 { chunk(secs[s as usize] as usize)? } else { vec![] };
                let b0 = bsec[s as usize] as usize;
                let b1 = if s < 15 { bsec[s as usize + 1] as usize } else { body.len() };
                let piece = body.get(b0..b1.max(b0).min(body.len())).unwrap_or(&[]);
                let sec = patch_section(&mut tab, &order, s, &old, piece, r[36 + s as usize] as usize, r[20 + s as usize] as usize, r[4 + s as usize] as usize);
                sections.insert(s, sec);
            }
        } else {
            live = (0..16).filter(|&s| secs[s as usize] != 0).collect();
            for &s in &live {
                sections.insert(s, chunk(secs[s as usize] as usize)?);
            }
        }
        for &c in live.iter().filter(|&&s| sections[&s].get(..4) == Some(&CTL_MAGIC[..])) {
            let ctls = &sections[&c];
            let n = u32_at(ctls, 0x20).unwrap_or(0) as usize;
            if n < 0x24 {
                continue;
            }
            let mut took = false;
            for &k in &live {
                if k == c {
                    continue;
                }
                let code = &sections[&k];
                let (mut found, mut p, mut i) = (vec![], 0usize, 0usize);
                let mut ok = true;
                while ctls.get(i * n..i * n + 4) == Some(&CTL_MAGIC[..]) && (i + 1) * n <= ctls.len() {
                    let ctl = &ctls[i * n..(i + 1) * n];
                    let ln = u32_at(ctl, 0x704).unwrap_or(0) as usize;
                    let cd = code.get(p..p + ln).unwrap_or(&[]);
                    let st = glslc::stage(ctl, cd);
                    match st {
                        Some(st) if check(st, ctl, cd).is_none() => found.push(Found::new(format!("{label}#NLG{v}:{c}@{:#x}", i * n), st, ctl, cd)),
                        _ => {
                            ok = false;
                            break;
                        }
                    }
                    p += ln;
                    i += 1;
                }
                if ok && !found.is_empty() {
                    out.found.extend(found);
                    took = true;
                    break;
                }
            }
            if !took {
                return Err(format!("{label}: variant {v} section {c} holds control sections, but no section of the variant holds their code areas in order"));
            }
        }
    }
    Ok(())
}

/// The stored length of the zstd frame at `p`, from its header and block headers (RFC 8878).
pub fn zstd_frame_len(d: &[u8], p: usize) -> Option<usize> {
    let fhd = *d.get(p + 4)? as usize;
    let single = fhd >> 5 & 1;
    let mut q = p + 5 + (1 - single) + [0, 1, 2, 4][fhd & 3] + [single, 2, 4, 8][fhd >> 6];
    loop {
        let h = u32::from_le_bytes([*d.get(q)?, *d.get(q + 1)?, *d.get(q + 2)?, 0]) as usize;
        q += 3 + if h >> 1 & 3 == 1 { 1 } else { h >> 3 };
        if h & 1 != 0 {
            let end = q + 4 * (fhd >> 2 & 1);
            return (end <= d.len()).then_some(end - p);
        }
    }
}

/// zstd frames decoded as one stream, the zero padding between them left out.
fn unzs_frames(d: &[u8], label: &str) -> R<Vec<u8>> {
    let mut out = vec![];
    let mut p = 0;
    while p < d.len() {
        if d[p..].starts_with(&archive::ZSTD_MAGIC) {
            let n = zstd_frame_len(d, p).ok_or_else(|| format!("{label}: a zstd frame at {p:#x} runs past the end"))?;
            let mut r = zstd::stream::read::Decoder::new(&d[p..p + n]).map_err(|e| format!("{label}: {e}"))?;
            std::io::Read::read_to_end(&mut r, &mut out).map_err(|e| format!("{label}: zstd frame at {p:#x}: {e}"))?;
            p += n;
        } else if d[p] == 0 {
            p += 1;
        } else {
            return Err(format!("{label}: not a zstd frame at {p:#x}"));
        }
    }
    Ok(out)
}

/// A revision-4 index's shader table: 12-byte entries {u8 kind, u16 type, u8 number, u32 length, u32 offset}, four per
/// shader (GROUP4), all of one type. → [(code length, code offset, control length, control offset)]
fn programs4(idx: &[u8]) -> Vec<(u32, u32, u32, u32)> {
    let group = |p: usize| -> Option<[(u32, u32, u32); 4]> {
        let mut e = [(0, 0, 0); 4];
        for (k, x) in e.iter_mut().enumerate() {
            *x = (u32_at(idx, p + 12 * k)?, u32_at(idx, p + 12 * k + 4)?, u32_at(idx, p + 12 * k + 8)?);
        }
        Some(e)
    };
    let fits = |e: &[(u32, u32, u32); 4], kind: Option<u32>| {
        e.iter().zip(GROUP4).all(|(x, g)| (x.0 & 0xff, x.0 >> 24) == g) && e.iter().all(|x| (x.0 >> 8 & 0xffff) == kind.unwrap_or(e[0].0 >> 8 & 0xffff))
    };
    let mut p = 0;
    let start = loop {
        if p + 48 >= idx.len() {
            return vec![];
        }
        if let Some(e) = group(p) {
            if fits(&e, None) && (0x760..=0x880).contains(&e[1].1) {
                break p;
            }
        }
        p += 4;
    };
    let kind = group(start).unwrap()[0].0 >> 8 & 0xffff;
    let mut out = vec![];
    let mut p = start;
    while let Some(e) = group(p).filter(|e| fits(e, Some(kind))) {
        out.push((e[0].1, e[0].2, e[1].1, e[1].2));
        p += 48;
    }
    out
}

/// Where a section starts in `buf`: the first start for which every offset lands on one of `magics`.
fn place(buf: &[u8], offs: &[u32], magics: &[[u8; 4]]) -> Option<usize> {
    let first = *offs.iter().min()? as usize;
    let mut qs: Vec<usize> = magics.iter().flat_map(|m| memchr::memmem::find_iter(buf, m).filter(|q| q % 4 == 0)).collect();
    qs.sort();
    qs.into_iter().filter(|&q| q >= first).map(|q| q - first).find(|&s| offs.iter().all(|&o| buf.get(s + o as usize..s + o as usize + 4).is_some_and(|x| magics.iter().any(|m| x == m))))
}

fn nlg4(ctx: &Ctx, index: &[u8], sibling: &dyn Fn(&str) -> Option<Vec<u8>>, label: &str, out: &mut Scanned) -> R<()> {
    let progs0 = programs4(&unzs_frames(index.get(0x40..).unwrap_or(&[]), label)?);
    let data = unzs_frames(&sibling(".data").ok_or_else(|| format!("{label}: no .data beside the index"))?, label)?;
    let patch = sibling(".patch");
    if let Some(p) = &patch {
        if p.get(4..6) != Some(&PATCH4[..]) {
            return Err(format!("{label}: the patch header {:02x?} is not a revision-4 patch", p.get(4..8).unwrap_or(&[])));
        }
    }
    let body = match &patch {
        Some(p) => unzs_frames(p.get(0x18..).unwrap_or(&[]), label)?,
        None => vec![],
    };
    // NintendoWare Ui2d / Font keep BNSH containers in other archives' data (menu text, tutorial overlays)
    for (name, buf) in [("", &data), (".patch", &body)] {
        if memchr::memmem::find(buf, b"BNSH\0\0\0\0").is_some() {
            crate::formats::scan_blob(ctx, buf, &format!("{label}{name}"), out, 0);
        }
    }
    if progs0.is_empty() {
        return Ok(());
    }
    let ctl_m = [CTL_MAGIC];
    let code_m = [GFX_MAGIC, CS_MAGIC];
    let cs0 = place(&data, &progs0.iter().map(|p| p.3).collect::<Vec<_>>(), &ctl_m);
    let ks0 = place(&data, &progs0.iter().map(|p| p.1).collect::<Vec<_>>(), &code_m);
    let (Some(cs0), Some(ks0)) = (cs0, ks0) else {
        return Err(format!("{label}: no start of the control / code section fits all {} programs of the shader table", progs0.len()));
    };
    let ctl_old = progs0.iter().map(|p| p.2 + p.3).max().unwrap();
    let code_old = progs0.iter().map(|p| p.0 + p.1).max().unwrap();
    let (mut cs1, mut ks1, mut progs) = (0usize, 0usize, progs0);
    if let Some(p) = &patch {
        let want = need32(p, 8, "patch header")? as usize;
        if body.len() != want {
            return Err(format!("{label}: the patch body inflates to {:#x} bytes, its header says {want:#x}", body.len()));
        }
        progs = programs4(&body);
        if progs.is_empty() {
            return Err(format!("{label}: the patch body has no shader table"));
        }
        let nc: Vec<u32> = progs.iter().filter(|p| p.3 >= ctl_old).map(|p| p.3 - ctl_old).collect();
        let nk: Vec<u32> = progs.iter().filter(|p| p.1 >= code_old).map(|p| p.1 - code_old).collect();
        let a = if nc.is_empty() { Some(0) } else { place(&body, &nc, &ctl_m) };
        let b = if nk.is_empty() { Some(0) } else { place(&body, &nk, &code_m) };
        let (Some(a), Some(b)) = (a, b) else {
            return Err(format!("{label}: no start of the patch body's new control / code data fits its programs"));
        };
        (cs1, ks1) = (a, b);
    }
    let at = |buf: &[u8], s: usize, o: u32, n: u32| buf.get(s + o as usize..s + o as usize + n as usize).unwrap_or(&[]).to_vec();
    for (i, &(kn, ko, cn, co)) in progs.iter().enumerate() {
        let ctl = if co < ctl_old { at(&data, cs0, co, cn) } else { at(&body, cs1, co - ctl_old, cn) };
        let code = if ko < code_old { at(&data, ks0, ko, kn) } else { at(&body, ks1, ko - code_old, kn) };
        let st = glslc::stage(&ctl, &code);
        let why = match st {
            None => Some("no stage"),
            Some(st) => check(st, &ctl, &code),
        };
        if let Some(w) = why {
            return Err(format!("{label}: program {i} of the shader table{}: {w}", if patch.is_some() { " (patched)" } else { "" }));
        }
        out.found.push(Found::new(format!("{label}#NLG4{}:{i}", if patch.is_some() { "p" } else { "" }), st.unwrap(), &ctl, &code));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvn::synth::*;

    #[test]
    fn dict_variant_pairs_sections() {
        let (c1, k1) = nvn1_vs(1);
        let (c2, k2) = nvn1_ps(2);
        let ctls = [c1.clone(), c2.clone()].concat();
        let codes = [k1.clone(), k2.clone()].concat();
        let table = vec![0u8; 12];
        // data: chunk 0 the table, chunk 1 the code areas (section 7), chunk 2 the control sections (section 8)
        let data = [table.clone(), codes.clone(), ctls.clone()].concat();
        let mut d = vec![0u8; 0x10];
        d[..4].copy_from_slice(&DICT);
        d[4] = 4;
        d[5] = 1;
        d[0xc] = 3;
        d[0xd] = 1;
        d[0xe] = 1;
        let mut rec = vec![0u8; 0x18];
        rec[8 + 7] = 1;
        rec[8 + 8] = 2;
        d.extend(rec);
        for (off, n) in [(0usize, 12usize), (12, codes.len()), (12 + codes.len(), ctls.len())] {
            let mut c = vec![0u8; 0x10];
            c[..4].copy_from_slice(&(off as u32).to_le_bytes());
            c[4..8].copy_from_slice(&(n as u32).to_le_bytes());
            d.extend(c);
        }
        d.extend(b".data\0");
        let sib = |e: &str| (e == ".data").then(|| data.clone());
        let mut s = Scanned::default();
        nlg(&Ctx::default(), &d, &sib, "/init.dict", &mut s).unwrap();
        assert_eq!(s.found.len(), 2);
        assert_eq!(s.found[1].stage, Stage::Ps);
        assert_eq!(s.found[1].code, k2);
    }

    #[test]
    fn zstd_frames_with_padding() {
        let a = zstd::bulk::compress(b"hello ", 3).unwrap();
        let b = zstd::bulk::compress(b"world", 3).unwrap();
        assert_eq!(zstd_frame_len(&a, 0), Some(a.len()));
        let mut d = a.clone();
        d.extend([0, 0, 0]);
        d.extend(&b);
        assert_eq!(unzs_frames(&d, "t").unwrap(), b"hello world");
    }
}
