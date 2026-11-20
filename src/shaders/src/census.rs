//! Census of the Maxwell texture instructions whose LOD clamp (`.LC`) or residency predicate the translator drops
//! (nxnative `dm/gfx/tools/tex_census.py`; Ryujinx.Graphics.Shader `InstEmitTexture.cs` decodes `Lc` and `DestPred`
//! and passes neither on). A game that clamps the LOD of a streamed sparse texture samples levels it has mapped but
//! not filled once the clamp is gone (Densha de GO!!: ASTC's error colour, nxnative issue #2350); the census says
//! which games and shaders have them. Not part of the translation: it decides no output.
//!
//! A graphics stage's instructions follow the code area's 0x30-byte header and its 0x50-byte SPH (+0x80); a compute
//! shader's start at +0x100 behind its 0x12345679 header, else at +0. Every fourth 64-bit word from the start is a
//! scheduling control word; a program ends at its branch to itself, or where its c1 constant data starts (control
//! +0x700). NVN2 (Ampere) shaders are skipped: their translator refuses a residency predicate and unknown LOD modes.
//! Opcode patterns and field bits are the translator's (`Decoders/InstTable.cs`, `InstDecoders.cs`).

use crate::nvn::{nvn2_ctl, u32_at, u64_at, Stage, CS_MAGIC};

const SELF_BRANCH: u64 = 0xE240_0FFF_FF87_000F;

