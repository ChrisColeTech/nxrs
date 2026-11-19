//! Function discovery, setjmp owners, C size estimates and chunk partitioning.

use crate::loader::nso::{R_AARCH64_GLOB_DAT, R_AARCH64_JUMP_SLOT};
use crate::loader::Module;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const EST_INT: i64 = 62;
pub const EST_SIMD: i64 = 75;
pub const EST_SIMD_LDST: i64 = 48;
pub const EST_FN: i64 = 38;
pub const EST_BR_LABELS: i64 = 10;

fn sext(v: u32, n: u32) -> i64 {
    let v = v as i64;
    if v & (1 << (n - 1)) != 0 { v - (1 << n) } else { v }
}

fn imm21(w: u32) -> i64 {
    sext((((w >> 5) & 0x7FFFF) << 2) | ((w >> 29) & 3), 21)
}

/// Discovery statistics (as in the manifest).
#[derive(Debug, Clone, Default)]
pub struct DiscoverStats {
    pub seeds: usize,
    pub bl_targets: usize,
    pub materialised_targets: usize,
    pub initial_starts: usize,
    pub rounds: usize,
    pub functions: usize,
}

/// Function starts of the module as sorted word indices into `m.words`, plus statistics.
///
/// Starts are the module seeds (.eh_frame_hdr, exported FUNC symbols, RELATIVE relocation targets), every BL
/// target and code addresses materialised by ADR or ADRP+ADD (minus ADRs anchoring a jump table); then a direct
/// branch leaving its function makes its target a start, until nothing new appears.
pub fn discover(m: &Module) -> (Vec<i64>, DiscoverStats) {
    let w = &m.words;
    let n = w.len() as i64;
    let seeds: Vec<i64> = m.seeds().iter().map(|&a| ((a - m.lo) >> 2) as i64).collect();

    let mut bl_tgt = vec![];
    let mut br_src = vec![];
    let mut br_tgt = vec![];
    // Python appends B targets first, then B.cond, CBZ/CBNZ, TBZ/TBNZ; order is irrelevant to the result.
    for (i, &x) in w.iter().enumerate() {
        let i = i as i64;
        if (x & 0x7C00_0000) == 0x1400_0000 {
            let t = i + sext(x & 0x03FF_FFFF, 26);
            if x >> 31 != 0 {
                bl_tgt.push(t);
            } else {
                br_src.push(i);
                br_tgt.push(t);
            }
        } else if (x & 0xFE00_0010) == 0x5400_0000 || (x & 0x7E00_0000) == 0x3400_0000 {
            br_src.push(i);
            br_tgt.push(i + sext((x >> 5) & 0x7FFFF, 19));
        } else if (x & 0x7E00_0000) == 0x3600_0000 {
            br_src.push(i);
            br_tgt.push(i + sext((x >> 5) & 0x3FFF, 14));
        }
    }
    let (br_src, br_tgt): (Vec<i64>, Vec<i64>) =
        br_src.into_iter().zip(br_tgt).filter(|&(_, t)| t >= 0 && t < n).unzip();

    let pc0 = m.lo as i64;
    let br: Vec<i64> = (0..n).filter(|&i| (w[i as usize] & 0xFFFF_FC1F) == 0xD61F_0000).collect();
    let mut mat = vec![];
    for i in 0..n {
        let x = w[i as usize];
        if (x & 0x9F00_0000) != 0x1000_0000 {
            continue;
        }
        // An ADR with a `br` in the next 16 instructions, or whose register is the base of an ADD (shifted or
        // extended register) in the next 4, anchors a jump table and is not a seed.
        let nxt = br.partition_point(|&b| b < i);
        let mut near = nxt < br.len() && br[nxt] - i <= 16;
        let rd = x & 31;
        for k in 1..5 {
            let y = w[(i + k).min(n - 1) as usize];
            near |= ((y & 0xFF20_0000) == 0x8B00_0000 || (y & 0xFFE0_0000) == 0x8B20_0000) && ((y >> 5) & 31) == rd;
        }
        if !near {
            mat.push(pc0 + i * 4 + imm21(x));
        }
    }
    let is_adrp = |i: i64| (w[i as usize] & 0x9F00_0000) == 0x9000_0000;
    let writes = |i: i64| {
        let x = w[i as usize];
        let dp = (x & 0x1C00_0000) == 0x1000_0000 || (x & 0x0E00_0000) == 0x0A00_0000;
        let load = (x & 0x0A00_0000) == 0x0800_0000 && (x >> 22) & 1 == 1;
        (dp || load) && !is_adrp(i)
    };
    for a in 0..n {
        let x = w[a as usize];
        if (x & 0xFFC0_0000) != 0x9100_0000 {
            continue;
        }
        let rn = (x >> 5) & 31;
        let mut found = -1i64;
        for k in 1..9 {
            let p = a - k;
            if p < 0 {
                break;
            }
            let same = (w[p as usize] & 31) == rn;
            if is_adrp(p) && same {
                found = p;
                break;
            }
            if writes(p) && same {
                break;
            }
        }
        if found >= 0 {
            let page = ((pc0 + found * 4) & !0xFFF) + (imm21(w[found as usize]) << 12);
            mat.push(page + ((x >> 10) & 0xFFF) as i64);
        }
    }
    let (lo, hi) = (m.lo as i64, m.hi as i64);
    let mat: Vec<i64> = mat.into_iter().filter(|&a| a >= lo && a < hi && a % 4 == 0).map(|a| (a - pc0) >> 2).collect();
    let bl_tgt: Vec<i64> = bl_tgt.into_iter().filter(|&t| t >= 0 && t < n).collect();

    let mut starts: BTreeSet<i64> = seeds.iter().copied().collect();
    starts.extend(bl_tgt.iter().copied());
    starts.extend(mat.iter().copied());
    let mut stats = DiscoverStats {
        seeds: seeds.len(),
        bl_targets: bl_tgt.iter().collect::<BTreeSet<_>>().len(),
        materialised_targets: mat.iter().collect::<BTreeSet<_>>().len(),
        initial_starts: starts.len(),
        ..Default::default()
    };
    let mut sorted: Vec<i64> = starts.iter().copied().collect();
    let mut rounds = 0;
    loop {
        rounds += 1;
        let mut new = BTreeSet::new();
        for (&s, &t) in br_src.iter().zip(&br_tgt) {
            let f = sorted.partition_point(|&x| x <= s);
            if f == 0 {
                continue;
            }
            let lo = sorted[f - 1];
            let hi = if f < sorted.len() { sorted[f] } else { n };
            if (t < lo || t >= hi) && !starts.contains(&t) {
                new.insert(t);
            }
        }
        if new.is_empty() {
            break;
        }
        starts.extend(new);
        sorted = starts.iter().copied().collect();
    }
    stats.rounds = rounds;
    stats.functions = sorted.len();
    (sorted, stats)
}

