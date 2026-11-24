//! Control sections loose in a main module, in no GLSLC output and no BNSH file: shaders compiled offline whose
//! control section and code area the code hands to NVN itself (Advance Wars 1+2: Re-Boot Camp's Bink 2 shaders,
//! Diablo II: Resurrected's records in main, FINAL FANTASY XII's PhyreEngine built-ins).
//!
//! A control section's code area is, in order:
//! - the one a data record names: two relocated slots, the second pointing at the control section, the first at a code
//!   area, then the code length (u32) equal to the control's +0x704;
//! - the start of a Diablo II record {5, control length, code length + 0xc, code length, 4} ending at the control
//!   section, the record referenced by a relocation or built by adrp + add;
//! - a code area whose address the code builds (adrp + add) within 1 KB of where it builds the control section's,
//!   right after the control section (its +0x20 length, 16-aligned) or right before it;
//! - with nothing referencing it: the code area with a header and exactly the control's +0x704 length that ends right
//!   at it (PhyreEngine's form; the only form an unaligned control section is taken in).

use super::glslc;
use crate::nvn::*;
use std::collections::{HashMap, HashSet};

/// A module image's relative relocations (RELA type 0x403, and DT_RELR): slot → target, module offsets.
pub fn rel_targets(img: &[u8]) -> HashMap<usize, usize> {
    let mut out = HashMap::new();
    let Some(m) = u32_at(img, 4).map(|x| x as usize) else {
        return out;
    };
    let Some(dynoff) = i32_at(img, m + 4).and_then(|x| m.checked_add_signed(x as isize)) else {
        return out;
    };
    let mut tags: HashMap<i64, u64> = HashMap::new();
    let mut o = dynoff;
    while let (Some(tag), Some(v)) = (u64_at(img, o), u64_at(img, o + 8)) {
        o += 16;
        if tag == 0 {
            break;
        }
        tags.entry(tag as i64).or_insert(v);
    }
    if let (Some(&rela), Some(&sz)) = (tags.get(&7), tags.get(&8)) {
        let mut i = 0;
        while i + 24 <= sz as usize {
            let a = rela as usize + i;
            let (Some(off), Some(info), Some(add)) =
                (u64_at(img, a), u64_at(img, a + 8), u64_at(img, a + 16))
            else {
                break;
            };
            if info & 0xffff_ffff == 0x403 {
                out.insert(off as usize, add as usize);
            }
            i += 24;
        }
    }
    if let (Some(&relr), Some(&sz)) = (tags.get(&36), tags.get(&35)) {
        let mut next = 0usize;
        let mut i = 0;
        while i + 8 <= sz as usize {
            let Some(e) = u64_at(img, relr as usize + i) else {
                break;
            };
            i += 8;
            if e & 1 == 0 {
                if let Some(t) = u64_at(img, e as usize) {
                    out.insert(e as usize, t as usize);
                }
                next = e as usize + 8;
                continue;
            }
            for b in 0..63 {
                if (e >> (b + 1)) & 1 != 0 {
                    let s = next + 8 * b;
                    if let Some(t) = u64_at(img, s) {
                        out.insert(s, t as usize);
                    }
                }
            }
            next += 8 * 63;
        }
    }
    out
}

/// Addresses in [lo, hi) the code builds with adrp + add on one register (the add within the next 8 instructions)
/// → the offsets of those adds. Every word is tried.
pub fn adrp_targets(d: &[u8], lo: usize, hi: usize) -> HashMap<usize, Vec<usize>> {
    let mut out: HashMap<usize, Vec<usize>> = HashMap::new();
    let n = d.len() / 4;
    let w = |i: usize| u32::from_le_bytes(d[4 * i..4 * i + 4].try_into().unwrap());
    for i in 0..n {
        let x = w(i);
        if x & 0x9f00_0000 != 0x9000_0000 {
            continue;
        }
        let rd = x & 31;
        let imm = (((x >> 5) & 0x7ffff) << 2 | (x >> 29) & 3) as i64;
        let imm = (imm << 43) >> 43; // 21 bits, signed
        let page = ((i as i64 * 4) & !0xfff) + (imm << 12);
        if page < lo as i64 - 0x1000 || page >= hi as i64 {
            continue;
        }
        for j in i + 1..(i + 9).min(n) {
            let y = w(j);
            if y & 0xffc0_0000 == 0x9100_0000 && (y >> 5) & 31 == rd {
                let t = page + ((y >> 10) & 0xfff) as i64;
                if t >= lo as i64 && t < hi as i64 {
                    out.entry(t as usize).or_default().push(4 * j);
                }
                break;
            }
        }
    }
    out
}