/// (name, mask, value, the LOD clamp bit (None: the form has none), has a residency predicate)
type Op = (&'static str, u64, u64, Option<u32>, bool);

fn pattern(bits: &str) -> (u64, u64) {
    let (mut m, mut v) = (0u64, 0u64);
    for (i, c) in bits.chars().enumerate() {
        let b = 63 - i;
        if c == '0' || c == '1' {
            m |= 1 << b;
            v |= ((c == '1') as u64) << b;
        }
    }
    (m, v)
}

fn ops() -> Vec<Op> {
    let x = |n: usize| "x".repeat(n);
    let mut v: Vec<Op> = [
        ("TEX", format!("11000{}", x(59)), Some(58), true),
        ("TEX.B", format!("1101111010{}", x(54)), Some(40), true),
        ("TLD", format!("11011100{}", x(56)), None, true),
        ("TLD.B", format!("11011101{}", x(56)), None, true),
        ("TLD4", format!("110010{}", x(58)), Some(58), true),
        ("TLD4.B", format!("1101111011{}0{}", x(13), x(40)), Some(40), true),
        ("TXD", format!("110111100{}", x(55)), Some(50), true),
        ("TXD.B", format!("1101111001{}", x(54)), Some(50), true),
        ("TEXS", format!("1101100{}", x(57)), None, false),
        ("TLDS", format!("1101101{}", x(57)), None, false),
        ("TLD4S", format!("1101111100{}", x(54)), None, false),
    ]
    .into_iter()
    .map(|(n, p, lc, tpd)| {
        let (m, val) = pattern(&p);
        (n, m, val, lc, tpd)
    })
    .collect();
    // the longer patterns first, so TEX.B / TLD4.B / TXD.B / TLD4S are not taken for a shorter one
    v.sort_by_key(|o| std::cmp::Reverse(o.1.count_ones()));
    v
}

/// One texture instruction: (op, LOD clamp set, residency predicate set, byte offset from the program start).
pub type Found = (&'static str, bool, bool, usize);

/// The texture instructions of one Maxwell shader (None for NVN2).
pub fn census(stage: Stage, ctl: &[u8], code: &[u8]) -> Option<Vec<Found>> {
    if nvn2_ctl(ctl) {
        return None;
    }
    let start = if stage == Stage::Cs {
        if code.get(..4) == Some(&CS_MAGIC[..]) {
            0x100
        } else {
            0
        }
    } else {
        0x80
    };
    let c1 = u32_at(ctl, 0x700).unwrap_or(0) as usize;
    let end = if c1 > 0 && c1 <= code.len() { c1 } else { code.len() };
    let ops = ops();
    let mut out = vec![];
    let mut i = start;
    while i + 8 <= end {
        if ((i - start) / 8) % 4 != 0 {
            let w = u64_at(code, i).unwrap();
            if w == SELF_BRANCH {
                break;
            }
            if let Some(&(name, _, _, lc, tpd)) = ops.iter().find(|o| w & o.1 == o.2) {
                let clamp = lc.is_some_and(|b| (w >> b) & 1 != 0);
                let resid = tpd && (w >> 51) & 7 != 7;
                out.push((name, clamp, resid, i - start));
            }
        }
        i += 8;
    }
    Some(out)
}

/// The census of an `nxshaders export` folder (`index.tsv` + `<key>.ctl` / `<key>.code`), as a report.
pub fn census_dir(dir: &std::path::Path, list: usize) -> Result<String, String> {
    use std::collections::BTreeMap;
    let idx = std::fs::read_to_string(dir.join("index.tsv")).map_err(|e| format!("{}: {e}", dir.join("index.tsv").display()))?;
    let (mut shaders, mut nvn2, mut with_tex) = (0usize, 0usize, 0usize);
    let mut ops: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    let (mut lc_keys, mut rp_keys) = (vec![], vec![]);
    for l in idx.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        let (Some(key), Some(st)) = (f.first(), f.get(1).and_then(|s| Stage::parse(s))) else { continue };
        let (Ok(ctl), Ok(code)) = (std::fs::read(dir.join(format!("{key}.ctl"))), std::fs::read(dir.join(format!("{key}.code")))) else { continue };
        let Some(found) = census(st, &ctl, &code) else {
            nvn2 += 1;
            continue;
        };
        shaders += 1;
        with_tex += !found.is_empty() as usize;
        for &(n, c, r, _) in &found {
            let e = ops.entry(n).or_default();
            e.0 += 1;
            e.1 += c as usize;
            e.2 += r as usize;
        }
        let (c, r) = (found.iter().filter(|x| x.1).count(), found.iter().filter(|x| x.2).count());
        if c > 0 {
            lc_keys.push(format!("{key} ({st:?}, {c} instructions)"));
        }
        if r > 0 {
            rp_keys.push(format!("{key} ({st:?}, {r} instructions)"));
        }
    }
    let mut s = format!("Maxwell shaders: {shaders}; NVN2 skipped: {nvn2}; with a texture instruction: {with_tex}\nop       count    with .LC residency\n");
    for (n, (k, c, r)) in &ops {
        s += &format!("{n:8} {k:9} {c:9} {r:9}\n");
    }
    s += &format!("shaders with a LOD clamp: {}; with a residency predicate: {}\n", lc_keys.len(), rp_keys.len());
    for k in lc_keys.iter().take(list) {
        s += &format!("  LOD clamp: {k}\n");
    }
    for k in rp_keys.iter().take(list) {
        s += &format!("  residency predicate: {k}\n");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TEX with .LC and a residency predicate, TLD4S (no clamp, no predicate), a scheduling word that looks like TEX
    /// (skipped), then the branch to itself; a TEX after it is not counted.
    #[test]
    fn texture_instructions_with_lod_clamp_and_residency() {
        let (mut ctl, _) = crate::nvn::synth::nvn1_ps(1);
        ctl[0x700..0x704].copy_from_slice(&0u32.to_le_bytes());
        let tex = |lc: bool, pred: u64| -> u64 { (0b11000u64 << 59) | ((lc as u64) << 58) | (pred << 51) };
        let tld4s = 0b1101111100u64 << 54;
        let words = [
            tex(true, 7), // a scheduling word slot: skipped whatever it holds
            tex(true, 2), // TEX.LC, predicate P2
            tld4s,
            tex(false, 7),
            0, // scheduling
            SELF_BRANCH,
            tex(true, 1),
            0,
        ];
        let mut code = vec![0u8; 0x80];
        for w in words {
            code.extend_from_slice(&w.to_le_bytes());
        }
        let f = census(Stage::Ps, &ctl, &code).unwrap();
        assert_eq!(f, vec![("TEX", true, true, 8), ("TLD4S", false, false, 16), ("TEX", false, false, 24)]);
        // NVN2 is skipped
        let (c2, k2) = crate::nvn::synth::nvn2(Stage::Ps, 1);
        assert!(census(Stage::Ps, &c2, &k2).is_none());
    }
}