const SETJMP_NAMES: [&str; 4] = ["setjmp", "_setjmp", "__setjmp", "sigsetjmp"];

/// Function starts (absolute) that call setjmp, through a PLT stub of an imported setjmp or directly an exported one.
pub fn setjmp_owners(m: &Module, starts: &[u64]) -> BTreeSet<u64> {
    let n = &m.nso;
    let mut names: HashMap<u64, &str> = HashMap::new();
    for r in &n.relocs {
        if (r.typ == R_AARCH64_JUMP_SLOT || r.typ == R_AARCH64_GLOB_DAT) && r.sym != 0 {
            if let Some(s) = n.syms.get(r.sym as usize) {
                names.insert(m.base.wrapping_add(r.off), s.name.as_str());
            }
        }
    }
    let w = &m.words;
    let pc = |i: usize| m.lo as i64 + 4 * i as i64;
    let mut targets: BTreeSet<i64> = BTreeSet::new();
    for i in 0..w.len().saturating_sub(3) {
        if (w[i] & 0x9F00_001F) == 0x9000_0010
            && (w[i + 1] & 0xFFC0_03FF) == 0xF940_0211
            && (w[i + 2] & 0xFFC0_03FF) == 0x9100_0210
            && w[i + 3] == 0xD61F_0220
        {
            let page = (pc(i) & !0xFFF) + (imm21(w[i]) << 12);
            let got = page + ((((w[i + 1] >> 10) & 0xFFF) as i64) << 3);
            if names.get(&(got as u64)).is_some_and(|s| SETJMP_NAMES.contains(s)) {
                targets.insert(pc(i));
            }
        }
    }
    for s in &n.syms {
        if s.shndx != 0 && (s.info & 0xF) == 2 && SETJMP_NAMES.contains(&s.name.as_str()) {
            targets.insert(m.base.wrapping_add(s.value) as i64);
        }
    }
    let mut out = BTreeSet::new();
    if targets.is_empty() {
        return out;
    }
    for (i, &x) in w.iter().enumerate() {
        if (x & 0xFC00_0000) == 0x9400_0000 {
            let site = pc(i);
            if targets.contains(&(site + 4 * sext(x & 0x03FF_FFFF, 26))) {
                let k = starts.partition_point(|&s| s as i64 <= site);
                if k > 0 {
                    out.insert(starts[k - 1]);
                }
            }
        }
    }
    out
}