/// Diablo II: Resurrected's record at d[at..end]: {u32 5, control length, code length + 0xc, code length, u32 4},
/// the code area, the control section → (control, code).
pub fn shad_record(d: &[u8], at: usize, end: usize) -> Option<(&[u8], &[u8])> {
    if at + 0x14 > end || end > d.len() {
        return None;
    }
    let w: Vec<u32> = (0..5).map(|i| u32_at(d, at + 4 * i).unwrap()).collect();
    if w[0] != 5
        || w[4] != 4
        || w[2] != w[3].wrapping_add(0xc)
        || at + 0x14 + w[3] as usize + w[1] as usize != end
    {
        return None;
    }
    Some((
        &d[at + 0x14 + w[3] as usize..end],
        &d[at + 0x14..at + 0x14 + w[3] as usize],
    ))
}

/// The loose control sections of `d` (a main module's image) with their code areas; `known` the control sections
/// other readers already took from it (compared at their own +0x20 length). Control sections with no code area
/// found go to `problems`.
pub fn raw_shaders(
    d: &[u8],
    label: &str,
    known: &HashSet<Vec<u8>>,
    problems: &mut Vec<String>,
) -> Vec<Found> {
    let w32 = |o: usize| u32_at(d, o).unwrap_or(0) as usize;
    let headed = |k: usize| {
        d.get(k..k + 4)
            .is_some_and(|h| h == GFX_MAGIC || h == CS_MAGIC)
    };
    let adjacent = |c: usize| -> Option<usize> {
        let (nctl, n) = (w32(c + 0x20), w32(c + 0x704));
        let k = c.checked_sub(n)?;
        (n >= 0x80
            && nctl >= 0x760
            && c + nctl <= d.len()
            && headed(k)
            && memchr::memmem::find(&d[k..c], &CTL_MAGIC).is_none())
        .then_some(k)
    };
    let mut skip: Vec<(usize, usize)> = vec![];
    for m in memchr::memmem::find_iter(d, &glslc::MAGIC) {
        let (size, _, first, _) = glslc::layout(d, m);
        if size > (if first == 0xa8 { 0 } else { 0x760 }) && size <= d.len() - m {
            skip.push((m, m + size));
        }
    }
    for m in memchr::memmem::find_iter(d, b"BNSH\0\0\0\0") {
        skip.push((m, m + w32(m + 0x1c)));
    }
    let ctls: Vec<usize> = memchr::memmem::find_iter(d, &CTL_MAGIC)
        .filter(|&c| {
            c + 0x708 <= d.len()
                && !skip.iter().any(|&(a, b)| a <= c && c < b)
                && (c & 3 == 0 || adjacent(c).is_some())
        })
        .filter(|&c| !known.contains(d.get(c..c + w32(c + 0x20)).unwrap_or(&[])))
        .collect();
    if ctls.is_empty() {
        return vec![];
    }
    let rel = rel_targets(d);
    let cset: HashSet<usize> = ctls.iter().copied().collect();
    let mut code_of: HashMap<usize, usize> = HashMap::new();
    for (&s, &c) in &rel {
        if cset.contains(&c) && !code_of.contains_key(&c) && s >= 8 && w32(s + 8) == w32(c + 0x704)
        {
            if let Some(&k) = rel.get(&(s - 8)) {
                code_of.insert(c, k);
            }
        }
    }
    let built = if ctls.iter().any(|c| !code_of.contains_key(c)) {
        adrp_targets(
            d,
            ctls[0].saturating_sub(0x100000),
            ctls[ctls.len() - 1] + 0x100000,
        )
    } else {
        HashMap::new()
    };
    let targets: HashSet<usize> = rel.values().copied().collect();
    let mut out = vec![];
    for &c in &ctls {
        let (nctl, n) = (w32(c + 0x20), w32(c + 0x704));
        if c + nctl > d.len() {
            continue;
        }
        if !code_of.contains_key(&c) {
            if let Some(r) = c.checked_sub(n + 0x14) {
                if (targets.contains(&r) || built.contains_key(&r))
                    && shad_record(d, r, r + 0x14 + n + w32(r + 4)).is_some()
                {
                    code_of.insert(c, c - n);
                }
            }
        }
        let ctl = &d[c..c + nctl];
        let near = |k: usize| -> usize {
            let (Some(a), Some(b)) = (built.get(&k), built.get(&c)) else {
                return usize::MAX;
            };
            a.iter()
                .flat_map(|x| b.iter().map(move |y| x.abs_diff(*y)))
                .min()
                .unwrap_or(usize::MAX)
        };
        let cands: Vec<usize> = match code_of.get(&c) {
            Some(&k) => vec![k],
            None => {
                let mut v: Vec<usize> = [(c + nctl + 15) & !15, c.wrapping_sub(n)]
                    .into_iter()
                    .filter(|&k| k < d.len() && near(k) < 0x400)
                    .collect();
                v.sort_by_key(|&k| near(k));
                v
            }
        };
        let mut done = false;
        for k in cands {
            let Some(code) = d.get(k..k + n) else {
                continue;
            };
            if memchr::memmem::find(code, &CTL_MAGIC).is_some() {
                continue;
            }
            if let Some(st) = glslc::stage(ctl, code).filter(|&st| check(st, ctl, code).is_none()) {
                out.push(Found::new(format!("{label}#RAW@{c:#x}"), st, ctl, code));
                done = true;
                break;
            }
        }
        if !done && !code_of.contains_key(&c) {
            if let Some(k) = adjacent(c) {
                let code = &d[k..c];
                if let Some(st) =
                    glslc::stage(ctl, code).filter(|&st| check(st, ctl, code).is_none())
                {
                    out.push(Found::new(format!("{label}#PHYRE@{k:#x}"), st, ctl, code));
                    done = true;
                }
            }
        }
        if !done {
            problems.push(format!("{label} control section @{c:#x}: no code area (no record points at one, the code builds the address of none beside it, none of its length ends at it); left out"));
        }
    }
    out
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
    fn adjacent_and_adrp() {
        // a code area ending right at its control section (PhyreEngine's form), nothing referencing either
        let (c1, k1) = nvn1_ps(11);
        let c1 = with_len(c1);
        let mut d = vec![0u8; 0x100];
        d.extend_from_slice(&k1);
        let at1 = d.len();
        d.extend_from_slice(&c1);
        while d.len() % 16 != 0 {
            d.push(0);
        }
        // a control section whose code area follows it, both addresses built by adrp + add
        let (c2, k2) = nvn1_vs(12);
        let c2 = with_len(c2);
        let at2 = d.len();
        d.extend_from_slice(&c2);
        while d.len() % 16 != 0 {
            d.push(0);
        }
        let kat2 = d.len();
        d.extend_from_slice(&k2);
        while d.len() % 0x1000 != 0 {
            d.push(0);
        }
        // code: adrp x0, the target's page (relative to its own); add x0, x0, #the target's page offset
        for t in [at2, kat2] {
            let here = d.len();
            let imm = ((t as i64 >> 12) - (here as i64 >> 12)) as u32 & 0x1f_ffff;
            let adrp = 0x9000_0000 | ((imm & 3) << 29) | ((imm >> 2) << 5);
            let add = 0x9100_0000 | ((t as u32 & 0xfff) << 10);
            d.extend_from_slice(&adrp.to_le_bytes());
            d.extend_from_slice(&add.to_le_bytes());
        }
        let mut p = vec![];
        let f = raw_shaders(&d, "main", &HashSet::new(), &mut p);
        assert!(p.is_empty(), "{p:?}");
        let got: Vec<(String, String)> = f.iter().map(|f| (f.label.clone(), f.key())).collect();
        assert!(
            got.contains(&(format!("main#PHYRE@{:#x}", 0x100), skey(&c1, &k1))),
            "{got:?}"
        );
        assert!(
            got.contains(&(format!("main#RAW@{at2:#x}"), skey(&c2, &k2))),
            "{got:?}"
        );
        let _ = at1;
    }
}