/// Estimated bytes of C per function (for chunking only).
pub fn estimate_sizes(m: &Module, starts: &[i64]) -> Vec<i64> {
    let w = &m.words;
    let mut csum = vec![0i64; w.len() + 1];
    let mut bsum = vec![0i64; w.len() + 1];
    for (i, &x) in w.iter().enumerate() {
        let mut cost = EST_INT;
        if (x & 0x0E00_0000) == 0x0E00_0000 {
            cost = EST_SIMD;
        }
        if (x & 0x0A00_0000) == 0x0800_0000 && (x >> 26) & 1 == 1 {
            cost = EST_SIMD_LDST;
        }
        csum[i + 1] = csum[i] + cost;
        bsum[i + 1] = bsum[i] + ((x & 0xFFFF_FC1F) == 0xD61F_0000) as i64;
    }
    (0..starts.len())
        .map(|k| {
            let s = starts[k] as usize;
            let e = if k + 1 < starts.len() { starts[k + 1] as usize } else { w.len() };
            let mut size = csum[e] - csum[s] + EST_FN;
            if bsum[e] - bsum[s] > 0 {
                size += (e - s) as i64 * EST_BR_LABELS;
            }
            size
        })
        .collect()
}

/// Contiguous [i0, i1) function ranges: at most max_fns functions and about max_bytes of C each; a function
/// estimated above big_fn gets a file of its own.
pub fn partition(est: &[i64], max_fns: usize, max_bytes: i64, big_fn: i64) -> Vec<(usize, usize)> {
    let mut chunks = vec![];
    let (mut i0, mut size) = (0usize, 0i64);
    for (i, &s) in est.iter().enumerate() {
        if s > big_fn {
            if i > i0 {
                chunks.push((i0, i));
            }
            chunks.push((i, i + 1));
            i0 = i + 1;
            size = 0;
            continue;
        }
        if i > i0 && (i - i0 >= max_fns || size + s > max_bytes) {
            chunks.push((i0, i));
            i0 = i;
            size = 0;
        }
        size += s;
    }
    if est.len() > i0 {
        chunks.push((i0, est.len()));
    }
    chunks
}

/// Unused helper kept for symmetry with the Python statistics.
pub fn count_by<T: Ord + Clone>(v: &[T]) -> BTreeMap<T, usize> {
    let mut m = BTreeMap::new();
    for x in v {
        *m.entry(x.clone()).or_default() += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_splits_big_and_full() {
        assert_eq!(partition(&[10, 10, 500, 10], 2, 100, 300), vec![(0, 2), (2, 3), (3, 4)]);
        assert_eq!(partition(&[60, 60, 60], 10, 100, 300), vec![(0, 1), (1, 2), (2, 3)]);
    }
}
